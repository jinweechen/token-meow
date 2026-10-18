//! Short-lived, in-memory proof for a budget crossing. Persisted usage and the
//! period latch remain facts; neither a restart nor a missing period implies 0.
use super::*;
use crate::budget_clock::{self, Reading};
use crate::notification_gate::{Gate, NotificationPermit};
use std::sync::Arc;

const CONTINUITY_MS: u64 = 15_000;
const CLOCK_SKEW_MS: i64 = 2_000;

#[derive(Default)]
pub(crate) struct Observer {
    watches: BTreeMap<String, Watch>,
    permits: BTreeMap<String, PermitEntry>,
    clock_high: Option<Reading>,
}
struct Watch {
    signature: String,
    domain: String,
    epoch: u64,
    at: Reading,
    fresh_floor: chrono::DateTime<Utc>,
    period: String,
    ordinary: bool,
    baselines: BTreeMap<String, i64>,
    data_version: i64,
}
struct PermitEntry {
    budget: String,
    data_version: i64,
    permit: NotificationPermit,
}
pub(crate) struct Scan {
    at: Reading,
    host: Option<(u64, Reading)>,
    rowid: i64,
    metadata_rowid: i64,
    turn_rowid: i64,
    domain: String,
    data_version: i64,
    baselines: BTreeMap<String, BTreeMap<String, i64>>,
    allowed: BTreeSet<String>,
}

