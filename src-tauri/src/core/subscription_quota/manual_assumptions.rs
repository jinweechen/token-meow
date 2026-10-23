//! Explicit manual scenarios. These values never enter calibration or grant a
//! sustainable-use verdict, a native permit, or a plan target.
use super::evaluate::remaining_tokens;
use super::model::{Interval, PercentReading, UsageProfile};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::collections::BTreeSet;

const MAX_SAFE: f64 = 9_007_199_254_740_991.0;
type EvidenceResult<T> = std::result::Result<T, &'static str>;
fn text(v: &Value) -> Option<&str> {
    v.as_str().filter(|v| !v.trim().is_empty())
}
fn decimal(v: &Value) -> Option<u64> {
    let s = v.as_str()?;
    let n = s.parse::<u64>().ok()?;
    (n.to_string() == s).then_some(n)
}
fn instant(v: &Value) -> Option<DateTime<Utc>> {
    if let Some(n) = v.as_f64() {
        let ms = n * 1000.0;
        return (ms.is_finite() && ms >= 0.0 && ms < i64::MAX as f64)
            .then(|| DateTime::<Utc>::from_timestamp_millis(ms.floor() as i64))
            .flatten();
    }
    DateTime::parse_from_rfc3339(v.as_str()?)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}
fn fresh(read: &Value, at: DateTime<Utc>) -> bool {
    read["outcome"] == "success"
        && instant(&read["freshnessAnchor"])
            .is_some_and(|t| t <= at && (at - t).num_milliseconds() < 240_000)
}
fn raw_window<'a>(snapshot: &'a Value, id: &str) -> Option<(&'a Value, &'a str)> {
    for b in snapshot["buckets"].as_array()? {
        let pool = text(&b["id"])?;
        for kind in ["primary", "secondary"] {
            if id == format!("{pool}:{kind}") && b[kind].is_object() {
                return Some((&b[kind], pool));
            }
        }
    }
    None
}
fn profile(
    window: &Value,
    source: &str,
    account: &str,
    id: &str,
    period: &str,
    model: Option<&str>,
) -> EvidenceResult<Value> {
    let e = &window["usageStructureEvidence"];
    if e["verified"] != true
        || e["source"] != "trusted-ledger"
        || text(&e["reference"]).is_none()
        || e["sourceId"] != source
        || e["accountScope"] != account
        || e["windowId"] != id
        || e["windowInstanceId"] != period
        || e["modelDimension"] != "response"
        || e.get("model").is_none()
    {
        return Err("usage_structure_unknown");
    }
    let p: UsageProfile =
        serde_json::from_value(e["profile"].clone()).map_err(|_| "usage_structure_unknown")?;
    // The attested model describes the observed structure. The user's `None`
    // selects that actual mixture, which may itself contain only model A.
    let actual_model =
        (p.all_models_attributed && p.models.len() == 1).then(|| p.models[0].model.as_str());
    if e["model"] != json!(actual_model) {
        return Err("usage_structure_unknown");
    }
    let known = |s: &Option<String>| {
        s.as_deref()
            .is_some_and(|s| !s.trim().is_empty() && s != "unknown")
    };
    let ratio = |n: f64| n.is_finite() && (0.0..=1.0).contains(&n);
    if !p.all_models_attributed
        || !known(&p.effort)
        || !known(&p.speed_mode)
        || !ratio(p.output_share)
        || !ratio(p.cache_share)
        || !p.median_input_tokens.is_finite()
        || p.median_input_tokens <= 0.0
        || p.models.is_empty()
    {
        return Err("usage_structure_unknown");
    }
    let mut names = BTreeSet::new();
    let mut sum = 0.0;
    for m in &p.models {
        if m.model.trim().is_empty()
            || m.model == "unknown"
            || !ratio(m.share)
            || m.share <= 0.0
            || !names.insert(m.model.as_str())
        {
            return Err("usage_structure_unknown");
        }
        sum += m.share;
    }
    if (sum - 1.0).abs() > 1e-9
        || model.is_some_and(|m| p.models.len() != 1 || p.models[0].model != m)
    {
        return Err("usage_structure_unknown");
    }
    // Order is presentation, not a different usage mixture.
    let mut p = p;
    p.models.sort_by(|a, b| a.model.cmp(&b.model));
    serde_json::to_value(p).map_err(|_| "usage_structure_unknown")
}

