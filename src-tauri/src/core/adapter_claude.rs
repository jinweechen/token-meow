//! Claude Code 本地 JSONL：只读取用户登记的 projects 树，不接入账户或推断生命周期。
//!
//! 参考来源（字段与 typed/忽略未知字段路线；不复用 ccusage 的取最大值去重策略）：
//! https://github.com/ccusage/ccusage/tree/7dc5c7684f5372e0275b2401e53d8c16adefbc43/rust/adapters/claude
//! https://code.claude.com/docs/en/monitoring-usage#map-input-tokens-to-opentelemetry-genai-semantic-conventions
//!
//! 开源出处 NOTICE：ccusage，固定提交 7dc5c7684f5372e0275b2401e53d8c16adefbc43。
//! MIT License — Copyright (c) 2025 ryoppippi
//! Permission is hereby granted, free of charge, to any person obtaining a copy
//! of this software and associated documentation files (the "Software"), to deal
//! in the Software without restriction, including without limitation the rights
//! to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//! copies of the Software, and to permit persons to whom the Software is
//! furnished to do so, subject to the following conditions:
//! The above copyright notice and this permission notice shall be included in all
//! copies or substantial portions of the Software.
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//! SOFTWARE.

use super::*;
use serde::de::{IgnoredAny, MapAccess, SeqAccess, Visitor};
use std::sync::atomic::{AtomicBool, Ordering};

const CLAUDE_SEMANTICS: &str = "claude-code-exclusive-input-v1";
const CLAUDE_NAMESPACE: &str = "claude-session-message-v1";
const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;
const MAX_TEXT_BYTES: usize = 512;
const MAX_DISCOVERY_ENTRIES: usize = 100_000;
const MAX_DISCOVERY_DEPTH: usize = 64;

pub(super) struct ClaudeAdapter;

