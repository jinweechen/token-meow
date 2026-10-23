//! In-memory admission for a newly committed local Token crossing.
//! A durable inbox row is history, never authority to recreate a native permit.
use super::super::{adapter::ResponseUsage, err, hash, Result};
use crate::budget_clock::{self, Reading};
use crate::notification_gate::{Gate, NotificationPermit};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

const LIFETIME_MS: u64 = 15_000;
const MAX_SAFE: i64 = 9_007_199_254_740_991;
const SKEW_MS: i128 = 2_000;

#[derive(Default)]
pub(crate) struct Delivery {
    instance: Arc<()>,
    permits: BTreeMap<String, Entry>,
    issued: BTreeSet<String>,
    clock_high: Option<Reading>,
}
struct Entry {
    crossing: TokenCrossing,
    plan: Value,
    event_identity: Value,
    facts: String,
    runtime: String,
    context: String,
    sampled: Reading,
    data_version: i64,
    toast: ChannelPermit,
    prompt: ChannelPermit,
}
struct ChannelPermit {
    permit: NotificationPermit,
    closed: AtomicBool,
}
impl ChannelPermit {
    fn new(permit: NotificationPermit) -> Self {
        Self {
            permit,
            closed: AtomicBool::new(false),
        }
    }
    fn is_open(&self) -> bool {
        !self.closed.load(Ordering::Acquire)
    }
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.permit.revoke();
    }
}
impl Entry {
    fn revoke(&self) {
        self.toast.close();
        self.prompt.close();
    }
    fn channels_current(&self, at: &Reading) -> bool {
        let mut open = false;
        for channel in [&self.toast, &self.prompt] {
            if channel.is_open() {
                open = true;
                if !channel.permit.is_current_at(at) {
                    return false;
                }
            }
        }
        open
    }
}

/// Captured before the collector writes. Private fields prevent a persisted row
/// or caller-supplied kind from manufacturing a fresh collection sample.
pub(crate) struct Sample {
    instance: Arc<()>,
    gate: Arc<Gate>,
    epoch: u64,
    at: Reading,
    runtime: String,
    context: String,
    data_version: i64,
    usage_rowid: i64,
    inbox_rowid: i64,
    before: BTreeMap<String, (Value, Value)>,
}

/// Core supplies IDs from its canonical, alias/conflict-filtered read model.
/// Every supporting row is re-read here; this is never a frontend payload.
#[derive(Clone)]
pub(crate) struct TokenCrossing {
    pub(crate) event_id: String,
    pub(crate) plan_id: String,
    pub(crate) source_id: String,
    /// Local plans have one fixed lifetime, currently identified by plan ID.
    pub(crate) period_instance_id: String,
    pub(crate) plan_revision: i64,
    pub(crate) threshold: i64,
    pub(crate) before_used: i64,
    pub(crate) after_used: i64,
    pub(crate) fact_ids: Vec<String>,
    pub(crate) fresh_fact_ids: Vec<String>,
}

