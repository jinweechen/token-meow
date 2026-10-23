//! Core-owned assembly of passive calibration evidence. Provider attestations
//! intentionally do not implement Deserialize: an arbitrary JSON `verified`
//! flag or a user's capacity assumption cannot become an adapter proof.
use super::super::{
    adapter::{self, ResponseUsage},
    err, hash, Result,
};
use super::projector::response_ledger_digest;
use super::{calibration::profiles_compatible, evaluate::percentage_bounds, model::*};
use crate::{account_bridge::AccountSnapshot, budget_clock::Reading};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

#[path = "observation_proof.rs"]
pub(super) mod observation_proof;
#[path = "usage_structure.rs"]
mod usage_structure;

const VERSION: &str = "calibration-assembly-v1";
const POLICY_VERSION: &str = "first-20-train-three-periods-then-holdout-v1";
const PREFIX: &str = "subscriptionHoldout:";
const MAX_OBSERVATIONS: usize = 1024;
const MAX_USAGE: usize = 100_000;
const MAX_PAIR_ROWS: usize = 10_000;
const MAX_PAIRS: usize = 64;
const MAX_BYTES: usize = 256 * 1024;
const MAX_POLICIES: usize = 100;
const MAX_SAFE: u64 = 9_007_199_254_740_991;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Domain {
    pub source_id: String,
    pub account_scope: String,
    pub entitlement_segment: String,
    pub rule_version: String,
    pub pool_id: String,
    pub clear_generation: String,
}
impl Domain {
    pub(crate) fn from_scope(scope: &EvaluationScope) -> Option<Self> {
        Some(Self {
            source_id: scope.source_id.clone(),
            account_scope: scope.account_scope.clone()?,
            entitlement_segment: scope.entitlement_segment.clone(),
            rule_version: scope.rule_version.clone(),
            pool_id: scope.pool_id.clone(),
            clear_generation: scope.clear_generation.clone(),
        })
    }
    fn valid(&self) -> bool {
        [
            &self.source_id,
            &self.account_scope,
            &self.entitlement_segment,
            &self.rule_version,
            &self.pool_id,
            &self.clear_generation,
        ]
        .into_iter()
        .all(|v| valid_text(v))
    }
}

