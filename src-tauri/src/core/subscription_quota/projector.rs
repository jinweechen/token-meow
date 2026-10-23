//! Read-only adapter from committed observation receipts and the trusted ledger.
//! Missing collector evidence stays missing; this module never manufactures a
//! live calibration, account coverage claim, or notification permit.
use super::super::{adapter::ResponseUsage, hash};
use super::{calibration::calibrate, evaluate::*, model::*};
use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

fn number(v: &Value) -> Option<f64> {
    v.as_f64().filter(|n| n.is_finite())
}
fn text(v: &Value) -> Option<&str> {
    v.as_str().filter(|s| !s.trim().is_empty())
}
fn decimal(v: &Value) -> Option<u64> {
    let s = v.as_str()?;
    let n = s.parse::<u64>().ok()?;
    (n.to_string() == s).then_some(n)
}
fn instant(v: &Value) -> Option<f64> {
    if let Some(n) = number(v) {
        return (n >= 0.0).then_some(n);
    }
    let t = DateTime::parse_from_rfc3339(v.as_str()?).ok()?;
    let n = t.timestamp() as f64 + t.timestamp_subsec_nanos() as f64 / 1e9;
    (n.is_finite() && n >= 0.0).then_some(n)
}
fn timestamp(value: Option<f64>, upper: bool) -> Value {
    let Some(value) = value else {
        return Value::Null;
    };
    let milliseconds = if upper {
        (value * 1000.0).ceil()
    } else {
        (value * 1000.0).floor()
    };
    if !milliseconds.is_finite() || milliseconds < 0.0 || milliseconds >= i64::MAX as f64 {
        return Value::Null;
    }
    DateTime::<Utc>::from_timestamp_millis(milliseconds as i64)
        .map(|t| json!(t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)))
        .unwrap_or(Value::Null)
}
fn bounded_time(read: &Value) -> Option<Interval> {
    let basis = text(&read["timeBasis"])?;
    if matches!(
        basis,
        "unknown" | "request_start_only" | "unbounded" | "request-start-fallback"
    ) {
        return None;
    }
    Interval::new(
        instant(&read["factTimeLower"])?,
        instant(&read["factTimeUpper"])?,
    )
}
fn recovery(v: &Value) -> RecoverySemantics {
    match v.as_str() {
        Some("fixed_reset" | "fixed" | "fixed-calendar") => RecoverySemantics::FixedReset,
        Some("anchored_reset" | "anchored" | "first-use-anchored") => {
            RecoverySemantics::AnchoredReset
        }
        Some("sliding") => RecoverySemantics::Sliding,
        Some("replenishing") => RecoverySemantics::Replenishing,
        Some("next_reset_only") => RecoverySemantics::NextResetOnly,
        _ => RecoverySemantics::Unknown,
    }
}
fn proven(snapshot: &Value) -> bool {
    snapshot["identityConsistency"] == "proven" && snapshot["identityEvidence"].is_object()
}
fn raw_windows(snapshot: &Value) -> Vec<(String, Value)> {
    let mut result = Vec::new();
    for bucket in snapshot["buckets"].as_array().into_iter().flatten() {
        let Some(id) = text(&bucket["id"]) else {
            continue;
        };
        for kind in ["primary", "secondary"] {
            if bucket[kind].is_object() {
                result.push((format!("{id}:{kind}"), bucket[kind].clone()));
            }
        }
    }
    result
}

fn build_sample(
    snapshot: &Value,
    id: &str,
    window: &Value,
    committed: bool,
) -> Result<NativeSample, Reason> {
    if !proven(snapshot) {
        return Err(Reason::IdentityUnproven);
    }
    if snapshot["observationOutcome"] != "success" || snapshot["status"] != "connected" {
        return Err(Reason::Discontinuous);
    }
    if !committed {
        return Err(Reason::NotPersisted);
    }
    let runtime = text(&snapshot["runtimeId"]).ok_or(Reason::InvalidScope)?;
    let source = text(&snapshot["sourceId"]).ok_or(Reason::InvalidScope)?;
    let binding = decimal(&snapshot["bindingGeneration"]).ok_or(Reason::InvalidScope)?;
    let epoch = decimal(&snapshot["identityEpoch"])
        .filter(|n| *n > 0)
        .ok_or(Reason::IdentityUnproven)?;
    let sequence = decimal(&snapshot["observationSequence"])
        .filter(|n| *n > 0)
        .ok_or(Reason::OutOfOrder)?;
    let quota = &snapshot["quotaRead"];
    if quota["outcome"] != "success" {
        return Err(Reason::Discontinuous);
    }
    let ttl_start = instant(&quota["freshnessAnchor"]).ok_or(Reason::TimeUnknown)?;
    let rule = text(&window["ruleVersion"]).unwrap_or("observed-rule-unknown");
    let entitlement = text(&snapshot["entitlementSegment"])
        .map(str::to_owned)
        .unwrap_or_else(|| {
            format!(
                "observed-plan:{}:{}",
                text(&snapshot["authMode"]).unwrap_or("unknown"),
                text(&snapshot["planType"]).unwrap_or("unknown")
            )
        });
    let domain = hash(
        json!([
            runtime,
            source,
            binding.to_string(),
            snapshot["clearGeneration"]
        ])
        .to_string()
        .as_bytes(),
    );
    let reset = window["resetsAt"].as_i64().filter(|n| *n >= 0);
    let period = if let Some(explicit) = text(&window["windowInstanceId"]) {
        explicit.to_owned()
    } else if let Some(reset) = reset {
        hash(
            format!(
                "{}|{}|{}|{}|{}",
                domain,
                snapshot["identityEpoch"],
                id,
                window["recovery"].as_str().unwrap_or("unknown"),
                reset
            )
            .as_bytes(),
        )
    } else {
        return Err(Reason::RecoveryUnknown);
    };
    let reset_time = if window["resetTimeLower"].is_null() && window["resetTimeUpper"].is_null() {
        reset.and_then(|t| Interval::exact(t as f64))
    } else {
        Some(
            Interval::new(
                instant(&window["resetTimeLower"]).ok_or(Reason::TimeUnknown)?,
                instant(&window["resetTimeUpper"]).ok_or(Reason::TimeUnknown)?,
            )
            .ok_or(Reason::TimeInvalid)?,
        )
    };
    let clock = text(&quota["clockEpoch"])
        .or_else(|| text(&snapshot["clockEpoch"]))
        .unwrap_or("");
    Ok(NativeSample {
        observation_id: format!("{runtime}:{sequence}:{id}"),
        sequence,
        scope: EvaluationScope {
            runtime_id: runtime.into(),
            source_id: source.into(),
            binding_generation: binding,
            identity_epoch: epoch,
            clear_generation: snapshot["clearGeneration"].to_string(),
            account_scope: text(&snapshot["accountScope"]).map(str::to_owned),
            entitlement_segment: entitlement,
            rule_version: rule.into(),
            pool_id: id.into(),
            window_instance_id: period,
        },
        percent: PercentReading {
            used_percent: number(&window["usedPercent"]).ok_or(Reason::InvalidNumber)?,
            rounding_step: number(&window["roundingStep"]),
            saturated: window["saturated"] == true,
            saturation_headroom_bounded: window["saturationHeadroomBounded"] == true,
        },
        fact_time: bounded_time(quota),
        ttl_start,
        persisted: true,
        identity_consistent: true,
        clock_epoch: clock.into(),
        reset_time,
        recovery: recovery(&window["recovery"]),
        discontinuity: snapshot["discontinuity"] == true,
    })
}