fn timestamp(at: Reading) -> String {
    at.utc.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
fn close(a: Reading, b: Reading) -> bool {
    let Some(mono) = b.mono_ms.checked_sub(a.mono_ms) else {
        return false;
    };
    let wall = (b.utc - a.utc).num_milliseconds();
    wall >= 0
        && mono <= CONTINUITY_MS
        && wall <= CONTINUITY_MS as i64
        && (wall - mono as i64).abs() <= CLOCK_SKEW_MS
}
fn max_rowid(db: &Connection, table: &str) -> Result<i64> {
    db.query_row(
        &format!("SELECT COALESCE(MAX(rowid),0) FROM {table}"),
        [],
        |r| r.get(0),
    )
    .map_err(err)
}
fn data_version(db: &Connection) -> Result<i64> {
    db.query_row("PRAGMA data_version", [], |r| r.get(0))
        .map_err(err)
}
fn signature(b: &Value) -> String {
    hash(
        json!([
            b["id"],
            b["version"],
            b["scope"],
            b["sourceId"],
            b["target"],
            b["threshold"],
            b["enabled"]
        ])
        .to_string()
        .as_bytes(),
    )
}
fn state_key(b: &Value) -> String {
    format!(
        "budgetState:{}:{}:{}",
        strv(b, "id"),
        b["version"],
        strv(b, "period")
    )
}
fn reminder_id(b: &Value, threshold: i64) -> String {
    hash(
        format!(
            "budget|{}|{}|{}|{}|{}|{}|{}",
            strv(b, "id"),
            b["version"],
            strv(b, "scope"),
            strv(b, "sourceId"),
            strv(b, "target"),
            strv(b, "period"),
            threshold
        )
        .as_bytes(),
    )
}

// Membership only: normal turn completion/time/model enrichment is not a
// project/root reassignment. New rows are allowed; edits to existing members
// must not borrow a preceding observation's authorization.
fn domain(core: &Core, metadata_limit: i64, turn_limit: i64) -> Result<String> {
    let mut digest = Sha256::new();
    let mut statement = core
        .db
        .prepare("SELECT id,root,enabled,client_kind,environment FROM source_instances ORDER BY id")
        .map_err(err)?;
    let rows = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, bool>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })
        .map_err(err)?;
    for row in rows {
        digest.update(serde_json::to_vec(&row.map_err(err)?).map_err(err)?);
    }
    for key in [
        "primarySourceId",
        "projectMappings",
        "importSince",
        "historyReplayAt",
        "clearWatermark",
        "historyPaused",
        "historyCancelled",
    ] {
        digest.update(key.as_bytes());
        digest.update(
            get_setting(&core.db, key)?
                .unwrap_or(Value::Null)
                .to_string()
                .as_bytes(),
        );
    }
    digest.update(core.settings()?["retentionDays"].to_string().as_bytes());
    for (table, column) in [("excluded_facts", "fact_id"), ("fact_aliases", "alias")] {
        digest.update(table.as_bytes());
        let mut s = core
            .db
            .prepare(&format!("SELECT {column} FROM {table} ORDER BY {column}"))
            .map_err(err)?;
        let rows = s.query_map([], |r| r.get::<_, String>(0)).map_err(err)?;
        for row in rows {
            digest.update(serde_json::to_vec(&row.map_err(err)?).map_err(err)?);
        }
    }
    for (table, limit) in [("thread_metadata", metadata_limit), ("turns", turn_limit)] {
        digest.update(table.as_bytes());
        let mut s = core
            .db
            .prepare(&format!(
                "SELECT id,data FROM {table} WHERE rowid<=?1 ORDER BY rowid"
            ))
            .map_err(err)?;
        let rows = s
            .query_map([limit], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(err)?;
        for row in rows {
            let (id, text) = row.map_err(err)?;
            let v: Value = serde_json::from_str(&text).map_err(err)?;
            let membership = if table == "thread_metadata" {
                json!([
                    id,
                    v["sourceId"],
                    v["id"],
                    v["cwd"],
                    v["parentThreadId"],
                    v["agentKindEvidence"],
                    v["agentKindReason"]
                ])
            } else {
                json!([id, v["sourceId"], v["id"], v["threadId"], v["rootTurnId"]])
            };
            digest.update(membership.to_string().as_bytes());
        }
    }
    Ok(format!("{:x}", digest.finalize()))
}
pub(crate) fn configuration_fingerprint(core: &Core) -> Result<String> {
    domain(core, i64::MAX, i64::MAX)
}
fn baselines(core: &Core, at: Reading) -> Result<BTreeMap<String, BTreeMap<String, i64>>> {
    let mut result: BTreeMap<String, BTreeMap<String, i64>> = BTreeMap::new();
    let next = at
        .utc
        .checked_add_signed(Duration::days(1))
        .ok_or("预算日期超出范围")?;
    for time in [at.utc, next] {
        for b in budget_views_at(core, time)? {
            if let Some(used) = b["usedTokens"].as_i64() {
                result
                    .entry(strv(&b, "id"))
                    .or_default()
                    .insert(strv(&b, "period"), used);
            }
        }
    }
    Ok(result)
}
impl Observer {
    // A rejected lower/skewed clock must never become a new live anchor. Domain
    // invalidation clears authorization, not this runtime clock high-water.
    fn observe_clock(&mut self, at: Reading) -> bool {
        if let Some(high) = self.clock_high {
            self.clock_high = Some(Reading {
                utc: high.utc.max(at.utc),
                mono_ms: high.mono_ms.max(at.mono_ms),
            });
            let Some(mono) = at.mono_ms.checked_sub(high.mono_ms) else {
                return false;
            };
            let wall = (at.utc - high.utc).num_milliseconds();
            if wall < 0 || (i128::from(wall) - i128::from(mono)).abs() > i128::from(CLOCK_SKEW_MS) {
                return false;
            }
        }
        self.clock_high = Some(at);
        true
    }
    fn prune(&mut self, db: &Connection) {
        let current_version = data_version(db).ok();
        self.permits.retain(|_, entry| {
            let current = entry.permit.is_current() && current_version == Some(entry.data_version);
            if !current {
                entry.permit.revoke();
            }
            current
        });
    }
    pub(crate) fn invalidate(&mut self) {
        self.watches.clear();
        self.revoke_all();
    }
    pub(crate) fn revoke_all(&mut self) {
        for (_, entry) in std::mem::take(&mut self.permits) {
            entry.permit.revoke();
        }
    }
    pub(crate) fn remove_rule(&mut self, id: &str) {
        self.watches.remove(id);
        self.permits.retain(|_, entry| {
            if entry.budget == id {
                entry.permit.revoke();
                false
            } else {
                true
            }
        });
    }
    pub(crate) fn finish(&mut self, id: &str) {
        if let Some(entry) = self.permits.remove(id) {
            entry.permit.revoke();
        }
    }
    pub(crate) fn permit(&self, db: &Connection, id: &str) -> Option<NotificationPermit> {
        let entry = self.permits.get(id)?;
        if data_version(db).ok() != Some(entry.data_version) {
            entry.permit.revoke();
            return None;
        }
        entry.permit.is_current().then(|| entry.permit.clone())
    }
    #[cfg(test)]
    pub(crate) fn test_permit(
        &mut self,
        db: &Connection,
        gate: Arc<Gate>,
        id: &str,
        lifetime_ms: u64,
    ) -> NotificationPermit {
        let at = budget_clock::now();
        let deadline = at.checked_deadline(lifetime_ms).unwrap();
        let epoch = gate
            .snapshot()
            .expect("test must explicitly publish an available host");
        let permit = NotificationPermit::new(gate, epoch, deadline.utc, deadline.mono_ms);
        self.permits.insert(
            id.to_owned(),
            PermitEntry {
                budget: "fixture".into(),
                data_version: data_version(db).unwrap(),
                permit: permit.clone(),
            },
        );
        permit
    }
}

