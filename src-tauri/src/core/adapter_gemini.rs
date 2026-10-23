//! Gemini CLI 本地计量：只处理登记目录中的 chats，正文与工具参数不进入事实或游标。
//!
//! 字段合同来自 Gemini CLI a713410aba5d594594e20d5c822e3aab8f822089 的
//! chatRecordingService.ts / chatRecordingTypes.ts，及 Google GenAI JS 1.30.0
//! (38cac5bbf4941ec5fa760238bd423c0ecc2c6f04) GenerateContentResponseUsageMetadata。
//! 官方来源及固定字节收据见 output/research/extensions-20261023/gemini-usage-semantics.md。
//! ccusage 7dc5c7684f5372e0275b2401e53d8c16adefbc43 的目录/白名单思路可复用；
//! 不采用其浮点转换、缺失补零、文件时间回退或最大/最后计量去重规则。

use super::*;
use serde::de::{IgnoredAny, MapAccess, SeqAccess, Visitor};
use std::io::{BufReader, Read};
use std::sync::atomic::{AtomicBool, Ordering};

const NAMESPACE: &str = "gemini-cli-session-message-v1";
const SEMANTICS_VERSION: &str = "gemini-cli-prompt-tool-candidate-thought-v1";
const CONTEXT_KEY: &str = "gemini.metadata.v1";
const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;
const MAX_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;
const MAX_MESSAGES: usize = 8192;
const MAX_TEXT_BYTES: usize = 256;
const MAX_EXACT_INTEGER: i64 = 9_007_199_254_740_991;
const MAX_DISCOVERY_ENTRIES: usize = 100_000;
const MAX_DISCOVERY_DEPTH: usize = 32;

pub(super) struct GeminiAdapter;

// derive 忽略未知字段时使用 IgnoredAny；不使用 flatten，避免未知正文先被缓存为 Value。
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SafeRecord {
    session_id: Option<String>,
    project_hash: Option<String>,
    kind: Option<String>,
    #[serde(rename = "type")]
    message_type: Option<String>,
    id: Option<String>,
    timestamp: Option<String>,
    model: Option<String>,
    tokens: Option<SafeTokens>,
    messages: Option<SafeMessages>,
    #[serde(rename = "$set")]
    set: Option<SafeSet>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SafeSet {
    session_id: Option<String>,
    project_hash: Option<String>,
    kind: Option<String>,
    messages: Option<SafeMessages>,
}

#[derive(Clone, Default, Deserialize)]
struct SafeMessage {
    #[serde(rename = "type")]
    message_type: Option<String>,
    id: Option<String>,
    timestamp: Option<String>,
    model: Option<String>,
    tokens: Option<SafeTokens>,
}

#[derive(Default)]
struct SafeMessages(Vec<SafeMessage>);
impl<'de> Deserialize<'de> for SafeMessages {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct MessagesVisitor;
        impl<'de> Visitor<'de> for MessagesVisitor {
            type Value = SafeMessages;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("有界消息数组")
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut messages = Vec::new();
                while let Some(message) = seq.next_element::<SafeMessage>()? {
                    if messages.len() >= MAX_MESSAGES {
                        return Err(serde::de::Error::custom("消息数量超过上限"));
                    }
                    messages.push(message);
                }
                Ok(SafeMessages(messages))
            }
        }
        deserializer.deserialize_seq(MessagesVisitor)
    }
}

#[derive(Clone, Default, Deserialize)]
struct SafeTokens {
    #[serde(default)]
    input: TokenMetric,
    #[serde(default)]
    output: TokenMetric,
    #[serde(default)]
    cached: TokenMetric,
    #[serde(default)]
    thoughts: TokenMetric,
    #[serde(default)]
    tool: TokenMetric,
    #[serde(default)]
    total: TokenMetric,
}

