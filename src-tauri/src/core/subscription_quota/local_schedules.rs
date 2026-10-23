//! 本地日程独立于官方额度。Token 实例复用标准计划，其他单位只接受明确的手动账本。
use super::*;
use chrono::{Datelike, FixedOffset, TimeZone};
use serde::de::DeserializeOwned;

const MAX_TEMPLATES: usize = 32;
const MAX_SEGMENTS: usize = 64;
const MAX_ENTRIES: usize = 1024;
const MAX_INSTANCE_VIEWS: usize = 256;
const DOMAIN_REVISION: &str = "localSchedulesRevision";
const LAST_CLOCK: &str = "localSchedulesLastUtc";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Rule {
    kind: String,
    anchor_at: String,
    ends_at: Option<String>,
    timezone: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Allocation {
    start_minute: u32,
    end_minute: u32,
    weight_bps: u32,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SaveInput {
    id: Option<String>,
    expected_revision: Option<i64>,
    expected_scope: String,
    source_id: String,
    title: String,
    unit: String,
    model: Option<String>,
    model_dimension: String,
    rule: Rule,
    start_policy: String,
    target: String,
    auto_renew: bool,
    allocation: Vec<Allocation>,
    reminders_enabled: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StatusInput {
    source_id: String,
    id: String,
    expected_revision: i64,
    state: String,
    expected_scope: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecordInput {
    source_id: String,
    instance_id: String,
    idempotency_key: String,
    occurred_at: String,
    amount: String,
    note: String,
    expected_scope: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VoidInput {
    source_id: String,
    entry_id: String,
    idempotency_key: String,
    expected_revision: i64,
    expected_scope: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Template {
    id: String,
    revision: i64,
    source_id: String,
    title: String,
    unit: String,
    model: Option<String>,
    model_dimension: String,
    rule: Rule,
    start_policy: String,
    target: String,
    auto_renew: bool,
    allocation: Vec<Allocation>,
    reminders_enabled: bool,
    state: String,
    created_at: String,
    updated_at: String,
    effective_from: String,
    first_starts_at: String,
    first_period_start: String,
    configuration_revision: String,
    binding_key: String,
    schedule_epoch: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Instance {
    id: String,
    template_id: String,
    template_revision: i64,
    source_id: String,
    title: String,
    unit: String,
    target: String,
    model: Option<String>,
    model_dimension: String,
    effective_from: String,
    period_starts_at: String,
    period_ends_at: String,
    ended_at: Option<String>,
    reminders_enabled: bool,
    token_plan_id: Option<String>,
    allocation: Vec<Allocation>,
    binding_key: String,
    configuration_revision: String,
    created_at: String,
    latched: Vec<i64>,
}
#[derive(Clone)]
struct Binding {
    id: String,
    root: String,
    kind: String,
    environment: String,
    enabled: bool,
    revision: i64,
}
impl Binding {
    fn key(&self) -> String {
        hash(
            json!([
                self.id,
                self.root,
                self.kind,
                self.environment,
                self.enabled,
                self.revision.to_string()
            ])
            .to_string()
            .as_bytes(),
        )
    }
    fn view(&self) -> Value {
        json!({"id":self.id,"clientKind":self.kind,"enabled":self.enabled,"configurationRevision":self.revision.to_string()})
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
struct Period {
    start: chrono::DateTime<Utc>,
    end: chrono::DateTime<Utc>,
}

fn input<T: DeserializeOwned>(value: &Value) -> Result<T> {
    serde_json::from_value(value.clone()).map_err(|_| "日程输入字段、类型或版本无效".into())
}
fn local_text(value: &str, max: usize) -> bool {
    text_ok(value,max) && value.encode_utf16().count()<=max && !value.chars().any(|c|matches!(c,'\u{061c}'|'\u{200e}'|'\u{200f}'|'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}'))
}
fn uuid_key(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| id.hyphenated().to_string() == value)
}
fn binding(db: &Connection, source: &str) -> Result<Binding> {
    let value=db.query_row("SELECT id,root,client_kind,enabled,configuration_revision,environment FROM source_instances WHERE id=?1",[source],|r|Ok(Binding{id:r.get(0)?,root:r.get(1)?,kind:r.get(2)?,enabled:r.get(3)?,revision:r.get(4)?,environment:r.get(5)?})).optional().map_err(err)?.ok_or("日程来源不存在")?;
    if !(0..=MAX_SAFE_INTEGER).contains(&value.revision) {
        return Err("来源配置修订无效".into());
    }
    Ok(value)
}
fn domain_revision(db: &Connection) -> Result<i64> {
    match get_setting(db, DOMAIN_REVISION)? {
        None => Ok(0),
        Some(value) => value
            .as_i64()
            .filter(|value| (0..=MAX_SAFE_INTEGER).contains(value))
            .ok_or("日程修订无效".into()),
    }
}
fn advance_domain(db: &Connection) -> Result<()> {
    put_setting(
        db,
        DOMAIN_REVISION,
        &json!(next_revision(domain_revision(db)?)?),
    )
}
fn latest_templates(db: &Connection) -> Result<Vec<Template>> {
    let mut statement=db.prepare("SELECT t.data FROM local_schedule_templates t JOIN (SELECT id,MAX(revision) rev FROM local_schedule_templates GROUP BY id) h ON t.id=h.id AND t.revision=h.rev ORDER BY t.id LIMIT 1001").map_err(err)?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(err)?;
    let values = rows
        .map(|row| {
            serde_json::from_str::<Template>(&row.map_err(err)?)
                .map_err(|_| "日程模板存储无效".to_owned())
        })
        .collect::<Result<Vec<_>>>()?;
    if values.len() > 1000 {
        return Err("日程模板历史超过有界读取上限".into());
    }
    Ok(values)
}
fn template(db: &Connection, id: &str) -> Result<Template> {
    latest_templates(db)?
        .into_iter()
        .find(|value| value.id == id)
        .ok_or("日程模板不存在".into())
}
fn save_template(db: &Connection, value: &Template) -> Result<()> {
    db.execute(
        "INSERT INTO local_schedule_templates(id,revision,source_id,data) VALUES(?1,?2,?3,?4)",
        params![
            value.id,
            value.revision,
            value.source_id,
            serde_json::to_string(value).map_err(err)?
        ],
    )
    .map_err(err)?;
    Ok(())
}
fn instance(db: &Connection, id: &str) -> Result<Instance> {
    let value: String = db
        .query_row(
            "SELECT data FROM local_schedule_instances WHERE id=?1",
            [id],
            |row| row.get(0),
        )
        .optional()
        .map_err(err)?
        .ok_or("日程实例不存在")?;
    serde_json::from_str(&value).map_err(|_| "日程实例存储无效".into())
}
fn save_instance(db: &Connection, value: &Instance) -> Result<()> {
    db.execute(
        "UPDATE local_schedule_instances SET data=?1 WHERE id=?2",
        params![serde_json::to_string(value).map_err(err)?, value.id],
    )
    .map_err(err)?;
    Ok(())
}

fn amount_minor(unit: &str, value: &str) -> Result<i128> {
    let (whole, fraction) = if unit == "credits" {
        value.split_once('.').map_or((value, ""), |(a, b)| (a, b))
    } else {
        (value, "")
    };
    if !["tokens", "requests", "credits"].contains(&unit)
        || whole.is_empty()
        || whole.len() > 16
        || (whole.len() > 1 && whole.starts_with('0'))
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || fraction.len() > 6
        || !fraction.bytes().all(|b| b.is_ascii_digit())
        || (unit != "credits" && value.contains('.'))
        || (value.contains('.') && fraction.is_empty())
    {
        return Err("数量必须是非负十进制；credits 最多六位小数".into());
    }
    let integer = whole.parse::<i128>().map_err(|_| "数量超出范围")?;
    let quantity = if unit == "credits" {
        integer.checked_mul(1_000_000).and_then(|n| {
            fraction
                .parse::<i128>()
                .unwrap_or(0)
                .checked_mul(10_i128.pow((6 - fraction.len()) as u32))
                .and_then(|f| n.checked_add(f))
        })
    } else {
        Some(integer)
    }
    .filter(|value| *value <= i128::from(MAX_SAFE_INTEGER))
    .ok_or("数量微单位超过安全整数范围")?;
    Ok(quantity)
}
fn amount_text(unit: &str, value: i128) -> String {
    if unit == "credits" {
        format!("{}.{:06}", value / 1_000_000, value % 1_000_000)
    } else {
        value.to_string()
    }
}
fn month_boundary(anchor: chrono::DateTime<Utc>, offset: i64) -> Result<chrono::DateTime<Utc>> {
    let zone = FixedOffset::east_opt(8 * 3600).unwrap();
    let local = anchor.with_timezone(&zone);
    let index = i64::from(local.year())
        .checked_mul(12)
        .and_then(|n| n.checked_add(i64::from(local.month0())))
        .and_then(|n| n.checked_add(offset))
        .ok_or("月份超出范围")?;
    let year = i32::try_from(index.div_euclid(12)).map_err(|_| "月份超出范围")?;
    if !(1..=9999).contains(&year) {
        return Err("月份超出支持的日历范围".into());
    }
    let month = index.rem_euclid(12) as u32 + 1;
    let mut day = local.day();
    let date = loop {
        if let Some(date) = chrono::NaiveDate::from_ymd_opt(year, month, day) {
            break date;
        }
        day = day.checked_sub(1).ok_or("月份无有效日期")?;
    };
    zone.from_local_datetime(&date.and_time(local.time()))
        .single()
        .map(|value| value.with_timezone(&Utc))
        .ok_or("月份边界无效".into())
}
fn period_at(rule: &Rule, at: chrono::DateTime<Utc>) -> Result<Option<Period>> {
    let anchor = time(&rule.anchor_at)?;
    if rule.kind == "once" {
        let end = time(rule.ends_at.as_deref().ok_or("一次日程缺少结束时间")?)?;
        return Ok((at < end).then_some(Period { start: anchor, end }));
    }
    let index = if at < anchor {
        0
    } else if rule.kind == "monthly" {
        let zone = FixedOffset::east_opt(8 * 3600).unwrap();
        let a = anchor.with_timezone(&zone);
        let b = at.with_timezone(&zone);
        let mut index =
            i64::from(b.year() - a.year()) * 12 + i64::from(b.month0()) - i64::from(a.month0());
        if month_boundary(anchor, index)? > at {
            index -= 1;
        }
        index.max(0)
    } else {
        let seconds = if rule.kind == "daily" {
            86_400
        } else {
            604_800
        };
        (at - anchor).num_seconds().div_euclid(seconds)
    };
    let boundary = |index: i64| -> Result<_> {
        if rule.kind == "monthly" {
            month_boundary(anchor, index)
        } else {
            let days = if rule.kind == "daily" { 1_i64 } else { 7_i64 };
            anchor
                .checked_add_signed(
                    Duration::try_days(index.checked_mul(days).ok_or("周期超出范围")?)
                        .ok_or("周期超出范围")?,
                )
                .ok_or("周期超出范围".into())
        }
    };
    Ok(Some(Period {
        start: boundary(index)?,
        end: boundary(index.checked_add(1).ok_or("周期超出范围")?)?,
    }))
}
fn first_period(
    rule: &Rule,
    policy: &str,
    at: chrono::DateTime<Utc>,
) -> Result<(Period, chrono::DateTime<Utc>)> {
    let mut period = period_at(rule, at)?.ok_or("日程已经结束")?;
    if policy == "next_period" && at >= period.start {
        period = period_at(rule, period.end)?.ok_or("一次日程没有下一周期")?;
    }
    Ok((period, period.start.max(at)))
}
fn validate_save(p: &mut SaveInput) -> Result<i128> {
    if !local_text(p.title.trim(), 80)
        || p.model
            .as_ref()
            .is_some_and(|value| !local_text(value, 512))
        || !["response", "request"].contains(&p.model_dimension.as_str())
        || !["now", "next_period"].contains(&p.start_policy.as_str())
        || p.rule.timezone != "+08:00"
        || !["once", "daily", "weekly", "monthly"].contains(&p.rule.kind.as_str())
    {
        return Err("日程名称、模型或上海时区规则无效".into());
    }
    let target = amount_minor(&p.unit, &p.target)?;
    if target == 0 {
        return Err("计划目标必须大于零".into());
    }
    if p.unit != "tokens" && p.reminders_enabled {
        return Err("手动 requests/credits 日程暂不支持提醒".into());
    }
    let anchor = time(&p.rule.anchor_at)?;
    p.rule.anchor_at = timestamp(anchor);
    let max_minutes = if p.rule.kind == "once" {
        if p.auto_renew || p.start_policy != "now" {
            return Err("一次日程不能自动续建或选择下一周期".into());
        }
        let end = time(
            p.rule
                .ends_at
                .as_deref()
                .ok_or("一次日程需要明确结束时间")?,
        )?;
        if end <= anchor {
            return Err("日程结束时间必须晚于起点".into());
        }
        p.rule.ends_at = Some(timestamp(end));
        (end - anchor).num_minutes()
    } else {
        if p.rule.ends_at.is_some() {
            return Err("重复日程不接受一次性结束时间".into());
        }
        match p.rule.kind.as_str() {
            "daily" => 1440,
            "weekly" => 10080,
            _ => 40320,
        }
    };
    if p.allocation.len() > MAX_SEGMENTS {
        return Err("自由日程最多 64 段".into());
    }
    p.allocation.sort_by_key(|value| value.start_minute);
    let mut prior = 0_u32;
    let mut weights = 0_u32;
    for segment in &p.allocation {
        if segment.start_minute < prior
            || segment.end_minute <= segment.start_minute
            || i64::from(segment.end_minute) > max_minutes
            || segment.weight_bps == 0
        {
            return Err("自由日程必须按分钟不重叠，且不能越过实际周期；月曲线最多28天".into());
        }
        prior = segment.end_minute;
        weights = weights
            .checked_add(segment.weight_bps)
            .ok_or("分配权重超出范围")?;
    }
    if !p.allocation.is_empty() && weights != 10000 {
        return Err("分配权重总和必须为 10000".into());
    }
    p.target = amount_text(&p.unit, target);
    p.title = p.title.trim().into();
    Ok(target)
}

fn last_clock(db: &Connection) -> Result<Option<chrono::DateTime<Utc>>> {
    get_setting(db, LAST_CLOCK)?
        .map(|value| {
            value
                .as_str()
                .ok_or("日程时钟记录无效")
                .and_then(|value| time(value).map_err(|_| "日程时钟记录无效"))
        })
        .transpose()
        .map_err(str::to_owned)
}
fn clock_rollback(db: &Connection, at: chrono::DateTime<Utc>) -> Result<bool> {
    Ok(last_clock(db)?.is_some_and(|last| at < last))
}
fn binding_state(current: &Binding, key: &str) -> &'static str {
    if !current.enabled {
        "source_unavailable"
    } else if current.key() != key {
        "pending_rebind"
    } else {
        "ready"
    }
}
fn instance_end(value: &Instance) -> Result<chrono::DateTime<Utc>> {
    let end = time(&value.period_ends_at)?;
    Ok(value
        .ended_at
        .as_deref()
        .map(time)
        .transpose()?
        .map_or(end, |closed| closed.min(end)))
}
fn insert_instance(db: &Connection, value: &Instance, epoch: &str) -> Result<()> {
    db.execute("INSERT INTO local_schedule_instances(id,template_id,template_revision,source_id,schedule_epoch,period_start,period_end,token_plan_id,data) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![value.id,value.template_id,value.template_revision,value.source_id,epoch,value.period_starts_at,value.period_ends_at,value.token_plan_id,serde_json::to_string(value).map_err(err)?]).map_err(err)?;
    Ok(())
}
fn instances_for_template(db: &Connection, id: &str) -> Result<Vec<Instance>> {
    // 不预建未来实例且实例不重叠，因此续建、编辑和停用只需最后一个实例。
    let mut statement=db.prepare("SELECT data FROM local_schedule_instances WHERE template_id=?1 ORDER BY period_start DESC,id DESC LIMIT 1").map_err(err)?;
    let rows = statement
        .query_map([id], |row| row.get::<_, String>(0))
        .map_err(err)?;
    let values = rows
        .map(|row| {
            serde_json::from_str::<Instance>(&row.map_err(err)?)
                .map_err(|_| "日程实例存储无效".into())
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(values)
}
fn create_due(core: &Core, db: &Connection, value: &Template, reading: Reading) -> Result<bool> {
    let at = reading.utc;
    if value.state != "active"
        || binding_state(&binding(db, &value.source_id)?, &value.binding_key) != "ready"
        || at < time(&value.first_starts_at)?
        || at < time(&value.effective_from)?
    {
        return Ok(false);
    }
    let Some(period) = period_at(&value.rule, at)? else {
        return Ok(false);
    };
    if period.start > at || period.start < time(&value.first_period_start)? {
        return Ok(false);
    }
    let existing = instances_for_template(db, &value.id)?;
    if existing
        .iter()
        .any(|instance| time(&instance.period_starts_at).is_ok_and(|start| start >= period.start))
        || (!value.auto_renew && !existing.is_empty())
    {
        return Ok(false);
    }
    // 编辑在旧实例结束后才生效；新锚点不能生成重叠实例或重置当前预算。
    if existing
        .iter()
        .any(|instance| instance_end(instance).is_ok_and(|end| end > period.start))
    {
        return Ok(false);
    }
    let effective = if timestamp(period.start) == value.first_period_start {
        time(&value.first_starts_at)?.max(period.start)
    } else {
        period.start
    };
    if effective >= period.end {
        return Ok(false);
    }
    let id = hash(
        json!([value.id, value.schedule_epoch, timestamp(period.start)])
            .to_string()
            .as_bytes(),
    );
    let token_id = (value.unit == "tokens").then(|| id.clone());
    let instance = Instance {
        id: id.clone(),
        template_id: value.id.clone(),
        template_revision: value.revision,
        source_id: value.source_id.clone(),
        title: value.title.clone(),
        unit: value.unit.clone(),
        target: value.target.clone(),
        model: value.model.clone(),
        model_dimension: value.model_dimension.clone(),
        effective_from: timestamp(effective),
        period_starts_at: timestamp(period.start),
        period_ends_at: timestamp(period.end),
        ended_at: None,
        reminders_enabled: value.reminders_enabled,
        token_plan_id: token_id.clone(),
        allocation: value.allocation.clone(),
        binding_key: value.binding_key.clone(),
        configuration_revision: value.configuration_revision.clone(),
        created_at: timestamp(at),
        latched: Vec::new(),
    };
    if let Some(plan_id) = token_id {
        let count = latest_plans(db)?.iter().try_fold(0_usize, |count, plan| {
            Ok::<_, String>(count + usize::from(active_configuration(db, plan, at)?))
        })?;
        if count >= MAX_PLANS as usize {
            return Ok(false);
        }
        let target = amount_minor("tokens", &value.target)? as i64;
        let config = json!({"id":plan_id,"revision":1,"type":"tokens","sourceId":value.source_id,"title":value.title,
            "model":value.model,"modelDimension":value.model_dimension,"target":target,"effectiveFrom":timestamp(effective),
            "endPolicy":"time","endsAt":timestamp(period.end),"windowId":null,"periodInstanceId":null,"lifecycle":"active","endedAt":null,
            "ledgerBaselineRevision":revision(db)?,"remindersEnabled":value.reminders_enabled,"updatedAt":timestamp(at),
            "origin":{"kind":"local_schedule","templateId":value.id,"templateRevision":value.revision,"instanceId":id,"sourceConfigurationRevision":value.configuration_revision}});
        save_config(db, &config)?;
        // 首次实例化只建立基线；离线补读、重启或已过的阈值不会被当作新越线。
        let responses = queries::subscription_responses(core)?;
        let view = plan_view(core, &config, &responses, at)?;
        let mut state = state(db, &plan_id)?;
        state["used"] = view["used"].clone();
        state["runtimeId"] = json!(core.runtime_id);
        state["baselineAt"] = json!(timestamp(at));
        state["baselineOnly"] = json!(true);
        state["officialBaselinePending"] = json!(true);
        save_state(db, &plan_id, &state)?;
        super::super::local_plan_forecast::observe(
            db,
            &config,
            &view,
            &core.runtime_id,
            &reading,
            true,
        )?;
        super::super::demand_history::initialize(
            db,
            &config,
            core.subscription.current.as_ref(),
            &core.runtime_id,
            &reading,
        )?;
        advance_revision(db)?;
    }
    insert_instance(db, &instance, &value.schedule_epoch)?;
    Ok(true)
}

fn planned(value: &Instance, at: chrono::DateTime<Utc>) -> Result<(i128, bool)> {
    let target = amount_minor(&value.unit, &value.target)?;
    let start = time(&value.effective_from)?;
    let end = time(&value.period_ends_at)?;
    let at = at.min(instance_end(value)?);
    if end <= start {
        return Ok((0, true));
    }
    if value.allocation.is_empty() {
        let elapsed = (at.min(end) - start).num_milliseconds().max(0) as i128;
        let duration = (end - start).num_milliseconds() as i128;
        return Ok((
            target.checked_mul(elapsed).ok_or("计划曲线超出整数范围")? / duration,
            false,
        ));
    }
    let nominal = time(&value.period_starts_at)?;
    let mut segments = Vec::new();
    const SCALE: i128 = 1_000_000_000_000_000;
    for segment in &value.allocation {
        let original_start = nominal
            .checked_add_signed(Duration::minutes(i64::from(segment.start_minute)))
            .ok_or("曲线时间超出范围")?;
        let original_end = nominal
            .checked_add_signed(Duration::minutes(i64::from(segment.end_minute)))
            .ok_or("曲线时间超出范围")?;
        if original_end > end {
            return Err("已保存曲线越过实际周期".into());
        }
        let clipped = original_start.max(start);
        if original_end <= clipped {
            continue;
        }
        let remaining = (original_end - clipped).num_milliseconds() as i128;
        let duration = (original_end - original_start).num_milliseconds() as i128;
        let weight = i128::from(segment.weight_bps)
            .checked_mul(remaining)
            .and_then(|n| n.checked_mul(SCALE))
            .ok_or("曲线权重超出整数范围")?
            / duration;
        segments.push((clipped, original_end, weight.max(1)));
    }
    let weight: i128 = segments.iter().map(|segment| segment.2).sum();
    if weight == 0 {
        return Ok((0, true));
    }
    let mut allocated = 0_i128;
    let mut result = 0_i128;
    for (index, (begin, end, score)) in segments.iter().enumerate() {
        let amount = if index + 1 == segments.len() {
            target - allocated
        } else {
            target.checked_mul(*score).ok_or("曲线分配超出整数范围")? / weight
        };
        allocated += amount;
        let elapsed = (at.min(*end) - *begin).num_milliseconds().max(0) as i128;
        let duration = (*end - *begin).num_milliseconds() as i128;
        result = result
            .checked_add(amount.checked_mul(elapsed).ok_or("曲线累计超出整数范围")? / duration)
            .ok_or("曲线累计超出整数范围")?;
    }
    Ok((result.min(target), false))
}
fn manual_entries(db: &Connection, value: &Instance, private: bool) -> Result<(Vec<Value>, i128)> {
    let mut statement=db.prepare("SELECT c.id,c.data,v.id FROM local_manual_consumption c LEFT JOIN local_manual_consumption v ON v.target_entry_id=c.id AND v.kind='void' WHERE c.instance_id=?1 AND c.kind='consume' ORDER BY c.event_time,c.id LIMIT 1025").map_err(err)?;
    let rows = statement
        .query_map([&value.id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(err)?;
    let mut entries = Vec::new();
    let mut used = 0_i128;
    for row in rows {
        let (id, data, void) = row.map_err(err)?;
        let data: Value = serde_json::from_str(&data).map_err(|_| "手动账本存储无效")?;
        let amount = data["amountMinor"]
            .as_str()
            .and_then(|value| value.parse::<i128>().ok())
            .filter(|value| *value > 0)
            .ok_or("手动账本数量无效")?;
        if void.is_none() {
            used = used.checked_add(amount).ok_or("手动累计数量超出范围")?;
        }
        entries.push(json!({"id":id,"revision":if void.is_some(){2}else{1},"occurredAt":data["occurredAt"],"amount":amount_text(&value.unit,amount),
            "note":if private {Value::String(String::new())} else {data["note"].clone()},"createdAt":data["createdAt"],"voided":void.is_some()}));
    }
    if entries.len() > MAX_ENTRIES {
        return Err("手动账本超过有界上限".into());
    }
    Ok((entries, used))
}
fn request_replay(db: &Connection, id: &str, digest: &str) -> Result<bool> {
    let old: Option<String> = db
        .query_row(
            "SELECT data FROM local_manual_consumption WHERE id=?1",
            [id],
            |row| row.get(0),
        )
        .optional()
        .map_err(err)?;
    let Some(old) = old else { return Ok(false) };
    let data: Value = serde_json::from_str(&old).map_err(|_| "手动账本存储无效")?;
    if data["requestDigest"] != digest {
        return Err("幂等键已被不同操作或来源使用".into());
    }
    Ok(true)
}
fn instance_view(
    core: &Core,
    value: &Instance,
    responses: &[ResponseUsage],
    at: chrono::DateTime<Utc>,
    private: bool,
) -> Result<Value> {
    let current = binding(&core.db, &value.source_id)?;
    let mut reasons = Vec::<String>::new();
    let bound = binding_state(&current, &value.binding_key);
    if bound != "ready" {
        reasons.push(bound.into());
    }
    if clock_rollback(&core.db, at)? {
        reasons.push("clock_rollback".into());
    }
    let target = amount_minor(&value.unit, &value.target)?;
    let (planned, no_segments) = planned(value, at)?;
    if no_segments {
        reasons.push("no_available_allocation".into());
    }
    let (entries, recorded) = if value.unit == "tokens" {
        (Vec::new(), None)
    } else {
        let (entries, total) = manual_entries(&core.db, value, private)?;
        (entries, Some(total))
    };
    let (mut used, mut evaluation) = if let Some(id) = value.token_plan_id.as_deref() {
        let config = current_plan(&core.db, id)?;
        let view = plan_view(core, &config, responses, at)?;
        if let Some(values) = view["reasons"].as_array() {
            reasons.extend(values.iter().filter_map(Value::as_str).map(str::to_owned));
        }
        (
            view["used"]
                .as_f64()
                .filter(|value| value.is_finite() && *value >= 0.0 && value.fract() == 0.0)
                .map(|value| value as i128),
            strv(&view, "evaluationState"),
        )
    } else {
        (
            recorded,
            if bound == "ready" {
                "ready".into()
            } else {
                "unknown".into()
            },
        )
    };
    if bound != "ready" || reasons.iter().any(|reason| reason == "clock_rollback") {
        used = None;
        evaluation = "unknown".into();
    }
    let observed = last_clock(&core.db)?.unwrap_or(at).max(at);
    let end = instance_end(value)?;
    let ended = value.ended_at.is_some() || observed >= end;
    let remaining = used.map(|used| (target - used).max(0));
    let progress = used.map(|used| 100.0 * used as f64 / target as f64);
    reasons.sort();
    reasons.dedup();
    Ok(
        json!({"id":value.id,"templateId":value.template_id,"templateRevision":value.template_revision,"sourceId":value.source_id,
        "title":if private {"本地日程"}else{&value.title},"unit":value.unit,"target":value.target,"model":if private {None}else{value.model.as_ref()},
        "modelDimension":value.model_dimension,"effectiveFrom":value.effective_from,"periodStartsAt":value.period_starts_at,"periodEndsAt":value.period_ends_at,
        "endedAt":if ended {Some(timestamp(end))} else {None},"lifecycle":if ended {"ended"} else {"active"},"evaluationState":evaluation,
        "used":used.map(|used|amount_text(&value.unit,used)),"actualUsed":if value.unit=="tokens" {used.map(|used|amount_text(&value.unit,used))} else {None},
        "recordedUsed":recorded.or(used).map(|used|amount_text(&value.unit,used)),"remaining":remaining.map(|used|amount_text(&value.unit,used)),"progressPercent":progress,
        "plannedByNow":amount_text(&value.unit,planned),"measurement":if value.unit=="tokens" {"source-recorded"} else {"manual-partial"},
        "remindersEnabled":value.reminders_enabled,"tokenPlanId":value.token_plan_id,"reasons":reasons,"entries":entries}),
    )
}

impl Core {
    pub(crate) fn local_schedule_scope_token(&self, source: &str) -> Result<String> {
        let source = binding(&self.db, source)?;
        Ok(hash(
            json!([
                self.runtime_id,
                self.subscription.context_generation.to_string(),
                source.key(),
                self.source_id,
                get_setting(&self.db, "clearWatermark")?,
                self.settings()?["privacyMode"]
            ])
            .to_string()
            .as_bytes(),
        ))
    }
    fn require_local_scope(&self, source: &str, expected: &str) -> Result<Binding> {
        if self.settings()?["privacyMode"] == true {
            return Err("隐私模式下不能修改本地日程或手动账本".into());
        }
        if expected != self.local_schedule_scope_token(source)? {
            return Err("日程范围已变化，请重新读取".into());
        }
        let value = binding(&self.db, source)?;
        if !value.enabled {
            return Err("日程来源已停用".into());
        }
        Ok(value)
    }
    pub(crate) fn read_local_schedules(&self, source: &str) -> Result<Value> {
        let current = binding(&self.db, source)?;
        let at = budget_clock::now().utc;
        let private = self.settings()?["privacyMode"] == true;
        let responses = queries::subscription_responses(self)?;
        let mut templates = Vec::new();
        for value in latest_templates(&self.db)?
            .into_iter()
            .filter(|value| value.source_id == source)
        {
            let state = binding_state(&current, &value.binding_key);
            let instances = instances_for_template(&self.db, &value.id)?;
            let next = if value.state != "active"
                || state != "ready"
                || (!value.auto_renew && !instances.is_empty())
            {
                None
            } else if at < time(&value.first_starts_at)? {
                Some(value.first_starts_at.clone())
            } else {
                period_at(&value.rule, at)?.map(|period| timestamp(period.end))
            };
            templates.push(json!({"id":value.id,"revision":value.revision,"sourceId":value.source_id,"title":if private {"本地日程"}else{&value.title},
                "unit":value.unit,"target":value.target,"model":if private {None}else{value.model.as_ref()},"modelDimension":value.model_dimension,
                "rule":value.rule,"startPolicy":value.start_policy,"autoRenew":value.auto_renew,"allocation":value.allocation,"remindersEnabled":value.reminders_enabled,
                "state":value.state,"createdAt":value.created_at,"updatedAt":value.updated_at,"effectiveFrom":value.effective_from,"firstStartsAt":value.first_starts_at,
                "configurationRevision":value.configuration_revision,"bindingState":state,"nextPeriodStartsAt":next}));
        }
        let mut statement=self.db.prepare("SELECT data FROM local_schedule_instances WHERE source_id=?1 ORDER BY period_start DESC,id DESC LIMIT 257").map_err(err)?;
        let rows = statement
            .query_map([source], |row| row.get::<_, String>(0))
            .map_err(err)?;
        let mut instances = Vec::new();
        let mut reasons = Vec::<String>::new();
        for row in rows {
            if instances.len() == MAX_INSTANCE_VIEWS {
                reasons.push("instance_history_truncated".into());
                break;
            }
            let value: Instance =
                serde_json::from_str(&row.map_err(err)?).map_err(|_| "日程实例存储无效")?;
            instances.push(instance_view(self, &value, &responses, at, private)?);
        }
        if !current.enabled {
            reasons.push("source_unavailable".into());
        }
        if clock_rollback(&self.db, at)? {
            reasons.push("clock_rollback".into());
        }
        Ok(
            json!({"schemaVersion":1,"revision":domain_revision(&self.db)?,"scopeToken":self.local_schedule_scope_token(source)?,"asOf":timestamp(at),
            "source":current.view(),"templates":templates,"instances":instances,"reasons":reasons}),
        )
    }
    pub(crate) fn save_local_schedule(&mut self, payload: &Value) -> Result<Value> {
        let mut p: SaveInput = input(payload)?;
        validate_save(&mut p)?;
        let current = self.require_local_scope(&p.source_id, &p.expected_scope)?;
        let reading = budget_clock::now();
        let at = reading.utc;
        if clock_rollback(&self.db, at)? {
            return Err("系统时钟回退，暂不创建或修改日程".into());
        }
        let tx = self.db.unchecked_transaction().map_err(err)?;
        let old = p.id.as_deref().map(|id| template(&tx, id)).transpose()?;
        let (id, revision, created, not_before, epoch) = if let Some(old) = old.as_ref() {
            if old.source_id != p.source_id || Some(old.revision) != p.expected_revision {
                return Err("日程已被修改或来源不同，请重新读取".into());
            }
            if old.state == "disabled" {
                return Err("已停用日程不能重新开始，请新建".into());
            }
            let existing = instances_for_template(&tx, &old.id)?;
            if old.rule.kind == "once" && !existing.is_empty() {
                return Err("一次日程开始后不可修改，请新建未来日程".into());
            }
            let not_before = existing
                .iter()
                .filter_map(|value| instance_end(value).ok())
                .filter(|end| *end > at)
                .max()
                .unwrap_or(at);
            let changed = old.rule != p.rule
                || old.unit != p.unit
                || old.model != p.model
                || old.model_dimension != p.model_dimension
                || old.binding_key != current.key();
            (
                old.id.clone(),
                next_revision(old.revision)?,
                old.created_at.clone(),
                not_before,
                if changed {
                    uuid::Uuid::new_v4().to_string()
                } else {
                    old.schedule_epoch.clone()
                },
            )
        } else {
            if p.expected_revision.is_some() {
                return Err("新日程不能引用旧修订".into());
            }
            let values = latest_templates(&tx)?;
            if values
                .iter()
                .filter(|value| value.state != "disabled")
                .count()
                >= MAX_TEMPLATES
                || values.len() >= 1000
            {
                return Err("最多32个未停用模板，模板历史最多1000项".into());
            }
            (
                uuid::Uuid::new_v4().to_string(),
                1,
                timestamp(at),
                at,
                uuid::Uuid::new_v4().to_string(),
            )
        };
        // 当前实例尚占用区间时紧接下一实例；历史已结束则尊重本次显式起点策略。
        let policy = if not_before > at {
            "now"
        } else {
            p.start_policy.as_str()
        };
        let (mut period, mut first) = first_period(&p.rule, policy, not_before)?;
        if old.is_some() && not_before > at && period.start < not_before {
            period = period_at(&p.rule, period.end)?.ok_or("修改后没有未来周期")?;
            first = period.start;
        }
        let value = Template {
            id,
            revision,
            source_id: p.source_id.clone(),
            title: p.title,
            unit: p.unit,
            model: p.model,
            model_dimension: p.model_dimension,
            rule: p.rule,
            start_policy: p.start_policy,
            target: p.target,
            auto_renew: p.auto_renew,
            allocation: p.allocation,
            reminders_enabled: p.reminders_enabled,
            state: old
                .as_ref()
                .map_or("active", |old| old.state.as_str())
                .into(),
            created_at: created,
            updated_at: timestamp(at),
            effective_from: timestamp(not_before),
            first_starts_at: timestamp(first),
            first_period_start: timestamp(period.start),
            configuration_revision: current.revision.to_string(),
            binding_key: current.key(),
            schedule_epoch: epoch,
        };
        save_template(&tx, &value)?;
        create_due(self, &tx, &value, reading)?;
        advance_domain(&tx)?;
        put_setting(&tx, LAST_CLOCK, &json!(timestamp(at)))?;
        #[cfg(test)]
        if get_setting(&tx, "testLocalScheduleFailBeforeCommit")?.and_then(|value| value.as_bool())
            == Some(true)
        {
            return Err("injected-local-schedule-before-commit".into());
        }
        tx.commit().map_err(err)?;
        self.read_local_schedules(&p.source_id)
    }
    pub(crate) fn set_local_schedule_status(&mut self, payload: &Value) -> Result<Value> {
        let p: StatusInput = input(payload)?;
        self.require_local_scope(&p.source_id, &p.expected_scope)?;
        if !["active", "paused", "disabled"].contains(&p.state.as_str()) {
            return Err("日程状态无效".into());
        }
        let at = budget_clock::now().utc;
        if clock_rollback(&self.db, at)? {
            return Err("系统时钟回退，暂不修改日程状态".into());
        }
        if p.state == "disabled" {
            self.revoke_subscription_deliveries();
        }
        let tx = self.db.unchecked_transaction().map_err(err)?;
        let mut value = template(&tx, &p.id)?;
        if value.source_id != p.source_id || value.revision != p.expected_revision {
            return Err("日程已被修改，请重新读取".into());
        }
        if value.state == "disabled" && p.state != "disabled" {
            return Err("已停用日程不能重新开始，请新建".into());
        }
        if value.state != p.state {
            value.revision = next_revision(value.revision)?;
            value.state = p.state.clone();
            value.updated_at = timestamp(at);
            if p.state == "disabled" {
                for mut instance in instances_for_template(&tx, &value.id)? {
                    if instance.ended_at.is_none() && instance_end(&instance)? > at {
                        instance.ended_at =
                            Some(timestamp(at.max(time(&instance.effective_from)?)));
                        save_instance(&tx, &instance)?;
                        if let Some(id) = instance.token_plan_id {
                            let mut plan = current_plan(&tx, &id)?;
                            plan["revision"] = json!(next_revision(
                                plan["revision"].as_i64().ok_or("计划修订无效")?
                            )?);
                            plan["lifecycle"] = json!("ended");
                            plan["endedAt"] = json!(timestamp(at));
                            save_config(&tx, &plan)?;
                            advance_revision(&tx)?;
                        }
                    }
                }
            }
            save_template(&tx, &value)?;
            advance_domain(&tx)?;
        }
        tx.commit().map_err(err)?;
        self.read_local_schedules(&p.source_id)
    }
    pub(crate) fn record_local_consumption(&mut self, payload: &Value) -> Result<Value> {
        let p: RecordInput = input(payload)?;
        let current = self.require_local_scope(&p.source_id, &p.expected_scope)?;
        if !uuid_key(&p.idempotency_key)
            || p.note.chars().count() > 500
            || (!p.note.is_empty() && !local_text(&p.note, 500))
        {
            return Err("手动记录的幂等键或备注无效".into());
        }
        let at = budget_clock::now().utc;
        let occurred = time(&p.occurred_at)?;
        let tx = self.db.unchecked_transaction().map_err(err)?;
        let value = instance(&tx, &p.instance_id)?;
        if value.source_id != p.source_id || value.unit == "tokens" {
            return Err("只能给同来源 requests/credits 实例记录手动消耗".into());
        }
        let amount = amount_minor(&value.unit, &p.amount)?;
        if amount == 0 {
            return Err("手动消耗必须大于零".into());
        }
        let digest = hash(
            json!([
                "consume",
                p.source_id,
                p.instance_id,
                timestamp(occurred),
                amount.to_string(),
                p.note
            ])
            .to_string()
            .as_bytes(),
        );
        if request_replay(&tx, &p.idempotency_key, &digest)? {
            drop(tx);
            return self.read_local_schedules(&p.source_id);
        }
        if binding_state(&current, &value.binding_key) != "ready" {
            return Err("实例绑定已变化，请重新建立未来日程".into());
        }
        if clock_rollback(&tx, at)? {
            return Err("系统时钟回退，暂不记录消耗".into());
        }
        if occurred < time(&value.effective_from)?
            || occurred >= instance_end(&value)?
            || occurred > at
        {
            return Err("消耗时间必须处于实例实际半开区间内，且不能晚于当前时间".into());
        }
        let (entries, _before) = manual_entries(&tx, &value, false)?;
        if entries.len() >= MAX_ENTRIES {
            return Err("每实例最多1024笔手动记录".into());
        }
        let data = json!({"requestDigest":digest,"amountMinor":amount.to_string(),"occurredAt":timestamp(occurred),"note":p.note,"createdAt":timestamp(at)});
        tx.execute("INSERT INTO local_manual_consumption(id,instance_id,source_id,event_time,amount_minor,kind,target_entry_id,data) VALUES(?1,?2,?3,?4,?5,'consume',NULL,?6)",
            params![p.idempotency_key,value.id,p.source_id,timestamp(occurred),amount as i64,data.to_string()]).map_err(err)?;
        advance_domain(&tx)?;
        put_setting(&tx, LAST_CLOCK, &json!(timestamp(at)))?;
        #[cfg(test)]
        if get_setting(&tx, "testLocalScheduleFailBeforeCommit")?.and_then(|value| value.as_bool())
            == Some(true)
        {
            return Err("injected-local-schedule-before-commit".into());
        }
        tx.commit().map_err(err)?;
        self.read_local_schedules(&p.source_id)
    }
    pub(crate) fn void_local_consumption(&mut self, payload: &Value) -> Result<Value> {
        let p: VoidInput = input(payload)?;
        self.require_local_scope(&p.source_id, &p.expected_scope)?;
        if !uuid_key(&p.idempotency_key) {
            return Err("撤销幂等键无效".into());
        }
        let at = budget_clock::now().utc;
        if clock_rollback(&self.db, at)? {
            return Err("系统时钟回退，暂不撤销记录".into());
        }
        let tx = self.db.unchecked_transaction().map_err(err)?;
        let digest = hash(
            json!(["void", p.source_id, p.entry_id, p.expected_revision])
                .to_string()
                .as_bytes(),
        );
        if request_replay(&tx, &p.idempotency_key, &digest)? {
            drop(tx);
            return self.read_local_schedules(&p.source_id);
        }
        let row:Option<(String,String)>=tx.query_row("SELECT instance_id,source_id FROM local_manual_consumption WHERE id=?1 AND kind='consume'",[&p.entry_id],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(err)?;
        let (instance_id, source) = row.ok_or("手动记录不存在")?;
        if source != p.source_id {
            return Err("不能撤销其他来源记录".into());
        }
        let voided:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM local_manual_consumption WHERE target_entry_id=?1 AND kind='void')",[&p.entry_id],|row|row.get(0)).map_err(err)?;
        if p.expected_revision != 1 || voided {
            return Err("记录已被修改或撤销，请重新读取".into());
        }
        let data = json!({"requestDigest":digest,"createdAt":timestamp(at)});
        tx.execute("INSERT INTO local_manual_consumption(id,instance_id,source_id,event_time,amount_minor,kind,target_entry_id,data) VALUES(?1,?2,?3,?4,0,'void',?5,?6)",
            params![p.idempotency_key,instance_id,p.source_id,timestamp(at),p.entry_id,data.to_string()]).map_err(err)?;
        advance_domain(&tx)?;
        #[cfg(test)]
        if get_setting(&tx, "testLocalScheduleFailBeforeCommit")?.and_then(|value| value.as_bool())
            == Some(true)
        {
            return Err("injected-local-schedule-before-commit".into());
        }
        tx.commit().map_err(err)?;
        self.read_local_schedules(&p.source_id)
    }
    pub(crate) fn advance_local_schedules(&mut self) -> Result<()> {
        // 隐私期间不续建；恢复时仍只实例化当前周期，不补建历史或回放提醒。
        if self.settings()?["privacyMode"] == true {
            return Ok(());
        }
        let reading = budget_clock::now();
        if clock_rollback(&self.db, reading.utc)? {
            return Ok(());
        }
        let values = latest_templates(&self.db)?;
        if !values.iter().any(|value| value.state == "active") {
            return Ok(());
        }
        let tx = self.db.unchecked_transaction().map_err(err)?;
        let mut changed = false;
        let started = std::time::Instant::now();
        for value in values
            .into_iter()
            .filter(|value| value.state == "active")
            .take(MAX_TEMPLATES)
        {
            if started.elapsed() > std::time::Duration::from_millis(200) {
                break;
            }
            changed |= create_due(self, &tx, &value, reading)?;
        }
        if changed {
            advance_domain(&tx)?;
        }
        if last_clock(&tx)?.is_none_or(|last| (reading.utc - last).num_seconds() >= 1) {
            put_setting(&tx, LAST_CLOCK, &json!(timestamp(reading.utc)))?;
        }
        tx.commit().map_err(err)
    }
}

#[cfg(test)]
#[path = "local_schedules_tests.rs"]
mod tests;