/// These values must be supplied by a trusted adapter implementation with a
/// real protocol/source contract. The current Codex bridge supplies none.
#[derive(Clone, Debug)]
pub(crate) struct AdapterQuotaProof {
    pub adapter_id: String,
    pub reference: String,
    pub native_digest: String,
    pub scope: EvaluationScope,
    pub sequence: u64,
    pub percent: PercentReading,
    pub fact_time: Interval,
    pub ttl_start: f64,
    pub clock_epoch: String,
    pub reset_time: Interval,
    pub recovery: RecoverySemantics,
    pub stable_identity_reference: String,
    pub entitlement_reference: String,
    pub rule_reference: String,
    pub period_reference: String,
    pub period_stable_across_runtimes: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct CommittedQuotaObservation {
    receipt_id: i64,
    sample: NativeSample,
    proof: AdapterQuotaProof,
    independent_training_sample: bool,
    derived_metadata_present: bool,
}
impl CommittedQuotaObservation {
    pub(crate) fn sample(&self) -> &NativeSample {
        &self.sample
    }
}

/// Collection completeness and event-time/billing alignment are separate.
/// A complete local scan alone never establishes the latter or pool coverage.
#[derive(Clone, Debug)]
pub(crate) struct LedgerSettlement {
    pub domain: Domain,
    pub revision: u64,
    pub complete_interval: Interval,
    pub captured_at: f64,
    pub source_check_digest: String,
    pub metering_version: String,
    pub billing_clock_epoch: String,
    pub billing_alignment_reference: String,
}
#[derive(Clone, Debug)]
pub(crate) enum CoverageKind {
    VerifiedComplete {
        adapter_id: String,
        reference: String,
    },
    FutureUserAssumption {
        declared_at: f64,
        reference: String,
    },
    KnownIncomplete,
}
#[derive(Clone, Debug)]
pub(crate) struct CoverageAttestation {
    pub domain: Domain,
    pub interval: Interval,
    pub kind: CoverageKind,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ExplicitMode {
    Known(String),
    Unknown,
}
impl ExplicitMode {
    fn value(&self) -> Option<String> {
        match self {
            Self::Known(v) => Some(v.clone()),
            Self::Unknown => None,
        }
    }
}
#[derive(Clone, Debug)]
pub(crate) struct UsageModeAttestation {
    pub domain: Domain,
    pub interval: Interval,
    pub effort: ExplicitMode,
    pub speed: ExplicitMode,
    pub adapter_id: String,
    pub reference: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Assignment {
    pair_id: String,
    start_receipt: i64,
    end_receipt: i64,
    start: f64,
    end: f64,
    period: String,
    holdout: bool,
    ledger_digest: String,
    input_digest: String,
    evidence_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FrozenHoldoutPolicy {
    pub id: String,
    pub schema_version: u32,
    pub policy_version: String,
    pub source_id: String,
    pub account_scope: String,
    pub domain: Domain,
    pub scope: EvaluationScope,
    pub anchor_profile: UsageProfile,
    pub created_at: String,
    pub created_mono_ms: String,
    assignments: Vec<Assignment>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ObservationPatch {
    pub receipt_id: i64,
    pub observation_id: String,
    pub window_id: String,
    pub snapshot_fields: Value,
    pub window_fields: Value,
    #[serde(skip)]
    pub(super) structure_seal: Option<usage_structure::Seal>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Rejection {
    pub observation_id: Option<String>,
    pub reason: Reason,
    pub detail: String,
}
#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Assembly {
    pub patches: Vec<ObservationPatch>,
    pub pairs: Vec<CalibrationPair>,
    pub missing: Vec<Rejection>,
    pub manifests: Vec<FrozenHoldoutPolicy>,
}
pub(crate) struct EvidenceBatch<'a> {
    pub observations: &'a [CommittedQuotaObservation],
    pub usage: &'a [ResponseUsage],
    pub settlement: Option<&'a LedgerSettlement>,
    pub coverage: &'a [CoverageAttestation],
    pub modes: &'a [UsageModeAttestation],
}

fn valid_text(v: &str) -> bool {
    !v.trim().is_empty() && v.len() <= 2000 && !v.chars().any(char::is_control)
}
fn text(v: &Value) -> Option<&str> {
    v.as_str().filter(|v| valid_text(v))
}
fn decimal(v: &Value) -> Option<u64> {
    let s = v.as_str()?;
    let n = s.parse::<u64>().ok()?;
    (n.to_string() == s).then_some(n)
}
fn instant(v: &Value) -> Option<f64> {
    if let Some(n) = v.as_f64() {
        return (n.is_finite() && n >= 0.0).then_some(n);
    }
    let t = DateTime::parse_from_rfc3339(v.as_str()?).ok()?;
    let n = t.timestamp() as f64 + t.timestamp_subsec_nanos() as f64 / 1e9;
    (n >= 0.0 && n.is_finite()).then_some(n)
}
fn seconds(v: DateTime<Utc>) -> f64 {
    v.timestamp() as f64 + v.timestamp_subsec_nanos() as f64 / 1e9
}
fn timestamp(n: f64) -> Option<String> {
    if !n.is_finite() || n < 0.0 || n > 253_402_300_799.0 {
        return None;
    }
    DateTime::<Utc>::from_timestamp(
        n.floor() as i64,
        ((n.fract() * 1e9).round() as u32).min(999_999_999),
    )
    .map(|v| v.to_rfc3339())
}
fn canonical_digest(v: &Value) -> String {
    let mut v = v.clone();
    v.sort_all_objects();
    hash(v.to_string().as_bytes())
}
fn reject(reason: Reason, detail: &str) -> Rejection {
    Rejection {
        observation_id: None,
        reason,
        detail: detail.into(),
    }
}
fn setting(db: &Connection, key: &str) -> Result<Option<Value>> {
    let value: Option<String> = db
        .query_row("SELECT value FROM settings WHERE key=?1", [key], |r| {
            r.get(0)
        })
        .optional()
        .map_err(err)?;
    value
        .map(|v| serde_json::from_str(&v).map_err(err))
        .transpose()
}
fn native_value(snapshot: &Value) -> Value {
    let read = |name: &str| {
        let r = &snapshot[name];
        json!({"requestStartedAt":r["requestStartedAt"],"responseReceivedAt":r["responseReceivedAt"],
        "freshnessAnchor":r["freshnessAnchor"],"factTimeLower":r["factTimeLower"],"factTimeUpper":r["factTimeUpper"],"timeBasis":r["timeBasis"],"outcome":r["outcome"]})
    };
    let buckets:Vec<_>=snapshot["buckets"].as_array().into_iter().flatten().map(|b| {
        let window=|kind:&str| {let w=&b[kind];if w.is_object(){json!({"usedPercent":w["usedPercent"],"windowDurationMins":w["windowDurationMins"],"resetsAt":w["resetsAt"]})}else{Value::Null}};
        json!({"id":b["id"],"name":b["name"],"primary":window("primary"),"secondary":window("secondary")})
    }).collect();
    json!({"status":snapshot["status"],"collectedAt":snapshot["collectedAt"],"authMode":snapshot["authMode"],"accountScope":snapshot["accountScope"],
        "planType":snapshot["planType"],"identityEpoch":snapshot["identityEpoch"],"identityConsistency":snapshot["identityConsistency"],
        "identityConsistencyReason":snapshot["identityConsistencyReason"],"observationSequence":snapshot["observationSequence"],
        "observationOutcome":snapshot["observationOutcome"],"readStartedAt":snapshot["readStartedAt"],"readFinishedAt":snapshot["readFinishedAt"],
        "accountRead":read("accountRead"),"quotaRead":read("quotaRead"),"buckets":buckets})
}
pub(crate) fn native_observation_digest(snapshot: &AccountSnapshot) -> String {
    canonical_digest(&native_value(
        &serde_json::to_value(snapshot).expect("finite typed account snapshot"),
    ))
}
pub(crate) fn source_check_digest(value: &Value) -> String {
    canonical_digest(value)
}
fn raw_window<'a>(snapshot: &'a Value, id: &str) -> Option<&'a Value> {
    for bucket in snapshot["buckets"].as_array()? {
        for kind in ["primary", "secondary"] {
            if format!("{}:{kind}", bucket["id"].as_str()?) == id {
                return bucket.get(kind).filter(|v| v.is_object());
            }
        }
    }
    None
}

/// A committed envelope and an independently typed adapter proof must agree.
/// Metadata such as `identityEvidence: {verified:true}` inside JSON is ignored.
pub(crate) fn load_observation(
    db: &Connection,
    receipt_id: i64,
    proof: Option<&AdapterQuotaProof>,
) -> std::result::Result<CommittedQuotaObservation, Rejection> {
    let Some(proof) = proof else {
        return Err(reject(
            Reason::IdentityUnproven,
            "adapter_quota_proof_missing",
        ));
    };
    let raw: String = db
        .query_row(
            "SELECT data FROM account_snapshots WHERE id=?1 AND length(data)<=?2",
            params![receipt_id, MAX_BYTES as i64],
            |r| r.get(0),
        )
        .map_err(|_| {
            reject(
                Reason::NotPersisted,
                "committed_receipt_missing_or_oversize",
            )
        })?;
    let stored: Value =
        serde_json::from_str(&raw).map_err(|_| reject(Reason::NotPersisted, "receipt_invalid"))?;
    let receipt = super::observation_storage::resolve(db, &stored)
        .map_err(|_| reject(Reason::NotPersisted, "receipt_storage_unavailable"))?;
    if receipt["storageEvidence"]["available"] == false {
        return Err(reject(
            Reason::EvidenceInvalidated,
            "receipt_storage_evidence_removed",
        ));
    }
    let snapshot = &receipt["observation"];
    if receipt_id <= 0
        || receipt["documentKind"] != "subscription_observation"
        || receipt["schemaVersion"] != 1
        || receipt["outcome"] != "success"
        || decimal(&receipt["observationSequence"]) != Some(proof.sequence)
        || decimal(&receipt["lastPersistedSequence"]) != Some(proof.sequence)
        || proof.sequence == 0
        || !snapshot.is_object()
    {
        return Err(reject(Reason::NotPersisted, "receipt_not_admitted"));
    }
    for key in [
        "runtimeId",
        "sourceId",
        "bindingGeneration",
        "identityEpoch",
        "clearGeneration",
        "observationSequence",
    ] {
        if snapshot.get(key) != receipt.get(key) {
            return Err(reject(Reason::ScopeChanged, "receipt_scope_mismatch"));
        }
    }
    let s = &proof.scope;
    if !s.is_valid()
        || s.identity_epoch == 0
        || s.account_scope.as_deref().is_none_or(|v| !valid_text(v))
        || text(&snapshot["runtimeId"]) != Some(s.runtime_id.as_str())
        || text(&snapshot["sourceId"]) != Some(s.source_id.as_str())
        || decimal(&snapshot["bindingGeneration"]) != Some(s.binding_generation)
        || decimal(&snapshot["identityEpoch"]) != Some(s.identity_epoch)
        || snapshot["clearGeneration"].to_string() != s.clear_generation
        || text(&snapshot["accountScope"]) != s.account_scope.as_deref()
        || snapshot["identityConsistency"] != "proven"
        || snapshot["status"] != "connected"
        || snapshot["observationOutcome"] != "success"
    {
        return Err(reject(
            Reason::IdentityUnproven,
            "stable_adapter_identity_missing",
        ));
    }
    if canonical_digest(&native_value(snapshot)) != proof.native_digest {
        return Err(reject(
            Reason::EvidenceInvalidated,
            "native_observation_changed",
        ));
    }
    if [
        &proof.adapter_id,
        &proof.reference,
        &proof.stable_identity_reference,
        &proof.entitlement_reference,
        &proof.rule_reference,
        &proof.period_reference,
        &proof.clock_epoch,
    ]
    .into_iter()
    .any(|v| !valid_text(v))
    {
        return Err(reject(Reason::ScopeChanged, "adapter_provenance_missing"));
    }
    let w = raw_window(snapshot, &s.pool_id)
        .ok_or_else(|| reject(Reason::InvalidScope, "window_missing"))?;
    if w["usedPercent"].as_f64() != Some(proof.percent.used_percent)
        || w["resetsAt"].as_i64().map(|v| v as f64) != Some(proof.reset_time.lower)
        || !proof.reset_time.is_valid()
        || proof.reset_time.lower != proof.reset_time.upper
        || !proof.recovery.supports_single_reset_projection()
    {
        return Err(reject(
            Reason::RecoveryUnknown,
            "official_window_proof_missing",
        ));
    }
    // A trusted unknown quantum remains unknown. It can participate only in
    // the independent empirical-signal branch, never bounded quantization math.
    if !proof.percent.used_percent.is_finite()
        || !(0.0..=100.0).contains(&proof.percent.used_percent)
        || (proof.percent.saturated && proof.percent.used_percent != 100.0)
    {
        return Err(reject(Reason::InvalidNumber, "rounding_proof_invalid"));
    }
    if proof.percent.rounding_step.is_some() {
        percentage_bounds(&proof.percent)
            .map_err(|reason| reject(reason, "rounding_proof_invalid"))?;
    }
    if !proof.fact_time.is_valid()
        || proof.fact_time.lower != proof.fact_time.upper
        || proof.fact_time.lower < 0.0
        || instant(&snapshot["quotaRead"]["factTimeLower"]) != Some(proof.fact_time.lower)
        || instant(&snapshot["quotaRead"]["factTimeUpper"]) != Some(proof.fact_time.upper)
        || snapshot["quotaRead"]["outcome"] != "success"
        || instant(&snapshot["quotaRead"]["freshnessAnchor"]) != Some(proof.ttl_start)
        || !matches!(
            snapshot["quotaRead"]["timeBasis"].as_str(),
            Some("verified_on_demand_bounds" | "official_event_time" | "adapter_billing_clock")
        )
        || snapshot["accountRead"]["outcome"] != "success"
    {
        return Err(reject(Reason::TimeUnknown, "billing_boundary_unproved"));
    }
    let sample = NativeSample {
        observation_id: format!("{}:{}:{}", s.runtime_id, proof.sequence, s.pool_id),
        sequence: proof.sequence,
        scope: s.clone(),
        percent: proof.percent.clone(),
        fact_time: Some(proof.fact_time),
        ttl_start: proof.ttl_start,
        persisted: true,
        identity_consistent: true,
        clock_epoch: proof.clock_epoch.clone(),
        reset_time: Some(proof.reset_time),
        recovery: proof.recovery,
        discontinuity: snapshot["discontinuity"] == true,
    };
    Ok(CommittedQuotaObservation {
        receipt_id,
        sample,
        proof: proof.clone(),
        independent_training_sample: receipt["storageEvidence"]["independentTrainingSample"]
            != false,
        derived_metadata_present: receipt["derivedCalibrationPatches"]
            .get(&s.pool_id)
            .is_some()
            && hydrated_observation(&receipt).is_some(),
    })
}

fn policy_valid(policy: &FrozenHoldoutPolicy) -> bool {
    policy.schema_version == 1
        && policy.policy_version == POLICY_VERSION
        && policy.domain.valid()
        && policy.source_id == policy.domain.source_id
        && policy.account_scope == policy.domain.account_scope
        && Domain::from_scope(&policy.scope).as_ref() == Some(&policy.domain)
        && instant(&json!(policy.created_at)).is_some()
        && policy
            .created_mono_ms
            .parse::<u64>()
            .ok()
            .is_some_and(|v| v < u64::MAX)
        && profiles_compatible(&policy.anchor_profile, &policy.anchor_profile)
        && policy.assignments.len() <= MAX_PAIRS
}
fn save_policy(db: &Connection, policy: &FrozenHoldoutPolicy) -> Result<()> {
    let value = serde_json::to_string(policy).map_err(err)?;
    if value.len() > MAX_BYTES {
        return Err("校准留出策略达到有界存储上限".into());
    }
    db.execute("INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![format!("{PREFIX}{}",policy.id),value]).map_err(err)?;
    Ok(())
}
pub(crate) fn freeze_holdout_policy(
    db: &Connection,
    scope: &EvaluationScope,
    anchor: &UsageProfile,
    at: &Reading,
) -> Result<FrozenHoldoutPolicy> {
    if db.is_autocommit() {
        return Err("留出策略必须在观测事务内冻结".into());
    }
    let domain = Domain::from_scope(scope)
        .filter(Domain::valid)
        .ok_or("留出策略缺少稳定账号及规则")?;
    if !profiles_compatible(anchor, anchor) || at.mono_ms == u64::MAX {
        return Err("留出策略画像或时间无效".into());
    }
    let mut q=db.prepare("SELECT value FROM settings WHERE key LIKE 'subscriptionHoldout:%' ORDER BY key LIMIT 101").map_err(err)?;
    let rows = q
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(err)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(err)?;
    if rows.len() > MAX_POLICIES {
        return Err("校准留出策略数量达到上限".into());
    }
    for row in &rows {
        if row.len() > MAX_BYTES {
            return Err("已有留出策略超过存储上限".into());
        }
        let policy: FrozenHoldoutPolicy = serde_json::from_str(row).map_err(err)?;
        if !policy_valid(&policy) {
            return Err("留出策略版本或证据无效".into());
        }
        if policy.domain == domain && profiles_compatible(&policy.anchor_profile, anchor) {
            return Ok(policy);
        }
    }
    if rows.len() == MAX_POLICIES {
        return Err("校准留出策略数量达到上限".into());
    }
    let created_at = at.utc.to_rfc3339();
    let id = canonical_digest(&json!([
        VERSION,
        domain,
        anchor,
        created_at,
        at.mono_ms.to_string()
    ]));
    let policy = FrozenHoldoutPolicy {
        id,
        schema_version: 1,
        policy_version: POLICY_VERSION.into(),
        source_id: domain.source_id.clone(),
        account_scope: domain.account_scope.clone(),
        domain,
        scope: scope.clone(),
        anchor_profile: anchor.clone(),
        created_at,
        created_mono_ms: at.mono_ms.to_string(),
        assignments: Vec::new(),
    };
    save_policy(db, &policy)?;
    Ok(policy)
}

fn contains(outer: Interval, start: f64, end: f64) -> bool {
    outer.is_valid() && outer.lower <= start && outer.upper >= end
}
fn scope_key(scope: &EvaluationScope) -> String {
    canonical_digest(&json!(scope))
}
fn continuous(
    db: &Connection,
    a: &CommittedQuotaObservation,
    b: &CommittedQuotaObservation,
) -> std::result::Result<(), Reason> {
    if a.sample.discontinuity || b.sample.discontinuity {
        return Err(Reason::Discontinuous);
    }
    if a.sample.scope != b.sample.scope
        || a.sample.clock_epoch != b.sample.clock_epoch
        || a.sample.reset_time != b.sample.reset_time
    {
        return Err(Reason::ScopeChanged);
    }
    if b.receipt_id <= a.receipt_id || b.sample.sequence <= a.sample.sequence {
        return Err(Reason::OutOfOrder);
    }
    let mut q=db.prepare("SELECT data FROM account_snapshots WHERE id>?1 AND id<=?2 AND json_extract(data,'$.sourceId')=?3 AND json_extract(data,'$.runtimeId')=?4 ORDER BY id LIMIT 1025").map_err(|_|Reason::NotPersisted)?;
    let rows = q
        .query_map(
            params![
                a.receipt_id,
                b.receipt_id,
                a.sample.scope.source_id,
                a.sample.scope.runtime_id
            ],
            |r| r.get::<_, String>(0),
        )
        .map_err(|_| Reason::NotPersisted)?;
    let mut previous = a.sample.percent.used_percent;
    let mut previous_sequence = a.sample.sequence;
    let mut count = 0;
    for row in rows {
        count += 1;
        if count > MAX_OBSERVATIONS {
            return Err(Reason::InvalidNumber);
        }
        let stored: Value = serde_json::from_str(&row.map_err(|_| Reason::NotPersisted)?)
            .map_err(|_| Reason::NotPersisted)?;
        let r =
            super::observation_storage::resolve(db, &stored).map_err(|_| Reason::NotPersisted)?;
        if r["storageEvidence"]["available"] == false {
            return Err(Reason::EvidenceInvalidated);
        }
        let s = &r["observation"];
        if [
            "runtimeId",
            "sourceId",
            "bindingGeneration",
            "identityEpoch",
            "clearGeneration",
            "observationSequence",
        ]
        .into_iter()
        .any(|key| s.get(key) != r.get(key))
            || decimal(&r["observationSequence"])
                .is_none_or(|sequence| sequence <= previous_sequence)
            || decimal(&r["lastPersistedSequence"]) != decimal(&r["observationSequence"])
        {
            return Err(Reason::OutOfOrder);
        }
        previous_sequence = decimal(&r["observationSequence"]).unwrap();
        if s["discontinuity"] == true
            || r["documentKind"] != "subscription_observation"
            || r["schemaVersion"] != 1
            || r["outcome"] != "success"
            || !s.is_object()
            || s["identityConsistency"] != "proven"
            || decimal(&r["bindingGeneration"]) != Some(a.sample.scope.binding_generation)
            || decimal(&r["identityEpoch"]) != Some(a.sample.scope.identity_epoch)
            || r["clearGeneration"].to_string() != a.sample.scope.clear_generation
            || s["accountScope"].as_str() != a.sample.scope.account_scope.as_deref()
            || s["quotaRead"]["outcome"] != "success"
        {
            return Err(Reason::Discontinuous);
        }
        let w = raw_window(s, &a.sample.scope.pool_id).ok_or(Reason::Discontinuous)?;
        let p = w["usedPercent"]
            .as_f64()
            .filter(|v| v.is_finite())
            .ok_or(Reason::InvalidNumber)?;
        if p < previous {
            return Err(Reason::PercentDecreased);
        }
        previous = p;
        if w["resetsAt"].as_i64().map(|v| v as f64) != a.sample.reset_time.map(|t| t.lower) {
            return Err(Reason::ScopeChanged);
        }
    }
    Ok(())
}

fn ledger_rows<'a>(
    db: &Connection,
    usage: &'a [ResponseUsage],
    domain: &Domain,
    start: f64,
    end: f64,
    settlement: &LedgerSettlement,
    now: f64,
) -> std::result::Result<Vec<&'a ResponseUsage>, Reason> {
    if settlement.domain != *domain
        || settlement.revision > MAX_SAFE
        || !contains(settlement.complete_interval, start, end)
        || !settlement.captured_at.is_finite()
        || settlement.captured_at < end
        || settlement.captured_at > now
        || !valid_text(&settlement.metering_version)
        || !valid_text(&settlement.billing_alignment_reference)
    {
        return Err(Reason::Unsettled);
    }
    let revision = setting(db, "subscriptionRevision")
        .map_err(|_| Reason::Unsettled)?
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if revision != settlement.revision {
        return Err(Reason::Unsettled);
    }
    let check = setting(db, &format!("lastSourceCheck:{}", domain.source_id))
        .map_err(|_| Reason::Unsettled)?
        .ok_or(Reason::Unsettled)?;
    if source_check_digest(&check) != settlement.source_check_digest
        || check["discoveryComplete"] != true
        || check["cancelled"] != false
        || ["pendingFiles", "issues", "discoveryErrors"]
            .into_iter()
            .any(|key| check[key].as_u64() != Some(0))
        || instant(&check["checkedAt"]) != Some(settlement.captured_at)
    {
        return Err(Reason::ConflictsOrGaps);
    }
    for key in ["importSince", "clearWatermark", "retentionWatermark"] {
        if let Some(value) = setting(db, key).map_err(|_| Reason::EvidenceInvalidated)? {
            if instant(&value).is_none_or(|cutoff| cutoff > start) {
                return Err(Reason::EvidenceInvalidated);
            }
        }
    }
    let sql="SELECT u.id,u.event_time,u.data,EXISTS(SELECT 1 FROM excluded_facts e WHERE e.fact_id=u.id),EXISTS(SELECT 1 FROM fact_aliases a WHERE a.alias=u.id) FROM usage_responses u WHERE source_id=?1 AND julianday(event_time)>=julianday(?2)-1.0/86400.0 AND julianday(event_time)<julianday(?3)+1.0/86400.0 ORDER BY u.id LIMIT 10001";
    let mut q = db.prepare(sql).map_err(|_| Reason::Unsettled)?;
    let raw = q
        .query_map(
            params![
                domain.source_id,
                timestamp(start).ok_or(Reason::TimeInvalid)?,
                timestamp(end).ok_or(Reason::TimeInvalid)?
            ],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, bool>(3)?,
                    r.get::<_, bool>(4)?,
                ))
            },
        )
        .map_err(|_| Reason::Unsettled)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| Reason::Unsettled)?;
    if raw.len() > MAX_PAIR_ROWS {
        return Err(Reason::InvalidNumber);
    }
    let mut canonical = Vec::new();
    for (id, column_time, data, excluded, alias) in raw {
        let at = instant(&json!(column_time)).ok_or(Reason::TimeUnknown)?;
        if at < start || at >= end || alias {
            continue;
        }
        if excluded {
            return Err(Reason::ConflictsOrGaps);
        }
        let row: ResponseUsage =
            serde_json::from_str(&data).map_err(|_| Reason::ConflictsOrGaps)?;
        if adapter::response_key(&row) != id || instant(&json!(row.event_time)) != Some(at) {
            return Err(Reason::ConflictsOrGaps);
        }
        canonical.push(row);
    }
    let mut paired = Vec::new();
    for row in usage {
        if row.source_id != domain.source_id {
            continue;
        }
        let at = instant(&json!(row.event_time)).ok_or(Reason::TimeUnknown)?;
        if at < start || at >= end {
            continue;
        }
        if row.account_scope_id.as_deref() != Some(domain.account_scope.as_str()) {
            return Err(Reason::IdentityUnproven);
        }
        if row.phase != "final"
            || !valid_text(&row.adapter_version)
            || row.semantics_version != settlement.metering_version
        {
            return Err(Reason::ConflictsOrGaps);
        }
        paired.push(row);
    }
    if response_ledger_digest(&paired)
        != response_ledger_digest(&canonical.iter().collect::<Vec<_>>())
    {
        return Err(Reason::EvidenceInvalidated);
    }
    Ok(paired)
}