fn version(db: &Connection) -> Result<i64> {
    db.query_row("PRAGMA data_version", [], |r| r.get(0))
        .map_err(err)
}
fn instant(v: &Value) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(v.as_str()?)
        .ok()
        .map(|v| v.with_timezone(&Utc))
}
fn integer(v: &Value) -> Option<i64> {
    let n = v.as_f64()?;
    (n.is_finite() && n >= 0.0 && n <= MAX_SAFE as f64 && n.fract() == 0.0).then_some(n as i64)
}
fn close(a: Reading, b: Reading) -> bool {
    let Some(mono) = b.mono_ms.checked_sub(a.mono_ms) else {
        return false;
    };
    let wall = (b.utc - a.utc).num_milliseconds();
    wall >= 0
        && wall <= LIFETIME_MS as i64
        && mono <= LIFETIME_MS
        && (i128::from(wall) - i128::from(mono)).abs() <= SKEW_MS
}
fn json_row(db: &Connection, sql: &str, id: &str) -> Result<Option<Value>> {
    db.query_row(sql, [id], |r| r.get::<_, String>(0))
        .optional()
        .map_err(err)?
        .map(|text| serde_json::from_str(&text).map_err(err))
        .transpose()
}
fn latest(db: &Connection, id: &str) -> Result<Option<Value>> {
    json_row(
        db,
        "SELECT data FROM usage_plans WHERE plan_id=?1 ORDER BY revision DESC LIMIT 1",
        id,
    )
}
fn state(db: &Connection, id: &str) -> Result<Option<Value>> {
    json_row(db, "SELECT data FROM plan_period_state WHERE owner_kind='plan' AND owner_id=?1 AND period_instance_id=?1", id)
}
fn event(db: &Connection, id: &str) -> Result<Option<(i64, Value)>> {
    let row = db
        .query_row(
            "SELECT rowid,data FROM inbox_events WHERE id=?1",
            [id],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(err)?;
    row.map(|(rowid, text)| serde_json::from_str(&text).map(|v| (rowid, v)).map_err(err))
        .transpose()
}
fn event_identity(v: &Value) -> Value {
    serde_json::json!([
        v["id"],
        v["kind"],
        v["sourceId"],
        v["planId"],
        v["periodInstanceId"],
        v["planRevision"],
        v["threshold"],
        v["eligibleRuntime"],
        v["eligibleContext"],
        v["eventTime"],
        v["notifyEligible"]
    ])
}
fn event_valid(v: &Value, c: &TokenCrossing, runtime: &str, context: &str) -> bool {
    v["id"] == c.event_id
        && v["kind"] == "subscription_plan"
        && v["sourceId"] == c.source_id
        && v["planId"] == c.plan_id
        && v["periodInstanceId"] == c.period_instance_id
        && v["planRevision"].as_i64() == Some(c.plan_revision)
        && v["threshold"].as_i64() == Some(c.threshold)
        && v["eligibleRuntime"] == runtime
        && v["eligibleContext"] == context
        && v["notifyEligible"] == true
}
fn configuration_valid(
    db: &Connection,
    p: &Value,
    c: &TokenCrossing,
    at: DateTime<Utc>,
) -> Result<bool> {
    if p["id"] != c.plan_id
        || p["sourceId"] != c.source_id
        || p["revision"].as_i64() != Some(c.plan_revision)
        || p["type"] != "tokens"
        || p["lifecycle"] != "active"
        || p["remindersEnabled"] != true
        || c.plan_id.is_empty()
        || c.period_instance_id != c.plan_id
        || ![80, 100].contains(&c.threshold)
        || c.plan_revision <= 0
        || c.plan_revision > MAX_SAFE
        || integer(&p["target"]).is_none_or(|t| t == 0)
        || instant(&p["effectiveFrom"]).is_none_or(|t| t > at)
        || !p["endedAt"].is_null()
    {
        return Ok(false);
    }
    let enabled: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM source_instances WHERE id=?1 AND enabled=1)",
            [&c.source_id],
            |r| r.get(0),
        )
        .map_err(err)?;
    if !enabled {
        return Ok(false);
    }
    match p["endPolicy"].as_str() {
        Some("time") => Ok(instant(&p["endsAt"]).is_some_and(|end| at < end)),
        Some("window") => {
            let projection = super::window_plans::projection(db, p, at)?;
            Ok(projection.same_window
                && !projection.archived
                && !projection.partial
                && projection.resolved_end.is_some_and(|end| at < end))
        }
        _ => Ok(false),
    }
}
fn reached(used: i64, p: &Value, threshold: i64) -> bool {
    integer(&p["target"])
        .is_some_and(|target| i128::from(used) * 100 >= i128::from(target) * i128::from(threshold))
}

