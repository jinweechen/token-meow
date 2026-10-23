//! Transactional panel-only crossings of an official percentage target.
//! No native permit, OS notification, or pet animation is created here.
use super::super::{err, hash, Result};
use super::window_plans;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Baseline {
    schema_version: u32,
    scope: Value,
    window_instance_id: String,
    observation_sequence: String,
    used_percent: f64,
    target_percent: f64,
    freshness_anchor: DateTime<Utc>,
    resolved_end: DateTime<Utc>,
}

struct Observation<'a> {
    snapshot: &'a Value,
    scope: Value,
    sequence: u64,
    anchor: DateTime<Utc>,
}

fn text(v: &Value) -> Option<&str> {
    v.as_str().filter(|s| !s.trim().is_empty())
}
fn decimal(v: &Value) -> Option<u64> {
    let s = v.as_str()?;
    let n = s.parse::<u64>().ok()?;
    (n.to_string() == s).then_some(n)
}
fn instant(v: &Value) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(v.as_str()?)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}
fn timestamp(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
fn percent(v: &Value) -> Option<f64> {
    v.as_f64()
        .filter(|n| n.is_finite() && (0.0..=100.0).contains(n))
}

fn observation<'a>(
    snapshot: &'a Value,
    runtime: &str,
    at: DateTime<Utc>,
) -> Option<Observation<'a>> {
    if snapshot["runtimeId"].as_str()? != runtime
        || snapshot["identityConsistency"] != "proven"
        || !snapshot["identityEvidence"].is_object()
        || snapshot["observationOutcome"] != "success"
        || snapshot["status"] != "connected"
        || snapshot["quotaRead"]["outcome"] != "success"
        || snapshot["historyOnly"] == true
        || snapshot["replayed"] == true
        || snapshot["live"] == false
        || !snapshot["persistenceError"].is_null()
    {
        return None;
    }
    let source = text(&snapshot["sourceId"])?;
    let binding = decimal(&snapshot["bindingGeneration"])?;
    let identity = decimal(&snapshot["identityEpoch"]).filter(|n| *n > 0)?;
    let sequence = decimal(&snapshot["observationSequence"]).filter(|n| *n > 0)?;
    let clear = snapshot.get("clearGeneration")?;
    let anchor = instant(&snapshot["quotaRead"]["freshnessAnchor"])?;
    if anchor > at || (at - anchor).num_milliseconds() >= 240_000 {
        return None;
    }
    Some(Observation {
        snapshot,
        sequence,
        anchor,
        scope: json!({
            "runtimeId":runtime,"sourceId":source,"bindingGeneration":binding.to_string(),
            "identityEpoch":identity.to_string(),"clearGeneration":clear,
            "accountScope":snapshot.get("accountScope").unwrap_or(&Value::Null)
        }),
    })
}

