//! M3's G4b-gated current-remaining Token draft. This is a pure projection over
//! Core-owned current observations and projector output, never an IPC proof.
//! Missing evidence returns None so the caller can retain the native-unit draft.
use super::calibration::{calibrate, profiles_compatible};
use super::evaluate::remaining_tokens;
use super::model::{
    CalibrationPair, CalibrationQuality, CalibrationResult, CoverageEvidence, EvaluationScope,
    IdentityEvidence, Interval, NativeSample, PercentReading, RecoverySemantics, UsageProfile,
    ALGORITHM_VERSION,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const MAX_SAFE: u64 = 9_007_199_254_740_991;
const MAX_WINDOWS: usize = 200;
const MAX_PAIRS: usize = 256;

/// Only these user choices may cross from the validated draft command. All
/// other arguments to token_draft must be constructed by Core, not the payload.
pub(super) struct Selection<'a> {
    pub source_id: &'a str,
    pub window_id: &'a str,
    pub model: Option<&'a str>,
    pub model_dimension: &'a str,
    pub reserve_percent: f64,
}

fn text(value: &Value) -> Option<&str> {
    value
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= 2000)
}
fn decimal(value: &Value) -> Option<u64> {
    let raw = value.as_str()?;
    let value = raw.parse::<u64>().ok()?;
    (value.to_string() == raw).then_some(value)
}
fn number(value: &Value) -> Option<f64> {
    value.as_f64().filter(|n| n.is_finite())
}
fn amount(value: &Value) -> Option<u64> {
    number(value)
        .filter(|n| *n >= 0.0 && *n <= MAX_SAFE as f64 && n.fract() == 0.0)
        .map(|n| n as u64)
}
fn instant(value: &Value) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.as_str()?)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}
fn seconds(value: &Value) -> Option<f64> {
    if let Some(value) = number(value) {
        return (value >= 0.0).then_some(value);
    }
    let value = instant(value)?;
    let seconds = value.timestamp() as f64 + value.timestamp_subsec_nanos() as f64 / 1e9;
    (seconds.is_finite() && seconds >= 0.0).then_some(seconds)
}
fn timestamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}
fn raw_windows(snapshot: &Value) -> Option<BTreeMap<String, &Value>> {
    let mut windows = BTreeMap::new();
    for bucket in snapshot["buckets"].as_array()? {
        let id = text(&bucket["id"])?;
        for kind in ["primary", "secondary"] {
            let window = &bucket[kind];
            if window.is_object() {
                if windows.insert(format!("{id}:{kind}"), window).is_some() {
                    return None;
                }
            } else if !window.is_null() {
                return None;
            }
        }
    }
    (!windows.is_empty() && windows.len() <= MAX_WINDOWS).then_some(windows)
}
fn canonical_profile(value: &Value) -> Option<UsageProfile> {
    let mut profile: UsageProfile = serde_json::from_value(value.clone()).ok()?;
    let known = |v: &Option<String>| {
        v.as_deref()
            .is_some_and(|v| !v.trim().is_empty() && v != "unknown")
    };
    if !known(&profile.effort)
        || !known(&profile.speed_mode)
        || !profiles_compatible(&profile, &profile)
        || profile.models.iter().any(|m| m.model == "unknown")
    {
        return None;
    }
    profile.models.sort_by(|a, b| a.model.cmp(&b.model));
    Some(profile)
}