/// A stable account, official entitlement/rule and a positively identified pool
/// period are prerequisites. A note, planType label or reset timestamp cannot
/// supply any of these missing proofs.
fn current_scope<'a>(
    snapshot: Option<&'a Value>,
    source: &str,
    id: &str,
    model: Option<&str>,
    at: DateTime<Utc>,
) -> EvidenceResult<(Value, &'a Value)> {
    let s = snapshot.ok_or("identity_unproven")?;
    if source.is_empty() || s["sourceId"] != source {
        return Err("source_or_runtime_changed");
    }
    if s["identityConsistency"] != "proven"
        || !s["identityEvidence"].is_object()
        || s["observationOutcome"] != "success"
        || s["status"] != "connected"
        || s["persisted"] != true
    {
        return Err("identity_unproven");
    }
    let runtime = text(&s["runtimeId"]).ok_or("scope_unknown")?;
    let account = text(&s["accountScope"]).ok_or("stable_account_identity_unknown")?;
    let binding = decimal(&s["bindingGeneration"]).ok_or("scope_unknown")?;
    let epoch = decimal(&s["identityEpoch"])
        .filter(|n| *n > 0)
        .ok_or("scope_unknown")?;
    decimal(&s["observationSequence"])
        .filter(|n| *n > 0)
        .ok_or("scope_unknown")?;
    let clear = s.get("clearGeneration").ok_or("scope_unknown")?;
    if !fresh(&s["accountRead"], at) {
        return Err("membership_stale");
    }
    if !fresh(&s["quotaRead"], at) {
        return Err("stale_observation");
    }
    let segment = text(&s["entitlementSegment"]).ok_or("entitlement_evidence_unknown")?;
    let e = &s["entitlementEvidence"];
    if e["verified"] != true
        || e["source"] != "official"
        || e["segmentId"] != segment
        || text(&e["reference"]).is_none()
    {
        return Err("entitlement_evidence_unknown");
    }
    let rule = text(&e["ruleVersion"]).ok_or("entitlement_evidence_unknown")?;
    let (w, pool) = raw_window(s, id).ok_or("manual_window_unverified")?;
    let period = text(&w["windowInstanceId"]).ok_or("manual_window_unverified")?;
    let we = &w["windowInstanceEvidence"];
    if we["verified"] != true
        || we["source"] != "official"
        || we["windowInstanceId"] != period
        || text(&we["reference"]).is_none()
    {
        return Err("manual_window_unverified");
    }
    if !matches!(
        w["recovery"].as_str(),
        Some("fixed_reset" | "anchored_reset")
    ) {
        return Err("recovery_unknown");
    }
    let window_rule = text(&w["ruleVersion"]).ok_or("entitlement_evidence_unknown")?;
    let reset = instant(&w["resetsAt"])
        .filter(|t| at < *t)
        .ok_or("window_confirmation_pending")?;
    if model.is_some_and(|m| m.trim().is_empty() || m == "unknown") {
        return Err("model_scope_unknown");
    }
    let structure = profile(w, source, account, id, period, model)?;
    Ok((
        json!({"runtimeId":runtime,"sourceId":source,"accountScope":account,"bindingGeneration":binding.to_string(),"identityEpoch":epoch.to_string(),"clearGeneration":clear,
        "entitlementSegment":segment,"entitlementRule":rule,"poolId":pool,"windowId":id,"windowInstanceId":period,"windowRule":window_rule,
        "recovery":w["recovery"],"resetsAt":reset.to_rfc3339(),"model":model,"usageStructure":structure}),
        w,
    ))
}

/// Persist this result as assumption.manualScope. Missing evidence does not
/// prohibit saving the user's independent capacity premise.
pub(crate) fn freeze(
    snapshot: Option<&Value>,
    source: &str,
    window_id: &str,
    model: Option<&str>,
    at: DateTime<Utc>,
) -> Value {
    match current_scope(snapshot, source, window_id, model, at) {
        Ok((scope, _)) => {
            json!({"schemaVersion":1,"scope":scope,"capturedAt":at.to_rfc3339(),"reasons":[]})
        }
        Err(reason) => {
            json!({"schemaVersion":1,"scope":null,"capturedAt":at.to_rfc3339(),"reasons":[reason]})
        }
    }
}