// 未知字段由 serde 的 IgnoredAny 路径消耗，不把 content/prompt/title 物化为 Value。
// Option 只用于整个 usage 是否存在；计数字段单独保留 missing/null/invalid。
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SafeLine {
    #[serde(default, rename = "type")]
    kind: String,
    session_id: Option<String>,
    timestamp: Option<String>,
    request_id: Option<String>,
    agent_id: Option<String>,
    is_sidechain: Option<bool>,
    is_api_error_message: Option<bool>,
    message: Option<SafeMessage>,
}
#[derive(Default, Deserialize)]
struct SafeMessage {
    id: Option<String>,
    role: Option<String>,
    model: Option<String>,
    usage: Option<SafeUsage>,
}
#[derive(Default, Deserialize)]
struct SafeUsage {
    #[serde(default)]
    input_tokens: TokenMetric,
    #[serde(default)]
    output_tokens: TokenMetric,
    #[serde(default)]
    cache_creation_input_tokens: TokenMetric,
    #[serde(default)]
    cache_read_input_tokens: TokenMetric,
}
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
        struct TokenVisitor;
        impl<'de> Visitor<'de> for TokenVisitor {
            type Value = TokenMetric;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("原生非负整数 token 字段")
            }
            fn visit_i64<E: serde::de::Error>(self, n: i64) -> std::result::Result<Self::Value, E> {
                Ok(TokenMetric(if n >= 0 {
                    Metric::Valid(n)
                } else {
                    Metric::Invalid
                }))
            }
            fn visit_u64<E: serde::de::Error>(self, n: u64) -> std::result::Result<Self::Value, E> {
                Ok(TokenMetric(
                    i64::try_from(n)
                        .map(Metric::Valid)
                        .unwrap_or(Metric::Invalid),
                ))
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
        deserializer.deserialize_any(TokenVisitor)
    }
}
impl SafeUsage {
    fn native(&self) -> BTreeMap<String, Metric> {
        [
            ("input_tokens", &self.input_tokens),
            ("output_tokens", &self.output_tokens),
            (
                "cache_creation_input_tokens",
                &self.cache_creation_input_tokens,
            ),
            ("cache_read_input_tokens", &self.cache_read_input_tokens),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.0.clone()))
        .collect()
    }
    fn has_counter(&self) -> bool {
        self.native()
            .values()
            .any(|value| !matches!(value, Metric::Missing))
    }
    fn safe_value(&self) -> Value {
        let mut object = serde_json::Map::new();
        for (key, metric) in self.native() {
            let value = match metric {
                Metric::Valid(value) => json!(value),
                Metric::Unknown => Value::Null,
                Metric::Invalid => json!({"invalidTokenMetric":true}),
                Metric::Missing => continue,
            };
            object.insert(key, value);
        }
        Value::Object(object)
    }
}
fn safe_text(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_TEXT_BYTES && !value.chars().any(char::is_control)
}
fn safe_identity(value: &str) -> bool {
    safe_text(value)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
}
impl SafeLine {
    fn validate_text(&self) -> Result<()> {
        let message = self.message.as_ref();
        for value in [
            Some(self.kind.as_str()),
            self.session_id.as_deref(),
            self.timestamp.as_deref(),
            self.request_id.as_deref(),
            self.agent_id.as_deref(),
            message.and_then(|m| m.id.as_deref()),
            message.and_then(|m| m.role.as_deref()),
            message.and_then(|m| m.model.as_deref()),
        ]
        .into_iter()
        .flatten()
        {
            // 非 assistant 行的 type 缺失不产生事实；非空已出现的字段仍有统一大小边界。
            if !value.is_empty() && !safe_text(value) {
                return Err("Claude 元数据字段超限或包含控制字符".into());
            }
        }
        Ok(())
    }
    fn validate_response_fields(&self) -> Result<()> {
        if self.kind != "assistant" || self.is_api_error_message == Some(true) {
            return Ok(());
        }
        let Some(message) = self.message.as_ref() else {
            return Ok(());
        };
        if message
            .role
            .as_deref()
            .is_some_and(|role| role != "assistant")
        {
            return Ok(());
        }
        if !message.usage.as_ref().is_some_and(SafeUsage::has_counter) {
            return Ok(());
        }
        if !self.session_id.as_deref().is_some_and(safe_identity) {
            return Err("Claude 用量缺少可信 sessionId".into());
        }
        if !message.id.as_deref().is_some_and(safe_identity) {
            return Err("Claude 用量缺少可信 message.id".into());
        }
        if !self
            .timestamp
            .as_deref()
            .is_some_and(|value| chrono::DateTime::parse_from_rfc3339(value).is_ok())
        {
            return Err("Claude 原生响应时间不是 RFC3339".into());
        }
        Ok(())
    }
    fn safe_value(&self) -> Value {
        let message = self.message.as_ref().map(|m| {
            json!({"id":m.id,"role":m.role,"model":m.model,
            "usage":m.usage.as_ref().map(SafeUsage::safe_value)})
        });
        json!({"type":self.kind,"sessionId":self.session_id,"timestamp":self.timestamp,
            "requestId":self.request_id,"agentId":self.agent_id,"isSidechain":self.is_sidechain,
            "isApiErrorMessage":self.is_api_error_message,"message":message})
    }
    fn metadata(&self) -> Value {
        json!({"sessionId":self.session_id,"requestId":self.request_id,"agentId":self.agent_id,
            "isSidechain":self.is_sidechain})
    }
}

/// reader 必须在构造 Observation 前调用；错误文本不包含输入片段或 serde 原始错误。
pub(super) fn project_line(bytes: &[u8]) -> Result<Value> {
    if bytes.len() > MAX_LINE_BYTES {
        return Err("Claude JSONL 完整行超过 4 MiB 上限".into());
    }
    let row: SafeLine = serde_json::from_slice(bytes)
        .map_err(|_| "Claude JSONL 不是支持的完整元数据记录".to_owned())?;
    row.validate_text()?;
    // 在 reader 的逐行错误隔离边界内拒绝坏身份/时间，避免 ingest 错误阻断整文件游标。
    row.validate_response_fields()?;
    Ok(row.safe_value())
}
fn parse_safe(value: &Value) -> Result<SafeLine> {
    let row: SafeLine = serde_json::from_value(value.clone())
        .map_err(|_| "Claude JSONL 元数据字段类型无效".to_owned())?;
    row.validate_text()?;
    row.validate_response_fields()?;
    Ok(row)
}
fn add_metrics(values: &[&Metric]) -> Metric {
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
        .map(Metric::Valid)
        .unwrap_or(Metric::Invalid)
}

