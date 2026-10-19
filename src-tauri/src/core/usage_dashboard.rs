//! A bounded read model over the same trusted facts as the main snapshot.
//! Prices are estimates; native provider identity and missing fields stay intact.
use super::super::pricing::{self, CostEstimate};
use super::*;
use chrono::{DateTime, FixedOffset, NaiveDate};

const MAX_DAYS: i64 = 366;
const MAX_PAGE: u64 = 1_000_000;

struct Request {
    page: usize,
    page_size: usize,
    quality: String,
}

impl Request {
    fn parse(value: &Value) -> Result<Self> {
        let fields = value.as_object().ok_or("统计明细参数必须是对象")?;
        if fields
            .keys()
            .any(|key| !["page", "pageSize", "quality"].contains(&key.as_str()))
        {
            return Err("统计明细包含未知参数".into());
        }
        let integer = |key, fallback| -> Result<u64> {
            fields.get(key).map_or(Ok(fallback), |v| {
                v.as_u64().ok_or_else(|| format!("{key} 必须是非负整数"))
            })
        };
        let page = integer("page", 0)?;
        let page_size = integer("pageSize", 20)?;
        if page > MAX_PAGE {
            return Err(format!("page 不得超过 {MAX_PAGE}"));
        }
        if !(1..=100).contains(&page_size) {
            return Err("pageSize 必须在 1 至 100 之间".into());
        }
        let quality = fields
            .get("quality")
            .map_or(Some("all"), Value::as_str)
            .filter(|s| ["all", "priced", "unpriced"].contains(s))
            .ok_or("quality 必须是 all、priced 或 unpriced")?;
        Ok(Self {
            page: page as usize,
            page_size: page_size as usize,
            quality: quality.into(),
        })
    }
    fn value(&self) -> Value {
        json!({"page":self.page,"pageSize":self.page_size,"quality":self.quality})
    }
    fn accepts(&self, estimate: &CostEstimate) -> bool {
        match self.quality.as_str() {
            "priced" => estimate.units.is_some(),
            "unpriced" => estimate.units.is_none(),
            _ => true,
        }
    }
}

fn validate_dates(q: &Value) -> Result<(Option<NaiveDate>, Option<NaiveDate>)> {
    let (from, to) = super::validate_query_dates(q)?;
    if let (Some(from), Some(to)) = (from, to) {
        validate_span(from, to)?;
    }
    Ok((from, to))
}

fn validate_span(from: NaiveDate, to: NaiveDate) -> Result<()> {
    let days = (to - from).num_days();
    if days < 0 {
        return Err("开始日期不得晚于结束日期".into());
    }
    if days >= MAX_DAYS {
        return Err(format!("统计范围最多 {MAX_DAYS} 天，请缩小日期范围"));
    }
    Ok(())
}

#[derive(Default)]
struct CostSum {
    units: i128,
    priced: usize,
}
impl CostSum {
    fn add(&mut self, cost: &CostEstimate) -> Result<()> {
        if let Some(units) = cost.units {
            self.units = self
                .units
                .checked_add(units)
                .ok_or("估算成本汇总超出整数范围，原始用量记录已保留")?;
            add_count(&mut self.priced, 1, "已定价响应数量")?;
        }
        Ok(())
    }
    fn value(&self) -> Value {
        if self.priced == 0 {
            Value::Null
        } else {
            json!(pricing::format_usd(self.units))
        }
    }
}

#[derive(Default)]
struct Group {
    tokens: Sum,
    cost: CostSum,
    turn_first_token_ms: i128,
    timed_responses: usize,
}
impl Group {
    fn add(&mut self, response: &ResponseUsage, cost: &CostEstimate) -> Result<()> {
        self.tokens.add(response)?;
        self.cost.add(cost)
    }
    fn add_timing(&mut self, time_to_first_token_ms: Option<i64>) -> Result<()> {
        if let Some(milliseconds) = time_to_first_token_ms {
            self.turn_first_token_ms = self
                .turn_first_token_ms
                .checked_add(i128::from(milliseconds))
                .ok_or("回合首 Token 耗时汇总超出整数范围，原始记录已保留")?;
            add_count(&mut self.timed_responses, 1, "回合计时样本数量")?;
        }
        Ok(())
    }
    fn value(&self, key: &str, name: &str) -> Value {
        let mut value = self.tokens.overview();
        value["key"] = json!(key);
        value["name"] = json!(name);
        value["estimatedCostUsd"] = self.cost.value();
        value["pricedResponses"] = json!(self.cost.priced);
        value["unpricedResponses"] = json!(self.tokens.count - self.cost.priced);
        value["turnTimeToFirstTokenMs"] = json!(if self.timed_responses > 0 {
            Some(self.turn_first_token_ms as f64 / self.timed_responses as f64)
        } else {
            None
        });
        value["timedResponses"] = json!(self.timed_responses);
        value["timingScope"] = json!("turn");
        value["successRate"] = Value::Null;
        value["speedTokensPerSecond"] = Value::Null;
        value
    }
}