pub(super) fn receipt_snapshot(receipt: &Value) -> Option<Value> {
    super::calibration_assembler::hydrated_observation(receipt)
}

struct Record {
    sample: NativeSample,
    window: Value,
    segment: u64,
}

fn same_runtime_segment(a: &EvaluationScope, b: &EvaluationScope) -> bool {
    a.runtime_id == b.runtime_id
        && a.binding_generation == b.binding_generation
        && a.identity_epoch == b.identity_epoch
        && a.clear_generation == b.clear_generation
}

/// A provider period key must explicitly be stable across process starts.
/// The fallback native key deliberately contains runtimeId and cannot prove
/// that two restarts represent two independent billing periods.
fn stable_period(record: &Record) -> bool {
    let evidence = &record.window["windowInstanceEvidence"];
    text(&record.window["windowInstanceId"])
        == Some(record.sample.scope.window_instance_id.as_str())
        && evidence["verified"] == true
        && evidence["source"] == "official"
        && evidence["stableAcrossRuntimes"] == true
        && text(&evidence["windowInstanceId"])
            == Some(record.sample.scope.window_instance_id.as_str())
        && text(&evidence["reference"]).is_some()
}

fn current_remaining_reason(sample: &NativeSample, now: f64) -> Option<Reason> {
    if !now.is_finite() || now < 0.0 || sample.ttl_start > now {
        return Some(Reason::TimeInvalid);
    }
    let Some(time) = sample.fact_time else {
        return Some(Reason::TimeUnknown);
    };
    if !time.is_valid() || time.lower < 0.0 || time.upper > now {
        return Some(Reason::TimeInvalid);
    }
    let ttl = NativeParams::default().ttl_seconds;
    if now - sample.ttl_start > ttl || now - time.lower > ttl {
        return Some(Reason::Stale);
    }
    if sample.reset_time.is_some_and(|reset| now >= reset.lower) {
        return Some(Reason::WaitingForReset);
    }
    None
}

// Unknown provider quantization permits only an explicitly unbounded empirical
// capacity point. It never fills a capacity/remaining interval or a native ETA,
// and known-quantization results keep their interval as the sole guidance.
fn empirical_capacity(result: &CalibrationResult) -> Option<f64> {
    (result.quality == CalibrationQuality::Conditional
        && result.reasons.contains(&Reason::PrecisionUnknown)
        && result.capacity.is_none())
    .then_some(result.empirical_capacity)
    .flatten()
    .filter(|n| n.is_finite() && *n > 0.0 && *n <= 9_007_199_254_740_991.0)
}

/// A paired fact deletion or correction invalidates the old numerator evidence.
pub(super) fn response_ledger_digest(rows: &[&ResponseUsage]) -> String {
    let mut canonical: Vec<Value> = rows.iter().map(|row| json!({
        "sourceId":row.source_id,"namespace":row.namespace,"threadId":row.thread_id,
        "responseId":row.response_id,"eventTime":row.event_time,"observedModel":row.observed_model,
        "accountScopeId":row.account_scope_id,"counts":row.counts,"semanticsVersion":row.semantics_version,
        "adapterVersion":row.adapter_version,"revision":row.revision,"phase":row.phase
    })).collect();
    for value in &mut canonical {
        value.sort_all_objects();
    }
    canonical.sort_by_key(Value::to_string);
    hash(
        serde_json::to_string(&canonical)
            .expect("finite metric projection")
            .as_bytes(),
    )
}

fn pair_from_usage(
    start: &Record,
    end: &Record,
    usage: &[ResponseUsage],
) -> Result<CalibrationPair, Reason> {
    if start.segment != end.segment || start.sample.scope != end.sample.scope {
        return Err(Reason::Discontinuous);
    }
    let evidence = &end.window["calibrationEvidence"];
    if !evidence.is_object()
        || evidence["boundaryAligned"] != true
        || evidence["conflictsOrGaps"] != false
        || decimal(&evidence["ledgerRevision"]).is_none()
        || text(&evidence["responseLedgerDigest"]).is_none()
        || !evidence
            .get("effort")
            .is_some_and(|v| v.is_null() || text(v).is_some())
        || !evidence
            .get("speedMode")
            .is_some_and(|v| v.is_null() || text(v).is_some())
    {
        return Err(Reason::Unsettled);
    }
    let metering = text(&evidence["meteringVersion"]).ok_or(Reason::Unsettled)?;
    let coverage = match evidence["coverage"].as_str() {
        Some("verified_complete") if evidence["coverageEvidence"].is_object() => {
            CoverageEvidence::VerifiedComplete
        }
        Some("conditional_user_assumption") => CoverageEvidence::ConditionalUserAssumption,
        Some("known_incomplete") => return Err(Reason::ExternalUsageUncovered),
        _ => return Err(Reason::CoverageUnknown),
    };
    let identity = match evidence["identityEvidence"].as_str() {
        Some("verified_account") => IdentityEvidence::VerifiedAccount,
        Some("user_specified") => IdentityEvidence::UserSpecified,
        Some("runtime_only") => IdentityEvidence::RuntimeOnly,
        _ => return Err(Reason::IdentityUnproven),
    };
    let (a, b) = (
        start.sample.fact_time.ok_or(Reason::TimeUnknown)?,
        end.sample.fact_time.ok_or(Reason::TimeUnknown)?,
    );
    // This adapter has no billing-delay allocator. Uncertain boundaries require
    // a future separately qualified allocator, rather than silently dropping
    // boundary responses and pairing an incomplete numerator with full Δp.
    if !a.is_valid()
        || !b.is_valid()
        || a.lower != a.upper
        || b.lower != b.upper
        || b.lower <= a.upper
    {
        return Err(Reason::TimeUnknown);
    }
    if instant(&evidence["settledThrough"]).is_none_or(|t| t < b.upper) {
        return Err(Reason::Unsettled);
    }
    if start.sample.scope.rule_version == "observed-rule-unknown"
        || start
            .sample
            .scope
            .entitlement_segment
            .starts_with("observed-plan:")
    {
        return Err(Reason::ScopeChanged);
    }
    let mut tokens = 0_u64;
    let mut output = 0_u64;
    let mut input = 0_u64;
    let mut cache = 0_u64;
    let mut inputs = Vec::new();
    let mut models = BTreeMap::<String, u64>::new();
    let mut ids = BTreeSet::new();
    let mut paired_rows = Vec::new();
    for row in usage {
        if row.source_id != start.sample.scope.source_id {
            continue;
        }
        let at = instant(&json!(row.event_time)).ok_or(Reason::TimeUnknown)?;
        if at < a.lower || at >= b.lower {
            continue;
        }
        paired_rows.push(row);
        if row.semantics_version != metering
            || row.phase != "final"
            || row.adapter_version.is_empty()
        {
            return Err(Reason::ConflictsOrGaps);
        }
        if identity == IdentityEvidence::VerifiedAccount
            && (row.account_scope_id.is_none()
                || row.account_scope_id != start.sample.scope.account_scope)
        {
            return Err(Reason::IdentityUnproven);
        }
        let model = row
            .observed_model
            .as_ref()
            .filter(|m| !m.trim().is_empty() && m.as_str() != "unknown")
            .ok_or(Reason::ModelCoverageUnknown)?;
        let get = |metric: &super::super::adapter::Metric| {
            metric
                .value()
                .filter(|n| *n >= 0)
                .map(|n| n as u64)
                .ok_or(Reason::ConflictsOrGaps)
        };
        let i = get(&row.counts.input)?;
        let o = get(&row.counts.output)?;
        let t = get(&row.counts.total)?;
        let cr = get(&row.counts.cache_read)?;
        let cw = get(&row.counts.cache_write)?;
        let reasoning = get(&row.counts.reasoning)?;
        if i.checked_add(o) != Some(t) || cr.checked_add(cw).is_none_or(|c| c > i) || reasoning > o
        {
            return Err(Reason::ConflictsOrGaps);
        }
        if !ids.insert((&row.namespace, &row.response_id)) {
            return Err(Reason::ConflictsOrGaps);
        }
        let add = |sum: u64, n: u64| {
            sum.checked_add(n)
                .filter(|v| *v <= 9_007_199_254_740_991)
                .ok_or(Reason::InvalidNumber)
        };
        tokens = add(tokens, t)?;
        input = add(input, i)?;
        output = add(output, o)?;
        cache = add(cache, cr)?;
        inputs.push(i as f64);
        let slot = models.entry(model.clone()).or_default();
        *slot = add(*slot, t)?;
    }
    if evidence["responseLedgerDigest"].as_str()
        != Some(response_ledger_digest(&paired_rows).as_str())
    {
        return Err(Reason::EvidenceInvalidated);
    }
    if tokens == 0 || input == 0 || inputs.is_empty() {
        return Err(Reason::LowSignal);
    }
    inputs.sort_by(f64::total_cmp);
    let middle = inputs.len() / 2;
    let median_input = if inputs.len().is_multiple_of(2) {
        inputs[middle - 1] / 2.0 + inputs[middle] / 2.0
    } else {
        inputs[middle]
    };
    let holdout = evidence["holdout"] == true;
    if holdout
        && (text(&evidence["holdoutManifestId"]).is_none()
            || instant(&evidence["holdoutFrozenAt"]).is_none_or(|t| t > a.lower))
    {
        return Err(Reason::HoldoutInvalid);
    }
    Ok(CalibrationPair {
        pair_id: format!(
            "{}:{}",
            start.sample.observation_id, end.sample.observation_id
        ),
        start: start.sample.clone(),
        end: end.sample.clone(),
        tokens,
        profile: UsageProfile {
            models: models
                .into_iter()
                .map(|(model, amount)| ModelShare {
                    model,
                    share: amount as f64 / tokens as f64,
                })
                .collect(),
            all_models_attributed: true,
            effort: text(&evidence["effort"]).map(str::to_owned),
            speed_mode: text(&evidence["speedMode"]).map(str::to_owned),
            output_share: output as f64 / tokens as f64,
            cache_share: cache as f64 / input as f64,
            median_input_tokens: median_input,
        },
        identity_evidence: identity,
        coverage,
        settled: true,
        conflicts_or_gaps: false,
        holdout,
        evidence_invalidated: evidence["invalidated"] == true,
    })
}