struct Facts {
    fingerprint: String,
    total: i64,
    fresh: i64,
    oldest_fresh: Option<DateTime<Utc>>,
}
fn facts(
    db: &Connection,
    c: &TokenCrossing,
    p: &Value,
    sample: Option<&Sample>,
    at: DateTime<Utc>,
) -> Result<Option<Facts>> {
    let ids: BTreeSet<_> = c.fact_ids.iter().collect();
    let fresh: BTreeSet<_> = c.fresh_fact_ids.iter().collect();
    if ids.is_empty()
        || ids.len() != c.fact_ids.len()
        || fresh.is_empty()
        || fresh.len() != c.fresh_fact_ids.len()
        || !fresh.is_subset(&ids)
    {
        return Ok(None);
    }
    let Some(start) = instant(&p["effectiveFrom"]) else {
        return Ok(None);
    };
    let end = if p["endPolicy"] == "window" {
        super::window_plans::projection(db, p, at)?.resolved_end
    } else {
        instant(&p["endsAt"])
    };
    let mut total = 0i64;
    let mut new_total = 0i64;
    let mut oldest = None::<DateTime<Utc>>;
    let mut fingerprints = Vec::with_capacity(ids.len());
    for id in ids {
        let row = db.query_row("SELECT rowid,data FROM usage_responses WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM excluded_facts WHERE fact_id=?1) AND NOT EXISTS(SELECT 1 FROM fact_aliases WHERE alias=?1)", [id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))).optional().map_err(err)?;
        let Some((rowid, text)) = row else {
            return Ok(None);
        };
        let r: ResponseUsage = serde_json::from_str(&text).map_err(err)?;
        let Some(event_time) = DateTime::parse_from_rfc3339(&r.event_time)
            .ok()
            .map(|v| v.with_timezone(&Utc))
        else {
            return Ok(None);
        };
        if r.source_id != c.source_id
            || super::super::adapter::response_key(&r) != *id
            || event_time < start
            || event_time > at
            || end.is_none_or(|end| event_time >= end)
        {
            return Ok(None);
        }
        if let Some(model) = p["model"].as_str() {
            // Request-model fallback uses the same persisted turn evidence as the Core read model.
            let observed = if p["modelDimension"] == "response" {
                r.observed_model.clone()
            } else if p["modelDimension"] == "request" {
                r.requested_model.clone().or_else(|| {
                    r.turn_id.as_deref().and_then(|turn| db.query_row("SELECT json_extract(data,'$.model') FROM turns WHERE json_extract(data,'$.sourceId')=?1 AND json_extract(data,'$.id')=?2", params![r.source_id,turn], |row|row.get::<_,Option<String>>(0)).optional().ok().flatten().flatten())
                })
            } else {
                return Ok(None);
            };
            if observed.as_deref() != Some(model) {
                return Ok(None);
            }
        }
        let Some(count) = r.counts.total.value().filter(|n| *n >= 0 && *n <= MAX_SAFE) else {
            return Ok(None);
        };
        let Some(sum) = total.checked_add(count).filter(|n| *n <= MAX_SAFE) else {
            return Ok(None);
        };
        total = sum;
        if fresh.contains(id) {
            if !r.live || sample.is_some_and(|s| rowid <= s.usage_rowid) {
                return Ok(None);
            }
            let Some(sum) = new_total.checked_add(count).filter(|n| *n <= MAX_SAFE) else {
                return Ok(None);
            };
            new_total = sum;
            oldest = Some(oldest.map_or(event_time, |v| v.min(event_time)));
        } else if sample.is_some_and(|s| rowid > s.usage_rowid) {
            // Newly inserted history cannot be smuggled into the old baseline.
            return Ok(None);
        }
        fingerprints.push((id, hash(text.as_bytes())));
    }
    Ok(Some(Facts {
        fingerprint: hash(
            serde_json::to_string(&fingerprints)
                .map_err(err)?
                .as_bytes(),
        ),
        total,
        fresh: new_total,
        oldest_fresh: oldest,
    }))
}

