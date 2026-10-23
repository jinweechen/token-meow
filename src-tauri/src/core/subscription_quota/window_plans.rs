//! Evidence-driven lifecycle for a plan bound to one official window.
//!
//! `advance` is called inside the admitted observation/save transaction. It does
//! not change plan configuration, infer resets from timestamps, or issue alerts.
use super::super::{err, Result};
use super::evaluate::classify_reset;
use super::model::{Interval, ResetEvidence, WindowStatus};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub(crate) struct WindowPlanProjection {
    pub resolved_end: Option<DateTime<Utc>>,
    pub archived: bool,
    pub partial: bool,
    /// This proof alone is not live eligibility. The caller must also compare
    /// proof_scope with the current runtime/identity and enforce freshness.
    pub same_window: bool,
    pub final_percent: Option<f64>,
    pub proof_scope: Option<Value>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredWindowPlan {
    schema_version: u32,
    bound_instance_id: String,
    proof_scope: Value,
    last_sequence: String,
    freshness_anchor: DateTime<Utc>,
    resolved_end: Option<DateTime<Utc>>,
    archived: bool,
    partial: bool,
    same_window: bool,
    final_percent: Option<f64>,
}

fn text(value: &Value) -> Option<&str> {
    value.as_str().filter(|value| !value.trim().is_empty())
}

fn decimal(value: &Value) -> Option<u64> {
    let string = value.as_str()?;
    let number = string.parse::<u64>().ok()?;
    (number.to_string() == string).then_some(number)
}

fn instant(value: &Value) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.as_str()?)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

fn seconds(at: DateTime<Utc>) -> f64 {
    at.timestamp() as f64 + f64::from(at.timestamp_subsec_nanos()) / 1e9
}

fn exact_time(interval: Option<Interval>) -> Option<DateTime<Utc>> {
    let interval = interval
        .filter(|value| value.is_valid() && value.lower == value.upper && value.lower >= 0.0)?;
    let whole = interval.lower.floor();
    if whole >= i64::MAX as f64 {
        return None;
    }
    let nanos = ((interval.lower - whole) * 1e9).round();
    if nanos >= 1e9 {
        DateTime::<Utc>::from_timestamp((whole as i64).checked_add(1)?, 0)
    } else {
        DateTime::<Utc>::from_timestamp(whole as i64, nanos as u32)
    }
}

fn scope(snapshot: &Value) -> Option<Value> {
    let runtime = text(&snapshot["runtimeId"])?;
    let source = text(&snapshot["sourceId"])?;
    let binding = decimal(&snapshot["bindingGeneration"])?;
    let identity = decimal(&snapshot["identityEpoch"]).filter(|value| *value > 0)?;
    let clear = snapshot.get("clearGeneration")?;
    Some(json!({
        "runtimeId": runtime, "sourceId": source,
        "bindingGeneration": binding.to_string(), "identityEpoch": identity.to_string(),
        "clearGeneration": clear, "accountScope": snapshot.get("accountScope").unwrap_or(&Value::Null),
    }))
}

fn admitted(snapshot: &Value, at: DateTime<Utc>) -> Option<(Value, u64, DateTime<Utc>)> {
    if snapshot["identityConsistency"] != "proven"
        || !snapshot["identityEvidence"].is_object()
        || snapshot["observationOutcome"] != "success"
        || snapshot["status"] != "connected"
        || snapshot["quotaRead"]["outcome"] != "success"
    {
        return None;
    }
    let sequence = decimal(&snapshot["observationSequence"]).filter(|value| *value > 0)?;
    let anchor = instant(&snapshot["quotaRead"]["freshnessAnchor"])?;
    if anchor > at || (at - anchor).num_milliseconds() >= 240_000 {
        return None;
    }
    Some((scope(snapshot)?, sequence, anchor))
}