fn current_sample_matches(
    sample: &NativeSample,
    current: &Value,
    frozen: &Value,
    raw: &Value,
    window: &Value,
    runtime: &str,
    at: DateTime<Utc>,
) -> Option<()> {
    let scope = &frozen["scope"];
    let expected = EvaluationScope {
        runtime_id: runtime.into(),
        source_id: text(&scope["sourceId"])?.into(),
        binding_generation: decimal(&current["bindingGeneration"])?,
        identity_epoch: decimal(&current["identityEpoch"])?,
        clear_generation: current["clearGeneration"].to_string(),
        account_scope: Some(text(&scope["accountScope"])?.into()),
        entitlement_segment: text(&scope["entitlementSegment"])?.into(),
        rule_version: text(&scope["windowRule"])?.into(),
        pool_id: text(&window["id"])?.into(),
        window_instance_id: text(&window["periodInstanceId"])?.into(),
    };
    if !expected.is_valid()
        || expected.identity_epoch == 0
        || scope["runtimeId"] != runtime
        || scope["windowInstanceId"] != window["periodInstanceId"]
        || sample.scope != expected
        || sample.sequence != decimal(&current["observationSequence"])?
        || sample.sequence == 0
        || !sample.persisted
        || !sample.identity_consistent
        || sample.discontinuity
        || current["discontinuity"] == true
    {
        return None;
    }
    let percent = PercentReading {
        used_percent: number(&raw["usedPercent"])?.clamp(0.0, 100.0),
        rounding_step: Some(number(&raw["roundingStep"])?),
        saturated: raw["saturated"] == true,
        saturation_headroom_bounded: raw["saturationHeadroomBounded"] == true,
    };
    if !(0.0..=100.0).contains(&number(&raw["usedPercent"])?)
        || percent.rounding_step.is_none_or(|q| q <= 0.0 || q > 100.0)
        || sample.percent != percent
        || window["usedPercent"].as_f64() != Some(percent.used_percent)
    {
        return None;
    }
    let read = &current["quotaRead"];
    if matches!(
        text(&read["timeBasis"])?,
        "unknown" | "request_start_only" | "unbounded" | "request-start-fallback"
    ) {
        return None;
    }
    let fact = Interval::new(
        seconds(&read["factTimeLower"])?,
        seconds(&read["factTimeUpper"])?,
    )?;
    let now = seconds(&json!(timestamp(at)))?;
    let anchor = seconds(&read["freshnessAnchor"])?;
    if sample.fact_time != Some(fact)
        || fact.lower < 0.0
        || fact.upper > now
        || now - fact.lower >= 240.0
        || anchor > now
        || now - anchor >= 240.0
        || sample.ttl_start != anchor
        || sample.clock_epoch
            != text(&read["clockEpoch"]).or_else(|| text(&current["clockEpoch"]))?
    {
        return None;
    }
    let reset = seconds(&window["resetsAt"])?;
    let raw_reset = seconds(&raw["resetsAt"])?;
    let reset_bounds = if raw["resetTimeLower"].is_null() && raw["resetTimeUpper"].is_null() {
        Interval::exact(raw_reset)?
    } else {
        Interval::new(
            seconds(&raw["resetTimeLower"])?,
            seconds(&raw["resetTimeUpper"])?,
        )?
    };
    let recovery = match raw["recovery"].as_str()? {
        "fixed_reset" => RecoverySemantics::FixedReset,
        "anchored_reset" => RecoverySemantics::AnchoredReset,
        _ => return None,
    };
    // An uncertain reset needs a separate solver. This first draft stops at one
    // precisely identified reset and never crosses an unproven boundary.
    if reset <= now
        || reset != raw_reset
        || reset_bounds.lower != reset
        || reset_bounds.upper != reset
        || sample.reset_time != Some(reset_bounds)
        || sample.recovery != recovery
    {
        return None;
    }
    Some(())
}