/// Inputs are committed history in ascending commit/collector order. Current
/// memory may be unpersisted; it is never upgraded by an older history row.
pub(crate) fn projections(
    current: &Value,
    history: &[Value],
    usage: &[ResponseUsage],
    runtime_id: &str,
    now: f64,
) -> Value {
    // Current memory is kept as the original observed body. Derive its proof
    // fields only from the matching committed envelope; a failed/unpersisted
    // current read is never repaired by older historical metadata.
    let mut hydrated_current = current.clone();
    if current["persisted"] == true {
        if let Some(receipt) = history.iter().rev().find(|receipt| {
            [
                "runtimeId",
                "sourceId",
                "bindingGeneration",
                "identityEpoch",
                "clearGeneration",
                "observationSequence",
            ]
            .into_iter()
            .all(|key| receipt.get(key) == current.get(key))
                && super::calibration_assembler::native_snapshot_digest(&receipt["observation"])
                    == super::calibration_assembler::native_snapshot_digest(current)
        }) {
            match receipt_snapshot(receipt) {
                Some(snapshot) => hydrated_current = snapshot,
                None => {
                    hydrated_current["identityConsistency"] = json!("unproven");
                }
            }
        }
    }
    let current = &hydrated_current;
    let mut risks = Map::new();
    let mut calibrations = Vec::new();
    for (id, window) in raw_windows(current) {
        let current_sample = build_sample(current, &id, &window, current["persisted"] == true)
            .and_then(|s| {
                if s.scope.runtime_id == runtime_id {
                    Ok(s)
                } else {
                    Err(Reason::ScopeChanged)
                }
            });
        let mut records: Vec<Record> = Vec::new();
        let mut native = Vec::new();
        let mut segment = 0_u64;
        for receipt in history {
            let same_runtime = receipt["runtimeId"].as_str() == Some(runtime_id);
            let same_source = receipt["sourceId"] == current["sourceId"];
            if !same_source {
                continue;
            }
            let Some(snapshot) = receipt_snapshot(receipt) else {
                if same_runtime {
                    native.clear();
                }
                segment = segment.saturating_add(1);
                continue;
            };
            let Some((_, old_window)) = raw_windows(&snapshot)
                .into_iter()
                .find(|(key, _)| key == &id)
            else {
                if same_runtime {
                    native.clear();
                }
                segment = segment.saturating_add(1);
                continue;
            };
            match build_sample(&snapshot, &id, &old_window, true) {
                Ok(sample) => {
                    if same_runtime {
                        native.push(sample.clone());
                    }
                    records.push(Record {
                        sample,
                        window: old_window,
                        segment,
                    });
                }
                Err(_) => {
                    if same_runtime {
                        native.clear();
                    }
                    segment = segment.saturating_add(1);
                }
            }
        }
        let forecast = match &current_sample {
            Ok(sample) => evaluate_native(&native, sample, now, &NativeParams::default()),
            Err(reason) => NativeForecast {
                reasons: vec![*reason],
                ..NativeForecast::default()
            },
        };
        let state = if forecast.exhaustion_time.is_none() {
            "unknown"
        } else if forecast.notify_candidate {
            "risk"
        } else if forecast.safe_candidate {
            "safe"
        } else {
            "uncertain"
        };
        risks.insert(id.clone(), json!({"state":state,"label":match state {"risk"=>"按近期活动可能在重置前触限","safe"=>"按当前情景预计可持续至重置","uncertain"=>"可能在重置前触限，范围较宽",_=>"缺少可靠预测依据"},
            "exhaustsAtLower":timestamp(forecast.exhaustion_time.map(|t|t.lower),false),
            "exhaustsAtUpper":timestamp(forecast.exhaustion_time.map(|t|t.upper),true),
            "notifyEligible":false,"notifyCandidate":forecast.notify_candidate,"safeCandidate":forecast.safe_candidate,"reasons":forecast.reasons,"evidence":forecast}));
        let mut pairs = Vec::new();
        let mut pairing_reasons = Vec::new();
        // Bind the result to the current domain before calibration chooses its
        // first accepted training pair. Checking the last raw pair afterwards
        // can attach account A's accepted capacity to account B's current read.
        // An unproven current read may retain historical invalidation metadata,
        // but can never publish current numerical guidance.
        let target_scope = current_sample
            .as_ref()
            .ok()
            .map(|sample| &sample.scope)
            .or_else(|| records.last().map(|record| &record.sample.scope));
        let current_run_verified = target_scope.is_some_and(|scope| {
            records.iter().any(|record| {
                record.sample.scope.same_calibration_domain(scope)
                    && same_runtime_segment(&record.sample.scope, scope)
                    && record.window["calibrationEvidence"]["identityEvidence"]
                        == "verified_account"
            })
        });
        for record in records.iter().filter(|record| {
            target_scope.is_some_and(|scope| {
                record.sample.scope.same_calibration_domain(scope)
                    && record.sample.scope.clear_generation == scope.clear_generation
                    && (same_runtime_segment(&record.sample.scope, scope)
                        || (current_run_verified
                            && record.window["calibrationEvidence"]["identityEvidence"]
                                == "verified_account"))
            })
        }) {
            let Some(from) = decimal(&record.window["calibrationEvidence"]["fromSequence"]) else {
                continue;
            };
            let Some(start) = records.iter().find(|start| {
                start.sample.scope.runtime_id == record.sample.scope.runtime_id
                    && start.sample.scope == record.sample.scope
                    && start.segment == record.segment
                    && start.sample.sequence == from
                    && start.sample.sequence < record.sample.sequence
            }) else {
                pairing_reasons.push(Reason::Unsettled);
                continue;
            };
            match pair_from_usage(start, record, usage) {
                Ok(pair) => {
                    // A user-selected archive or local declaration is not a
                    // stable provider identity and cannot join process starts.
                    if pair.identity_evidence != IdentityEvidence::VerifiedAccount
                        && target_scope
                            .is_some_and(|scope| !same_runtime_segment(&pair.end.scope, scope))
                    {
                        continue;
                    }
                    pairs.push(pair);
                }
                Err(reason) => pairing_reasons.push(reason),
            }
        }
        let mut result = pairs.last().map(|pair| calibrate(&pairs, &pair.profile));
        if let Some(calibration) = result.as_mut() {
            let runtime_count = pairs
                .iter()
                .map(|pair| pair.start.scope.runtime_id.as_str())
                .collect::<BTreeSet<_>>()
                .len();
            let periods_proven = pairs.iter().all(|pair| {
                [&pair.start.observation_id, &pair.end.observation_id]
                    .into_iter()
                    .all(|id| {
                        records
                            .iter()
                            .find(|record| &record.sample.observation_id == id)
                            .is_some_and(stable_period)
                    })
            });
            if runtime_count > 1 && !periods_proven {
                // Covered pairs still support a conditional aggregate. Do not
                // count runtime-specific hashes as independent provider cycles.
                calibration.independent_periods = 0;
                if calibration.quality == CalibrationQuality::Validated {
                    calibration.quality = CalibrationQuality::Conditional;
                }
                calibration.reasons.push(Reason::InsufficientPeriods);
            }
            if pairing_reasons.contains(&Reason::EvidenceInvalidated) {
                calibration.quality = CalibrationQuality::Invalidated;
                calibration.capacity = None;
                calibration.empirical_capacity = None;
                calibration.reasons.push(Reason::EvidenceInvalidated);
            }
            let current_reason = match current_sample.as_ref() {
                Err(reason) => Some(*reason),
                Ok(sample) => pairs.last().and_then(|pair| {
                    if !pair.end.scope.same_calibration_domain(&sample.scope) {
                        Some(Reason::ScopeChanged)
                    } else if pair.identity_evidence != IdentityEvidence::VerifiedAccount
                        && !same_runtime_segment(&pair.end.scope, &sample.scope)
                    {
                        Some(Reason::IdentityUnproven)
                    } else {
                        None
                    }
                }),
            };
            if let Some(reason) = current_reason {
                calibration.quality = CalibrationQuality::Invalidated;
                calibration.capacity = None;
                calibration.empirical_capacity = None;
                calibration.reasons.push(reason);
            }
        }
        let remaining_reason = current_sample
            .as_ref()
            .ok()
            .and_then(|sample| current_remaining_reason(sample, now));
        let remaining = result.as_ref().and_then(|r| r.capacity).and_then(|c| {
            current_sample
                .as_ref()
                .ok()
                .filter(|_| remaining_reason.is_none())
                .filter(|current_sample| pairs.last().is_some_and(|p| p.end == **current_sample))
                .and_then(|sample| remaining_tokens(c, &sample.percent).ok())
        });
        let mut reasons = result
            .as_ref()
            .map_or_else(|| vec![Reason::InsufficientSamples], |r| r.reasons.clone());
        reasons.extend(pairing_reasons);
        if let Some(reason) = remaining_reason {
            reasons.push(reason);
        }
        if let Err(reason) = current_sample {
            reasons.push(reason);
        }
        if pairs.is_empty() {
            reasons.push(Reason::CoverageUnknown);
        }
        let profile = pairs.last().map(|p| &p.profile);
        let structure = profile.map_or_else(
            || "尚未建立配对画像".to_owned(),
            |p| {
                format!(
                    "{}；effort：{}；速度：{}；输出 {:.0}% / 缓存 {:.0}%；输入中位数 {:.0}",
                    p.models
                        .iter()
                        .map(|m| m.model.as_str())
                        .collect::<Vec<_>>()
                        .join(" + "),
                    p.effort.as_deref().unwrap_or("未知"),
                    p.speed_mode.as_deref().unwrap_or("未知"),
                    p.output_share * 100.0,
                    p.cache_share * 100.0,
                    p.median_input_tokens
                )
            },
        );
        let model = profile
            .filter(|p| p.models.len() == 1 && p.all_models_attributed)
            .map(|p| p.models[0].model.clone());
        let included = result
            .as_ref()
            .map(|r| r.included_pair_ids.iter().collect::<BTreeSet<_>>())
            .unwrap_or_default();
        let covered: Vec<_> = pairs
            .iter()
            .filter(|pair| included.contains(&pair.pair_id))
            .collect();
        let coverage = if covered
            .iter()
            .any(|pair| pair.coverage == CoverageEvidence::ConditionalUserAssumption)
        {
            "conditional_user_assumption"
        } else if !covered.is_empty()
            && covered
                .iter()
                .all(|pair| pair.coverage == CoverageEvidence::VerifiedComplete)
        {
            "verified_complete"
        } else {
            "unknown"
        };
        let deferred_pair = history.iter().any(|receipt| {
            receipt["storageEvidence"]["reason"] == "projection_budget_exceeded"
                && receipt["storageEvidence"]["deferredCalibrationWindows"]
                    .as_array()
                    .is_some_and(|windows| {
                        windows
                            .iter()
                            .any(|window| window.as_str() == Some(id.as_str()))
                    })
        });
        let mut card = json!({"id":id,"windowId":id,"model":model,"label":if model.is_some(){"已记录实际模型"}else{"当前模型组合"},
            "quality":result.as_ref().map(|r|r.quality).unwrap_or(CalibrationQuality::Learning),
            "capacityLower":result.as_ref().and_then(|r|r.capacity).map(|c|c.lower.floor()),
            "capacityUpper":result.as_ref().and_then(|r|r.capacity).map(|c|c.upper.ceil()),
            "empiricalCapacity":result.as_ref().and_then(empirical_capacity),
            "remainingLower":remaining.map(|c|c.lower.floor()),"remainingUpper":remaining.map(|c|c.upper.ceil()),
            "sampleCount":result.as_ref().map_or(0,|r|r.train_pairs),"cycleCount":result.as_ref().map_or(0,|r|r.independent_periods),
            "coverage":coverage,"structure":structure,"profileEvidence":profile,"reasons":reasons,"evidence":result,
            "inputPairs":covered});
        if deferred_pair {
            // A memory budget must never select only successful holdouts or
            // hide an excluded failure and thereby upgrade current guidance.
            card["quality"] = json!("learning");
            for field in [
                "capacityLower",
                "capacityUpper",
                "empiricalCapacity",
                "remainingLower",
                "remainingUpper",
            ] {
                card[field] = Value::Null;
            }
            card["reasons"]
                .as_array_mut()
                .unwrap()
                .push(json!("projection_budget_exceeded"));
            card["evidence"] = Value::Null;
            card["inputPairs"] = json!([]);
        }
        calibrations.push(card);
    }
    json!({"algorithmVersion":ALGORITHM_VERSION,"risks":risks,"calibrations":calibrations})
}

