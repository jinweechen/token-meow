//! Small pet read models. No full snapshot, source coverage, or fact collection is built here.
use super::*;
use chrono::{DateTime, FixedOffset, NaiveDate};
use rusqlite::types::ValueRef;
use serde::Serialize;

const PROMPT_LIMIT: usize = 8;
const PET_SETTINGS: &[&str] = &[
    "muted",
    "reducedMotion",
    "doNotDisturb",
    "effectiveDoNotDisturb",
    "scheduledDoNotDisturb",
    "temporaryDoNotDisturb",
    "petVisible",
    "petScale",
    "clickThrough",
    "alphaHitTest",
    "privacyMode",
    "idlePlayEnabled",
    "workAppLinkEnabled",
    "idleRoamingEnabled",
    "idleRoamRange",
    "workCareEnabled",
    "careTone",
    "careIntervalMinutes",
];

fn day_at(at: DateTime<Utc>) -> String {
    at.with_timezone(&FixedOffset::east_opt(8 * 3600).unwrap())
        .format("%Y-%m-%d")
        .to_string()
}

fn parsed(time: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(time)
        .ok()
        .map(|v| v.with_timezone(&Utc))
}

/// Keep JSON string semantics; SQLite also extracts numeric/boolean scalars.
fn text(row: &rusqlite::Row<'_>, column: usize) -> Result<Option<String>> {
    match row.get_ref(column).map_err(err)? {
        ValueRef::Null => Ok(None),
        ValueRef::Text(v) => Ok(Some(std::str::from_utf8(v).map_err(err)?.to_owned())),
        _ => Ok(None),
    }
}

fn flag_at(row: &rusqlite::Row<'_>, column: usize) -> Result<bool> {
    Ok(matches!(
        row.get_ref(column).map_err(err)?,
        ValueRef::Integer(1)
    ))
}

fn source_scope(core: &Core, day: &str) -> Result<String> {
    let mut statement = core
        .db
        .prepare("SELECT id,root,enabled FROM source_instances ORDER BY id")
        .map_err(err)?;
    let mut rows = statement.query([]).map_err(err)?;
    let mut sources = Vec::new();
    while let Some(row) = rows.next().map_err(err)? {
        let id: String = row.get(0).map_err(err)?;
        let root: String = row.get(1).map_err(err)?;
        let enabled: bool = row.get(2).map_err(err)?;
        let check = get_setting(&core.db, &format!("lastSourceCheck:{id}"))?;
        let runtime = super::queries::source_runtime(Path::new(&root), enabled, check.as_ref());
        sources.push(json!([id, root, enabled, runtime]));
    }
    let scope = json!({
        "protocolVersion":1,"runtimeEpoch":core.runtime_id,"day":day,
        "primarySourceId":core.source_id,"sources":sources,
        "clearWatermark":get_setting(&core.db,"clearWatermark")?,
        "inboxWatermark":get_setting(&core.db,"inboxWatermark")?
    });
    Ok(hash(&serde_json::to_vec(&scope).map_err(err)?))
}

fn feed_high_watermark(core: &Core) -> Result<i64> {
    let high: Option<i64> = core
        .db
        .query_row(
            "SELECT seq FROM sqlite_sequence WHERE name='pet_inbox_feed'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(err)?;
    match high.unwrap_or(0) {
        n if n >= 0 => Ok(n),
        _ => Err("宠物提示到达序号无效".into()),
    }
}

fn cursor_sequence(cursor: &Value) -> Result<i64> {
    if !cursor.is_object() {
        return Err("宠物提示游标必须为对象".into());
    }
    if cursor.as_object().unwrap().keys().any(|key| {
        !["runtimeEpoch", "sourceScopeRevision", "day", "sequence"].contains(&key.as_str())
    }) {
        return Err("宠物提示游标含未知字段".into());
    }
    for field in ["runtimeEpoch", "sourceScopeRevision", "day"] {
        if cursor.get(field).and_then(Value::as_str).is_none() {
            return Err(format!("宠物提示游标缺少有效 {field}"));
        }
    }
    let sequence = cursor
        .get("sequence")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit()))
        .ok_or("宠物提示序号必须为非负十进制字符串")?;
    sequence.parse().map_err(|_| "宠物提示序号超出范围".into())
}