fn windows(snapshot: &Value) -> BTreeMap<String, &Value> {
    let mut values = BTreeMap::new();
    for bucket in snapshot["buckets"].as_array().into_iter().flatten() {
        let Some(id) = text(&bucket["id"]) else {
            continue;
        };
        for kind in ["primary", "secondary"] {
            if bucket[kind].is_object() {
                values.insert(format!("{id}:{kind}"), &bucket[kind]);
            }
        }
    }
    values
}

fn latest_plans(db: &Connection) -> Result<Vec<Value>> {
    let mut query = db.prepare(
        "SELECT p.data FROM usage_plans p JOIN (SELECT plan_id,MAX(revision) revision FROM usage_plans GROUP BY plan_id) c ON c.plan_id=p.plan_id AND c.revision=p.revision ORDER BY p.plan_id",
    ).map_err(err)?;
    let rows = query
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(err)?;
    rows.map(|row| serde_json::from_str(&row.map_err(err)?).map_err(err))
        .collect()
}

fn state(db: &Connection, id: &str) -> Result<Value> {
    let stored: Option<String> = db.query_row(
        "SELECT data FROM plan_period_state WHERE owner_kind='plan' AND owner_id=?1 AND period_instance_id=?1",
        [id], |row| row.get(0),
    ).optional().map_err(err)?;
    stored
        .map(|value| serde_json::from_str(&value).map_err(err))
        .transpose()
        .map(|value| value.unwrap_or_else(|| json!({})))
}

fn stored(state: &Value, bound: &str) -> Result<Option<StoredWindowPlan>> {
    let Some(value) = state.get("windowPlan") else {
        return Ok(None);
    };
    let value: StoredWindowPlan = serde_json::from_value(value.clone()).map_err(err)?;
    if value.schema_version != 1 || value.bound_instance_id != bound {
        return Err("窗口计划状态版本或绑定实例不匹配".into());
    }
    Ok(Some(value))
}

fn save(db: &Connection, id: &str, state: &Value) -> Result<()> {
    db.execute(
        "INSERT INTO plan_period_state(owner_kind,owner_id,period_instance_id,data) VALUES('plan',?1,?1,?2) ON CONFLICT(owner_kind,owner_id,period_instance_id) DO UPDATE SET data=excluded.data",
        params![id, state.to_string()],
    ).map_err(err)?;
    Ok(())
}