/// Both committed previous reads and the current in-transaction receipt must
/// exist. The current snapshot's pre-commit persisted=false is not itself proof.
fn has_receipt(db: &Connection, observation: &Observation<'_>) -> Result<bool> {
    let mut query=db.prepare(
        "SELECT data FROM account_snapshots WHERE json_extract(data,'$.documentKind')='subscription_observation' AND json_extract(data,'$.runtimeId')=?1 AND json_extract(data,'$.sourceId')=?2 AND json_extract(data,'$.observationSequence')=?3 ORDER BY id DESC LIMIT 2"
    ).map_err(err)?;
    let rows = query
        .query_map(
            params![
                observation.scope["runtimeId"].as_str(),
                observation.scope["sourceId"].as_str(),
                observation.sequence.to_string()
            ],
            |r| r.get::<_, String>(0),
        )
        .map_err(err)?;
    for row in rows {
        let stored: Value = serde_json::from_str(&row.map_err(err)?).map_err(err)?;
        let receipt = super::observation_storage::resolve(db, &stored)?;
        if receipt["schemaVersion"] != 1
            || receipt["outcome"] != "success"
            || decimal(&receipt["lastPersistedSequence"]) != Some(observation.sequence)
        {
            continue;
        }
        let Some(raw) = super::calibration_assembler::hydrated_observation(&receipt) else {
            continue;
        };
        let keys = [
            "runtimeId",
            "sourceId",
            "bindingGeneration",
            "identityEpoch",
            "clearGeneration",
            "observationSequence",
        ];
        if keys.iter().any(|key| {
            raw.get(*key).is_none()
                || raw[*key] != observation.snapshot[*key]
                || receipt[*key] != observation.snapshot[*key]
        }) {
            continue;
        }
        if [
            "identityConsistency",
            "identityEvidence",
            "observationOutcome",
            "status",
            "quotaRead",
            "buckets",
        ]
        .iter()
        .all(|key| raw[*key] == observation.snapshot[*key])
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn window<'a>(snapshot: &'a Value, id: &str) -> Option<&'a Value> {
    for bucket in snapshot["buckets"].as_array().into_iter().flatten() {
        let Some(bucket_id) = text(&bucket["id"]) else {
            continue;
        };
        for kind in ["primary", "secondary"] {
            if id == format!("{bucket_id}:{kind}") && bucket[kind].is_object() {
                return Some(&bucket[kind]);
            }
        }
    }
    None
}
fn latest_plans(db: &Connection) -> Result<Vec<Value>> {
    let mut query=db.prepare("SELECT p.data FROM usage_plans p JOIN (SELECT plan_id,MAX(revision) revision FROM usage_plans GROUP BY plan_id) c ON c.plan_id=p.plan_id AND c.revision=p.revision ORDER BY p.plan_id").map_err(err)?;
    let rows = query
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(err)?;
    rows.map(|row| serde_json::from_str(&row.map_err(err)?).map_err(err))
        .collect()
}
fn state(db: &Connection, id: &str) -> Result<Value> {
    let stored:Option<String>=db.query_row("SELECT data FROM plan_period_state WHERE owner_kind='plan' AND owner_id=?1 AND period_instance_id=?1",[id],|r|r.get(0)).optional().map_err(err)?;
    stored
        .map(|v| serde_json::from_str(&v).map_err(err))
        .transpose()
        .map(|v| v.unwrap_or_else(|| json!({})))
}
fn save(db: &Connection, id: &str, value: &Value) -> Result<()> {
    db.execute("INSERT INTO plan_period_state(owner_kind,owner_id,period_instance_id,data) VALUES('plan',?1,?1,?2) ON CONFLICT(owner_kind,owner_id,period_instance_id) DO UPDATE SET data=excluded.data",params![id,value.to_string()]).map_err(err)?;
    Ok(())
}

