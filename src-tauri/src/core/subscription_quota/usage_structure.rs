//! A complete settled usage profile is independent of a capacity denominator.
//! Its private seal permits freezing only the exact profile Core just checked.
use super::*;

const STRUCTURE_VERSION: &str = "settled-usage-profile-v1";

#[derive(Clone, Debug)]
pub(crate) struct Seal {
    receipt_id: i64,
    digest: String,
}
impl Seal {
    pub(super) fn matches(&self, receipt_id: i64, structure: &Value) -> bool {
        self.receipt_id == receipt_id && self.digest == canonical_digest(structure)
    }
}

/// Uses the same independent billing/coverage/ledger contracts as calibration,
/// but makes no pair, holdout assignment, capacity or precision assertion.
pub(super) fn at_endpoint(
    db: &Connection,
    batch: &EvidenceBatch<'_>,
    start: &CommittedQuotaObservation,
    end: &CommittedQuotaObservation,
    settlement: &LedgerSettlement,
    now: f64,
) -> Option<(Value, Seal)> {
    continuous(db, start, end).ok()?;
    let a = start.sample.fact_time?.lower;
    let b = end.sample.fact_time?.upper;
    if a >= b
        || b > now
        || b >= end.sample.reset_time?.lower
        || start.sample.percent.saturated
        || end.sample.percent.saturated
        || settlement.billing_clock_epoch != end.sample.clock_epoch
    {
        return None;
    }
    let domain = Domain::from_scope(&end.sample.scope)?;
    let covers: Vec<_> = batch
        .coverage
        .iter()
        .filter(|proof| proof.domain == domain && contains(proof.interval, a, b))
        .collect();
    if covers
        .iter()
        .any(|proof| matches!(proof.kind, CoverageKind::KnownIncomplete))
    {
        return None;
    }
    let mut verified: Vec<_> = covers
        .iter()
        .filter_map(|proof| match &proof.kind {
            CoverageKind::VerifiedComplete {
                adapter_id,
                reference,
            } if valid_text(adapter_id) && valid_text(reference) => Some((adapter_id, reference)),
            _ => None,
        })
        .collect();
    verified.sort();
    let (coverage_adapter, coverage_reference) = *verified.first()?;
    let modes: Vec<_> = batch
        .modes
        .iter()
        .filter(|mode| mode.domain == domain && contains(mode.interval, a, b))
        .collect();
    let mode = *modes.first()?;
    if modes
        .iter()
        .any(|other| other.effort != mode.effort || other.speed != mode.speed)
    {
        return None;
    }
    let rows = ledger_rows(db, batch.usage, &domain, a, b, settlement, now).ok()?;
    let (_, profile) = usage_profile(&rows, mode).ok()?;
    if [&profile.effort, &profile.speed_mode]
        .into_iter()
        .any(|value| {
            value
                .as_deref()
                .is_none_or(|value| !valid_text(value) || value == "unknown")
        })
    {
        return None;
    }
    let model = (profile.models.len() == 1 && profile.all_models_attributed)
        .then(|| profile.models[0].model.as_str());
    let structure = json!({"verified":true,"source":"trusted-ledger",
        "reference":response_ledger_digest(&rows),"sourceId":end.sample.scope.source_id,
        "accountScope":end.sample.scope.account_scope,"windowId":end.sample.scope.pool_id,
        "windowInstanceId":end.sample.scope.window_instance_id,"modelDimension":"response",
        "model":model,"profile":profile,"ledgerRevision":settlement.revision.to_string(),
        "fromSequence":start.sample.sequence.to_string(),"observationSequence":end.sample.sequence.to_string(),
        "ledgerInterval":{"lower":a,"upper":b},"nativeDigest":end.proof.native_digest,
        "basis":{"version":STRUCTURE_VERSION,"coverage":"verified_complete",
            "coverageAdapter":coverage_adapter,"coverageReference":coverage_reference,
            "modeAdapter":mode.adapter_id,"modeReference":mode.reference,
            "effort":profile.effort,"speedMode":profile.speed_mode,
            "meteringVersion":settlement.metering_version,"clockEpoch":settlement.billing_clock_epoch,
            "alignmentReference":settlement.billing_alignment_reference,
            "sourceCheckDigest":settlement.source_check_digest,"settledThrough":settlement.complete_interval.upper}});
    let seal = Seal {
        receipt_id: end.receipt_id,
        digest: canonical_digest(&structure),
    };
    Some((structure, seal))
}

pub(super) fn valid_basis(structure: &Value, clock_epoch: &Value) -> bool {
    let Some(structure_fields) = structure.as_object() else {
        return false;
    };
    if structure_fields.len() != 16
        || structure_fields.keys().any(|key| {
            !matches!(
                key.as_str(),
                "verified"
                    | "source"
                    | "reference"
                    | "sourceId"
                    | "accountScope"
                    | "windowId"
                    | "windowInstanceId"
                    | "modelDimension"
                    | "model"
                    | "profile"
                    | "ledgerRevision"
                    | "fromSequence"
                    | "observationSequence"
                    | "ledgerInterval"
                    | "nativeDigest"
                    | "basis"
            )
        })
    {
        return false;
    }
    let basis = &structure["basis"];
    let Some(fields) = basis.as_object() else {
        return false;
    };
    if fields.len() != 13
        || fields.keys().any(|key| {
            !matches!(
                key.as_str(),
                "version"
                    | "coverage"
                    | "coverageAdapter"
                    | "coverageReference"
                    | "modeAdapter"
                    | "modeReference"
                    | "effort"
                    | "speedMode"
                    | "meteringVersion"
                    | "clockEpoch"
                    | "alignmentReference"
                    | "sourceCheckDigest"
                    | "settledThrough"
            )
        })
    {
        return false;
    }
    basis["version"] == STRUCTURE_VERSION
        && basis["coverage"] == "verified_complete"
        && basis["clockEpoch"] == *clock_epoch
        && [
            "coverageAdapter",
            "coverageReference",
            "modeAdapter",
            "modeReference",
            "meteringVersion",
            "clockEpoch",
            "alignmentReference",
            "sourceCheckDigest",
        ]
        .into_iter()
        .all(|key| text(&basis[key]).is_some())
        && ["effort", "speedMode"].into_iter().all(|key| {
            text(&basis[key]).is_some_and(|v| v != "unknown")
                && basis[key] == structure["profile"][key]
        })
        && instant(&basis["settledThrough"])
            .zip(structure["ledgerInterval"]["upper"].as_f64())
            .is_some_and(|(through, end)| through >= end)
}