/// Commit only inside Core's scope/sequence-checked transaction. A new plan must
/// also be initialized here in its creation transaction, after other state writes.
/// Actual adapters own `resetEvidence`; raw `resetsAt` changes are not evidence.
pub(crate) fn advance(db: &Connection, snapshot: &Value, at: DateTime<Utc>) -> Result<()> {
    let Some((proof_scope, sequence, anchor)) = admitted(snapshot, at) else {
        return Ok(());
    };
    let windows = windows(snapshot);
    for plan in latest_plans(db)? {
        if plan["endPolicy"] != "window"
            || plan["lifecycle"] == "ended"
            || plan["sourceId"] != proof_scope["sourceId"]
        {
            continue;
        }
        let (Some(id), Some(bound), Some(window_id)) = (
            text(&plan["id"]),
            text(&plan["periodInstanceId"]),
            text(&plan["windowId"]),
        ) else {
            continue;
        };
        let Some(window) = windows.get(window_id) else {
            continue;
        };
        let Some(current_id) = text(&window["windowInstanceId"]) else {
            continue;
        };
        let mut original = state(db, id)?;
        let mut value = if let Some(value) = stored(&original, bound)? {
            if value.archived
                || value.proof_scope != proof_scope
                || sequence <= value.last_sequence.parse::<u64>().map_err(err)?
            {
                continue;
            }
            value
        } else {
            // Initialization uses the exact currently bound instance, never a
            // guessed reset anchor or an observed successor's percentage.
            if current_id != bound {
                continue;
            }
            StoredWindowPlan {
                schema_version: 1,
                bound_instance_id: bound.into(),
                proof_scope: proof_scope.clone(),
                last_sequence: "0".into(),
                freshness_anchor: anchor,
                resolved_end: instant(&plan["endsAt"]),
                archived: false,
                partial: false,
                same_window: true,
                final_percent: None,
            }
        };
        let old_end = value
            .resolved_end
            .and_then(|end| Interval::exact(seconds(end)));
        let evidence_value = &window["resetEvidence"];
        let evidence = if evidence_value.is_object() {
            serde_json::from_value::<ResetEvidence>(evidence_value.clone())
                .ok()
                .filter(|evidence| {
                    if evidence.confirmed_new_window {
                        text(&evidence_value["previousWindowInstanceId"]) == Some(bound)
                            && evidence.window_instance_id.as_deref() == Some(current_id)
                            && current_id != bound
                    } else {
                        evidence.confirmed_same_window
                            && evidence.window_instance_id.as_deref() == Some(bound)
                            && current_id == bound
                    }
                })
        } else {
            None
        };
        let decision = classify_reset(bound, old_end, evidence.as_ref(), seconds(at));
        value.last_sequence = sequence.to_string();
        value.freshness_anchor = anchor;
        // Waiting is latched. An ordinary later read cannot silently undo an
        // uncertain boundary or an observed-but-unproven instance change.
        value.same_window =
            value.same_window && current_id == bound && decision.status == WindowStatus::Ready;
        if decision.ended_previous_window {
            let actual_end = evidence
                .as_ref()
                .and_then(|evidence| exact_time(evidence.actual_boundary));
            let starts_before_end = actual_end
                .zip(instant(&plan["effectiveFrom"]))
                .is_some_and(|(end, start)| end >= start && end <= at);
            value.archived = true;
            value.partial = decision.previous_usage_partial || !starts_before_end;
            value.resolved_end = if value.partial { None } else { actual_end };
            value.same_window = false;
            value.final_percent = if value.partial {
                None
            } else {
                evidence_value["finalUsedPercent"]
                    .as_f64()
                    .filter(|value| value.is_finite() && (0.0..=100.0).contains(value))
            };
        } else if let Some(evidence) = evidence.as_ref().filter(|evidence| {
            decision.status != WindowStatus::Unknown
                && evidence.confirmed_same_window
                && !evidence.confirmed_new_window
        }) {
            if let Some(end) = exact_time(evidence.next_reset) {
                value.resolved_end = Some(end);
                value.same_window = decision.status == WindowStatus::Ready && at < end;
            } else if evidence.next_reset.is_some() {
                // An uncertain boundary cannot be collapsed into a precise
                // configured end or used to resume definite Token accounting.
                value.same_window = false;
            } else {
                value.same_window = decision.status == WindowStatus::Ready
                    && value.resolved_end.is_some_and(|end| at < end);
            }
        }
        original["windowPlan"] = serde_json::to_value(value).map_err(err)?;
        save(db, id, &original)?;
    }
    Ok(())
}

pub(crate) fn projection(
    db: &Connection,
    plan: &Value,
    at: DateTime<Utc>,
) -> Result<WindowPlanProjection> {
    let fallback = WindowPlanProjection {
        resolved_end: instant(&plan["endsAt"]),
        archived: false,
        partial: false,
        same_window: false,
        final_percent: None,
        proof_scope: None,
    };
    if plan["endPolicy"] != "window" {
        return Ok(fallback);
    }
    let (Some(id), Some(bound)) = (text(&plan["id"]), text(&plan["periodInstanceId"])) else {
        return Ok(fallback);
    };
    let Some(value) = stored(&state(db, id)?, bound)? else {
        return Ok(fallback);
    };
    let fresh =
        value.freshness_anchor <= at && (at - value.freshness_anchor).num_milliseconds() < 240_000;
    Ok(WindowPlanProjection {
        resolved_end: value.resolved_end,
        archived: value.archived,
        partial: value.partial,
        same_window: !value.archived
            && value.same_window
            && fresh
            && value.resolved_end.is_some_and(|end| at < end),
        final_percent: value.final_percent,
        proof_scope: Some(value.proof_scope),
    })
}

#[cfg(test)]
#[path = "window_plans_tests.rs"]
mod tests;
