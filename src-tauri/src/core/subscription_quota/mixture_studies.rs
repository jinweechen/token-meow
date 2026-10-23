//! 手动多模型/滑动窗口条件实验。所有输出保持条件口径，不写官方额度或校准证明。
use super::super::{mixture_fit, rolling_scenario};
use super::*;
use chrono::Datelike;
use serde::de::DeserializeOwned;

const KEY: &str = "mixtureStudiesV1";
const MAX_STUDIES: usize = 32;
const MAX_ROWS: usize = 128;
const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_POINTS: f64 = 1_000_000_000.0;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Condition {
    account_label: String,
    window_label: String,
    effort: String,
    speed: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Sample {
    id: String,
    from_at: String,
    to_at: String,
    delta_points: f64,
    tokens: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Charge {
    Points {
        id: String,
        #[serde(rename = "occurredAt")]
        occurred_at: String,
        #[serde(rename = "amountPoints")]
        amount_points: f64,
    },
    Models {
        id: String,
        #[serde(rename = "occurredAt")]
        occurred_at: String,
        tokens: Vec<String>,
    },
}
impl Charge {
    fn id(&self) -> &str {
        match self {
            Self::Points { id, .. } | Self::Models { id, .. } => id,
        }
    }
    fn at(&self) -> &str {
        match self {
            Self::Points { occurred_at, .. } | Self::Models { occurred_at, .. } => occurred_at,
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Rolling {
    as_of: String,
    history_complete_from: Option<String>,
    capacity_points: f64,
    required_points: f64,
    window_minutes: i64,
    charges: Vec<Charge>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Config {
    title: String,
    condition: Condition,
    models: Vec<String>,
    samples: Vec<Sample>,
    rolling: Rolling,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Save {
    source_id: String,
    expected_scope: String,
    id: Option<String>,
    expected_revision: Option<i64>,
    config: Config,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Delete {
    source_id: String,
    expected_scope: String,
    id: String,
    expected_revision: i64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Study {
    id: String,
    revision: i64,
    source_id: String,
    configuration_revision: String,
    binding_key: String,
    created_at: String,
    updated_at: String,
    config: Config,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Book {
    schema_version: u32,
    revision: i64,
    studies: Vec<Study>,
}
impl Default for Book {
    fn default() -> Self {
        Self {
            schema_version: 1,
            revision: 0,
            studies: Vec::new(),
        }
    }
}
struct Binding {
    id: String,
    kind: String,
    enabled: bool,
    configuration_revision: i64,
    key: String,
}
impl Binding {
    fn view(&self) -> Value {
        json!({"id":self.id,"clientKind":self.kind,"enabled":self.enabled,"configurationRevision":self.configuration_revision.to_string()})
    }
    fn state(&self, study: &Study) -> &'static str {
        if !self.enabled {
            "source_unavailable"
        } else if self.key != study.binding_key {
            "pending_rebind"
        } else {
            "ready"
        }
    }
}

fn deserialize<T: DeserializeOwned>(value: &Value) -> Result<T> {
    if value.to_string().len() > MAX_BYTES {
        return Err("条件实验输入超过有界大小".into());
    }
    serde_json::from_value(value.clone()).map_err(|_| "条件实验字段、类型或版本无效".into())
}
fn text(value: &str, max: usize) -> bool {
    text_ok(value,max)&&value.encode_utf16().count()<=max&&!value.chars().any(|c|matches!(c,'\u{061c}'|'\u{200e}'|'\u{200f}'|'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}'))
}
fn uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| id.hyphenated().to_string() == value)
}
fn instant(value: &str) -> Result<chrono::DateTime<Utc>> {
    let at = time(value)?;
    if !(1..=9999).contains(&at.year()) {
        return Err("实验时间超出支持年份".into());
    }
    Ok(at)
}
fn tokens(values: &[String], models: usize) -> Result<Vec<u64>> {
    if values.len() != models {
        return Err("Token向量必须逐模型显式提供，不能用缺失值代替零".into());
    }
    let values = values
        .iter()
        .map(|value| {
            value
                .parse::<u64>()
                .ok()
                .filter(|n| *n <= MAX_SAFE_INTEGER as u64 && n.to_string() == *value)
                .ok_or("Token必须是安全范围内的规范非负整数字符串".to_owned())
        })
        .collect::<Result<Vec<_>>>()?;
    if !values.iter().any(|n| *n > 0) {
        return Err("Token向量至少一个模型大于零".into());
    }
    Ok(values)
}
fn binding(db: &Connection, source: &str) -> Result<Binding> {
    let row:Option<(String,String,String,bool,String,i64)>=db.query_row("SELECT id,root,client_kind,enabled,environment,configuration_revision FROM source_instances WHERE id=?1",[source],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?))).optional().map_err(err)?;
    let (id, root, kind, enabled, environment, revision) = row.ok_or("实验来源不存在")?;
    if !(0..=MAX_SAFE_INTEGER).contains(&revision) {
        return Err("实验来源配置修订无效".into());
    }
    let key = hash(
        json!([id, root, kind, enabled, environment, revision.to_string()])
            .to_string()
            .as_bytes(),
    );
    Ok(Binding {
        id,
        kind,
        enabled,
        configuration_revision: revision,
        key,
    })
}
fn window(
    config: &Config,
) -> Result<(
    chrono::DateTime<Utc>,
    chrono::DateTime<Utc>,
    chrono::DateTime<Utc>,
)> {
    let at = instant(&config.rolling.as_of)?;
    let delta = Duration::try_minutes(config.rolling.window_minutes).ok_or("窗口时长超出范围")?;
    let latest = at.checked_add_signed(delta).ok_or("未来计划时间超出范围")?;
    let end = latest.checked_add_signed(delta).ok_or("模拟终点超出范围")?;
    if end.year() > 9999 {
        return Err("模拟终点超出支持年份".into());
    }
    Ok((at, latest, end))
}
fn validate(config: &mut Config) -> Result<()> {
    if !text(&config.title, 80)
        || ![
            &config.condition.account_label,
            &config.condition.window_label,
            &config.condition.effort,
            &config.condition.speed,
        ]
        .iter()
        .all(|value| text(value, 80))
    {
        return Err("实验名称、账户标签、窗口标签、努力程度与速度条件均须明确填写".into());
    }
    if !(1..=3).contains(&config.models.len())
        || config.samples.len() > MAX_ROWS
        || config.rolling.charges.len() > MAX_ROWS
    {
        return Err("最多3模型、128样本与128笔计划扣额".into());
    }
    let mut names = BTreeSet::new();
    for name in &config.models {
        if !text(name, 512) || !names.insert(name) {
            return Err("模型名称无效或重复".into());
        }
    }
    let rolling = &config.rolling;
    if !(1..=10080).contains(&rolling.window_minutes)
        || !rolling.capacity_points.is_finite()
        || !(0.0..=MAX_POINTS).contains(&rolling.capacity_points)
        || rolling.capacity_points == 0.0
        || !rolling.required_points.is_finite()
        || rolling.required_points <= 0.0
        || rolling.required_points > rolling.capacity_points
    {
        return Err("手动窗口容量、所需扣额或时长无效".into());
    }
    let (at, latest, _) = window(config)?;
    config.rolling.as_of = timestamp(at);
    if let Some(start) = config.rolling.history_complete_from.as_mut() {
        let start_at = instant(start)?;
        if start_at > at {
            return Err("历史完整起点不能晚于模拟时刻".into());
        }
        *start = timestamp(start_at);
    }
    let mut ids = BTreeSet::new();
    for sample in &mut config.samples {
        if !uuid(&sample.id) || !ids.insert(sample.id.clone()) {
            return Err("样本与计划扣额ID必须是全局唯一的规范UUID".into());
        }
        let start = instant(&sample.from_at)?;
        let end = instant(&sample.to_at)?;
        if start >= end
            || end > at
            || !sample.delta_points.is_finite()
            || !(0.0..=MAX_POINTS).contains(&sample.delta_points)
        {
            return Err("样本必须在模拟时刻以前且实际区间、手动扣额点有效".into());
        }
        tokens(&sample.tokens, config.models.len())?;
        sample.from_at = timestamp(start);
        sample.to_at = timestamp(end);
        // 统一毫秒后重验，不能把极短区间截成空区间。
        if sample.from_at >= sample.to_at {
            return Err("样本区间必须至少跨一个完整毫秒".into());
        }
    }
    config
        .samples
        .sort_by(|a, b| a.from_at.cmp(&b.from_at).then(a.id.cmp(&b.id)));
    if config
        .samples
        .windows(2)
        .any(|rows| rows[0].to_at > rows[1].from_at)
    {
        return Err("样本实际区间不能重叠，允许半开区间端点相接".into());
    }
    for charge in &mut config.rolling.charges {
        if !uuid(charge.id()) || !ids.insert(charge.id().to_owned()) {
            return Err("样本与计划扣额ID必须是全局唯一的规范UUID".into());
        }
        let when = instant(charge.at())?;
        if when > latest {
            return Err("未来计划扣额不能越过模拟时刻加一个窗口".into());
        }
        match charge {
            Charge::Points {
                occurred_at,
                amount_points,
                ..
            } => {
                if !amount_points.is_finite()
                    || *amount_points <= 0.0
                    || *amount_points > MAX_POINTS
                {
                    return Err("手动扣额点必须大于零且不超过上限".into());
                }
                *occurred_at = timestamp(when);
            }
            Charge::Models {
                occurred_at,
                tokens: values,
                ..
            } => {
                tokens(values, config.models.len())?;
                *occurred_at = timestamp(when);
            }
        }
    }
    config
        .rolling
        .charges
        .sort_by(|a, b| a.at().cmp(b.at()).then(a.id().cmp(b.id())));
    Ok(())
}
fn load(db: &Connection) -> Result<Book> {
    let length: Option<i64> = db
        .query_row(
            "SELECT length(CAST(value AS BLOB)) FROM settings WHERE key=?1",
            [KEY],
            |row| row.get(0),
        )
        .optional()
        .map_err(err)?;
    let Some(length) = length else {
        return Ok(Book::default());
    };
    if !(0..=MAX_BYTES as i64).contains(&length) {
        return Err("条件实验存储超过有界大小".into());
    }
    let raw: String = db
        .query_row("SELECT value FROM settings WHERE key=?1", [KEY], |row| {
            row.get(0)
        })
        .map_err(err)?;
    let book: Book = serde_json::from_str(&raw).map_err(|_| "条件实验存储无效")?;
    if book.schema_version != 1
        || !(0..=MAX_SAFE_INTEGER).contains(&book.revision)
        || book.studies.len() > MAX_STUDIES
    {
        return Err("条件实验存储版本或上限无效".into());
    }
    let mut ids = BTreeSet::new();
    for study in &book.studies {
        if !uuid(&study.id)
            || !ids.insert(&study.id)
            || !(1..=MAX_SAFE_INTEGER).contains(&study.revision)
            || !text(&study.source_id, 512)
            || study
                .configuration_revision
                .parse::<i64>()
                .ok()
                .filter(|n| (0..=MAX_SAFE_INTEGER).contains(n))
                .map(|n| n.to_string())
                .as_deref()
                != Some(study.configuration_revision.as_str())
            || study.binding_key.len() != 64
            || !study.binding_key.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("条件实验记录修订或绑定无效".into());
        }
        instant(&study.created_at)?;
        instant(&study.updated_at)?;
        validate(&mut study.config.clone())?;
    }
    Ok(book)
}
fn persist(db: &Connection, book: &Book) -> Result<()> {
    let raw = serde_json::to_string(book).map_err(err)?;
    if raw.len() > MAX_BYTES {
        return Err("条件实验存储超过2MiB上限".into());
    }
    db.execute("INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![KEY,raw]).map_err(err)?;
    Ok(())
}
fn manual(mut value: Value) -> Value {
    value["quality"] = json!("conditional-manual");
    value["constraintsVerified"] = json!(false);
    value["notifyEligible"] = json!(false);
    value["uncertaintyBounds"] = Value::Null;
    value
}
fn blocked(config: &Config, reasons: &[String]) -> Result<Value> {
    let (at, _, end) = window(config)?;
    let fit = manual(
        json!({"kind":"unavailable","weights":null,"fixedMix":null,"rank":0,"conditionNumber":null,"trainSamples":0,"holdoutSamples":0,"trainWape":null,"holdoutWape":null,"reasons":reasons}),
    );
    let rolling = manual(
        json!({"asOf":timestamp(at),"endsAt":timestamp(end),"points":[
            {"at":timestamp(at),"usedPoints":null,"remainingPoints":null,"available":null},
            {"at":timestamp(end),"usedPoints":null,"remainingPoints":null,"available":null}],
            "availabilityWindows":[],"nextAvailableAt":null,"reasons":reasons}),
    );
    Ok(manual(
        json!({"fit":fit,"rolling":rolling,"reasons":reasons}),
    ))
}
fn projected(config: &Config) -> Result<Value> {
    let n = config.samples.len();
    let held = (n.div_ceil(4)).max(2);
    let train = n.saturating_sub(held);
    let samples = config
        .samples
        .iter()
        .enumerate()
        .map(|(i, s)| {
            Ok(mixture_fit::MixtureSample {
                id: s.id.clone(),
                from_ms: instant(&s.from_at)?.timestamp_millis(),
                to_ms: instant(&s.to_at)?.timestamp_millis(),
                charge_points: s.delta_points,
                tokens: tokens(&s.tokens, config.models.len())?,
                holdout: i >= train,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let fit = mixture_fit::fit(&config.models, &samples);
    let fit_json = serde_json::to_value(&fit).map_err(|_| "拟合结果包含无效数值")?;
    let mut unpriced = BTreeSet::<String>::new();
    let charges = config
        .rolling
        .charges
        .iter()
        .map(|charge| {
            let points = match charge {
                Charge::Points { amount_points, .. } => Some(*amount_points),
                Charge::Models { tokens: values, .. } => {
                    let values = tokens(values, config.models.len())?;
                    match mixture_fit::predict_tokens(&fit, &values) {
                        Ok(points) => Some(points),
                        Err(reason) => {
                            unpriced.insert(reason.into());
                            None
                        }
                    }
                }
            };
            Ok(rolling_scenario::RollingCharge {
                id: charge.id().to_owned(),
                occurred_at_ms: instant(charge.at())?.timestamp_millis(),
                points,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let (at, _, end) = window(config)?;
    let result = rolling_scenario::simulate(&rolling_scenario::RollingInput {
        as_of_ms: at.timestamp_millis(),
        history_complete_from_ms: config
            .rolling
            .history_complete_from
            .as_deref()
            .map(instant)
            .transpose()?
            .map(|v| v.timestamp_millis()),
        window_ms: config.rolling.window_minutes * 60_000,
        horizon_ms: end.timestamp_millis(),
        capacity_points: config.rolling.capacity_points,
        required_points: config.rolling.required_points,
        charges,
    })
    .map_err(|_| "滑动窗口条件实验参数或数值无效")?;
    let raw = serde_json::to_value(&result).map_err(|_| "滑动窗口结果包含无效数值")?;
    let stamp = |ms: &Value| -> Result<String> {
        let ms = ms.as_i64().ok_or("模拟时间数值无效")?;
        chrono::DateTime::<Utc>::from_timestamp_millis(ms)
            .map(timestamp)
            .ok_or("模拟时间越界".into())
    };
    let rows = raw["points"].as_array().ok_or("模拟曲线缺失")?;
    if rows.len() > 260 {
        return Err("模拟曲线超过260点上限".into());
    }
    let points=rows.iter().map(|p|Ok(json!({"at":stamp(&p["atMs"])?,"usedPoints":p["usedPoints"],"remainingPoints":p["remainingPoints"],"available":p["available"]}))).collect::<Result<Vec<_>>>()?;
    let windows = raw["availabilityWindows"]
        .as_array()
        .ok_or("模拟可用区间缺失")?
        .iter()
        .map(|p| Ok(json!({"fromAt":stamp(&p["fromMs"])?,"toAt":stamp(&p["toMs"])?})))
        .collect::<Result<Vec<_>>>()?;
    let next = if raw["nextAvailableMs"].is_null() {
        None
    } else {
        Some(stamp(&raw["nextAvailableMs"])?)
    };
    let mut reasons = raw["reasons"]
        .as_array()
        .ok_or("模拟理由缺失")?
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    reasons.extend(unpriced);
    reasons.sort();
    reasons.dedup();
    let rolling = manual(
        json!({"asOf":timestamp(at),"endsAt":timestamp(end),"points":points,"availabilityWindows":windows,"nextAvailableAt":next,"reasons":reasons}),
    );
    Ok(manual(
        json!({"fit":manual(fit_json),"rolling":rolling,"reasons":[]}),
    ))
}

impl Core {
    pub(crate) fn mixture_study_scope_token(&self, source: &str) -> Result<String> {
        Ok(hash(
            format!(
                "mixture-studies-v1|{}",
                self.local_schedule_scope_token(source)?
            )
            .as_bytes(),
        ))
    }
    fn require_mixture_scope(&self, source: &str, expected: &str) -> Result<Binding> {
        if self.settings()?["privacyMode"] == true {
            return Err("隐私模式下不能修改条件实验".into());
        }
        if self.mixture_study_scope_token(source)? != expected {
            return Err("实验范围已变化，请重新读取".into());
        }
        let value = binding(&self.db, source)?;
        if !value.enabled {
            return Err("实验来源已停用".into());
        }
        Ok(value)
    }
    pub(crate) fn read_mixture_studies(&self, source: &str) -> Result<Value> {
        let current = binding(&self.db, source)?;
        let at = budget_clock::now().utc;
        let private = self.settings()?["privacyMode"] == true;
        let book = load(&self.db)?;
        let mut studies = Vec::new();
        let mut reasons = Vec::<String>::new();
        if private {
            reasons.push("privacy_mode".into());
        } else {
            for value in book
                .studies
                .iter()
                .filter(|value| value.source_id == source)
            {
                let state = current.state(value);
                let mut why = Vec::new();
                if state != "ready" {
                    why.push(
                        if state == "pending_rebind" {
                            "source_binding_changed"
                        } else {
                            "source_unavailable"
                        }
                        .into(),
                    );
                }
                if at < instant(&value.updated_at)? {
                    why.push("clock_rollback".into());
                }
                let projection = if why.is_empty() {
                    projected(&value.config)?
                } else {
                    blocked(&value.config, &why)?
                };
                studies.push(json!({"id":value.id,"revision":value.revision,"sourceId":value.source_id,"configurationRevision":value.configuration_revision,
                    "bindingState":state,"createdAt":value.created_at,"updatedAt":value.updated_at,"config":value.config,"projection":projection}));
            }
        }
        if !current.enabled {
            reasons.push("source_unavailable".into());
        }
        Ok(
            json!({"schemaVersion":1,"revision":book.revision,"scopeToken":self.mixture_study_scope_token(source)?,"asOf":timestamp(at),"source":current.view(),"studies":studies,"reasons":reasons}),
        )
    }
    pub(crate) fn save_mixture_study(&mut self, payload: &Value) -> Result<Value> {
        let mut p: Save = deserialize(payload)?;
        validate(&mut p.config)?;
        let current = self.require_mixture_scope(&p.source_id, &p.expected_scope)?;
        let at = budget_clock::now().utc;
        let tx = self.db.unchecked_transaction().map_err(err)?;
        let mut book = load(&tx)?;
        let value = if let Some(id) = p.id.as_deref() {
            let old = book
                .studies
                .iter()
                .find(|s| s.id == id)
                .ok_or("条件实验不存在或已删除")?;
            if old.source_id != p.source_id || Some(old.revision) != p.expected_revision {
                return Err("条件实验已被修改或来源不同，请重新读取".into());
            }
            if at < instant(&old.updated_at)? {
                return Err("系统时钟回退，暂不修改实验".into());
            }
            Study {
                id: old.id.clone(),
                revision: next_revision(old.revision)?,
                source_id: old.source_id.clone(),
                configuration_revision: current.configuration_revision.to_string(),
                binding_key: current.key.clone(),
                created_at: old.created_at.clone(),
                updated_at: timestamp(at),
                config: p.config,
            }
        } else {
            if p.expected_revision.is_some() || book.studies.len() >= MAX_STUDIES {
                return Err("新实验不能引用旧修订，且最多保存32份".into());
            }
            Study {
                id: uuid::Uuid::new_v4().to_string(),
                revision: 1,
                source_id: p.source_id.clone(),
                configuration_revision: current.configuration_revision.to_string(),
                binding_key: current.key.clone(),
                created_at: timestamp(at),
                updated_at: timestamp(at),
                config: p.config,
            }
        };
        // 先完成纯数值计算，无法序列化的结果不得留下半份配置。
        projected(&value.config)?;
        book.studies.retain(|s| s.id != value.id);
        book.studies.push(value);
        book.studies.sort_by(|a, b| a.id.cmp(&b.id));
        book.revision = next_revision(book.revision)?;
        persist(&tx, &book)?;
        #[cfg(test)]
        if get_setting(&tx, "testMixtureStudyFailBeforeCommit")?.and_then(|v| v.as_bool())
            == Some(true)
        {
            return Err("injected-mixture-study-before-commit".into());
        }
        tx.commit().map_err(err)?;
        self.read_mixture_studies(&p.source_id)
    }
    pub(crate) fn delete_mixture_study(&mut self, payload: &Value) -> Result<Value> {
        let p: Delete = deserialize(payload)?;
        self.require_mixture_scope(&p.source_id, &p.expected_scope)?;
        let tx = self.db.unchecked_transaction().map_err(err)?;
        let mut book = load(&tx)?;
        let old = book
            .studies
            .iter()
            .find(|s| s.id == p.id)
            .ok_or("条件实验不存在或已删除")?;
        if old.source_id != p.source_id || old.revision != p.expected_revision {
            return Err("条件实验已被修改或来源不同，请重新读取".into());
        }
        book.studies.retain(|s| s.id != p.id);
        book.revision = next_revision(book.revision)?;
        persist(&tx, &book)?;
        #[cfg(test)]
        if get_setting(&tx, "testMixtureStudyFailBeforeCommit")?.and_then(|v| v.as_bool())
            == Some(true)
        {
            return Err("injected-mixture-study-before-commit".into());
        }
        tx.commit().map_err(err)?;
        self.read_mixture_studies(&p.source_id)
    }
}

#[cfg(test)]
#[path = "mixture_studies_tests.rs"]
mod tests;