fn usage_profile(
    rows: &[&ResponseUsage],
    mode: &UsageModeAttestation,
) -> std::result::Result<(u64, UsageProfile), Reason> {
    if !valid_text(&mode.adapter_id)
        || !valid_text(&mode.reference)
        || [&mode.effort, &mode.speed]
            .into_iter()
            .any(|v| matches!(v,ExplicitMode::Known(value) if !valid_text(value)))
    {
        return Err(Reason::ProfileChanged);
    }
    let mut total = 0u64;
    let mut input = 0u64;
    let mut output = 0u64;
    let mut cache = 0u64;
    let mut inputs = Vec::new();
    let mut models = BTreeMap::<String, u64>::new();
    let mut ids = BTreeSet::new();
    for row in rows {
        if !ids.insert((&row.namespace, &row.response_id)) {
            return Err(Reason::ConflictsOrGaps);
        }
        let model = row
            .observed_model
            .as_deref()
            .filter(|v| valid_text(v) && *v != "unknown")
            .ok_or(Reason::ModelCoverageUnknown)?;
        let get = |m: &adapter::Metric| {
            m.value()
                .filter(|v| *v >= 0)
                .map(|v| v as u64)
                .ok_or(Reason::ConflictsOrGaps)
        };
        let i = get(&row.counts.input)?;
        let o = get(&row.counts.output)?;
        let t = get(&row.counts.total)?;
        let cr = get(&row.counts.cache_read)?;
        let cw = get(&row.counts.cache_write)?;
        let reasoning = get(&row.counts.reasoning)?;
        if i.checked_add(o) != Some(t) || cr.checked_add(cw).is_none_or(|v| v > i) || reasoning > o
        {
            return Err(Reason::ConflictsOrGaps);
        }
        let add = |a: u64, b: u64| {
            a.checked_add(b)
                .filter(|v| *v <= MAX_SAFE)
                .ok_or(Reason::InvalidNumber)
        };
        total = add(total, t)?;
        input = add(input, i)?;
        output = add(output, o)?;
        cache = add(cache, cr)?;
        let entry = models.entry(model.into()).or_default();
        *entry = add(*entry, t)?;
        inputs.push(i as f64);
    }
    if input == 0 || total == 0 || inputs.is_empty() {
        return Err(Reason::LowSignal);
    }
    inputs.sort_by(f64::total_cmp);
    let mid = inputs.len() / 2;
    let median = if inputs.len().is_multiple_of(2) {
        inputs[mid - 1] / 2.0 + inputs[mid] / 2.0
    } else {
        inputs[mid]
    };
    Ok((
        total,
        UsageProfile {
            models: models
                .into_iter()
                .map(|(model, t)| ModelShare {
                    model,
                    share: t as f64 / total as f64,
                })
                .collect(),
            all_models_attributed: true,
            effort: mode.effort.value(),
            speed_mode: mode.speed.value(),
            output_share: output as f64 / total as f64,
            cache_share: cache as f64 / input as f64,
            median_input_tokens: median,
        },
    ))
}