#[cfg(test)]
mod tests {
    use super::super::super::adapter::{Counts, Metric};
    use super::*;

    fn snapshot(seq: u64, at: f64, used: f64) -> Value {
        json!({"runtimeId":"test-runtime","sourceId":"test-source","bindingGeneration":"1",
            "clearGeneration":null,"identityEpoch":"1","observationSequence":seq.to_string(),
            "observationOutcome":"success","status":"connected","persisted":true,
            "identityConsistency":"proven","identityEvidence":{"kind":"fixed-fixture-only"},
            "accountScope":"fixed-account","authMode":"chatgpt","planType":"test-plan",
            "entitlementSegment":"fixed-entitlement","quotaRead":{"outcome":"success",
                "freshnessAnchor":at,"factTimeLower":at,"factTimeUpper":at,
                "timeBasis":"verified_on_demand_bounds","clockEpoch":"test-clock"},
            "buckets":[{"id":"shared","primary":{"usedPercent":used,"roundingStep":1.0,
                "resetsAt":10000,"recovery":"fixed_reset","ruleVersion":"fixed-rule"}}]})
    }
    fn receipt(snapshot: &Value) -> Value {
        let mut observation = snapshot.clone();
        observation["persisted"] = json!(false);
        json!({"documentKind":"subscription_observation","schemaVersion":1,
            "runtimeId":snapshot["runtimeId"],"sourceId":snapshot["sourceId"],
            "bindingGeneration":snapshot["bindingGeneration"],"clearGeneration":snapshot["clearGeneration"],
            "identityEpoch":snapshot["identityEpoch"],"observationSequence":snapshot["observationSequence"],
            "lastPersistedSequence":snapshot["observationSequence"],"outcome":"success","observation":observation})
    }
    fn response(index: usize, at: f64) -> ResponseUsage {
        let n = Metric::Valid;
        ResponseUsage {
            source_id: "test-source".into(),
            channel_id: "fixture-only".into(),
            namespace: "fixture".into(),
            thread_id: "thread".into(),
            response_id: format!("response-{index}"),
            turn_id: None,
            root_turn_id: None,
            event_time: timestamp(Some(at), false).as_str().unwrap().into(),
            collected_at: timestamp(Some(at), false).as_str().unwrap().into(),
            requested_model: Some("requested-different-model".into()),
            observed_model: Some("actual-model".into()),
            provider_id: None,
            account_scope_id: Some("fixed-account".into()),
            native_counts: BTreeMap::new(),
            counts: Counts {
                input: n(80000),
                output: n(20000),
                cache_read: n(24000),
                cache_write: n(0),
                reasoning: n(5000),
                total: n(100000),
            },
            semantics_version: "fixed-metering".into(),
            adapter_version: "fixture-adapter".into(),
            completeness: "source-recorded".into(),
            phase: "final".into(),
            revision: None,
            live: false,
        }
    }
    fn learning_input() -> (Value, Vec<Value>, Vec<ResponseUsage>) {
        let mut snapshots = Vec::new();
        let mut usage = Vec::new();
        for i in 0..4 {
            let mut value = snapshot(i + 1, i as f64 * 300.0, 10.0 + i as f64 * 10.0);
            if i > 0 {
                value["buckets"][0]["primary"]["calibrationEvidence"] = json!({
                    "fromSequence":i.to_string(),"ledgerRevision":i.to_string(),"meteringVersion":"fixed-metering",
                    "coverage":"conditional_user_assumption","identityEvidence":"verified_account",
                    "boundaryAligned":true,"conflictsOrGaps":false,"settledThrough":i as f64 * 300.0,
                    "effort":"medium","speedMode":null,"holdout":false});
                usage.push(response(i as usize, i as f64 * 300.0 - 150.0));
            }
            snapshots.push(value);
        }
        for i in 1..4 {
            snapshots[i]["buckets"][0]["primary"]["calibrationEvidence"]["responseLedgerDigest"] =
                json!(response_ledger_digest(&[&usage[i - 1]]));
        }
        (
            snapshots.last().unwrap().clone(),
            snapshots.iter().map(receipt).collect(),
            usage,
        )
    }