struct Fact<'a> {
    response: &'a ResponseUsage,
    time: DateTime<FixedOffset>,
    key: String,
    cost: CostEstimate,
}

type TurnKey = (String, String, String);

#[derive(Default)]
struct ScopeGroup {
    group: Group,
    sessions: BTreeSet<(String, String)>,
    sources: BTreeSet<String>,
    first: Option<DateTime<FixedOffset>>,
    last: Option<DateTime<FixedOffset>>,
}
impl ScopeGroup {
    fn add(&mut self, fact: &Fact<'_>, timing: Option<i64>) -> Result<()> {
        self.group.add(fact.response, &fact.cost)?;
        self.group.add_timing(timing)?;
        self.sessions.insert((
            fact.response.source_id.clone(),
            fact.response.thread_id.clone(),
        ));
        self.sources.insert(fact.response.source_id.clone());
        self.first = Some(self.first.map_or(fact.time, |time| time.min(fact.time)));
        self.last = Some(self.last.map_or(fact.time, |time| time.max(fact.time)));
        Ok(())
    }
    fn value(
        &self,
        key: &str,
        name: &str,
        project: &str,
        thread: Option<&str>,
        privacy: bool,
    ) -> Value {
        let mut value = self.group.value(key, name);
        value["sourceId"] = if !privacy && self.sources.len() == 1 {
            json!(self
                .sources
                .iter()
                .next()
                .filter(|source| !source.is_empty()))
        } else {
            Value::Null
        };
        value["threadId"] = json!(thread.filter(|thread| !privacy && !thread.is_empty()));
        value["projectId"] = if privacy { Value::Null } else { json!(project) };
        value["sessionCount"] = json!(self.sessions.len());
        value["firstEventTime"] = json!(self.first.map(|time| time.to_rfc3339()));
        value["lastEventTime"] = json!(self.last.map(|time| time.to_rfc3339()));
        value
    }
}

fn scope_key(kind: &str, identity: Value) -> String {
    hash(json!([kind, identity]).to_string().as_bytes())
}

fn session_metadata<'a>(loaded: &'a Loaded, response: &ResponseUsage) -> Option<&'a Value> {
    loaded
        .sessions
        .get(&format!("{}|{}", response.source_id, response.thread_id))
}

fn response_project(loaded: &Loaded, response: &ResponseUsage) -> String {
    session_metadata(loaded, response)
        .and_then(|session| session["project"].as_str())
        .filter(|project| !project.is_empty())
        .unwrap_or("unknown")
        .to_owned()
}