/// Pure, read-only result. Render its numbers only in an explicitly selected
/// manual scenario; never merge them with automatic calibration or A/B totals.
pub(crate) fn scenario(
    assumption: &Value,
    snapshot: Option<&Value>,
    source: &str,
    at: DateTime<Utc>,
) -> Value {
    let mut out = json!({"kind":"manual_capacity_scenario","active":false,"provenance":"manual-assumption","quality":"conditional",
        "assumptionId":assumption["id"],"revision":assumption["revision"],"sourceId":assumption["sourceId"],"windowId":assumption["windowId"],"model":assumption["model"],
        "capacityTokens":assumption["capacityTokens"],"remainingEstimate":null,"remainingLower":null,"remainingUpper":null,
        "estimatePrecision":"unknown","constraintsVerified":false,"notifyEligible":false,"mutuallyExclusive":true,"reasons":[]});
    let result = (|| -> EvidenceResult<()> {
        if assumption["sourceId"] != source || assumption["provenance"] != "manual-assumption" {
            return Err("manual_scope_changed");
        }
        let capacity = assumption["capacityTokens"]
            .as_f64()
            .filter(|n| n.is_finite() && *n > 0.0 && *n <= MAX_SAFE && n.fract() == 0.0)
            .ok_or("manual_capacity_invalid")?;
        if instant(&assumption["validUntil"]).is_none_or(|t| at >= t) {
            return Err("manual_assumption_expired");
        }
        if instant(&assumption["effectiveFrom"]).is_none_or(|t| at < t) {
            return Err("manual_assumption_not_started");
        }
        let stored = &assumption["manualScope"];
        if stored["schemaVersion"] != 1
            || !stored["scope"].is_object()
            || instant(&stored["capturedAt"]).is_none_or(|t| at < t)
        {
            return Err("manual_scope_unverified");
        }
        let id = text(&assumption["windowId"]).ok_or("manual_scope_unverified")?;
        let model = if assumption["model"].is_null() {
            None
        } else {
            Some(text(&assumption["model"]).ok_or("model_scope_unknown")?)
        };
        let (scope, w) = current_scope(snapshot, source, id, model, at)?;
        if stored["scope"] != scope {
            return Err("manual_scope_changed");
        }
        let used = w["usedPercent"]
            .as_f64()
            .filter(|v| v.is_finite() && (0.0..=100.0).contains(v))
            .ok_or("manual_percent_unknown")?;
        let reading = PercentReading {
            used_percent: used,
            rounding_step: w["roundingStep"].as_f64(),
            saturated: w["saturated"] == true,
            saturation_headroom_bounded: w["saturationHeadroomBounded"] == true,
        };
        // A point estimate is labelled separately from an interval. At
        // saturation without bounded headroom even the point stays unknown.
        if (reading.saturated || used == 100.0) && !reading.saturation_headroom_bounded {
            return Err("manual_percent_saturated");
        }
        match remaining_tokens(
            Interval::exact(capacity).ok_or("manual_capacity_invalid")?,
            &reading,
        ) {
            Ok(interval) => {
                out["remainingEstimate"] = json!(capacity * (100.0 - used) / 100.0);
                out["remainingLower"] = json!(interval.lower);
                out["remainingUpper"] = json!(interval.upper);
                out["estimatePrecision"] = json!("bounded");
            }
            Err(_) if reading.rounding_step.is_none() => {
                out["remainingEstimate"] = json!(capacity * (100.0 - used) / 100.0);
                out["reasons"] = json!(["precision_unknown"]);
            }
            Err(_) => return Err("manual_percent_invalid"),
        }
        out["active"] = json!(true);
        Ok(())
    })();
    if let Err(reason) = result {
        out["remainingEstimate"] = Value::Null;
        out["remainingLower"] = Value::Null;
        out["remainingUpper"] = Value::Null;
        out["reasons"] = json!([reason]);
    }
    out
}

#[cfg(test)]
#[path = "manual_assumptions_tests.rs"]
mod tests;