    fn account_input(
        runtime: &str,
        account: &str,
        epoch: &str,
        base: f64,
        responses_per_pair: usize,
    ) -> (Value, Vec<Value>, Vec<ResponseUsage>) {
        let mut snapshots = Vec::new();
        let mut usage = Vec::new();
        for i in 0..4 {
            let mut value = snapshot(i + 1, base + i as f64 * 300.0, 10.0 + i as f64 * 10.0);
            value["runtimeId"] = json!(runtime);
            value["accountScope"] = json!(account);
            value["identityEpoch"] = json!(epoch);
            if i > 0 {
                let mut rows = Vec::new();
                for j in 0..responses_per_pair {
                    let mut row = response(
                        i as usize * 10 + j,
                        base + i as f64 * 300.0 - 150.0 + j as f64,
                    );
                    row.account_scope_id = Some(account.into());
                    row.response_id = format!("{runtime}:{account}:{base}:{}", row.response_id);
                    rows.push(row);
                }
                value["buckets"][0]["primary"]["calibrationEvidence"] = json!({
                    "fromSequence":i.to_string(),"ledgerRevision":i.to_string(),"meteringVersion":"fixed-metering",
                    "coverage":"conditional_user_assumption","identityEvidence":"verified_account",
                    "boundaryAligned":true,"conflictsOrGaps":false,"settledThrough":base+i as f64*300.0,
                    "effort":"medium","speedMode":null,"holdout":false,
                    "responseLedgerDigest":response_ledger_digest(&rows.iter().collect::<Vec<_>>())});
                usage.extend(rows);
            }
            snapshots.push(value);
        }
        (
            snapshots.last().unwrap().clone(),
            snapshots.iter().map(receipt).collect(),
            usage,
        )
    }

    fn longitudinal_input(proven_periods: bool) -> (Value, Vec<Value>, Vec<ResponseUsage>) {
        let mut snapshots = Vec::new();
        let mut usage = Vec::new();
        for (period, trains, holdouts) in [(0, 7, 0), (1, 7, 0), (2, 6, 5)] {
            let base = period as f64 * 10000.0;
            for i in 0..=trains + holdouts {
                let mut value = snapshot(i + 1, base + i as f64 * 300.0, 10.0 + i as f64 * 5.0);
                value["runtimeId"] = json!(format!("runtime-{period}"));
                let w = &mut value["buckets"][0]["primary"];
                w["resetsAt"] = json!(base + 9000.0);
                // The production wire format uses integer reset seconds.
                w["resetsAt"] = json!((base + 9000.0) as i64);
                w["windowInstanceId"] = json!(format!("provider-cycle-{period}"));
                if proven_periods {
                    w["windowInstanceEvidence"] = json!({"verified":true,"source":"official",
                    "stableAcrossRuntimes":true,"windowInstanceId":format!("provider-cycle-{period}"),"reference":"fixed-fixture-only"});
                }
                if i > 0 {
                    let mut row = response(
                        period as usize * 100 + i as usize,
                        base + i as f64 * 300.0 - 150.0,
                    );
                    row.counts.input = Metric::Valid(40000);
                    row.counts.output = Metric::Valid(10000);
                    row.counts.total = Metric::Valid(50000);
                    row.counts.cache_read = Metric::Valid(10000);
                    w["calibrationEvidence"] = json!({"fromSequence":i.to_string(),"ledgerRevision":i.to_string(),
                        "meteringVersion":"fixed-metering","coverage":"verified_complete","coverageEvidence":{"kind":"fixed-fixture-only"},
                        "identityEvidence":"verified_account","boundaryAligned":true,"conflictsOrGaps":false,
                        "settledThrough":base+i as f64*300.0,"effort":"medium","speedMode":null,"holdout":i>trains,
                        "holdoutManifestId":"fixed-fixture-only","holdoutFrozenAt":base,
                        "responseLedgerDigest":response_ledger_digest(&[&row])});
                    usage.push(row);
                }
                snapshots.push(value);
            }
        }
        (
            snapshots.last().unwrap().clone(),
            snapshots.iter().map(receipt).collect(),
            usage,
        )
    }