fn make_patch(
    observation: &CommittedQuotaObservation,
    evidence: Option<Value>,
) -> ObservationPatch {
    let s = &observation.sample;
    let p = &observation.proof;
    let mut window = json!({"roundingStep":s.percent.rounding_step,"saturated":s.percent.saturated,"saturationHeadroomBounded":s.percent.saturation_headroom_bounded,
        "ruleVersion":s.scope.rule_version,"windowInstanceId":s.scope.window_instance_id,"recovery":s.recovery,
        "windowInstanceEvidence":{"verified":true,"source":"official","stableAcrossRuntimes":p.period_stable_across_runtimes,"windowInstanceId":s.scope.window_instance_id,"reference":p.period_reference}});
    if let Some(evidence) = evidence {
        window["calibrationEvidence"] = evidence;
    }
    ObservationPatch {
        receipt_id: observation.receipt_id,
        observation_id: s.observation_id.clone(),
        window_id: s.scope.pool_id.clone(),
        snapshot_fields: json!({"identityEvidence":{"adapterId":p.adapter_id,"reference":p.stable_identity_reference},"entitlementSegment":s.scope.entitlement_segment,
            "entitlementEvidence":{"verified":true,"source":"official","segmentId":s.scope.entitlement_segment,"ruleVersion":s.scope.rule_version,"reference":p.entitlement_reference},
            "clockEpoch":s.clock_epoch}),
        window_fields: window,
        structure_seal: None,
    }
}

