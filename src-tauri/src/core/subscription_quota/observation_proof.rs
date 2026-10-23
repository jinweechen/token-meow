//! Admission of adapter attestations before a receipt exists. These are internal
//! Rust inputs, never frontend DTOs. Original vendor fields stay in the raw body;
//! protected metadata is frozen separately by `seal_receipt`.
use super::super::model::{EvaluationScope, Interval, PercentReading};
use super::{
    canonical_digest, decimal, instant, native_snapshot_digest, raw_window, text, valid_text,
    AdapterQuotaProof,
};
use crate::budget_clock::Reading;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const VERSION: &str = "adapter-observation-proof-v1";
const NAMESPACE: &str = "derivedObservationProof";
const MAX_PROOFS: usize = 200;
const MAX_BYTES: usize = 256 * 1024;
// The storage envelope has a 512 KiB hard limit. Leave 64 KiB for Core's
// duplicated dynamic header; this guard runs only for a typed positive proof.
const MAX_BODY_WITH_PROOF: usize = 448 * 1024;
const SNAPSHOT_FIELDS: &[&str] = &[
    "identityEvidence",
    "entitlementSegment",
    "entitlementEvidence",
    "clockEpoch",
];
const WINDOW_FIELDS: &[&str] = &[
    "roundingStep",
    "saturated",
    "saturationHeadroomBounded",
    "ruleVersion",
    "windowInstanceId",
    "recovery",
    "windowInstanceEvidence",
    "resetEvidence",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ResetTransition {
    SameWindow,
    NewWindow,
}

/// Both native endpoints and the adapter's official transition contract are
/// required. A changed reset timestamp or UI JSON cannot construct this proof.
#[derive(Clone, Debug)]
pub(crate) struct AdapterResetTransitionProof {
    pub adapter_id: String,
    pub reference: String,
    pub scope: EvaluationScope,
    pub sequence: u64,
    pub native_digest: String,
    pub previous_sequence: u64,
    pub previous_native_digest: String,
    pub previous_window_instance_id: String,
    pub current_window_instance_id: String,
    pub transition: ResetTransition,
    pub next_reset: Option<Interval>,
    pub actual_boundary: Option<Interval>,
    pub final_used_percent: Option<f64>,
}

fn strip(snapshot: &mut Value) {
    if let Some(object) = snapshot.as_object_mut() {
        object.remove(NAMESPACE);
        for key in SNAPSHOT_FIELDS {
            object.remove(*key);
        }
        if let Some(buckets) = object.get_mut("buckets").and_then(Value::as_array_mut) {
            for bucket in buckets {
                for kind in ["primary", "secondary"] {
                    if let Some(window) = bucket.get_mut(kind).and_then(Value::as_object_mut) {
                        for key in WINDOW_FIELDS.iter().copied().chain([
                            "calibrationEvidence",
                            "usageStructureEvidence",
                            "resetTimeLower",
                            "resetTimeUpper",
                        ]) {
                            window.remove(key);
                        }
                    }
                }
            }
        }
    }
}

fn scope_matches(snapshot: &Value, scope: &EvaluationScope) -> bool {
    scope.is_valid()
        && scope.identity_epoch > 0
        && scope.account_scope.as_deref().is_some_and(valid_text)
        && [
            scope.runtime_id.as_str(),
            scope.source_id.as_str(),
            scope.entitlement_segment.as_str(),
            scope.rule_version.as_str(),
            scope.pool_id.as_str(),
            scope.window_instance_id.as_str(),
        ]
        .into_iter()
        .all(valid_text)
        && text(&snapshot["runtimeId"]) == Some(scope.runtime_id.as_str())
        && text(&snapshot["sourceId"]) == Some(scope.source_id.as_str())
        && decimal(&snapshot["bindingGeneration"]) == Some(scope.binding_generation)
        && decimal(&snapshot["identityEpoch"]) == Some(scope.identity_epoch)
        && snapshot["clearGeneration"].to_string() == scope.clear_generation
        && text(&snapshot["accountScope"]) == scope.account_scope.as_deref()
}

fn percent_valid(percent: &PercentReading) -> bool {
    percent.used_percent.is_finite()
        && (0.0..=100.0).contains(&percent.used_percent)
        && (!percent.saturated || percent.used_percent == 100.0)
        && percent
            .rounding_step
            .is_none_or(|q| q.is_finite() && q > 0.0 && q <= 100.0)
}

fn unique_windows(snapshot: &Value) -> bool {
    let Some(buckets) = snapshot["buckets"].as_array() else {
        return false;
    };
    let mut ids = BTreeSet::new();
    for bucket in buckets {
        let Some(id) = text(&bucket["id"]) else {
            return false;
        };
        for kind in ["primary", "secondary"] {
            if bucket[kind].is_object() && !ids.insert(format!("{id}:{kind}")) {
                return false;
            }
        }
    }
    !ids.is_empty() && ids.len() <= MAX_PROOFS
}

fn proof_metadata(
    snapshot: &Value,
    proof: &AdapterQuotaProof,
    at: &Reading,
) -> Option<(Value, Value)> {
    let now = at.utc.timestamp() as f64 + f64::from(at.utc.timestamp_subsec_nanos()) / 1e9;
    if !scope_matches(snapshot, &proof.scope)
        || !unique_windows(snapshot)
        || at.mono_ms == u64::MAX
        || decimal(&snapshot["observationSequence"]) != Some(proof.sequence)
        || proof.sequence == 0
        || snapshot["identityConsistency"] != "proven"
        || snapshot["status"] != "connected"
        || snapshot["observationOutcome"] != "success"
        || native_snapshot_digest(snapshot) != proof.native_digest
        || [
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
        || !percent_valid(&proof.percent)
        || !proof.reset_time.is_valid()
        || proof.reset_time.lower != proof.reset_time.upper
        || proof.reset_time.lower < 0.0
        || !proof.recovery.supports_single_reset_projection()
        || !proof.fact_time.is_valid()
        || proof.fact_time.lower < 0.0
        || proof.fact_time.upper > now
        || !proof.ttl_start.is_finite()
        || proof.ttl_start < 0.0
        || proof.ttl_start > now
    {
        return None;
    }
    let window = raw_window(snapshot, &proof.scope.pool_id)?;
    if window["usedPercent"].as_f64() != Some(proof.percent.used_percent)
        || window["resetsAt"].as_i64().map(|n| n as f64) != Some(proof.reset_time.lower)
        || instant(&snapshot["quotaRead"]["factTimeLower"]) != Some(proof.fact_time.lower)
        || instant(&snapshot["quotaRead"]["factTimeUpper"]) != Some(proof.fact_time.upper)
        || instant(&snapshot["quotaRead"]["freshnessAnchor"]) != Some(proof.ttl_start)
        || !matches!(
            snapshot["quotaRead"]["timeBasis"].as_str(),
            Some("verified_on_demand_bounds" | "official_event_time" | "adapter_billing_clock")
        )
    {
        return None;
    }
    let start = instant(&snapshot["readStartedAt"])?;
    let finish = instant(&snapshot["readFinishedAt"])?;
    if start > finish || finish > now {
        return None;
    }
    for kind in ["accountRead", "quotaRead"] {
        let read = &snapshot[kind];
        let a = instant(&read["requestStartedAt"])?;
        let b = instant(&read["responseReceivedAt"])?;
        let anchor = instant(&read["freshnessAnchor"])?;
        if read["outcome"] != "success"
            || a < start
            || a > b
            || b > finish
            || anchor > b
            || now - anchor >= 240.0
        {
            return None;
        }
        if kind == "quotaRead"
            && (proof.fact_time.upper > b
                || (read["timeBasis"] == "verified_on_demand_bounds" && proof.fact_time.lower < a))
        {
            return None;
        }
    }
    let scope = &proof.scope;
    Some((
        json!({"identityEvidence":{"adapterId":proof.adapter_id,"reference":proof.stable_identity_reference},
        "entitlementSegment":scope.entitlement_segment,
        "entitlementEvidence":{"verified":true,"source":"official","segmentId":scope.entitlement_segment,"ruleVersion":scope.rule_version,"reference":proof.entitlement_reference},"clockEpoch":proof.clock_epoch}),
        json!({"roundingStep":proof.percent.rounding_step,"saturated":proof.percent.saturated,"saturationHeadroomBounded":proof.percent.saturation_headroom_bounded,
        "ruleVersion":scope.rule_version,"windowInstanceId":scope.window_instance_id,"recovery":proof.recovery,
        "windowInstanceEvidence":{"verified":true,"source":"official","stableAcrossRuntimes":proof.period_stable_across_runtimes,"windowInstanceId":scope.window_instance_id,"reference":proof.period_reference}}),
    ))
}

fn reset_metadata(
    current: &Value,
    previous: Option<&Value>,
    native: &AdapterQuotaProof,
    reset: &AdapterResetTransitionProof,
    at: &Reading,
) -> Option<Value> {
    let previous = previous?;
    let scope = &reset.scope;
    let now = at.utc.timestamp() as f64 + f64::from(at.utc.timestamp_subsec_nanos()) / 1e9;
    if reset.scope != native.scope
        || reset.sequence != native.sequence
        || reset.native_digest != native.native_digest
        || ![
            &reset.adapter_id,
            &reset.reference,
            &reset.previous_native_digest,
            &reset.previous_window_instance_id,
            &reset.current_window_instance_id,
        ]
        .into_iter()
        .all(|s| valid_text(s))
        || reset.current_window_instance_id != scope.window_instance_id
        || reset.previous_sequence == 0
        || reset.previous_sequence >= reset.sequence
        || decimal(&previous["observationSequence"]) != Some(reset.previous_sequence)
        || native_snapshot_digest(previous) != reset.previous_native_digest
        || !scope_matches(previous, scope)
        || previous["persisted"] != true
        || previous["identityConsistency"] != "proven"
        || !previous["identityEvidence"].is_object()
        || previous["observationOutcome"] != "success"
        || previous["entitlementSegment"] != scope.entitlement_segment
        || previous["entitlementEvidence"]["ruleVersion"] != scope.rule_version
        || previous["clockEpoch"] != native.clock_epoch
        || raw_window(previous, &scope.pool_id)?["windowInstanceId"]
            != reset.previous_window_instance_id
        || raw_window(previous, &scope.pool_id)?["ruleVersion"] != scope.rule_version
        || raw_window(current, &scope.pool_id)?["resetsAt"]
            .as_i64()
            .map(|v| v as f64)
            != Some(native.reset_time.lower)
        || reset
            .next_reset
            .is_some_and(|t| !t.is_valid() || t.lower < 0.0 || t != native.reset_time)
        || reset
            .actual_boundary
            .is_some_and(|t| !t.is_valid() || t.lower < 0.0 || t.upper > now)
        || reset
            .final_used_percent
            .is_some_and(|v| !v.is_finite() || !(0.0..=100.0).contains(&v))
    {
        return None;
    }
    let same = reset.transition == ResetTransition::SameWindow;
    if same != (reset.previous_window_instance_id == reset.current_window_instance_id)
        || (same && (reset.actual_boundary.is_some() || reset.final_used_percent.is_some()))
        || (!same
            && reset
                .actual_boundary
                .is_some_and(|t| t.upper > native.fact_time.lower))
        || (!same
            && reset.actual_boundary.is_some_and(|t| {
                instant(&previous["quotaRead"]["factTimeUpper"])
                    .is_none_or(|previous_fact| previous_fact > t.lower)
            }))
        || (!same && reset.actual_boundary.is_none() && reset.final_used_percent.is_some())
    {
        return None;
    }
    Some(
        json!({"confirmedNewWindow":!same,"confirmedSameWindow":same,"windowInstanceId":reset.current_window_instance_id,
        "previousWindowInstanceId":reset.previous_window_instance_id,"nextReset":reset.next_reset,"actualBoundary":reset.actual_boundary,
        "finalUsedPercent":reset.final_used_percent,"adapterId":reset.adapter_id,"reference":reset.reference,
        "previousSequence":reset.previous_sequence.to_string(),"previousNativeDigest":reset.previous_native_digest}),
    )
}

fn apply_fields(snapshot: &mut Value, snapshot_fields: &Value, windows: &Value) -> Option<()> {
    for (key, value) in snapshot_fields.as_object()? {
        snapshot.as_object_mut()?.insert(key.clone(), value.clone());
    }
    for (id, fields) in windows.as_object()? {
        let mut found = false;
        for bucket in snapshot.get_mut("buckets")?.as_array_mut()? {
            for kind in ["primary", "secondary"] {
                if format!("{}:{kind}", bucket["id"].as_str()?) == *id {
                    let window = bucket.get_mut(kind)?.as_object_mut()?;
                    for (key, value) in fields.as_object()? {
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
    Some(())
}

/// Does not mutate original scalar/read fields, persist, renew freshness, or
/// accept JSON evidence. A single bad duplicate/conflicting current proof closes
/// admission, while earlier proofs in a learning batch are ignored here.
pub(crate) fn admit(
    snapshot: &mut Value,
    previous: Option<&Value>,
    proofs: &[&AdapterQuotaProof],
    resets: &[&AdapterResetTransitionProof],
    at: &Reading,
) -> Value {
    strip(snapshot);
    if proofs.len() > 1024 || resets.len() > MAX_PROOFS {
        return json!({"status":"unavailable","reasonCodes":["observation_proof_limit"]});
    }
    let sequence = decimal(&snapshot["observationSequence"]);
    let mut windows = BTreeMap::new();
    let mut shared: Option<Value> = None;
    let mut reasons = BTreeSet::new();
    for reset in resets.iter().copied().filter(|r| {
        Some(r.sequence) == sequence
            && r.scope.runtime_id == snapshot["runtimeId"].as_str().unwrap_or_default()
    }) {
        if !proofs.iter().any(|proof| {
            proof.scope == reset.scope
                && proof.sequence == reset.sequence
                && proof.native_digest == reset.native_digest
        }) {
            reasons.insert("reset_transition_native_proof_missing");
        }
    }
    for proof in proofs.iter().copied().filter(|p| {
        Some(p.sequence) == sequence
            && p.scope.runtime_id == snapshot["runtimeId"].as_str().unwrap_or_default()
    }) {
        let Some((fields, mut window)) = proof_metadata(snapshot, proof, at) else {
            reasons.insert("adapter_observation_proof_mismatch");
            continue;
        };
        if shared.as_ref().is_some_and(|old| old != &fields)
            || windows.contains_key(&proof.scope.pool_id)
        {
            reasons.insert("adapter_observation_proof_conflict");
            continue;
        }
        let matching: Vec<_> = resets
            .iter()
            .copied()
            .filter(|r| {
                r.sequence == proof.sequence
                    && r.scope.pool_id == proof.scope.pool_id
                    && r.scope.runtime_id == proof.scope.runtime_id
            })
            .collect();
        if matching.len() > 1 {
            reasons.insert("reset_transition_proof_conflict");
        } else if let Some(reset) = matching.first() {
            if let Some(value) = reset_metadata(snapshot, previous, proof, reset, at) {
                window["resetEvidence"] = value;
            } else {
                reasons.insert("reset_transition_proof_mismatch");
            }
        }
        shared = Some(fields);
        windows.insert(proof.scope.pool_id.clone(), window);
    }
    if windows.len() > MAX_PROOFS || !reasons.is_empty() || shared.is_none() {
        if reasons.is_empty() {
            reasons.insert("adapter_quota_proof_missing");
        }
        return json!({"status":"unavailable","reasonCodes":reasons});
    }
    let mut metadata = json!({"schemaVersion":1,"algorithmVersion":VERSION,"nativeDigest":native_snapshot_digest(snapshot),
        "scope":{"runtimeId":snapshot["runtimeId"],"sourceId":snapshot["sourceId"],"bindingGeneration":snapshot["bindingGeneration"],"identityEpoch":snapshot["identityEpoch"],"clearGeneration":snapshot["clearGeneration"],"observationSequence":snapshot["observationSequence"]},
        "snapshotFields":shared.unwrap(),"windowFields":windows});
    let digest = canonical_digest(&metadata);
    metadata["metadataDigest"] = json!(digest);
    if metadata.to_string().len() > MAX_BYTES {
        return json!({"status":"unavailable","reasonCodes":["observation_proof_limit"]});
    }
    let mut bounded_body = snapshot.clone();
    if let Some(body) = bounded_body.as_object_mut() {
        body.remove("usage");
    }
    bounded_body[NAMESPACE] = metadata.clone();
    if bounded_body.to_string().len() > MAX_BODY_WITH_PROOF {
        return json!({"status":"unavailable","reasonCodes":["observation_proof_payload_limit"]});
    }
    let fields = metadata["snapshotFields"].clone();
    let windows = metadata["windowFields"].clone();
    if apply_fields(snapshot, &fields, &windows).is_none() {
        strip(snapshot);
        return json!({"status":"unavailable","reasonCodes":["observation_proof_window_missing"]});
    }
    snapshot[NAMESPACE] = metadata;
    json!({"status":"admitted","reasonCodes":[]})
}

fn metadata_valid(raw: &Value, metadata: &Value) -> bool {
    if !unique_windows(raw)
        || metadata["schemaVersion"] != 1
        || metadata["algorithmVersion"] != VERSION
        || metadata["nativeDigest"] != native_snapshot_digest(raw)
        || metadata.to_string().len() > MAX_BYTES
    {
        return false;
    }
    let mut check = metadata.clone();
    let Some(object) = check.as_object_mut() else {
        return false;
    };
    object.remove("metadataDigest");
    if metadata["metadataDigest"] != canonical_digest(&check) {
        return false;
    }
    for key in [
        "runtimeId",
        "sourceId",
        "bindingGeneration",
        "identityEpoch",
        "clearGeneration",
        "observationSequence",
    ] {
        if metadata["scope"].get(key) != raw.get(key) {
            return false;
        }
    }
    let Some(fields) = metadata["snapshotFields"].as_object() else {
        return false;
    };
    let Some(windows) = metadata["windowFields"].as_object() else {
        return false;
    };
    !windows.is_empty()
        && windows.len() <= MAX_PROOFS
        && fields.len() == SNAPSHOT_FIELDS.len()
        && fields.keys().all(|k| SNAPSHOT_FIELDS.contains(&k.as_str()))
        && fields["identityEvidence"].is_object()
        && text(&fields["identityEvidence"]["adapterId"]).is_some()
        && text(&fields["identityEvidence"]["reference"]).is_some()
        && text(&fields["entitlementSegment"]).is_some()
        && text(&fields["clockEpoch"]).is_some()
        && fields["entitlementEvidence"]["verified"] == true
        && fields["entitlementEvidence"]["source"] == "official"
        && fields["entitlementEvidence"]["segmentId"] == fields["entitlementSegment"]
        && text(&fields["entitlementEvidence"]["ruleVersion"]).is_some()
        && text(&fields["entitlementEvidence"]["reference"]).is_some()
        && windows.iter().all(|(id, value)| {
            raw_window(raw, id).is_some()
                && value.as_object().is_some_and(|window| {
                    window
                        .keys()
                        .all(|key| WINDOW_FIELDS.contains(&key.as_str()))
                        && text(&value["windowInstanceId"]).is_some()
                        && text(&value["ruleVersion"]).is_some()
                        && matches!(
                            value["recovery"].as_str(),
                            Some("fixed_reset" | "anchored_reset")
                        )
                        && value["windowInstanceEvidence"]["verified"] == true
                        && value["windowInstanceEvidence"]["source"] == "official"
                        && value["windowInstanceEvidence"]["windowInstanceId"]
                            == value["windowInstanceId"]
                        && text(&value["windowInstanceEvidence"]["reference"]).is_some()
                        && value.get("roundingStep").is_some_and(|q| {
                            q.is_null()
                                || q.as_f64()
                                    .is_some_and(|q| q.is_finite() && q > 0.0 && q <= 100.0)
                        })
                        && value["saturated"].is_boolean()
                        && value["saturationHeadroomBounded"].is_boolean()
                })
        })
}

/// Called after constructing the admitted receipt, before encode/INSERT. The
/// immutable proof namespace is outer metadata, never a rewritten vendor body.
pub(crate) fn seal_receipt(receipt: &mut Value) -> Result<(), String> {
    let metadata = receipt["observation"].get(NAMESPACE).cloned();
    let Some(metadata) = metadata else {
        return Ok(());
    };
    if !metadata_valid(&receipt["observation"], &metadata) {
        return Err("观测证明与原始输入不一致".into());
    }
    if receipt.get(NAMESPACE).is_some_and(|old| old != &metadata) {
        return Err("已冻结的观测证明不能覆写".into());
    }
    strip(&mut receipt["observation"]);
    receipt[NAMESPACE] = metadata;
    Ok(())
}

/// Returns `None` for a damaged namespace. Legacy receipts remain readable; new
/// writes cannot obtain admission from their JSON metadata alone.
pub(crate) fn hydrate(receipt: &Value) -> Option<Value> {
    let mut raw = receipt["observation"].clone();
    let Some(metadata) = receipt.get(NAMESPACE) else {
        return Some(raw);
    };
    if !metadata_valid(&raw, metadata) {
        return None;
    }
    apply_fields(
        &mut raw,
        &metadata["snapshotFields"],
        &metadata["windowFields"],
    )?;
    Some(raw)
}

#[cfg(test)]
#[path = "observation_proof_tests.rs"]
mod tests;
