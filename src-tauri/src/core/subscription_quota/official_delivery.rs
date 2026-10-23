//! Native admission for newly committed official target/risk transitions.
//! Historical inbox rows and caller JSON cannot construct a collection ticket.
use super::super::{err, hash, Result};
use super::{model::*, observation_storage, projector, window_plans};
use crate::{
    budget_clock::{self, Reading},
    notification_gate::{Gate, NotificationPermit},
};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

const MAX_EVENTS: usize = 400;
const MAX_INPUTS: usize = 1024;
const MAX_ROW: usize = 512 * 1024;
const MAX_BYTES: usize = 32 * 1024 * 1024;
const LEASE_MS: u64 = 15_000;

#[derive(Default)]
pub(crate) struct Delivery {
    instance: Arc<()>,
    entries: BTreeMap<String, Entry>,
    high: Option<Reading>,
}
pub(crate) struct Ticket {
    instance: Arc<()>,
    gate: Arc<Gate>,
    epoch: u64,
    at: Reading,
    version: i64,
    inbox_rowid: i64,
    prepared: AtomicBool,
    installed: AtomicBool,
    context: String,
}
pub(crate) struct Candidate {
    event: Value,
    owner: Value,
    scope: Value,
    window: String,
    sequence: String,
    dependencies: Vec<(i64, String)>,
    deadline: DateTime<Utc>,
}
struct Channel {
    permit: NotificationPermit,
    closed: AtomicBool,
}
impl Channel {
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.permit.revoke();
    }
    fn open(&self) -> bool {
        !self.closed.load(Ordering::Acquire)
    }
}
struct Entry {
    candidate: Candidate,
    context: String,
    version: i64,
    sampled: Reading,
    toast: Channel,
    prompt: Channel,
}
impl Entry {
    fn revoke(&self) {
        self.toast.close();
        self.prompt.close();
    }
    fn current(&self, at: &Reading) -> bool {
        let channels = [&self.toast, &self.prompt];
        channels.iter().any(|c| c.open())
            && channels
                .iter()
                .all(|c| !c.open() || c.permit.is_current_at(at))
    }
}
fn decimal(v: &Value) -> Option<u64> {
    let s = v.as_str()?;
    let n = s.parse::<u64>().ok()?;
    (n.to_string() == s && n > 0).then_some(n)
}
fn text(v: &Value) -> Option<&str> {
    v.as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= 2000)
}
fn instant(v: &Value) -> Option<DateTime<Utc>> {
    if let Some(n) = v.as_f64() {
        return (n.is_finite() && n >= 0.0 && n <= 253_402_300_799.0)
            .then(|| DateTime::from_timestamp_millis((n * 1000.0).floor() as i64))
            .flatten();
    }
    DateTime::parse_from_rfc3339(v.as_str()?)
        .ok()
        .map(|v| v.with_timezone(&Utc))
}
fn digest(v: &Value) -> String {
    let mut v = v.clone();
    v.sort_all_objects();
    hash(v.to_string().as_bytes())
}
fn version(db: &Connection) -> Result<i64> {
    db.query_row("PRAGMA data_version", [], |r| r.get(0))
        .map_err(err)
}
fn json_row(db: &Connection, sql: &str, values: &[&str]) -> Result<Option<Value>> {
    db.query_row(sql, rusqlite::params_from_iter(values), |r| {
        r.get::<_, String>(0)
    })
    .optional()
    .map_err(err)?
    .map(|raw| {
        (raw.len() <= MAX_ROW)
            .then(|| serde_json::from_str(&raw).map_err(err))
            .unwrap_or(Err("官方提醒依据超出单条预算".into()))
    })
    .transpose()
}
fn raw_window<'a>(snapshot: &'a Value, id: &str) -> Option<&'a Value> {
    for pool in snapshot["buckets"].as_array()? {
        for kind in ["primary", "secondary"] {
            if id == format!("{}:{kind}", pool["id"].as_str()?) && pool[kind].is_object() {
                return Some(&pool[kind]);
            }
        }
    }
    None
}
fn scope(snapshot: &Value, window: &str) -> Option<Value> {
    let w = raw_window(snapshot, window)?;
    if snapshot["identityConsistency"] != "proven"
        || snapshot["status"] != "connected"
        || snapshot["observationOutcome"] != "success"
        || snapshot["discontinuity"] == true
        || snapshot["historyOnly"] == true
        || snapshot["replayed"] == true
        || snapshot["live"] == false
        || !snapshot["persistenceError"].is_null()
        || !snapshot["identityEvidence"].is_object()
        || !matches!(
            w["recovery"].as_str(),
            Some("fixed_reset" | "anchored_reset")
        )
    {
        return None;
    }
    for key in ["runtimeId", "sourceId", "entitlementSegment", "clockEpoch"] {
        text(&snapshot[key])?;
    }
    decimal(&snapshot["identityEpoch"])?;
    decimal(&snapshot["observationSequence"])?;
    text(&w["ruleVersion"])?;
    text(&w["windowInstanceId"])?;
    Some(json!([
        snapshot["runtimeId"],
        snapshot["sourceId"],
        snapshot["bindingGeneration"],
        snapshot["identityEpoch"],
        snapshot["clearGeneration"],
        snapshot["accountScope"],
        snapshot["entitlementSegment"],
        snapshot["clockEpoch"],
        window,
        w["ruleVersion"],
        w["windowInstanceId"]
    ]))
}
fn event_identity(event: &Value) -> Value {
    let mut v = event.clone();
    if let Some(o) = v.as_object_mut() {
        for field in [
            "notificationState",
            "notified",
            "read",
            "animationAcknowledged",
            "claimedAt",
            "deliveryError",
            "deliveryUpdatedAt",
            "animationEligible",
        ] {
            o.remove(field);
        }
    }
    v
}
fn basic_scope(current: &Value) -> Value {
    json!({"runtimeId":current["runtimeId"],"sourceId":current["sourceId"],
        "bindingGeneration":current["bindingGeneration"],"identityEpoch":current["identityEpoch"],
        "clearGeneration":current["clearGeneration"],"accountScope":current["accountScope"]})
}
fn risk_scope(current: &Value, window: &str) -> Option<Value> {
    let w = raw_window(current, window)?;
    let mut scope = basic_scope(current);
    scope["entitlementSegment"] = current["entitlementSegment"].clone();
    scope["ruleVersion"] = w["ruleVersion"].clone();
    scope["poolId"] = json!(window);
    Some(scope)
}
fn close(a: Reading, b: Reading) -> bool {
    let Some(mono) = b.mono_ms.checked_sub(a.mono_ms) else {
        return false;
    };
    let wall = (b.utc - a.utc).num_milliseconds();
    wall >= 0
        && mono < LEASE_MS
        && wall < LEASE_MS as i64
        && (i128::from(wall) - i128::from(mono)).abs() <= 2000
}
fn reset(snapshot: &Value, id: &str) -> Option<DateTime<Utc>> {
    let w = raw_window(snapshot, id)?;
    if w["resetTimeLower"].is_null() && w["resetTimeUpper"].is_null() {
        instant(&w["resetsAt"])
    } else {
        instant(&w["resetTimeLower"])
    }
}
fn owner(
    db: &Connection,
    event: &Value,
    current: &Value,
    at: DateTime<Utc>,
) -> Result<Option<Value>> {
    if event["kind"] == "subscription_plan" && event["reminderKind"] == "official_target" {
        let Some(id) = text(&event["planId"]) else {
            return Ok(None);
        };
        let Some(p) = json_row(
            db,
            "SELECT data FROM usage_plans WHERE plan_id=?1 ORDER BY revision DESC LIMIT 1",
            &[id],
        )?
        else {
            return Ok(None);
        };
        let Some(s) = json_row(db,"SELECT data FROM plan_period_state WHERE owner_kind='plan' AND owner_id=?1 AND period_instance_id=?1", &[id])? else { return Ok(None); };
        let Some(window) = text(&p["windowId"]) else {
            return Ok(None);
        };
        let projection = window_plans::projection(db, &p, at)?;
        let used = raw_window(current, window).and_then(|w| w["usedPercent"].as_f64());
        if p["type"] != "percent"
            || p["endPolicy"] != "window"
            || p["lifecycle"] != "active"
            || p["remindersEnabled"] != true
            || p["sourceId"] != current["sourceId"]
            || p["revision"] != event["planRevision"]
            || p["target"] != event["officialTargetPercent"]
            || p["periodInstanceId"] != event["periodInstanceId"]
            || p["windowId"] != event["windowId"]
            || event["ownerKind"] != "plan"
            || event["ownerId"] != id
            || event["threshold"] != 100
            || event["id"]
                != hash(
                    format!(
                        "plan|{id}|{}|official_target|100|0",
                        event["periodInstanceId"].as_str().unwrap_or("")
                    )
                    .as_bytes(),
                )
            || !projection.same_window
            || projection.archived
            || projection.partial
            || projection.resolved_end.is_none_or(|end| end <= at)
            || s["officialBaselinePending"] != false
            || s["officialBaseline"]["scope"] != basic_scope(current)
            || projection.proof_scope != Some(basic_scope(current))
            || raw_window(current, window)
                .is_none_or(|w| s["officialBaseline"]["usedPercent"] != w["usedPercent"])
            || s["officialBaseline"]["observationSequence"] != current["observationSequence"]
            || s["latched"]
                .as_array()
                .is_none_or(|a| !a.contains(&json!(100)))
            || used
                .zip(p["target"].as_f64())
                .is_none_or(|(u, t)| !u.is_finite() || t <= 0.0 || u < t || u > 100.0)
        {
            return Ok(None);
        }
        return Ok(Some(json!([
            p,
            s["officialBaseline"],
            s["latched"],
            s["windowPlan"]
        ])));
    }
    if event["kind"] == "quota_eta"
        && event["reminderKind"] == "native_quota_eta"
        && event["ownerKind"] == "quota_watch"
    {
        let (Some(id), Some(period)) = (text(&event["ownerId"]), text(&event["periodInstanceId"]))
        else {
            return Ok(None);
        };
        let Some(s) = json_row(db,"SELECT data FROM plan_period_state WHERE owner_kind='quota_watch' AND owner_id=?1 AND period_instance_id=?2", &[id,period])? else { return Ok(None); };
        let Some(window) = text(&event["windowId"]) else {
            return Ok(None);
        };
        let Some(expected_scope) = risk_scope(current, window) else {
            return Ok(None);
        };
        if s["riskActive"] != true
            || s["schemaVersion"] != 1
            || s["windowId"] != window
            || s["ownerId"] != id
            || s["periodInstanceId"] != period
            || event["scope"] != expected_scope
            || id != hash(expected_scope.to_string().as_bytes())
            || event["id"]
                != hash(
                    format!(
                        "quota_watch|{id}|{period}|native_quota_eta|risk|{}",
                        event["riskEpisode"].as_str().unwrap_or("")
                    )
                    .as_bytes(),
                )
            || s["condition"] != "risk"
            || s["candidateEligible"] != true
            || s["riskStreak"].as_u64().is_none_or(|v| v < 2)
            || s["lastSequence"] != current["observationSequence"]
            || s["lastEventId"] != event["id"]
            || s["riskEpisode"] != event["riskEpisode"]
            || s["scope"] != event["scope"]
            || s["algorithmVersion"] != ALGORITHM_VERSION
            || s["params"] != json!(NativeParams::default())
        {
            return Ok(None);
        }
        return Ok(Some(s));
    }
    Ok(None)
}
fn receipts(
    db: &Connection,
    event: &Value,
    current: &Value,
    window: &str,
    at: DateTime<Utc>,
    sampled: Reading,
) -> Result<Option<(Vec<(i64, String)>, DateTime<Utc>)>> {
    let Some(expected_scope) = scope(current, window) else {
        return Ok(None);
    };
    let Some(confirm) = event["inputObservationSequences"]
        .as_array()
        .filter(|a| a.len() == 2)
    else {
        return Ok(None);
    };
    let Some(previous) = decimal(&confirm[0]) else {
        return Ok(None);
    };
    let Some(last) = decimal(&confirm[1]) else {
        return Ok(None);
    };
    if previous >= last || confirm[1] != current["observationSequence"] {
        return Ok(None);
    }
    let mut sequences = if event["kind"] == "quota_eta" {
        let Some(values) = event["forecastInputSequences"]
            .as_array()
            .filter(|a| a.len() >= 6 && a.len() <= MAX_INPUTS)
        else {
            return Ok(None);
        };
        values.clone()
    } else {
        confirm.clone()
    };
    if sequences.last() != Some(&current["observationSequence"]) {
        return Ok(None);
    }
    let mut last_seq = 0;
    for value in &sequences {
        let Some(seq) = decimal(value) else {
            return Ok(None);
        };
        if seq <= last_seq {
            return Ok(None);
        }
        last_seq = seq;
    }
    if !sequences.contains(&confirm[0]) {
        sequences.push(confirm[0].clone());
    }
    let mut dependencies = Vec::new();
    let mut bytes = 0usize;
    let Some(mut deadline) = reset(current, window).filter(|t| *t > at) else {
        return Ok(None);
    };
    for sequence in sequences {
        if !close(sampled, budget_clock::now()) {
            return Ok(None);
        }
        let mut q=db.prepare("SELECT id,data FROM account_snapshots WHERE json_extract(data,'$.documentKind')='subscription_observation' AND json_extract(data,'$.runtimeId')=?1 AND json_extract(data,'$.sourceId')=?2 AND json_extract(data,'$.observationSequence')=?3 ORDER BY id DESC LIMIT 2").map_err(err)?;
        let found = q
            .query_map(
                params![
                    current["runtimeId"].as_str(),
                    current["sourceId"].as_str(),
                    sequence.as_str()
                ],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
            )
            .map_err(err)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(err)?;
        if found.len() != 1 {
            return Ok(None);
        }
        let (id, raw) = &found[0];
        bytes = bytes.saturating_add(raw.len());
        if raw.len() > MAX_ROW || bytes > MAX_BYTES {
            return Ok(None);
        }
        let stored: Value = serde_json::from_str(raw).map_err(err)?;
        let receipt = observation_storage::resolve(db, &stored)?;
        let Some(snapshot) = projector::receipt_snapshot(&receipt) else {
            return Ok(None);
        };
        if scope(&snapshot, window).as_ref() != Some(&expected_scope)
            || snapshot["observationSequence"] != sequence
            || receipt["lastPersistedSequence"] != sequence
        {
            return Ok(None);
        }
        if sequence == current["observationSequence"]
            && super::calibration_assembler::native_snapshot_digest(&snapshot)
                != super::calibration_assembler::native_snapshot_digest(current)
        {
            return Ok(None);
        }
        if confirm.contains(&sequence) {
            let q = &snapshot["quotaRead"];
            let (Some(anchor), Some(lower), Some(upper)) = (
                instant(&q["freshnessAnchor"]),
                instant(&q["factTimeLower"]),
                instant(&q["factTimeUpper"]),
            ) else {
                return Ok(None);
            };
            if q["outcome"] != "success"
                || lower > upper
                || upper > at
                || anchor > at
                || at - anchor >= Duration::seconds(240)
                || at - lower >= Duration::seconds(240)
            {
                return Ok(None);
            }
            deadline = deadline
                .min(anchor + Duration::seconds(240))
                .min(lower + Duration::seconds(240));
        }
        dependencies.push((*id, digest(&receipt)));
    }
    Ok(Some((dependencies, deadline)))
}
impl Delivery {
    fn observe(&mut self, at: Reading) -> bool {
        let good = self.high.is_none_or(|old| {
            at.utc >= old.utc
                && at.mono_ms >= old.mono_ms
                && (i128::from((at.utc - old.utc).num_milliseconds())
                    - i128::from(at.mono_ms - old.mono_ms))
                .abs()
                    <= 2000
        });
        self.high = Some(self.high.map_or(at, |old| Reading {
            utc: old.utc.max(at.utc),
            mono_ms: old.mono_ms.max(at.mono_ms),
        }));
        if !good {
            self.revoke_all();
        }
        good
    }
    fn prune(&mut self, at: &Reading) {
        self.entries.retain(|_, entry| {
            if entry.current(at) && close(entry.sampled, *at) {
                true
            } else {
                entry.revoke();
                false
            }
        });
    }
    pub(crate) fn begin(
        &mut self,
        db: &Connection,
        gate: Arc<Gate>,
        context: String,
    ) -> Result<Option<Ticket>> {
        let at = budget_clock::now();
        self.prune(&at);
        if !db.is_autocommit() || !self.observe(at) || context.is_empty() {
            return Ok(None);
        }
        let Some(epoch) = gate.snapshot() else {
            self.revoke_all();
            return Ok(None);
        };
        let inbox_rowid = db
            .query_row("SELECT COALESCE(MAX(rowid),0) FROM inbox_events", [], |r| {
                r.get(0)
            })
            .map_err(err)?;
        Ok(Some(Ticket {
            instance: Arc::clone(&self.instance),
            gate,
            epoch,
            at,
            version: version(db)?,
            inbox_rowid,
            prepared: AtomicBool::new(false),
            installed: AtomicBool::new(false),
            context,
        }))
    }
    /// Executed in the observation transaction. Only genuinely new panel rows
    /// can be promoted; creation of RAM leases waits for a successful COMMIT.
    pub(crate) fn prepare(
        db: &Connection,
        ticket: Option<&Ticket>,
        current: &Value,
        derived: &Value,
        qualified: bool,
    ) -> Result<Vec<Candidate>> {
        let Some(ticket) = ticket else {
            return Ok(Vec::new());
        };
        if !qualified
            || db.is_autocommit()
            || ticket.prepared.swap(true, Ordering::AcqRel)
            || ticket.gate.snapshot() != Some(ticket.epoch)
            || !close(ticket.at, budget_clock::now())
            || version(db)? != ticket.version
        {
            return Ok(Vec::new());
        }
        let mut q = db
            .prepare("SELECT data FROM inbox_events WHERE rowid>?1 ORDER BY rowid LIMIT 401")
            .map_err(err)?;
        let rows = q
            .query_map([ticket.inbox_rowid], |r| r.get::<_, String>(0))
            .map_err(err)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(err)?;
        drop(q);
        if rows.len() > MAX_EVENTS {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for raw in rows {
            if !close(ticket.at, budget_clock::now())
                || ticket.gate.snapshot() != Some(ticket.epoch)
            {
                break;
            }
            if raw.len() > MAX_ROW {
                continue;
            }
            let mut event: Value = serde_json::from_str(&raw).map_err(err)?;
            if event["notificationState"] != "panel_only"
                || event["notified"] != false
                || event["eligibleRuntime"] != current["runtimeId"]
                || event["sourceId"] != current["sourceId"]
            {
                continue;
            }
            let Some(window) = text(&event["windowId"]).map(str::to_owned) else {
                continue;
            };
            let Some(scope) = scope(current, &window) else {
                continue;
            };
            if raw_window(current, &window)
                .is_none_or(|w| w["windowInstanceId"] != event["periodInstanceId"])
            {
                continue;
            }
            let Some(owner) = owner(db, &event, current, ticket.at.utc)? else {
                continue;
            };
            if event["kind"] == "quota_eta" {
                let Ok(forecast) = serde_json::from_value::<NativeForecast>(
                    derived["risks"][&window]["evidence"].clone(),
                ) else {
                    continue;
                };
                let mut evidence = json!(forecast);
                evidence
                    .as_object_mut()
                    .unwrap()
                    .remove("inputObservationIds");
                let prefix = format!("{}:", current["runtimeId"].as_str().unwrap_or(""));
                let suffix = format!(":{window}");
                let sequences: Option<Vec<_>> = forecast
                    .input_observation_ids
                    .iter()
                    .map(|id| {
                        id.strip_prefix(&prefix)
                            .and_then(|id| id.strip_suffix(&suffix))
                            .and_then(|value| decimal(&json!(value)).map(|_| json!(value)))
                    })
                    .collect();
                if !forecast.notify_candidate
                    || sequences.as_ref().map(|v| json!(v)).as_ref()
                        != Some(&event["forecastInputSequences"])
                    || forecast
                        .exhaustion_time
                        .zip(forecast.reset_time)
                        .is_none_or(|(end, reset)| {
                            !end.is_valid()
                                || !reset.is_valid()
                                || end.upper >= reset.lower
                                || end.upper <= ticket.at.utc.timestamp_millis() as f64 / 1000.0
                        })
                    || forecast.will_exhaust_before_reset != Some(true)
                    || !forecast.reasons.is_empty()
                    || forecast.algorithm_version != ALGORITHM_VERSION
                    || event["algorithmVersion"] != ALGORITHM_VERSION
                    || event["params"] != json!(NativeParams::default())
                    || event["evidence"] != evidence
                    || event["forecastInputDigest"]
                        != hash(serde_json::to_string(&forecast).map_err(err)?.as_bytes())
                {
                    continue;
                }
            }
            let Some((dependencies, mut deadline)) =
                receipts(db, &event, current, &window, ticket.at.utc, ticket.at)?
            else {
                continue;
            };
            if event["kind"] == "subscription_plan" {
                if let Some(plan_end) = instant(&owner[3]["resolvedEnd"]) {
                    deadline = deadline.min(plan_end);
                }
            }
            deadline = deadline.min(ticket.at.utc + Duration::milliseconds(LEASE_MS as i64));
            if deadline <= budget_clock::now().utc {
                continue;
            }
            event["notifyEligible"] = json!(true);
            event["animationAcknowledged"] = json!(false);
            event["notificationState"] = json!("pending");
            event["eligibleContext"] = json!(ticket.context);
            db.execute(
                "UPDATE inbox_events SET data=?1 WHERE id=?2",
                params![event.to_string(), event["id"].as_str()],
            )
            .map_err(err)?;
            out.push(Candidate {
                event: event_identity(&event),
                owner,
                scope,
                window,
                sequence: current["observationSequence"].as_str().unwrap().into(),
                dependencies,
                deadline,
            });
        }
        Ok(out)
    }
    pub(crate) fn install(
        &mut self,
        db: &Connection,
        ticket: Option<Ticket>,
        candidates: Vec<Candidate>,
        current: &Value,
        context: &str,
        qualified: bool,
    ) -> Result<()> {
        let Some(ticket) = ticket else {
            return Ok(());
        };
        let at = budget_clock::now();
        self.prune(&at);
        if candidates.len() > MAX_EVENTS
            || !qualified
            || !db.is_autocommit()
            || !Arc::ptr_eq(&self.instance, &ticket.instance)
            || ticket.installed.swap(true, Ordering::AcqRel)
            || !ticket.prepared.load(Ordering::Acquire)
            || ticket.context != context
            || ticket.gate.snapshot() != Some(ticket.epoch)
            || !self.observe(at)
            || !close(ticket.at, at)
            || version(db)? != ticket.version
        {
            return Ok(());
        }
        for candidate in candidates {
            if self.entries.len() >= MAX_EVENTS {
                break;
            }
            if !valid_candidate(db, &candidate, current, at.utc)? {
                continue;
            }
            if version(db)? != ticket.version {
                continue;
            }
            let delta = (candidate.deadline - ticket.at.utc).num_milliseconds();
            let Some(mono) = u64::try_from(delta)
                .ok()
                .and_then(|n| ticket.at.mono_ms.checked_add(n))
            else {
                continue;
            };
            let expires = candidate.deadline;
            let new_channel = || Channel {
                permit: NotificationPermit::new(
                    Arc::clone(&ticket.gate),
                    ticket.epoch,
                    expires,
                    mono,
                ),
                closed: AtomicBool::new(false),
            };
            let entry = Entry {
                candidate,
                context: ticket.context.clone(),
                version: ticket.version,
                sampled: ticket.at,
                toast: new_channel(),
                prompt: new_channel(),
            };
            if entry.current(&budget_clock::now()) {
                self.entries
                    .insert(entry.candidate.event["id"].as_str().unwrap().into(), entry);
            }
        }
        Ok(())
    }
    pub(crate) fn permit(
        &self,
        db: &Connection,
        event: &Value,
        current: Option<&Value>,
        context: &str,
        qualified: bool,
        prompt: bool,
    ) -> Option<NotificationPermit> {
        let entry = self.entries.get(event["id"].as_str()?)?;
        if event_identity(event) != entry.candidate.event {
            return None;
        }
        let at = budget_clock::now();
        let good = (|| -> Result<bool> {
            Ok(qualified
                && db.is_autocommit()
                && context == entry.context
                && close(entry.sampled, at)
                && self
                    .high
                    .is_none_or(|old| at.utc >= old.utc && at.mono_ms >= old.mono_ms)
                && entry.current(&at)
                && version(db)? == entry.version
                && current.is_some_and(|s| s["persisted"] == true)
                && valid_candidate(
                    db,
                    &entry.candidate,
                    current.unwrap_or(&Value::Null),
                    at.utc,
                )?)
        })()
        .unwrap_or(false);
        if !good {
            entry.revoke();
            return None;
        }
        let actual = match json_row(
            db,
            "SELECT data FROM inbox_events WHERE id=?1",
            &[event["id"].as_str()?],
        ) {
            Ok(Some(actual)) => actual,
            _ => {
                entry.revoke();
                return None;
            }
        };
        if event_identity(&actual) != entry.candidate.event
            || !matches!(
                actual["notificationState"].as_str(),
                Some("pending" | "claimed" | "sent" | "shown" | "failed")
            )
        {
            entry.revoke();
            return None;
        }
        // This is the final DB read. The subsequent paired-clock/host check
        // also covers time spent resolving the latest channel state.
        let final_version = version(db).ok();
        let finished = budget_clock::now();
        if final_version != Some(entry.version)
            || !close(entry.sampled, finished)
            || !entry.current(&finished)
        {
            entry.revoke();
            return None;
        }
        let channel = if prompt { &entry.prompt } else { &entry.toast };
        let permitted = if prompt {
            actual["animationAcknowledged"] != true
        } else {
            matches!(
                actual["notificationState"].as_str(),
                Some("pending" | "claimed")
            )
        };
        if !permitted {
            channel.close();
            return None;
        }
        channel.open().then(|| channel.permit.clone())
    }
    pub(crate) fn finish(&mut self, id: &str, prompt: Option<bool>) {
        if let Some(entry) = self.entries.get(id) {
            match prompt {
                Some(true) => entry.prompt.close(),
                Some(false) => entry.toast.close(),
                None => entry.revoke(),
            }
        }
        self.prune(&budget_clock::now());
    }
    pub(crate) fn revoke_owner(&mut self, id: &str) {
        self.entries.retain(|_, entry| {
            if entry.candidate.event["ownerKind"] == "plan"
                && entry.candidate.event["ownerId"] == id
            {
                entry.revoke();
                false
            } else {
                true
            }
        });
    }
    pub(crate) fn revoke_all(&mut self) {
        self.instance = Arc::new(());
        for entry in self.entries.values() {
            entry.revoke();
        }
        self.entries.clear();
    }
}
fn valid_candidate(
    db: &Connection,
    c: &Candidate,
    current: &Value,
    at: DateTime<Utc>,
) -> Result<bool> {
    if scope(current, &c.window).as_ref() != Some(&c.scope)
        || current["observationSequence"] != c.sequence
        || at >= c.deadline
    {
        return Ok(false);
    }
    let Some(event) = json_row(
        db,
        "SELECT data FROM inbox_events WHERE id=?1",
        &[c.event["id"].as_str().unwrap_or("")],
    )?
    else {
        return Ok(false);
    };
    if event_identity(&event) != c.event
        || !matches!(
            event["notificationState"].as_str(),
            Some("pending" | "claimed" | "sent" | "shown" | "failed")
        )
        || owner(db, &event, current, at)?.as_ref() != Some(&c.owner)
    {
        return Ok(false);
    }
    for (id, expected) in &c.dependencies {
        if budget_clock::now().utc >= c.deadline {
            return Ok(false);
        }
        let Some(raw) = db
            .query_row(
                "SELECT data FROM account_snapshots WHERE id=?1 AND length(data)<=?2",
                params![id, MAX_ROW as i64],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(err)?
        else {
            return Ok(false);
        };
        let stored: Value = serde_json::from_str(&raw).map_err(err)?;
        let receipt = observation_storage::resolve(db, &stored)?;
        if digest(&receipt) != *expected || projector::receipt_snapshot(&receipt).is_none() {
            return Ok(false);
        }
    }
    Ok(true)
}
impl Drop for Delivery {
    fn drop(&mut self) {
        self.revoke_all();
    }
}
