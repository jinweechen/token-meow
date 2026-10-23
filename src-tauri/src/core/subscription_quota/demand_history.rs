//! Persistent local demand samples. These are covered local Token intervals,
//! never subscription capacity estimates. All writes belong to the caller's
//! plan/scan transaction and merge one child of plan_period_state.data.
use super::super::{adapter, err, hash, Result};
use super::evaluate::demand_draft;
use super::model::DemandInterval;
use crate::budget_clock::Reading;
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const VERSION: &str = "demand-history-v1";
const MAX_GAP_MS: u64 = 15_000;
const MAX_CLOCK_SKEW_MS: i128 = 2_000;
const DURATION_TOLERANCE: f64 = 0.05;
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Key {
    source_id: String,
    model_dimension: String,
    model: Option<String>,
    account_scope: String,
    entitlement_segment: String,
    rule_version: String,
    metering_version: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Capture {
    schema_version: u32,
    algorithm_version: String,
    plan_id: String,
    key: Option<Key>,
    runtime_scope: Option<Value>,
    entitlement_evidence: Option<Value>,
    starts_at: DateTime<Utc>,
    configured_end: Option<DateTime<Utc>>,
    end_policy: String,
    last_utc: DateTime<Utc>,
    last_mono_ms: String,
    last_used: Option<f64>,
    status: String,
    actual_end: Option<DateTime<Utc>>,
    amount: Option<f64>,
    ledger_fingerprint: Option<String>,
    reasons: Vec<String>,
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
    v.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
}
fn decimal(v: &Value) -> Option<u64> {
    let s = v.as_str()?;
    let n = s.parse::<u64>().ok()?;
    (n.to_string() == s).then_some(n)
}
fn amount(v: &Value) -> Option<f64> {
    v.as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0 && *n <= MAX_SAFE_INTEGER && n.fract() == 0.0)
}
fn add_reason(c: &mut Capture, reason: &str) {
    if !c.reasons.iter().any(|r| r == reason) {
        c.reasons.push(reason.into());
    }
}
fn invalidate(c: &mut Capture, reason: &str) {
    c.status = "invalidated".into();
    c.amount = None;
    add_reason(c, reason);
}
fn state(db: &Connection, id: &str) -> Result<Value> {
    let stored:Option<String>=db.query_row("SELECT data FROM plan_period_state WHERE owner_kind='plan' AND owner_id=?1 AND period_instance_id=?1",[id],|r|r.get(0)).optional().map_err(err)?;
    let v: Value = stored
        .map(|v| serde_json::from_str(&v).map_err(err))
        .transpose()?
        .unwrap_or_else(|| json!({}));
    if !v.is_object() {
        return Err("需求画像状态格式无效".into());
    }
    Ok(v)
}
fn save(db: &Connection, id: &str, mut original: Value, capture: &Capture) -> Result<()> {
    original["demandHistory"] = serde_json::to_value(capture).map_err(err)?;
    db.execute("INSERT INTO plan_period_state(owner_kind,owner_id,period_instance_id,data) VALUES('plan',?1,?1,?2) ON CONFLICT(owner_kind,owner_id,period_instance_id) DO UPDATE SET data=excluded.data",params![id,original.to_string()]).map_err(err)?;
    Ok(())
}
fn scope(
    snapshot: &Value,
    source: &str,
    runtime: &str,
    at: DateTime<Utc>,
) -> std::result::Result<(Value, String, String, String, Value), &'static str> {
    if snapshot["runtimeId"].as_str() != Some(runtime)
        || snapshot["sourceId"].as_str() != Some(source)
    {
        return Err("source_or_runtime_changed");
    }
    if snapshot["persisted"] != true
        || snapshot["identityConsistency"] != "proven"
        || !snapshot["identityEvidence"].is_object()
        || snapshot["observationOutcome"] != "success"
        || snapshot["status"] != "connected"
    {
        return Err("identity_unproven");
    }
    let account = text(&snapshot["accountScope"]).ok_or("stable_account_identity_unknown")?;
    let binding = decimal(&snapshot["bindingGeneration"]).ok_or("scope_unknown")?;
    let epoch = decimal(&snapshot["identityEpoch"])
        .filter(|n| *n > 0)
        .ok_or("scope_unknown")?;
    let clear = snapshot.get("clearGeneration").ok_or("scope_unknown")?;
    let anchor = instant(&snapshot["accountRead"]["freshnessAnchor"])
        .or_else(|| instant(&snapshot["quotaRead"]["freshnessAnchor"]))
        .ok_or("membership_stale")?;
    if anchor > at || (at - anchor).num_milliseconds() >= 240_000 {
        return Err("membership_stale");
    }
    let segment = text(&snapshot["entitlementSegment"]).ok_or("entitlement_evidence_unknown")?;
    let evidence = &snapshot["entitlementEvidence"];
    if evidence["verified"] != true
        || evidence["source"] != "official"
        || text(&evidence["segmentId"]) != Some(segment)
    {
        return Err("entitlement_evidence_unknown");
    }
    let rule = text(&evidence["ruleVersion"]).ok_or("entitlement_evidence_unknown")?;
    let reference = text(&evidence["reference"]).ok_or("entitlement_evidence_unknown")?;
    Ok((
        json!({"runtimeId":runtime,"sourceId":source,"bindingGeneration":binding.to_string(),"identityEpoch":epoch.to_string(),"clearGeneration":clear,"accountScope":account}),
        account.into(),
        segment.into(),
        rule.into(),
        json!({"verified":true,"source":"official","segmentId":segment,"ruleVersion":rule,"reference":reference}),
    ))
}
fn model_scope(value: &Value) -> std::result::Result<(String, Option<String>), &'static str> {
    let dimension = text(&value["modelDimension"])
        .filter(|d| matches!(*d, "response" | "request"))
        .ok_or("model_scope_unknown")?;
    let model = if value["model"].is_null() {
        None
    } else {
        Some(
            text(&value["model"])
                .ok_or("model_scope_unknown")?
                .to_owned(),
        )
    };
    Ok((dimension.into(), model))
}
fn actual_end(plan: &Value, view: &Value) -> Option<DateTime<Utc>> {
    let configured = instant(&view["endsAt"]).or_else(|| {
        if plan["endPolicy"] == "time" {
            instant(&plan["endsAt"])
        } else {
            None
        }
    });
    match (configured, instant(&plan["endedAt"])) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}