    #[test]
    fn current_real_bridge_shape_stays_unknown_without_identity_time_or_precision_proof() {
        let mut current = snapshot(1, 900.0, 60.0);
        current["identityConsistency"] = json!("unproven");
        current.as_object_mut().unwrap().remove("identityEvidence");
        current["quotaRead"]["factTimeLower"] = Value::Null;
        current["quotaRead"]["factTimeUpper"] = Value::Null;
        current["buckets"][0]["primary"]
            .as_object_mut()
            .unwrap()
            .remove("roundingStep");
        let result = projections(&current, &[], &[], "test-runtime", 900.0);
        assert_eq!(result["risks"]["shared:primary"]["state"], "unknown");
        assert_eq!(result["risks"]["shared:primary"]["notifyEligible"], false);
        assert!(result["calibrations"][0]["capacityLower"].is_null());
        assert_eq!(result["calibrations"][0]["quality"], "learning");
    }

    #[test]
    fn committed_future_evidence_really_calls_native_algorithm_without_granting_a_permit() {
        let snapshots: Vec<_> = (0..6)
            .map(|i| snapshot(i + 1, i as f64 * 180.0, 69.0 + i as f64 * 6.0))
            .collect();
        let history: Vec<_> = snapshots.iter().map(receipt).collect();
        let result = projections(
            snapshots.last().unwrap(),
            &history,
            &[],
            "test-runtime",
            900.0,
        );
        let risk = &result["risks"]["shared:primary"];
        assert_eq!(risk["state"], "risk");
        assert_eq!(risk["notifyCandidate"], true);
        assert_eq!(risk["notifyEligible"], false);
        assert_eq!(risk["evidence"]["sampleCount"], 6);
        let lower = risk["evidence"]["etaSeconds"]["lower"].as_f64().unwrap();
        assert!((lower - 450.0 / 31.0).abs() < 1e-9);
        assert!(risk["exhaustsAtLower"]
            .as_str()
            .unwrap()
            .starts_with("1970-01-01T00:15:"));
    }

    #[test]
    fn failed_or_unversioned_history_breaks_the_live_continuous_segment() {
        let snapshots: Vec<_> = (0..6)
            .map(|i| snapshot(i + 1, i as f64 * 180.0, 69.0 + i as f64 * 6.0))
            .collect();
        let mut history: Vec<_> = snapshots.iter().map(receipt).collect();
        history.push(json!({"documentKind":"subscription_observation","schemaVersion":1,
            "runtimeId":"test-runtime","sourceId":"test-source","outcome":"failure","observationSequence":"7"}));
        let current = snapshot(8, 1080.0, 99.0);
        history.push(receipt(&current));
        let result = projections(&current, &history, &[], "test-runtime", 1080.0);
        assert_eq!(result["risks"]["shared:primary"]["state"], "unknown");
        assert_eq!(result["risks"]["shared:primary"]["notifyCandidate"], false);
    }

    #[test]
    fn trusted_usage_is_projected_into_real_calibration_not_a_prefilled_token_constant() {
        let (current, history, usage) = learning_input();
        let result = projections(&current, &history, &usage, "test-runtime", 900.0);
        let calibration = &result["calibrations"][0];
        assert_eq!(calibration["quality"], "conditional");
        assert_eq!(calibration["sampleCount"], 3);
        assert_eq!(calibration["model"], "actual-model");
        assert_eq!(calibration["capacityLower"], 909090.0);
        assert_eq!(calibration["capacityUpper"], 1111112.0);
        assert!(calibration["remainingLower"].as_f64().unwrap() > 0.0);
        assert_eq!(calibration["coverage"], "conditional_user_assumption");
    }

    #[test]
    fn requested_model_missing_effort_or_unknown_counts_cannot_fill_actual_profile() {
        let (current, history, mut usage) = learning_input();
        for row in &mut usage {
            row.observed_model = None;
        }
        let result = projections(&current, &history, &usage, "test-runtime", 900.0);
        assert_eq!(result["calibrations"][0]["quality"], "learning");
        assert!(result["calibrations"][0]["capacityLower"].is_null());
        let (mut current, mut history, usage) = learning_input();
        current["buckets"][0]["primary"]["calibrationEvidence"]
            .as_object_mut()
            .unwrap()
            .remove("effort");
        *history.last_mut().unwrap() = receipt(&current);
        assert_eq!(
            projections(&current, &history, &usage, "test-runtime", 900.0)["calibrations"][0]
                ["quality"],
            "learning"
        );
        let (current, history, mut usage) = learning_input();
        usage[0].counts.cache_read = Metric::Unknown;
        assert_eq!(
            projections(&current, &history, &usage, "test-runtime", 900.0)["calibrations"][0]
                ["quality"],
            "learning"
        );
    }

    #[test]
    fn uncertain_billing_boundaries_do_not_drop_events_to_create_a_capacity_sample() {
        let (current, mut history, usage) = learning_input();
        history[1]["observation"]["quotaRead"]["factTimeUpper"] = json!(303.8);
        let result = projections(&current, &history, &usage, "test-runtime", 900.0);
        assert_eq!(result["calibrations"][0]["quality"], "learning");
        assert!(result["calibrations"][0]["capacityLower"].is_null());
    }

    #[test]
    fn revoked_identity_invalidates_guidance_and_new_account_starts_its_own_learning() {
        let (mut current, history, usage) = learning_input();
        current["identityConsistency"] = json!("unproven");
        let result = projections(&current, &history, &usage, "test-runtime", 900.0);
        assert_eq!(result["calibrations"][0]["quality"], "invalidated");
        assert!(result["calibrations"][0]["capacityLower"].is_null());
        current["identityConsistency"] = json!("proven");
        current["accountScope"] = json!("different-account");
        let result = projections(&current, &history, &usage, "test-runtime", 900.0);
        assert_eq!(result["calibrations"][0]["quality"], "learning");
        assert_eq!(result["calibrations"][0]["sampleCount"], 0);
        assert!(result["calibrations"][0]["capacityLower"].is_null());
        assert!(result["calibrations"][0]["remainingLower"].is_null());
    }

    #[test]
    fn unpersisted_current_memory_is_not_upgraded_by_a_persisted_old_receipt() {
        let snapshots: Vec<_> = (0..6)
            .map(|i| snapshot(i + 1, i as f64 * 180.0, 69.0 + i as f64 * 6.0))
            .collect();
        let history: Vec<_> = snapshots.iter().map(receipt).collect();
        let mut current = snapshots.last().unwrap().clone();
        current["persisted"] = json!(false);
        let result = projections(&current, &history, &[], "test-runtime", 900.0);
        assert_eq!(result["risks"]["shared:primary"]["state"], "unknown");
        assert_eq!(result["risks"]["shared:primary"]["notifyCandidate"], false);
    }