fn scope_groups(
    facts: &[Fact<'_>],
    loaded: &Loaded,
    timings: &TurnTimings<'_>,
    privacy: bool,
) -> Result<(Vec<Value>, Vec<Value>)> {
    let mut projects = BTreeMap::<String, ScopeGroup>::new();
    let mut sessions = BTreeMap::<(String, String), ScopeGroup>::new();
    for fact in facts {
        let response = fact.response;
        let timing = timings.metric(response, "timeToFirstTokenMs");
        projects
            .entry(response_project(loaded, response))
            .or_default()
            .add(fact, timing)?;
        sessions
            .entry((response.source_id.clone(), response.thread_id.clone()))
            .or_default()
            .add(fact, timing)?;
    }
    let projects = projects
        .into_iter()
        .map(|(project, group)| {
            let name = project_name(&project, project.strip_prefix("path:").unwrap_or(""));
            group.value(
                &scope_key("project", json!(project)),
                &name,
                &project,
                None,
                privacy,
            )
        })
        .collect();
    let sessions = sessions
        .into_iter()
        .map(|((source, thread), group)| {
            let metadata = loaded.sessions.get(&format!("{source}|{thread}"));
            let project = metadata
                .and_then(|session| session["project"].as_str())
                .unwrap_or("unknown");
            let name = metadata
                .and_then(|session| session["title"].as_str())
                .filter(|title| !title.is_empty())
                .unwrap_or(if thread.is_empty() {
                    "会话未知"
                } else {
                    &thread
                });
            group.value(
                &scope_key("session", json!([source, thread])),
                name,
                project,
                Some(&thread),
                privacy,
            )
        })
        .collect();
    let sort = |mut groups: Vec<Value>, anonymous: &str| {
        groups.sort_by(|left, right| {
            right["totalTokens"]
                .as_i64()
                .cmp(&left["totalTokens"].as_i64())
                .then_with(|| left["key"].as_str().cmp(&right["key"].as_str()))
        });
        if privacy {
            for (index, group) in groups.iter_mut().enumerate() {
                group["name"] = json!(format!("匿名{anonymous} {}", index + 1));
            }
        }
        groups
    };
    Ok((sort(projects, "项目"), sort(sessions, "会话")))
}

fn forecast_basis(
    facts: &[Fact<'_>],
    timings: &TurnTimings<'_>,
    from: NaiveDate,
    to: NaiveDate,
    as_of: DateTime<Utc>,
) -> Result<Value> {
    // These are range-aligned local observations, not proof of source coverage.
    // Forecast consumers must not treat a responseCount=0 day as measured zero use.
    let mut daily = BTreeMap::<String, Sum>::new();
    let mut date = from;
    loop {
        daily.insert(date.to_string(), Sum::default());
        if date == to {
            break;
        }
        date = date.succ_opt().ok_or("统计日期超出范围")?;
    }
    let mut candidates = BTreeMap::<TurnKey, Sum>::new();
    for fact in facts {
        if fact.time > as_of {
            continue;
        }
        daily
            .entry(fact.time.date_naive().to_string())
            .or_default()
            .add(fact.response)?;
        let response = fact.response;
        // A response without a turn identity cannot establish how many turns
        // were omitted. Keep it in daily totals without inventing a turn sample.
        if let Some(turn) = response.turn_id.as_deref().filter(|turn| !turn.is_empty()) {
            if !response.source_id.is_empty() && !response.thread_id.is_empty() {
                candidates
                    .entry((
                        response.source_id.clone(),
                        response.thread_id.clone(),
                        turn.into(),
                    ))
                    .or_default()
                    .add(response)?;
            }
        }
    }
    let mut completed = Vec::new();
    let mut omitted = 0usize;
    for (key, sum) in candidates {
        let turn = timings.turns.get(&key).copied().flatten();
        let completion = turn
            .filter(|turn| {
                turn["status"] == "completed"
                    && turn["resultEvidence"] == "explicit-terminal"
                    && timings.counts.get(&key) == Some(&sum.count)
            })
            .and_then(|turn| turn["completedAt"].as_str())
            .and_then(|time| DateTime::parse_from_rfc3339(time).ok());
        let Some(completion) = completion.filter(|time| {
            let date = time
                .with_timezone(&FixedOffset::east_opt(8 * 3600).unwrap())
                .date_naive();
            date >= from && date <= to && *time <= as_of
        }) else {
            add_count(&mut omitted, 1, "缺完整计量的预测回合数量")?;
            continue;
        };
        completed.push((completion, json!({"key":scope_key("turn", json!(key)),
            "completedAt":completion.to_rfc3339(),"totalTokens":sum.total,"responseCount":sum.count})));
    }
    completed.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| left.1["key"].as_str().cmp(&right.1["key"].as_str()))
    });
    Ok(
        json!({"daily":daily.into_iter().map(|(date,sum)| json!({"date":date,
        "totalTokens":sum.total,"responseCount":sum.count})).collect::<Vec<_>>(),
        "completedTurns":completed.into_iter().take(20).map(|(_,value)|value).collect::<Vec<_>>(),
        "omittedIncompleteTurns":omitted}),
    )
}

/// One timing sample can be attributed only when the complete native turn has
/// exactly one observed response, including observations outside this filter.
struct TurnTimings<'a> {
    turns: BTreeMap<TurnKey, Option<&'a Value>>,
    counts: &'a BTreeMap<TurnKey, usize>,
}
impl<'a> TurnTimings<'a> {
    fn new(loaded: &'a Loaded) -> Self {
        let mut turns = BTreeMap::new();
        for turn in &loaded.turns {
            let key = (
                strv(turn, "sourceId"),
                strv(turn, "threadId"),
                strv(turn, "id"),
            );
            if key.0.is_empty() || key.1.is_empty() || key.2.is_empty() {
                continue;
            }
            turns
                .entry(key)
                .and_modify(|entry| *entry = None)
                .or_insert(Some(turn));
        }
        Self {
            turns,
            counts: &loaded.turn_response_counts,
        }
    }
    fn metric(&self, response: &ResponseUsage, metric: &str) -> Option<i64> {
        let turn_id = response.turn_id.as_ref()?;
        if response.source_id.is_empty() || response.thread_id.is_empty() || turn_id.is_empty() {
            return None;
        }
        let key = (
            response.source_id.clone(),
            response.thread_id.clone(),
            turn_id.clone(),
        );
        if self.counts.get(&key) != Some(&1) {
            return None;
        }
        self.turns
            .get(&key)
            .copied()
            .flatten()?
            .get(metric)?
            .as_i64()
            .filter(|value| *value >= 0)
    }
}

