//! 仅保留白名单元数据；摘要和对比均取同一只读账本快照。
use super::*;
use chrono::{NaiveDate, TimeZone};

const DOMAIN: &str = "observed-insights-v1";

pub(crate) fn safe_text(value: &Value, max: usize) -> Option<String> {
    value
        .as_str()
        .filter(|s| {
            !s.trim().is_empty()
                && s.encode_utf16().count() <= max
                && !s.chars().any(|c| {
                    c.is_control() || matches!(c,'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}')
                })
        })
        .map(|s| s.trim().to_owned())
}

/// `compacted` 是 rollout 事实。兼容通知和 item 投影不再计一次压缩。
pub(crate) fn ingest_metadata(
    db: &Connection,
    source: &str,
    time: &str,
    v: &Value,
    ctx: &mut reader::Context,
    within: bool,
) -> Result<()> {
    if !within {
        return Ok(());
    }
    let thread = safe_text(&v["payload"]["thread_id"], 512).unwrap_or_else(|| ctx.thread(v));
    if thread.is_empty() {
        return Ok(());
    }
    let p = &v["payload"];
    let kind = strv(v, "type");
    let ordinal = v["ordinal"].as_u64().map(|v| v.to_string());
    let position = ctx.record_position.map(|(file, generation, offset)| {
        (
            hash(format!("{source}|{thread}|{file}|{generation}").as_bytes()),
            offset.to_string(),
        )
    });
    let record_key = ordinal
        .as_ref()
        .map(|v| format!("ordinal:{v}"))
        .or_else(|| {
            position
                .as_ref()
                .map(|(stream, offset)| format!("position:{stream}:{offset}"))
        });
    let mut data;
    let identity;
    if kind == "compacted" {
        let window = safe_text(&p["window_id"], 128);
        identity = format!(
            "compaction:{}",
            window.as_deref().or(record_key.as_deref()).unwrap_or(time)
        );
        data = json!({"kind":"compaction","windowId":window,
            "previousWindowId":safe_text(&p["previous_window_id"],128),"windowNumber":p["window_number"].as_u64().map(|v|v.to_string()),
            "turnId":if ctx.current_turn.is_empty(){None}else{Some(&ctx.current_turn)}});
    } else {
        let fields = if kind == "turn_context" {
            p
        } else if kind == "event_msg" && p["type"] == "thread_settings_applied" {
            &p["thread_settings"]
        } else {
            return Ok(());
        };
        if !fields.is_object() || safe_text(&fields["model"], 512).is_none() {
            return Ok(());
        }
        if kind == "event_msg" && safe_text(&fields["model_provider_id"], 512).is_none() {
            return Ok(());
        }
        let mut condition = serde_json::Map::new();
        if kind == "event_msg" {
            // 官方 thread_settings 是完整快照，None 在 JSON 中省略，须清掉旧可选配置。
            for key in ["model", "providerId", "effort", "speed"] {
                condition.insert(key.into(), Value::Null);
            }
        }
        for (out, names) in [
            ("model", &["model"][..]),
            ("providerId", &["model_provider_id", "model_provider"][..]),
            ("effort", &["reasoning_effort", "effort"][..]),
            ("speed", &["service_tier"][..]),
        ] {
            if let Some(value) = names.iter().find_map(|key| fields.get(*key)) {
                if value.is_null() && kind == "event_msg" {
                    condition.insert(out.into(), Value::Null);
                } else if let Some(value) = safe_text(value, 512) {
                    condition.insert(out.into(), json!(value));
                }
            }
        }
        if condition.is_empty() {
            return Ok(());
        }
        let previous = ctx
            .observed_settings
            .get(&thread)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let mut current = previous.clone();
        current.extend(condition);
        if current == previous {
            return Ok(());
        }
        let changed: Vec<_> = current
            .keys()
            .filter(|key| previous.get(*key) != current.get(*key))
            .cloned()
            .collect();
        identity = format!(
            "settings:{}:{time}:{}",
            record_key.as_deref().unwrap_or("sequence-unknown"),
            hash(Value::Object(current.clone()).to_string().as_bytes())
        );
        data = json!({"kind":if previous.is_empty(){"settings_snapshot"}else{"settings_change"},
            "condition":current,"changedFields":changed,"turnId":safe_text(&p["turn_id"],128)});
        if ctx.observed_settings.len() >= 128 && !ctx.observed_settings.contains_key(&thread) {
            ctx.observed_settings.clear();
        }
        ctx.observed_settings
            .insert(thread.clone(), Value::Object(current));
    }
    let id = hash(format!("{DOMAIN}|{source}|{thread}|{identity}").as_bytes());
    data["id"] = json!(id);
    data["domain"] = json!(DOMAIN);
    data["eventTime"] = json!(time);
    data["sourceId"] = json!(source);
    data["threadId"] = json!(thread);
    data["ordinal"] = json!(ordinal);
    data["recordStream"] = json!(position.as_ref().map(|(stream, _)| stream));
    data["recordPosition"] = json!(position.as_ref().map(|(_, offset)| offset));
    db.execute(
        "INSERT OR IGNORE INTO context_snapshots(id,source_id,event_time,data) VALUES(?1,?2,?3,?4)",
        params![id, source, time, data.to_string()],
    )
    .map_err(err)?;
    Ok(())
}