/// All policy writes are part of the caller's observation/scan transaction.
/// Returned patches must be persisted in that same transaction by the owner.
pub(crate) fn assemble(
    db: &Connection,
    batch: &EvidenceBatch<'_>,
    at: &Reading,
) -> Result<Assembly> {
    if db.is_autocommit() {
        return Err("容量证据必须在观测事务内组装".into());
    }
    let mut out = Assembly::default();
    if batch.observations.len() > MAX_OBSERVATIONS
        || batch.usage.len() > MAX_USAGE
        || batch.coverage.len() > MAX_OBSERVATIONS
        || batch.modes.len() > MAX_OBSERVATIONS
    {
        out.missing
            .push(reject(Reason::InvalidNumber, "assembly_batch_limit"));
        return Ok(out);
    }
    if batch.observations.is_empty() {
        out.missing.push(reject(
            Reason::IdentityUnproven,
            "adapter_quota_proof_missing",
        ));
        return Ok(out);
    }
    let now = seconds(at.utc);
    if at.mono_ms == u64::MAX || !now.is_finite() || now < 0.0 {
        out.missing
            .push(reject(Reason::TimeInvalid, "clock_invalid"));
        return Ok(out);
    }
    let Some(settlement) = batch.settlement else {
        out.missing
            .push(reject(Reason::Unsettled, "independent_settlement_missing"));
        return Ok(out);
    };
    let mut baselines = BTreeMap::<String, &CommittedQuotaObservation>::new();
    let mut last_receipt = 0;
    let mut patches = BTreeMap::<i64, ObservationPatch>::new();
    let mut manifests = BTreeMap::<String, FrozenHoldoutPolicy>::new();
    let mut assigned_ends = BTreeSet::new();
    let mut old_policies = db
        .prepare("SELECT value FROM settings WHERE key LIKE 'subscriptionHoldout:%' LIMIT 101")
        .map_err(err)?;
    let old_rows = old_policies
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(err)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(err)?;
    if old_rows.len() > MAX_POLICIES {
        return Err("校准留出策略数量达到上限".into());
    }
    for value in old_rows {
        if value.len() > MAX_BYTES {
            return Err("留出策略超过有界存储上限".into());
        }
        let policy: FrozenHoldoutPolicy = serde_json::from_str(&value).map_err(err)?;
        if !policy_valid(&policy) {
            return Err("留出策略版本或证据无效".into());
        }
        for assignment in policy.assignments {
            assigned_ends.insert((assignment.end_receipt, policy.domain.pool_id.clone()));
        }
    }
    for end in batch.observations {
        if end.receipt_id <= last_receipt {
            out.missing
                .push(reject(Reason::OutOfOrder, "commit_order_invalid"));
            continue;
        }
        last_receipt = end.receipt_id;
        let key = scope_key(&end.sample.scope);
        let verified = match load_observation(db, end.receipt_id, Some(&end.proof)) {
            Ok(verified) => verified,
            Err(mut rejection) => {
                rejection.observation_id = Some(end.sample.observation_id.clone());
                out.missing.push(rejection);
                baselines.remove(&key);
                continue;
            }
        };
        // A fresh typed native read remains usable for native ETA even when
        // unchanged storage is not a new independent capacity training sample.
        // Bound this extra metadata to the current batch endpoint, not history.
        if !verified.derived_metadata_present
            && batch
                .observations
                .last()
                .is_some_and(|last| last.receipt_id == end.receipt_id)
        {
            let mut patch = make_patch(end, None);
            if let Some(start) = baselines.get(&key).copied() {
                if let Some((structure, seal)) =
                    usage_structure::at_endpoint(db, batch, start, end, settlement, now)
                {
                    patch.window_fields["usageStructureEvidence"] = structure;
                    patch.structure_seal = Some(seal);
                }
            }
            patches.entry(end.receipt_id).or_insert(patch);
        }
        if !verified.independent_training_sample {
            // Preserve the last independent baseline; a duplicate's zero delta
            // must neither create a pair nor shorten the next ledger interval.
            continue;
        }
        // Persisted roles and original digest remain immutable on replay. The
        // projector rechecks their frozen digest against the live trusted
        // ledger; replay cannot erase a failed pair or turn it into training.
        if assigned_ends.contains(&(end.receipt_id, end.sample.scope.pool_id.clone())) {
            baselines.insert(key, end);
            continue;
        }
        let Some(start) = baselines.get(&key).copied() else {
            baselines.insert(key, end);
            continue;
        };
        let a = start.sample.fact_time.unwrap().lower;
        let b = end.sample.fact_time.unwrap().upper;
        let fail = |reason, detail| Rejection {
            observation_id: Some(end.sample.observation_id.clone()),
            reason,
            detail,
        };
        if let Err(reason) = continuous(db, start, end) {
            out.missing
                .push(fail(reason, "observation_segment_broken".into()));
            baselines.insert(key, end);
            continue;
        }
        if b <= a
            || b > now
            || b >= end.sample.reset_time.unwrap().lower
            || end.sample.percent.used_percent >= 100.0
            || end.sample.percent.saturated
            || start.sample.percent.saturated
        {
            out.missing.push(fail(
                Reason::WaitingForReset,
                "pair_boundary_invalid".into(),
            ));
            baselines.insert(key, end);
            continue;
        }
        let minimum_signal = start
            .sample
            .percent
            .rounding_step
            .zip(end.sample.percent.rounding_step)
            .map_or(3.0, |(a, b)| (5.0 * a.max(b)).max(3.0));
        if end.sample.percent.used_percent - start.sample.percent.used_percent < minimum_signal {
            continue;
        }
        let domain = Domain::from_scope(&end.sample.scope).ok_or("容量证据缺少稳定账号")?;
        if settlement.billing_clock_epoch != start.sample.clock_epoch {
            out.missing
                .push(fail(Reason::ClockChanged, "billing_clock_mismatch".into()));
            continue;
        }
        let mut coverages: Vec<_> = batch
            .coverage
            .iter()
            .filter(|proof| proof.domain == domain && contains(proof.interval, a, b))
            .collect();
        if coverages
            .iter()
            .any(|proof| matches!(proof.kind, CoverageKind::KnownIncomplete))
        {
            out.missing.push(fail(
                Reason::ExternalUsageUncovered,
                "conflicting_or_incomplete_coverage".into(),
            ));
            continue;
        }
        coverages.sort_by_key(|proof| match &proof.kind {
            CoverageKind::VerifiedComplete {
                adapter_id,
                reference,
            } => (0, adapter_id.clone(), reference.clone()),
            CoverageKind::FutureUserAssumption { reference, .. } => {
                (1, String::new(), reference.clone())
            }
            CoverageKind::KnownIncomplete => (2, String::new(), String::new()),
        });
        let coverage = coverages.first().copied();
        let (coverage_kind, coverage_evidence) = match coverage.map(|p| &p.kind) {
            Some(CoverageKind::VerifiedComplete {
                adapter_id,
                reference,
            }) if valid_text(adapter_id) && valid_text(reference) => (
                CoverageEvidence::VerifiedComplete,
                json!({"adapterId":adapter_id,"reference":reference}),
            ),
            Some(CoverageKind::FutureUserAssumption {
                declared_at,
                reference,
            }) if declared_at.is_finite()
                && *declared_at >= 0.0
                && *declared_at <= a
                && valid_text(reference) =>
            {
                (
                    CoverageEvidence::ConditionalUserAssumption,
                    json!({"declaredAt":declared_at,"reference":reference}),
                )
            }
            Some(CoverageKind::KnownIncomplete) => {
                out.missing.push(fail(
                    Reason::ExternalUsageUncovered,
                    "known_external_usage".into(),
                ));
                continue;
            }
            _ => {
                out.missing.push(fail(
                    Reason::CoverageUnknown,
                    "independent_coverage_missing".into(),
                ));
                continue;
            }
        };
        let Some(mode) = batch
            .modes
            .iter()
            .find(|mode| mode.domain == domain && contains(mode.interval, a, b))
        else {
            out.missing.push(fail(
                Reason::ProfileChanged,
                "explicit_mode_evidence_missing".into(),
            ));
            continue;
        };
        if batch
            .modes
            .iter()
            .filter(|proof| proof.domain == domain && contains(proof.interval, a, b))
            .any(|proof| proof.effort != mode.effort || proof.speed != mode.speed)
        {
            out.missing.push(fail(
                Reason::ProfileChanged,
                "conflicting_mode_evidence".into(),
            ));
            continue;
        }
        let rows = match ledger_rows(db, batch.usage, &domain, a, b, settlement, now) {
            Ok(rows) => rows,
            Err(reason) => {
                out.missing
                    .push(fail(reason, "ledger_pair_unsettled".into()));
                continue;
            }
        };
        let (tokens, profile) = match usage_profile(&rows, mode) {
            Ok(value) => value,
            Err(reason) => {
                out.missing
                    .push(fail(reason, "usage_profile_incomplete".into()));
                continue;
            }
        };
        let mut policy = freeze_holdout_policy(db, &end.sample.scope, &profile, at)?;
        let pair_id = format!(
            "{}:{}",
            start.sample.observation_id, end.sample.observation_id
        );
        let digest = response_ledger_digest(&rows);
        let input_digest = canonical_digest(&json!([
            start.sample,
            end.sample,
            tokens,
            profile,
            coverage_evidence,
            mode.reference,
            settlement.billing_alignment_reference
        ]));
        let previous = policy
            .assignments
            .iter()
            .find(|old| old.pair_id == pair_id)
            .cloned();
        let invalidated = previous
            .as_ref()
            .is_some_and(|old| old.input_digest != input_digest || old.ledger_digest != digest);
        let assignment = if let Some(old) = previous {
            old
        } else {
            if policy.assignments.len() >= MAX_PAIRS {
                out.missing
                    .push(fail(Reason::InvalidNumber, "manifest_pair_limit".into()));
                continue;
            }
            if policy
                .assignments
                .iter()
                .any(|old| a < old.end && old.start < b)
            {
                out.missing.push(fail(
                    Reason::OverlappingPairs,
                    "persisted_pair_overlap".into(),
                ));
                continue;
            }
            if policy.assignments.last().is_some_and(|old| a < old.end) {
                out.missing.push(fail(
                    Reason::OutOfOrder,
                    "late_pair_does_not_rewrite_holdout".into(),
                ));
                continue;
            }
            let train: Vec<_> = policy
                .assignments
                .iter()
                .filter(|old| !old.holdout)
                .collect();
            let periods = train
                .iter()
                .map(|old| old.period.as_str())
                .collect::<BTreeSet<_>>()
                .len();
            let holdout = train.len() >= 20
                && periods >= 3
                && instant(&json!(policy.created_at)).is_some_and(|frozen| frozen < a);
            let assignment = Assignment {
                pair_id: pair_id.clone(),
                start_receipt: start.receipt_id,
                end_receipt: end.receipt_id,
                start: a,
                end: b,
                period: end.sample.scope.window_instance_id.clone(),
                holdout,
                ledger_digest: digest.clone(),
                input_digest,
                evidence_digest: String::new(),
            };
            policy.assignments.push(assignment.clone());
            assignment
        };
        let evidence = json!({"fromSequence":start.sample.sequence.to_string(),"ledgerRevision":settlement.revision.to_string(),
            "meteringVersion":settlement.metering_version,"responseLedgerDigest":assignment.ledger_digest,
            "coverage":coverage_kind,"coverageEvidence":coverage_evidence,"identityEvidence":"verified_account",
            "boundaryAligned":true,"conflictsOrGaps":false,"settledThrough":settlement.complete_interval.upper,
            "effort":profile.effort,"speedMode":profile.speed_mode,"holdout":assignment.holdout,
            "holdoutManifestId":policy.id,"holdoutFrozenAt":policy.created_at,"invalidated":invalidated,"assemblerVersion":VERSION});
        let registered = policy
            .assignments
            .iter_mut()
            .find(|old| old.pair_id == pair_id)
            .ok_or("配对角色登记缺失")?;
        let evidence_digest = canonical_digest(&evidence);
        if registered.evidence_digest.is_empty() {
            registered.evidence_digest = evidence_digest;
            save_policy(db, &policy)?;
        } else if registered.evidence_digest != evidence_digest {
            return Err("冻结容量证据不能重写".into());
        }
        patches
            .entry(start.receipt_id)
            .or_insert_with(|| make_patch(start, None));
        let mut end_patch = make_patch(end, Some(evidence));
        // A local declaration never proves the model mix of an account pool.
        // Only the same settled, fully attributed, independently covered pair
        // can supply the structure used by a manual capacity premise.
        if coverage_kind == CoverageEvidence::VerifiedComplete
            && !invalidated
            && profile
                .effort
                .as_deref()
                .is_some_and(|v| valid_text(v) && v != "unknown")
            && profile
                .speed_mode
                .as_deref()
                .is_some_and(|v| valid_text(v) && v != "unknown")
        {
            let model = (profile.models.len() == 1 && profile.all_models_attributed)
                .then(|| profile.models[0].model.as_str());
            end_patch.window_fields["usageStructureEvidence"] = json!({"verified":true,"source":"trusted-ledger",
                "reference":assignment.ledger_digest,"sourceId":end.sample.scope.source_id,"accountScope":end.sample.scope.account_scope,
                "windowId":end.sample.scope.pool_id,"windowInstanceId":end.sample.scope.window_instance_id,
                "modelDimension":"response","model":model,"profile":profile,"ledgerRevision":settlement.revision.to_string(),
                "fromSequence":start.sample.sequence.to_string(),"observationSequence":end.sample.sequence.to_string(),
                "ledgerInterval":{"lower":a,"upper":b},"nativeDigest":end.proof.native_digest});
        }
        patches.insert(end.receipt_id, end_patch);
        out.pairs.push(CalibrationPair {
            pair_id,
            start: start.sample.clone(),
            end: end.sample.clone(),
            tokens,
            profile,
            identity_evidence: IdentityEvidence::VerifiedAccount,
            coverage: coverage_kind,
            settled: true,
            conflicts_or_gaps: false,
            holdout: assignment.holdout,
            evidence_invalidated: invalidated,
        });
        manifests.insert(policy.id.clone(), policy);
        baselines.insert(key, end);
        if out.pairs.len() >= MAX_PAIRS {
            out.missing
                .push(reject(Reason::InvalidNumber, "assembly_pair_limit"));
            break;
        }
    }
    out.patches = patches.into_values().collect();
    out.manifests = manifests.into_values().collect();
    if serde_json::to_vec(&out).map_err(err)?.len() > MAX_BYTES {
        return Err("本轮容量证据超过有界输出上限，事务须回滚".into());
    }
    Ok(out)
}