#[derive(Clone)]
struct TokenMetric(Metric);
impl Default for TokenMetric {
    fn default() -> Self {
        Self(Metric::Missing)
    }
}
impl<'de> Deserialize<'de> for TokenMetric {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct CounterVisitor;
        impl<'de> Visitor<'de> for CounterVisitor {
            type Value = TokenMetric;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("来源报告的非负精确整数")
            }
            fn visit_i64<E: serde::de::Error>(
                self,
                value: i64,
            ) -> std::result::Result<Self::Value, E> {
                Ok(TokenMetric(if (0..=MAX_EXACT_INTEGER).contains(&value) {
                    Metric::Valid(value)
                } else {
                    Metric::Invalid
                }))
            }
            fn visit_u64<E: serde::de::Error>(
                self,
                value: u64,
            ) -> std::result::Result<Self::Value, E> {
                Ok(TokenMetric(if value <= MAX_EXACT_INTEGER as u64 {
                    Metric::Valid(value as i64)
                } else {
                    Metric::Invalid
                }))
            }
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> std::result::Result<Self::Value, E> {
                Ok(TokenMetric(Metric::Invalid))
            }
            fn visit_bool<E: serde::de::Error>(
                self,
                _: bool,
            ) -> std::result::Result<Self::Value, E> {
                Ok(TokenMetric(Metric::Invalid))
            }
            fn visit_str<E: serde::de::Error>(
                self,
                _: &str,
            ) -> std::result::Result<Self::Value, E> {
                Ok(TokenMetric(Metric::Invalid))
            }
            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(TokenMetric(Metric::Unknown))
            }
            fn visit_none<E: serde::de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(TokenMetric(Metric::Unknown))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(TokenMetric(Metric::Invalid))
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(TokenMetric(Metric::Invalid))
            }
        }
        deserializer.deserialize_any(CounterVisitor)
    }
}

impl SafeTokens {
    fn native(&self) -> BTreeMap<String, Metric> {
        [
            ("input", &self.input),
            ("output", &self.output),
            ("cached", &self.cached),
            ("thoughts", &self.thoughts),
            ("tool", &self.tool),
            ("total", &self.total),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.0.clone()))
        .collect()
    }
    fn safe_value(&self) -> Value {
        let mut values = serde_json::Map::new();
        for (key, metric) in self.native() {
            let value = match metric {
                Metric::Valid(value) => json!(value),
                Metric::Unknown => Value::Null,
                Metric::Missing => continue,
                // 不保留错误字段的原始字符串、数组或对象；标记仍会解为 Invalid。
                Metric::Invalid => json!({"invalidTokenMetric":true}),
            };
            values.insert(key, value);
        }
        Value::Object(values)
    }
    fn counts(&self) -> Counts {
        let input = sum_metrics(&[&self.input.0, &self.tool.0]);
        let output = sum_metrics(&[&self.output.0, &self.thoughts.0]);
        let computed = sum_metrics(&[&input, &output]);
        let total = match (&computed, &self.total.0) {
            (Metric::Valid(expected), Metric::Valid(actual)) if expected == actual => {
                Metric::Valid(*actual)
            }
            (Metric::Valid(_), Metric::Valid(_)) | (_, Metric::Invalid) | (Metric::Invalid, _) => {
                Metric::Invalid
            }
            (_, Metric::Unknown) | (Metric::Unknown, _) => Metric::Unknown,
            _ => Metric::Missing,
        };
        let cache_read = match (self.cached.0.value(), self.input.0.value()) {
            (Some(cached), Some(prompt)) if cached > prompt => Metric::Invalid,
            (Some(_), None) => Metric::Unknown,
            _ => self.cached.0.clone(),
        };
        Counts {
            input,
            output,
            cache_read,
            cache_write: Metric::Missing,
            reasoning: self.thoughts.0.clone(),
            total,
        }
    }
}

fn sum_metrics(values: &[&Metric]) -> Metric {
    if values.iter().any(|value| matches!(value, Metric::Invalid)) {
        return Metric::Invalid;
    }
    if values.iter().any(|value| matches!(value, Metric::Unknown)) {
        return Metric::Unknown;
    }
    if values.iter().any(|value| matches!(value, Metric::Missing)) {
        return Metric::Missing;
    }
    values
        .iter()
        .try_fold(0_i64, |sum, value| sum.checked_add(value.value()?))
        .filter(|sum| *sum <= MAX_EXACT_INTEGER)
        .map(Metric::Valid)
        .unwrap_or(Metric::Invalid)
}