pub(crate) fn has_enabled(core: &Core) -> Result<bool> {
    core.db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM budgets WHERE json_extract(data,'$.enabled')=1)",
            [],
            |r| r.get(0),
        )
        .map_err(err)
}

pub(crate) fn begin(core: &mut Core, host: Option<(u64, Reading)>) -> Result<Option<Scan>> {
    let at = budget_clock::now();
    let clock_ok = core.budget_observer.observe_clock(at);
    if !clock_ok {
        core.budget_observer.invalidate();
    }
    if !has_enabled(core)? {
        core.budget_observer.invalidate();
        return Ok(None);
    }
    core.budget_observer.prune(&core.db);
    let rowid = max_rowid(&core.db, "usage_responses")?;
    let metadata_rowid = max_rowid(&core.db, "thread_metadata")?;
    let turn_rowid = max_rowid(&core.db, "turns")?;
    let domain = domain(core, metadata_rowid, turn_rowid)?;
    let data_version = data_version(&core.db)?;
    let baselines = baselines(core, at)?;
    let valid_host = host.filter(|(epoch, sampled)| {
        clock_ok && core.notification_gate.snapshot() == Some(*epoch) && close(*sampled, at)
    });
    if valid_host.is_none() {
        core.budget_observer.invalidate();
    }
    let mut allowed = BTreeSet::new();
    for b in budget_views_at(core, at.utc)? {
        let id = strv(&b, "id");
        if let (Some((epoch, _)), Some(watch)) = (valid_host, core.budget_observer.watches.get(&id))
        {
            let current = strv(&b, "period");
            let same_period = watch.period == current;
            if watch.signature == signature(&b)
                && watch.domain == domain
                && watch.epoch == epoch
                && close(watch.at, at)
                && watch.data_version == data_version
                && (same_period || watch.ordinary)
                && baselines.get(&id).and_then(|v| v.get(&current)) == watch.baselines.get(&current)
            {
                allowed.insert(id);
            }
        }
    }
    Ok(Some(Scan {
        at,
        host: valid_host,
        rowid,
        metadata_rowid,
        turn_rowid,
        domain,
        data_version,
        baselines,
        allowed,
    }))
}