fn patch_shape(patch: &ObservationPatch) -> bool {
    let Some(snapshot) = patch.snapshot_fields.as_object() else {
        return false;
    };
    let Some(window) = patch.window_fields.as_object() else {
        return false;
    };
    if snapshot.keys().any(|key| {
        !matches!(
            key.as_str(),
            "identityEvidence" | "entitlementSegment" | "entitlementEvidence" | "clockEpoch"
        )
    }) || window.keys().any(|key| {
        !matches!(
            key.as_str(),
            "roundingStep"
                | "saturated"
                | "saturationHeadroomBounded"
                | "ruleVersion"
                | "windowInstanceId"
                | "recovery"
                | "windowInstanceEvidence"
                | "calibrationEvidence"
                | "usageStructureEvidence"
        )
    }) {
        return false;
    }
    if !patch.snapshot_fields["identityEvidence"].is_object()
        || text(&patch.snapshot_fields["identityEvidence"]["adapterId"]).is_none()
        || text(&patch.snapshot_fields["identityEvidence"]["reference"]).is_none()
        || text(&patch.snapshot_fields["entitlementSegment"]).is_none()
        || text(&patch.snapshot_fields["clockEpoch"]).is_none()
        || patch.snapshot_fields["entitlementEvidence"]["verified"] != true
        || patch.snapshot_fields["entitlementEvidence"]["source"] != "official"
        || patch.snapshot_fields["entitlementEvidence"]["segmentId"]
            != patch.snapshot_fields["entitlementSegment"]
    {
        return false;
    }
    let rounding_valid = patch
        .window_fields
        .get("roundingStep")
        .is_some_and(|value| {
            value.is_null()
                || value
                    .as_f64()
                    .is_some_and(|v| v.is_finite() && v > 0.0 && v <= 100.0)
        });
    if !rounding_valid
        || text(&patch.window_fields["ruleVersion"]).is_none()
        || text(&patch.window_fields["windowInstanceId"]).is_none()
        || !matches!(
            patch.window_fields["recovery"].as_str(),
            Some("fixed_reset" | "anchored_reset")
        )
        || patch.window_fields["windowInstanceEvidence"]["windowInstanceId"]
            != patch.window_fields["windowInstanceId"]
    {
        return false;
    }
    if let Some(e) = patch.window_fields.get("calibrationEvidence") {
        if !e.is_object()
            || e.as_object().unwrap().keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "fromSequence"
                        | "ledgerRevision"
                        | "meteringVersion"
                        | "responseLedgerDigest"
                        | "coverage"
                        | "coverageEvidence"
                        | "identityEvidence"
                        | "boundaryAligned"
                        | "conflictsOrGaps"
                        | "settledThrough"
                        | "effort"
                        | "speedMode"
                        | "holdout"
                        | "holdoutManifestId"
                        | "holdoutFrozenAt"
                        | "invalidated"
                        | "assemblerVersion"
                )
            })
            || e["assemblerVersion"] != VERSION
            || e["identityEvidence"] != "verified_account"
            || e["boundaryAligned"] != true
            || e["conflictsOrGaps"] != false
            || decimal(&e["fromSequence"]).is_none_or(|v| v == 0)
            || decimal(&e["ledgerRevision"]).is_none()
            || text(&e["meteringVersion"]).is_none()
            || text(&e["responseLedgerDigest"]).is_none()
            || instant(&e["settledThrough"]).is_none()
            || !e["coverageEvidence"].is_object()
            || !matches!(
                e["coverage"].as_str(),
                Some("verified_complete" | "conditional_user_assumption")
            )
            || ["effort", "speedMode"]
                .into_iter()
                .any(|key| !e.get(key).is_some_and(|v| v.is_null() || text(v).is_some()))
            || !e["holdout"].is_boolean()
            || !e["invalidated"].is_boolean()
            || text(&e["holdoutManifestId"]).is_none()
            || instant(&e["holdoutFrozenAt"]).is_none()
        {
            return false;
        }
    }
    if let Some(structure) = patch.window_fields.get("usageStructureEvidence") {
        let evidence = &patch.window_fields["calibrationEvidence"];
        let independent = structure.get("basis").is_some();
        if independent
            && (patch.window_fields.get("calibrationEvidence").is_some()
                || !usage_structure::valid_basis(structure, &patch.snapshot_fields["clockEpoch"]))
        {
            return false;
        }
        let Ok(profile) = serde_json::from_value::<UsageProfile>(structure["profile"].clone())
        else {
            return false;
        };
        let Ok(interval) = serde_json::from_value::<Interval>(structure["ledgerInterval"].clone())
        else {
            return false;
        };
        let model = (profile.models.len() == 1 && profile.all_models_attributed)
            .then(|| profile.models[0].model.as_str());
        if structure["verified"] != true
            || structure["source"] != "trusted-ledger"
            || text(&structure["reference"]).is_none()
            || (!independent && structure["reference"] != evidence["responseLedgerDigest"])
            || text(&structure["sourceId"]).is_none()
            || text(&structure["accountScope"]).is_none()
            || structure["windowId"] != patch.window_id
            || structure["windowInstanceId"] != patch.window_fields["windowInstanceId"]
            || structure["modelDimension"] != "response"
            || structure["model"] != json!(model)
            || !interval.is_valid()
            || interval.lower < 0.0
            || interval.upper <= interval.lower
            || text(&structure["nativeDigest"]).is_none()
            || decimal(&structure["ledgerRevision"]).is_none()
            || (!independent && structure["ledgerRevision"] != evidence["ledgerRevision"])
            || (!independent && structure["fromSequence"] != evidence["fromSequence"])
            || decimal(&structure["observationSequence"]).is_none_or(|sequence| {
                decimal(&structure["fromSequence"]).is_none_or(|from| from >= sequence)
            })
            || (!independent && evidence["coverage"] != "verified_complete")
            || (!independent && evidence["invalidated"] != false)
            || profile
                .effort
                .as_deref()
                .is_none_or(|v| !valid_text(v) || v == "unknown")
            || profile
                .speed_mode
                .as_deref()
                .is_none_or(|v| !valid_text(v) || v == "unknown")
            || (!independent && evidence["effort"] != json!(profile.effort))
            || (!independent && evidence["speedMode"] != json!(profile.speed_mode))
            || !profiles_compatible(&profile, &profile)
        {
            return false;
        }
    }
    true
}

