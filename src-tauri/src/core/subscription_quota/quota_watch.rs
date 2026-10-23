//! Two-read confirmation and persistent panel latches for official quota risk.
//! This consumes an admitted receipt inside its transaction; queries never advance it.
use super::super::{err, hash, Result};
use super::model::{NativeForecast, NativeParams, ALGORITHM_VERSION};
use crate::budget_clock::Reading;
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const MAX_WINDOWS: usize = 200;
const MAX_STATES: usize = 4096;
const MAX_STATE_BYTES: usize = 16 * 1024;
const MAX_EVENT_BYTES: usize = 64 * 1024;
const MAX_INPUT_SAMPLES: usize = 1024;
const REENTRY_COOLDOWN_MS: u64 = 30 * 60 * 1000;
const MAX_GAP_MS: u64 = 240_000;
const CLOCK_TOLERANCE_MS: i64 = 2000;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct State {
    schema_version: u32,
    owner_id: String,
    scope: Value,
    period_instance_id: String,
    window_id: String,
    last_sequence: String,
    last_utc: DateTime<Utc>,
    last_mono_ms: String,
    fact_lower: Option<f64>,
    fact_upper: Option<f64>,
    clock_epoch: Option<String>,
    condition: String,
    risk_active: bool,
    ever_risk: bool,
    risk_streak: u8,
    safe_streak: u8,
    risk_episode: String,
    candidate_eligible: bool,
    last_notified_utc: Option<DateTime<Utc>>,
    last_notified_mono_ms: Option<String>,
    last_event_id: Option<String>,
    algorithm_version: String,
    params: NativeParams,
    updated_at: DateTime<Utc>,
}

struct Round {
    source: String,
    sequence: u64,
    successful: bool,
}

struct Window {
    owner_id: String,
    scope: Value,
    id: String,
    period: String,
    condition: &'static str,
    fact: Option<(f64, f64)>,
    clock: Option<String>,
    forecast: Option<NativeForecast>,
    input_sequences: Vec<String>,
}

