use super::super::adapter::ResponseUsage;
use super::super::*;
use crate::budget_clock::{self, Reading};
use std::collections::{BTreeMap, BTreeSet};

#[path = "local_schedules.rs"]
mod local_schedules;
#[path = "mixture_studies.rs"]
mod mixture_studies;

const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;
const MAX_PLANS: i64 = 100;
const MAX_ASSUMPTIONS: usize = 100;
// Opening this requires the separate Windows notification and pet acceptance.
const NATIVE_DELIVERY_QUALIFIED: bool = false;

#[derive(Default)]
pub(crate) struct Runtime {
    context_generation: u64,
    domain: String,
    settled: u64,
    persisted: u64,
    current: Option<Value>,
    observation_discontinuity: bool,
    scan_at: Option<Reading>,
    pub(super) delivery: super::delivery::Delivery,
    official_delivery: super::official_delivery::Delivery,
    #[cfg(test)]
    native_test_qualified: bool,
}
pub(crate) struct Scan {
    at: Reading,
    rowid: i64,
    before: BTreeMap<String, Value>,
    fingerprint: String,
    continuous: bool,
    delivery_sample: Option<super::delivery::Sample>,
    delivery_context: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SavePlan {
    id: Option<String>,
    expected_revision: Option<i64>,
    title: String,
    #[serde(rename = "type")]
    kind: String,
    source_id: String,
    model: Option<String>,
    model_dimension: String,
    target: f64,
    end_policy: String,
    ends_at: Option<String>,
    window_id: Option<String>,
    reminders_enabled: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PlanStatus {
    id: String,
    expected_revision: i64,
    reminders_enabled: Option<bool>,
    end: Option<bool>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AssumptionInput {
    id: Option<String>,
    expected_revision: Option<i64>,
    source_id: String,
    window_id: String,
    model: Option<String>,
    capacity_tokens: f64,
    valid_until: String,
    note: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DraftInput {
    intent: String,
    source_id: String,
    window_id: Option<String>,
    model: Option<String>,
    model_dimension: String,
    reserve_percent: f64,
}
fn time(text: &str) -> Result<chrono::DateTime<Utc>> {
    chrono::DateTime::parse_from_rfc3339(text)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|_| "时间必须包含有效时区".to_string())
}
fn timestamp(at: chrono::DateTime<Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
fn text_ok(text: &str, max: usize) -> bool {
    !text.trim().is_empty() && text.chars().count() <= max && !text.chars().any(char::is_control)
}
fn next_revision(current: i64) -> Result<i64> {
    current
        .checked_add(1)
        .filter(|v| *v <= MAX_SAFE_INTEGER)
        .ok_or_else(|| "修订序号已耗尽".into())
}
fn revision(db: &Connection) -> Result<i64> {
    Ok(get_setting(db, "subscriptionRevision")?
        .and_then(|v| v.as_i64())
        .unwrap_or(0))
}
fn advance_revision(db: &Connection) -> Result<i64> {
    let v = next_revision(revision(db)?)?;
    put_setting(db, "subscriptionRevision", &json!(v))?;
    Ok(v)
}
fn latest_plans(db: &Connection) -> Result<Vec<Value>> {
    let mut s=db.prepare("SELECT p.data FROM usage_plans p JOIN (SELECT plan_id, MAX(revision) revision FROM usage_plans GROUP BY plan_id) c ON c.plan_id=p.plan_id AND c.revision=p.revision ORDER BY p.plan_id").map_err(err)?;
    let rows = s.query_map([], |r| r.get::<_, String>(0)).map_err(err)?;
    rows.map(|s| serde_json::from_str(&s.map_err(err)?).map_err(err))
        .collect()
}
fn current_plan(db: &Connection, id: &str) -> Result<Value> {
    latest_plans(db)?
        .into_iter()
        .find(|p| p["id"] == id)
        .ok_or_else(|| "计划不存在".into())
}
fn active_configuration(db: &Connection, p: &Value, at: chrono::DateTime<Utc>) -> Result<bool> {
    Ok(p["lifecycle"] == "active"
        && !super::window_plans::projection(db, p, at)?.archived
        && (p["endPolicy"] != "time"
            || p["endsAt"]
                .as_str()
                .map(time)
                .transpose()?
                .is_none_or(|end| at < end)))
}
fn save_config(db: &Connection, plan: &Value) -> Result<()> {
    db.execute(
        "INSERT INTO usage_plans(plan_id,revision,source_id,data) VALUES(?1,?2,?3,?4)",
        params![
            strv(plan, "id"),
            plan["revision"].as_i64().ok_or("计划修订缺失")?,
            strv(plan, "sourceId"),
            plan.to_string()
        ],
    )
    .map_err(err)?;
    Ok(())
}
fn state(db: &Connection, id: &str) -> Result<Value> {
    let text: Option<String>=db.query_row("SELECT data FROM plan_period_state WHERE owner_kind='plan' AND owner_id=?1 AND period_instance_id=?1",[id],|r|r.get(0)).optional().map_err(err)?;
    text.map(|v| serde_json::from_str(&v).map_err(err))
        .transpose()
        .map(|v| v.unwrap_or(json!({"latched":[],"runtimeId":null})))
}
fn save_state(db: &Connection, id: &str, value: &Value) -> Result<()> {
    db.execute("INSERT INTO plan_period_state(owner_kind,owner_id,period_instance_id,data) VALUES('plan',?1,?1,?2) ON CONFLICT(owner_kind,owner_id,period_instance_id) DO UPDATE SET data=excluded.data",params![id,value.to_string()]).map_err(err)?;
    Ok(())
}
fn source_exists(core: &Core, id: &str) -> Result<()> {
    let exists: bool = core
        .db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM source_instances WHERE id=?1 AND enabled=1)",
            [id],
            |r| r.get(0),
        )
        .map_err(err)?;
    if !exists {
        return Err("来源不存在或已停用".into());
    }
    Ok(())
}
fn observation_history(db: &Connection, source: &str) -> Result<Vec<Value>> {
    let mut statement=db.prepare("SELECT data FROM account_snapshots WHERE json_extract(data,'$.documentKind')='subscription_observation' AND json_extract(data,'$.sourceId')=?1 ORDER BY id LIMIT 100000").map_err(err)?;
    let rows = statement
        .query_map([source], |r| r.get::<_, String>(0))
        .map_err(err)?;
    rows.map(|r| serde_json::from_str(&r.map_err(err)?).map_err(err))
        .collect()
}
fn plan_configuration_fingerprint(core: &Core) -> Result<String> {
    let mut digest = Sha256::new();
    let mut s = core
        .db
        .prepare("SELECT id,root,enabled,client_kind,environment FROM source_instances ORDER BY id")
        .map_err(err)?;
    for row in s
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, bool>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })
        .map_err(err)?
    {
        digest.update(serde_json::to_vec(&row.map_err(err)?).map_err(err)?);
    }
    for key in [
        "primarySourceId",
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
    for (table, key) in [("excluded_facts", "fact_id"), ("fact_aliases", "alias")] {
        let mut s = core
            .db
            .prepare(&format!("SELECT {key} FROM {table} ORDER BY {key}"))
            .map_err(err)?;
        for row in s.query_map([], |r| r.get::<_, String>(0)).map_err(err)? {
            digest.update(row.map_err(err)?.as_bytes());
        }
    }
    Ok(format!("{:x}", digest.finalize()))
}
fn model_of<'a>(r: &'a ResponseUsage, dimension: &str) -> Option<&'a str> {
    if dimension == "response" {
        r.observed_model.as_deref()
    } else {
        r.requested_model.as_deref()
    }
}
fn matches_plan(
    r: &ResponseUsage,
    p: &Value,
    start: chrono::DateTime<Utc>,
    end: chrono::DateTime<Utc>,
) -> bool {
    let Ok(at) = time(&r.event_time) else {
        return false;
    };
    r.source_id == strv(p, "sourceId")
        && at >= start
        && at < end
        && p["model"]
            .as_str()
            .is_none_or(|m| model_of(r, &strv(p, "modelDimension")) == Some(m))
}
// Only the current writer can attest that an old head changed solely because
// canonical live facts were newly inserted during this committed scan. The
// forecast module separately validates scope, clocks, proof binding and minima.
fn fresh_local_append_proof(
    core: &Core,
    plan: &Value,
    view: &Value,
    responses: &[ResponseUsage],
    inserted: &BTreeSet<String>,
    scan: &Scan,
    at: &Reading,
    reliable: bool,
) -> Result<Option<super::local_plan_forecast::FreshAppendProof>> {
    if !reliable || plan["type"] != "tokens" || view["evaluationState"] != "ready" {
        return Ok(None);
    }
    let original = state(&core.db, &strv(plan, "id"))?;
    let head = &original["localForecast"]["head"];
    let (Some(old_utc), Some(old_digest), Some(new_utc), Some(new_digest)) = (
        head["utc"].as_str().and_then(|v| time(v).ok()),
        head["ledgerFingerprint"].as_str(),
        view["measuredUntil"].as_str().and_then(|v| time(v).ok()),
        view["forecastLedgerFingerprint"].as_str(),
    ) else {
        return Ok(None);
    };
    if view["forecastLedgerHeadFingerprint"].as_str() == Some(old_digest) {
        return Ok(None);
    }
    let start = time(plan["effectiveFrom"].as_str().ok_or("计划起点缺失")?)?;
    if old_utc < start || old_utc > new_utc {
        return Ok(None);
    }
    let source = strv(plan, "sourceId");
    let mut fresh = BTreeSet::new();
    let mut truncate_from = None;
    for fact in responses {
        let id = adapter::response_key(fact);
        let (Ok(event), Ok(collected)) = (time(&fact.event_time), time(&fact.collected_at)) else {
            continue;
        };
        // Use the actual finish reading, never a rounded head or scan start, as
        // the 15-second lateness boundary. Half-open old-head semantics matter.
        if fact.source_id != source
            || !inserted.contains(&id)
            || !fact.live
            || fact
                .counts
                .total
                .value()
                .is_none_or(|n| !(0..=MAX_SAFE_INTEGER).contains(&n))
            || event < start
            || event >= old_utc
            || event > at.utc
            || at.utc - event > Duration::seconds(15)
            || collected < scan.at.utc
            || collected > at.utc
        {
            continue;
        }
        fresh.insert(id);
        truncate_from = Some(truncate_from.map_or(event, |v: chrono::DateTime<Utc>| v.min(event)));
    }
    let Some(cutoff) = truncate_from else {
        return Ok(None);
    };
    // This helper retains all existing metadata in the digest and rejects any
    // alias/exclusion connected to a purported fresh fact instead of hiding it.
    let prior = super::demand_history::ledger_fingerprint_excluding_fresh(
        &core.db, &source, start, old_utc, &fresh,
    )?;
    if prior.as_deref() != Some(old_digest) {
        return Ok(None);
    }
    Ok(super::local_plan_forecast::FreshAppendProof::verified(
        plan,
        &core.runtime_id,
        old_utc,
        old_digest,
        new_utc,
        new_digest,
        cutoff,
        view["ledgerRevision"].as_u64().ok_or("账本修订缺失")?,
        at,
    ))
}
fn coverage_reasons(
    core: &Core,
    p: &Value,
    start: chrono::DateTime<Utc>,
    end: chrono::DateTime<Utc>,
) -> Result<Vec<String>> {
    let mut reasons = Vec::new();
    let source = strv(p, "sourceId");
    let enabled: bool = core
        .db
        .query_row(
            "SELECT enabled FROM source_instances WHERE id=?1",
            [&source],
            |r| r.get(0),
        )
        .optional()
        .map_err(err)?
        .unwrap_or(false);
    if !enabled {
        reasons.push("source_unavailable".into());
    }
    if p["origin"]["kind"] == "local_schedule" {
        let configuration_revision: Option<i64> = core.db.query_row(
            "SELECT configuration_revision FROM source_instances WHERE id=?1",[&source],|row|row.get(0)).optional().map_err(err)?;
        if configuration_revision.map(|value|value.to_string()).as_deref() != p["origin"]["sourceConfigurationRevision"].as_str() {
            reasons.push("source_binding_changed".into());
        }
    }
    let check = get_setting(&core.db, &format!("lastSourceCheck:{source}"))?.unwrap_or(Value::Null);
    if check["discoveryComplete"] != true
        || check["cancelled"] == true
        || check["issues"].as_u64() != Some(0)
        || check["discoveryErrors"].as_u64() != Some(0)
        || check["pendingFiles"].as_u64() != Some(0)
    {
        reasons.push("collection_incomplete".into());
    }
    let excluded:bool=core.db.query_row("SELECT EXISTS(SELECT 1 FROM usage_responses u JOIN excluded_facts e ON e.fact_id=u.id WHERE u.source_id=?1 AND julianday(u.event_time)>=julianday(?2) AND julianday(u.event_time)<julianday(?3))",params![source,timestamp(start),timestamp(end)],|r|r.get(0)).map_err(err)?;
    if excluded {
        reasons.push("usage_conflict_excluded".into());
    }
    // 最近一次扫描无新诊断不能证明历史计量完整；未知/无效事实仍可能被查询排除。
    let incomplete_metering: bool = core.db.query_row(
        "SELECT EXISTS(SELECT 1 FROM usage_responses WHERE source_id=?1 AND julianday(event_time)>=julianday(?2) AND julianday(event_time)<julianday(?3) AND COALESCE(json_extract(data,'$.counts.total.state'),'missing')<>'valid')",
        params![source,timestamp(start),timestamp(end)],|row|row.get(0)).map_err(err)?;
    if incomplete_metering {
        reasons.push("metering_incomplete".into());
    }
    for key in ["importSince", "clearWatermark", "retentionWatermark"] {
        if let Some(value) = get_setting(&core.db, key)? {
            match value.as_str().map(time).transpose() {
                Ok(Some(t)) if t > start => reasons.push("evidence_removed".into()),
                Err(_) => reasons.push("coverage_unknown".into()),
                _ => {}
            }
        }
    }
    Ok(reasons)
}
fn plan_view(
    core: &Core,
    p: &Value,
    responses: &[ResponseUsage],
    at: chrono::DateTime<Utc>,
) -> Result<Value> {
    let mut v = p.clone();
    let window_plan = super::window_plans::projection(&core.db, p, at)?;
    let start = time(p["effectiveFrom"].as_str().ok_or("计划起点缺失")?)?;
    let configured_end = if p["endPolicy"] == "window" {
        window_plan
            .resolved_end
            .or(p["endsAt"].as_str().map(time).transpose()?)
    } else {
        p["endsAt"].as_str().map(time).transpose()?
    };
    let manually_ended = p["endedAt"].as_str().map(time).transpose()?;
    let ends = match (configured_end, manually_ended) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    // Persisted heads use millisecond timestamps. Select the same exact
    // half-open interval for counts, fingerprints and measuredUntil so a
    // sub-millisecond fact cannot enter a head whose saved boundary excludes it.
    let end = time(&timestamp(ends.unwrap_or(at).min(at)))?;
    let mut reasons = coverage_reasons(core, p, start, end)?;
    let mut used = None;
    let current_scope=core.subscription.current.as_ref().map(|s|json!({"runtimeId":s["runtimeId"],"sourceId":s["sourceId"],"bindingGeneration":s["bindingGeneration"],"identityEpoch":s["identityEpoch"],"clearGeneration":s["clearGeneration"],"accountScope":s["accountScope"]}));
    let current_window = windows(core, at)?
        .into_iter()
        .find(|w| w["id"] == p["windowId"] && w["periodInstanceId"] == p["periodInstanceId"]);
    let window_ready = window_plan.same_window
        && window_plan.proof_scope == current_scope
        && current_window
            .as_ref()
            .is_some_and(|w| w["canBindPlan"] == true);
    if p["type"] == "tokens" {
        if p["model"].as_str().is_some()
            && responses.iter().any(|r| {
                r.source_id == strv(p, "sourceId")
                    && time(&r.event_time).is_ok_and(|t| t >= start && t < end)
                    && model_of(r, &strv(p, "modelDimension")).is_none()
            })
        {
            reasons.push("model_coverage_incomplete".into());
        }
        if p["endPolicy"] == "window" {
            if window_plan.archived && window_plan.partial {
                reasons.push("window_ended_partial".into());
            } else if !window_plan.archived && !window_ready {
                reasons.push("window_confirmation_pending".into());
            }
        }
        if reasons.is_empty() {
            let total = responses
                .iter()
                .filter(|r| matches_plan(r, p, start, end))
                .try_fold(0i64, |sum, r| {
                    sum.checked_add(r.counts.total.value().unwrap_or(0))
                        .filter(|v| *v <= MAX_SAFE_INTEGER)
                        .ok_or("Token 总量超出安全范围")
                })?;
            used = Some(total as f64);
        }
    } else if window_plan.archived {
        reasons.clear();
        used = window_plan.final_percent;
        if window_plan.partial || used.is_none() {
            used = None;
            reasons.push("window_ended_partial".into());
        }
    } else if let Some(w) = current_window {
        reasons.clear();
        if window_ready {
            used = w["usedPercent"].as_f64();
        } else {
            reasons.push("window_confirmation_pending".into());
        }
    } else {
        reasons = vec!["window_confirmation_pending".into()];
    }
    let ended = p["lifecycle"] == "ended"
        || window_plan.archived
        || (p["endPolicy"] == "time" && ends.is_some_and(|t| at >= t));
    if p["endPolicy"] == "window" {
        v["endsAt"] = json!(if window_plan.archived && window_plan.partial {
            None
        } else {
            configured_end.map(timestamp)
        });
    }
    v["lifecycle"] = json!(if ended { "ended" } else { "active" });
    v["evaluationState"] = json!(if used.is_some() {
        "ready"
    } else if reasons.iter().any(|r| r == "window_confirmation_pending") {
        "pending"
    } else {
        "unknown"
    });
    v["used"] = json!(used);
    let target = p["target"].as_f64().ok_or("计划目标缺失")?;
    v["remaining"] = json!(used.map(|u| (target - u).max(0.0)));
    v["progressPercent"] = json!(used.map(|u| 100.0 * u / target));
    v["reasons"] = json!(reasons);
    v["coverage"] = json!(if used.is_some() {
        "recorded-local"
    } else {
        "unknown"
    });
    let relevant: Vec<_> = responses
        .iter()
        .filter(|r| {
            r.source_id == strv(p, "sourceId")
                && time(&r.event_time).is_ok_and(|t| t >= start && t < end)
        })
        .collect();
    let versions: BTreeSet<_> = relevant
        .iter()
        .map(|r| r.semantics_version.clone())
        .collect();
    let registered_kind: Option<String> = core
        .db
        .query_row(
            "SELECT client_kind FROM source_instances WHERE id=?1",
            [strv(p, "sourceId")],
            |r| r.get(0),
        )
        .optional()
        .map_err(err)?;
    let metering = if versions.len() == 1 {
        versions.first().cloned()
    } else if versions.is_empty() && registered_kind.as_deref() == Some("codex") {
        Some(adapter::SEMANTICS.to_string())
    } else {
        None
    };
    let snapshot = core.subscription.current.as_ref();
    let account = snapshot
        .and_then(|s| s["accountScope"].as_str())
        .filter(|s| !s.is_empty());
    let scope_proven = snapshot.is_some_and(|s| {
        identity_proven(s)
            && s["sourceId"] == p["sourceId"]
            && s["status"] == "connected"
            && s["persisted"] == true
            && s["observationOutcome"] == "success"
            && s["quotaRead"]["freshnessAnchor"]
                .as_str()
                .and_then(|v| time(v).ok())
                .is_some_and(|t| t <= at && (at - t).num_seconds() < 240)
    });
    let account_covered = scope_proven
        && account.is_some()
        && relevant
            .iter()
            .all(|r| r.account_scope_id.as_deref() == account);
    v["measuredUntil"] = json!(timestamp(end));
    v["demandEvidence"] = json!({"meteringVersion":metering,"ledgerComplete":reasons.is_empty() && metering.is_some(),"accountScope":account,"accountCoverageVerified":account_covered,"modelCoverageComplete":p["model"].is_null() || relevant.iter().all(|r|model_of(r,&strv(p,"modelDimension")).is_some())});
    v["notifyEligible"] = json!(false);
    v["ledgerRevision"] = json!(revision(&core.db)?);
    if p["type"] == "tokens" && used.is_some() {
        v["forecastLedgerFingerprint"] = json!(super::demand_history::ledger_fingerprint(
            &core.db,
            &strv(p, "sourceId"),
            start,
            end,
        )?);
        let previous_head = state(&core.db, &strv(p, "id"))?["localForecast"]["head"]["utc"]
            .as_str()
            .map(time)
            .transpose()?;
        if let Some(head) = previous_head.filter(|head| *head >= start && *head <= end) {
            v["forecastLedgerHeadFingerprint"] = json!(super::demand_history::ledger_fingerprint(
                &core.db,
                &strv(p, "sourceId"),
                start,
                head,
            )?);
        }
    }
    v["forecasts"] = super::local_plan_forecast::projection(
        &core.db,
        p,
        &v,
        &core.runtime_id,
        &budget_clock::now(),
    )?;
    Ok(v)
}
fn decimal(value: &Value) -> Option<u64> {
    let s = value.as_str()?;
    if s.is_empty() || (s.len() > 1 && s.starts_with('0')) || !s.bytes().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    s.parse().ok()
}
fn identity_proven(snapshot: &Value) -> bool {
    snapshot["identityConsistency"] == "proven" && snapshot["identityEvidence"].is_object()
}
// A saved current observation proves its settled interval only. Revalidate
// structure against today's canonical ledger before a manual or draft consumer
// uses it; do not rewrite the receipt or renew the observation in a read.
fn structure_checked_snapshot(core: &Core, responses: &[ResponseUsage]) -> Option<Value> {
    let mut snapshot = core.subscription.current.clone()?;
    let mut invalid = BTreeSet::new();
    for bucket in snapshot["buckets"].as_array().into_iter().flatten() {
        for kind in ["primary", "secondary"] {
            let id = format!("{}:{kind}", strv(bucket, "id"));
            if bucket[kind].is_object()
                && !super::calibration_assembler::usage_structure_current(&snapshot, &id, responses)
            {
                invalid.insert(id);
            }
        }
    }
    for bucket in snapshot
        .get_mut("buckets")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        let pool = strv(bucket, "id");
        for kind in ["primary", "secondary"] {
            if invalid.contains(&format!("{pool}:{kind}")) {
                if let Some(window) = bucket.get_mut(kind).and_then(Value::as_object_mut) {
                    window.remove("usageStructureEvidence");
                }
            }
        }
    }
    Some(snapshot)
}
fn windows(core: &Core, at: chrono::DateTime<Utc>) -> Result<Vec<Value>> {
    let Some(snapshot) = core.subscription.current.as_ref() else {
        return Ok(Vec::new());
    };
    let fresh = snapshot["observationOutcome"] == "success"
        && snapshot["persisted"] == true
        && snapshot["status"] == "connected";
    let mut out = Vec::new();
    for bucket in snapshot["buckets"].as_array().into_iter().flatten() {
        for kind in ["primary", "secondary"] {
            let w = &bucket[kind];
            if !w.is_object() {
                continue;
            }
            let used = w["usedPercent"]
                .as_f64()
                .filter(|v| v.is_finite() && (0.0..=100.0).contains(v));
            let reset = w["resetsAt"]
                .as_i64()
                .and_then(|n| chrono::DateTime::from_timestamp(n, 0));
            let anchor = snapshot["quotaRead"]["freshnessAnchor"]
                .as_str()
                .or(snapshot["collectedAt"].as_str())
                .map(time)
                .transpose()?;
            let current = fresh
                && anchor.is_some_and(|t| t <= at && (at - t).num_seconds() < 240)
                && reset.is_none_or(|t| at < t);
            let proven = identity_proven(snapshot);
            let recovery = w["recovery"].as_str().unwrap_or("unknown");
            let window_id = format!("{}:{kind}", strv(bucket, "id"));
            let instance = w["windowInstanceId"].as_str().filter(|s| text_ok(s, 200));
            let period = instance.map(str::to_owned).or_else(|| {
                reset.map(|r| {
                    hash(
                        format!(
                            "{}|{}|{}|{}|{}",
                            core.subscription.domain,
                            snapshot["identityEpoch"],
                            window_id,
                            recovery,
                            r.timestamp()
                        )
                        .as_bytes(),
                    )
                })
            });
            let mut reasons = Vec::new();
            if !proven {
                reasons.push("identity_unproven");
            }
            if !current {
                reasons.push("stale_observation");
            }
            if recovery == "unknown" {
                reasons.push("recovery_unknown");
            }
            if snapshot["quotaRead"]["factTimeLower"].is_null() {
                reasons.push("fact_time_unbounded");
            }
            if w["roundingStep"].is_null() {
                reasons.push("precision_unknown");
            }
            let bind = current
                && proven
                && reset.is_some()
                && instance.is_some()
                && matches!(recovery, "fixed_reset" | "anchored_reset")
                && snapshot["persisted"] == true;
            out.push(json!({"id":window_id,"bucketId":bucket["id"],"bucketName":bucket["name"].as_str().or(bucket["id"].as_str()).unwrap_or("额度池"),"label":if kind=="primary"{"主窗口"}else{"次窗口"},"periodInstanceId":period,"usedPercent":used,"remainingPercent":used.map(|p|100.0-p),"resetsAt":reset.map(timestamp),"resetState":if reset.is_some_and(|r|at>=r){"waiting"}else if recovery=="unknown"{"unknown"}else{"current"},"freshness":if current{"fresh"}else{"stale"},"canBindPlan":bind,"reasons":reasons,"risk":{"state":"unknown","label":"缺少可靠预测依据","exhaustsAtLower":null,"exhaustsAtUpper":null,"notifyEligible":false,"reasons":reasons}}));
        }
    }
    Ok(out)
}
impl Core {
    #[cfg(test)]
    pub(crate) fn qualify_subscription_native_for_test(&mut self) {
        self.subscription.native_test_qualified = true;
        self.notification_gate.publish(true, false);
    }
    fn subscription_native_qualified(&self) -> bool {
        #[cfg(test)]
        if self.subscription.native_test_qualified {
            return true;
        }
        #[cfg(all(debug_assertions, feature = "subscription-native-audit"))]
        if self
            .primary_source()
            .ok()
            .and_then(|s| s["root"].as_str().map(std::path::PathBuf::from))
            .is_some_and(|source| {
                crate::subscription_native_audit::enabled(&self.data_dir, &source)
            })
        {
            return true;
        }
        NATIVE_DELIVERY_QUALIFIED
    }
    fn subscription_delivery_context(&self) -> Result<String> {
        Ok(hash(
            json!([
                self.runtime_id,
                self.subscription.context_generation.to_string(),
                self.primary_source()?,
                get_setting(&self.db, "clearWatermark")?,
                self.settings()?["privacyMode"],
                plan_configuration_fingerprint(self)?,
            ])
            .to_string()
            .as_bytes(),
        ))
    }
    pub(crate) fn subscription_notification_permit(
        &self,
        event: &Value,
    ) -> Option<crate::notification_gate::NotificationPermit> {
        self.subscription_delivery_permit(event, false)
    }
    pub(crate) fn subscription_prompt_is_current(&self, event: &Value) -> bool {
        self.subscription_delivery_permit(event, true).is_some()
    }
    fn subscription_delivery_permit(
        &self,
        event: &Value,
        prompt: bool,
    ) -> Option<crate::notification_gate::NotificationPermit> {
        if matches!(
            event["reminderKind"].as_str(),
            Some("official_target" | "native_quota_eta")
        ) {
            let context = self.subscription_delivery_context().unwrap_or_default();
            return self.subscription.official_delivery.permit(
                &self.db,
                event,
                self.subscription.current.as_ref(),
                &context,
                !context.is_empty() && self.subscription_native_qualified(),
                prompt,
            );
        }
        let context = (|| -> Option<String> {
            if event["kind"] != "subscription_plan" {
                return None;
            }
            let plan = current_plan(&self.db, &strv(event, "planId")).ok()?;
            if plan["endPolicy"] == "window" {
                let responses = queries::subscription_responses(self).ok()?;
                let view = plan_view(self, &plan, &responses, budget_clock::now().utc).ok()?;
                if view["evaluationState"] != "ready" || view["lifecycle"] != "active" {
                    return None;
                }
            }
            self.subscription_delivery_context().ok()
        })();
        let delivery = &self.subscription.delivery;
        let id = strv(event, "id");
        let context = context.as_deref().unwrap_or("");
        let qualified = !context.is_empty() && self.subscription_native_qualified();
        if prompt {
            delivery.prompt_permit(&self.db, &id, &self.runtime_id, context, qualified)
        } else {
            delivery.permit(&self.db, &id, &self.runtime_id, context, qualified)
        }
    }
    pub(crate) fn revoke_subscription_deliveries(&mut self) {
        self.subscription.delivery.revoke_all();
        self.subscription.official_delivery.revoke_all();
    }
    pub(crate) fn finish_subscription_notification(&mut self, id: &str) {
        self.subscription.delivery.finish(id);
        self.subscription.official_delivery.finish(id, None);
    }
    pub(crate) fn settle_subscription_notification(&mut self, id: &str, status: &str) {
        self.subscription.official_delivery.finish(
            id,
            if status == "suppressed" {
                None
            } else {
                Some(false)
            },
        );
        if status == "suppressed" {
            self.subscription.delivery.finish(id);
        } else {
            self.subscription.delivery.finish_toast(id);
        }
    }
    pub(crate) fn acknowledge_subscription_prompt(&mut self, id: &str) {
        self.subscription.delivery.finish_prompt(id);
        self.subscription.official_delivery.finish(id, Some(true));
    }
    pub(crate) fn subscription_scope_token(&self) -> Result<String> {
        if self.subscription.context_generation == u64::MAX {
            return Err("订阅上下文序号已耗尽，请重启应用".into());
        }
        let identity = self.subscription.current.as_ref().map(|s| {
            json!([
                s["bindingGeneration"],
                s["identityEpoch"],
                s["accountScope"]
            ])
        });
        Ok(hash(
            json!([
                self.runtime_id,
                self.subscription.context_generation.to_string(),
                self.primary_source()?,
                get_setting(&self.db, "clearWatermark")?,
                self.settings()?["privacyMode"],
                identity
            ])
            .to_string()
            .as_bytes(),
        ))
    }
    pub(crate) fn subscription_command(&mut self, command: &str, payload: &Value) -> Result<Value> {
        if command == "read" {
            return self.read_subscription_overview();
        }
        if payload["expectedScope"].as_str() != Some(self.subscription_scope_token()?.as_str()) {
            return Err("订阅范围已变化，请重新读取后再保存".into());
        }
        let mut input = payload.clone();
        input
            .as_object_mut()
            .ok_or("订阅输入必须是对象")?
            .remove("expectedScope");
        if self.settings()?["privacyMode"] == true {
            return Err("隐私模式下暂不能编辑订阅计划或容量依据".into());
        }
        let primary = strv(&self.primary_source()?, "id");
        let source = if command == "status" {
            current_plan(&self.db, &strv(&input, "id"))?["sourceId"].clone()
        } else {
            input["sourceId"].clone()
        };
        if source != primary {
            return Err("只能操作当前主来源的订阅计划".into());
        }
        match command {
            "save" => self.save_usage_plan(&input),
            "status" => self.set_usage_plan_status(&input),
            "assumption" => self.save_quota_assumption(&input),
            "draft" => self.generate_usage_plan_draft(&input),
            _ => Err("未知订阅操作".into()),
        }
    }
    pub fn read_subscription_overview(&self) -> Result<Value> {
        let at = budget_clock::now().utc;
        let responses = queries::subscription_responses(self)?;
        let source = self.primary_source()?;
        let source_id = strv(&source, "id");
        let private = self.settings()?["privacyMode"] == true;
        let mut plans = latest_plans(&self.db)?
            .iter()
            .filter(|p| p["sourceId"] == source_id)
            .filter(|p| p["origin"]["kind"] != "local_schedule")
            .map(|p| plan_view(self, p, &responses, at))
            .collect::<Result<Vec<_>>>()?;
        if private {
            for p in &mut plans {
                p["title"] = json!("使用计划");
            }
        }
        let mut ws = windows(self, at)?;
        let checked_snapshot = structure_checked_snapshot(self, &responses);
        let snapshot = checked_snapshot.as_ref();
        // Only committed, versioned envelopes are evidence. Legacy account
        // observations are never relabelled with today's account identity.
        let history = observation_history(&self.db, &source_id)?;
        let projection_history = super::observation_storage::resolve_many(&self.db, &history)?;
        let derived = super::projector::projections(
            snapshot.unwrap_or(&Value::Null),
            &projection_history,
            &responses,
            &self.runtime_id,
            at.timestamp_millis() as f64 / 1000.0,
        );
        for window in &mut ws {
            if let Some(risk) = derived["risks"].get(strv(window, "id")) {
                window["risk"] = risk.clone();
            }
            let risk_scope = json!({"sourceId":source_id,"windowId":window["id"],"periodInstanceId":window["periodInstanceId"],"runtimeId":self.runtime_id,"bindingGeneration":snapshot.map(|s|s["bindingGeneration"].clone()),"identityEpoch":snapshot.map(|s|s["identityEpoch"].clone()),"clearGeneration":snapshot.map(|s|s["clearGeneration"].clone()),"accountScope":snapshot.map(|s|s["accountScope"].clone())});
            let risk = &mut window["risk"];
            risk["kind"] = json!("native_quota_eta");
            risk["scope"] = risk_scope;
            risk["asOf"] = snapshot
                .map(|s| s["quotaRead"]["freshnessAnchor"].clone())
                .unwrap_or(Value::Null);
            risk["asOfRevision"] = snapshot
                .filter(|s| s["persisted"] == true)
                .map(|s| s["asOfRevision"].clone())
                .unwrap_or(Value::Null);
            risk["observationSequence"] = snapshot
                .map(|s| s["observationSequence"].clone())
                .unwrap_or(Value::Null);
            risk["lastSettledSequence"] = json!(self.subscription.settled.to_string());
            risk["lastPersistedSequence"] = json!(self.subscription.persisted.to_string());
            risk["factTimeEvidence"] = snapshot
                .map(|s| s["quotaRead"].clone())
                .unwrap_or(Value::Null);
            risk["identityEvidence"] = snapshot
                .map(|s| s["identityEvidence"].clone())
                .unwrap_or(Value::Null);
            risk["algorithmVersion"] = derived["algorithmVersion"].clone();
            risk["parameters"] = json!(super::model::NativeParams::default());
            risk["quality"] = json!(if risk["state"] == "unknown" {
                "unavailable"
            } else {
                "conditional"
            });
            risk["rounding"] = json!("outward-minute");
        }
        let membership=snapshot.map(|s| {
            let anchor=s["quotaRead"]["freshnessAnchor"].as_str().and_then(|v|time(v).ok());
            let fresh=s["status"]=="connected" && s["observationOutcome"]=="success" && s["persisted"]==true && anchor.is_some_and(|t|t<=at && (at-t).num_seconds()<240);
            let mut reasons=Vec::new();if !identity_proven(s){reasons.push("identity_unproven");}if !fresh{reasons.push("stale_observation");}if s["persisted"]!=true{reasons.push("not_persisted");}
            json!({"label":s["planType"].as_str().unwrap_or("会员类型未提供"),"planType":s["planType"],"subTier":null,"source":"official-observation","freshness":if fresh{"fresh"}else{"stale"},"reasons":reasons})
        });
        let mut models: BTreeMap<(String, String), (i64, Option<String>, Option<String>)> =
            BTreeMap::new();
        for r in responses.iter().filter(|r| r.source_id == source_id) {
            for dimension in ["response", "request"] {
                let name = model_of(r, dimension).unwrap_or("unknown");
                let entry = models
                    .entry((name.to_string(), dimension.to_string()))
                    .or_default();
                entry.0 = entry
                    .0
                    .checked_add(r.counts.total.value().unwrap_or(0))
                    .filter(|v| *v <= MAX_SAFE_INTEGER)
                    .ok_or("模型总量超出安全范围")?;
                if entry.1.as_ref().is_none_or(|s| s > &r.event_time) {
                    entry.1 = Some(r.event_time.clone());
                }
                if entry.2.as_ref().is_none_or(|s| s < &r.event_time) {
                    entry.2 = Some(r.event_time.clone());
                }
            }
        }
        let model_usage:Vec<Value>=models.into_iter().map(|((model,dimension),(tokens,from,to))|json!({"model":model,"modelDimension":dimension,"tokens":tokens,"from":from,"to":to,"coverage":"recorded-local","reasons":if model=="unknown"{vec!["model_unknown"]}else{vec![]}})).collect();
        let mut calibrations = derived["calibrations"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for card in &mut calibrations {
            if let Some(o) = card.as_object_mut() {
                o.remove("inputPairs");
                o.remove("evidence");
            }
        }
        calibrations.extend(super::calibration_archive::cards(
            &self.db, &source_id, &history, snapshot,
        )?);
        let mut assumptions: Vec<Value> = get_setting(&self.db, "subscriptionAssumptions")?
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            .filter(|v| v["sourceId"] == source_id)
            .collect();
        for a in &mut assumptions {
            a["manualScenario"] = super::manual_assumptions::scenario(a, snapshot, &source_id, at);
            if let Some(object) = a.as_object_mut() {
                object.remove("manualScope");
            }
        }
        if private {
            for a in &mut assumptions {
                a["note"] = json!("");
            }
        }
        let native = self.subscription_native_qualified();
        let learning = snapshot.and_then(|s|s.get("calibrationAssembly")).cloned().unwrap_or_else(||json!({"status":"unavailable","reasonCodes":["adapter_quota_proof_missing"],"assembledPairs":0}));
        let evidence = super::observation_storage::availability(
            &self.db,
            Some(&source_id),
            at,
            observation_retention_days(&self.db)?,
        )?;
        let mut forecast_results = ws.iter().map(|w| w["risk"].clone()).collect::<Vec<_>>();
        for p in &plans {
            for kind in ["localPlanEta", "periodEndUsage"] {
                forecast_results.push(p["forecasts"][kind].clone());
            }
        }
        Ok(
            json!({"schemaVersion":1,"scopeToken":self.subscription_scope_token()?,"revision":revision(&self.db)?,"asOf":timestamp(at),"runtimeId":self.runtime_id,"lastSettledSequence":self.subscription.settled.to_string(),"lastPersistedSequence":self.subscription.persisted.to_string(),"observationPersistence":{"persisted":snapshot.map(|s|s["persisted"].clone()),"error":snapshot.map(|s|s["persistenceError"].clone())},"learningStatus":learning,"evidenceAvailability":evidence,"forecastResults":forecast_results,"source":{"id":source_id,"label":source["label"].as_str().unwrap_or("当前 Codex 来源")},"membership":membership,"capabilities":{"officialQuota":true,"manualPlans":true,"capacityAssumptions":true,"drafts":true,"nativeNotifications":native},"windows":ws,"modelUsage":model_usage,"plans":plans,"calibrations":calibrations,"assumptions":assumptions,"notifications":{"channel":if native{"native"}else{"panel"},"qualified":native,"reasons":if native{Vec::<&str>::new()}else{vec!["native_qualification_pending"]}},"reasons":if snapshot.is_some_and(|s|s["persisted"]!=true){vec!["not_persisted"]}else{vec![]}}),
        )
    }
    #[cfg(test)]
    pub(crate) fn test_read_local_usage_plan(&self, id: &str) -> Result<Value> {
        let plan = current_plan(&self.db, id)?;
        let responses = queries::subscription_responses(self)?;
        plan_view(self, &plan, &responses, budget_clock::now().utc)
    }
    pub fn save_usage_plan(&mut self, payload: &Value) -> Result<Value> {
        let p: SavePlan = serde_json::from_value(payload.clone()).map_err(err)?;
        if !text_ok(&p.title, 120)
            || !["tokens", "percent"].contains(&p.kind.as_str())
            || !["response", "request"].contains(&p.model_dimension.as_str())
            || !["time", "window"].contains(&p.end_policy.as_str())
            || p.model.as_ref().is_some_and(|m| !text_ok(m, 200))
        {
            return Err("计划名称、类型或模型范围无效".into());
        }
        if !p.target.is_finite()
            || p.target <= 0.0
            || (p.kind == "tokens"
                && (p.target.fract() != 0.0 || p.target > MAX_SAFE_INTEGER as f64))
            || (p.kind == "percent" && p.target > 100.0)
        {
            return Err("计划目标超出范围".into());
        }
        source_exists(self, &p.source_id)?;
        let reading = budget_clock::now();
        let at = reading.utc;
        let is_new = p.id.is_none();
        let mut config = if let Some(id) = p.id.as_ref() {
            let mut old = current_plan(&self.db, id)?;
            if old["origin"]["kind"] == "local_schedule" { return Err("请在周期模板中管理此实例；当前实例起点与范围不可移动。".into()); }
            if p.expected_revision != old["revision"].as_i64() {
                return Err("计划已被修改，请重新加载".into());
            }
            if !active_configuration(&self.db, &old, at)? {
                return Err("结束的计划不能重新开始，请新建计划".into());
            }
            if old["type"] != p.kind
                || old["sourceId"] != p.source_id
                || old["model"] != json!(p.model)
                || old["modelDimension"] != p.model_dimension
                || old["endPolicy"] != p.end_policy
                || (p.end_policy == "time"
                    && old["endsAt"]
                        != json!(p.ends_at.as_deref().map(time).transpose()?.map(timestamp)))
                || old["windowId"] != json!(p.window_id)
            {
                return Err("修改不能移动计划起点、范围或结束依据".into());
            }
            old["revision"] = json!(next_revision(old["revision"].as_i64().ok_or("修订无效")?)?);
            old
        } else {
            if p.expected_revision.is_some() {
                return Err("新计划不能指定旧修订".into());
            }
            let count = latest_plans(&self.db)?
                .iter()
                .try_fold(0usize, |count, p| {
                    Ok::<_, String>(count + usize::from(active_configuration(&self.db, p, at)?))
                })?;
            if count >= MAX_PLANS as usize {
                return Err("最多同时运行 100 个计划，请先结束不再需要的计划".into());
            }
            let mut end = p.ends_at.as_deref().map(time).transpose()?;
            let mut period = Value::Null;
            if p.end_policy == "time" {
                if p.kind != "tokens" || p.window_id.is_some() || end.is_none_or(|e| e <= at) {
                    return Err("本机 Token 计划需要未来的固定结束时间".into());
                }
            } else {
                let ws = windows(self, at)?;
                let w = ws
                    .iter()
                    .find(|w| w["id"] == json!(p.window_id))
                    .ok_or("额度窗口不存在")?;
                if w["canBindPlan"] != true {
                    return Err("当前窗口归属或重置语义尚未证明，暂不能绑定计划".into());
                }
                if p.source_id != strv(&self.primary_source()?, "id") {
                    return Err("只能绑定当前主来源窗口".into());
                }
                end = w["resetsAt"].as_str().map(time).transpose()?;
                period = w["periodInstanceId"].clone();
            }
            json!({"id":uuid::Uuid::new_v4().to_string(),"revision":1,"type":p.kind,"sourceId":p.source_id,"model":p.model,"modelDimension":p.model_dimension,"endPolicy":p.end_policy,"endsAt":end.map(timestamp),"windowId":p.window_id,"periodInstanceId":period,"effectiveFrom":timestamp(at),"lifecycle":"active","endedAt":null,"ledgerBaselineRevision":revision(&self.db)?})
        };
        config["title"] = json!(p.title.trim());
        config["target"] = json!(p.target);
        config["remindersEnabled"] = json!(p.reminders_enabled);
        config["updatedAt"] = json!(timestamp(at));
        self.subscription
            .delivery
            .revoke_owner(&strv(&config, "id"));
        self.subscription
            .official_delivery
            .revoke_owner(&strv(&config, "id"));
        let responses = queries::subscription_responses(self)?;
        let view = plan_view(self, &config, &responses, at)?;
        let tx = self.db.transaction().map_err(err)?;
        save_config(&tx, &config)?;
        let mut s = state(&tx, &strv(&config, "id"))?;
        s["used"] = view["used"].clone();
        s["runtimeId"] = json!(self.runtime_id);
        s["baselineAt"] = json!(timestamp(at));
        s["baselineOnly"] = json!(true);
        s["officialBaselinePending"] = json!(true);
        save_state(&tx, &strv(&config, "id"), &s)?;
        if let Some(snapshot) = self.subscription.current.as_ref() {
            super::window_plans::advance(&tx, snapshot, at)?;
        }
        if is_new {
            super::local_plan_forecast::observe(
                &tx,
                &config,
                &view,
                &self.runtime_id,
                &reading,
                true,
            )?;
            super::demand_history::initialize(
                &tx,
                &config,
                self.subscription.current.as_ref(),
                &self.runtime_id,
                &reading,
            )?;
        }
        advance_revision(&tx)?;
        tx.commit().map_err(err)?;
        self.read_subscription_overview()
    }
    pub fn set_usage_plan_status(&mut self, payload: &Value) -> Result<Value> {
        let p: PlanStatus = serde_json::from_value(payload.clone()).map_err(err)?;
        if p.reminders_enabled.is_none() && p.end != Some(true) {
            return Err("请选择提醒开关或结束计划".into());
        }
        let mut config = current_plan(&self.db, &p.id)?;
        if config["origin"]["kind"] == "local_schedule" { return Err("请在周期模板中管理此实例的续建和结束状态。".into()); }
        if config["revision"].as_i64() != Some(p.expected_revision) {
            return Err("计划已被修改，请重新加载".into());
        }
        if !active_configuration(&self.db, &config, budget_clock::now().utc)? {
            return Err("计划已经结束".into());
        }
        self.subscription.delivery.revoke_owner(&p.id);
        self.subscription.official_delivery.revoke_owner(&p.id);
        config["revision"] = json!(next_revision(p.expected_revision)?);
        if let Some(on) = p.reminders_enabled {
            config["remindersEnabled"] = json!(on);
        }
        if p.end == Some(true) {
            config["lifecycle"] = json!("ended");
            config["endedAt"] = json!(now());
        }
        let responses = queries::subscription_responses(self)?;
        let view = plan_view(self, &config, &responses, budget_clock::now().utc)?;
        let tx = self.db.transaction().map_err(err)?;
        save_config(&tx, &config)?;
        let mut s = state(&tx, &p.id)?;
        s["used"] = view["used"].clone();
        s["baselineOnly"] = json!(true);
        s["officialBaselinePending"] = json!(true);
        s["runtimeId"] = json!(self.runtime_id);
        save_state(&tx, &p.id, &s)?;
        advance_revision(&tx)?;
        tx.commit().map_err(err)?;
        self.read_subscription_overview()
    }
    pub fn save_quota_assumption(&mut self, payload: &Value) -> Result<Value> {
        let p: AssumptionInput = serde_json::from_value(payload.clone()).map_err(err)?;
        source_exists(self, &p.source_id)?;
        if !p.capacity_tokens.is_finite()
            || p.capacity_tokens <= 0.0
            || p.capacity_tokens.fract() != 0.0
            || p.capacity_tokens > MAX_SAFE_INTEGER as f64
            || !text_ok(&p.window_id, 200)
            || !text_ok(&p.note, 1000)
            || p.model.as_ref().is_some_and(|m| !text_ok(m, 200))
            || time(&p.valid_until)? <= budget_clock::now().utc
        {
            return Err("容量假设的范围、数值或有效期无效".into());
        }
        let mut values = get_setting(&self.db, "subscriptionAssumptions")?
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default();
        let (id, rev, index) = if let Some(id) = p.id {
            let index = values
                .iter()
                .position(|v| v["id"] == id)
                .ok_or("容量假设不存在")?;
            if values[index]["revision"].as_i64() != p.expected_revision {
                return Err("容量假设已被修改".into());
            }
            (
                id,
                next_revision(p.expected_revision.ok_or("缺少修订")?)?,
                Some(index),
            )
        } else {
            if p.expected_revision.is_some() || values.len() >= MAX_ASSUMPTIONS {
                return Err("新假设修订无效或数量已达上限".into());
            }
            (uuid::Uuid::new_v4().to_string(), 1, None)
        };
        let responses = queries::subscription_responses(self)?;
        let checked_snapshot = structure_checked_snapshot(self, &responses);
        let manual_scope = super::manual_assumptions::freeze(
            checked_snapshot.as_ref(),
            &p.source_id,
            &p.window_id,
            p.model.as_deref(),
            budget_clock::now().utc,
        );
        let v = json!({"id":id,"revision":rev,"sourceId":p.source_id,"windowId":p.window_id,"model":p.model,"capacityTokens":p.capacity_tokens,"validUntil":timestamp(time(&p.valid_until)?),"note":p.note,"manualScope":manual_scope,"provenance":"manual-assumption","quality":"conditional","effectiveFrom":now()});
        if let Some(i) = index {
            values[i] = v;
        } else {
            values.push(v);
        }
        let tx = self.db.transaction().map_err(err)?;
        put_setting(&tx, "subscriptionAssumptions", &json!(values))?;
        advance_revision(&tx)?;
        tx.commit().map_err(err)?;
        self.read_subscription_overview()
    }
    pub fn generate_usage_plan_draft(&self, payload: &Value) -> Result<Value> {
        let p: DraftInput = serde_json::from_value(payload.clone()).map_err(err)?;
        source_exists(self, &p.source_id)?;
        if !["response", "request"].contains(&p.model_dimension.as_str())
            || !p.reserve_percent.is_finite()
            || !(0.0..100.0).contains(&p.reserve_percent)
        {
            return Err("草案范围或预留比例无效".into());
        }
        if p.model.as_ref().is_some_and(|m| !text_ok(m, 200)) {
            return Err("草案模型范围无效".into());
        }
        if p.intent == "habit" {
            let mut result = super::demand_history::draft(
                &self.db,
                payload,
                self.subscription.current.as_ref(),
                &self.runtime_id,
                budget_clock::now().utc,
            );
            result["kind"] = json!("historical_demand");
            result["scope"] =
                json!({"sourceId":p.source_id,"model":p.model,"modelDimension":p.model_dimension});
            result["asOf"] = json!(now());
            result["asOfRevision"] = json!(revision(&self.db)?);
            result["notifyEligible"] = json!(false);
            return Ok(result);
        }
        if p.intent != "remaining" {
            return Err("未知草案意图".into());
        }
        if p.source_id != strv(&self.primary_source()?, "id") {
            return Err("只能按当前主来源的官方额度生成草案".into());
        }
        let at = budget_clock::now().utc;
        let ws = windows(self, at)?;
        let w = ws
            .iter()
            .find(|w| w["id"] == json!(p.window_id))
            .ok_or("请选择当前额度窗口")?;
        if w["canBindPlan"] != true {
            return Ok(
                json!({"plan":null,"basis":["官方剩余比例"],"constraintsVerified":false,"reasons":w["reasons"]}),
            );
        }
        // A Token target is available only after the complete long-term gate
        // and every current related pool independently pass their constraints.
        // The fallback below remains the native-percent suggestion.
        let responses = queries::subscription_responses(self)?;
        let checked_snapshot = structure_checked_snapshot(self, &responses);
        if let Some(current) = checked_snapshot.as_ref() {
            let history = observation_history(&self.db, &p.source_id)?;
            let history = super::observation_storage::resolve_many(&self.db, &history)?;
            let derived = super::projector::projections(
                current,
                &history,
                &responses,
                &self.runtime_id,
                at.timestamp() as f64 + f64::from(at.timestamp_subsec_nanos()) / 1e9,
            );
            if let Some(calibrations) = derived["calibrations"].as_array() {
                let selection = super::remaining_draft::Selection {
                    source_id: &p.source_id,
                    window_id: p.window_id.as_deref().unwrap_or(""),
                    model: p.model.as_deref(),
                    model_dimension: &p.model_dimension,
                    reserve_percent: p.reserve_percent,
                };
                if let Some(draft) = super::remaining_draft::token_draft(
                    &selection,
                    current,
                    &ws,
                    calibrations,
                    &self.runtime_id,
                    at,
                ) {
                    return Ok(draft);
                }
            }
        }
        let used = w["usedPercent"].as_f64().ok_or("官方比例缺失")?;
        let target = super::evaluate::reserved_native_target(used, p.reserve_percent / 100.0)
            .ok_or("原生额度目标无法计算")?;
        let others: Vec<_> = ws
            .iter()
            .filter(|v| v["id"] != w["id"])
            .map(|v| v["id"].clone())
            .collect();
        let end = w["resetsAt"].as_str().map(time).transpose()?;
        let crosses_related_reset = ws.iter().filter(|v| v["id"] != w["id"]).any(|v| {
            v["resetsAt"]
                .as_str()
                .and_then(|v| time(v).ok())
                .zip(end)
                .is_some_and(|(other, selected)| other < selected)
        });
        if crosses_related_reset {
            return Ok(
                json!({"plan":null,"basis":["首批草案只安排至下一个相关重置边界"],"constraintsVerified":false,"reasons":["earlier_related_reset"],"unverifiedWindows":others}),
            );
        }
        Ok(
            json!({"plan":{"expectedRevision":null,"title":"按当前余量安排","type":"percent","sourceId":p.source_id,"model":null,"modelDimension":p.model_dimension,"target":target,"endPolicy":"window","endsAt":w["resetsAt"],"windowId":w["id"],"remindersEnabled":true},"basis":[format!("预留当前剩余量的 {}%",p.reserve_percent),"草案保存后才生效；共享池目标不拆成独立模型额度"],"constraintsVerified":others.is_empty(),"reasons":if others.is_empty(){json!([])}else{json!(["related_pool_constraints_unverified"])} ,"unverifiedWindows":others}),
        )
    }
    pub(crate) fn settle_subscription_observation(
        &mut self,
        binding_generation: u64,
        snapshot: Value,
    ) -> Result<Value> {
        self.settle_subscription_observation_with_evidence(binding_generation, snapshot, &[])
    }
    pub(crate) fn settle_subscription_observation_with_evidence(
        &mut self,
        binding_generation: u64,
        mut snapshot: Value,
        learning_batches: &[super::learning::CollectorBatch<'_>],
    ) -> Result<Value> {
        let mut previous = self.subscription.current.clone();
        let Some(seq) = decimal(&snapshot["observationSequence"]) else {
            self.subscription.delivery.revoke_quota_bound();
            self.subscription.official_delivery.revoke_all();
            self.subscription.observation_discontinuity = true;
            // A current admission/resource failure is a safety boundary even
            // though no RPC ran and neither observation watermark may advance.
            snapshot["persisted"] = json!(false);
            snapshot["asOfRevision"] = Value::Null;
            self.subscription.current = Some(snapshot.clone());
            return Ok(snapshot);
        };
        let source = strv(&self.primary_source()?, "id");
        let clear = get_setting(&self.db, "clearWatermark")?.unwrap_or(Value::Null);
        let domain = hash(
            json!([
                self.runtime_id,
                source,
                binding_generation.to_string(),
                clear
            ])
            .to_string()
            .as_bytes(),
        );
        if self.subscription.domain != domain {
            self.subscription.delivery.revoke_quota_bound();
            self.subscription.official_delivery.revoke_all();
            self.subscription.domain = domain;
            self.subscription.settled = 0;
            self.subscription.persisted = 0;
            self.subscription.current = None;
            self.subscription.observation_discontinuity = false;
            previous = None;
        }
        if seq <= self.subscription.settled {
            return self
                .subscription
                .current
                .clone()
                .ok_or_else(|| "旧账户读取已被丢弃".into());
        }
        self.subscription.delivery.revoke_quota_bound();
        self.subscription.official_delivery.revoke_all();
        self.subscription.settled = seq;
        if self.subscription.observation_discontinuity {
            snapshot["discontinuity"] = json!(true);
        }
        // Any error after admission, including ledger input loading before
        // BEGIN, must break the next real observation. Clear only on commit.
        self.subscription.observation_discontinuity = true;
        snapshot["runtimeId"] = json!(self.runtime_id);
        snapshot["sourceId"] = json!(source);
        snapshot["bindingGeneration"] = json!(binding_generation.to_string());
        snapshot["clearGeneration"] = clear.clone();
        snapshot["persisted"] = json!(false);
        let proof_admission = super::learning::admit_observation(
            &mut snapshot,
            previous.as_ref(),
            learning_batches,
            &budget_clock::now(),
        );
        self.subscription.current = Some(snapshot.clone());
        let admitted = identity_proven(&snapshot) && snapshot["observationOutcome"] == "success";
        let responses = queries::subscription_responses(self)?;
        let native_qualified = self.subscription_native_qualified();
        let delivery_context = self.subscription_delivery_context()?;
        let mut official_ticket = if admitted && native_qualified {
            self.subscription.official_delivery.begin(
                &self.db,
                std::sync::Arc::clone(&self.notification_gate),
                delivery_context,
            )?
        } else {
            None
        };
        let mut receipt = json!({"documentKind":"subscription_observation","schemaVersion":1,"runtimeId":self.runtime_id,"sourceId":source,"bindingGeneration":binding_generation.to_string(),"clearGeneration":clear,"identityEpoch":snapshot["identityEpoch"],"observationSequence":seq.to_string(),"lastSettledSequence":seq.to_string(),"lastPersistedSequence":seq.to_string(),"outcome":snapshot["observationOutcome"],"identityConsistency":snapshot["identityConsistency"],"identityConsistencyReason":snapshot["identityConsistencyReason"],"readStartedAt":snapshot["readStartedAt"],"readFinishedAt":snapshot["readFinishedAt"],"accountRead":snapshot["accountRead"],"quotaRead":snapshot["quotaRead"],"usageRead":snapshot["usageRead"]});
        if admitted {
            receipt["observation"] = snapshot.clone();
            // Daily trend data has an independent provenance and is already
            // available in RAM. It is not an input to quota calibration.
            receipt["observation"]
                .as_object_mut()
                .unwrap()
                .remove("usage");
        }
        receipt["proofAdmission"] = proof_admission;
        super::learning::seal_receipt(&mut receipt)?;
        let committed = (|| -> Result<(i64, Vec<super::official_delivery::Candidate>)> {
            let tx = self.db.transaction().map_err(err)?;
            let stored_receipt =
                super::observation_storage::encode(&tx, &receipt, budget_clock::now().utc)?;
            tx.execute(
                "INSERT INTO account_snapshots(collected_at,data) VALUES(?1,?2)",
                params![now(), stored_receipt.to_string()],
            )
            .map_err(err)?;
            let current_receipt_id = tx.last_insert_rowid();
            let learning = super::learning::advance(
                &tx,
                &source,
                learning_batches,
                &responses,
                &budget_clock::now(),
            )?;
            let current_receipt: Value = serde_json::from_str(
                &tx.query_row(
                    "SELECT data FROM account_snapshots WHERE id=?1",
                    [current_receipt_id],
                    |r| r.get::<_, String>(0),
                )
                .map_err(err)?,
            )
            .map_err(err)?;
            let current_receipt = super::observation_storage::resolve(&tx, &current_receipt)?;
            if let Some(enriched) = super::projector::receipt_snapshot(&current_receipt) {
                let trend = snapshot.get("usage").cloned();
                snapshot = enriched;
                if let Some(trend) = trend {
                    snapshot["usage"] = trend;
                }
                snapshot["persisted"] = json!(false);
            }
            snapshot["calibrationAssembly"] = learning;
            if admitted {
                super::window_plans::advance(&tx, &snapshot, budget_clock::now().utc)?;
                super::official_reminders::advance(
                    &tx,
                    previous.as_ref(),
                    &snapshot,
                    &self.runtime_id,
                    budget_clock::now().utc,
                )?;
            }
            let rev = advance_revision(&tx)?;
            trim_observations(&tx)?;
            let history = observation_history(&tx, &source)?;
            let projection_history = super::observation_storage::resolve_many(&tx, &history)?;
            let mut committed_snapshot = snapshot.clone();
            committed_snapshot["persisted"] = json!(true);
            let derived = super::projector::projections(
                &committed_snapshot,
                &projection_history,
                &responses,
                &self.runtime_id,
                budget_clock::now().utc.timestamp_millis() as f64 / 1000.0,
            );
            super::quota_watch::advance(
                &tx,
                &committed_snapshot,
                &derived,
                &self.runtime_id,
                &budget_clock::now(),
            )?;
            super::calibration_archive::advance(
                &tx,
                &source,
                &derived,
                &history,
                rev,
                budget_clock::now().utc,
            )?;
            let candidates = super::official_delivery::Delivery::prepare(
                &tx,
                official_ticket.as_ref(),
                &committed_snapshot,
                &derived,
                native_qualified,
            )?;
            tx.commit().map_err(err)?;
            Ok((rev, candidates))
        })();
        match committed {
            Ok((rev, candidates)) => {
                self.subscription.observation_discontinuity = false;
                self.subscription.persisted = seq;
                snapshot["persisted"] = json!(true);
                snapshot["asOfRevision"] = json!(rev);
                let context = self.subscription_delivery_context().unwrap_or_default();
                let qualified = !context.is_empty() && self.subscription_native_qualified();
                if self
                    .subscription
                    .official_delivery
                    .install(
                        &self.db,
                        official_ticket.take(),
                        candidates,
                        &snapshot,
                        &context,
                        qualified,
                    )
                    .is_err()
                {
                    self.subscription.official_delivery.revoke_all();
                    snapshot["nativeDeliveryAdmission"] = json!("failed_closed");
                }
            }
            Err(_) => {
                self.subscription.observation_discontinuity = true;
                snapshot["persistenceError"] = json!("observation_persistence_failed");
                snapshot["calibrationAssembly"] = json!({"status":"unavailable","reasonCodes":["observation_persistence_failed"],"assembledPairs":0});
                snapshot["asOfRevision"] = Value::Null;
            }
        }
        self.subscription.current = Some(snapshot.clone());
        Ok(snapshot)
    }
    pub(crate) fn invalidate_subscription(&mut self) {
        self.subscription = Runtime {
            context_generation: self
                .subscription
                .context_generation
                .checked_add(1)
                .unwrap_or(u64::MAX),
            ..Default::default()
        };
    }
    pub(crate) fn subscription_unavailable(&mut self) {
        self.subscription.delivery.revoke_quota_bound();
        self.subscription.official_delivery.revoke_all();
        self.subscription.observation_discontinuity = true;
        if let Some(s) = self.subscription.current.as_mut() {
            s["status"] = json!("stale");
            s["observationOutcome"] = json!("failure");
        }
    }
    pub(crate) fn retain_subscription_observations(&mut self) -> Result<()> {
        if retain_observations(&mut self.db)? {
            self.subscription.scan_at = None;
            self.subscription.delivery.revoke_all();
            self.subscription.official_delivery.revoke_all();
        }
        Ok(())
    }
    pub(crate) fn begin_subscription_scan(&mut self) -> Result<Option<Scan>> {
        if latest_plans(&self.db)?.is_empty() {
            return Ok(None);
        }
        let at = budget_clock::now();
        let continuous = self.subscription.scan_at.is_some_and(|last| {
            at.mono_ms >= last.mono_ms
                && at.mono_ms - last.mono_ms <= 15_000
                && (at.utc - last.utc).num_milliseconds() >= 0
                && ((at.utc - last.utc).num_milliseconds() as i128
                    - i128::from(at.mono_ms - last.mono_ms))
                .abs()
                    <= 2000
        });
        let responses = queries::subscription_responses(self)?;
        let before = latest_plans(&self.db)?
            .iter()
            .map(|p| Ok((strv(p, "id"), plan_view(self, p, &responses, at.utc)?)))
            .collect::<Result<_>>()?;
        let rowid = self
            .db
            .query_row(
                "SELECT COALESCE(MAX(rowid),0) FROM usage_responses",
                [],
                |r| r.get(0),
            )
            .map_err(err)?;
        let delivery_context = self.subscription_delivery_context()?;
        let gate = self.notification_gate();
        let delivery_sample = self.subscription.delivery.begin(
            &self.db,
            gate,
            &self.runtime_id,
            &delivery_context,
        )?;
        Ok(Some(Scan {
            at,
            rowid,
            before,
            fingerprint: plan_configuration_fingerprint(self)?,
            continuous,
            delivery_sample,
            delivery_context,
        }))
    }
    pub(crate) fn finish_subscription_scan(
        &mut self,
        scan: Option<Scan>,
        committed: &BTreeSet<String>,
        cancelled: bool,
    ) -> Result<()> {
        let Some(scan) = scan else {
            return Ok(());
        };
        let at = budget_clock::now();
        let scan_stable = !cancelled
            && at.mono_ms.checked_sub(scan.at.mono_ms).is_some_and(|v| {
                v <= 15_000
                    && ((at.utc - scan.at.utc).num_milliseconds() as i128 - i128::from(v)).abs()
                        <= 2000
            })
            && at.utc >= scan.at.utc
            && scan.fingerprint == plan_configuration_fingerprint(self)?;
        let stable = scan_stable && scan.continuous;
        let qualified = self.subscription_native_qualified()
            && stable
            && self.subscription_delivery_context()? == scan.delivery_context;
        if !scan_stable {
            self.subscription.delivery.revoke_all();
            self.subscription.official_delivery.revoke_all();
        }
        let responses = queries::subscription_responses(self)?;
        let plans = latest_plans(&self.db)?;
        let mut inserted = BTreeSet::new();
        {
            let mut s = self
                .db
                .prepare("SELECT id FROM usage_responses WHERE rowid>?1")
                .map_err(err)?;
            for row in s
                .query_map([scan.rowid], |r| r.get::<_, String>(0))
                .map_err(err)?
            {
                inserted.insert(row.map_err(err)?);
            }
        }
        let mut updates = Vec::new();
        for p in plans {
            let view = plan_view(self, &p, &responses, at.utc)?;
            let old = scan.before.get(&strv(&p, "id"));
            let start = time(p["effectiveFrom"].as_str().ok_or("计划起点缺失")?)?;
            let end = view["measuredUntil"]
                .as_str()
                .map(time)
                .transpose()?
                .ok_or("计划计量边界缺失")?;
            let fresh_tokens = responses
                .iter()
                .filter(|r| {
                    inserted.contains(&adapter::response_key(r))
                        && r.live
                        && time(&r.event_time)
                            .is_ok_and(|t| t >= scan.at.utc - Duration::seconds(15))
                        && matches_plan(r, &p, start, end)
                })
                .try_fold(0i64, |sum, r| {
                    sum.checked_add(r.counts.total.value().unwrap_or(0))
                        .ok_or("Token 累计溢出")
                })?;
            let old_used = old.and_then(|o| o["used"].as_f64());
            let used = view["used"].as_f64();
            let can_cross = stable
                && committed.contains(&strv(&p, "sourceId"))
                && p["remindersEnabled"] == true
                && view["lifecycle"] == "active"
                && p["type"] == "tokens"
                && fresh_tokens > 0
                && old_used
                    .zip(used)
                    .is_some_and(|(a, b)| (b - a - fresh_tokens as f64).abs() < 0.01);
            let reliable = scan_stable
                && committed.contains(&strv(&p, "sourceId"))
                && old_used
                    .zip(used)
                    .is_some_and(|(a, b)| (b - a - fresh_tokens as f64).abs() < 0.01);
            let fact_ids = responses
                .iter()
                .filter(|r| matches_plan(r, &p, start, end))
                .map(adapter::response_key)
                .collect::<Vec<_>>();
            let fresh_fact_ids = responses
                .iter()
                .filter(|r| {
                    inserted.contains(&adapter::response_key(r))
                        && r.live
                        && time(&r.event_time)
                            .is_ok_and(|t| t >= scan.at.utc - Duration::seconds(15))
                        && matches_plan(r, &p, start, end)
                })
                .map(adapter::response_key)
                .collect::<Vec<_>>();
            let fresh_append = fresh_local_append_proof(
                self, &p, &view, &responses, &inserted, &scan, &at, reliable,
            )?;
            updates.push((
                p,
                view,
                old_used,
                can_cross,
                reliable,
                fact_ids,
                fresh_fact_ids,
                fresh_append,
            ));
        }
        let mut crossings = Vec::new();
        let tx = self.db.transaction().map_err(err)?;
        for (p, view, old_used, can_cross, reliable, fact_ids, fresh_fact_ids, fresh_append) in
            updates
        {
            let id = strv(&p, "id");
            let mut s = state(&tx, &id)?;
            let baseline = s["runtimeId"] == self.runtime_id && s["used"].as_f64() == old_used;
            let mut latched = s["latched"].as_array().cloned().unwrap_or_default();
            if can_cross && baseline {
                for level in [80, 100] {
                    let target = p["target"].as_f64().ok_or("计划目标缺失")? as i64;
                    let reached = |used: f64| {
                        i128::from(used as i64) * 100 >= i128::from(target) * i128::from(level)
                    };
                    if old_used.is_some_and(|u| !reached(u))
                        && view["used"].as_f64().is_some_and(reached)
                        && !latched.contains(&json!(level))
                    {
                        latched.push(json!(level));
                        let event_id =
                            hash(format!("plan|{id}|{id}|threshold|{level}|0").as_bytes());
                        let event = json!({"id":event_id,"kind":"subscription_plan","sourceId":p["sourceId"],"threadId":"","turnId":null,"eventTime":timestamp(at.utc),"createdAt":timestamp(at.utc),"title":format!("使用计划达到 {level}%"),"message":format!("{} 已达到计划的 {}%",strv(&p,"title"),level),"planId":id,"periodInstanceId":id,"planRevision":p["revision"],"threshold":level,"notifyEligible":qualified,"animationAcknowledged":!qualified,"eligibleRuntime":self.runtime_id,"eligibleContext":scan.delivery_context,"notificationState":if qualified{"pending"}else{"panel_only"},"read":false});
                        let inserted=tx.execute("INSERT OR IGNORE INTO inbox_events(id,thread_id,turn_id,event_time,data) VALUES(?1,'',NULL,?2,?3)",params![event_id,timestamp(at.utc),event.to_string()]).map_err(err)?;
                        if inserted == 1 && qualified {
                            crossings.push(super::delivery::TokenCrossing {
                                event_id,
                                plan_id: id.clone(),
                                source_id: strv(&p, "sourceId"),
                                period_instance_id: id.clone(),
                                plan_revision: p["revision"].as_i64().ok_or("计划修订无效")?,
                                threshold: i64::from(level),
                                before_used: old_used.ok_or("越线前用量缺失")? as i64,
                                after_used: view["used"].as_f64().ok_or("越线后用量缺失")? as i64,
                                fact_ids: fact_ids.clone(),
                                fresh_fact_ids: fresh_fact_ids.clone(),
                            });
                        }
                    }
                }
            }
            s["latched"] = json!(latched);
            s["used"] = view["used"].clone();
            s["runtimeId"] = json!(self.runtime_id);
            s["baselineAt"] = json!(timestamp(at.utc));
            s["evaluationState"] = view["evaluationState"].clone();
            save_state(&tx, &id, &s)?;
            super::demand_history::observe(
                &tx,
                &p,
                &view,
                self.subscription.current.as_ref(),
                &self.runtime_id,
                &at,
                reliable,
            )?;
            super::local_plan_forecast::observe_with_fresh_append(
                &tx,
                &p,
                &view,
                &self.runtime_id,
                &at,
                reliable,
                fresh_append.as_ref(),
            )?;
        }
        for source in committed {
            let history = observation_history(&tx, source)?;
            super::calibration_archive::reconcile(&tx, source, &history, at.utc)?;
        }
        tx.commit().map_err(err)?;
        if let Some(sample) = scan.delivery_sample.as_ref() {
            for crossing in crossings {
                self.subscription
                    .delivery
                    .grant(&self.db, sample, crossing, qualified)?;
            }
        }
        self.subscription.scan_at = if cancelled { None } else { Some(at) };
        Ok(())
    }
}
fn observation_retention_days(db: &Connection) -> Result<i64> {
    Ok(get_setting(db, "preferences")?
        .and_then(|v| v["retentionDays"].as_i64())
        .unwrap_or(90)
        .clamp(1, 90))
}
fn observation_sources(db: &Connection) -> Result<Vec<String>> {
    let mut s=db.prepare("SELECT DISTINCT json_extract(data,'$.sourceId') FROM account_snapshots WHERE json_extract(data,'$.documentKind')='subscription_observation' AND json_type(data,'$.sourceId')='text' UNION SELECT DISTINCT source_id FROM quota_calibrations").map_err(err)?;
    let rows = s.query_map([], |r| r.get::<_, String>(0)).map_err(err)?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(err)
}
fn trim_observations(db: &Connection) -> Result<()> {
    let at = budget_clock::now().utc;
    let days = observation_retention_days(db)?;
    let retained = super::observation_storage::retain(db, at, days)?;
    if retained["deletedTotal"].as_u64().is_some_and(|v| v > 0) {
        for source in observation_sources(db)? {
            let history = observation_history(db, &source)?;
            super::calibration_archive::reconcile(db, &source, &history, at)?;
        }
    }
    super::calibration_archive::retain(db, at, days)?;
    super::observation_storage::retain_holdouts(db, at, days)?;
    db.execute("DELETE FROM plan_period_state WHERE owner_kind='quota_watch' AND julianday(json_extract(data,'$.updatedAt'))<julianday(?1)",[timestamp(at-Duration::days(days))]).map_err(err)?;
    Ok(())
}
pub(super) fn retain_observations(db: &mut Connection) -> Result<bool> {
    let schema: i64 = db
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(err)?;
    if schema < 7 {
        // Legacy retention baselines have no subscription domain. A current
        // schema with missing tables still errors rather than hiding damage.
        return Ok(false);
    }
    let before = db.total_changes();
    let tx = db.transaction().map_err(err)?;
    trim_observations(&tx)?;
    for source in observation_sources(&tx)? {
        let history = observation_history(&tx, &source)?;
        super::calibration_archive::reconcile(&tx, &source, &history, budget_clock::now().utc)?;
    }
    tx.commit().map_err(err)?;
    Ok(db.total_changes() != before)
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