/// Call after window_plans::advance, inside the same admitted-read transaction.
/// Errors must roll back the caller's receipt, latch, and inbox event together.
pub(crate) fn advance(
    db: &Connection,
    previous: Option<&Value>,
    current: &Value,
    runtime: &str,
    at: DateTime<Utc>,
) -> Result<()> {
    if db.is_autocommit() {
        return Err("官方比例提醒必须与观测收据在同一事务内提交".into());
    }
    let Some(current) = observation(current, runtime, at) else {
        return Ok(());
    };
    if !has_receipt(db, &current)? {
        return Ok(());
    };
    let previous = previous
        .filter(|v| v["persisted"] == true)
        .and_then(|v| observation(v, runtime, at));
    let previous = match previous {
        Some(value)
            if value.scope == current.scope
                && value.sequence < current.sequence
                && has_receipt(db, &value)? =>
        {
            Some(value)
        }
        _ => None,
    };
    for plan in latest_plans(db)? {
        if plan["type"] != "percent"
            || plan["endPolicy"] != "window"
            || plan["lifecycle"] != "active"
            || plan["sourceId"] != current.scope["sourceId"]
        {
            continue;
        }
        let (Some(id), Some(bound), Some(window_id), Some(target), Some(starts)) = (
            text(&plan["id"]),
            text(&plan["periodInstanceId"]),
            text(&plan["windowId"]),
            percent(&plan["target"]).filter(|v| *v > 0.0),
            instant(&plan["effectiveFrom"]),
        ) else {
            continue;
        };
        let Some(current_window) = window(current.snapshot, window_id) else {
            continue;
        };
        let Some(used) = percent(&current_window["usedPercent"]) else {
            continue;
        };
        if text(&current_window["windowInstanceId"]) != Some(bound) || current.anchor < starts {
            continue;
        }
        let projection = window_plans::projection(db, &plan, at)?;
        let Some(resolved_end) = projection.resolved_end.filter(|end| *end > at) else {
            continue;
        };
        if !projection.same_window
            || projection.archived
            || projection.partial
            || projection.proof_scope.as_ref() != Some(&current.scope)
        {
            continue;
        }
        let mut original = state(db, id)?;
        let baseline = original
            .get("officialBaseline")
            .map(|v| serde_json::from_value::<Baseline>(v.clone()).map_err(err))
            .transpose()?;
        if let Some(baseline) = baseline.as_ref() {
            if baseline.schema_version != 1 {
                return Err("官方提醒基线版本不受支持".into());
            }
            if baseline.scope == current.scope
                && decimal(&json!(baseline.observation_sequence))
                    .is_some_and(|sequence| sequence >= current.sequence)
            {
                continue;
            }
        }
        let mut latched = original["latched"].as_array().cloned().unwrap_or_default();
        let can_cross = original["officialBaselinePending"] != true
            && plan["remindersEnabled"] == true
            && !latched.contains(&json!(100))
            && previous
                .as_ref()
                .zip(baseline.as_ref())
                .is_some_and(|(previous, baseline)| {
                    let Some(previous_window) = window(previous.snapshot, window_id) else {
                        return false;
                    };
                    let Some(previous_used) = percent(&previous_window["usedPercent"]) else {
                        return false;
                    };
                    baseline.scope == current.scope
                        && baseline.window_instance_id == bound
                        && decimal(&json!(baseline.observation_sequence)) == Some(previous.sequence)
                        && baseline.freshness_anchor == previous.anchor
                        && baseline.target_percent == target
                        && baseline.used_percent == previous_used
                        && baseline.resolved_end > at
                        && previous.anchor >= starts
                        && text(&previous_window["windowInstanceId"]) == Some(bound)
                        && previous_used < target
                        && used >= target
                        && used > previous_used
                });
        if can_cross {
            let event_id = hash(format!("plan|{id}|{bound}|official_target|100|0").as_bytes());
            let event = json!({"id":event_id,"kind":"subscription_plan","reminderKind":"official_target",
                "ownerKind":"plan","ownerId":id,"sourceId":plan["sourceId"],"threadId":"","turnId":null,
                "eventTime":timestamp(at),"createdAt":timestamp(at),"title":"官方额度达到计划目标",
                "message":format!("官方窗口已用 {used}%，达到设定上限 {target}%。"),"planId":id,
                "periodInstanceId":bound,"windowId":window_id,"planRevision":plan["revision"],"threshold":100,"level":100,"riskEpisode":0,"officialTargetPercent":target,
                "usedPercent":used,"inputObservationSequences":[previous.as_ref().unwrap().sequence.to_string(),current.sequence.to_string()],
                "notifyEligible":false,"animationAcknowledged":true,"eligibleRuntime":runtime,
                "notificationState":"panel_only","notified":false,"read":false});
            db.execute("INSERT OR IGNORE INTO inbox_events(id,thread_id,turn_id,event_time,data) VALUES(?1,'',NULL,?2,?3)",params![event_id,timestamp(at),event.to_string()]).map_err(err)?;
            latched.push(json!(100));
            original["latched"] = json!(latched);
        }
        original["officialBaseline"] = serde_json::to_value(Baseline {
            schema_version: 1,
            scope: current.scope.clone(),
            window_instance_id: bound.into(),
            observation_sequence: current.sequence.to_string(),
            used_percent: used,
            target_percent: target,
            freshness_anchor: current.anchor,
            resolved_end,
        })
        .map_err(err)?;
        original["officialBaselinePending"] = json!(false);
        save(db, id, &original)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "official_reminders_tests.rs"]
mod tests;
