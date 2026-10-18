use super::adapter::{response_key, Metric, ResponseUsage};
use super::*;
use std::collections::{BTreeMap, BTreeSet};

#[path = "usage_dashboard.rs"]
mod usage_dashboard;

#[path = "budget.rs"]
pub(super) mod budget;

#[cfg(test)]
#[path = "health_tests.rs"]
mod health_tests;

#[cfg(test)]
#[path = "session_scope_tests.rs"]
mod session_scope_tests;

fn local_day(time: &str) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(time).ok().map(|t| {
        t.with_timezone(&chrono::FixedOffset::east_opt(8 * 3600).unwrap())
            .format("%Y-%m-%d")
            .to_string()
    })
}
fn matches(v: &Value, k: &str, s: &str) -> bool {
    v.get(k)
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .map(|v| v == s)
        .unwrap_or(true)
}
fn validate_query_dates(q: &Value) -> Result<(Option<chrono::NaiveDate>, Option<chrono::NaiveDate>)> {
    if !q.is_object() {
        return Err("统计筛选必须是对象".into());
    }
    let parse = |key: &str| -> Result<Option<chrono::NaiveDate>> {
        q.get(key).map(|value| {
            let text = value.as_str().ok_or_else(|| format!("{key} 必须是 YYYY-MM-DD 日期"))?;
            let date = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d")
                .map_err(|_| format!("{key} 必须是有效 YYYY-MM-DD 日期"))?;
            if text.len() != 10 || date.format("%Y-%m-%d").to_string() != text {
                return Err(format!("{key} 必须是 YYYY-MM-DD 日期"));
            }
            Ok(date)
        }).transpose()
    };
    let dates = (parse("from")?, parse("to")?);
    if matches!(dates, (Some(from), Some(to)) if from > to) {
        return Err("开始日期不得晚于结束日期".into());
    }
    Ok(dates)
}
fn in_dates(q: &Value, time: &str) -> bool {
    let Some(day) = local_day(time) else {
        return false;
    };
    let from = q.get("from").and_then(Value::as_str).unwrap_or("");
    let to = q.get("to").and_then(Value::as_str).unwrap_or("9999-12-31");
    day.as_str() >= from && day.as_str() <= to
}
fn project_for(cwd: &str, mapping: &Value) -> String {
    let normalized = cwd.replace('\\', "/").trim_end_matches('/').to_lowercase();
    let mut best: Option<(usize, String)> = None;
    if let Some(m) = mapping.as_object() {
        for (path, v) in m {
            let p = path.replace('\\', "/").trim_end_matches('/').to_lowercase();
            if normalized == p || normalized.starts_with(&format!("{p}/")) {
                if best.as_ref().map(|(n, _)| p.len() > *n).unwrap_or(true) {
                    if let Some(name) = v.as_str() {
                        best = Some((p.len(), name.into()));
                    }
                }
            }
        }
    }
    best.map(|(_, s)| format!("mapped:{s}")).unwrap_or_else(|| {
        if cwd.is_empty() {
            "unknown".into()
        } else {
            format!("path:{normalized}")
        }
    })
}
fn project_name(id: &str, path: &str) -> String {
    if let Some(name) = id.strip_prefix("mapped:") {
        name.into()
    } else if id == "unknown" {
        "项目未知".into()
    } else {
        path.replace('\\', "/")
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(path)
            .into()
    }
}

// Read the collector's existing record without inventing zero counts for an
// absent, malformed, or not-yet-completed observation. No age policy is added.
struct SourceCheck {
    issues: u64,
    discovery_complete: bool,
    discovery_errors: u64,
    cancelled: bool,
}

impl SourceCheck {
    fn discovery_finished(&self) -> bool {
        self.discovery_complete && self.discovery_errors == 0 && !self.cancelled
    }
}

fn source_check(health: &Value) -> Option<SourceCheck> {
    chrono::DateTime::parse_from_rfc3339(health["checkedAt"].as_str()?).ok()?;
    health["pendingFiles"].as_u64()?;
    health["files"].as_u64()?;
    Some(SourceCheck {
        issues: health["issues"].as_u64()?,
        discovery_complete: health["discoveryComplete"].as_bool()?,
        discovery_errors: health["discoveryErrors"].as_u64()?,
        cancelled: health["cancelled"].as_bool()?,
    })
}

/// Shared by the full source view and the pet's small source-scope projection.
pub(super) fn source_runtime(root: &Path, enabled: bool, health: Option<&Value>) -> &'static str {
    if !enabled {
        "未配置"
    } else if !root.is_dir() {
        "不可用"
    } else if let Some(check) = health.and_then(source_check) {
        if check.issues == 0 && check.discovery_finished() {
            "可用"
        } else {
            "不可用"
        }
    } else {
        "未知"
    }
}