fn normal_input(response: &ResponseUsage) -> Option<i64> {
    response
        .counts
        .input
        .value()?
        .checked_sub(response.counts.cache_read.value()?)?
        .checked_sub(response.counts.cache_write.value()?)
        .filter(|n| *n >= 0)
}

fn build(
    core: &QueryReadContext<'_>,
    request: &Request,
    dates: (Option<NaiveDate>, Option<NaiveDate>),
    loaded: &Loaded,
) -> Result<Value> {
    let as_of = Utc::now();
    let versions = pricing::versions(&core.db)?;
    let offset = FixedOffset::east_opt(8 * 3600).unwrap();
    let mut facts = Vec::with_capacity(loaded.responses.len());
    for response in &loaded.responses {
        let time = DateTime::parse_from_rfc3339(&response.event_time)
            .map_err(|_| "响应时间无法解析，原始用量记录已保留".to_string())?
            .with_timezone(&offset);
        facts.push(Fact {
            response,
            time,
            key: response_key(response),
            cost: pricing::estimate(&versions, response)?,
        });
    }
    let today = as_of.with_timezone(&offset).date_naive();
    let first = facts.iter().map(|fact| fact.time.date_naive()).min();
    let last = facts.iter().map(|fact| fact.time.date_naive()).max();
    let from = dates.0.or(first).or(dates.1).unwrap_or(today);
    let to = dates.1.or(last).or(dates.0).unwrap_or(today);
    validate_span(from, to)?;
    let hourly = from == to;
    let bucket_for = |time: &DateTime<FixedOffset>| {
        if hourly {
            time.format("%Y-%m-%dT%H:00:00+08:00").to_string()
        } else {
            time.format("%Y-%m-%d").to_string()
        }
    };
    let mut trend = BTreeMap::<String, Group>::new();
    if hourly {
        for hour in 0..24 {
            trend.insert(format!("{from}T{hour:02}:00:00+08:00"), Group::default());
        }
    } else {
        let mut date = from;
        loop {
            trend.insert(date.to_string(), Group::default());
            if date == to {
                break;
            }
            date = date.succ_opt().ok_or("统计日期超出范围")?;
        }
    }
    let mut total = CostSum::default();
    let mut models = BTreeMap::<String, Group>::new();
    let mut providers = BTreeMap::<String, Group>::new();
    let timings = TurnTimings::new(loaded);
    for fact in &facts {
        total.add(&fact.cost)?;
        trend
            .entry(bucket_for(&fact.time))
            .or_default()
            .add(fact.response, &fact.cost)?;
        let time_to_first_token = timings.metric(fact.response, "timeToFirstTokenMs");
        let model_group = models
            .entry(
                fact.response
                    .requested_model
                    .as_deref()
                    .unwrap_or("unknown")
                    .into(),
            )
            .or_default();
        model_group.add(fact.response, &fact.cost)?;
        model_group.add_timing(time_to_first_token)?;
        let provider_group = providers
            .entry(
                fact.response
                    .provider_id
                    .as_deref()
                    .unwrap_or("unknown")
                    .into(),
            )
            .or_default();
        provider_group.add(fact.response, &fact.cost)?;
        provider_group.add_timing(time_to_first_token)?;
    }
    let clients: BTreeMap<_, _> = loaded
        .sources
        .iter()
        .map(|source| (strv(source, "id"), strv(source, "clientKind")))
        .collect();
    let privacy = core.settings()?["privacyMode"].as_bool() == Some(true);
    let (projects, sessions) = scope_groups(&facts, loaded, &timings, privacy)?;
    let forecast_basis = forecast_basis(&facts, &timings, from, to, as_of)?;
    facts.sort_by(|left, right| {
        right
            .time
            .cmp(&left.time)
            .then_with(|| left.key.cmp(&right.key))
    });
    let total_rows = facts
        .iter()
        .filter(|fact| request.accepts(&fact.cost))
        .count();
    let start = request
        .page
        .checked_mul(request.page_size)
        .ok_or("统计分页超出范围")?;
    let rows: Vec<Value> = facts.iter().filter(|fact| request.accepts(&fact.cost))
        .skip(start).take(request.page_size).map(|fact| {
            let response = fact.response;
            let timing = |key| timings.metric(response,key);
            json!({"id":hash(fact.key.as_bytes()),"sourceId":if privacy{None}else{Some(&response.source_id)},
                "threadId":if privacy{None}else{Some(&response.thread_id)},
                "responseId":if privacy{None}else{Some(&response.response_id)},
                "eventTime":response.event_time,"clientKind":clients.get(&response.source_id).map(String::as_str).unwrap_or("unknown"),
                "providerId":response.provider_id,"requestedModel":response.requested_model,"observedModel":response.observed_model,
                "inputTokens":response.counts.input.value(),"normalInputTokens":normal_input(response),
                "outputTokens":response.counts.output.value(),"cachedInputTokens":response.counts.cache_read.value(),
                "cacheWriteInputTokens":response.counts.cache_write.value(),"totalTokens":response.counts.total.value(),
                "reasoningOutputTokens":response.counts.reasoning.value(),"estimatedCostUsd":fact.cost.estimated_cost_usd,
                "pricingVersionId":fact.cost.pricing_version_id,"priceReason":fact.cost.reason,
                "turnDurationMs":timing("durationMs"),"turnTimeToFirstTokenMs":timing("timeToFirstTokenMs"),
                "timingScope":"turn","speedTokensPerSecond":Value::Null})
        }).collect();
    let trend: Vec<Value> = trend.into_iter().map(|(bucket,group)| {
        let label = if hourly { bucket[11..16].to_owned() } else { bucket[5..].replace('-',"/") };
        json!({"bucket":bucket,"label":label,"totalTokens":group.tokens.total,
            "responseCount":group.tokens.count,"estimatedCostUsd":group.cost.value(),"pricedResponses":group.cost.priced,
            "unpricedResponses":group.tokens.count-group.cost.priced})
    }).collect();
    let groups = |groups: BTreeMap<String, Group>| -> Vec<Value> {
        let mut groups: Vec<_> = groups
            .into_iter()
            .map(|(key, group)| {
                let name = if key == "unknown" { "未知" } else { &key };
                group.value(&key, name)
            })
            .collect();
        groups.sort_by(|left, right| {
            right["responseCount"]
                .as_u64()
                .cmp(&left["responseCount"].as_u64())
                .then_with(|| left["key"].as_str().cmp(&right["key"].as_str()))
        });
        groups
    };
    Ok(
        json!({"asOf":as_of.to_rfc3339(),"request":request.value(),"totalRows":total_rows,
        "summary":{"estimatedCostUsd":total.value(),"pricedResponses":total.priced,"unpricedResponses":facts.len()-total.priced},
        "granularity":if hourly{"hour"}else{"day"},"timezone":"Asia/Shanghai","from":from.to_string(),"to":to.to_string(),
        "trend":trend,"rows":rows,"models":groups(models),"providers":groups(providers),
        "projects":projects,"sessions":sessions,"forecastBasis":forecast_basis,
        "modelKind":"requested","pricingVersions":versions,"costScope":"trusted-metered-responses",
        "timingScope":"turn","source":"local-response-ledger"}),
    )
}

impl Core {
    pub fn usage_dashboard_snapshot(&self, query: Value, request: Value) -> Result<Value> {
        usage_dashboard_snapshot_read(&self.query_read_context(), query, request)
    }
}

pub(super) fn snapshot_read(core: &QueryReadContext<'_>, query: Value, request: Value, include_tools: bool) -> Result<Value> {
    let request = Request::parse(&request)?;
    let dates = validate_dates(&query)?;
    let loaded = load_read(core, &query)?;
    let mut dashboard = build(core, &request, dates, &loaded)?;
    let mut data = snapshot_from_loaded(core, &query, loaded, include_tools)?;
    let pricing_summary = dashboard["summary"].take();
    dashboard["summary"] = data["overview"].clone();
    for (key, value) in pricing_summary.as_object().unwrap() {
        dashboard["summary"][key] = value.clone();
    }
    data["usageDashboard"] = dashboard;
    Ok(data)
}

#[cfg(test)]
#[path = "usage_dashboard_tests.rs"]
mod tests;