/// Freeze only derived metadata in its own namespace. The original vendor
/// observation, scope, percentage, reset and account fields are never edited.
/// Replays do not change metadata or extend any freshness/settlement timestamp.
pub(crate) fn apply(db: &Connection, assembly: &Assembly) -> Result<()> {
    if db.is_autocommit() {
        return Err("容量证据补丁必须与观测事务一起提交".into());
    }
    if assembly.patches.len() > MAX_PAIRS * 2
        || serde_json::to_vec(assembly).map_err(err)?.len() > MAX_BYTES
    {
        return Err("容量证据补丁超过有界写入上限".into());
    }
    let mut updates = BTreeMap::<i64, String>::new();
    for patch in &assembly.patches {
        if !patch_shape(patch) {
            return Err("容量证据补丁包含未授权字段或缺少类型证据".into());
        }
        let raw: String = if let Some(value) = updates.get(&patch.receipt_id) {
            value.clone()
        } else {
            db.query_row(
                "SELECT data FROM account_snapshots WHERE id=?1 AND length(data)<=?2",
                params![patch.receipt_id, MAX_BYTES as i64],
                |r| r.get(0),
            )
            .map_err(err)?
        };
        let stored: Value = serde_json::from_str(&raw).map_err(err)?;
        let mut receipt = super::observation_storage::resolve(db, &stored)?;
        let Some(original) = hydrated_observation(&receipt) else {
            return Err("容量补丁的原始观测无效".into());
        };
        let native = &receipt["observation"];
        let runtime = text(&receipt["runtimeId"]).ok_or("观测运行段缺失")?;
        let sequence = decimal(&receipt["observationSequence"]).ok_or("观测序号缺失")?;
        if patch.observation_id != format!("{runtime}:{sequence}:{}", patch.window_id)
            || raw_window(native, &patch.window_id).is_none()
        {
            return Err("容量补丁不属于原始窗口或观测".into());
        }
        if let Some(e) = patch.window_fields.get("calibrationEvidence") {
            if decimal(&e["fromSequence"]).is_none_or(|from| from >= sequence) {
                return Err("容量配对顺序无效".into());
            }
            let policy_value = setting(
                db,
                &format!("{PREFIX}{}", e["holdoutManifestId"].as_str().unwrap()),
            )?
            .ok_or("容量补丁缺少冻结留出策略")?;
            let policy: FrozenHoldoutPolicy = serde_json::from_value(policy_value).map_err(err)?;
            let assignment = policy
                .assignments
                .iter()
                .find(|a| a.end_receipt == patch.receipt_id)
                .ok_or("容量补丁未登记不可变配对角色")?;
            let start_sequence:String=db.query_row("SELECT json_extract(data,'$.observationSequence') FROM account_snapshots WHERE id=?1",[assignment.start_receipt],|r|r.get(0)).map_err(err)?;
            if !policy_valid(&policy)
                || e["holdoutFrozenAt"] != policy.created_at
                || e["holdout"].as_bool() != Some(assignment.holdout)
                || e["responseLedgerDigest"].as_str() != Some(assignment.ledger_digest.as_str())
                || e["fromSequence"] != start_sequence
                || canonical_digest(e) != assignment.evidence_digest
                || policy.domain.source_id != receipt["sourceId"]
                || Some(policy.domain.account_scope.as_str())
                    != receipt["observation"]["accountScope"].as_str()
                || policy.domain.entitlement_segment != patch.snapshot_fields["entitlementSegment"]
                || policy.domain.rule_version != patch.window_fields["ruleVersion"]
                || policy.domain.pool_id != patch.window_id
                || policy.domain.clear_generation != receipt["clearGeneration"].to_string()
            {
                return Err("容量补丁改变了冻结样本或留出角色".into());
            }
        }
        if let Some(structure) = patch.window_fields.get("usageStructureEvidence") {
            if structure.get("basis").is_some()
                && patch
                    .structure_seal
                    .as_ref()
                    .is_none_or(|seal| !seal.matches(patch.receipt_id, structure))
            {
                return Err("独立使用结构缺少本轮受信封印".into());
            }
            if structure["sourceId"] != receipt["sourceId"]
                || structure["accountScope"] != native["accountScope"]
                || structure["observationSequence"] != receipt["observationSequence"]
                || structure["nativeDigest"] != native_snapshot_digest(native)
                || instant(&native["quotaRead"]["factTimeLower"])
                    != structure["ledgerInterval"]["upper"].as_f64()
                || instant(&native["quotaRead"]["factTimeUpper"])
                    != structure["ledgerInterval"]["upper"].as_f64()
            {
                return Err("使用结构不属于原始观测作用域".into());
            }
        }
        let value = json!({"schemaVersion":1,"algorithmVersion":VERSION,"observationId":patch.observation_id,"windowId":patch.window_id,
            "nativeDigest":canonical_digest(&native_value(native)),"snapshotFields":patch.snapshot_fields,"windowFields":patch.window_fields});
        let value_digest = canonical_digest(&value);
        let mut value = value;
        value["metadataDigest"] = json!(value_digest);
        if receipt.get("derivedCalibrationPatches").is_none() {
            receipt["derivedCalibrationPatches"] = json!({});
        }
        let namespace = receipt["derivedCalibrationPatches"]
            .as_object_mut()
            .ok_or("容量派生命名空间格式无效")?;
        if let Some(old) = namespace.get(&patch.window_id) {
            if old == &value {
                continue;
            }
            let mut old_base = old.clone();
            old_base.as_object_mut().unwrap().remove("metadataDigest");
            let mut new_base = value.clone();
            new_base.as_object_mut().unwrap().remove("metadataDigest");
            old_base["windowFields"]
                .as_object_mut()
                .unwrap()
                .remove("calibrationEvidence");
            old_base["windowFields"]
                .as_object_mut()
                .unwrap()
                .remove("usageStructureEvidence");
            new_base["windowFields"]
                .as_object_mut()
                .unwrap()
                .remove("calibrationEvidence");
            new_base["windowFields"]
                .as_object_mut()
                .unwrap()
                .remove("usageStructureEvidence");
            if old_base != new_base {
                return Err("已经冻结的容量依据不能覆写".into());
            }
            if patch.window_fields.get("calibrationEvidence").is_none() {
                continue;
            }
            if old["windowFields"].get("calibrationEvidence").is_some() {
                return Err("已有容量配对不能重写或续期".into());
            }
        }
        namespace.insert(patch.window_id.clone(), value);
        // Cross-window patches must agree on observation-level proof metadata.
        if hydrated_observation(&receipt).is_none() {
            return Err("同一观测的窗口证明不一致".into());
        }
        if original["accountScope"] != receipt["observation"]["accountScope"]
            || receipt.to_string().len() > MAX_BYTES
        {
            return Err("容量补丁改变了原始账号或超过预算".into());
        }
        let encoded = super::observation_storage::with_derived(db, &stored, &receipt)?;
        updates.insert(patch.receipt_id, encoded.to_string());
    }
    for (id, value) in updates {
        db.execute(
            "UPDATE account_snapshots SET data=?1 WHERE id=?2",
            params![value, id],
        )
        .map_err(err)?;
    }
    Ok(())
}

