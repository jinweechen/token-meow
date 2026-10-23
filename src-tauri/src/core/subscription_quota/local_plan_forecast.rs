//! Conditional forecasts of a local Token plan, never official quota capacity.
//! The caller owns the scan transaction and supplies Core-normalized accounting.
//! Reads never renew a sample or write a plan, reminder, or notification.
use super::super::{err, Result};
use crate::budget_clock::Reading;
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;

const VERSION: &str = "local-plan-forecast-experimental-v1";
const MAX_POINTS: usize = 512;
const POINT_INTERVAL_MS: u64 = 10_000;
const MAX_GAP_MS: u64 = 15_000;
const MAX_SKEW_MS: i128 = 2_000;
const MAX_FRESH_APPEND_LATENESS_MS: i64 = 15_000;
const MIN_POINTS: usize = 6;
const MIN_SPAN_MS: i64 = 60_000;
const MIN_DISTINCT: usize = 3;
const LAST_CHANGE_MS: i64 = 600_000;
const PREFERRED_MS: i64 = 1_800_000;
const MAX_LOOKBACK_MS: i64 = 7_200_000;
const SAFE: f64 = 9_007_199_254_740_991.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Key {
    plan_id: String,
    source_id: String,
    model_dimension: String,
    model: Option<String>,
    effective_from: DateTime<Utc>,
    end_policy: String,
    configured_end: DateTime<Utc>,
    window_id: Option<String>,
    period_instance_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Point {
    utc: DateTime<Utc>,
    // Process-relative counters remain lossless when the JSON reaches JS.
    mono_ms: String,
    used: f64,
    #[serde(default)]
    ledger_fingerprint: Option<String>,
    #[serde(default)]
    ledger_revision: Option<u64>,
}

/// Writer-only authority for a narrowly verified append, never an IPC or
/// persisted DTO. The Core caller must first prove that removing only this
/// scan's eligible new live facts exactly restores the old head fingerprint,
/// without masking any existing fact, alias or exclusion changes.
#[derive(Clone, Debug)]
pub(super) struct FreshAppendProof {
    key: Key,
    runtime_id: String,
    plan_revision: u64,
    old_head_utc: DateTime<Utc>,
    old_head_fingerprint: String,
    new_head_utc: DateTime<Utc>,
    new_head_fingerprint: String,
    new_head_ledger_revision: u64,
    truncate_from: DateTime<Utc>,
    verified_at: Reading,
}
impl FreshAppendProof {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn verified(
        plan: &Value,
        runtime: &str,
        old_head_utc: DateTime<Utc>,
        old_head_fingerprint: &str,
        new_head_utc: DateTime<Utc>,
        new_head_fingerprint: &str,
        truncate_from: DateTime<Utc>,
        new_head_ledger_revision: u64,
        verified_at: &Reading,
    ) -> Option<Self> {
        let key = key(plan)?;
        let age = verified_at.utc - truncate_from;
        if text(&json!(runtime)).is_none()
            || text(&json!(old_head_fingerprint)).is_none()
            || text(&json!(new_head_fingerprint)).is_none()
            || truncate_from < key.effective_from
            || truncate_from >= old_head_utc
            || old_head_utc >= new_head_utc
            || new_head_utc > verified_at.utc
            || age < Duration::zero()
            || age > Duration::milliseconds(MAX_FRESH_APPEND_LATENESS_MS)
            || new_head_ledger_revision > SAFE as u64
            || verified_at.mono_ms == u64::MAX
        {
            return None;
        }
        Some(Self {
            key,
            runtime_id: runtime.into(),
            plan_revision: revision(plan)?,
            old_head_utc,
            old_head_fingerprint: old_head_fingerprint.into(),
            new_head_utc,
            new_head_fingerprint: new_head_fingerprint.into(),
            new_head_ledger_revision,
            truncate_from,
            verified_at: *verified_at,
        })
    }

    fn matches(
        &self,
        plan: &Value,
        runtime: &str,
        previous: &Point,
        next: &Point,
        at: &Reading,
    ) -> bool {
        key(plan).as_ref() == Some(&self.key)
            && revision(plan) == Some(self.plan_revision)
            && self.runtime_id == runtime
            && self.verified_at == *at
            && previous.utc == self.old_head_utc
            && previous.ledger_fingerprint.as_deref() == Some(self.old_head_fingerprint.as_str())
            && next.utc == self.new_head_utc
            && next.ledger_fingerprint.as_deref() == Some(self.new_head_fingerprint.as_str())
            && next.ledger_revision == Some(self.new_head_ledger_revision)
            && next.used > previous.used
            && self.truncate_from >= self.key.effective_from
            && self.truncate_from < previous.utc
            && previous.utc < next.utc
            && (at.utc - self.truncate_from) >= Duration::zero()
            && (at.utc - self.truncate_from) <= Duration::milliseconds(MAX_FRESH_APPEND_LATENESS_MS)
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Capture {
    schema_version: u32,
    algorithm_version: String,
    key: Option<Key>,
    runtime_id: String,
    revision: u64,
    head: Option<Point>,
    points: Vec<Point>,
    last_change: Option<DateTime<Utc>>,
    break_reason: Option<String>,
}
fn text(v: &Value) -> Option<&str> {
    v.as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= 2000)
}
fn instant(v: &Value) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(v.as_str()?)
        .ok()
        .map(|v| v.with_timezone(&Utc))
}
fn timestamp(v: DateTime<Utc>) -> String {
    v.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}
fn amount(v: &Value) -> Option<f64> {
    v.as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0 && *n <= SAFE && n.fract() == 0.0)
}
fn revision(p: &Value) -> Option<u64> {
    p["revision"]
        .as_u64()
        .filter(|v| *v > 0 && *v <= SAFE as u64)
}
fn key(p: &Value) -> Option<Key> {
    if p["type"] != "tokens" {
        return None;
    }
    let dimension = text(&p["modelDimension"])?;
    if !matches!(dimension, "request" | "response") {
        return None;
    }
    let policy = text(&p["endPolicy"])?;
    if !matches!(policy, "time" | "window") {
        return None;
    }
    let window = if policy == "window" {
        (
            Some(text(&p["windowId"])?.to_owned()),
            Some(text(&p["periodInstanceId"])?.to_owned()),
        )
    } else {
        (None, None)
    };
    Some(Key {
        plan_id: text(&p["id"])?.into(),
        source_id: text(&p["sourceId"])?.into(),
        model_dimension: dimension.into(),
        model: if p["model"].is_null() {
            None
        } else {
            Some(text(&p["model"])?.into())
        },
        effective_from: instant(&p["effectiveFrom"])?,
        end_policy: policy.into(),
        configured_end: instant(&p["endsAt"])?,
        window_id: window.0,
        period_instance_id: window.1,
    })
}
fn state(db: &Connection, id: &str) -> Result<Value> {
    let raw: Option<String> = db.query_row("SELECT data FROM plan_period_state WHERE owner_kind='plan' AND owner_id=?1 AND period_instance_id=?1", [id], |r| r.get(0)).optional().map_err(err)?;
    let value = raw
        .map(|s| serde_json::from_str(&s).map_err(err))
        .transpose()?
        .unwrap_or_else(|| json!({}));
    if !value.is_object() {
        return Err("本机计划预测状态格式无效".into());
    }
    Ok(value)
}
fn load(v: &Value) -> Option<Capture> {
    let capture: Capture = serde_json::from_value(v.get("localForecast")?.clone()).ok()?;
    (capture.schema_version == 1
        && capture.algorithm_version == VERSION
        && capture.points.len() <= MAX_POINTS)
        .then_some(capture)
}
fn save(db: &Connection, id: &str, mut original: Value, capture: &Capture) -> Result<()> {
    original["localForecast"] = serde_json::to_value(capture).map_err(err)?;
    db.execute("INSERT INTO plan_period_state(owner_kind,owner_id,period_instance_id,data) VALUES('plan',?1,?1,?2) ON CONFLICT(owner_kind,owner_id,period_instance_id) DO UPDATE SET data=excluded.data", params![id,original.to_string()]).map_err(err)?;
    Ok(())
}
fn blank(p: &Value, runtime: &str, reason: Option<&str>) -> Capture {
    Capture {
        schema_version: 1,
        algorithm_version: VERSION.into(),
        key: key(p),
        runtime_id: runtime.into(),
        revision: revision(p).unwrap_or(0),
        head: None,
        points: vec![],
        last_change: None,
        break_reason: reason.map(str::to_owned),
    }
}
fn elapsed(previous: &Point, now: &Reading) -> Option<u64> {
    let previous_mono = previous.mono_ms.parse::<u64>().ok()?;
    if previous_mono.to_string() != previous.mono_ms || now.mono_ms == u64::MAX {
        return None;
    }
    let mono = now.mono_ms.checked_sub(previous_mono)?;
    let wall = (now.utc - previous.utc).num_milliseconds();
    (wall >= 0
        && wall <= MAX_GAP_MS as i64
        && mono <= MAX_GAP_MS
        && (i128::from(wall) - i128::from(mono)).abs() <= MAX_SKEW_MS)
        .then_some(mono)
}
fn baseline_clock_agrees(first: &Point, last: &Point) -> bool {
    let pair = first
        .mono_ms
        .parse::<u64>()
        .ok()
        .zip(last.mono_ms.parse::<u64>().ok());
    pair.and_then(|(a, b)| b.checked_sub(a))
        .is_some_and(|mono| {
            let wall = (last.utc - first.utc).num_milliseconds();
            wall >= 0 && (i128::from(wall) - i128::from(mono)).abs() <= MAX_SKEW_MS
        })
}
fn current(
    plan: &Value,
    view: &Value,
    at: &Reading,
) -> std::result::Result<(Point, f64, f64, DateTime<Utc>), &'static str> {
    if key(plan).is_none() || revision(plan).is_none() {
        return Err("local_forecast_scope_unknown");
    }
    if view["id"] != plan["id"]
        || view["sourceId"] != plan["sourceId"]
        || view["revision"] != plan["revision"]
    {
        return Err("local_forecast_view_mismatch");
    }
    if view["evaluationState"] != "ready"
        || view["coverage"] != "recorded-local"
        || !view["reasons"].as_array().is_some_and(Vec::is_empty)
    {
        return Err("local_forecast_coverage_unknown");
    }
    if plan["lifecycle"] != "active" || view["lifecycle"] != "active" || !plan["endedAt"].is_null()
    {
        return Err("local_forecast_period_ended");
    }
    let used = amount(&view["used"]).ok_or("local_forecast_usage_unknown")?;
    let target = amount(&plan["target"])
        .filter(|v| *v > 0.0)
        .ok_or("local_forecast_target_unknown")?;
    let remaining = amount(&view["remaining"])
        .filter(|v| *v == (target - used).max(0.0))
        .ok_or("local_forecast_view_mismatch")?;
    let until = instant(&view["measuredUntil"]).ok_or("local_forecast_measurement_unknown")?;
    let lag = (at.utc - until).num_milliseconds();
    if !(0..=MAX_SKEW_MS as i64).contains(&lag) || at.mono_ms == u64::MAX {
        return Err("local_forecast_measurement_stale");
    }
    let end = instant(&view["endsAt"])
        .filter(|e| *e > at.utc && *e > until)
        .ok_or("local_forecast_period_ended")?;
    if instant(&plan["effectiveFrom"]).is_none_or(|start| start > until) {
        return Err("local_forecast_scope_unknown");
    }
    let mono = at
        .mono_ms
        .checked_sub(lag as u64)
        .ok_or("local_forecast_clock_discontinuous")?;
    Ok((
        Point {
            utc: until,
            mono_ms: mono.to_string(),
            used,
            ledger_fingerprint: text(&view["forecastLedgerFingerprint"]).map(str::to_owned),
            ledger_revision: view["ledgerRevision"].as_u64(),
        },
        target,
        remaining,
        end,
    ))
}