fn text(v: &Value, max: usize) -> Option<&str> {
    v.as_str()
        .filter(|s| !s.is_empty() && s.len() <= max && !s.chars().any(char::is_control))
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
fn seconds(v: &Value) -> Option<f64> {
    instant(v).map(|t| t.timestamp_millis() as f64 / 1000.0)
}
fn timestamp(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn round(db: &Connection, current: &Value, runtime: &str) -> Result<Option<Round>> {
    if current["runtimeId"].as_str() != Some(runtime) {
        return Ok(None);
    }
    let Some(source) = text(&current["sourceId"], 512) else {
        return Ok(None);
    };
    let Some(sequence) = decimal(&current["observationSequence"]).filter(|n| *n > 0) else {
        return Ok(None);
    };
    // The latest receipt, rather than a caller's persisted=true bit, establishes
    // that this is the current transaction's newly accepted official round.
    let raw: Option<String> = db.query_row(
        "SELECT data FROM account_snapshots WHERE json_extract(data,'$.documentKind')='subscription_observation' AND json_extract(data,'$.runtimeId')=?1 AND json_extract(data,'$.sourceId')=?2 ORDER BY id DESC LIMIT 1",
        params![runtime,source],|r|r.get(0),
    ).optional().map_err(err)?;
    let Some(raw) = raw else {
        return Ok(None);
    };
    let stored: Value = serde_json::from_str(&raw).map_err(err)?;
    let receipt = super::observation_storage::resolve(db, &stored)?;
    if receipt["schemaVersion"] != 1
        || decimal(&receipt["observationSequence"]) != Some(sequence)
        || decimal(&receipt["lastSettledSequence"]) != Some(sequence)
        || decimal(&receipt["lastPersistedSequence"]) != Some(sequence)
        || receipt["outcome"] != current["observationOutcome"]
        || [
            "runtimeId",
            "sourceId",
            "bindingGeneration",
            "identityEpoch",
            "clearGeneration",
            "observationSequence",
        ]
        .iter()
        .any(|key| current.get(*key).is_none() || receipt.get(*key) != current.get(*key))
    {
        return Ok(None);
    }
    let claimed_success = current["observationOutcome"] == "success"
        && current["identityConsistency"] == "proven"
        && current["identityEvidence"].is_object();
    let hydrated = super::calibration_assembler::hydrated_observation(&receipt);
    let matching_proof = hydrated.as_ref().is_some_and(|hydrated| {
        [
            "runtimeId",
            "sourceId",
            "bindingGeneration",
            "identityEpoch",
            "clearGeneration",
            "observationSequence",
            "identityConsistency",
            "identityEvidence",
            "observationOutcome",
            "status",
            "quotaRead",
            "buckets",
            "accountScope",
            "entitlementSegment",
            "entitlementEvidence",
            "clockEpoch",
            "authMode",
            "planType",
            "historyOnly",
            "replayed",
            "live",
            "discontinuity",
        ]
        .iter()
        .all(|key| hydrated.get(*key) == current.get(*key))
    });
    if claimed_success && !matching_proof {
        return Ok(None);
    }
    let successful = claimed_success
        && current["identityEvidence"].is_object()
        && current["status"] == "connected"
        && current["quotaRead"]["outcome"] == "success"
        && current["persistenceError"].is_null()
        && current["historyOnly"] != true
        && current["replayed"] != true
        && current["live"] != false
        && matching_proof;
    Ok(Some(Round {
        source: source.into(),
        sequence,
        successful,
    }))
}

fn window(
    current: &Value,
    round: &Round,
    id: String,
    raw: &Value,
    derived: &Value,
    at: &Reading,
) -> Option<Window> {
    let runtime = text(&current["runtimeId"], 128)?;
    let binding = decimal(&current["bindingGeneration"])?;
    let epoch = decimal(&current["identityEpoch"]).filter(|n| *n > 0)?;
    let clear = current.get("clearGeneration")?;
    if !clear.is_null() && text(clear, 128).is_none() {
        return None;
    }
    let period = text(&raw["windowInstanceId"], 512)?.to_owned();
    let account = current.get("accountScope").unwrap_or(&Value::Null);
    if !account.is_null() && text(account, 512).is_none() {
        return None;
    }
    let entitlement = text(&current["entitlementSegment"], 512)
        .map(str::to_owned)
        .unwrap_or_else(|| {
            format!(
                "observed-plan:{}:{}",
                text(&current["authMode"], 128).unwrap_or("unknown"),
                text(&current["planType"], 128).unwrap_or("unknown")
            )
        });
    let scope = json!({"runtimeId":runtime,"sourceId":round.source,"bindingGeneration":binding.to_string(),
        "identityEpoch":epoch.to_string(),"clearGeneration":clear,"accountScope":account,
        "entitlementSegment":entitlement,"ruleVersion":text(&raw["ruleVersion"],512).unwrap_or("observed-rule-unknown"),"poolId":id});
    let owner_id = hash(scope.to_string().as_bytes());
    let mut w = Window {
        owner_id,
        scope,
        id,
        period,
        condition: "unknown",
        fact: None,
        clock: None,
        forecast: None,
        input_sequences: vec![],
    };
    let quota = &current["quotaRead"];
    let Some(basis) = text(&quota["timeBasis"], 128) else {
        return Some(w);
    };
    if matches!(
        basis,
        "unknown"
            | "request_start_only"
            | "unbounded"
            | "unbounded-upstream"
            | "request-start-fallback"
    ) {
        return Some(w);
    }
    let Some(anchor) = instant(&quota["freshnessAnchor"]) else {
        return Some(w);
    };
    let Some(lower) = seconds(&quota["factTimeLower"]) else {
        return Some(w);
    };
    let Some(upper) = seconds(&quota["factTimeUpper"]) else {
        return Some(w);
    };
    let now = at.utc.timestamp_millis() as f64 / 1000.0;
    let Some(clock) = text(&quota["clockEpoch"], 128).or_else(|| text(&current["clockEpoch"], 128))
    else {
        return Some(w);
    };
    if anchor > at.utc
        || (at.utc - anchor).num_milliseconds() >= MAX_GAP_MS as i64
        || lower < 0.0
        || upper < lower
        || upper > now
        || now - lower > 240.0
        || !matches!(
            raw["recovery"].as_str(),
            Some("fixed_reset" | "anchored_reset")
        )
        || current["discontinuity"] == true
    {
        return Some(w);
    }
    w.fact = Some((lower, upper));
    w.clock = Some(clock.into());
    let value = &derived["risks"][&w.id]["evidence"];
    if value.to_string().len() > MAX_EVENT_BYTES {
        return Some(w);
    }
    let Ok(forecast) = serde_json::from_value::<NativeForecast>(value.clone()) else {
        return Some(w);
    };
    let prefix = format!("{runtime}:");
    let suffix = format!(":{}", w.id);
    let mut last = None;
    if forecast.input_observation_ids.len() > MAX_INPUT_SAMPLES {
        return Some(w);
    }
    for input in &forecast.input_observation_ids {
        let Some(seq) = input
            .strip_prefix(&prefix)
            .and_then(|s| s.strip_suffix(&suffix))
            .and_then(|s| decimal(&json!(s)))
        else {
            return Some(w);
        };
        if seq == 0 || seq > round.sequence || last.is_some_and(|old| seq <= old) {
            return Some(w);
        }
        last = Some(seq);
        w.input_sequences.push(seq.to_string());
    }
    let Some(exhaustion) = forecast.exhaustion_time.filter(|i| i.is_valid()) else {
        return Some(w);
    };
    let Some(reset) = forecast.reset_time.filter(|i| i.is_valid()) else {
        return Some(w);
    };
    let raw_reset = if raw["resetTimeLower"].is_null() && raw["resetTimeUpper"].is_null() {
        raw["resetsAt"]
            .as_i64()
            .filter(|t| *t >= 0)
            .map(|t| (t as f64, t as f64))
    } else {
        seconds(&raw["resetTimeLower"]).zip(seconds(&raw["resetTimeUpper"]))
    };
    if forecast.algorithm_version != ALGORITHM_VERSION
        || derived["algorithmVersion"] != ALGORITHM_VERSION
        || forecast.sample_count < 6
        || forecast.input_observation_ids.len() < forecast.sample_count
        || last != Some(round.sequence)
        || !forecast.reasons.is_empty()
        || raw_reset != Some((reset.lower, reset.upper))
        || reset.lower <= now
        || exhaustion.upper <= now
    {
        return Some(w);
    }
    w.condition = if forecast.notify_candidate
        && forecast.will_exhaust_before_reset == Some(true)
        && exhaustion.upper < reset.lower
    {
        "risk"
    } else if forecast.safe_candidate
        && forecast.will_exhaust_before_reset == Some(false)
        && exhaustion.lower > reset.upper
    {
        "safe"
    } else {
        "unknown"
    };
    w.forecast = Some(forecast);
    Some(w)
}

fn continuous(state: &State, w: &Window, at: &Reading) -> bool {
    let Some((lower, upper)) = w.fact else {
        return false;
    };
    let Some((old_lower, old_upper)) = state.fact_lower.zip(state.fact_upper) else {
        return false;
    };
    let Some(old_mono) = decimal(&json!(state.last_mono_ms)) else {
        return false;
    };
    let Some(elapsed) = at.mono_ms.checked_sub(old_mono) else {
        return false;
    };
    let wall = (at.utc - state.last_utc).num_milliseconds();
    wall >= 0
        && elapsed <= MAX_GAP_MS
        && (wall - i64::try_from(elapsed).unwrap_or(i64::MAX)).abs() <= CLOCK_TOLERANCE_MS
        && lower >= old_lower
        && upper >= old_upper
        && upper - old_lower <= 240.0
        && state.clock_epoch == w.clock
        && state.algorithm_version == ALGORITHM_VERSION
        && state.params == NativeParams::default()
}

fn fresh_state(w: &Window, round: &Round, at: &Reading) -> State {
    State {
        schema_version: 1,
        owner_id: w.owner_id.clone(),
        scope: w.scope.clone(),
        period_instance_id: w.period.clone(),
        window_id: w.id.clone(),
        last_sequence: round.sequence.to_string(),
        last_utc: at.utc,
        last_mono_ms: at.mono_ms.to_string(),
        fact_lower: w.fact.map(|f| f.0),
        fact_upper: w.fact.map(|f| f.1),
        clock_epoch: w.clock.clone(),
        condition: w.condition.into(),
        // A first risk is a baseline, not a newly observed transition to notify.
        risk_active: w.condition == "risk",
        ever_risk: w.condition == "risk",
        risk_streak: 0,
        safe_streak: 0,
        risk_episode: "0".into(),
        candidate_eligible: false,
        last_notified_utc: None,
        last_notified_mono_ms: None,
        last_event_id: None,
        algorithm_version: ALGORITHM_VERSION.into(),
        params: NativeParams::default(),
        updated_at: at.utc,
    }
}

fn save(db: &Connection, state: &State) -> Result<()> {
    let data = serde_json::to_string(state).map_err(err)?;
    if data.len() > MAX_STATE_BYTES {
        return Err("额度风险状态超过容量上限".into());
    }
    db.execute("INSERT INTO plan_period_state(owner_kind,owner_id,period_instance_id,data) VALUES('quota_watch',?1,?2,?3) ON CONFLICT(owner_kind,owner_id,period_instance_id) DO UPDATE SET data=excluded.data",
        params![state.owner_id,state.period_instance_id,data]).map_err(err)?;
    Ok(())
}

fn prune(db: &Connection, at: DateTime<Utc>) -> Result<()> {
    db.execute("DELETE FROM plan_period_state WHERE owner_kind='quota_watch' AND julianday(json_extract(data,'$.updatedAt'))<julianday(?1)",
        [timestamp(at-Duration::days(90))]).map_err(err)?;
    db.execute("DELETE FROM plan_period_state WHERE rowid IN (SELECT rowid FROM plan_period_state WHERE owner_kind='quota_watch' ORDER BY json_extract(data,'$.updatedAt') DESC,rowid DESC LIMIT -1 OFFSET ?1)",
        [MAX_STATES as i64]).map_err(err)?;
    Ok(())
}

/// Run after insertion of the current accepted receipt, before the same commit.
/// Native notification and pet delivery deliberately remain unqualified here.
pub(crate) fn advance(
    db: &Connection,
    current: &Value,
    derived: &Value,
    runtime: &str,
    at: &Reading,
) -> Result<()> {
    if db.is_autocommit() {
        return Err("额度风险状态必须与观测收据同事务提交".into());
    }
    let Some(round) = round(db, current, runtime)? else {
        return Ok(());
    };
    let mut rows = db.prepare("SELECT owner_id,period_instance_id,data FROM plan_period_state WHERE owner_kind='quota_watch' AND json_extract(data,'$.scope.runtimeId')=?1 AND json_extract(data,'$.scope.sourceId')=?2 ORDER BY rowid DESC LIMIT ?3").map_err(err)?;
    let original = rows
        .query_map(
            params![runtime, round.source, (MAX_STATES + 1) as i64],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        )
        .map_err(err)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(err)?;
    if original
        .iter()
        .any(|(_, _, raw)| raw.len() > MAX_STATE_BYTES)
    {
        return Err("额度风险状态超过容量上限".into());
    }
    let mut states = BTreeMap::new();
    for (owner, period, raw) in original {
        let state: State = serde_json::from_str(&raw).map_err(err)?;
        if state.schema_version != 1
            || state.owner_id != owner
            || state.period_instance_id != period
        {
            return Err("额度风险状态版本或归属无效".into());
        }
        // Old/repeated callbacks have no state, latch, freshness or prune effect.
        if decimal(&json!(state.last_sequence)).is_none_or(|seq| seq >= round.sequence) {
            return Ok(());
        }
        states.insert((owner, period), state);
    }
    let mut windows = Vec::new();
    if round.successful {
        let buckets = current["buckets"].as_array();
        if buckets.is_some_and(|b| b.len() <= MAX_WINDOWS / 2) {
            for bucket in buckets.into_iter().flatten() {
                let Some(pool) = text(&bucket["id"], 480) else {
                    continue;
                };
                for kind in ["primary", "secondary"] {
                    if bucket[kind].is_object() {
                        if let Some(w) = window(
                            current,
                            &round,
                            format!("{pool}:{kind}"),
                            &bucket[kind],
                            derived,
                            at,
                        ) {
                            windows.push(w);
                        }
                    }
                }
            }
        }
    }
    let current_keys: Vec<_> = windows
        .iter()
        .map(|w| (w.owner_id.clone(), w.period.clone()))
        .collect();
    if current_keys.iter().collect::<BTreeSet<_>>().len() != current_keys.len() {
        windows.clear();
    }
    let current_keys: BTreeSet<_> = windows
        .iter()
        .map(|w| (w.owner_id.clone(), w.period.clone()))
        .collect();
    if states.is_empty() && windows.is_empty() {
        return Ok(());
    }
    let mut cursor_saved = false;
    for (key, state) in &mut states {
        if !current_keys.contains(key) {
            // Retired windows need one invalidation, not a write at every poll.
            // If no window can be evaluated, retain one sequence watermark so
            // a repeated failed receipt cannot refresh state or run pruning.
            if state.condition == "unknown"
                && !state.candidate_eligible
                && state.risk_streak == 0
                && state.safe_streak == 0
                && (!windows.is_empty() || cursor_saved)
            {
                continue;
            }
            state.risk_streak = 0;
            state.safe_streak = 0;
            state.candidate_eligible = false;
            state.condition = "unknown".into();
            state.last_sequence = round.sequence.to_string();
            state.last_utc = at.utc;
            state.last_mono_ms = at.mono_ms.to_string();
            state.fact_lower = None;
            state.fact_upper = None;
            state.clock_epoch = None;
            state.updated_at = at.utc;
            save(db, state)?;
            cursor_saved = true;
        }
    }
    for w in windows {
        let key = (w.owner_id.clone(), w.period.clone());
        // If a row exists with an equal/newer sequence, it was intentionally
        // excluded above and must not be recreated as a new baseline.
        let stored_sequence: Option<String> = db.query_row("SELECT json_extract(data,'$.lastSequence') FROM plan_period_state WHERE owner_kind='quota_watch' AND owner_id=?1 AND period_instance_id=?2",params![w.owner_id,w.period],|r|r.get(0)).optional().map_err(err)?;
        if stored_sequence
            .as_ref()
            .and_then(|s| decimal(&json!(s)))
            .is_some_and(|s| s >= round.sequence)
        {
            continue;
        }
        let Some(mut state) = states.remove(&key) else {
            save(db, &fresh_state(&w, &round, at))?;
            continue;
        };
        let connected = continuous(&state, &w, at);
        let previous_condition = state.condition.clone();
        state.candidate_eligible = false;
        if w.condition == "risk" {
            state.safe_streak = 0;
            state.risk_streak = if connected && previous_condition == "risk" {
                state.risk_streak.saturating_add(1).min(2)
            } else {
                1
            };
            if !state.risk_active && state.risk_streak >= 2 {
                let episode = decimal(&json!(state.risk_episode)).ok_or("额度风险轮次无效")?;
                let episode = if state.ever_risk {
                    episode.checked_add(1).ok_or("额度风险轮次已耗尽")?
                } else {
                    episode
                };
                state.risk_episode = episode.to_string();
                state.risk_active = true;
                state.ever_risk = true;
                state.candidate_eligible = true;
                let cooled = match (
                    state.last_notified_utc,
                    state.last_notified_mono_ms.as_ref(),
                ) {
                    (None, None) => true,
                    (Some(wall), Some(mono)) => {
                        let elapsed = decimal(&json!(mono)).and_then(|n| at.mono_ms.checked_sub(n));
                        let wall_ms = (at.utc - wall).num_milliseconds();
                        elapsed.is_some_and(|ms| {
                            ms >= REENTRY_COOLDOWN_MS && wall_ms >= REENTRY_COOLDOWN_MS as i64
                        })
                    }
                    _ => false,
                };
                if cooled {
                    let event_id = hash(
                        format!(
                            "quota_watch|{}|{}|native_quota_eta|risk|{}",
                            w.owner_id, w.period, state.risk_episode
                        )
                        .as_bytes(),
                    );
                    let mut evidence = serde_json::to_value(&w.forecast).map_err(err)?;
                    if let Some(evidence) = evidence.as_object_mut() {
                        evidence.remove("inputObservationIds");
                    }
                    let event = json!({"id":event_id,"kind":"quota_eta","reminderKind":"native_quota_eta","ownerKind":"quota_watch","ownerId":w.owner_id,
                        "sourceId":round.source,"threadId":"","turnId":null,"periodInstanceId":w.period,"windowId":w.id,"riskEpisode":state.risk_episode,
                        "eventTime":timestamp(at.utc),"createdAt":timestamp(at.utc),"title":"官方额度可能在重置前触限",
                        "message":"连续两次新官方观测均确认近期活动情景早于下一次重置。","scope":w.scope,
                        "inputObservationSequences":[state.last_sequence,round.sequence.to_string()],"forecastInputSequences":w.input_sequences,
                        "evidence":evidence,"forecastInputDigest":hash(serde_json::to_string(&w.forecast).map_err(err)?.as_bytes()),"algorithmVersion":ALGORITHM_VERSION,"params":NativeParams::default(),
                        "notifyEligible":false,"animationAcknowledged":true,"eligibleRuntime":runtime,"notificationState":"panel_only","notified":false,"read":false});
                    let data = event.to_string();
                    if data.len() > MAX_EVENT_BYTES {
                        return Err("额度风险事件超过容量上限".into());
                    }
                    db.execute("INSERT OR IGNORE INTO inbox_events(id,thread_id,turn_id,event_time,data) VALUES(?1,'',NULL,?2,?3)",params![event_id,timestamp(at.utc),data]).map_err(err)?;
                    state.last_notified_utc = Some(at.utc);
                    state.last_notified_mono_ms = Some(at.mono_ms.to_string());
                    state.last_event_id = Some(event_id);
                }
            }
        } else if w.condition == "safe" {
            state.risk_streak = 0;
            state.safe_streak = if connected && previous_condition == "safe" {
                state.safe_streak.saturating_add(1).min(2)
            } else {
                1
            };
            if state.safe_streak >= 2 {
                state.risk_active = false;
            }
        } else {
            state.risk_streak = 0;
            state.safe_streak = 0;
        }
        state.condition = w.condition.into();
        state.last_sequence = round.sequence.to_string();
        state.last_utc = at.utc;
        state.last_mono_ms = at.mono_ms.to_string();
        state.fact_lower = w.fact.map(|f| f.0);
        state.fact_upper = w.fact.map(|f| f.1);
        state.clock_epoch = w.clock;
        state.algorithm_version = ALGORITHM_VERSION.into();
        state.params = NativeParams::default();
        state.updated_at = at.utc;
        save(db, &state)?;
    }
    prune(db, at.utc)?;
    Ok(())
}

#[cfg(test)]
#[path = "quota_watch_tests.rs"]
mod tests;