    #[test]
    fn deleted_or_revised_paired_ledger_evidence_does_not_retrain_old_percent_deltas() {
        let (current, history, mut usage) = learning_input();
        usage.remove(0);
        let result = projections(&current, &history, &usage, "test-runtime", 900.0);
        assert_eq!(result["calibrations"][0]["quality"], "invalidated");
        assert!(result["calibrations"][0]["capacityLower"].is_null());
        let (current, history, mut usage) = learning_input();
        usage[0].counts.input = Metric::Valid(70000);
        usage[0].counts.total = Metric::Valid(90000);
        let result = projections(&current, &history, &usage, "test-runtime", 900.0);
        assert_eq!(result["calibrations"][0]["quality"], "invalidated");
        assert!(result["calibrations"][0]["capacityLower"].is_null());
        assert!(result["calibrations"][0]["reasons"]
            .as_array()
            .unwrap()
            .contains(&json!("evidence_invalidated")));
    }

    #[test]
    fn current_account_selects_its_own_training_domain_before_calibration() {
        let (_, mut history, mut usage) = account_input("runtime-a", "account-a", "1", 0.0, 1);
        let (current, new_history, new_usage) =
            account_input("runtime-a", "account-b", "2", 1000.0, 2);
        history.extend(new_history);
        usage.extend(new_usage);
        let result = projections(&current, &history, &usage, "runtime-a", 1900.0);
        let c = &result["calibrations"][0];
        assert_eq!(c["quality"], "conditional");
        assert_eq!(c["sampleCount"], 3);
        assert_eq!(c["capacityLower"], 1818181.0);
        assert_eq!(c["capacityUpper"], 2222223.0);
        assert!(c["remainingLower"].as_f64().unwrap() > 1_000_000.0);
        // Corrections to a different account's ledger cannot poison this one.
        usage.remove(0);
        assert_eq!(
            projections(&current, &history, &usage, "runtime-a", 1900.0)["calibrations"][0]
                ["quality"],
            "conditional"
        );
    }

    #[test]
    fn new_account_with_one_pair_cannot_reuse_previous_accounts_three_pairs() {
        let (_, mut history, mut usage) = account_input("runtime-a", "account-a", "1", 0.0, 1);
        let (_, new_history, new_usage) = account_input("runtime-a", "account-b", "2", 1000.0, 2);
        history.extend(new_history[..2].iter().cloned());
        usage.extend(new_usage);
        let current = receipt_snapshot(history.last().unwrap()).unwrap();
        let result = projections(&current, &history, &usage, "runtime-a", 1300.0);
        assert_eq!(result["calibrations"][0]["quality"], "learning");
        assert_eq!(result["calibrations"][0]["sampleCount"], 1);
        assert!(result["calibrations"][0]["capacityLower"].is_null());
    }

    #[test]
    fn failure_without_observation_breaks_an_otherwise_admissible_native_history() {
        let snapshots: Vec<_> = (0..6)
            .map(|i| {
                snapshot(
                    if i < 3 { i + 1 } else { i + 2 },
                    i as f64 * 180.0,
                    69.0 + i as f64 * 6.0,
                )
            })
            .collect();
        let successful: Vec<_> = snapshots.iter().map(receipt).collect();
        assert_eq!(
            projections(
                snapshots.last().unwrap(),
                &successful,
                &[],
                "test-runtime",
                900.0
            )["risks"]["shared:primary"]["state"],
            "risk"
        );
        let mut history = successful[..3].to_vec();
        history.push(json!({"documentKind":"subscription_observation","schemaVersion":1,"runtimeId":"test-runtime",
            "sourceId":"test-source","outcome":"failure","observationSequence":"4"}));
        history.extend(successful[3..].iter().cloned());
        let result = projections(
            snapshots.last().unwrap(),
            &history,
            &[],
            "test-runtime",
            900.0,
        );
        assert_eq!(result["risks"]["shared:primary"]["state"], "unknown");
        assert!(result["risks"]["shared:primary"]["exhaustsAtLower"].is_null());
    }

    #[test]
    fn only_verified_account_pairs_may_aggregate_across_runtime_starts() {
        let (_, mut history, mut usage) =
            account_input("runtime-old", "fixed-account", "1", 0.0, 1);
        let (current, new_history, new_usage) =
            account_input("runtime-new", "fixed-account", "1", 1000.0, 1);
        history.extend(new_history[..2].iter().cloned());
        usage.extend(new_usage);
        let current = receipt_snapshot(history.last().unwrap()).unwrap_or(current);
        let result = projections(&current, &history, &usage, "runtime-new", 1300.0);
        assert_eq!(result["calibrations"][0]["quality"], "conditional");
        assert_eq!(result["calibrations"][0]["sampleCount"], 4);
        assert_eq!(result["calibrations"][0]["cycleCount"], 0);
        for evidence in ["runtime_only", "user_specified"] {
            let mut unverified = history.clone();
            for receipt in &mut unverified {
                if receipt["observation"]["buckets"][0]["primary"]["calibrationEvidence"]
                    .is_object()
                {
                    receipt["observation"]["buckets"][0]["primary"]["calibrationEvidence"]
                        ["identityEvidence"] = json!(evidence);
                }
            }
            let current = receipt_snapshot(unverified.last().unwrap()).unwrap();
            let result = projections(&current, &unverified, &usage, "runtime-new", 1300.0);
            assert_eq!(
                result["calibrations"][0]["quality"], "learning",
                "{evidence}"
            );
            assert_eq!(result["calibrations"][0]["sampleCount"], 1, "{evidence}");
        }
    }

    #[test]
    fn cross_runtime_validation_needs_real_period_identity_not_runtime_hashes() {
        let (current, history, usage) = longitudinal_input(true);
        let result = projections(&current, &history, &usage, "runtime-2", 23300.0);
        let c = &result["calibrations"][0];
        assert_eq!(c["quality"], "validated");
        assert_eq!(c["sampleCount"], 20);
        assert_eq!(c["cycleCount"], 3);
        assert_eq!(c["evidence"]["holdoutPairs"], 5);
        let (current, history, usage) = longitudinal_input(false);
        let result = projections(&current, &history, &usage, "runtime-2", 23300.0);
        assert_eq!(result["calibrations"][0]["quality"], "conditional");
        assert_eq!(result["calibrations"][0]["cycleCount"], 0);
        assert!(result["calibrations"][0]["reasons"]
            .as_array()
            .unwrap()
            .contains(&json!("insufficient_periods")));
    }

    #[test]
    fn verified_old_account_cannot_supply_a_current_runtime_only_identity_link() {
        let (_, mut history, mut usage) =
            account_input("runtime-old", "fixed-account", "1", 0.0, 1);
        let (_, mut new_history, new_usage) =
            account_input("runtime-new", "fixed-account", "1", 1000.0, 1);
        for receipt in &mut new_history {
            if receipt["observation"]["buckets"][0]["primary"]["calibrationEvidence"].is_object() {
                receipt["observation"]["buckets"][0]["primary"]["calibrationEvidence"]
                    ["identityEvidence"] = json!("runtime_only");
            }
        }
        history.extend(new_history[..2].iter().cloned());
        usage.extend(new_usage);
        let current = receipt_snapshot(history.last().unwrap()).unwrap();
        // A revision of the old runtime is also outside the selected evidence.
        usage.remove(0);
        let result = projections(&current, &history, &usage, "runtime-new", 1300.0);
        assert_eq!(result["calibrations"][0]["quality"], "learning");
        assert_eq!(result["calibrations"][0]["sampleCount"], 1);
        assert!(result["calibrations"][0]["capacityLower"].is_null());
    }