#[derive(Default)]
struct Cache {
    input: i128,
    read: i128,
    count: usize,
    unknown: usize,
}
impl Cache {
    fn add(&mut self, r: &ResponseUsage) -> Result<()> {
        match (r.counts.input.value(), r.counts.cache_read.value()) {
            (Some(input), Some(read)) if input >= 0 && read >= 0 && read <= input => {
                self.input = self
                    .input
                    .checked_add(i128::from(input))
                    .ok_or("缓存输入汇总溢出")?;
                self.read = self
                    .read
                    .checked_add(i128::from(read))
                    .ok_or("缓存读取汇总溢出")?;
                self.count += 1;
            }
            _ => self.unknown += 1,
        }
        Ok(())
    }
    fn value(&self) -> Value {
        json!({"inputTokens":if self.count>0{Some(self.input.to_string())}else{None},
            "cachedInputTokens":if self.count>0{Some(self.read.to_string())}else{None},
            "cacheReadPercent":if self.input>0{Some(self.read as f64/self.input as f64*100.0)}else{None},
            "knownResponses":self.count,"unknownResponses":self.unknown,"basis":"paired-valid-input-cache-subset"})
    }
}
#[derive(Default)]
struct Daily {
    sum: Sum,
    cache: Cache,
    cost: i128,
    priced: usize,
}
#[derive(Default)]
struct ModelStats {
    count: usize,
    cache: Cache,
    ttft: i128,
    timed: usize,
    cost: i128,
    priced: usize,
}

fn event_order(a: &Value, b: &Value) -> std::cmp::Ordering {
    let ordinal = |v: &Value| v["ordinal"].as_str().and_then(|n| n.parse::<u64>().ok());
    let position = |v: &Value| {
        v["recordPosition"]
            .as_str()
            .and_then(|n| n.parse::<u64>().ok())
    };
    b["eventTime"]
        .as_str()
        .cmp(&a["eventTime"].as_str())
        .then_with(|| strv(a, "sourceId").cmp(&strv(b, "sourceId")))
        .then_with(|| strv(a, "threadId").cmp(&strv(b, "threadId")))
        .then_with(|| ordinal(b).is_some().cmp(&ordinal(a).is_some()))
        .then_with(|| ordinal(b).cmp(&ordinal(a)))
        .then_with(|| strv(a, "recordStream").cmp(&strv(b, "recordStream")))
        .then_with(|| position(b).cmp(&position(a)))
        .then_with(|| strv(a, "id").cmp(&strv(b, "id")))
}