fn activity(core: &Core, at: DateTime<Utc>) -> Result<Value> {
    let mut statement = core.db.prepare(
        "SELECT json_extract(data,'$.id'),json_extract(data,'$.rootTurnId'),json_extract(data,'$.lastEventAt') FROM turns WHERE json_extract(data,'$.status')='active'",
    ).map_err(err)?;
    let mut rows = statement.query([]).map_err(err)?;
    let (mut active, mut uncertain) = (0u64, 0u64);
    while let Some(row) = rows.next().map_err(err)? {
        let id = text(row, 0)?.unwrap_or_default();
        if text(row, 1)?.is_some_and(|root| root != id) {
            continue;
        }
        let fresh = text(row, 2)?
            .and_then(|v| parsed(&v))
            .is_some_and(|time| (-5..600).contains(&at.signed_duration_since(time).num_seconds()));
        if fresh {
            active += 1;
        } else {
            uncertain += 1;
        }
    }
    Ok(
        json!({"activeTurns":active,"uncertainTurns":uncertain,"state":if active>0{"active"}else if uncertain>0{"uncertain"}else{"idle"}}),
    )
}

fn unread_count(core: &Core, day: &str) -> Result<u64> {
    let mut statement = core
        .db
        .prepare(
            "SELECT json_extract(data,'$.createdAt'),json_extract(data,'$.read') FROM inbox_events",
        )
        .map_err(err)?;
    let mut rows = statement.query([]).map_err(err)?;
    let mut count = 0;
    while let Some(row) = rows.next().map_err(err)? {
        // Match the current UI's !item.read, including missing/null legacy values.
        let read = match row.get_ref(1).map_err(err)? {
            ValueRef::Null => false,
            ValueRef::Integer(v) => v != 0,
            ValueRef::Real(v) => v != 0.0,
            ValueRef::Text(v) | ValueRef::Blob(v) => !v.is_empty(),
        };
        if !read
            && text(row, 0)?
                .and_then(|v| parsed(&v))
                .is_some_and(|at| day_at(at) == day)
        {
            count += 1;
        }
    }
    Ok(count)
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Dropped {
    overflow: u64,
    expired: u64,
    invalid_time: u64,
    future: u64,
    ineligible: u64,
    other_day: u64,
}

struct Candidate {
    value: Value,
    time: DateTime<Utc>,
    alert: bool,
    id: String,
}

fn prompts(
    core: &Core,
    after: i64,
    high: i64,
    day: &str,
    at: DateTime<Utc>,
) -> Result<(Vec<Value>, Dropped)> {
    let (_, temporary_filter) = temporary_dnd::read(&core.temporary_dnd, &core.db)?;
    let mut statement = core.db.prepare(
        "SELECT i.id,json_extract(i.data,'$.kind'),json_extract(i.data,'$.status'),json_extract(i.data,'$.createdAt'),json_extract(i.data,'$.eligibleRuntime'),json_type(i.data,'$.notifyEligible')='true',json_type(i.data,'$.animationAcknowledged')='true',f.sequence FROM pet_inbox_feed f JOIN inbox_events i ON i.id=f.inbox_id WHERE f.sequence>?1 AND f.sequence<=?2 ORDER BY f.sequence",
    ).map_err(err)?;
    let mut rows = statement.query(params![after, high]).map_err(err)?;
    let mut selected = Vec::<Candidate>::new();
    let mut dropped = Dropped::default();
    while let Some(row) = rows.next().map_err(err)? {
        let created = text(row, 3)?.unwrap_or_default();
        let Some(time) = parsed(&created) else {
            dropped.invalid_time += 1;
            continue;
        };
        // Categories are disjoint, in this order; every observed row still advances the cursor.
        if day_at(time) != day {
            dropped.other_day += 1;
            continue;
        }
        let age = at.signed_duration_since(time);
        if age < chrono::Duration::zero() {
            dropped.future += 1;
            continue;
        }
        if age > chrono::Duration::seconds(10) {
            dropped.expired += 1;
            continue;
        }
        if text(row, 4)?.as_deref() != Some(core.runtime_id.as_str())
            || !flag_at(row, 5)?
            || flag_at(row, 6)?
            || temporary_filter.suppresses(Some(row.get::<_,i64>(7).map_err(err)?), Some(time))
        {
            dropped.ineligible += 1;
            continue;
        }
        let id: String = row.get(0).map_err(err)?;
        let kind = text(row, 1)?.unwrap_or_else(|| "unknown".into());
        // Persistent eligibility is not a fresh delivery permit. A revoked,
        // expired, or pre-suspend budget must still advance the arrival cursor.
        if kind != "completion" && !core.domain_prompt_is_current(&kind,&id) {
            dropped.ineligible += 1;
            continue;
        }
        let status = text(row, 2)?.unwrap_or_else(|| "unknown".into());
        let alert = kind == "budget"
            || !matches!(
                status.to_lowercase().as_str(),
                "completed" | "success" | "succeeded" | "normal" | "ok"
            );
        selected.push(Candidate {
            value: json!({"id":id,"kind":kind,"status":status,"createdAt":created,"animationEligible":true}),
            time,
            alert,
            id,
        });
        selected.sort_by(|a, b| {
            b.alert
                .cmp(&a.alert)
                .then_with(|| b.time.cmp(&a.time))
                .then_with(|| a.id.cmp(&b.id))
        });
        if selected.len() > PROMPT_LIMIT {
            selected.pop();
            dropped.overflow += 1;
        }
    }
    Ok((selected.into_iter().map(|v| v.value).collect(), dropped))
}

pub(super) fn pulse(core: &Core, cursor: Option<&Value>) -> Result<Value> {
    pulse_at(core, cursor, crate::budget_clock::now().utc)
}

pub(super) fn pulse_at(core: &Core, cursor: Option<&Value>, at: DateTime<Utc>) -> Result<Value> {
    let day = day_at(at);
    let scope = source_scope(core, &day)?;
    let high = feed_high_watermark(core)?;
    let cursor = cursor.filter(|v| !v.is_null());
    let after = cursor.map(cursor_sequence).transpose()?;
    let baseline = cursor.is_none_or(|v| {
        v["runtimeEpoch"] != core.runtime_id || v["sourceScopeRevision"] != scope || v["day"] != day
    });
    if !baseline && after.is_some_and(|sequence| sequence > high) {
        return Err("宠物提示序号超过账本到达高水位".into());
    }
    let (prompts, dropped) = if baseline {
        (Vec::new(), Dropped::default())
    } else {
        prompts(core, after.unwrap(), high, &day, at)?
    };
    let all_settings = core.settings_at(at)?;
    let settings: serde_json::Map<String, Value> = PET_SETTINGS
        .iter()
        .filter_map(|field| {
            all_settings
                .get(*field)
                .map(|v| ((*field).into(), v.clone()))
        })
        .collect();
    let issues: i64 = core.db.query_row(
        "SELECT COUNT(*) FROM (SELECT kind,message FROM ingestion_issues GROUP BY kind,message)",
        [], |row|row.get(0),
    ).map_err(err)?;
    let conflicts: i64 = core.db.query_row(
        "SELECT COUNT(*) FROM identity_conflicts WHERE COALESCE(json_extract(details,'$.resolved'),0)=0",
        [], |row|row.get(0),
    ).map_err(err)?;
    let last_event: Option<String> = core
        .db
        .query_row(
            "SELECT MAX(json_extract(value,'$')) FROM settings WHERE key LIKE 'sourceLastEvent:%'",
            [],
            |row| row.get(0),
        )
        .map_err(err)?;
    let activity = activity(core, at)?;
    let unread = unread_count(core, &day)?;
    let last_scan = get_setting(&core.db, "lastSuccessfulScan")?;
    // This is the only mutation, and only after all fallible reads have succeeded.
    let revision = core.next_pet_revision()?;
    Ok(json!({
        "protocolVersion":1,"runtimeEpoch":core.runtime_id,"revision":revision,
        "day":day,"asOf":at.to_rfc3339_opts(chrono::SecondsFormat::Millis,true),
        "sourceScopeRevision":scope,"settings":settings,"activity":activity,
        "health":{"lastSuccessfulScan":last_scan,"lastSourceEvent":last_event,"issues":issues,"conflicts":conflicts},
        "unreadCount":unread,"prompts":prompts,
        "promptFeed":{"cursor":{"runtimeEpoch":core.runtime_id,"sourceScopeRevision":scope,"day":day,"sequence":high.to_string()},"baseline":baseline,"dropped":dropped}
    }))
}

fn valid_metric(row: &rusqlite::Row<'_>, state: usize, value: usize) -> Result<Option<i64>> {
    if text(row, state)?.as_deref() != Some("valid") {
        return Ok(None);
    }
    match row.get_ref(value).map_err(err)? {
        ValueRef::Integer(v) => Ok(Some(v)),
        _ => Err("宠物详情计量字段不是有效整数".into()),
    }
}

fn today_overview(core: &Core, day: &str) -> Result<Value> {
    let mut statement = core.db.prepare(
        "SELECT u.event_time,json_extract(u.data,'$.counts.total.state'),json_extract(u.data,'$.counts.total.value'),json_extract(u.data,'$.counts.input.state'),json_extract(u.data,'$.counts.input.value'),json_extract(u.data,'$.counts.cacheRead.state'),json_extract(u.data,'$.counts.cacheRead.value') FROM usage_responses u WHERE NOT EXISTS(SELECT 1 FROM fact_aliases a WHERE a.alias=u.id) AND NOT EXISTS(SELECT 1 FROM excluded_facts e WHERE e.fact_id=u.id)",
    ).map_err(err)?;
    let mut rows = statement.query([]).map_err(err)?;
    let (mut count, mut total, mut input, mut cached) = (0u64, 0i64, 0i64, 0i64);
    while let Some(row) = rows.next().map_err(err)? {
        let time: String = row.get(0).map_err(err)?;
        if !parsed(&time).is_some_and(|at| day_at(at) == day) {
            continue;
        }
        let Some(value) = valid_metric(row, 1, 2)? else {
            continue;
        };
        total = total.checked_add(value).ok_or("宠物详情总量超出整数范围")?;
        count += 1;
        if let (Some(i), Some(c)) = (valid_metric(row, 3, 4)?, valid_metric(row, 5, 6)?) {
            input = input.checked_add(i).ok_or("宠物详情缓存分母超出整数范围")?;
            cached = cached
                .checked_add(c)
                .ok_or("宠物详情缓存分子超出整数范围")?;
        }
    }
    Ok(
        json!({"totalTokens":if count>0{Some(total)}else{None},"cacheHitRate":if input>0{Some(cached as f64/input as f64)}else{None}}),
    )
}

fn optional_number(row: &rusqlite::Row<'_>, column: usize) -> Result<Value> {
    if let Some(kind) = text(row, column + 1)? {
        if !["null", "integer", "real"].contains(&kind.as_str()) {
            return Err("宠物详情耗时不是有效数值".into());
        }
    }
    match row.get_ref(column).map_err(err)? {
        ValueRef::Null => Ok(Value::Null),
        ValueRef::Integer(v) => Ok(json!(v)),
        ValueRef::Real(v) => serde_json::Number::from_f64(v)
            .map(Value::Number)
            .ok_or_else(|| "宠物详情耗时不是有限数值".into()),
        _ => Err("宠物详情耗时不是有效数值".into()),
    }
}

fn current_chat_source(core: &Core) -> Result<bool> {
    let source: Option<(String, bool)> = core.db.query_row(
        "SELECT client_kind,enabled FROM source_instances WHERE id=?1",
        [&core.source_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional().map_err(err)?;
    Ok(source.is_some_and(|(kind, enabled)| {
        enabled && kind == "codex" && adapter::adapter_for(&kind).is_some()
    }))
}

fn verified_chat_member(core: &Core, source: &str, thread: &str) -> Result<bool> {
    if thread.is_empty() {
        return Ok(false);
    }
    let mut statement = core.db.prepare(
        "SELECT CASE WHEN json_type(data,'$.sourceId')='text' THEN json_extract(data,'$.sourceId') END,CASE WHEN json_type(data,'$.id')='text' THEN json_extract(data,'$.id') END FROM thread_metadata WHERE id=?1",
    ).map_err(err)?;
    let mut rows = statement.query([format!("{source}|{thread}")]).map_err(err)?;
    let Some(row) = rows.next().map_err(err)? else { return Ok(false) };
    Ok(text(row, 0)?.as_deref() == Some(source) && text(row, 1)?.as_deref() == Some(thread))
}

fn latest_turn(core: &Core, day: &str) -> Result<(Value, Value)> {
    let chat_available = current_chat_source(core)?;
    let mut statement = core.db.prepare(
        "SELECT rowid,json_extract(data,'$.completedAt'),json_extract(data,'$.startedAt'),json_extract(data,'$.lastEventAt'),json_extract(data,'$.timeToFirstTokenMs'),json_type(data,'$.timeToFirstTokenMs'),CASE WHEN json_type(data,'$.sourceId')='text' THEN json_extract(data,'$.sourceId') END,CASE WHEN json_type(data,'$.threadId')='text' THEN json_extract(data,'$.threadId') END FROM turns ORDER BY rowid",
    ).map_err(err)?;
    let mut rows = statement.query([]).map_err(err)?;
    // Snapshot's stable lastEventAt sort precedes latestTimedTurn's stable Date.parse sort.
    let mut latest: Option<(Option<i64>, String, Value)> = None;
    let mut chat: Option<(Option<i64>, String, Option<String>)> = None;
    while let Some(row) = rows.next().map_err(err)? {
        let completed = text(row, 1)?;
        let started = text(row, 2)?;
        let last_event = text(row, 3)?.unwrap_or_default();
        let order_time = completed.as_ref().or(started.as_ref());
        let day_time = order_time.map(String::as_str).unwrap_or(&last_event);
        if !parsed(day_time).is_some_and(|time| day_at(time) == day) {
            continue;
        }
        // JavaScript Date.parse clips sub-millisecond precision; None means -Infinity.
        let order = order_time
            .and_then(|v| parsed(v))
            .map(|v| v.timestamp_millis());
        if latest.as_ref().is_none_or(|(old_order, old_last, _)| {
            order > *old_order || (order == *old_order && last_event > *old_last)
        }) {
            latest = Some((order, last_event.clone(), optional_number(row, 4)?));
        }
        // Select the source's latest row before validating its thread. An
        // incomplete newest identity must not silently fall back to an older one.
        if chat_available && text(row, 6)?.as_deref() == Some(core.source_id.as_str())
            && chat.as_ref().is_none_or(|(old_order, old_last, _)| {
                order > *old_order || (order == *old_order && last_event > *old_last)
            })
        {
            chat = Some((order, last_event, text(row, 7)?));
        }
    }
    let recent_chat = match chat.and_then(|(_, _, thread)| thread) {
        Some(thread) if verified_chat_member(core, &core.source_id, &thread)? => {
            json!({"sourceId":core.source_id,"threadId":thread})
        }
        _ => Value::Null,
    };
    Ok((latest
        .map(|(_, _, ttft)| json!({"timeToFirstTokenMs":ttft}))
        .unwrap_or(Value::Null), recent_chat))
}

pub(super) fn details(core: &Core, request: &Value) -> Result<Value> {
    details_at(core, request, Utc::now())
}

pub(super) fn validate_domain_at(
    core: &Core,
    request: &Value,
    at: DateTime<Utc>,
) -> Result<(String, String)> {
    if !request.is_object() {
        return Err("宠物详情请求必须为对象".into());
    }
    if request
        .as_object()
        .unwrap()
        .keys()
        .any(|key| !["day", "runtimeEpoch", "sourceScopeRevision"].contains(&key.as_str()))
    {
        return Err("宠物详情不接受统计筛选或未知字段".into());
    }
    let requested_day = request
        .get("day")
        .and_then(Value::as_str)
        .filter(|v| NaiveDate::parse_from_str(v, "%Y-%m-%d").is_ok())
        .ok_or("宠物详情缺少有效日期")?;
    let day = day_at(at);
    let scope = source_scope(core, &day)?;
    if requested_day != day
        || request["runtimeEpoch"].as_str() != Some(core.runtime_id.as_str())
        || request["sourceScopeRevision"].as_str() != Some(scope.as_str())
    {
        return Err("宠物详情范围已过期，请先重新读取宠物状态".into());
    }
    Ok((day, scope))
}

fn chat_recorded_today(core: &Core, source: &str, thread: &str, day: &str) -> Result<bool> {
    // Only the explicit click checks the displayed member's day again. Keep
    // scalar JSON text semantics and do not rebuild Details or the full ledger.
    let mut statement = core.db.prepare(
        "SELECT CASE WHEN json_type(data,'$.sourceId')='text' THEN json_extract(data,'$.sourceId') END,CASE WHEN json_type(data,'$.threadId')='text' THEN json_extract(data,'$.threadId') END,json_extract(data,'$.completedAt'),json_extract(data,'$.startedAt'),json_extract(data,'$.lastEventAt') FROM turns WHERE thread_id=?1",
    ).map_err(err)?;
    let mut rows = statement.query([thread]).map_err(err)?;
    while let Some(row) = rows.next().map_err(err)? {
        if text(row, 0)?.as_deref() != Some(source)
            || text(row, 1)?.as_deref() != Some(thread)
        {
            continue;
        }
        let completed = text(row, 2)?;
        let started = text(row, 3)?;
        let last = text(row, 4)?.unwrap_or_default();
        let time = completed.as_ref().or(started.as_ref()).map(String::as_str).unwrap_or(&last);
        if parsed(time).is_some_and(|time| day_at(time) == day) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn chat_target_at(core: &Core, request: &Value, at: DateTime<Utc>) -> Result<PetChatTarget> {
    let fields = request.as_object().ok_or("宠物聊天请求必须为对象")?;
    if fields.keys().any(|key| !["petDomain", "chat"].contains(&key.as_str())) {
        return Err("宠物聊天请求含未知字段".into());
    }
    let (day, _) = validate_domain_at(core, &request["petDomain"], at)?;
    let chat = request["chat"].as_object().ok_or("宠物聊天缺少已验证身份")?;
    if chat.keys().any(|key| !["sourceId", "threadId"].contains(&key.as_str())) {
        return Err("宠物聊天身份含未知字段".into());
    }
    let source = chat.get("sourceId").and_then(Value::as_str).filter(|id| !id.is_empty())
        .ok_or("宠物聊天缺少有效来源身份")?;
    let thread = chat.get("threadId").and_then(Value::as_str).filter(|id| !id.is_empty())
        .ok_or("宠物聊天缺少有效会话身份")?;
    if source != core.source_id || !current_chat_source(core)? {
        return Err("主来源已变化或停用，请重新读取宠物状态".into());
    }
    if !verified_chat_member(core, source, thread)?
        || !chat_recorded_today(core, source, thread, &day)?
    {
        return Err("此会话未在当前来源今日记录中验证，请重新读取小卡".into());
    }
    match core.classified_chat_target(source, thread) {
        Ok(target) => Ok(PetChatTarget::Open { target, thread_id: thread.into() }),
        Err(ChatTargetFailure::Unlocatable(message)) => {
            Ok(PetChatTarget::CopyRequired { thread_id: thread.into(), message })
        }
        Err(error) => Err(error.message()),
    }
}

pub(super) fn details_at(core: &Core, request: &Value, at: DateTime<Utc>) -> Result<Value> {
    let (day, scope) = validate_domain_at(core, request, at)?;
    let overview = today_overview(core, &day)?;
    let (latest, recent_chat) = latest_turn(core, &day)?;
    let revision = core.next_pet_revision()?;
    Ok(
        json!({"protocolVersion":1,"runtimeEpoch":core.runtime_id,"revision":revision,
        "day":day,"asOf":at.to_rfc3339_opts(chrono::SecondsFormat::Millis,true),
        "sourceScopeRevision":scope,"overview":overview,"latestTurn":latest,"recentChat":recent_chat}),
    )
}

#[cfg(test)]
#[path = "pet_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "pet_card_tests.rs"]
mod card_tests;