/// Must run inside the same writer transaction as the Core scan. `reliable_scan`
/// excludes imports/backfills/replays and any interval without local coverage.
/// Turning reminders off is deliberately absent from the sampling identity.
pub(crate) fn observe(
    db: &Connection,
    plan: &Value,
    view: &Value,
    runtime: &str,
    at: &Reading,
    reliable_scan: bool,
) -> Result<()> {
    observe_with_fresh_append(db, plan, view, runtime, at, reliable_scan, None)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn observe_with_fresh_append(
    db: &Connection,
    plan: &Value,
    view: &Value,
    runtime: &str,
    at: &Reading,
    reliable_scan: bool,
    fresh_append: Option<&FreshAppendProof>,
) -> Result<()> {
    if db.is_autocommit() {
        return Err("本机计划预测必须在扫描事务内维护".into());
    }
    let Some(id) = text(&plan["id"]) else {
        return Ok(());
    };
    let original = state(db, id)?;
    let mut capture = load(&original).unwrap_or_else(|| blank(plan, runtime, None));
    let next = if !reliable_scan {
        Err("local_forecast_scan_unreliable")
    } else if text(&json!(runtime)).is_none() {
        Err("local_forecast_scope_unknown")
    } else {
        current(plan, view, at)
    };
    let (point, _, _, _) = match next {
        Ok(value) => value,
        Err(reason) => return save(db, id, original, &blank(plan, runtime, Some(reason))),
    };
    // The original dense head remains the clock/collection witness even when
    // its old count is incomplete. A pruned ten-second trend anchor is not the
    // acquisition clock, and may legitimately be farther than MAX_GAP_MS away.
    let clock_invalid = capture.head.as_ref().is_some_and(|h| {
        elapsed(
            h,
            &Reading {
                utc: point.utc,
                mono_ms: point.mono_ms.parse().unwrap_or(u64::MAX),
            },
        )
        .is_none()
    }) || capture
        .points
        .first()
        .is_some_and(|first| !baseline_clock_agrees(first, &point));
    let ledger_changed = capture.head.as_ref().is_some_and(|h| {
        h.ledger_fingerprint.as_deref().is_none()
            || h.ledger_fingerprint.as_deref() != text(&view["forecastLedgerHeadFingerprint"])
    });
    let accepted_append = fresh_append.filter(|proof| {
        !clock_invalid
            && ledger_changed
            && capture
                .head
                .as_ref()
                .is_some_and(|head| proof.matches(plan, runtime, head, &point, at))
    });
    let reason = if capture.runtime_id != runtime {
        Some("local_forecast_runtime_changed")
    } else if capture.key != key(plan) {
        Some("local_forecast_scope_changed")
    } else if revision(plan).is_none_or(|r| r < capture.revision) {
        Some("local_forecast_revision_regressed")
    } else if ledger_changed && accepted_append.is_none() {
        Some("local_forecast_ledger_changed")
    } else if capture.head.as_ref().is_some_and(|h| point.used < h.used) {
        Some("local_forecast_usage_reduced")
    } else if clock_invalid {
        Some("local_forecast_clock_discontinuous")
    } else {
        None
    };
    if let Some(reason) = reason {
        capture = blank(plan, runtime, Some(reason));
    } else if let Some(proof) = accepted_append {
        // Conservatively discard equality too. Every retained cumulative count
        // predates all admitted late events and remains an intact rate anchor.
        capture.points.retain(|p| p.utc < proof.truncate_from);
        capture.head = capture.points.last().cloned();
        capture.last_change = capture.last_change.filter(|t| *t < proof.truncate_from);
    }
    if capture.head.as_ref().is_some_and(|h| point.used > h.used) {
        capture.last_change = Some(point.utc);
    }
    let point_mono = point.mono_ms.parse::<u64>().map_err(err)?;
    if capture.points.last().is_none_or(|p| {
        p.mono_ms
            .parse::<u64>()
            .ok()
            .and_then(|m| point_mono.checked_sub(m))
            .is_some_and(|gap| gap >= POINT_INTERVAL_MS)
            && (point.utc - p.utc).num_milliseconds() >= POINT_INTERVAL_MS as i64
    }) {
        capture.points.push(point.clone());
    }
    capture
        .points
        .retain(|p| (point.utc - p.utc).num_milliseconds() <= MAX_LOOKBACK_MS);
    // A more recent head is itself a persisted observation, so reserve a slot
    // when it does not coincide with the last throttled trend point.
    let point_limit =
        MAX_POINTS - usize::from(capture.points.last().is_some_and(|p| p.utc != point.utc));
    if capture.points.len() > point_limit {
        capture.points.drain(..capture.points.len() - point_limit);
    }
    capture.head = Some(point);
    capture.revision = revision(plan).unwrap_or(0);
    save(db, id, original, &capture)
}

fn parameters() -> Value {
    json!({"status":"local-experimental-initial-values","maxSamples":MAX_POINTS,"sampleIntervalSeconds":10,"maxContinuousGapSeconds":15,"maxClockSkewSeconds":2,"maxFreshAppendLatenessSeconds":MAX_FRESH_APPEND_LATENESS_MS/1000,"freshAppendPolicy":"writer-proof-prune-affected-tail-v1","minSamples":MIN_POINTS,"minSpanSeconds":60,"minDistinctUsageValues":MIN_DISTINCT,"lastChangeMaxAgeSeconds":600,"preferredLookbackMinutes":30,"maxLookbackMinutes":120})
}
fn empty(plan: &Value, runtime: &str, reason: &str) -> Value {
    let common = json!({"state":"unavailable","scope":"local-plan","condition":"按本机计划最近活动速度继续使用","planId":plan["id"],"sourceId":plan["sourceId"],"modelDimension":plan["modelDimension"],"model":plan["model"],"runtimeId":runtime,"planConfigurationRevision":plan["revision"],"asOfRevision":null,"inputLedgerFingerprint":null,"algorithmVersion":VERSION,"parameters":parameters(),"quality":"conditional-local-experiment","notifyEligible":false,"uncertaintyBounds":null,"empiricalScenarioRange":null,"relationToPeriodEnd":null,"willReachBeforePeriodEnd":null,"asOf":null,"reasons":[reason]});
    let mut eta = common.clone();
    eta["kind"] = json!("local_plan_eta");
    let mut end = common;
    end["kind"] = json!("period_end_usage");
    json!({"localPlanEta":eta,"periodEndUsage":end})
}
fn unavailable(plan: &Value, runtime: &str, reason: &str) -> Result<Value> {
    Ok(empty(plan, runtime, reason))
}
fn minute(v: DateTime<Utc>, up: bool) -> Option<DateTime<Utc>> {
    let ms = v.timestamp_millis();
    let base = ms.div_euclid(60_000).checked_mul(60_000)?;
    DateTime::from_timestamp_millis(if up && ms != base {
        base.checked_add(60_000)?
    } else {
        base
    })
}
fn eta_at(at: DateTime<Utc>, remaining: f64, rate: f64) -> Option<DateTime<Utc>> {
    let ms = (remaining / rate * 1000.0).ceil();
    if !ms.is_finite() || ms < 0.0 || ms > i64::MAX as f64 {
        return None;
    }
    at.checked_add_signed(Duration::try_milliseconds(ms as i64)?)
}
/// Read-only. The persisted head, not the query time or the current UI's
/// measuredUntil, anchors both scenarios. Scenario extrema are empirical
/// baseline comparisons, explicitly not statistical or guaranteed bounds.
pub(crate) fn projection(
    db: &Connection,
    plan: &Value,
    view: &Value,
    runtime: &str,
    at: &Reading,
) -> Result<Value> {
    let Some(id) = text(&plan["id"]) else {
        return unavailable(plan, runtime, "local_forecast_scope_unknown");
    };
    let Some(c) = load(&state(db, id)?) else {
        return unavailable(plan, runtime, "local_forecast_samples_insufficient");
    };
    let (_, target, remaining, end) = match current(plan, view, at) {
        Ok(v) => v,
        Err(reason) => return unavailable(plan, runtime, reason),
    };
    if c.runtime_id != runtime
        || c.key != key(plan)
        || revision(plan).is_none_or(|r| r < c.revision)
    {
        return unavailable(plan, runtime, "local_forecast_scope_changed");
    }
    let Some(head) = &c.head else {
        return unavailable(
            plan,
            runtime,
            c.break_reason
                .as_deref()
                .unwrap_or("local_forecast_samples_insufficient"),
        );
    };
    if head.ledger_fingerprint.as_deref().is_none()
        || head.ledger_fingerprint.as_deref() != text(&view["forecastLedgerHeadFingerprint"])
    {
        return unavailable(plan, runtime, "local_forecast_ledger_changed");
    }
    if amount(&view["used"]) != Some(head.used) {
        return unavailable(plan, runtime, "local_forecast_measurement_unobserved");
    }
    if elapsed(head, at).is_none() {
        return unavailable(plan, runtime, "local_forecast_sample_expired");
    }
    if capture_clock_invalid(&c, head) {
        return unavailable(plan, runtime, "local_forecast_clock_discontinuous");
    }
    let mut result = empty(plan, runtime, "local_forecast_samples_insufficient");
    for field in ["localPlanEta", "periodEndUsage"] {
        result[field]["asOfRevision"] = json!(head.ledger_revision);
        result[field]["revisionKind"] = json!("subscription-domain-revision");
        result[field]["inputLedgerFingerprint"] = json!(head.ledger_fingerprint);
    }
    if remaining == 0.0 {
        result["localPlanEta"]["state"] = json!("reached");
        result["localPlanEta"]["asOf"] = json!(timestamp(head.utc));
        result["localPlanEta"]["reasons"] = json!([]);
    }
    let last_change = c
        .last_change
        .filter(|t| *t <= head.utc && (at.utc - *t).num_milliseconds() < LAST_CHANGE_MS);
    if last_change.is_none() {
        result["periodEndUsage"]["reasons"] = json!(["local_forecast_recent_change_missing"]);
        if remaining > 0.0 {
            result["localPlanEta"]["reasons"] = json!(["local_forecast_recent_change_missing"]);
        }
        return Ok(result);
    }
    let rates_for = |lookback: i64| -> Vec<f64> {
        let mut values = c
            .points
            .iter()
            .filter(|p| p.utc <= head.utc && (head.utc - p.utc).num_milliseconds() <= lookback)
            .cloned()
            .collect::<Vec<_>>();
        if values.last().is_none_or(|p| p.utc != head.utc) {
            values.push(head.clone());
        }
        let mut rates = vec![];
        for index in 0..values.len() {
            let suffix = &values[index..];
            let first = &suffix[0];
            let span = (head.utc - first.utc).num_milliseconds();
            let distinct = suffix
                .iter()
                .map(|p| p.used as u64)
                .collect::<BTreeSet<_>>()
                .len();
            if suffix.len() >= MIN_POINTS
                && span >= MIN_SPAN_MS
                && distinct >= MIN_DISTINCT
                && head.used > first.used
                && baseline_clock_agrees(first, head)
            {
                let rate = (head.used - first.used) / (span as f64 / 1000.0);
                if rate.is_finite() && rate > 0.0 {
                    rates.push(rate);
                }
            }
        }
        rates
    };
    let mut lookback = PREFERRED_MS;
    let mut rates = rates_for(lookback);
    if rates.is_empty() {
        lookback = MAX_LOOKBACK_MS;
        rates = rates_for(lookback);
    }
    if rates.is_empty() {
        return Ok(result);
    }
    let low = rates.iter().copied().fold(f64::INFINITY, f64::min);
    let high = rates.iter().copied().fold(0.0, f64::max);
    let span = (end - head.utc).num_milliseconds() as f64 / 1000.0;
    let minimum = (head.used + low * span).floor();
    let maximum = (head.used + high * span).ceil();
    if !minimum.is_finite() || !maximum.is_finite() || maximum > SAFE {
        return unavailable(plan, runtime, "local_forecast_numeric_overflow");
    }
    for field in ["localPlanEta", "periodEndUsage"] {
        let value = &mut result[field];
        value["state"] = json!(if field == "localPlanEta" && remaining == 0.0 {
            "reached"
        } else {
            "conditional"
        });
        value["reasons"] = json!([]);
        value["asOf"] = json!(timestamp(head.utc));
        value["used"] = json!(head.used);
        value["remaining"] = json!(remaining);
        value["target"] = json!(target);
        value["periodEndsAt"] = json!(timestamp(end));
        value["lastChangeAt"] = json!(last_change.map(timestamp));
        value["sampleCount"] =
            json!(c.points.len() + usize::from(c.points.last().is_some_and(|p| p.utc != head.utc)));
        value["trendSampleCount"] = json!(c.points.len());
        value["distinctUsageCount"] = json!(c
            .points
            .iter()
            .map(|p| p.used as u64)
            .chain(std::iter::once(head.used as u64))
            .collect::<BTreeSet<_>>()
            .len());
        value["sampledPlanRevision"] = json!(c.revision);
        value["asOfRevision"] = json!(head.ledger_revision);
        value["inputLedgerFingerprint"] = json!(head.ledger_fingerprint);
        value["sampledFrom"] = json!(c.points.first().map(|p| timestamp(p.utc)));
        value["speedBasis"] = json!("covered-local-token-deltas");
        value["baselineCount"] = json!(rates.len());
        value["lookbackMinutes"] = json!(lookback / 60_000);
        value["speedUnit"] = json!("local-tokens-per-second");
        value["speedScenarios"] = json!({"minimum":low,"maximum":high});
        value["rangeMeaning"] =
            json!("empirical-recent-baseline-scenarios-not-confidence-or-guaranteed-bounds");
        value["breakReason"] = json!(c.break_reason);
    }
    result["periodEndUsage"]["empiricalScenarioRange"] = json!({"minimum":minimum,"maximum":maximum,"unit":"local-plan-tokens","rounding":"outward-whole-token"});
    if remaining > 0.0 {
        let endpoints = eta_at(head.utc, remaining, high)
            .and_then(|earliest| minute(earliest, false))
            .zip(eta_at(head.utc, remaining, low).and_then(|latest| minute(latest, true)));
        if let Some((earliest, latest)) = endpoints {
            result["localPlanEta"]["empiricalScenarioRange"] = json!({"earliestAt":timestamp(earliest),"latestAt":timestamp(latest),"rounding":"outward-minute"});
            let relation = if latest < end {
                "before"
            } else if earliest > end {
                "after"
            } else {
                "overlapping"
            };
            result["localPlanEta"]["relationToPeriodEnd"] = json!(relation);
            result["localPlanEta"]["willReachBeforePeriodEnd"] = match relation {
                "before" => json!(true),
                "after" => json!(false),
                _ => Value::Null,
            };
            result["localPlanEta"]["periodEndJudgmentBasis"] =
                json!("empirical-scenarios-only-not-a-guarantee");
        } else {
            return unavailable(plan, runtime, "local_forecast_numeric_overflow");
        }
    }
    Ok(result)
}

fn capture_clock_invalid(c: &Capture, head: &Point) -> bool {
    c.points
        .first()
        .is_some_and(|first| !baseline_clock_agrees(first, head))
}

#[cfg(test)]
#[path = "local_plan_forecast_tests.rs"]
mod tests;