fn calibrated_remaining(
    card: &Value,
    window: &Value,
    raw: &Value,
    current: &Value,
    frozen: &Value,
    profile: &UsageProfile,
    runtime: &str,
    at: DateTime<Utc>,
) -> Option<u64> {
    if card["windowId"] != window["id"]
        || card["id"] != window["id"]
        || card["archived"] == true
        || card["quality"] != "validated"
        || card["coverage"] != "verified_complete"
        || card.get("manualScenario").is_some()
        || card
            .get("provenance")
            .is_some_and(|v| v != "automatic-calibration")
        || !card["reasons"].as_array().is_some_and(Vec::is_empty)
    {
        return None;
    }
    let card_profile = canonical_profile(&card["profileEvidence"])?;
    if !profiles_compatible(profile, &card_profile) {
        return None;
    }
    let actual_model =
        (card_profile.models.len() == 1).then(|| card_profile.models[0].model.as_str());
    if card["model"] != json!(actual_model) {
        return None;
    }
    let evidence: CalibrationResult = serde_json::from_value(card["evidence"].clone()).ok()?;
    let pairs: Vec<CalibrationPair> = serde_json::from_value(card["inputPairs"].clone()).ok()?;
    if evidence.algorithm_version != ALGORITHM_VERSION
        || evidence.quality != CalibrationQuality::Validated
        || !evidence.reasons.is_empty()
        || pairs.len() < 25
        || pairs.len() > MAX_PAIRS
        || pairs.iter().any(|p| {
            p.coverage != CoverageEvidence::VerifiedComplete
                || p.identity_evidence != IdentityEvidence::VerifiedAccount
                || !p.settled
                || p.conflicts_or_gaps
                || p.evidence_invalidated
        })
    {
        return None;
    }
    let last = pairs.last()?;
    if canonical_profile(&serde_json::to_value(&last.profile).ok()?)? != card_profile {
        return None;
    }
    current_sample_matches(&last.end, current, frozen, raw, window, runtime, at)?;
    let checked = calibrate(&pairs, &card_profile);
    let capacity = checked.capacity?;
    if checked.quality != CalibrationQuality::Validated
        || checked.train_pairs < 20
        || checked.holdout_pairs < 5
        || checked.independent_periods < 3
        || checked.capacity != evidence.capacity
        || checked.train_pairs != evidence.train_pairs
        || checked.holdout_pairs != evidence.holdout_pairs
        || checked.independent_periods != evidence.independent_periods
        || card["sampleCount"].as_u64() != Some(checked.train_pairs as u64)
        || card["cycleCount"].as_u64() != Some(checked.independent_periods as u64)
        || capacity.lower <= 0.0
        || capacity.upper > MAX_SAFE as f64
        || amount(&card["capacityLower"])? as f64 != capacity.lower.floor()
        || amount(&card["capacityUpper"])? as f64 != capacity.upper.ceil()
    {
        return None;
    }
    let remaining = remaining_tokens(capacity, &last.end.percent).ok()?;
    let lower = amount(&card["remainingLower"])?;
    let upper = amount(&card["remainingUpper"])?;
    (lower <= upper
        && lower as f64 == remaining.lower.floor()
        && upper as f64 == remaining.upper.ceil())
    .then_some(lower)
}

/// Compute the floor using the user's canonical decimal percentage, avoiding an
/// upward floating-point round at the safe-integer boundary. Extreme decimals
/// that exceed the checked integer budget simply keep the native fallback.
fn reserved_target(lower: u64, reserve: f64) -> Option<u64> {
    if !reserve.is_finite() || !(0.0..100.0).contains(&reserve) || lower > MAX_SAFE {
        return None;
    }
    let raw = reserve.to_string();
    let (whole, fraction) = raw.split_once('.').unwrap_or((&raw, ""));
    if fraction.len() > 20
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let denominator = 10u128
        .checked_pow(fraction.len() as u32)?
        .checked_mul(100)?;
    let numerator = format!("{whole}{fraction}").parse::<u128>().ok()?;
    let target = u128::from(lower).checked_mul(denominator.checked_sub(numerator)?)? / denominator;
    (target > 0 && target <= u128::from(MAX_SAFE)).then_some(target as u64)
}