pub(super) fn source_rows(core: &Core) -> Result<Vec<Value>> {
    let mut s = core
        .db
        .prepare("SELECT id,root,client_kind,enabled,environment,created_at FROM source_instances")
        .map_err(err)?;
    let mut sources=s.query_map([],|r|Ok(json!({"id":r.get::<_,String>(0)?,"root":r.get::<_,String>(1)?,"clientKind":r.get::<_,String>(2)?,"enabled":r.get::<_,bool>(3)?,"environment":r.get::<_,String>(4)?,"createdAt":r.get::<_,String>(5)?}))).map_err(err)?.collect::<std::result::Result<Vec<_>,_>>().map_err(err)?;
    for source in &mut sources {
        let id = strv(source, "id");
        source["primary"] = json!(id == core.source_id);
        let mut health = get_setting(&core.db, &format!("lastSourceCheck:{id}"))?
            .filter(Value::is_object)
            .unwrap_or(json!({"checkedAt":null,"pendingFiles":null,"issues":null,"files":null}));
        if health.get("unsupportedFiles").is_none() {
            health["unsupportedFiles"] = Value::Null;
        }
        health["lastSuccessfulCheck"] =
            get_setting(&core.db, &format!("lastSourceSuccess:{id}"))?.unwrap_or(Value::Null);
        health["lastEvent"] =
            get_setting(&core.db, &format!("sourceLastEvent:{id}"))?.unwrap_or(Value::Null);
        let (from,to):(Option<String>,Option<String>)=core.db.query_row("SELECT MIN(event_time),MAX(event_time) FROM (SELECT event_time FROM usage_responses WHERE source_id=?1 UNION ALL SELECT event_time FROM legacy_usage_segments WHERE source_id=?1 UNION ALL SELECT event_time FROM turns WHERE json_extract(data,'$.sourceId')=?1 UNION ALL SELECT event_time FROM items WHERE json_extract(data,'$.sourceId')=?1)",[&id],|r|Ok((r.get(0)?,r.get(1)?))).map_err(err)?;
        health["observedFrom"] = json!(from);
        health["observedTo"] = json!(to);
        let unknown:i64=core.db.query_row("SELECT COUNT(*) FROM usage_responses WHERE source_id=?1 AND COALESCE(NULLIF(json_extract(data,'$.observedModel'),''),NULLIF(json_extract(data,'$.requestedModel'),'')) IS NULL",[&id],|r|r.get(0)).map_err(err)?;
        health["unknownModels"] = if health["checkedAt"].is_null() && from.is_none() {
            Value::Null
        } else {
            json!(unknown)
        };
        health["coverageStatus"] = json!("observed-event-range");
        health["coverageComplete"] = json!(false);
        source["health"] = health;
        source["runtime"] = json!(source_runtime(
            Path::new(&strv(source, "root")),
            source["enabled"].as_bool() == Some(true),
            Some(&source["health"]),
        ));
    }
    Ok(sources)
}
struct Loaded {
    responses: Vec<ResponseUsage>,
    all_count: usize,
    excluded_count: usize,
    sessions: BTreeMap<String, Value>,
    turns: Vec<Value>,
    sources: Vec<Value>,
    metadata: BTreeMap<String, Value>,
    recorded_sessions: BTreeMap<String, usize>,
    // Counts use the complete source/thread/turn identity before query filtering.
    // A filtered view must not make a multi-response turn appear single-response.
    turn_response_counts: BTreeMap<(String, String, String), usize>,
    health_usage: Value,
}
// Every caller supplies a static internal table name. Keep only one decoded
// row alive until its visitor either retains it or finishes using it.
fn for_each_json_value(
    db: &Connection,
    table: &'static str,
    mut visit: impl FnMut(Value) -> Result<()>,
) -> Result<()> {
    let mut statement = db
        .prepare(&format!("SELECT data FROM {table}"))
        .map_err(err)?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(err)?;
    for text in rows {
        visit(serde_json::from_str(&text.map_err(err)?).map_err(err)?)?;
    }
    Ok(())
}
fn load(core: &Core, q: &Value) -> Result<Loaded> {
    agent_kind::validate_query(q)?;
    // Snapshot, dashboard and CSV must reject the same malformed date filter.
    // An empty input is not an instruction to silently widen the export.
    validate_query_dates(q)?;
    let sources = source_rows(core)?;
    let source_map: BTreeMap<_, _> = sources.iter().map(|s| (strv(s, "id"), s.clone())).collect();
    let mapping = get_setting(&core.db, "projectMappings")?.unwrap_or(json!({}));
    let mut sessions = BTreeMap::new();
    for mut s in json_rows(&core.db, "thread_metadata")? {
        agent_kind::project(&mut s);
        s.as_object_mut().unwrap().remove("agentKindEvidence");
        let key = format!("{}|{}", strv(&s, "sourceId"), strv(&s, "id"));
        let project = project_for(&strv(&s, "cwd"), &mapping);
        s["project"] = json!(project);
        s["projectName"] = json!(project_name(
            s["project"].as_str().unwrap_or("unknown"),
            &strv(&s, "cwd")
        ));
        // No trusted usage record is different from a recorded zero-token response.
        s["totalTokens"] = Value::Null;
        s["responseCount"] = json!(0);
        sessions.insert(key, s);
    }
    // A root requires positive primary evidence at the end of a complete chain.
    // An absent parent, missing ancestor or conflict cannot prove a root.
    let original = sessions.clone();
    for s in sessions.values_mut() {
        let source = strv(s, "sourceId");
        let mut id = strv(s, "id");
        let mut seen = BTreeSet::new();
        let mut root = None;
        for _ in 0..64 {
            if !seen.insert(id.clone()) {
                root = None;
                break;
            }
            let key = format!("{source}|{id}");
            let Some(ancestor) = original.get(&key) else {
                break;
            };
            if ancestor["agentKindReason"] == "relationship_conflict" {
                break;
            }
            match ancestor.get("parentThreadId").and_then(Value::as_str) {
                Some(p) if !p.is_empty() => {
                    id = p.into();
                }
                _ => {
                    if ancestor["agentKind"] == "primary" {
                        root = Some(id.clone());
                    }
                    break;
                }
            }
        }
        s["rootThreadId"] = json!(root);
    }
    drop(original);
    let turns = json_rows(&core.db, "turns")?;
    let turn_map: BTreeMap<_, _> = turns
        .iter()
        .map(|t| (format!("{}|{}", strv(t, "sourceId"), strv(t, "id")), t))
        .collect();
    let mut ex = core
        .db
        .prepare("SELECT fact_id FROM excluded_facts")
        .map_err(err)?;
    let excluded = ex
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(err)?
        .collect::<std::result::Result<BTreeSet<_>, _>>()
        .map_err(err)?;
    let mut al = core
        .db
        .prepare("SELECT alias FROM fact_aliases")
        .map_err(err)?;
    let aliases = al
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(err)?
        .collect::<std::result::Result<BTreeSet<_>, _>>()
        .map_err(err)?;
    let mut responses = Vec::new();
    let mut metadata = BTreeMap::new();
    let mut recorded_sessions = BTreeMap::new();
    for t in &turns {
        let source = strv(t, "sourceId");
        let thread = strv(t, "threadId");
        let turn = strv(t, "id");
        let root = t
            .get("rootTurnId")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                if agent_kind::prevents_root_fallback(sessions.get(&format!("{source}|{thread}"))) {
                    None
                } else {
                    Some(turn.clone())
                }
            });
        metadata.insert(format!("{source}|{thread}|{turn}"),json!({"model":t.get("model").and_then(Value::as_str).unwrap_or("unknown"),"modelFromTurn":t.get("model").and_then(Value::as_str).is_some(),"providerId":"unknown","rootTurnId":root,"providerCandidates":[],"modelCandidates":[]}));
    }
    let mut all_count = 0;
    let mut excluded_count = 0;
    let mut turn_response_counts = BTreeMap::new();
    let mut health_from = None::<String>;
    let mut health_to = None::<String>;
    let mut health_unknown_models = 0usize;
    let mut health_excluded = 0usize;
    // Preserve the complete row traversal: out-of-scope responses still establish
    // model/provider metadata used to filter turns, tools and inbox entries.
    // Decode each persisted row directly instead of retaining a whole-table JSON DOM.
    let mut response_statement = core
        .db
        .prepare("SELECT data FROM usage_responses")
        .map_err(err)?;
    let response_rows = response_statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(err)?;
    for text in response_rows {
        let mut r: ResponseUsage = serde_json::from_str(&text.map_err(err)?).map_err(err)?;
        if aliases.contains(&response_key(&r)) {
            continue;
        }
        if let Some(turn) = &r.turn_id {
            add_count(
                turn_response_counts
                    .entry((r.source_id.clone(), r.thread_id.clone(), turn.clone()))
                    .or_insert(0),
                1,
                "回合响应数量",
            )?;
        }
        if r.requested_model.is_none() {
            if let Some(id) = &r.turn_id {
                r.requested_model = turn_map
                    .get(&format!("{}|{id}", r.source_id))
                    .and_then(|t| t.get("model"))
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
        }
        let session = sessions.get(&format!("{}|{}", r.source_id, r.thread_id));
        if r.root_turn_id.is_none() {
            r.root_turn_id = r
                .turn_id
                .as_ref()
                .and_then(|turn| metadata.get(&format!("{}|{}|{turn}", r.source_id, r.thread_id)))
                .and_then(|t| t["rootTurnId"].as_str())
                .map(str::to_owned)
                .or_else(|| {
                    if agent_kind::prevents_root_fallback(session) {
                        None
                    } else {
                        r.turn_id.clone()
                    }
                });
        }
        let meta=metadata.entry(format!("{}|{}|{}",r.source_id,r.thread_id,r.turn_id.as_deref().unwrap_or(""))).or_insert_with(||json!({"model":"unknown","modelFromTurn":false,"providerId":"unknown","rootTurnId":r.root_turn_id,"providerCandidates":[],"modelCandidates":[]}));
        for (field, candidates, value) in [
            (
                "providerId",
                "providerCandidates",
                r.provider_id.as_deref().unwrap_or("unknown"),
            ),
            (
                "model",
                "modelCandidates",
                r.requested_model.as_deref().unwrap_or("unknown"),
            ),
        ] {
            let list = meta[candidates].as_array_mut().unwrap();
            if !list.iter().any(|v| v.as_str() == Some(value)) {
                list.push(json!(value));
            }
            let resolved = if list.len() == 1 {
                list[0].clone()
            } else {
                json!("unknown")
            };
            if field != "model" || meta["modelFromTurn"].as_bool() != Some(true) {
                meta[field] = resolved;
            }
        }
        let model = r.requested_model.as_deref().unwrap_or("unknown");
        if excluded.contains(&response_key(&r)) || r.counts.total.value().is_none() {
            add_count(&mut health_excluded, 1, "排除响应数量")?;
        } else {
            add_count(
                &mut health_unknown_models,
                usize::from(model == "unknown"),
                "未知模型响应数量",
            )?;
            if health_from.as_ref().is_none_or(|time| r.event_time < *time) {
                health_from = Some(r.event_time.clone());
            }
            if health_to.as_ref().is_none_or(|time| r.event_time > *time) {
                health_to = Some(r.event_time.clone());
            }
        }
        let source = source_map.get(&r.source_id);
        let project = session
            .map(|s| strv(s, "project"))
            .unwrap_or_else(|| "unknown".into());
        if !in_dates(q, &r.event_time)
            || !matches(q, "sourceId", &r.source_id)
            || !matches(q, "threadId", &r.thread_id)
            || !matches(q, "model", model)
            || !matches(q, "project", &project)
            || !agent_kind::matches(q, session)
            || !matches(
                q,
                "clientKind",
                &source.map(|s| strv(s, "clientKind")).unwrap_or_default(),
            )
            || !matches(
                q,
                "providerId",
                r.provider_id.as_deref().unwrap_or("unknown"),
            )
            || !matches(q, "rootTurnId", r.root_turn_id.as_deref().unwrap_or(""))
        {
            continue;
        }
        add_count(&mut all_count, 1, "已记录响应数量")?;
        add_count(
            recorded_sessions
                .entry(format!("{}|{}", r.source_id, r.thread_id))
                .or_insert(0),
            1,
            "会话已记录响应数量",
        )?;
        if excluded.contains(&response_key(&r)) || r.counts.total.value().is_none() {
            add_count(&mut excluded_count, 1, "筛选范围内排除响应数量")?;
            continue;
        }
        responses.push(r);
    }
    drop(response_statement);
    drop(turn_map);
    Ok(Loaded {
        responses,
        all_count,
        excluded_count,
        sessions,
        turns,
        sources,
        metadata,
        recorded_sessions,
        turn_response_counts,
        health_usage: json!({"coverageFrom":health_from,"coverageTo":health_to,"unknownModels":health_unknown_models,"excludedResponses":health_excluded}),
    })
}
fn scoped(
    q: &Value,
    source: &str,
    thread: &str,
    turn: &str,
    explicit_model: Option<&str>,
    sessions: &BTreeMap<String, Value>,
    metadata: &BTreeMap<String, Value>,
    sources: &[Value],
) -> bool {
    let session = sessions.get(&format!("{source}|{thread}"));
    let meta = metadata.get(&format!("{source}|{thread}|{turn}"));
    let client = sources
        .iter()
        .find(|s| strv(s, "id") == source)
        .map(|s| strv(s, "clientKind"))
        .unwrap_or_else(|| "unknown".into());
    matches(q, "sourceId", source)
        && agent_kind::matches(q, session)
        && matches(q, "threadId", thread)
        && matches(q, "clientKind", &client)
        && matches(
            q,
            "project",
            session
                .and_then(|s| s["project"].as_str())
                .unwrap_or("unknown"),
        )
        && matches(
            q,
            "model",
            explicit_model
                .or_else(|| meta.and_then(|m| m["model"].as_str()))
                .unwrap_or("unknown"),
        )
        && matches(
            q,
            "providerId",
            meta.and_then(|m| m["providerId"].as_str())
                .unwrap_or("unknown"),
        )
        && matches(
            q,
            "rootTurnId",
            meta.and_then(|m| m["rootTurnId"].as_str()).unwrap_or(""),
        )
}
fn add_tokens(total: &mut i64, value: i64, metric: &str) -> Result<()> {
    *total = total
        .checked_add(value)
        .ok_or_else(|| format!("{metric}汇总超出整数范围，原始用量记录已保留"))?;
    Ok(())
}
fn add_count(total: &mut usize, value: usize, metric: &str) -> Result<()> {
    *total = total
        .checked_add(value)
        .ok_or_else(|| format!("{metric}汇总超出整数范围，原始用量记录已保留"))?;
    Ok(())
}
#[derive(Default)]
struct Sum {
    input: i64,
    output: i64,
    total: i64,
    cache: i64,
    cache_write: i64,
    reasoning: i64,
    normal: i64,
    count: usize,
    cache_count: usize,
    cache_input: i64,
    write_count: usize,
    reasoning_count: usize,
    normal_count: usize,
}
impl Sum {
    fn add(&mut self, r: &ResponseUsage) -> Result<()> {
        add_tokens(
            &mut self.input,
            r.counts.input.value().unwrap_or(0),
            "输入 token",
        )?;
        add_tokens(
            &mut self.output,
            r.counts.output.value().unwrap_or(0),
            "输出 token",
        )?;
        add_tokens(
            &mut self.total,
            r.counts.total.value().unwrap_or(0),
            "总 token",
        )?;
        add_count(&mut self.count, 1, "响应数量")?;
        if let (Some(c), Some(i)) = (r.counts.cache_read.value(), r.counts.input.value()) {
            add_tokens(&mut self.cache, c, "缓存读取 token")?;
            add_tokens(&mut self.cache_input, i, "缓存命中率分母")?;
            add_count(&mut self.cache_count, 1, "缓存读取响应数量")?;
        }
        if let Some(w) = r.counts.cache_write.value() {
            add_tokens(&mut self.cache_write, w, "缓存写入 token")?;
            add_count(&mut self.write_count, 1, "缓存写入响应数量")?;
        }
        if let Some(n) = r.counts.reasoning.value() {
            add_tokens(&mut self.reasoning, n, "推理输出 token")?;
            add_count(&mut self.reasoning_count, 1, "推理输出响应数量")?;
        }
        if let (Some(i), Some(c), Some(w)) = (
            r.counts.input.value(),
            r.counts.cache_read.value(),
            r.counts.cache_write.value(),
        ) {
            if let Some(n) = i
                .checked_sub(c)
                .and_then(|n| n.checked_sub(w))
                .filter(|n| *n >= 0)
            {
                add_tokens(&mut self.normal, n, "普通输入 token")?;
                add_count(&mut self.normal_count, 1, "普通输入响应数量")?;
            }
        }
        Ok(())
    }
    fn overview(&self) -> Value {
        json!({"inputTokens":if self.count>0{Some(self.input)}else{None},"outputTokens":if self.count>0{Some(self.output)}else{None},"totalTokens":if self.count>0{Some(self.total)}else{None},"cachedInputTokens":if self.cache_count>0{Some(self.cache)}else{None},"cacheWriteInputTokens":if self.write_count>0{Some(self.cache_write)}else{None},"reasoningOutputTokens":if self.reasoning_count>0{Some(self.reasoning)}else{None},"responseCount":self.count,"cacheHitRate":if self.cache_input>0{Some(self.cache as f64/self.cache_input as f64)}else{None},"cacheCoverage":if self.input>0{Some(self.cache_input as f64/self.input as f64)}else{None},"normalInputTokens":if self.normal_count>0{Some(self.normal)}else{None},"cacheWriteCoverage":if self.count>0{Some(self.write_count as f64/self.count as f64)}else{None},"normalInputCoverage":if self.count>0{Some(self.normal_count as f64/self.count as f64)}else{None}})
    }
}
pub(super) fn snapshot(core: &Core, q: &Value) -> Result<Value> {
    snapshot_from_loaded(core, q, load(core, q)?)
}
fn snapshot_from_loaded(core: &Core, q: &Value, mut loaded: Loaded) -> Result<Value> {
    let mut overall = Sum::default();
    let mut days: BTreeMap<String, Sum> = BTreeMap::new();
    let mut models: BTreeMap<String, Sum> = BTreeMap::new();
    let mut projects: BTreeMap<String, (String, Sum)> = BTreeMap::new();
    let mut selected_sessions: BTreeSet<String> =
        loaded.recorded_sessions.keys().cloned().collect();
    for r in &loaded.responses {
        overall.add(r)?;
        let day = local_day(&r.event_time).unwrap();
        days.entry(day).or_default().add(r)?;
        let model = r.requested_model.as_deref().unwrap_or("unknown");
        models.entry(model.into()).or_default().add(r)?;
        let key = format!("{}|{}", r.source_id, r.thread_id);
        selected_sessions.insert(key.clone());
        if let Some(s) = loaded.sessions.get_mut(&key) {
            let mut count = s["responseCount"].as_i64().unwrap_or(0);
            add_tokens(&mut count, 1, "会话响应数量")?;
            let mut tokens = s["totalTokens"].as_i64().unwrap_or(0);
            add_tokens(
                &mut tokens,
                r.counts.total.value().unwrap_or(0),
                "会话 token",
            )?;
            s["responseCount"] = json!(count);
            s["totalTokens"] = json!(tokens);
            s["model"] = json!(r.requested_model);
            let project = strv(s, "project");
            let path = strv(s, "cwd");
            projects
                .entry(project)
                .or_insert_with(|| (path, Sum::default()))
                .1
                .add(r)?;
        }
    }
    let native_keys: BTreeSet<(String, String, String)> = loaded
        .responses
        .iter()
        .filter_map(|r| {
            r.turn_id
                .as_ref()
                .map(|t| (r.source_id.clone(), r.thread_id.clone(), t.clone()))
        })
        .collect();
    let providers: Vec<String> = loaded
        .responses
        .iter()
        .map(|r| r.provider_id.clone().unwrap_or_else(|| "unknown".into()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    // Aggregates, overlap identities and provider filters own everything needed
    // below; the full response list is never part of the returned snapshot.
    drop(loaded.responses);
    let mut legacy_tokens = 0i64;
    let mut legacy_count = 0usize;
    let mut legacy_suppressed = 0usize;
    for_each_json_value(&core.db, "legacy_usage_segments", |mut v| {
        if !in_dates(q, &strv(&v, "eventTime"))
            || !in_dates(q, &strv(&v, "from"))
            || !scoped(
                q,
                &strv(&v, "sourceId"),
                &strv(&v, "threadId"),
                &strv(&v, "turnId"),
                v.get("model").and_then(Value::as_str),
                &loaded.sessions,
                &loaded.metadata,
                &loaded.sources,
            )
        {
            return Ok(());
        }
        let key = (
            strv(&v, "sourceId"),
            strv(&v, "threadId"),
            strv(&v, "turnId"),
        );
        let native_overlap=core.db.query_row("SELECT EXISTS(SELECT 1 FROM usage_responses WHERE source_id=?1 AND thread_id=?2 AND turn_id=?3)",params![key.0,key.1,key.2],|r|r.get::<_,bool>(0)).map_err(err)?;
        if key.2.is_empty() || native_keys.contains(&key) || native_overlap {
            add_count(&mut legacy_suppressed, 1, "已抑制旧语义片段数量")?;
            return Ok(());
        }
        v["overlapState"] = json!("no-native-in-same-turn");
        if let Ok(c) = serde_json::from_value::<super::adapter::Counts>(v["counts"].clone()) {
            if let Some(n) = c.total.value() {
                add_tokens(&mut legacy_tokens, n, "旧语义 token")?;
                add_count(&mut legacy_count, 1, "旧语义片段数量")?;
            }
        }
        Ok(())
    })?;
    drop(native_keys);
    let mut overview = overall.overview();
    overview["trustedMeteredResponseCount"] = json!(overall.count);
    overview["responseCount"] = json!(loaded.all_count);
    overview["legacyTokens"] = json!(legacy_tokens);
    overview["legacySegments"] = json!(legacy_count);
    overview["legacySuppressedSegments"] = json!(legacy_suppressed);
    overview["observedResponseCount"] = json!(loaded.all_count);
    overview["excludedResponses"] = json!(loaded.excluded_count);
    overview["totalLabel"] = json!("已记录 token");
    overview["legacyIncludedInTotal"] = json!(false);
    let daily:Vec<Value>=days.into_iter().map(|(date,s)|json!({"date":date,"totalTokens":s.total,"inputTokens":s.input,"outputTokens":s.output,"cachedInputTokens":if s.cache_count>0{Some(s.cache)}else{None},"responseCount":s.count})).collect();
    let models:Vec<Value>=models.into_iter().map(|(name,s)|json!({"name":name,"totalTokens":s.total,"inputTokens":s.input,"outputTokens":s.output,"responseCount":s.count,"modelKind":"requested"})).collect();
    let projects:Vec<Value>=projects.into_iter().map(|(id,(path,s))|json!({"id":id,"name":project_name(&id,&path),"path":path,"totalTokens":s.total,"responseCount":s.count,"responseCountScope":"trusted-metered-subset"})).collect();
    for (key, count) in &loaded.recorded_sessions {
        if let Some(session) = loaded.sessions.get_mut(key) {
            session["trustedMeteredResponseCount"] = session["responseCount"].clone();
            session["responseCount"] = json!(count);
        }
    }
    let is_root = |t: &&Value| {
        t.get("rootTurnId")
            .and_then(Value::as_str)
            .map(|id| id == strv(t, "id"))
            .unwrap_or(true)
    };
    let active = loaded
        .turns
        .iter()
        .filter(is_root)
        .filter(|t| strv(t, "status") == "active" && fresh(t))
        .count();
    let uncertain = loaded
        .turns
        .iter()
        .filter(is_root)
        .filter(|t| strv(t, "status") == "active" && !fresh(t))
        .count();
    let child_active = loaded
        .turns
        .iter()
        .filter(|t| !is_root(t) && strv(t, "status") == "active" && fresh(t))
        .count();
    let child_uncertain = loaded
        .turns
        .iter()
        .filter(|t| !is_root(t) && strv(t, "status") == "active" && !fresh(t))
        .count();
    let session_scope = &loaded.sessions;
    let metadata_scope = &loaded.metadata;
    let source_scope = &loaded.sources;
    let mut turns: Vec<Value> = loaded
        .turns
        .into_iter()
        .filter(|t| {
            scoped(
                q,
                &strv(t, "sourceId"),
                &strv(t, "threadId"),
                &strv(t, "id"),
                t.get("model").and_then(Value::as_str),
                session_scope,
                metadata_scope,
                source_scope,
            ) && in_dates(
                q,
                t["completedAt"]
                    .as_str()
                    .or_else(|| t["startedAt"].as_str())
                    .unwrap_or(&strv(t, "lastEventAt")),
            )
        })
        .collect();
    turns.sort_by_key(|t| std::cmp::Reverse(strv(t, "lastEventAt")));
    let mut tools = Vec::new();
    for_each_json_value(&core.db, "items", |t| {
        if scoped(
            q,
            &strv(&t, "sourceId"),
            &strv(&t, "threadId"),
            &strv(&t, "turnId"),
            None,
            session_scope,
            metadata_scope,
            source_scope,
        ) && in_dates(q, &strv(&t, "eventTime"))
        {
            tools.push(t);
        }
        Ok(())
    })?;
    tools.sort_by_key(|t| std::cmp::Reverse(strv(t, "eventTime")));
    let mut inbox = Vec::new();
    for_each_json_value(&core.db, "inbox_events", |t| {
        if scoped(
            q,
            &strv(&t, "sourceId"),
            &strv(&t, "threadId"),
            &strv(&t, "turnId"),
            None,
            session_scope,
            metadata_scope,
            source_scope,
        ) && in_dates(q, &strv(&t, "createdAt"))
        {
            inbox.push(t);
        }
        Ok(())
    })?;
    for i in &mut inbox {
        let current = i["eligibleRuntime"].as_str() == Some(core.runtime_id.as_str());
        i["animationEligible"] = json!(
            current
                && i["notifyEligible"].as_bool().unwrap_or(false)
                && !i["animationAcknowledged"].as_bool().unwrap_or(false)
        );
        if !current {
            i["notifyEligible"] = json!(false);
        }
    }
    inbox.sort_by_key(|i| std::cmp::Reverse(strv(i, "createdAt")));
    // A session can have a timed turn or tool result without any usage record.
    // These facts have already passed the same scope/date filters as their
    // detail views. Keep their navigation entry without inventing token usage.
    for fact in turns.iter().chain(tools.iter()).chain(inbox.iter()) {
        let source = strv(fact, "sourceId");
        let thread = strv(fact, "threadId");
        if !source.is_empty() && !thread.is_empty() {
            selected_sessions.insert(format!("{source}|{thread}"));
        }
    }
    // All scoped fact filtering is complete. Consume session values only now,
    // rather than cloning the selected values while their lookup map is alive.
    let mut sessions: Vec<Value> = loaded
        .sessions
        .into_values()
        .filter(|s| {
            matches(q, "sourceId", &strv(s, "sourceId"))
                && agent_kind::matches(q, Some(s))
                && matches(q, "threadId", &strv(s, "id"))
                && matches(q, "project", &strv(s, "project"))
                && (selected_sessions.contains(&format!(
                    "{}|{}",
                    strv(s, "sourceId"),
                    strv(s, "id")
                )) || q.get("from").is_none()
                    && q.get("to").is_none()
                    && q.get("model").is_none()
                    && q.get("providerId").is_none()
                    && q.get("rootTurnId").is_none()
                    && q.get("clientKind").is_none())
        })
        .collect();
    sessions.sort_by_key(|s| std::cmp::Reverse(s["totalTokens"].as_i64().unwrap_or(0)));
    drop(loaded.metadata);
    drop(loaded.recorded_sessions);
    drop(selected_sessions);
    let mut issues = core
        .db
        .prepare(
            "SELECT kind,message,COUNT(*),MIN(file) FROM ingestion_issues GROUP BY kind,message",
        )
        .map_err(err)?;
    let issues:Vec<Value>=issues.query_map([],|r|Ok(json!({"kind":r.get::<_,String>(0)?,"message":r.get::<_,String>(1)?,"count":r.get::<_,i64>(2)?,"file":r.get::<_,Option<String>>(3)?}))).map_err(err)?.collect::<std::result::Result<Vec<_>,_>>().map_err(err)?;
    let count = |table: &str| -> Result<i64> {
        core.db
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .map_err(err)
    };
    let budgets = budget_views(core)?;
    let capabilities = json_rows(&core.db, "source_capabilities")?;
    let channels = json_rows(&core.db, "collection_channels")?;
    let latest_source: Option<String> = core
        .db
        .query_row(
            "SELECT MAX(json_extract(value,'$')) FROM settings WHERE key LIKE 'sourceLastEvent:%'",
            [],
            |r| r.get(0),
        )
        .map_err(err)?;
    let mut data = json!({"settings":core.settings()?,"capabilities":{"usage":"已接入","cacheRead":"已接入","cacheWrite":"字段已接入，非零场景以固定样本验证","turnTiming":"已接入","requestSpeed":"来源未提供","context":"来源未提供","claudeCode":"未发布","crossSource":"按身份与覆盖证据筛选可信总量"},"health":{"lastSuccessfulScan":get_setting(&core.db,"lastSuccessfulScan")?,"lastSourceEvent":latest_source,"conflicts":count("identity_conflicts")?,"files":count("source_files")?,"unsupportedFiles":get_setting(&core.db,"unsupportedFiles")?.unwrap_or(json!(0)),"coverageFrom":loaded.health_usage["coverageFrom"],"coverageTo":loaded.health_usage["coverageTo"],"unknownModels":loaded.health_usage["unknownModels"],"sourceRoot":core.source_root.to_string_lossy(),"pendingFiles":get_setting(&core.db,"pendingFiles")?.unwrap_or(json!(0)),"excludedResponses":loaded.health_usage["excludedResponses"]},"activity":{"activeTurns":active,"uncertainTurns":uncertain,"state":if active>0{"active"}else if uncertain>0{"uncertain"}else{"idle"}}});
    // json! serializes borrowed expressions. Move the already-built trees into
    // the response instead of allocating a second copy of every visible fact.
    let fields = data.as_object_mut().unwrap();
    fields.insert("overview".into(), overview);
    for (name, values) in [
        ("daily", daily),
        ("projects", projects),
        ("models", models),
        ("sessions", sessions),
        ("turns", turns),
        ("tools", tools),
        ("inbox", inbox),
        ("budgets", budgets),
        ("sources", loaded.sources),
        ("channels", channels),
        ("sourceCapabilities", capabilities),
    ] {
        fields.insert(name.into(), Value::Array(values));
    }
    data["health"]["issues"] = Value::Array(issues);
    data["activity"]["activeChildTurns"] = json!(child_active);
    data["activity"]["uncertainChildTurns"] = json!(child_uncertain);
    data["primarySourceId"] = json!(core.source_id);
    data["health"]["conflictHistory"] = json!(count("identity_conflicts")?);
    let unresolved:i64=core.db.query_row("SELECT COUNT(*) FROM identity_conflicts WHERE COALESCE(json_extract(details,'$.resolved'),0)=0",[],|r|r.get(0)).map_err(err)?;
    data["health"]["conflicts"] = json!(unresolved);
    data["health"]["coverageStatus"] = json!("observed-event-range");
    data["health"]["coverageComplete"] = json!(false);
    let mut unsupported_known = 0u64;
    let mut unsupported_complete = true;
    for source in data["sources"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["enabled"].as_bool() == Some(true))
    {
        if let Some(count) = source["health"]["unsupportedFiles"].as_u64() {
            unsupported_known = unsupported_known
                .checked_add(count)
                .ok_or("不支持的文件数量汇总超出整数范围，原始用量记录已保留")?;
        } else {
            unsupported_complete = false;
        }
    }
    data["health"]["unsupportedFiles"] = if unsupported_complete {
        json!(unsupported_known)
    } else {
        Value::Null
    };
    data["health"]["unsupportedKnownFiles"] = json!(unsupported_known);
    let mut history_pending = 0u64;
    let mut history_complete = true;
    let mut enabled_sources = 0usize;
    for source in data["sources"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["enabled"].as_bool() == Some(true))
    {
        enabled_sources += 1;
        let discovery_finished = Path::new(&strv(source, "root")).is_dir()
            && source_check(&source["health"]).is_some_and(|check| check.discovery_finished());
        let pending = get_setting(&core.db, &format!("historyPending:{}", strv(source, "id")))?
            .and_then(|v| v.as_u64());
        if let Some(pending) = pending.filter(|_| discovery_finished) {
            history_pending = history_pending
                .checked_add(pending)
                .ok_or("历史待处理文件数量汇总超出整数范围，原始用量记录已保留")?;
        } else {
            history_complete = false;
        }
    }
    let history_known = enabled_sources > 0 && history_complete;
    let cancelled = get_setting(&core.db, "historyCancelled")?
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let paused = get_setting(&core.db, "historyPaused")?
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    data["health"]["history"] = json!({"status":if cancelled{"cancelled"}else if paused{"paused"}else if history_known && history_pending==0{"idle"}else{"running"},"from":get_setting(&core.db,"importSince")?,"pendingFiles":if history_known{Some(history_pending)}else{None},"knownPendingFiles":history_pending,"cancelledAt":get_setting(&core.db,"historyCancelledAt")?});
    let source_runtime: BTreeMap<String, Value> = data["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|source| (strv(source, "id"), source["runtime"].clone()))
        .collect();
    if let Some(capabilities) = data["sourceCapabilities"].as_array_mut() {
        for capability in capabilities {
            let id = strv(capability, "sourceId");
            if let Some(runtime) = source_runtime.get(&id) {
                let runtime = runtime.clone();
                if let Some(fields) = capability.as_object_mut() {
                    for field in fields.values_mut() {
                        if field.get("support").is_some() {
                            field["sourceRuntime"] = runtime.clone();
                            if field["runtime"] == "可用" && runtime != "可用" {
                                field["runtime"] = runtime.clone();
                            }
                        }
                    }
                }
            }
        }
    }
    if data
        .pointer("/settings/privacyMode")
        .and_then(Value::as_bool)
        == Some(true)
    {
        redact(&mut data);
    }
    let source_ids: Vec<String> = data["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| strv(s, "id"))
        .collect();
    let client_kinds: Vec<String> = data["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| strv(s, "clientKind"))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    data["filters"] =
        json!({"sourceIds":source_ids,"clientKinds":client_kinds,"providerIds":providers});
    Ok(data)
}
fn fresh(t: &Value) -> bool {
    chrono::DateTime::parse_from_rfc3339(&strv(t, "lastEventAt"))
        .map(|d| {
            let seconds = Utc::now()
                .signed_duration_since(d.with_timezone(&Utc))
                .num_seconds();
            (-5..600).contains(&seconds)
        })
        .unwrap_or(false)
}
fn redact(v: &mut Value) {
    if let Some(m) = v.as_object_mut() {
        for (k, value) in m {
            if ["cwd", "path", "root", "sourceRoot", "file"].contains(&k.as_str()) {
                *value = json!("已隐藏");
            } else if ["project", "projectName", "title"].contains(&k.as_str()) && value.is_string()
            {
                *value = json!("隐私模式");
            } else if k == "projects" {
                if let Some(projects) = value.as_array_mut() {
                    for (index, project) in projects.iter_mut().enumerate() {
                        project["name"] = json!(format!("项目 {}", index + 1));
                        redact(project);
                    }
                }
            } else {
                redact(value);
            }
        }
    } else if let Some(a) = v.as_array_mut() {
        for i in a {
            redact(i);
        }
    }
}
pub(super) fn budget_views(core: &Core) -> Result<Vec<Value>> {
    budget_views_at(core, crate::budget_clock::now().utc)
}
fn budget_views_at(core: &Core, at: chrono::DateTime<Utc>) -> Result<Vec<Value>> {
    let budgets = json_rows(&core.db, "budgets")?;
    if budgets.is_empty() {
        return Ok(vec![]);
    }
    let day = local_day(&at.to_rfc3339()).ok_or("预算日期无效")?;
    let q = json!({"from":day,"to":day});
    let daily = load(core, &q)?;
    let task = if budgets.iter().any(|b| strv(b, "scope") == "rootTurn") {
        Some(load(core, &json!({}))?)
    } else {
        None
    };
    let mut result = Vec::new();
    for mut b in budgets {
        let scope = strv(&b, "scope");
        let target = strv(&b, "target");
        let loaded = if scope == "rootTurn" {
            task.as_ref().unwrap()
        } else {
            &daily
        };
        if ["thread", "rootTurn"].contains(&scope.as_str()) && b["sourceId"].as_str().is_none() {
            let sources: BTreeSet<_> = loaded
                .sessions
                .values()
                .filter(|s| {
                    if scope == "thread" {
                        strv(s, "id") == target
                    } else {
                        loaded.turns.iter().any(|t| {
                            strv(t, "id") == target && strv(t, "sourceId") == strv(s, "sourceId")
                        })
                    }
                })
                .map(|s| strv(s, "sourceId"))
                .collect();
            if sources.len() == 1 {
                b["sourceId"] = json!(sources.into_iter().next());
            } else {
                b["usedTokens"] = Value::Null;
                b["scopeStatus"] = json!("ambiguous-source");
                result.push(b);
                continue;
            }
        }
        let used = loaded
            .responses
            .iter()
            .filter(|r| budget_matches(&b, r, &loaded.sessions))
            .try_fold(0i64, |sum, r| {
                sum.checked_add(r.counts.total.value().unwrap_or(0))
            });
        b["usedTokens"] = json!(used);
        b["period"] = json!(budget_period_at(&b, at));
        b["metric"] = json!("trusted-local-total-tokens");
        b["timezone"] = json!("Asia/Shanghai");
        result.push(b);
    }
    Ok(result)
}
fn budget_period_at(b: &Value, at: chrono::DateTime<Utc>) -> String {
    if strv(b, "scope") == "rootTurn" {
        format!("task:{}:{}", strv(b, "sourceId"), strv(b, "target"))
    } else {
        local_day(&at.to_rfc3339()).unwrap_or_default()
    }
}
pub(super) fn initialize_budget(core: &mut Core, id: &str) -> Result<()> {
    budget::initialize(core, id)
}
fn budget_matches(b: &Value, r: &ResponseUsage, sessions: &BTreeMap<String, Value>) -> bool {
    let source = b
        .get("sourceId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    if source.is_some_and(|s| s != r.source_id) {
        return false;
    }
    let target = strv(b, "target");
    match strv(b, "scope").as_str() {
        "project" => sessions
            .get(&format!("{}|{}", r.source_id, r.thread_id))
            .map(|s| strv(s, "project") == target || strv(s, "cwd") == target)
            .unwrap_or(false),
        "thread" => source.is_some() && r.thread_id == target,
        "rootTurn" => source.is_some() && r.root_turn_id.as_deref() == Some(target.as_str()),
        _ => true,
    }
}
pub(super) fn evaluate_budgets(core: &mut Core) -> Result<()> {
    budget::evaluate_without_scan(core)
}
fn retention_boundary(previous: Option<&Value>, proposed: chrono::DateTime<Utc>) -> Result<String> {
    let boundary = if let Some(value) = previous {
        let invalid = || "本地保留时间水位无效，已停止清理并保留现有历史保护边界".to_owned();
        let recorded = value.as_str().ok_or_else(invalid)?;
        let recorded = chrono::DateTime::parse_from_rfc3339(recorded)
            .map_err(|_| invalid())?
            .with_timezone(&Utc);
        proposed.max(recorded)
    } else {
        proposed
    };
    // Stored boundaries use millisecond precision. Round upward so a valid
    // higher-precision protected instant is never weakened by formatting.
    let remainder = boundary.timestamp_subsec_nanos() % 1_000_000;
    let boundary = if remainder == 0 {
        boundary
    } else {
        boundary
            .checked_add_signed(Duration::nanoseconds((1_000_000 - remainder) as i64))
            .ok_or("保留时间水位超出可表示范围，已停止清理")?
    };
    Ok(boundary.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

pub(super) fn apply_retention(core: &mut Core) -> Result<()> {
    let days = core.settings()?["retentionDays"]
        .as_i64()
        .unwrap_or(90)
        .clamp(1, 3650);
    let proposed = crate::budget_clock::now().utc - Duration::days(days);
    let tx = core.db.transaction().map_err(err)?;
    // A wider preference is not an explicit history import. Keep the latest
    // protected instant; reject corrupt persisted state before deleting facts.
    let previous = get_setting(&tx, "retentionWatermark")?;
    let cutoff = retention_boundary(previous.as_ref(), proposed)?;
    let mut budget_facts_removed = false;
    for table in [
        "usage_responses",
        "legacy_usage_segments",
        "turns",
        "items",
        "inbox_events",
        "usage_aggregates",
        "context_snapshots",
        "timing_observations",
        "fact_observations",
    ] {
        let removed = tx.execute(
            &format!("DELETE FROM {table} WHERE event_time<?1"),
            [&cutoff],
        )
        .map_err(err)?;
        if ["usage_responses","turns"].contains(&table) && removed > 0 { budget_facts_removed = true; }
    }
    put_setting(&tx, "retentionWatermark", &json!(cutoff))?;
    tx.commit().map_err(err)?;
    if budget_facts_removed { core.budget_observer.invalidate(); }
    Ok(())
}
fn csv_cell(value: &str) -> String {
    let v = value.replace(['\r', '\n'], " ");
    let trimmed = v.trim_start_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    let safe = if trimmed.starts_with(['=', '+', '-', '@']) {
        format!("'{v}")
    } else {
        v
    };
    format!("\"{}\"", safe.replace('"', "\"\""))
}
pub(super) fn export_csv(core: &Core, q: &Value) -> Result<String> {
    let loaded = load(core, q)?;
    let private = q
        .get("anonymize")
        .and_then(Value::as_bool)
        .unwrap_or(core.settings()?["privacyMode"].as_bool().unwrap_or(false));
    let mut csv=String::from("\u{feff}source_id,thread_id,response_id,event_time,project,requested_model,input_tokens,output_tokens,total_tokens,cache_read,cache_write,reasoning_output,metric_semantics,quality\r\n");
    for r in loaded.responses {
        let project = loaded
            .sessions
            .get(&format!("{}|{}", r.source_id, r.thread_id))
            .map(|s| strv(s, "project"))
            .unwrap_or_default();
        let text = |m: &Metric| m.value().map(|n| n.to_string()).unwrap_or_default();
        let cells = [
            r.source_id.clone(),
            r.thread_id.clone(),
            r.response_id.clone(),
            r.event_time.clone(),
            if private {
                format!("项目-{}", &hash(project.as_bytes())[..8])
            } else {
                project
            },
            r.requested_model.unwrap_or_default(),
            text(&r.counts.input),
            text(&r.counts.output),
            text(&r.counts.total),
            text(&r.counts.cache_read),
            text(&r.counts.cache_write),
            text(&r.counts.reasoning),
            r.semantics_version,
            r.completeness,
        ];
        csv.push_str(
            &cells
                .iter()
                .map(|s| csv_cell(s))
                .collect::<Vec<_>>()
                .join(","),
        );
        csv.push_str("\r\n");
    }
    Ok(csv)
}

#[cfg(test)]
mod retention_boundary_tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, Core, PathBuf, PathBuf, String) {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        let data = tmp.path().join("data");
        std::fs::create_dir(&source).unwrap();
        let core = Core::open(&data, &source).unwrap();
        let old =
            (Utc::now() - Duration::days(10)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        for (file, thread, time, total) in [
            ("old.jsonl", "retention-old", old.clone(), 120),
            ("today.jsonl", "retention-today", now(), 220),
        ] {
            let meta = json!({"timestamp":time,"type":"session_meta","payload":{"id":thread,"cwd":"D:/authored/retention"}});
            let usage = json!({"timestamp":time,"type":"token_usage_record","payload":{"thread_id":thread,"response_id":thread,"requested_model":"retention-model","usage":{"input_tokens":total-20,"output_tokens":20,"total_tokens":total,"cached_input_tokens":0,"cache_write_input_tokens":0,"reasoning_output_tokens":0}}});
            std::fs::write(source.join(file), format!("{meta}\n{usage}\n")).unwrap();
        }
        (tmp, core, source, data, old)
    }
    fn drain(core: &mut Core) {
        for _ in 0..5 {
            core.scan().unwrap();
        }
    }
    fn total(core: &Core) -> Value {
        core.snapshot(json!({})).unwrap()["overview"]["totalTokens"].clone()
    }

    #[test]
    fn wider_retention_cannot_reimport_new_old_copy_but_explicit_history_can() {
        let (_tmp, mut core, source, data, old) = fixture();
        drain(&mut core);
        assert_eq!(total(&core), 340);
        core.action("update_settings", &json!({"retentionDays":7}))
            .unwrap();
        core.action("apply_retention", &json!({})).unwrap();
        assert_eq!(total(&core), 220);
        std::fs::copy(
            source.join("old.jsonl"),
            source.join("copy-before-widen.jsonl"),
        )
        .unwrap();
        core.shutdown().unwrap();
        drop(core);
        let mut core = Core::open(&data, &source).unwrap();
        drain(&mut core);
        assert_eq!(total(&core), 220);
        let boundary = get_setting(&core.db, "retentionWatermark").unwrap();
        core.action("update_settings", &json!({"retentionDays":365}))
            .unwrap();
        drain(&mut core);
        assert_eq!(
            get_setting(&core.db, "retentionWatermark").unwrap(),
            boundary
        );
        std::fs::copy(
            source.join("old.jsonl"),
            source.join("copy-after-widen.jsonl"),
        )
        .unwrap();
        drain(&mut core);
        assert_eq!(
            total(&core),
            220,
            "a wider setting cannot undo historical cleanup"
        );
        core.action("import_history", &json!({"from":old})).unwrap();
        drain(&mut core);
        assert_eq!(
            total(&core),
            340,
            "explicit import may restore the original history once"
        );
        let restored = get_setting(&core.db, "retentionWatermark").unwrap();
        assert!(
            chrono::DateTime::parse_from_rfc3339(restored.as_ref().unwrap().as_str().unwrap())
                .unwrap()
                < chrono::DateTime::parse_from_rfc3339(
                    boundary.as_ref().unwrap().as_str().unwrap()
                )
                .unwrap()
        );
        core.shutdown().unwrap();
        drop(core);
        let mut reopened = Core::open(&data, &source).unwrap();
        drain(&mut reopened);
        assert_eq!(total(&reopened), 340);
    }

    #[test]
    fn timezone_order_is_chronological_and_invalid_boundary_leaves_facts_untouched() {
        let proposed = chrono::DateTime::parse_from_rfc3339("2026-10-10T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        // The first stored text sorts later but represents an earlier instant.
        assert_eq!(
            retention_boundary(Some(&json!("2026-10-10T07:30:00+08:00")), proposed).unwrap(),
            "2026-10-10T00:00:00.000Z"
        );
        // The second sorts earlier but represents the later protected instant.
        assert_eq!(
            retention_boundary(Some(&json!("2026-10-09T23:30:00-01:00")), proposed).unwrap(),
            "2026-10-10T00:30:00.000Z"
        );
        assert_eq!(
            retention_boundary(Some(&json!("2026-10-10T00:00:00.000900Z")), proposed).unwrap(),
            "2026-10-10T00:00:00.001Z"
        );
        let (_tmp, mut core, _source, _data, _old) = fixture();
        drain(&mut core);
        assert_eq!(total(&core), 340);
        core.action("update_settings", &json!({"retentionDays":7}))
            .unwrap();
        // Deliberately corrupt only this private fixture's metadata. No fact is
        // inserted through SQL: both measured responses came from public scan.
        put_setting(&core.db, "retentionWatermark", &json!("not-rfc3339")).unwrap();
        let message = core.action("apply_retention", &json!({})).unwrap_err();
        assert!(message.contains("水位无效"));
        assert_eq!(
            get_setting(&core.db, "retentionWatermark").unwrap(),
            Some(json!("not-rfc3339"))
        );
        assert_eq!(
            total(&core),
            340,
            "a refused cleanup must not delete any fact"
        );
    }
}

#[cfg(test)]
mod allocation_parity_tests {
    use super::*;

    #[test]
    fn filtered_snapshot_keeps_metadata_from_unselected_rows_and_all_output_arrays() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        std::fs::create_dir(&source).unwrap();
        let mut core = Core::open(&tmp.path().join("data"), &source).unwrap();
        let current = Utc::now();
        let previous = current - Duration::days(1);
        let event = |time: chrono::DateTime<Utc>, kind: &str, payload: Value| json!({"timestamp":time.to_rfc3339_opts(chrono::SecondsFormat::Millis,true),"type":kind,"payload":payload});
        let usage = |time, id, provider, total| {
            event(
                time,
                "token_usage_record",
                json!({"thread_id":"parity-thread","response_id":id,"turn_id":"parity-turn","root_turn_id":"parity-turn","requested_model":"recorded-model","provider_id":provider,"usage":{"input_tokens":total-20,"output_tokens":20,"total_tokens":total,"cached_input_tokens":0,"cache_write_input_tokens":0,"reasoning_output_tokens":0}}),
            )
        };
        let fixture = [
            event(
                previous,
                "session_meta",
                json!({"id":"parity-thread","cwd":"D:/authored/parity"}),
            ),
            event(
                previous,
                "event_msg",
                json!({"type":"task_started","turn_id":"parity-turn","root_turn_id":"parity-turn"}),
            ),
            event(
                previous,
                "turn_context",
                json!({"turn_id":"parity-turn","root_turn_id":"parity-turn","model":"recorded-model"}),
            ),
            usage(previous, "previous-320", "provider-b", 320),
            usage(current, "current-120", "provider-a", 120),
            event(
                current,
                "event_msg",
                json!({"type":"exec_command_begin","call_id":"parity-tool"}),
            ),
            event(
                current,
                "event_msg",
                json!({"type":"exec_command_end","call_id":"parity-tool","exit_code":0}),
            ),
            event(
                current,
                "event_msg",
                json!({"type":"task_complete","turn_id":"parity-turn","root_turn_id":"parity-turn","status":"completed"}),
            ),
        ];
        let text = fixture
            .iter()
            .map(|v| v.to_string() + "\n")
            .collect::<String>();
        std::fs::write(source.join("parity.jsonl"), text).unwrap();
        for _ in 0..5 {
            core.scan().unwrap();
        }
        let all = core.snapshot(json!({})).unwrap();
        assert_eq!(all["overview"]["totalTokens"], 440);
        assert_eq!(all["overview"]["responseCount"], 2);
        for field in ["sessions", "turns", "tools", "inbox"] {
            assert_eq!(all[field].as_array().unwrap().len(), 1, "{field}");
        }
        assert_eq!(all["turns"][0]["model"], "recorded-model");
        assert_eq!(all["tools"][0]["exitCode"], 0);
        assert_eq!(all["inbox"][0]["status"], "completed");
        assert_eq!(
            all["filters"]["providerIds"],
            json!(["provider-a", "provider-b"])
        );
        let selected_day = local_day(&current.to_rfc3339()).unwrap();
        let selected = core
            .snapshot(json!({"providerId":"provider-a","from":selected_day,"to":selected_day}))
            .unwrap();
        assert_eq!(selected["overview"]["totalTokens"], 120);
        assert_eq!(selected["overview"]["responseCount"], 1);
        assert_eq!(selected["sessions"].as_array().unwrap().len(), 1);
        assert_eq!(selected["filters"]["providerIds"], json!(["provider-a"]));
        // A response outside the selected date/provider still proves that this
        // turn has ambiguous provider metadata. Its derived facts must not be
        // relabelled as provider-a merely because only that response is selected.
        for field in ["turns", "tools", "inbox"] {
            assert!(selected[field].as_array().unwrap().is_empty(), "{field}");
        }
        assert_eq!(selected["activity"], all["activity"]);
        assert_eq!(selected["health"]["files"], all["health"]["files"]);
        assert_eq!(selected["sources"], all["sources"]);
    }
}

#[cfg(test)]
pub(super) fn test_csv_cell(s: &str) -> String {
    csv_cell(s)
}

#[cfg(test)]
#[path = "aggregation_overflow_tests.rs"]
mod aggregation_overflow_tests;