pub(super) fn ledger_fingerprint(
    db: &Connection,
    source: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<String> {
    ledger_fingerprint_inner(db, source, start, end, None)?
        .ok_or_else(|| "普通账本摘要不应排除新事实".into())
}

/// Reconstruct the old interval's fingerprint after removing only genuinely
/// fresh raw facts. Related alias/exclusion metadata is never filtered out:
/// such a fresh ID makes subtraction ambiguous and must fail closed.
pub(super) fn ledger_fingerprint_excluding_fresh(
    db: &Connection,
    source: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    fresh_ids: &BTreeSet<String>,
) -> Result<Option<String>> {
    ledger_fingerprint_inner(db, source, start, end, Some(fresh_ids))
}

fn requested_model_turn_evidence(
    db: &Connection,
    raw: &[(String, String, String)],
) -> Result<Option<Value>> {
    let mut keys = BTreeSet::new();
    for (_, _, data) in raw {
        // Raw bytes remain authoritative fingerprint input. Legacy fixtures or
        // malformed non-response data without an actual lookup retain their
        // old byte semantics; the canonical queries loader validates responses.
        let Ok(response) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        if response
            .get("requestedModel")
            .is_some_and(|model| !model.is_null())
        {
            continue;
        }
        if let (Some(source), Some(turn)) =
            (response["sourceId"].as_str(), response["turnId"].as_str())
        {
            keys.insert((source.to_owned(), turn.to_owned()));
        }
    }
    if keys.is_empty() {
        return Ok(None);
    }
    let wanted = keys
        .iter()
        .map(|(source, id)| format!("{source}|{id}"))
        .collect::<BTreeSet<_>>();
    let mut models = BTreeMap::<String, Option<String>>::new();
    // Exactly the queries.rs turn_map traversal: no time/source predicate or
    // alternate ordering, and the last JSON sourceId|id collision wins. Keep
    // only its effective model string, including an empty string if present.
    let mut query = db.prepare("SELECT data FROM turns").map_err(err)?;
    for row in query
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(err)?
    {
        let turn: Value = serde_json::from_str(&row.map_err(err)?).map_err(err)?;
        let source = turn["sourceId"].as_str().unwrap_or("");
        let id = turn["id"].as_str().unwrap_or("");
        let key = format!("{source}|{id}");
        if wanted.contains(&key) {
            models.insert(key, turn["model"].as_str().map(str::to_owned));
        }
    }
    let evidence = keys
        .into_iter()
        .map(|(source, id)| {
            let key = format!("{source}|{id}");
            json!({"sourceId":source,"id":id,"exists":models.contains_key(&key),"model":models.get(&key).cloned().flatten()})
        })
        .collect::<Vec<_>>();
    Ok(Some(json!({
        "namespace":"requested-model-turn-fallback-v1",
        "turns":evidence
    })))
}

fn ledger_fingerprint_inner(
    db: &Connection,
    source: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    fresh_ids: Option<&BTreeSet<String>>,
) -> Result<Option<String>> {
    // SQLite's date parser rounds sub-millisecond timestamps. Fetch a padded
    // range and select exact half-open boundaries with chrono instead.
    let broad="SELECT id FROM usage_responses WHERE source_id=?1 AND julianday(event_time)>=julianday(?2)-1.0/86400.0 AND julianday(event_time)<julianday(?3)+1.0/86400.0";
    let mut facts = db
        .prepare(&format!(
            "SELECT id,event_time,data FROM usage_responses WHERE id IN ({broad}) ORDER BY id"
        ))
        .map_err(err)?;
    let rows = facts
        .query_map(params![source, timestamp(start), timestamp(end)], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(err)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(err)?;
    let mut raw: Vec<_> = rows
        .into_iter()
        .filter(|(_, time, _)| instant(&json!(time)).is_some_and(|at| at >= start && at < end))
        .collect();
    let ids: BTreeSet<_> = raw.iter().map(|(id, _, _)| id.as_str()).collect();
    let mut excluded=db.prepare(&format!("SELECT fact_id,reason FROM excluded_facts WHERE fact_id IN ({broad}) ORDER BY fact_id,reason")).map_err(err)?;
    let exclusions = excluded
        .query_map(params![source, timestamp(start), timestamp(end)], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(err)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(err)?
        .into_iter()
        .filter(|(id, _)| ids.contains(id.as_str()))
        .collect::<Vec<_>>();
    let mut aliases=db.prepare(&format!("SELECT alias,fact_id,evidence FROM fact_aliases WHERE alias IN ({broad}) OR fact_id IN ({broad}) ORDER BY alias")).map_err(err)?;
    let alias_rows = aliases
        .query_map(params![source, timestamp(start), timestamp(end)], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(err)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(err)?
        .into_iter()
        .filter(|(alias, id, _)| ids.contains(alias.as_str()) || ids.contains(id.as_str()))
        .collect::<Vec<_>>();
    if let Some(fresh_ids) = fresh_ids {
        if exclusions.iter().any(|(id, _)| fresh_ids.contains(id))
            || alias_rows
                .iter()
                .any(|(alias, id, _)| fresh_ids.contains(alias) || fresh_ids.contains(id))
        {
            return Ok(None);
        }
        // Drop the borrowed ID index before mutating raw. The metadata above
        // intentionally reflects the complete exact interval before subtraction.
        drop(ids);
        raw.retain(|(id, _, _)| !fresh_ids.contains(id));
    }
    let turn_evidence = requested_model_turn_evidence(db, &raw)?;
    let mut material = json!([
        VERSION,
        source,
        timestamp(start),
        timestamp(end),
        raw,
        exclusions,
        alias_rows
    ]);
    // Leave the original seven-element byte contract untouched when the
    // normalized queries path does not need any turn-model fallback.
    if let Some(evidence) = turn_evidence {
        material.as_array_mut().unwrap().push(evidence);
    }
    Ok(Some(hash(material.to_string().as_bytes())))
}
fn config_matches(c: &Capture, plan: &Value) -> bool {
    let Some(key) = c.key.as_ref() else {
        return false;
    };
    text(&plan["id"]) == Some(c.plan_id.as_str())
        && text(&plan["sourceId"]) == Some(key.source_id.as_str())
        && model_scope(plan).ok() == Some((key.model_dimension.clone(), key.model.clone()))
        && instant(&plan["effectiveFrom"]) == Some(c.starts_at)
        && instant(&plan["endsAt"]) == c.configured_end
        && plan["endPolicy"].as_str() == Some(c.end_policy.as_str())
        && plan["type"] == "tokens"
}

pub(crate) fn initialize(
    db: &Connection,
    plan: &Value,
    snapshot: Option<&Value>,
    runtime: &str,
    at: &Reading,
) -> Result<()> {
    if db.is_autocommit() {
        return Err("需求历史必须在计划事务内初始化".into());
    }
    if plan["type"] != "tokens" {
        return Ok(());
    }
    let id = text(&plan["id"]).ok_or("计划ID缺失")?;
    let original = state(db, id)?;
    if original.get("demandHistory").is_some() {
        return Ok(());
    }
    let starts = instant(&plan["effectiveFrom"]).ok_or("计划起点缺失")?;
    let mut capture = Capture {
        schema_version: 1,
        algorithm_version: VERSION.into(),
        plan_id: id.into(),
        key: None,
        runtime_scope: None,
        entitlement_evidence: None,
        starts_at: starts,
        configured_end: instant(&plan["endsAt"]),
        end_policy: plan["endPolicy"].as_str().unwrap_or("").into(),
        last_utc: at.utc,
        last_mono_ms: at.mono_ms.to_string(),
        last_used: None,
        status: "collecting".into(),
        actual_end: None,
        amount: None,
        ledger_fingerprint: None,
        reasons: Vec::new(),
    };
    let decision = (|| -> std::result::Result<(), &'static str> {
        if starts > at.utc || (at.utc - starts).num_milliseconds() > 1000 || at.mono_ms == u64::MAX
        {
            return Err("initial_clock_unknown");
        }
        if !matches!(capture.end_policy.as_str(), "time" | "window")
            || capture.configured_end.is_none_or(|end| end <= starts)
        {
            return Err("actual_period_unknown");
        }
        let source = text(&plan["sourceId"]).ok_or("source_unknown")?;
        let (dimension, model) = model_scope(plan)?;
        let (runtime_scope, account, segment, rule, evidence) = scope(
            snapshot.ok_or("identity_unproven")?,
            source,
            runtime,
            at.utc,
        )?;
        capture.runtime_scope = Some(runtime_scope);
        capture.entitlement_evidence = Some(evidence);
        capture.key = Some(Key {
            source_id: source.into(),
            model_dimension: dimension,
            model,
            account_scope: account,
            entitlement_segment: segment,
            rule_version: rule,
            metering_version: adapter::SEMANTICS.into(),
        });
        Ok(())
    })();
    if let Err(reason) = decision {
        invalidate(&mut capture, reason);
    }
    save(db, id, original, &capture)
}

pub(crate) fn observe(
    db: &Connection,
    plan: &Value,
    view: &Value,
    snapshot: Option<&Value>,
    runtime: &str,
    at: &Reading,
    reliable_scan: bool,
) -> Result<()> {
    if db.is_autocommit() {
        return Err("需求历史必须在真实扫描事务内维护".into());
    }
    let Some(id) = text(&plan["id"]) else {
        return Ok(());
    };
    let original = state(db, id)?;
    let Some(raw) = original.get("demandHistory") else {
        return Ok(());
    };
    let mut capture: Capture = serde_json::from_value(raw.clone()).map_err(err)?;
    if capture.schema_version != 1 || capture.algorithm_version != VERSION {
        return Err("需求历史状态版本不受支持".into());
    }
    if capture.status == "invalidated" {
        return Ok(());
    }
    if !config_matches(&capture, plan) {
        invalidate(&mut capture, "plan_scope_changed");
        return save(db, id, original, &capture);
    }
    let used = amount(&view["used"]);
    if capture.status == "complete" {
        let unchanged = capture
            .key
            .as_ref()
            .zip(capture.actual_end)
            .and_then(|(key, end)| {
                ledger_fingerprint(db, &key.source_id, capture.starts_at, end).ok()
            })
            .is_some_and(|digest| Some(digest) == capture.ledger_fingerprint);
        // Later scans may be incomplete for reasons outside this closed period.
        // Its sealed facts and actual boundary, rather than today's global
        // collection status, decide whether the historical sample still exists.
        if actual_end(plan, view) != capture.actual_end || !unchanged {
            invalidate(&mut capture, "historical_evidence_changed");
            return save(db, id, original, &capture);
        }
        return Ok(());
    }
    let decision = (|| -> std::result::Result<(), &'static str> {
        if !reliable_scan {
            return Err("scan_coverage_incomplete");
        }
        let previous_mono = capture
            .last_mono_ms
            .parse::<u64>()
            .map_err(|_| "paired_clock_discontinuous")?;
        let mono = at
            .mono_ms
            .checked_sub(previous_mono)
            .ok_or("paired_clock_discontinuous")?;
        let wall = (at.utc - capture.last_utc).num_milliseconds();
        if at.mono_ms == u64::MAX
            || mono > MAX_GAP_MS
            || wall < 0
            || (i128::from(wall) - i128::from(mono)).abs() > MAX_CLOCK_SKEW_MS
        {
            return Err("paired_clock_discontinuous");
        }
        let key = capture.key.as_ref().ok_or("scope_unknown")?;
        let (runtime_scope, account, segment, rule, _) = scope(
            snapshot.ok_or("identity_unproven")?,
            &key.source_id,
            runtime,
            at.utc,
        )?;
        if capture.runtime_scope.as_ref() != Some(&runtime_scope)
            || account != key.account_scope
            || segment != key.entitlement_segment
            || rule != key.rule_version
        {
            return Err("entitlement_or_identity_changed");
        }
        let evidence = &view["demandEvidence"];
        if evidence["meteringVersion"].as_str() != Some(key.metering_version.as_str())
            || evidence["ledgerComplete"] != true
        {
            return Err("metering_evidence_unknown");
        }
        if evidence["accountScope"].as_str() != Some(key.account_scope.as_str())
            || evidence["accountCoverageVerified"] != true
        {
            return Err("account_usage_attribution_unknown");
        }
        if key.model.is_some() && evidence["modelCoverageComplete"] != true {
            return Err("model_coverage_unknown");
        }
        if used
            .zip(capture.last_used)
            .is_some_and(|(next, previous)| next < previous)
        {
            return Err("historical_evidence_changed");
        }
        let reasons = view["reasons"].as_array().ok_or("view_coverage_unknown")?;
        let pending_window = view["evaluationState"] == "pending"
            && capture.end_policy == "window"
            && !reasons.is_empty()
            && reasons
                .iter()
                .all(|reason| reason == "window_confirmation_pending");
        if !pending_window
            && (view["evaluationState"] != "ready" || !reasons.is_empty() || used.is_none())
        {
            return Err("view_coverage_unknown");
        }
        if view["lifecycle"] == "ended" {
            let end = actual_end(plan, view).ok_or("actual_period_unknown")?;
            if end <= capture.starts_at || end > at.utc || pending_window || used.is_none() {
                return Err("actual_period_unknown");
            }
            if instant(&view["measuredUntil"]) != Some(end) {
                return Err("actual_period_coverage_unknown");
            }
            let digest = ledger_fingerprint(db, &key.source_id, capture.starts_at, end)
                .map_err(|_| "ledger_evidence_unavailable")?;
            capture.actual_end = Some(end);
            capture.amount = used;
            capture.ledger_fingerprint = Some(digest);
            capture.status = "complete".into();
        }
        Ok(())
    })();
    if let Err(reason) = decision {
        invalidate(&mut capture, reason);
    }
    capture.last_utc = at.utc;
    capture.last_mono_ms = at.mono_ms.to_string();
    capture.last_used = used.or(capture.last_used);
    save(db, id, original, &capture)
}

fn setting(db: &Connection, key: &str) -> Result<Option<Value>> {
    let value: Option<String> = db
        .query_row("SELECT value FROM settings WHERE key=?1", [key], |r| {
            r.get(0)
        })
        .optional()
        .map_err(err)?;
    value
        .map(|v| serde_json::from_str(&v).map_err(err))
        .transpose()
}
fn failure(reasons: Vec<String>) -> Value {
    json!({"plan":null,"basis":["完整可比的本机历史需求"],"constraintsVerified":false,"reasons":reasons,"algorithmVersion":VERSION})
}

pub(crate) fn draft(
    db: &Connection,
    input: &Value,
    current: Option<&Value>,
    runtime: &str,
    now: DateTime<Utc>,
) -> Value {
    let result = (|| -> Result<Value> {
        let Some(source) = text(&input["sourceId"]) else {
            return Ok(failure(vec!["source_unknown".into()]));
        };
        let (dimension, model) = match model_scope(input) {
            Ok(value) => value,
            Err(reason) => return Ok(failure(vec![reason.into()])),
        };
        let (_, account, segment, rule, _) = match current
            .ok_or("identity_unproven")
            .and_then(|s| scope(s, source, runtime, now))
        {
            Ok(value) => value,
            Err(reason) => return Ok(failure(vec![reason.into()])),
        };
        let key = Key {
            source_id: source.into(),
            model_dimension: dimension.clone(),
            model: model.clone(),
            account_scope: account,
            entitlement_segment: segment,
            rule_version: rule,
            metering_version: adapter::SEMANTICS.into(),
        };
        let preferences = setting(db, "preferences")?;
        let retention = preferences
            .as_ref()
            .and_then(|v| v["retentionDays"].as_i64())
            .unwrap_or(90)
            .clamp(1, 90);
        let mut cutoff = now
            .checked_sub_signed(Duration::days(retention))
            .ok_or("历史范围无效")?;
        for name in ["importSince", "clearWatermark", "retentionWatermark"] {
            if let Some(value) = setting(db, name)? {
                let Some(at) = instant(&value) else {
                    return Ok(failure(vec!["history_retention_unknown".into()]));
                };
                cutoff = cutoff.max(at);
            }
        }
        let mut query=db.prepare("SELECT s.data FROM plan_period_state s WHERE s.owner_kind='plan' AND EXISTS(SELECT 1 FROM usage_plans p WHERE p.plan_id=s.owner_id) AND json_type(s.data,'$.demandHistory')='object'").map_err(err)?;
        let rows = query
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(err)?;
        let mut candidates = Vec::new();
        for row in rows {
            let state: Value = serde_json::from_str(&row.map_err(err)?).map_err(err)?;
            let c: Capture = serde_json::from_value(state["demandHistory"].clone()).map_err(err)?;
            if c.schema_version == 1
                && c.algorithm_version == VERSION
                && c.status == "complete"
                && c.key.as_ref() == Some(&key)
                && c.starts_at >= cutoff
                && c.actual_end
                    .is_some_and(|end| end > c.starts_at && end <= now)
                && c.amount.is_some_and(|n| {
                    n.is_finite() && n >= 0.0 && n <= MAX_SAFE_INTEGER && n.fract() == 0.0
                })
                && c.ledger_fingerprint.as_ref().is_some_and(|old| {
                    ledger_fingerprint(db, source, c.starts_at, c.actual_end.unwrap())
                        .ok()
                        .as_ref()
                        == Some(old)
                })
            {
                candidates.push(c);
            }
        }
        candidates.sort_by_key(|c| std::cmp::Reverse(c.actual_end));
        let explicit_end = if input["endsAt"].is_null() {
            None
        } else {
            Some(instant(&input["endsAt"]).ok_or("草案结束时刻无效")?)
        };
        let explicit_ms = if input["durationSeconds"].is_null() {
            None
        } else {
            let seconds = input["durationSeconds"]
                .as_f64()
                .filter(|v| v.is_finite() && *v > 0.0 && *v <= 90.0 * 86400.0)
                .ok_or("草案时长无效")?;
            Some((seconds * 1000.0).round() as i64)
        };
        let duration_ms = explicit_end
            .map(|end| (end - now).num_milliseconds())
            .or(explicit_ms)
            .or_else(|| {
                candidates
                    .first()
                    .map(|c| (c.actual_end.unwrap() - c.starts_at).num_milliseconds())
            });
        let Some(duration_ms) = duration_ms.filter(|ms| *ms > 0 && *ms <= 90 * 86_400_000) else {
            return Ok(failure(vec![
                "comparable_covered_intervals_insufficient".into()
            ]));
        };
        if explicit_end.is_some() && explicit_ms.is_some_and(|ms| (ms - duration_ms).abs() > 1) {
            return Ok(failure(vec!["duration_mismatch".into()]));
        }
        // Reverse interval scheduling maximizes the number of independent
        // intervals. Sorting by latest end instead can discard two short
        // admissible intervals for one slightly longer overlapping interval.
        candidates.sort_by_key(|c| std::cmp::Reverse(c.starts_at));
        let mut accepted: Vec<&Capture> = Vec::new();
        for c in &candidates {
            let end = c.actual_end.unwrap();
            let duration = (end - c.starts_at).num_milliseconds();
            if ((duration as f64 / duration_ms as f64) - 1.0).abs() > DURATION_TOLERANCE {
                continue;
            }
            if accepted
                .iter()
                .any(|old| c.starts_at < old.actual_end.unwrap() && old.starts_at < end)
            {
                continue;
            }
            accepted.push(c);
        }
        let history: Vec<_> = accepted
            .iter()
            .map(|c| DemandInterval {
                amount: c.amount.unwrap(),
                covered: true,
                comparable: true,
            })
            .collect();
        let Some(demand) = demand_draft(&history) else {
            return Ok(failure(vec![
                "comparable_covered_intervals_insufficient".into()
            ]));
        };
        let end = now
            .checked_add_signed(Duration::milliseconds(duration_ms))
            .ok_or("草案结束时刻溢出")?;
        let metrics = json!({"p50":demand.target_p50,"p90":demand.reference_p90,"sampleCount":demand.samples,"durationSeconds":duration_ms as f64/1000.0,"durationToleranceFraction":DURATION_TOLERANCE,"intervalPlanIds":accepted.iter().map(|c|c.plan_id.clone()).collect::<Vec<_>>()});
        if demand.target_p50 == 0.0 {
            let mut value = failure(vec!["zero_historical_demand_choose_manual_target".into()]);
            value["historicalDemand"] = metrics;
            return Ok(value);
        }
        Ok(
            json!({"plan":{"expectedRevision":null,"title":"按历史习惯安排","type":"tokens","sourceId":source,"model":model,"modelDimension":dimension,
            "target":demand.target_p50,"endPolicy":"time","endsAt":timestamp(end),"windowId":null,"remindersEnabled":true},
            "basis":[format!("{} 个互不重叠、完整采集且同权益的历史时段",demand.samples),"按实际时长 ±5% 筛选；不缩放历史 Token、不包含工作日程模型",format!("历史 P50 目标 {} Token，P90 高需求参考 {} Token",demand.target_p50,demand.reference_p90),"这是本机需求建议，厂商额度可持续性未验证；保存后才生效"],
            "constraintsVerified":false,"reasons":["quota_sustainability_unverified"],"historicalDemand":metrics,"algorithmVersion":VERSION}),
        )
    })();
    result.unwrap_or_else(|_| failure(vec!["history_unavailable".into()]))
}

#[cfg(test)]
#[path = "demand_history_tests.rs"]
mod tests;