pub(crate) fn initialize(core: &mut Core, id: &str) -> Result<()> {
    core.budget_observer.remove_rule(id);
    let at = budget_clock::now();
    let clock_ok = core.budget_observer.observe_clock(at);
    if !clock_ok {
        core.budget_observer.invalidate();
    }
    let all_baselines = baselines(core, at)?;
    let Some(b) = budget_views_at(core, at.utc)?
        .into_iter()
        .find(|b| strv(b, "id") == id)
    else {
        return Ok(());
    };
    if let Some(used) = b["usedTokens"].as_i64() {
        put_setting(
            &core.db,
            &state_key(&b),
            &json!({"used":used,"at":timestamp(at),"runtime":core.runtime_id,
            "handled":false,"allowInitialHistory":true}),
        )?;
        if let Some(epoch) = core.notification_gate.snapshot().filter(|_| clock_ok) {
            let watch = Watch {
                signature: signature(&b),
                domain: configuration_fingerprint(core)?,
                epoch,
                at,
                fresh_floor: at.utc,
                period: strv(&b, "period"),
                ordinary: false,
                baselines: all_baselines.get(id).cloned().unwrap_or_default(),
                data_version: data_version(&core.db)?,
            };
            core.budget_observer.watches.insert(id.to_owned(), watch);
        }
    }
    Ok(())
}

pub(crate) fn evaluate_without_scan(core: &mut Core) -> Result<()> {
    core.budget_observer.invalidate();
    evaluate(core, None, &BTreeSet::new(), budget_clock::now())
}
pub(crate) fn finish(
    core: &mut Core,
    mut scan: Option<Scan>,
    committed: &BTreeSet<String>,
    cancelled: bool,
) -> Result<()> {
    let at = budget_clock::now();
    if !core.budget_observer.observe_clock(at) {
        core.budget_observer.invalidate();
        if let Some(scan) = scan.as_mut() {
            scan.host = None;
        }
    }
    if cancelled {
        core.budget_observer.invalidate();
        return Ok(());
    }
    if scan.is_none() {
        return Ok(());
    }
    evaluate(core, scan, committed, at)
}