impl Delivery {
    fn prune(&mut self, at: Reading) {
        self.permits.retain(|_, entry| {
            if close(entry.sampled, at) && entry.channels_current(&at) {
                true
            } else {
                entry.revoke();
                false
            }
        });
    }
    fn observe_clock(&mut self, at: Reading) -> bool {
        let valid = self.clock_high.is_none_or(|old| {
            at.utc >= old.utc
                && at.mono_ms >= old.mono_ms
                && (i128::from((at.utc - old.utc).num_milliseconds())
                    - i128::from(at.mono_ms - old.mono_ms))
                .abs()
                    <= SKEW_MS
        });
        self.clock_high = Some(self.clock_high.map_or(at, |old| Reading {
            utc: old.utc.max(at.utc),
            mono_ms: old.mono_ms.max(at.mono_ms),
        }));
        if !valid {
            self.revoke_all();
        }
        valid
    }
    pub(crate) fn begin(
        &mut self,
        db: &Connection,
        gate: Arc<Gate>,
        runtime: &str,
        context: &str,
    ) -> Result<Option<Sample>> {
        let at = budget_clock::now();
        self.prune(at);
        if !self.observe_clock(at)
            || !db.is_autocommit()
            || runtime.is_empty()
            || context.is_empty()
        {
            return Ok(None);
        }
        let Some(epoch) = gate.snapshot() else {
            self.revoke_all();
            return Ok(None);
        };
        let data_version = version(db)?;
        let mut before = BTreeMap::new();
        let mut stmt = db.prepare("SELECT p.data FROM usage_plans p JOIN (SELECT plan_id,MAX(revision) revision FROM usage_plans GROUP BY plan_id) c ON c.plan_id=p.plan_id AND c.revision=p.revision").map_err(err)?;
        for row in stmt.query_map([], |r| r.get::<_, String>(0)).map_err(err)? {
            let p: Value = serde_json::from_str(&row.map_err(err)?).map_err(err)?;
            if let Some(id) = p["id"].as_str() {
                if let Some(s) = state(db, id)? {
                    before.insert(id.to_owned(), (p, s));
                }
            }
        }
        let usage_rowid = db
            .query_row(
                "SELECT COALESCE(MAX(rowid),0) FROM usage_responses",
                [],
                |r| r.get(0),
            )
            .map_err(err)?;
        let inbox_rowid = db
            .query_row("SELECT COALESCE(MAX(rowid),0) FROM inbox_events", [], |r| {
                r.get(0)
            })
            .map_err(err)?;
        if version(db)? != data_version || gate.snapshot() != Some(epoch) {
            self.revoke_all();
            return Ok(None);
        }
        Ok(Some(Sample {
            instance: Arc::clone(&self.instance),
            gate,
            epoch,
            at,
            runtime: runtime.into(),
            context: context.into(),
            data_version,
            usage_rowid,
            inbox_rowid,
            before,
        }))
    }
    /// Call only after the new event/state transaction committed. This method
    /// refuses an open transaction and never extends the sampled/fact lifetime.
    pub(crate) fn grant(
        &mut self,
        db: &Connection,
        sample: &Sample,
        c: TokenCrossing,
        qualified: bool,
    ) -> Result<bool> {
        let at = budget_clock::now();
        self.prune(at);
        if !qualified {
            self.revoke_all();
            return Ok(false);
        }
        if !self.observe_clock(at)
            || !db.is_autocommit()
            || !Arc::ptr_eq(&self.instance, &sample.instance)
            || !close(sample.at, at)
            || sample.gate.snapshot() != Some(sample.epoch)
            || version(db)? != sample.data_version
            || self.issued.contains(&c.event_id)
        {
            return Ok(false);
        }
        let Some((old_plan, old_state)) = sample.before.get(&c.plan_id) else {
            return Ok(false);
        };
        let Some(plan) = latest(db, &c.plan_id)? else {
            return Ok(false);
        };
        if plan != *old_plan
            || !configuration_valid(db, &plan, &c, at.utc)?
            || c.before_used < 0
            || c.after_used > MAX_SAFE
            || c.after_used <= c.before_used
            || reached(c.before_used, &plan, c.threshold)
            || !reached(c.after_used, &plan, c.threshold)
            || old_state["runtimeId"] != sample.runtime
            || integer(&old_state["used"]) != Some(c.before_used)
            || old_state["evaluationState"] != "ready"
            || old_state["latched"]
                .as_array()
                .is_none_or(|a| a.contains(&Value::from(c.threshold)))
        {
            return Ok(false);
        }
        let Some((rowid, event)) = event(db, &c.event_id)? else {
            return Ok(false);
        };
        if rowid <= sample.inbox_rowid
            || !event_valid(&event, &c, &sample.runtime, &sample.context)
            || event["notificationState"] != "pending"
            || event["notified"] == true
            || instant(&event["eventTime"]).is_none_or(|t| t < sample.at.utc || t > at.utc)
        {
            return Ok(false);
        }
        let Some(current) = state(db, &c.plan_id)? else {
            return Ok(false);
        };
        if current["runtimeId"] != sample.runtime
            || current["evaluationState"] != "ready"
            || integer(&current["used"]) != Some(c.after_used)
            || current["latched"]
                .as_array()
                .is_none_or(|a| !a.contains(&Value::from(c.threshold)))
        {
            return Ok(false);
        }
        let Some(facts) = facts(db, &c, &plan, Some(sample), at.utc)? else {
            return Ok(false);
        };
        if facts.total != c.after_used
            || facts.fresh != c.after_used - c.before_used
            || facts.fresh <= 0
        {
            return Ok(false);
        }
        let Some(mut deadline) = sample.at.checked_deadline(LIFETIME_MS) else {
            return Ok(false);
        };
        let Some(fact_expiry) = facts
            .oldest_fresh
            .and_then(|v| v.checked_add_signed(Duration::milliseconds(LIFETIME_MS as i64)))
        else {
            return Ok(false);
        };
        let delta = (fact_expiry - sample.at.utc).num_milliseconds();
        let Some(fact_mono) = u64::try_from(delta)
            .ok()
            .and_then(|v| sample.at.mono_ms.checked_add(v))
        else {
            return Ok(false);
        };
        deadline.utc = deadline.utc.min(fact_expiry);
        deadline.mono_ms = deadline.mono_ms.min(fact_mono);
        // A queued clone cannot outlive this fixed plan period, even if Core
        // has not read the database again when the host submits the queue.
        let plan_end = if plan["endPolicy"] == "window" {
            super::window_plans::projection(db, &plan, at.utc)?.resolved_end
        } else {
            instant(&plan["endsAt"])
        };
        let Some(plan_end) = plan_end else {
            return Ok(false);
        };
        let Some(plan_mono) = u64::try_from((plan_end - sample.at.utc).num_milliseconds())
            .ok()
            .and_then(|v| sample.at.mono_ms.checked_add(v))
        else {
            return Ok(false);
        };
        deadline.utc = deadline.utc.min(plan_end);
        deadline.mono_ms = deadline.mono_ms.min(plan_mono);
        // Recheck after the last DB read. No asynchronous work can renew this lease.
        let final_at = budget_clock::now();
        if !self.observe_clock(final_at)
            || !close(sample.at, final_at)
            || final_at.utc >= deadline.utc
            || final_at.mono_ms >= deadline.mono_ms
            || version(db)? != sample.data_version
            || sample.gate.snapshot() != Some(sample.epoch)
        {
            return Ok(false);
        }
        let toast = NotificationPermit::new(
            Arc::clone(&sample.gate),
            sample.epoch,
            deadline.utc,
            deadline.mono_ms,
        );
        let prompt = NotificationPermit::new(
            Arc::clone(&sample.gate),
            sample.epoch,
            deadline.utc,
            deadline.mono_ms,
        );
        if !self.observe_clock(budget_clock::now()) || !toast.is_current() || !prompt.is_current() {
            toast.revoke();
            prompt.revoke();
            return Ok(false);
        }
        self.issued.insert(c.event_id.clone());
        self.permits.insert(
            c.event_id.clone(),
            Entry {
                crossing: c,
                plan,
                event_identity: event_identity(&event),
                facts: facts.fingerprint,
                runtime: sample.runtime.clone(),
                context: sample.context.clone(),
                sampled: sample.at,
                data_version: sample.data_version,
                toast: ChannelPermit::new(toast),
                prompt: ChannelPermit::new(prompt),
            },
        );
        Ok(true)
    }
    pub(crate) fn permit(
        &self,
        db: &Connection,
        id: &str,
        runtime: &str,
        context: &str,
        qualified: bool,
    ) -> Option<NotificationPermit> {
        self.channel_permit(db, id, runtime, context, qualified, false)
    }
    pub(crate) fn prompt_permit(
        &self,
        db: &Connection,
        id: &str,
        runtime: &str,
        context: &str,
        qualified: bool,
    ) -> Option<NotificationPermit> {
        self.channel_permit(db, id, runtime, context, qualified, true)
    }
    fn channel_permit(
        &self,
        db: &Connection,
        id: &str,
        runtime: &str,
        context: &str,
        qualified: bool,
        is_prompt: bool,
    ) -> Option<NotificationPermit> {
        let entry = self.permits.get(id)?;
        let at = budget_clock::now();
        let mut channel_valid = false;
        let valid = (|| -> Result<bool> {
            if !qualified
                || !db.is_autocommit()
                || runtime != entry.runtime
                || context != entry.context
                || !close(entry.sampled, at)
                || self
                    .clock_high
                    .is_some_and(|high| at.utc < high.utc || at.mono_ms < high.mono_ms)
                || !entry.channels_current(&at)
                || version(db)? != entry.data_version
            {
                return Ok(false);
            }
            let Some((_, v)) = event(db, id)? else {
                return Ok(false);
            };
            if !event_valid(&v, &entry.crossing, runtime, context)
                || event_identity(&v) != entry.event_identity
                || !["pending", "claimed", "sent", "shown", "failed"]
                    .contains(&v["notificationState"].as_str().unwrap_or(""))
            {
                return Ok(false);
            }
            channel_valid = if is_prompt {
                v["animationAcknowledged"] != true
            } else {
                ["pending", "claimed"].contains(&v["notificationState"].as_str().unwrap_or(""))
            };
            let Some(p) = latest(db, &entry.crossing.plan_id)? else {
                return Ok(false);
            };
            if p != entry.plan || !configuration_valid(db, &p, &entry.crossing, at.utc)? {
                return Ok(false);
            }
            let Some(s) = state(db, &entry.crossing.plan_id)? else {
                return Ok(false);
            };
            if s["runtimeId"] != runtime
                || s["evaluationState"] != "ready"
                || integer(&s["used"])
                    .is_none_or(|used| !reached(used, &p, entry.crossing.threshold))
                || s["latched"]
                    .as_array()
                    .is_none_or(|a| !a.contains(&Value::from(entry.crossing.threshold)))
            {
                return Ok(false);
            }
            let Some(f) = facts(db, &entry.crossing, &p, None, at.utc)? else {
                return Ok(false);
            };
            Ok(f.fingerprint == entry.facts
                && reached(f.total, &p, entry.crossing.threshold)
                && version(db)? == entry.data_version)
        })()
        .unwrap_or(false);
        if !valid {
            entry.revoke();
            return None;
        }
        let channel = if is_prompt {
            &entry.prompt
        } else {
            &entry.toast
        };
        if !channel_valid {
            channel.close();
            return None;
        }
        if !channel.is_open() {
            return None;
        }
        if !channel.permit.is_current() {
            entry.revoke();
            return None;
        }
        Some(channel.permit.clone())
    }
    /// Completion of one output never renews or cancels the other output.
    pub(crate) fn finish_toast(&mut self, id: &str) {
        self.finish_channel(id, false);
    }
    pub(crate) fn finish_prompt(&mut self, id: &str) {
        self.finish_channel(id, true);
    }
    fn finish_channel(&mut self, id: &str, is_prompt: bool) {
        let at = budget_clock::now();
        if !self.observe_clock(at) {
            return;
        }
        self.prune(at);
        if let Some(entry) = self.permits.get(id) {
            if is_prompt {
                entry.prompt.close();
            } else {
                entry.toast.close();
            }
            if !entry.toast.is_open() && !entry.prompt.is_open() {
                self.permits.remove(id);
            }
        }
    }
    pub(crate) fn finish(&mut self, id: &str) {
        if let Some(e) = self.permits.remove(id) {
            e.revoke();
        }
    }
    pub(crate) fn revoke_owner(&mut self, plan_id: &str) {
        // Cancel in-flight scans too. Core will take a fresh baseline after a
        // configuration change; another plan's existing permit stays valid.
        self.instance = Arc::new(());
        self.permits.retain(|_, e| {
            if e.crossing.plan_id == plan_id {
                e.revoke();
                false
            } else {
                true
            }
        });
    }
    pub(crate) fn revoke_quota_bound(&mut self) {
        // Account RPC availability does not own independent source Token
        // plans. Preserve their queued permits and in-flight scan samples.
        // Core separately requires a current window proof before granting a
        // new window-bound crossing.
        self.permits.retain(|_, entry| {
            if entry.plan["endPolicy"] == "window" {
                entry.revoke();
                false
            } else {
                true
            }
        });
    }
    pub(crate) fn revoke_all(&mut self) {
        self.instance = Arc::new(());
        for (_, e) in std::mem::take(&mut self.permits) {
            e.revoke();
        }
    }
}
impl Drop for Delivery {
    fn drop(&mut self) {
        self.revoke_all();
    }
}

#[cfg(test)]
#[path = "delivery_tests.rs"]
mod tests;