fn safe_text(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_TEXT_BYTES
        && !value.chars().any(|c| c.is_control() || matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
}
fn safe_id(value: &str) -> bool {
    safe_text(value) && !value.contains('|')
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn validate_metadata(
    session: Option<&str>,
    project: Option<&str>,
    kind: Option<&str>,
) -> Result<()> {
    if session.is_some_and(|value| !safe_id(value))
        || project.is_some_and(|value| !valid_hash(value))
        || kind.is_some_and(|value| !matches!(value, "main" | "subagent"))
    {
        return Err("Gemini 会话元数据不是支持的原生身份格式".into());
    }
    Ok(())
}
fn validate_message(
    kind: Option<&str>,
    id: Option<&str>,
    time: Option<&str>,
    model: Option<&str>,
) -> Result<()> {
    if kind.is_some_and(|value| !safe_text(value))
        || id.is_some_and(|value| !safe_id(value))
        || time.is_some_and(|value| !safe_text(value))
        || model.is_some_and(|value| !safe_text(value))
    {
        return Err("Gemini 消息元数据超限或包含控制字符".into());
    }
    Ok(())
}
impl SafeMessage {
    fn validate(&self) -> Result<()> {
        validate_message(
            self.message_type.as_deref(),
            self.id.as_deref(),
            self.timestamp.as_deref(),
            self.model.as_deref(),
        )
    }
    fn safe_value(&self) -> Value {
        json!({"type":self.message_type,"id":self.id,"timestamp":self.timestamp,
            "model":self.model,"tokens":self.tokens.as_ref().map(SafeTokens::safe_value)})
    }
}
impl SafeRecord {
    fn validate(&self) -> Result<()> {
        validate_metadata(
            self.session_id.as_deref(),
            self.project_hash.as_deref(),
            self.kind.as_deref(),
        )?;
        validate_message(
            self.message_type.as_deref(),
            self.id.as_deref(),
            self.timestamp.as_deref(),
            self.model.as_deref(),
        )?;
        if let Some(set) = &self.set {
            validate_metadata(
                set.session_id.as_deref(),
                set.project_hash.as_deref(),
                set.kind.as_deref(),
            )?;
        }
        if self
            .messages
            .as_ref()
            .map_or(0, |messages| messages.0.len())
            + self
                .set
                .as_ref()
                .and_then(|set| set.messages.as_ref())
                .map_or(0, |messages| messages.0.len())
            > MAX_MESSAGES
        {
            return Err("Gemini 单记录消息数量超过上限".into());
        }
        for message in self
            .messages
            .iter()
            .chain(self.set.iter().filter_map(|set| set.messages.as_ref()))
            .flat_map(|messages| &messages.0)
        {
            message.validate()?;
        }
        Ok(())
    }
    fn safe_value(&self) -> Value {
        let set = self.set.as_ref().map(|set| json!({"sessionId":set.session_id,"projectHash":set.project_hash,
            "kind":set.kind,"messages":set.messages.as_ref().map(|messages| messages.0.iter().map(SafeMessage::safe_value).collect::<Vec<_>>())}));
        json!({"sessionId":self.session_id,"projectHash":self.project_hash,"kind":self.kind,
            "type":self.message_type,"id":self.id,"timestamp":self.timestamp,"model":self.model,
            "tokens":self.tokens.as_ref().map(SafeTokens::safe_value),
            "messages":self.messages.as_ref().map(|messages| messages.0.iter().map(SafeMessage::safe_value).collect::<Vec<_>>()),"$set":set})
    }
}
fn parse_safe(value: &Value) -> Result<SafeRecord> {
    let record: SafeRecord = serde_json::from_value(value.clone())
        .map_err(|_| "Gemini 元数据字段类型无效".to_owned())?;
    record.validate()?;
    Ok(record)
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Metadata {
    session_id: Option<String>,
    project_hash: Option<String>,
    kind: Option<String>,
    #[serde(default)]
    invalid_session: bool,
}
impl Metadata {
    fn merge(&mut self, session: Option<&str>, project: Option<&str>, kind: Option<&str>) {
        if let Some(session) = session {
            if self.session_id.as_deref().is_some_and(|old| old != session) {
                // 同一游标不能换会话重新标记旧消息；仅 reader 开新 generation 可清除此状态。
                self.invalid_session = true;
            } else {
                self.session_id = Some(session.to_owned());
            }
        }
        if let Some(project) = project {
            if self
                .project_hash
                .as_deref()
                .is_some_and(|old| old != project)
            {
                self.invalid_session = true;
            } else {
                self.project_hash = Some(project.to_owned());
            }
        }
        if let Some(kind) = kind {
            self.kind = Some(kind.to_owned());
        }
    }
    fn apply(&mut self, record: &SafeRecord) {
        self.merge(
            record.session_id.as_deref(),
            record.project_hash.as_deref(),
            record.kind.as_deref(),
        );
        if let Some(set) = &record.set {
            self.merge(
                set.session_id.as_deref(),
                set.project_hash.as_deref(),
                set.kind.as_deref(),
            );
        }
    }
    fn from_context(state: &Context) -> Self {
        state
            .legacy
            .get(CONTEXT_KEY)
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default()
    }
    fn save(&self, state: &mut Context) {
        state.legacy.insert(CONTEXT_KEY.into(), json!(self));
        if !self.invalid_session {
            if let Some(session) = &self.session_id {
                state.active_thread = session.clone();
            }
        }
    }
}

fn response(
    observation: &Observation,
    metadata: &Metadata,
    message: &SafeMessage,
) -> Result<Option<ResponseUsage>> {
    if message.message_type.as_deref() != Some("gemini") {
        return Ok(None);
    }
    // null -> 完整的首次追加不是 revision；待补计量不创建零用量事实。
    let Some(tokens) = &message.tokens else {
        return Ok(None);
    };
    if metadata.invalid_session {
        return Err("Gemini 文件内会话身份矛盾，已停止该游标的计量准入".into());
    }
    let session = metadata
        .session_id
        .as_deref()
        .filter(|id| safe_id(id))
        .ok_or("Gemini 计量缺少原生 sessionId")?;
    let id = message
        .id
        .as_deref()
        .filter(|id| safe_id(id))
        .ok_or("Gemini 计量缺少原生消息 id")?;
    let time = message
        .timestamp
        .as_deref()
        .and_then(normalize_time)
        .ok_or("Gemini 计量缺少有效的原生 RFC3339 消息时间")?;
    let counts = tokens.counts();
    let complete = counts.total.value().is_some();
    Ok(Some(ResponseUsage {
        source_id: observation.source_id.clone(),
        channel_id: observation.channel_id.clone(),
        namespace: NAMESPACE.into(),
        thread_id: session.into(),
        response_id: id.into(),
        turn_id: None,
        root_turn_id: None,
        event_time: time,
        collected_at: now(),
        requested_model: None,
        observed_model: message.model.clone(),
        provider_id: None,
        account_scope_id: None,
        native_counts: tokens.native(),
        counts,
        semantics_version: SEMANTICS_VERSION.into(),
        adapter_version: "gemini-local-json-jsonl-v1".into(),
        completeness: if complete {
            "source-recorded;upstreamdefaults-unproven;history-coverage-unproven"
        } else {
            "partial-source-recorded;upstreamdefaults-unproven;history-coverage-unproven"
        }
        .into(),
        phase: "source-recorded".into(),
        revision: None,
        live: false,
    }))
}

fn each_message<F>(record: &SafeRecord, mut visit: F) -> Result<()>
where
    F: FnMut(SafeMessage) -> Result<()>,
{
    visit(SafeMessage {
        message_type: record.message_type.clone(),
        id: record.id.clone(),
        timestamp: record.timestamp.clone(),
        model: record.model.clone(),
        tokens: record.tokens.clone(),
    })?;
    // 元数据已全部检查，checkpoint 只是追加观测，不是消费账本的替换集合。
    for message in record
        .messages
        .iter()
        .chain(record.set.iter().filter_map(|set| set.messages.as_ref()))
        .flat_map(|messages| &messages.0)
    {
        visit(message.clone())?;
    }
    Ok(())
}

fn store_metadata(db: &Connection, response: &ResponseUsage, metadata: &Metadata) -> Result<()> {
    let value = json!({"id":response.thread_id,"sourceId":response.source_id,
        "title":format!("Gemini 会话 {}",response.thread_id.chars().take(8).collect::<String>()),
        "cwd":"","parentThreadId":null,"rootThreadId":null,"model":response.observed_model,
        "accountScopeId":null,"gemini":{"sessionId":metadata.session_id,"projectHash":metadata.project_hash,"kind":metadata.kind}});
    db.execute(
        "INSERT OR IGNORE INTO thread_metadata(id,data) VALUES(?1,?2)",
        params![
            format!("{}|{}", response.source_id, response.thread_id),
            value.to_string()
        ],
    )
    .map_err(err)?;
    Ok(())
}

fn store_checked(db: &Connection, response: &ResponseUsage) -> Result<usize> {
    let key = response_key(response);
    if let Some(old) = load_fact(db, "usage_responses", &key)? {
        let old: ResponseUsage = serde_json::from_value(old).map_err(err)?;
        // core 的计量 digest 不含模型/时间。补齐未知模型可以，已知值矛盾必须隔离。
        if old.event_time != response.event_time
            || old
                .observed_model
                .as_ref()
                .zip(response.observed_model.as_ref())
                .is_some_and(|(old, new)| old != new)
        {
            let incoming = hash(
                json!({"time":response.event_time,"model":response.observed_model})
                    .to_string()
                    .as_bytes(),
            );
            let conflict = hash(format!("gemini-native-metadata|{key}|{incoming}").as_bytes());
            db.execute("INSERT OR IGNORE INTO identity_conflicts(id,kind,event_time,details) VALUES(?1,'same_identity_different_native_metadata',?2,?3)",
                params![conflict,now(),json!({"factId":key,"sourceId":response.source_id,"incomingDigest":incoming}).to_string()]).map_err(err)?;
            db.execute("INSERT OR IGNORE INTO excluded_facts(fact_id,reason) VALUES(?1,'unresolved-native-metadata-conflict')",[&key]).map_err(err)?;
            return Ok(0);
        }
    }
    store_response(db, response, false)
}

impl SourceAdapter for GeminiAdapter {
    fn client_kind(&self) -> &'static str {
        "gemini"
    }
    fn parse_line(&self, bytes: &[u8]) -> Result<Value> {
        if bytes.len() > MAX_LINE_BYTES {
            return Err("Gemini JSONL 完整行超过 4 MiB 上限".into());
        }
        let record: SafeRecord = serde_json::from_slice(bytes)
            .map_err(|_| "Gemini JSONL 不是支持的完整元数据记录".to_owned())?;
        record.validate()?;
        Ok(record.safe_value())
    }
    fn is_json_snapshot(&self, path: &Path) -> bool {
        path.extension().and_then(|value| value.to_str()) == Some("json")
    }
    fn snapshot_records(&self, reader: &mut dyn Read) -> Result<Vec<Value>> {
        // caller 另负责取消/文件句柄校验/事务；此处也限制物化预算，且永不复制未知正文字段。
        // 直接从受控 reader 解码，避免先收集原文字节后在脱离取消/时间预算的阶段解析。
        let mut limited = reader.take((MAX_SNAPSHOT_BYTES + 1) as u64);
        let parsed = {
            let buffered = BufReader::with_capacity(64 * 1024, &mut limited);
            let mut decoder = serde_json::Deserializer::from_reader(buffered);
            SafeRecord::deserialize(&mut decoder).and_then(|record| decoder.end().map(|_| record))
        };
        if limited.limit() == 0 {
            return Err("Gemini JSON 快照超过 16 MiB 上限".into());
        }
        let record = parsed.map_err(|_| "Gemini JSON 快照不是支持的元数据格式".to_owned())?;
        record.validate()?;
        if record.session_id.is_none()
            || record.messages.is_none()
            || record.set.is_some()
            || record.message_type.is_some()
            || record.tokens.is_some()
        {
            return Err("Gemini JSON 快照缺少原生会话或消息集合".into());
        }
        let header = json!({"sessionId":record.session_id,"projectHash":record.project_hash,"kind":record.kind});
        let mut records = vec![header.clone()];
        for message in &record.messages.as_ref().expect("已检查消息集合").0 {
            if message.message_type.as_deref() != Some("gemini") {
                continue;
            }
            let mut value = message.safe_value();
            for key in ["sessionId", "projectHash", "kind"] {
                value[key] = header[key].clone();
            }
            records.push(value);
        }
        Ok(records)
    }
    fn identity_policy(&self) -> IdentityPolicy {
        IdentityPolicy {
            namespace: NAMESPACE.into(),
            scope: "source+native-sessionId+message.id".into(),
            revisions: false,
        }
    }
    fn capabilities(&self) -> Value {
        let field = |supported: bool, scope: &str| {
            json!({"support":if supported {"仅文档支持"} else {"不支持"},
            "runtime":null,"version":null,"supportedVersion":"仅固定 writer 字段合同；运行中 CLI 版本未探测","testedVersion":null,
            "channel":"local-json-jsonl","granularity":if supported {"response"} else {"none"},
            "prerequisites":["用户登记 Gemini 根目录或 tmp；仅 chats 内 JSON/JSONL","原生 sessionId、消息 id 和消息 RFC3339 时间有效"],
            "evidenceRefs":["https://github.com/google-gemini/gemini-cli/blob/a713410aba5d594594e20d5c822e3aab8f822089/packages/core/src/services/chatRecordingService.ts",
                "https://github.com/googleapis/js-genai/blob/38cac5bbf4941ec5fa760238bd423c0ecc2c6f04/src/types.ts"],
            "verifiedAt":null,"verificationScope":scope,
            "versionPolicy":"固定源码与合成合同样本；不是用户实际 CLI 样本资格验收。CLI 的 source 0 不能证明上游显式为零；历史覆盖未证明完整"})
        };
        json!({"usage":field(true,"仅来源原生计量：input=P+U，output=C+R，校验 T=P+U+C+R；不代表账单"),
            "cacheRead":field(true,"cached 属于 prompt 子集；不重复加到总量"),"cacheWrite":field(false,"无独立缓存写入计量"),
            "reasoning":field(true,"thoughts 已包含于规范化 output"),"turnCompletion":field(false,"消息或 stop reason 不证明任务完成"),
            "turnTiming":field(false,"没有真实任务生命周期"),"requestTiming":field(false,"没有请求起止时间"),
            "subagents":field(false,"kind 仅保留原生标记，没有父子身份与完成证据"),"context":field(false,"未接入上下文窗口"),
            "chatTarget":field(false,"无验证过的跳转协议"),"quota":field(false,"本地 token 不能转换成官方订阅限额"),
            "accountUsage":field(false,"不读取凭据、账户或官方额度")})
    }
    fn normalize(&self, observation: &Observation) -> Result<Vec<StandardEvent>> {
        let record = parse_safe(&observation.event)?;
        let mut metadata = Metadata::default();
        metadata.apply(&record);
        let mut events = Vec::new();
        each_message(&record, |message| {
            if let Some(response) = response(observation, &metadata, &message)? {
                events.push(StandardEvent::ResponseUsage(response));
            }
            Ok(())
        })?;
        Ok(events)
    }
    fn fact_reference_with_context(
        &self,
        observation: &Observation,
        state: &Context,
    ) -> Option<String> {
        let record = parse_safe(&observation.event).ok()?;
        // 一条 checkpoint 可能产生多项事实，单个 ingested_records.fact_id 不能冒充整个集合。
        if record.messages.is_some()
            || record
                .set
                .as_ref()
                .is_some_and(|set| set.messages.is_some())
        {
            return None;
        }
        let mut metadata = Metadata::from_context(state);
        metadata.apply(&record);
        let mut reference = None;
        each_message(&record, |message| {
            if let Some(row) = response(observation, &metadata, &message)? {
                reference = Some(response_key(&row));
            }
            Ok(())
        })
        .ok()?;
        reference
    }
    fn advance_local_context(&self, observation: &Observation, state: &mut Context) {
        if let Ok(record) = parse_safe(&observation.event) {
            let mut metadata = Metadata::from_context(state);
            metadata.apply(&record);
            metadata.save(state);
        }
    }
    fn prime_local_metadata(
        &self,
        _db: &Connection,
        observation: &Observation,
        _file: &str,
        state: &mut Context,
    ) -> Result<()> {
        self.advance_local_context(observation, state);
        Ok(())
    }
    fn ingest_local(
        &self,
        db: &Connection,
        observation: &Observation,
        file: &str,
        state: &mut Context,
        cutoff: Option<&chrono::DateTime<Utc>>,
    ) -> Result<usize> {
        let record = parse_safe(&observation.event)?;
        let mut metadata = Metadata::from_context(state);
        metadata.apply(&record);
        metadata.save(state);
        let mut inserted = 0;
        each_message(&record, |message| {
            let response = match response(observation, &metadata, &message) {
                Ok(Some(response)) => response,
                Ok(None) => return Ok(()),
                Err(_) => {
                    // 身份/时间缺失不能用文件名、mtime、采集时间代替，也不把错误正文存入诊断。
                    issue(db, "gemini_usage_identity", file,
                        &hash(json!({"session":metadata.session_id,"id":message.id,"time":message.timestamp}).to_string().as_bytes()),
                        "Gemini 计量缺少可信的原生会话/消息身份或消息时间，未进入用量事实")?;
                    return Ok(());
                }
            };
            if cutoff.is_some_and(|cutoff| {
                chrono::DateTime::parse_from_rfc3339(&response.event_time)
                    .is_ok_and(|time| time.with_timezone(&Utc) < *cutoff)
            }) {
                return Ok(());
            }
            if response.counts.total.value().is_none()
                || matches!(response.counts.cache_read, Metric::Invalid)
            {
                issue(
                    db,
                    "gemini_metric_contract",
                    file,
                    &canonical_digest(&response),
                    "Gemini 原生计量字段不完整或关系无效；未补零，保留白名单计量与覆盖缺口",
                )?;
            }
            store_metadata(db, &response, &metadata)?;
            inserted += store_checked(db, &response)?;
            Ok(())
        })?;
        Ok(inserted)
    }
    fn ingest_replayed_local(
        &self,
        db: &Connection,
        observation: &Observation,
        file: &str,
        state: &mut Context,
        cutoff: Option<&chrono::DateTime<Utc>>,
    ) -> Result<usize> {
        // 重放与首次观察使用同一身份和冲突策略，既不授予 revision，也不撤销已消费计量。
        self.ingest_local(db, observation, file, state, cutoff)
    }
    fn discover(&self, config: &SourceConfig) -> Result<Discovery> {
        self.discover_with_cancel(config, &AtomicBool::new(false))
    }
    fn discover_with_cancel(
        &self,
        config: &SourceConfig,
        cancel: &AtomicBool,
    ) -> Result<Discovery> {
        discover_with(config, cancel, &|path| fs::read_dir(path))
    }
}

fn is_link(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    false
}
fn ancestor_link(path: &Path) -> std::io::Result<bool> {
    for ancestor in path.ancestors().filter(|path| !path.as_os_str().is_empty()) {
        if is_link(&fs::symlink_metadata(ancestor)?) {
            return Ok(true);
        }
    }
    Ok(false)
}
fn registered_local_path(path: &Path) -> bool {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return false;
    }
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        if path.components().any(|component| matches!(component, Component::Prefix(prefix) if !matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))) { return false; }
    }
    true
}
struct DiscoveryBudget {
    entries: usize,
    stopped: bool,
}
fn discover_with<F>(config: &SourceConfig, cancel: &AtomicBool, read_dir: &F) -> Result<Discovery>
where
    F: Fn(&Path) -> std::io::Result<fs::ReadDir>,
{
    let mut found = Discovery::default();
    if cancel.load(Ordering::Relaxed) {
        found.cancelled = true;
        return Ok(found);
    }
    if !registered_local_path(&config.root) {
        return Err("Gemini 来源必须是用户登记的绝对本地目录".into());
    }
    let root = fs::symlink_metadata(&config.root)
        .map_err(|_| "Gemini 来源目录不存在或无法读取".to_owned())?;
    if ancestor_link(&config.root).map_err(|_| "Gemini 来源目录链无法检查".to_owned())? {
        found.unsupported = 1;
        return Ok(found);
    }
    if !root.is_dir() {
        return Err("Gemini 来源不是目录".into());
    }
    let tmp = if config
        .root
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("tmp"))
    {
        config.root.clone()
    } else {
        config.root.join("tmp")
    };
    let mut budget = DiscoveryBudget {
        entries: 0,
        stopped: false,
    };
    // tmp/<project>/chats 是唯一入口，不扫描同根的配置、凭据或任意其它 JSON。
    for project in child_paths(&tmp, &mut found, &mut budget, cancel, read_dir) {
        if cancelled(cancel, &mut found) || budget.stopped {
            break;
        }
        match fs::symlink_metadata(&project) {
            Ok(metadata) if is_link(&metadata) => {
                found.unsupported += 1;
            }
            Ok(metadata) if metadata.is_dir() => {
                let chats = project.join("chats");
                match fs::symlink_metadata(&chats) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => found.errors.push(chats),
                    Ok(_) => walk_chats(&chats, 0, &mut found, &mut budget, cancel, read_dir),
                }
            }
            Ok(_) => {}
            Err(_) => found.errors.push(project),
        }
    }
    found.files.sort();
    found.files.dedup();
    found.errors.sort();
    found.errors.dedup();
    Ok(found)
}
fn cancelled(cancel: &AtomicBool, found: &mut Discovery) -> bool {
    if cancel.load(Ordering::Relaxed) {
        found.cancelled = true;
        true
    } else {
        false
    }
}
fn child_paths<F>(
    directory: &Path,
    found: &mut Discovery,
    budget: &mut DiscoveryBudget,
    cancel: &AtomicBool,
    read_dir: &F,
) -> Vec<PathBuf>
where
    F: Fn(&Path) -> std::io::Result<fs::ReadDir>,
{
    let mut paths = Vec::new();
    if cancelled(cancel, found) || budget.stopped {
        return paths;
    }
    match fs::symlink_metadata(directory) {
        Ok(metadata) if is_link(&metadata) => {
            found.unsupported += 1;
            return paths;
        }
        Ok(metadata) if metadata.is_dir() => {}
        _ => {
            found.errors.push(directory.to_path_buf());
            return paths;
        }
    }
    let entries = match read_dir(directory) {
        Ok(entries) => entries,
        Err(_) => {
            found.errors.push(directory.to_path_buf());
            return paths;
        }
    };
    for entry in entries {
        if cancelled(cancel, found) {
            break;
        }
        if budget.entries >= MAX_DISCOVERY_ENTRIES {
            budget.stopped = true;
            found.errors.push(directory.to_path_buf());
            break;
        }
        budget.entries += 1;
        match entry {
            Ok(entry) => paths.push(entry.path()),
            Err(_) => found.errors.push(directory.to_path_buf()),
        }
    }
    paths
}
fn walk_chats<F>(
    directory: &Path,
    depth: usize,
    found: &mut Discovery,
    budget: &mut DiscoveryBudget,
    cancel: &AtomicBool,
    read_dir: &F,
) where
    F: Fn(&Path) -> std::io::Result<fs::ReadDir>,
{
    if cancelled(cancel, found) || budget.stopped {
        return;
    }
    if depth > MAX_DISCOVERY_DEPTH {
        found.errors.push(directory.to_path_buf());
        return;
    }
    for path in child_paths(directory, found, budget, cancel, read_dir) {
        if cancelled(cancel, found) {
            break;
        }
        // 已枚举的同层候选仍检查；下钻受全局预算控制，超限始终保留 partial 错误。
        match fs::symlink_metadata(&path) {
            Ok(metadata) if is_link(&metadata) => found.unsupported += 1,
            Ok(metadata) if metadata.is_dir() => {
                walk_chats(&path, depth + 1, found, budget, cancel, read_dir)
            }
            Ok(metadata) if metadata.is_file() => {
                match path.extension().and_then(|value| value.to_str()) {
                    Some("json" | "jsonl") => found.files.push(path),
                    Some("gz" | "zip" | "zst") => found.unsupported += 1,
                    _ => {}
                }
            }
            Ok(_) => {}
            Err(_) => found.errors.push(path),
        }
    }
}

#[cfg(test)]
#[path = "adapter_gemini_tests.rs"]
mod tests;