/// Read-only hydration of metadata previously frozen by `apply`. It validates
/// the original envelope first and refuses metadata after native fact changes.
pub(crate) fn hydrated_observation(receipt: &Value) -> Option<Value> {
    if receipt["documentKind"] != "subscription_observation"
        || receipt["schemaVersion"] != 1
        || receipt["outcome"] != "success"
    {
        return None;
    }
    let sequence = decimal(&receipt["observationSequence"])?;
    if sequence == 0 || decimal(&receipt["lastPersistedSequence"])? != sequence {
        return None;
    }
    let raw = &receipt["observation"];
    let mut snapshot = observation_proof::hydrate(receipt)?.as_object()?.clone();
    for key in [
        "runtimeId",
        "sourceId",
        "bindingGeneration",
        "identityEpoch",
        "clearGeneration",
        "observationSequence",
    ] {
        if raw.get(key)? != receipt.get(key)? {
            return None;
        }
    }
    if let Some(namespace) = receipt.get("derivedCalibrationPatches") {
        let namespace = namespace.as_object()?;
        if namespace.len() > 64 {
            return None;
        }
        let mut shared: Option<Value> = None;
        for (window_id, stored) in namespace {
            if stored["schemaVersion"] != 1
                || stored["algorithmVersion"] != VERSION
                || stored["windowId"].as_str() != Some(window_id)
                || stored["nativeDigest"].as_str()
                    != Some(canonical_digest(&native_value(raw)).as_str())
            {
                return None;
            }
            let expected = format!("{}:{sequence}:{window_id}", receipt["runtimeId"].as_str()?);
            if stored["observationId"] != expected {
                return None;
            }
            let mut check = stored.clone();
            check.as_object_mut()?.remove("metadataDigest");
            if stored["metadataDigest"].as_str() != Some(canonical_digest(&check).as_str()) {
                return None;
            }
            let patch = ObservationPatch {
                receipt_id: 0,
                observation_id: expected,
                window_id: window_id.clone(),
                snapshot_fields: stored["snapshotFields"].clone(),
                window_fields: stored["windowFields"].clone(),
                structure_seal: None,
            };
            if !patch_shape(&patch)
                || shared
                    .as_ref()
                    .is_some_and(|old| old != &patch.snapshot_fields)
            {
                return None;
            }
            shared = Some(patch.snapshot_fields.clone());
            for (key, value) in patch.snapshot_fields.as_object()? {
                if receipt.get("derivedObservationProof").is_some()
                    && snapshot.get(key).is_some_and(|old| old != value)
                {
                    return None;
                }
                snapshot.insert(key.clone(), value.clone());
            }
            let buckets = snapshot.get_mut("buckets")?.as_array_mut()?;
            let mut found = false;
            for bucket in buckets {
                for kind in ["primary", "secondary"] {
                    if format!("{}:{kind}", bucket["id"].as_str()?) == *window_id {
                        let window = bucket.get_mut(kind)?.as_object_mut()?;
                        for (key, value) in patch.window_fields.as_object()? {
                            if receipt.get("derivedObservationProof").is_some()
                                && window.get(key).is_some_and(|old| old != value)
                            {
                                return None;
                            }
                            window.insert(key.clone(), value.clone());
                        }
                        found = true;
                    }
                }
            }
            if !found {
                return None;
            }
        }
    }
    snapshot.insert("persisted".into(), json!(true));
    Some(Value::Object(snapshot))
}

pub(super) fn native_snapshot_digest(value: &Value) -> String {
    canonical_digest(&native_value(value))
}

/// Revalidates the latest settled profile against current canonical query rows.
/// This proves only its exact half-open historical interval, never usage/modes
/// between that endpoint and now. Any new current/unknown-account row beyond
/// the endpoint closes the profile until a new official settled pair exists.
pub(super) fn usage_structure_current(
    snapshot: &Value,
    window_id: &str,
    usage: &[ResponseUsage],
) -> bool {
    let Some(window) = raw_window(snapshot, window_id) else {
        return false;
    };
    let structure = &window["usageStructureEvidence"];
    let calibration = &window["calibrationEvidence"];
    let independent = structure.get("basis").is_some();
    if independent
        && (window.get("calibrationEvidence").is_some()
            || !usage_structure::valid_basis(structure, &snapshot["clockEpoch"]))
    {
        return false;
    }
    let Ok(interval) = serde_json::from_value::<Interval>(structure["ledgerInterval"].clone())
    else {
        return false;
    };
    let Some(account) = text(&snapshot["accountScope"]) else {
        return false;
    };
    if usage.len() > MAX_USAGE
        || !interval.is_valid()
        || interval.lower < 0.0
        || interval.upper <= interval.lower
        || structure["verified"] != true
        || structure["source"] != "trusted-ledger"
        || structure["sourceId"] != snapshot["sourceId"]
        || structure["accountScope"] != account
        || structure["windowId"] != window_id
        || structure["windowInstanceId"] != window["windowInstanceId"]
        || structure["observationSequence"] != snapshot["observationSequence"]
        || decimal(&structure["fromSequence"]).is_none_or(|from| {
            decimal(&snapshot["observationSequence"]).is_none_or(|to| from >= to)
        })
        || (!independent && structure["reference"] != calibration["responseLedgerDigest"])
        || (!independent && calibration["coverage"] != "verified_complete")
        || (!independent && calibration["invalidated"] != false)
        || structure["nativeDigest"] != native_snapshot_digest(snapshot)
        || instant(&snapshot["quotaRead"]["factTimeLower"]) != Some(interval.upper)
        || instant(&snapshot["quotaRead"]["factTimeUpper"]) != Some(interval.upper)
    {
        return false;
    }
    let mut rows = Vec::new();
    for row in usage
        .iter()
        .filter(|row| snapshot["sourceId"] == row.source_id)
    {
        let Some(at) = instant(&json!(row.event_time)) else {
            return false;
        };
        if at >= interval.upper
            && (row.account_scope_id.is_none() || row.account_scope_id.as_deref() == Some(account))
        {
            return false;
        }
        if at >= interval.lower && at < interval.upper {
            if row.account_scope_id.as_deref() != Some(account) {
                return false;
            }
            rows.push(row);
        }
    }
    !rows.is_empty() && structure["reference"] == response_ledger_digest(&rows)
}

#[cfg(test)]
#[path = "calibration_assembler_tests.rs"]
mod tests;