fn evaluate(
    core: &mut Core,
    scan: Option<Scan>,
    committed: &BTreeSet<String>,
    at: Reading,
) -> Result<()> {
    if !has_enabled(core)? {
        core.budget_observer.invalidate();
        return Ok(());
    }
    let views = budget_views_at(core, at.utc)?;
    if views.is_empty() {
        core.budget_observer.invalidate();
        return Ok(());
    }
    let mut new_ids = BTreeMap::new();
    let mut stable = if let Some(scan) = scan.as_ref() {
        let mut s = core
            .db
            .prepare("SELECT id,source_id FROM usage_responses WHERE rowid>?1")
            .map_err(err)?;
        let rows = s
            .query_map([scan.rowid], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(err)?;
        for row in rows {
            let (id, source) = row.map_err(err)?;
            new_ids.insert(id, source);
        }
        scan.host.is_some_and(|(epoch, sampled)| {
            core.notification_gate.snapshot() == Some(epoch) && close(sampled, at)
        }) && close(scan.at, at)
            && data_version(&core.db)? == scan.data_version
            && domain(core, scan.metadata_rowid, scan.turn_rowid)? == scan.domain
    } else {
        false
    };
    if !stable {
        core.budget_observer.invalidate();
    }
    // Retain scalar summaries only, never two complete Loaded snapshots.
    let post_baselines = baselines(core, at)?;
    let full_domain = configuration_fingerprint(core)?;
    let external_version = data_version(&core.db)?;
    stable &= scan
        .as_ref()
        .is_some_and(|s| external_version == s.data_version);
    if !stable {
        core.budget_observer.invalidate();
    }
    let mut next_watches = BTreeMap::new();
    for b in views {
        if b["enabled"].as_bool() != Some(true) {
            continue;
        }
        let Some(used) = b["usedTokens"].as_i64() else {
            continue;
        };
        let id = strv(&b, "id");
        let period = strv(&b, "period");
        let threshold = b["threshold"].as_i64().unwrap_or(i64::MAX);
        let loaded = load(
            core,
            &if strv(&b, "scope") == "rootTurn" {
                json!({})
            } else {
                json!({"from":period,"to":period})
            },
        )?;
        let mut all_new = Some(0i64);
        let mut fresh = Some(0i64);
        let mut fresh_until: Option<Reading> = None;
        let mut origins_known = true;
        for r in loaded
            .responses
            .iter()
            .filter(|r| budget_matches(&b, r, &loaded.sessions))
        {
            let Some(source) = new_ids.get(&response_key(r)) else {
                continue;
            };
            let n = r.counts.total.value().unwrap_or(0);
            all_new = all_new.and_then(|v| v.checked_add(n));
            // New identity + live decoder flag + successful source transaction
            // proves this live cursor; unrelated history health is not admission.
            let origin = source == &r.source_id && committed.contains(source);
            origins_known &= origin;
            let fresh_time = scan.as_ref().is_some_and(|scan| {
                let event = chrono::DateTime::parse_from_rfc3339(&r.event_time)
                    .ok()
                    .map(|d| d.with_timezone(&Utc));
                let collected = chrono::DateTime::parse_from_rfc3339(&r.collected_at)
                    .ok()
                    .map(|d| d.with_timezone(&Utc));
                // Keep the beginning of this continuous observation, rather
                // than each previous scan's finish: a file can append after its
                // length was sampled and before that scan completed.
                let prior = core
                    .budget_observer
                    .watches
                    .get(&id)
                    .map(|w| w.fresh_floor)
                    .unwrap_or(scan.at.utc);
                event.is_some_and(|event| {
                    event >= prior
                        && event
                            >= chrono::DateTime::parse_from_rfc3339(&core.runtime_start)
                                .map(|d| d.with_timezone(&Utc))
                                .unwrap_or(at.utc)
                        && event <= at.utc + Duration::seconds(5)
                        && (at.utc - event).num_milliseconds() <= CONTINUITY_MS as i64
                }) && collected.is_some_and(|collected| {
                    collected >= scan.at.utc - Duration::milliseconds(1) && collected <= at.utc
                })
            });
            if r.live && origin && fresh_time && n > 0 {
                fresh = fresh.and_then(|v| v.checked_add(n));
                let deadline = chrono::DateTime::parse_from_rfc3339(&r.event_time)
                    .ok()
                    .and_then(|event| {
                        event
                            .with_timezone(&Utc)
                            .checked_add_signed(Duration::milliseconds(CONTINUITY_MS as i64))
                    })
                    .and_then(|expires| u64::try_from((expires - at.utc).num_milliseconds()).ok())
                    .and_then(|remaining| at.checked_deadline(remaining));
                fresh_until = match (fresh_until, deadline) {
                    (Some(a), Some(b)) => Some(Reading {
                        utc: a.utc.min(b.utc),
                        mono_ms: a.mono_ms.min(b.mono_ms),
                    }),
                    (None, Some(b)) => Some(b),
                    _ => {
                        fresh = None;
                        None
                    }
                };
            }
        }
        let pre = scan
            .as_ref()
            .and_then(|s| s.baselines.get(&id))
            .and_then(|v| v.get(&period))
            .copied();
        let additive = pre
            .zip(all_new)
            .and_then(|(pre, delta)| pre.checked_add(delta))
            == Some(used);
        let eligible = stable
            && origins_known
            && additive
            && scan.as_ref().is_some_and(|s| s.allowed.contains(&id))
            && core.budget_observer.watches.get(&id).is_some_and(|w| {
                w.signature == signature(&b) && (w.period == period || w.ordinary)
            })
            && pre.is_some_and(|n| n < threshold)
            && fresh_until
                .is_some_and(|deadline| deadline.utc > at.utc && deadline.mono_ms > at.mono_ms)
            && pre
                .zip(fresh)
                .and_then(|(pre, delta)| pre.checked_add(delta))
                .is_some_and(|n| n >= threshold);
        if stable && !committed.is_empty() && origins_known && additive {
            let epoch = scan.as_ref().and_then(|s| s.host).map(|v| v.0).unwrap();
            next_watches.insert(
                id.clone(),
                Watch {
                    signature: signature(&b),
                    domain: full_domain.clone(),
                    epoch,
                    at,
                    fresh_floor: scan
                        .as_ref()
                        .filter(|s| s.allowed.contains(&id))
                        .and_then(|_| core.budget_observer.watches.get(&id).map(|w| w.fresh_floor))
                        .unwrap_or_else(|| scan.as_ref().map(|s| s.at.utc).unwrap_or(at.utc)),
                    period: period.clone(),
                    ordinary: true,
                    baselines: post_baselines.get(&id).cloned().unwrap_or_default(),
                    data_version: external_version,
                },
            );
        }
        let key = state_key(&b);
        let previous = get_setting(&core.db, &key)?;
        let reminder_id = reminder_id(&b, threshold);
        let old = adapter::load_fact(&core.db, "inbox_events", &reminder_id)?;
        let legacy = previous
            .as_ref()
            .is_some_and(|v| v.get("handled").is_none());
        let handled = old.is_some()
            || legacy
            || previous
                .as_ref()
                .and_then(|v| v["handled"].as_bool())
                .unwrap_or(false);
        let create = used >= threshold && !handled;
        let next_state = json!({"used":used,"at":timestamp(at),"runtime":core.runtime_id,"handled":handled || create,
            "legacyHistoryBoundary":legacy || previous.as_ref().and_then(|v|v["legacyHistoryBoundary"].as_bool()).unwrap_or(false)});
        let mut data = old.clone();
        if create {
            let mut value = json!({"id":reminder_id,"kind":"budget","threadId":if strv(&b,"scope")=="thread"{strv(&b,"target")}else{String::new()},"turnId":Value::Null,
                "title":"本地软预算达到提醒阈值","status":"threshold","createdAt":timestamp(at),"read":false,"budgetId":b["id"],"period":period,"sourceId":b["sourceId"],"scope":b["scope"],"target":b["target"],
                "usedTokens":used,"threshold":threshold,"notifyEligible":eligible,"animationAcknowledged":false,"eligibleRuntime":core.runtime_id,
                "notificationState":if eligible{"pending"}else{"history-only"},"notified":false});
            if strv(&b, "scope") == "rootTurn" {
                if let Some(turn) = loaded.turns.iter().find(|t| {
                    strv(t, "sourceId") == strv(&b, "sourceId")
                        && strv(t, "id") == strv(&b, "target")
                }) {
                    value["threadId"] = turn["threadId"].clone();
                    value["turnId"] = b["target"].clone();
                }
            }
            data = Some(value);
        } else if let Some(value) = data.as_mut() {
            value["usedTokens"] = json!(used);
        }
        drop(loaded);
        let tx = core.db.transaction().map_err(err)?;
        put_setting(&tx, &key, &next_state)?;
        if let Some(data) = data {
            tx.execute("INSERT INTO inbox_events(id,event_time,data) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET data=excluded.data",params![reminder_id,timestamp(at),data.to_string()]).map_err(err)?;
        }
        tx.commit().map_err(err)?;
        if create && eligible {
            let deadline = scan
                .as_ref()
                .and_then(|s| s.host)
                .and_then(|(_, t)| t.checked_deadline(CONTINUITY_MS))
                .zip(fresh_until)
                .map(|(host, event)| Reading {
                    utc: host.utc.min(event.utc),
                    mono_ms: host.mono_ms.min(event.mono_ms),
                });
            if let (Some((epoch, sampled)), Some(deadline)) =
                (scan.as_ref().and_then(|s| s.host), deadline)
            {
                // Original sampled lifetime: no fresh lease is minted at claim/queue.
                let final_at = budget_clock::now();
                if core.budget_observer.observe_clock(final_at)
                    && close(sampled, final_at)
                    && core.notification_gate.snapshot() == Some(epoch)
                    && data_version(&core.db)? == external_version
                {
                    let permit = NotificationPermit::new(
                        Arc::clone(&core.notification_gate),
                        epoch,
                        deadline.utc,
                        deadline.mono_ms,
                    );
                    core.budget_observer.permits.insert(
                        reminder_id,
                        PermitEntry {
                            budget: id,
                            data_version: external_version,
                            permit,
                        },
                    );
                }
            }
        }
    }
    if core.budget_observer.observe_clock(budget_clock::now()) {
        core.budget_observer.watches = next_watches;
    } else {
        core.budget_observer.invalidate();
    }
    Ok(())
}

#[cfg(test)]
#[path = "budget_midnight_tests.rs"]
mod tests;