    #[test]
    fn stale_current_percent_does_not_publish_remaining_tokens_from_old_capacity() {
        let (current, history, usage) = learning_input();
        let result = projections(&current, &history, &usage, "test-runtime", 1141.0);
        let c = &result["calibrations"][0];
        assert_eq!(c["quality"], "conditional");
        assert!(c["capacityLower"].is_number());
        assert!(c["remainingLower"].is_null());
        assert!(c["reasons"].as_array().unwrap().contains(&json!("stale")));
    }

    #[test]
    fn deferred_pair_cannot_select_only_successful_holdouts_and_upgrade_capacity() {
        let (current, mut history, usage) = longitudinal_input(true);
        let qualified = projections(&current, &history, &usage, "runtime-2", 23300.0);
        assert_eq!(qualified["calibrations"][0]["quality"], "validated");
        history.insert(0,json!({"documentKind":"subscription_observation","schemaVersion":1,
            "sourceId":"test-source","runtimeId":"older-omitted-period","observationSequence":"1",
            "storageEvidence":{"available":false,"reason":"projection_budget_exceeded","evidenceRemoved":false,
                "deferredCalibrationWindows":["shared:primary"]},"observation":null}));
        let partial = projections(&current, &history, &usage, "runtime-2", 23300.0);
        let card = &partial["calibrations"][0];
        assert_eq!(card["quality"], "learning");
        assert!(card["capacityLower"].is_null());
        assert!(card["remainingLower"].is_null());
        assert!(card["empiricalCapacity"].is_null());
        assert!(card["inputPairs"].as_array().unwrap().is_empty());
        assert!(card["reasons"]
            .as_array()
            .unwrap()
            .contains(&json!("projection_budget_exceeded")));
    }

    fn unknown_precision_input() -> (Value, Vec<Value>, Vec<ResponseUsage>) {
        let (mut current, mut history, usage) = learning_input();
        current["buckets"][0]["primary"]["roundingStep"] = Value::Null;
        for receipt in &mut history {
            receipt["observation"]["buckets"][0]["primary"]["roundingStep"] = Value::Null;
        }
        (current, history, usage)
    }

    #[test]
    fn unknown_quantization_exposes_only_a_conditional_empirical_capacity_point() {
        let (current, history, usage) = unknown_precision_input();
        let projected = projections(&current, &history, &usage, "test-runtime", 900.0);
        let card = &projected["calibrations"][0];
        assert_eq!(card["quality"], "conditional");
        assert_eq!(card["empiricalCapacity"], 1_000_000.0);
        for field in ["capacityLower", "capacityUpper", "remainingLower", "remainingUpper"] {
            assert!(card[field].is_null(), "{field}: {card}");
        }
        assert!(card["reasons"].as_array().unwrap().contains(&json!("precision_unknown")));
        let risk = &projected["risks"]["shared:primary"];
        assert_eq!(risk["state"], "unknown");
        assert!(risk["exhaustsAtLower"].is_null() && risk["exhaustsAtUpper"].is_null());
        assert_eq!(risk["notifyEligible"], false);
        assert_eq!(risk["notifyCandidate"], false);
    }

    #[test]
    fn empirical_capacity_requires_unknown_precision_and_a_finite_positive_safe_point() {
        let (current, history, usage) = unknown_precision_input();
        let projected = projections(&current, &history, &usage, "test-runtime", 900.0);
        let result: CalibrationResult = serde_json::from_value(projected["calibrations"][0]["evidence"].clone()).unwrap();
        assert_eq!(empirical_capacity(&result), Some(1_000_000.0));
        for value in [None, Some(0.0), Some(-1.0), Some(f64::NAN), Some(f64::INFINITY), Some(9_007_199_254_740_992.0)] {
            let mut bad = result.clone();
            bad.empirical_capacity = value;
            assert!(empirical_capacity(&bad).is_none(), "{value:?}");
        }
        for quality in [CalibrationQuality::Learning, CalibrationQuality::Validated, CalibrationQuality::Invalidated] {
            let mut bad = result.clone();
            bad.quality = quality;
            assert!(empirical_capacity(&bad).is_none(), "{quality:?}");
        }
        let mut known = result.clone();
        known.reasons.retain(|reason| *reason != Reason::PrecisionUnknown);
        assert!(empirical_capacity(&known).is_none());
        known.reasons.push(Reason::PrecisionUnknown);
        known.capacity = Interval::exact(1_000_000.0);
        assert!(empirical_capacity(&known).is_none(), "a known interval stays the sole bounded guidance");
    }

    #[test]
    fn known_quantization_keeps_intervals_without_an_extra_empirical_point() {
        let (current, history, usage) = learning_input();
        let projected = projections(&current, &history, &usage, "test-runtime", 900.0);
        let card = &projected["calibrations"][0];
        assert!(card["capacityLower"].is_number());
        assert!(card["empiricalCapacity"].is_null());
        let (current, history, usage) = longitudinal_input(true);
        let projected = projections(&current, &history, &usage, "runtime-2", 23300.0);
        assert_eq!(projected["calibrations"][0]["quality"], "validated");
        assert!(projected["calibrations"][0]["empiricalCapacity"].is_null());
    }

    #[test]
    fn many_unknown_precision_pairs_never_promote_a_point_to_validated_or_a_remaining_interval() {
        let (mut current, mut history, usage) = longitudinal_input(true);
        current["buckets"][0]["primary"]["roundingStep"] = Value::Null;
        for receipt in &mut history {
            receipt["observation"]["buckets"][0]["primary"]["roundingStep"] = Value::Null;
        }
        let projected = projections(&current, &history, &usage, "runtime-2", 23300.0);
        let card = &projected["calibrations"][0];
        assert_eq!(card["sampleCount"], 20);
        assert_eq!(card["cycleCount"], 3);
        assert_eq!(card["quality"], "conditional");
        assert_eq!(card["empiricalCapacity"], 1_000_000.0);
        for field in ["capacityLower", "capacityUpper", "remainingLower", "remainingUpper"] {
            assert!(card[field].is_null());
        }
    }

    #[test]
    fn removed_scope_zero_quantum_or_deferred_evidence_cannot_publish_an_empirical_point() {
        for mutation in ["identity", "zero-quantum", "deferred"] {
            let (mut current, mut history, usage) = unknown_precision_input();
            match mutation {
                "identity" => current["identityConsistency"] = json!("unproven"),
                "zero-quantum" => {
                    current["buckets"][0]["primary"]["roundingStep"] = json!(0);
                    for receipt in &mut history { receipt["observation"]["buckets"][0]["primary"]["roundingStep"] = json!(0); }
                }
                "deferred" => history.insert(0, json!({"documentKind":"subscription_observation","schemaVersion":1,
                    "sourceId":"test-source","runtimeId":"older-omitted-period","observationSequence":"1",
                    "storageEvidence":{"available":false,"reason":"projection_budget_exceeded","evidenceRemoved":false,
                        "deferredCalibrationWindows":["shared:primary"]},"observation":null})),
                _ => unreachable!(),
            }
            let projected = projections(&current, &history, &usage, "test-runtime", 900.0);
            assert!(projected["calibrations"][0]["empiricalCapacity"].is_null(), "{mutation}: {projected}");
        }
    }
}