pub(super) fn project(
    core: &QueryReadContext<'_>,
    loaded: &Loaded,
    q: &Value,
    from: &str,
    to: &str,
    facts: &[usage_dashboard::Fact<'_>],
) -> Result<Value> {
    let privacy = core.settings()?["privacyMode"] == true;
    let first = NaiveDate::parse_from_str(from, "%Y-%m-%d").map_err(err)?;
    let last = NaiveDate::parse_from_str(to, "%Y-%m-%d").map_err(err)?;
    let offset = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
    let start = offset
        .from_local_datetime(&first.and_hms_opt(0, 0, 0).unwrap())
        .single()
        .ok_or("日期无效")?
        .with_timezone(&Utc);
    let end = offset
        .from_local_datetime(
            &last
                .succ_opt()
                .ok_or("日期越界")?
                .and_hms_opt(0, 0, 0)
                .unwrap(),
        )
        .single()
        .ok_or("日期无效")?
        .with_timezone(&Utc);
    let mut days = BTreeMap::<String, Daily>::new();
    let mut day = first;
    while day <= last {
        days.insert(day.to_string(), Daily::default());
        day = day.succ_opt().ok_or("日期越界")?;
    }
    let mut cache = Cache::default();
    let mut cache_timeline = Vec::new();
    let mut cache_truncated = false;
    let mut groups = BTreeMap::<String, (Value, BTreeMap<String, ModelStats>)>::new();
    let mut excluded = 0usize;
    let mut comparison_truncated = false;
    let turns: BTreeMap<_, _> = loaded
        .turns
        .iter()
        .map(|t| ((strv(t, "sourceId"), strv(t, "threadId"), strv(t, "id")), t))
        .collect();
    for fact in facts {
        let r = fact.response;
        let day = fact.time.date_naive().to_string();
        let Some(daily) = days.get_mut(&day) else {
            continue;
        };
        daily.sum.add(r)?;
        daily.cache.add(r)?;
        cache.add(r)?;
        let cost = &fact.cost;
        if let Some(units) = cost.units {
            daily.cost = daily.cost.checked_add(units).ok_or("每日成本汇总溢出")?;
            daily.priced += 1;
        }
        if !privacy {
            cache_timeline.push(json!({"id":hash(adapter::response_key(r).as_bytes()),"kind":"cache_usage",
            "eventTime":r.event_time,"sourceId":r.source_id,"threadId":r.thread_id,"observedModel":r.observed_model,
            "inputTokens":r.counts.input.value().map(|v|v.to_string()),"cachedInputTokens":r.counts.cache_read.value().map(|v|v.to_string())}));
        }
        if cache_timeline.len() > 512 {
            cache_truncated = true;
            cache_timeline.sort_by(|a, b| {
                b["eventTime"]
                    .as_str()
                    .cmp(&a["eventTime"].as_str())
                    .then_with(|| strv(a, "id").cmp(&strv(b, "id")))
            });
            cache_timeline.truncate(256);
        }
        if privacy {
            continue;
        }
        let turnkey = (
            r.source_id.clone(),
            r.thread_id.clone(),
            r.turn_id.clone().unwrap_or_default(),
        );
        let turn = turns.get(&turnkey).copied();
        let model = r
            .observed_model
            .as_deref()
            .filter(|v| !v.is_empty() && *v != "unknown");
        let effort = turn
            .and_then(|v| v["reasoningEffort"].as_str())
            .filter(|v| !v.is_empty())
            .unwrap_or("unknown");
        let speed = turn
            .and_then(|v| v["serviceTier"].as_str())
            .filter(|v| !v.is_empty())
            .unwrap_or("unknown");
        let input = r.counts.input.value().filter(|n| *n > 0);
        let read = r.counts.cache_read.value();
        let provider = r
            .provider_id
            .as_deref()
            .filter(|v| !v.is_empty())
            .unwrap_or("unknown");
        let account = r
            .account_scope_id
            .as_deref()
            .filter(|v| !v.is_empty() && *v != "unknown");
        let (Some(model), Some(input), Some(read)) = (model, input, read) else {
            excluded += 1;
            continue;
        };
        if read < 0 || read > input {
            excluded += 1;
            continue;
        }
        let known =
            effort != "unknown" && speed != "unknown" && provider != "unknown" && account.is_some();
        let condition = json!({"sourceId":r.source_id,"providerId":provider,"accountScope":account.map(|v|hash(v.as_bytes())),"known":known,
            "effort":effort,"speed":speed,"inputMagnitude":input.ilog10(),"cacheDecile":(i128::from(read)*10/i128::from(input)).min(9)});
        let key = condition.to_string();
        if groups.len() >= 128 && !groups.contains_key(&key) {
            comparison_truncated = true;
            excluded += 1;
            continue;
        }
        if groups
            .get(&key)
            .is_some_and(|(_, models)| models.len() >= 64 && !models.contains_key(model))
        {
            comparison_truncated = true;
            excluded += 1;
            continue;
        }
        let stats = groups
            .entry(key)
            .or_insert_with(|| (condition, BTreeMap::new()))
            .1
            .entry(model.into())
            .or_default();
        stats.count += 1;
        stats.cache.add(r)?;
        if loaded.turn_response_counts.get(&turnkey) == Some(&1) {
            if let Some(ms) = turn
                .and_then(|v| v["timeToFirstTokenMs"].as_i64())
                .filter(|n| *n >= 0)
            {
                stats.ttft = stats
                    .ttft
                    .checked_add(i128::from(ms))
                    .ok_or("计时汇总溢出")?;
                stats.timed += 1;
            }
        }
        if let Some(units) = cost.units {
            stats.cost = stats.cost.checked_add(units).ok_or("成本汇总溢出")?;
            stats.priced += 1;
        }
    }
    let daily:Vec<_>=days.into_iter().map(|(day,d)|{
        let text=if d.sum.count==0 {format!("{day}：当前筛选未记录有效响应；不能据此判断实际没有使用。")}
            else{format!("{day}：当前筛选记录 {} 条有效响应；缓存比例仅覆盖 {} 条输入与缓存字段均有效的记录，另有 {} 条未知。",d.sum.count,d.cache.count,d.cache.unknown)};
        json!({"day":day,"usage":d.sum.overview(),"recordedTokens":if d.sum.count>0{Some(d.sum.total.to_string())}else{None},"cache":d.cache.value(),"text":text,
            "estimatedCostUsd":if d.priced>0{Some(pricing::format_usd(d.cost))}else{None},"pricedResponses":d.priced})
    }).collect();
    let mut timeline = Vec::new();
    let mut truncated = cache_truncated;
    if !privacy {
        let source = q["sourceId"].as_str().unwrap_or("");
        let thread = q["threadId"].as_str().unwrap_or("");
        let mut stmt=core.db.prepare("SELECT data FROM context_snapshots WHERE event_time>=?1 AND event_time<?2 AND (?3='' OR source_id=?3) AND json_valid(data) AND json_extract(data,'$.domain')=?4 AND (?5='' OR json_extract(data,'$.threadId')=?5) ORDER BY event_time DESC,id DESC LIMIT 4097").map_err(err)?;
        let rows = stmt
            .query_map(
                params![
                    start.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                    end.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                    source,
                    DOMAIN,
                    thread
                ],
                |r| r.get::<_, String>(0),
            )
            .map_err(err)?;
        for (i, row) in rows.enumerate() {
            if i == 4096 {
                truncated = true;
                break;
            }
            let v: Value = serde_json::from_str(&row.map_err(err)?).map_err(err)?;
            if scoped_with_provider(
                q,
                &strv(&v, "sourceId"),
                &strv(&v, "threadId"),
                &strv(&v, "turnId"),
                v.pointer("/condition/model").and_then(Value::as_str),
                v.pointer("/condition/providerId").and_then(Value::as_str),
                &loaded.sessions,
                &loaded.metadata,
                &loaded.sources,
            ) {
                timeline.push(v);
            }
        }
        timeline.extend(cache_timeline);
        timeline.sort_by(event_order);
        if timeline.len() > 256 {
            truncated = true;
            timeline.truncate(256);
        }
    }
    let comparisons:Vec<_>=groups.into_values().map(|(condition,models)|{
        let eligible=models.values().filter(|m|m.count>=5).count();
        let models:Vec<_>=models.into_iter().map(|(model,m)|json!({"model":model,"samples":m.count,"eligible":m.count>=5,
            "cache":m.cache.value(),"timedSamples":m.timed,"meanTurnTimeToFirstTokenMs":if m.timed>=5{Some(m.ttft as f64/m.timed as f64)}else{None},
            "pricedSamples":m.priced,"meanEstimatedCostUsd":if m.priced>=5{Some(pricing::format_usd(m.cost/i128::try_from(m.priced).unwrap()))}else{None}})).collect();
        let known=condition["known"]==true;
        json!({"condition":condition,"comparable":known&&eligible>=2,"models":models,"quality":"conditional-observed","rank":Value::Null,
            "reasons":if !known{vec!["condition_incomplete","observational_not_causal"]}else if eligible>=2{vec!["observational_not_causal"]}else{vec!["insufficient_comparable_samples"]}})
    }).take(128).collect();
    Ok(
        json!({"schemaVersion":1,"from":from,"to":to,"timezone":"Asia/Shanghai","cache":cache.value(),"daily":daily,
        "timeline":timeline,"timelineTruncated":truncated,"comparisons":comparisons,"comparisonTruncated":comparison_truncated,"excludedComparisonResponses":excluded,
        "reasons":if privacy{vec!["privacy_mode","local_recorded_only"]}else{vec!["local_recorded_only","timeline_forward_collection","quality_not_measured"]}}),
    )
}

#[cfg(test)]
#[path = "observed_insights_tests.rs"]
mod tests;