/// Pure/read-only. A successful result is still an editable, conditional draft;
/// it neither saves a plan nor grants or renews a notification permission.
pub(super) fn token_draft(
    selection: &Selection<'_>,
    current: &Value,
    windows: &[Value],
    calibrations: &[Value],
    runtime: &str,
    at: DateTime<Utc>,
) -> Option<Value> {
    if selection.model_dimension != "response"
        || selection.source_id.trim().is_empty()
        || selection.window_id.trim().is_empty()
        || selection
            .model
            .is_some_and(|m| m.trim().is_empty() || m == "unknown")
        || current["runtimeId"] != runtime
        || current["sourceId"] != selection.source_id
        || current["persisted"] != true
        || current["identityConsistency"] != "proven"
        || current["status"] != "connected"
        || current["observationOutcome"] != "success"
    {
        return None;
    }
    let raw = raw_windows(current)?;
    if windows.is_empty()
        || windows.len() > MAX_WINDOWS
        || raw.len() != windows.len()
        || calibrations.len() != windows.len()
    {
        return None;
    }
    let mut seen = BTreeSet::new();
    let mut minimum = MAX_SAFE;
    let mut end: Option<DateTime<Utc>> = None;
    let mut common_profile: Option<UsageProfile> = None;
    let mut constraints = Vec::new();
    for window in windows {
        let id = text(&window["id"])?;
        if !seen.insert(id.to_owned())
            || window["freshness"] != "fresh"
            || window["canBindPlan"] != true
            || window["resetState"] != "current"
            || !window["reasons"].as_array().is_some_and(Vec::is_empty)
        {
            return None;
        }
        let raw_window = *raw.get(id)?;
        // This shared validator checks official/current identity, entitlement,
        // rule, window and trusted-ledger structure. It reads no manual capacity.
        let frozen = super::manual_assumptions::freeze(
            Some(current),
            selection.source_id,
            id,
            selection.model,
            at,
        );
        if !frozen["reasons"].as_array().is_some_and(Vec::is_empty) {
            return None;
        }
        let profile = canonical_profile(&frozen["scope"]["usageStructure"])?;
        if common_profile
            .as_ref()
            .is_some_and(|p| !profiles_compatible(p, &profile))
        {
            return None;
        }
        if common_profile.is_none() {
            common_profile = Some(profile.clone());
        }
        let mut matching = calibrations.iter().filter(|card| card["windowId"] == id);
        let card = matching.next()?;
        if matching.next().is_some() {
            return None;
        }
        let lower = calibrated_remaining(
            card, window, raw_window, current, &frozen, &profile, runtime, at,
        )?;
        let reset = instant(&window["resetsAt"])?;
        if reset <= at {
            return None;
        }
        minimum = minimum.min(lower);
        end = Some(end.map_or(reset, |old| old.min(reset)));
        constraints.push(json!({"windowId":id,"periodInstanceId":window["periodInstanceId"],"remainingLower":lower,"resetsAt":timestamp(reset)}));
    }
    if !seen.contains(selection.window_id) {
        return None;
    }
    let target = reserved_target(minimum, selection.reserve_percent)?;
    let profile = common_profile?;
    Some(json!({
        "plan":{"expectedRevision":null,"title":"按验证余量安排","type":"tokens","sourceId":selection.source_id,
            "model":selection.model,"modelDimension":"response","target":target,"endPolicy":"time","endsAt":timestamp(end?),"windowId":null,"remindersEnabled":true},
        "basis":[format!("所有 {} 个已知相关窗口均有跨周期验证且完整覆盖的当前余量依据",windows.len()),
            format!("取相关窗口剩余 Token 下界的最小值，并预留 {}% 后向下取整",selection.reserve_percent),
            "仅在继续当前响应模型组合及相同使用结构的条件下成立；草案止于下一个相关重置边界，保存后才启用"],
        "constraintsVerified":true,"reasons":[],"unverifiedWindows":[],"notifyEligible":false,
        "sustainability":{"conditional":true,"provenance":"validated-current-calibration","quality":"validated","coverage":"verified_complete",
            "reservePercent":selection.reserve_percent,"usageStructure":profile,"constraints":constraints},
        "scope":{"sourceId":selection.source_id,"model":selection.model,"modelDimension":"response"},
        "asOf":current["quotaRead"]["freshnessAnchor"],"observationSequence":current["observationSequence"]
    }))
}

#[cfg(test)]
#[path = "remaining_draft_tests.rs"]
mod tests;