impl SourceAdapter for ClaudeAdapter {
    fn client_kind(&self) -> &'static str {
        "claude"
    }
    fn parse_line(&self, bytes: &[u8]) -> Result<Value> {
        project_line(bytes)
    }
    fn identity_policy(&self) -> IdentityPolicy {
        IdentityPolicy {
            namespace: CLAUDE_NAMESPACE.into(),
            scope: "source+native-sessionId+message.id".into(),
            revisions: false,
        }
    }
    fn capabilities(&self) -> Value {
        let field = |supported: bool, scope: &str| {
            json!({
            "support":if supported {"仅文档支持"} else {"不支持"},"runtime":null,"version":null,
            "supportedVersion":"仅已实现字段子集；当前 CLI 版本未探测","testedVersion":null,
            "channel":"local-jsonl","granularity":if supported {"response"} else {"none"},
            "prerequisites":["用户登记本地 Claude projects 根目录；只处理完整 JSONL 行","sessionId、message.id 与原生 RFC3339 时间有效"],
            "evidenceRefs":["https://code.claude.com/docs/en/monitoring-usage","https://github.com/ccusage/ccusage/tree/7dc5c7684f5372e0275b2401e53d8c16adefbc43/rust/adapters/claude"],
            "verifiedAt":null,"verificationScope":scope,"versionPolicy":"未进行真实 Claude CLI 脱敏样本资格验收；字段缺失保留未知"})
        };
        json!({"usage":field(true,"仅本地 assistant 消息原生用量；不代表账单或订阅额度"),
            "cacheRead":field(true,"原生 cache_read_input_tokens；属于规范化输入子集"),
            "cacheWrite":field(true,"原生 cache_creation_input_tokens；不重复叠加缓存时长子类"),
            "reasoning":field(false,"未提供可独立计数的原生推理字段"),
            "turnCompletion":field(false,"不从消息正文、文件时间或 assistant 行推断任务完成"),
            "turnTiming":field(false,"没有真实任务生命周期"),"requestTiming":field(false,"没有请求起止证据"),
            "subagents":field(false,"仅保留 agentId/isSidechain 元数据；未证明父子树和完成状态"),
            "context":field(false,"未接入 context window"),"chatTarget":field(false,"无验证过的会话跳转协议"),
            "quota":field(false,"日志 token 不是订阅百分比；未接入官方限额通道"),
            "accountUsage":field(false,"不读取配置、凭据或账户")})
    }
    fn normalize(&self, observation: &Observation) -> Result<Vec<StandardEvent>> {
        let row = parse_safe(&observation.event)?;
        if row.kind != "assistant" || row.is_api_error_message == Some(true) {
            return Ok(vec![]);
        }
        let Some(message) = row.message.as_ref() else {
            return Ok(vec![]);
        };
        if message
            .role
            .as_deref()
            .is_some_and(|role| role != "assistant")
        {
            return Ok(vec![]);
        }
        let Some(usage) = message.usage.as_ref().filter(|usage| usage.has_counter()) else {
            return Ok(vec![]);
        };
        let session = row
            .session_id
            .as_deref()
            .filter(|id| safe_identity(id))
            .ok_or("Claude 用量缺少可信 sessionId")?;
        let response = message
            .id
            .as_deref()
            .filter(|id| safe_identity(id))
            .ok_or("Claude 用量缺少可信 message.id")?;
        let time = row
            .timestamp
            .as_deref()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .ok_or("Claude 原生响应时间不是 RFC3339")?
            .with_timezone(&Utc)
            .to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true);
        let input = add_metrics(&[
            &usage.input_tokens.0,
            &usage.cache_creation_input_tokens.0,
            &usage.cache_read_input_tokens.0,
        ]);
        let output = usage.output_tokens.0.clone();
        let total = add_metrics(&[&input, &output]);
        Ok(vec![StandardEvent::ResponseUsage(ResponseUsage {
            source_id: observation.source_id.clone(),
            channel_id: observation.channel_id.clone(),
            namespace: CLAUDE_NAMESPACE.into(),
            thread_id: session.into(),
            response_id: response.into(),
            turn_id: None,
            root_turn_id: None,
            event_time: time,
            collected_at: now(),
            requested_model: None,
            observed_model: message
                .model
                .as_deref()
                .filter(|model| safe_text(model) && *model != "<synthetic>")
                .map(str::to_owned),
            provider_id: None,
            account_scope_id: None,
            native_counts: usage.native(),
            counts: Counts {
                input,
                output,
                cache_read: usage.cache_read_input_tokens.0.clone(),
                cache_write: usage.cache_creation_input_tokens.0.clone(),
                reasoning: Metric::Missing,
                total,
            },
            semantics_version: CLAUDE_SEMANTICS.into(),
            adapter_version: "claude-jsonl-v1".into(),
            completeness: "source-recorded".into(),
            phase: "source-recorded".into(),
            revision: None,
            live: false,
        })])
    }
    fn advance_local_context(&self, observation: &Observation, state: &mut Context) {
        if let Ok(row) = parse_safe(&observation.event) {
            if row.kind != "assistant" {
                return;
            }
            if let Some(session) = row.session_id.as_deref().filter(|id| safe_identity(id)) {
                state.active_thread = session.to_owned();
                // 固定一个键避免每行增长，也不把原始 observation 或消息正文保留到游标。
                state
                    .legacy
                    .insert("claude.metadata".into(), row.metadata());
            }
        }
    }
    fn ingest_local(
        &self,
        db: &Connection,
        observation: &Observation,
        file: &str,
        state: &mut Context,
        cutoff: Option<&chrono::DateTime<Utc>>,
    ) -> Result<usize> {
        let events = self.normalize(observation)?;
        self.advance_local_context(observation, state);
        let mut inserted = 0;
        for event in events {
            let StandardEvent::ResponseUsage(response) = &event else {
                continue;
            };
            if cutoff.is_some_and(|cutoff| {
                chrono::DateTime::parse_from_rfc3339(&response.event_time)
                    .is_ok_and(|time| time.with_timezone(&Utc) < *cutoff)
            }) {
                continue;
            }
            if response.counts.total.value().is_none() {
                // 重放也执行诊断；不能把保留了 Unknown/Missing/Invalid 的事实默默当作零。
                // 只保存规范计量摘要；字段原文、正文与工具参数均不进入 issue。
                issue(
                    db,
                    "claude_metric_contract",
                    file,
                    &canonical_digest(response),
                    "Claude 原生计量不完整或无效；未补零，保留白名单计量与覆盖缺口",
                )?;
            }
            let row = parse_safe(&observation.event)?;
            let metadata = json!({"id":response.thread_id,"sourceId":response.source_id,
                "title":format!("Claude 会话 {}",response.thread_id.chars().take(8).collect::<String>()),
                "cwd":"","parentThreadId":null,"rootThreadId":null,"model":response.observed_model,"accountScopeId":null,
                "claude":row.metadata()});
            db.execute(
                "INSERT OR IGNORE INTO thread_metadata(id,data) VALUES(?1,?2)",
                params![
                    format!("{}|{}", response.source_id, response.thread_id),
                    metadata.to_string()
                ],
            )
            .map_err(err)?;
            inserted += store_standard_event(db, &event, false)?;
        }
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
        // 文件游标重放不提供新修订权；仍经过同一事实身份与冲突检查。
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
    if !config.root.is_absolute() {
        return Err("Claude 来源必须是用户登记的绝对本地目录".into());
    }
    let root = fs::symlink_metadata(&config.root)
        .map_err(|_| "Claude 来源目录不存在或无法读取".to_owned())?;
    if ancestor_link(&config.root).map_err(|_| "Claude 来源目录链无法检查".to_owned())? {
        found.unsupported = 1;
        return Ok(found);
    }
    if !root.is_dir() {
        return Err("Claude 来源不是目录".into());
    }
    let projects = if config
        .root
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("projects"))
    {
        config.root.clone()
    } else {
        config.root.join("projects")
    };
    let mut budget = DiscoveryBudget {
        entries: 0,
        stopped: false,
    };
    walk_projects(&projects, 0, &mut found, &mut budget, cancel, read_dir);
    found.files.sort();
    found.files.dedup();
    found.errors.sort();
    found.errors.dedup();
    Ok(found)
}
fn walk_projects<F>(
    directory: &Path,
    depth: usize,
    found: &mut Discovery,
    budget: &mut DiscoveryBudget,
    cancel: &AtomicBool,
    read_dir: &F,
) where
    F: Fn(&Path) -> std::io::Result<fs::ReadDir>,
{
    if cancel.load(Ordering::Relaxed) {
        found.cancelled = true;
        return;
    }
    if budget.stopped {
        return;
    }
    if depth > MAX_DISCOVERY_DEPTH {
        found.errors.push(directory.to_path_buf());
        return;
    }
    match fs::symlink_metadata(directory) {
        Ok(metadata) if is_link(&metadata) => {
            found.unsupported += 1;
            return;
        }
        Ok(metadata) if metadata.is_dir() => {}
        _ => {
            found.errors.push(directory.to_path_buf());
            return;
        }
    }
    let entries = match read_dir(directory) {
        Ok(entries) => entries,
        Err(_) => {
            found.errors.push(directory.to_path_buf());
            return;
        }
    };
    for entry in entries {
        if cancel.load(Ordering::Relaxed) {
            found.cancelled = true;
            break;
        }
        if budget.entries >= MAX_DISCOVERY_ENTRIES {
            found.errors.push(directory.to_path_buf());
            budget.stopped = true;
            break;
        }
        budget.entries += 1;
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                found.errors.push(directory.to_path_buf());
                continue;
            }
        };
        let path = entry.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => {
                found.errors.push(path);
                continue;
            }
        };
        if is_link(&metadata) {
            found.unsupported += 1;
            continue;
        }
        if metadata.is_dir() {
            walk_projects(&path, depth + 1, found, budget, cancel, read_dir);
        } else if metadata.is_file() && depth >= 1 {
            match path.extension().and_then(|extension| extension.to_str()) {
                Some("jsonl") => found.files.push(path),
                Some("gz" | "zip" | "zst") => found.unsupported += 1,
                _ => {}
            }
        }
        if budget.stopped {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(usage: Value) -> Value {
        json!({"type":"assistant","sessionId":"synthetic-session","requestId":"req-original",
            "timestamp":"2026-10-23T00:00:00Z","message":{"id":"msg-original","role":"assistant",
            "model":"synthetic-model","usage":usage,"content":[{"text":"PRIVATE_ASSISTANT_BODY"}]},
            "title":"PRIVATE_TITLE","cwd":"PRIVATE_CWD","prompt":"PRIVATE_PROMPT"})
    }
    fn usage() -> Value {
        json!({"input_tokens":100,"output_tokens":20,"cache_creation_input_tokens":50,"cache_read_input_tokens":300})
    }
    fn observation(event: Value) -> Observation {
        Observation {
            source_id: "claude-source".into(),
            channel_id: "claude-source:jsonl".into(),
            event,
        }
    }
    fn normalized(event: Value) -> ResponseUsage {
        let projected = project_line(&serde_json::to_vec(&event).unwrap()).unwrap();
        match ClaudeAdapter
            .normalize(&observation(projected))
            .unwrap()
            .pop()
            .unwrap()
        {
            StandardEvent::ResponseUsage(row) => row,
            _ => panic!("只允许响应事实"),
        }
    }
    fn core() -> (tempfile::TempDir, Core) {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let core = Core::open(&temp.path().join("data"), &source).unwrap();
        (temp, core)
    }
    #[test]
    fn exclusive_input_is_normalized_without_adding_cache_subtypes_twice() {
        let mut counters = usage();
        counters["cache_creation"] =
            json!({"ephemeral_5m_input_tokens":30,"ephemeral_1h_input_tokens":20});
        let row = normalized(fixture(counters));
        assert_eq!(row.counts.input, Metric::Valid(450));
        assert_eq!(row.counts.output, Metric::Valid(20));
        assert_eq!(row.counts.total, Metric::Valid(470));
        assert_eq!(row.counts.cache_read, Metric::Valid(300));
        assert_eq!(row.counts.cache_write, Metric::Valid(50));
        assert_eq!(row.counts.reasoning, Metric::Missing);
        assert_eq!(row.native_counts["input_tokens"], Metric::Valid(100));
        assert_eq!(row.observed_model.as_deref(), Some("synthetic-model"));
        assert!(
            row.requested_model.is_none()
                && row.provider_id.is_none()
                && row.account_scope_id.is_none()
        );
        assert!(
            row.turn_id.is_none()
                && row.root_turn_id.is_none()
                && row.revision.is_none()
                && !row.live
        );
        assert!(!ClaudeAdapter.allows_revisions());
    }
    #[test]
    fn absent_null_invalid_and_zero_tokens_remain_distinct() {
        let row = normalized(fixture(
            json!({"input_tokens":0,"output_tokens":0,"cache_read_input_tokens":null}),
        ));
        assert_eq!(row.native_counts["input_tokens"], Metric::Valid(0));
        assert_eq!(
            row.native_counts["cache_read_input_tokens"],
            Metric::Unknown
        );
        assert_eq!(
            row.native_counts["cache_creation_input_tokens"],
            Metric::Missing
        );
        assert_eq!(row.counts.input, Metric::Unknown);
        assert_eq!(row.counts.total, Metric::Unknown);
        for invalid in [
            json!(-1),
            json!(1.0),
            json!("2"),
            json!(true),
            json!([]),
            json!({"tokens":2}),
            json!(u64::MAX),
        ] {
            let mut counters = usage();
            counters["input_tokens"] = invalid;
            let row = normalized(fixture(counters));
            assert_eq!(row.native_counts["input_tokens"], Metric::Invalid);
            assert_eq!(row.counts.input, Metric::Invalid);
            assert_eq!(row.counts.total, Metric::Invalid);
        }
        let row = normalized(fixture(
            json!({"input_tokens":i64::MAX,"output_tokens":1,"cache_creation_input_tokens":0,"cache_read_input_tokens":1}),
        ));
        assert_eq!(row.counts.input, Metric::Invalid);
    }
    #[test]
    fn null_or_absent_usage_and_non_assistant_rows_never_create_response() {
        for counters in [Value::Null, json!({}), json!({"tokens":null})] {
            assert!(ClaudeAdapter
                .normalize(&observation(fixture(counters)))
                .unwrap()
                .is_empty());
        }
        let mut event = fixture(usage());
        event["message"].as_object_mut().unwrap().remove("usage");
        assert!(ClaudeAdapter
            .normalize(&observation(event))
            .unwrap()
            .is_empty());
        let mut event = fixture(usage());
        event["type"] = json!("user");
        assert!(ClaudeAdapter
            .normalize(&observation(event))
            .unwrap()
            .is_empty());
        let mut event = fixture(usage());
        event["isApiErrorMessage"] = json!(true);
        assert!(ClaudeAdapter
            .normalize(&observation(event))
            .unwrap()
            .is_empty());
    }
    #[test]
    fn response_identity_uses_session_message_and_never_line_uuid_or_request_id() {
        let first = normalized(fixture(usage()));
        let mut event = fixture(usage());
        event["uuid"] = json!("new-line-uuid");
        event["requestId"] = json!("new-request-id");
        event["timestamp"] = json!("2026-10-23T01:00:00Z");
        assert_eq!(response_key(&first), response_key(&normalized(event)));
        let mut event = fixture(usage());
        event["sessionId"] = json!("another-session");
        assert_ne!(response_key(&first), response_key(&normalized(event)));
    }
    #[test]
    fn invalid_native_identity_is_rejected_at_line_boundary_and_line_uuid_cannot_replace_it() {
        for field in ["sessionId", "messageId"] {
            let mut event = fixture(usage());
            if field == "sessionId" {
                event["sessionId"] = json!("session|injected");
            } else {
                event["message"]["id"] = Value::Null;
                event["uuid"] = json!("line-uuid");
            }
            assert!(project_line(&serde_json::to_vec(&event).unwrap()).is_err());
        }
        let mut event = fixture(usage());
        event["timestamp"] = json!("2026-10-23T00:00:00.123456789Z");
        assert_eq!(
            normalized(event).event_time,
            "2026-10-23T00:00:00.123456789Z"
        );
        let row = normalized(fixture(
            json!({"input_tokens":i64::MAX,"output_tokens":1,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}),
        ));
        assert_eq!(row.counts.input, Metric::Valid(i64::MAX));
        assert_eq!(row.counts.total, Metric::Invalid);
    }
    #[test]
    fn duplicate_is_idempotent_but_changed_counts_are_excluded_without_fake_revision() {
        let (_temp, core) = core();
        let mut context = Context::default();
        let original = observation(fixture(usage()));
        assert_eq!(
            ClaudeAdapter
                .ingest_local(&core.db, &original, "synthetic.jsonl", &mut context, None)
                .unwrap(),
            1
        );
        assert_eq!(
            ClaudeAdapter
                .ingest_local(&core.db, &original, "copy.jsonl", &mut context, None)
                .unwrap(),
            0
        );
        let mut changed = original.clone();
        changed.event["message"]["usage"]["output_tokens"] = json!(250);
        assert_eq!(
            ClaudeAdapter
                .ingest_local(&core.db, &changed, "rewrite.jsonl", &mut context, None)
                .unwrap(),
            0
        );
        let (rows,excluded,conflicts):(i64,i64,i64)=core.db.query_row(
            "SELECT (SELECT count(*) FROM usage_responses),(SELECT count(*) FROM excluded_facts),(SELECT count(*) FROM identity_conflicts)",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!((rows, excluded, conflicts), (1, 1, 1));
        let stored: String = core
            .db
            .query_row("SELECT data FROM usage_responses", [], |r| r.get(0))
            .unwrap();
        let row: ResponseUsage = serde_json::from_str(&stored).unwrap();
        assert_eq!(row.counts.output, Metric::Valid(20));
        assert!(row.revision.is_none());
    }
    #[test]
    fn body_is_discarded_before_value_and_never_enters_context_or_ledger() {
        let (_temp, core) = core();
        let mut context = Context::default();
        let projected = project_line(&serde_json::to_vec(&fixture(usage())).unwrap()).unwrap();
        assert!(!projected.to_string().contains("PRIVATE_"));
        ClaudeAdapter
            .ingest_local(
                &core.db,
                &observation(projected),
                "synthetic.jsonl",
                &mut context,
                None,
            )
            .unwrap();
        let mut stored = serde_json::to_string(&context).unwrap();
        for table in [
            "usage_responses",
            "thread_metadata",
            "fact_observations",
            "turns",
            "items",
        ] {
            let column = if table == "fact_observations" {
                "digest"
            } else {
                "data"
            };
            let text: Option<String> = core
                .db
                .query_row(
                    &format!("SELECT group_concat({column}) FROM {table}"),
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            stored.push_str(text.as_deref().unwrap_or(""));
        }
        assert!(!stored.contains("PRIVATE_"));
        let turns: i64 = core
            .db
            .query_row("SELECT count(*) FROM turns", [], |r| r.get(0))
            .unwrap();
        assert_eq!(turns, 0);
    }
    #[test]
    fn cutoff_is_native_time_only_and_bad_timestamp_has_no_mtime_fallback() {
        let (_temp, core) = core();
        let mut context = Context::default();
        let cutoff = chrono::DateTime::parse_from_rfc3339("2026-10-24T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            ClaudeAdapter
                .ingest_local(
                    &core.db,
                    &observation(fixture(usage())),
                    "synthetic.jsonl",
                    &mut context,
                    Some(&cutoff)
                )
                .unwrap(),
            0
        );
        for bad in [json!("2026-10-23"), json!(1_792_710_000), Value::Null] {
            let mut event = fixture(usage());
            event["timestamp"] = bad;
            assert!(ClaudeAdapter.normalize(&observation(event)).is_err());
        }
    }
    #[test]
    fn relevant_duplicate_members_and_incomplete_lines_are_rejected_without_echoing_body() {
        let bad = br#"{"type":"assistant","sessionId":"s","sessionId":"PRIVATE_DUPLICATE"}"#;
        let error = project_line(bad).unwrap_err();
        assert!(!error.contains("PRIVATE_"));
        assert!(project_line(
            br#"{"type":"assistant","message":{"usage":{"input_tokens":1,"input_tokens":2}}}"#
        )
        .is_err());
        assert!(project_line(br#"{"type":"assistant""#).is_err());
        assert!(project_line(&vec![b' '; MAX_LINE_BYTES + 1]).is_err());
    }
    fn config(root: PathBuf) -> SourceConfig {
        SourceConfig {
            id: "claude-source".into(),
            root,
            environment: "fixture".into(),
        }
    }
    #[test]
    fn discovery_only_reads_project_descendants_and_accepts_selected_projects_root() {
        let temp = tempfile::tempdir().unwrap();
        let projects = temp.path().join("projects");
        let nested = projects
            .join("project-a")
            .join("session-a")
            .join("subagents");
        fs::create_dir_all(&nested).unwrap();
        let expected = nested.join("agent-a.jsonl");
        fs::write(&expected, b"{}\n").unwrap();
        fs::write(projects.join("not-a-project.jsonl"), b"{}\n").unwrap();
        fs::write(temp.path().join("credentials.json"), b"PRIVATE_CREDENTIAL").unwrap();
        fs::write(temp.path().join("outside.jsonl"), b"{}\n").unwrap();
        for root in [temp.path().to_path_buf(), projects] {
            let found = ClaudeAdapter.discover(&config(root)).unwrap();
            assert_eq!(found.files, vec![expected.clone()]);
            assert!(found.errors.is_empty());
        }
    }
    #[test]
    fn unreadable_sibling_is_partial_and_cancellation_stops_before_open() {
        let temp = tempfile::tempdir().unwrap();
        let projects = temp.path().join("projects");
        let denied = projects.join("denied");
        let readable = projects.join("readable");
        fs::create_dir_all(&denied).unwrap();
        fs::create_dir(&readable).unwrap();
        let expected = readable.join("one.jsonl");
        fs::write(&expected, b"{}\n").unwrap();
        let found = discover_with(
            &config(temp.path().to_path_buf()),
            &AtomicBool::new(false),
            &|path| {
                if path == denied {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "synthetic denied",
                    ))
                } else {
                    fs::read_dir(path)
                }
            },
        )
        .unwrap();
        assert_eq!(found.files, vec![expected]);
        assert_eq!(found.errors, vec![denied]);
        let found = discover_with(
            &config(temp.path().to_path_buf()),
            &AtomicBool::new(true),
            &|_| panic!("取消后不得打开目录"),
        )
        .unwrap();
        assert!(found.cancelled && found.files.is_empty());
    }
    #[cfg(windows)]
    #[test]
    fn directory_junction_is_not_followed_even_as_registered_root() {
        use std::os::windows::process::CommandExt;
        let temp = tempfile::tempdir().unwrap();
        let external = temp.path().join("external");
        let projects = temp.path().join("projects");
        fs::create_dir_all(external.join("project-a")).unwrap();
        fs::write(external.join("project-a").join("outside.jsonl"), b"{}\n").unwrap();
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&projects)
            .arg(&external)
            .creation_flags(0x08000000)
            .output()
            .unwrap();
        assert!(status.status.success());
        let found = ClaudeAdapter.discover(&config(projects.clone())).unwrap();
        assert!(found.files.is_empty());
        assert_eq!(found.unsupported, 1);
        fs::remove_dir(&projects).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn directory_symlink_is_not_followed() {
        let temp = tempfile::tempdir().unwrap();
        let external = temp.path().join("external");
        fs::create_dir_all(external.join("project-a")).unwrap();
        let projects = temp.path().join("projects");
        std::os::unix::fs::symlink(&external, &projects).unwrap();
        let found = ClaudeAdapter.discover(&config(projects)).unwrap();
        assert!(found.files.is_empty());
        assert_eq!(found.unsupported, 1);
    }
}
