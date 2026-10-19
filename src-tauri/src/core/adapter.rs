//! Source-specific field semantics terminate here; the ledger receives typed observations.
use super::reader::Context;
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const SEMANTICS: &str = "codex-native-0.147-v1";
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum Metric {
    Valid(i64),
    Missing,
    Unknown,
    Invalid,
}
impl Metric {
    pub fn value(&self) -> Option<i64> {
        if let Self::Valid(n) = self {
            Some(*n)
        } else {
            None
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub input: Metric,
    pub output: Metric,
    pub cache_read: Metric,
    pub cache_write: Metric,
    pub reasoning: Metric,
    pub total: Metric,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseUsage {
    pub source_id: String,
    pub channel_id: String,
    pub namespace: String,
    pub thread_id: String,
    pub response_id: String,
    pub turn_id: Option<String>,
    pub root_turn_id: Option<String>,
    pub event_time: String,
    pub collected_at: String,
    pub requested_model: Option<String>,
    pub observed_model: Option<String>,
    pub provider_id: Option<String>,
    pub account_scope_id: Option<String>,
    pub native_counts: BTreeMap<String, Metric>,
    pub counts: Counts,
    pub semantics_version: String,
    pub adapter_version: String,
    pub completeness: String,
    pub phase: String,
    pub revision: Option<u64>,
    #[serde(default)]
    pub live: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum StandardEvent {
    ResponseUsage(ResponseUsage),
    TurnLifecycle(LifecycleObservation),
    ToolLifecycle(LifecycleObservation),
    UsageAggregate(AggregateObservation),
    QuotaSnapshot(SnapshotObservation),
    ContextSnapshot(SnapshotObservation),
    SourceHealth(HealthObservation),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Granularity {
    Response,
    Turn,
    Session,
    Account,
    Window,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementMode {
    Delta,
    Cumulative,
    Gauge,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleObservation {
    pub source_id: String,
    pub channel_id: String,
    pub native_id: String,
    pub namespace: String,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub root_turn_id: Option<String>,
    pub status: String,
    pub event_time: String,
    pub duration_ms: Metric,
    pub timing_semantics: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AggregateObservation {
    pub id: String,
    pub source_id: String,
    pub channel_id: String,
    pub granularity: Granularity,
    pub mode: MeasurementMode,
    pub interval_start: String,
    pub interval_end: String,
    pub epoch: String,
    pub dimensions: BTreeMap<String, String>,
    pub metrics: BTreeMap<String, Metric>,
    pub semantics_version: String,
    pub coverage: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotObservation {
    pub id: String,
    pub source_id: String,
    pub channel_id: String,
    pub observed_at: String,
    pub account_scope_id: Option<String>,
    pub metrics: BTreeMap<String, Metric>,
    pub semantics_version: String,
    pub validity: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthObservation {
    pub source_id: String,
    pub channel_id: String,
    pub observed_at: String,
    pub support: String,
    pub runtime: String,
    pub issue_code: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceConfig {
    pub id: String,
    pub root: PathBuf,
    pub environment: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityPolicy {
    pub namespace: String,
    pub scope: String,
    pub revisions: bool,
}
pub trait ReadOnlyAccountChannel {
    fn read_account_at(&mut self, root: &Path) -> Result<Value>;
    fn list_sessions_at(&mut self, root: &Path) -> Result<Vec<Value>>;
}
impl ReadOnlyAccountChannel for crate::account_bridge::AccountBridge {
    fn read_account_at(&mut self, root: &Path) -> Result<Value> {
        self.set_source_root(root);
        serde_json::to_value(self.refresh()).map_err(err)
    }
    fn list_sessions_at(&mut self, root: &Path) -> Result<Vec<Value>> {
        self.set_source_root(root);
        self.list_sessions()
    }
}
#[derive(Clone, Debug, Default)]
pub struct Discovery {
    pub files: Vec<PathBuf>,
    pub unsupported: usize,
    pub errors: Vec<PathBuf>,
    pub cancelled: bool,
}
#[derive(Clone, Debug)]
pub struct Observation {
    pub source_id: String,
    pub channel_id: String,
    pub event: Value,
}
pub trait SourceAdapter: Sync {
    fn client_kind(&self) -> &'static str;
    fn capabilities(&self) -> Value;
    fn normalize(&self, observation: &Observation) -> Result<Vec<StandardEvent>>;
    fn allows_revisions(&self) -> bool {
        false
    }
    fn probe(&self, config: &SourceConfig) -> Value {
        json!({"sourceId":config.id,"clientKind":self.client_kind(),"runtime":if config.root.is_dir(){"可用"}else{"不可用"},"environment":config.environment})
    }
    fn identity_policy(&self) -> IdentityPolicy {
        IdentityPolicy {
            namespace: format!("{}-native-v1", self.client_kind()),
            scope: "source-defined".into(),
            revisions: self.allows_revisions(),
        }
    }
    fn build_chat_target(&self, _thread_id: &str) -> Option<String> {
        None
    }
    fn read_account(
        &self,
        _config: &SourceConfig,
        _channel: &mut dyn ReadOnlyAccountChannel,
    ) -> Result<Value> {
        Err("此适配器未提供账户读取能力".into())
    }
    fn list_sessions(
        &self,
        _config: &SourceConfig,
        _channel: &mut dyn ReadOnlyAccountChannel,
    ) -> Result<Vec<Value>> {
        Err("此适配器未提供会话目录读取能力".into())
    }
    fn discover(&self, _config: &SourceConfig) -> Result<Discovery> {
        Err("此适配器未提供文件通道".into())
    }
    fn discover_with_cancel(
        &self,
        config: &SourceConfig,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<Discovery> {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(Discovery {
                cancelled: true,
                ..Discovery::default()
            });
        }
        self.discover(config)
    }
    fn ingest_local(
        &self,
        db: &Connection,
        observation: &Observation,
        file: &str,
        _state: &mut Context,
        _cutoff: Option<&chrono::DateTime<Utc>>,
    ) -> Result<usize> {
        let mut n = 0;
        for event in self.normalize(observation)? {
            n += store_standard_event(db, &event, self.allows_revisions())?;
        }
        let _ = file;
        Ok(n)
    }
    fn advance_local_context(&self, _observation: &Observation, _state: &mut Context) {}
    fn ingest_replayed_local(
        &self,
        _db: &Connection,
        observation: &Observation,
        _file: &str,
        state: &mut Context,
        _cutoff: Option<&chrono::DateTime<Utc>>,
    ) -> Result<usize> {
        self.advance_local_context(observation, state);
        Ok(0)
    }
    fn prime_local_metadata(
        &self,
        _db: &Connection,
        _observation: &Observation,
        _file: &str,
        _state: &mut Context,
    ) -> Result<()> {
        Ok(())
    }
    fn fact_reference(&self, observation: &Observation) -> Option<String> {
        self.normalize(observation)
            .ok()?
            .into_iter()
            .find_map(|e| match e {
                StandardEvent::ResponseUsage(r) => Some(response_key(&r)),
                _ => None,
            })
    }
}
static CODEX: CodexAdapter = CodexAdapter;
pub fn adapter_for(client_kind: &str) -> Option<&'static dyn SourceAdapter> {
    match client_kind {
        "codex" => Some(&CODEX),
        _ => None,
    }
}
pub struct CodexAdapter;
impl SourceAdapter for CodexAdapter {
    fn client_kind(&self) -> &'static str {
        "codex"
    }
    fn read_account(
        &self,
        config: &SourceConfig,
        channel: &mut dyn ReadOnlyAccountChannel,
    ) -> Result<Value> {
        channel.read_account_at(&config.root)
    }
    fn list_sessions(
        &self,
        config: &SourceConfig,
        channel: &mut dyn ReadOnlyAccountChannel,
    ) -> Result<Vec<Value>> {
        channel.list_sessions_at(&config.root)
    }
    fn discover(&self, config: &SourceConfig) -> Result<Discovery> {
        self.discover_with_cancel(config, &std::sync::atomic::AtomicBool::new(false))
    }
    fn discover_with_cancel(
        &self,
        config: &SourceConfig,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<Discovery> {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(Discovery {
                cancelled: true,
                ..Discovery::default()
            });
        }
        if !config.root.is_dir() {
            return Err("Codex 来源目录不存在".into());
        }
        let mut result = Discovery::default();
        let sessions = config.root.join("sessions");
        let archived = config.root.join("archived_sessions");
        for directory in [&sessions, &archived] {
            match fs::symlink_metadata(directory) {
                Ok(_) => discover_codex(directory, &mut result, true, cancel),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => result.errors.push(directory.clone()),
            }
        }
        let selected_sessions = ["sessions", "archived_sessions"].contains(
            &config
                .root
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(""),
        );
        discover_codex(&config.root, &mut result, selected_sessions, cancel);
        result.files.sort();
        result.files.dedup();
        result.errors.sort();
        result.errors.dedup();
        Ok(result)
    }
    fn ingest_local(
        &self,
        db: &Connection,
        o: &Observation,
        file: &str,
        state: &mut Context,
        cutoff: Option<&chrono::DateTime<Utc>>,
    ) -> Result<usize> {
        ingest(db, &o.source_id, file, &o.event, state, cutoff)
    }
    fn advance_local_context(&self, o: &Observation, state: &mut Context) {
        advance_codex_context(&o.event, state);
    }
    fn ingest_replayed_local(
        &self,
        db: &Connection,
        o: &Observation,
        file: &str,
        state: &mut Context,
        cutoff: Option<&chrono::DateTime<Utc>>,
    ) -> Result<usize> {
        let kind = strv(&o.event, "type");
        if kind == "token_usage_record"
            || kind == "event_msg" && strv(&o.event["payload"], "type") == "token_count"
        {
            // A full historical baseline may complete a formerly unknown legacy
            // delta; native response identities still deduplicate independently.
            ingest(db, &o.source_id, file, &o.event, state, cutoff)
        } else {
            self.advance_local_context(o, state);
            Ok(0)
        }
    }
    fn prime_local_metadata(
        &self,
        db: &Connection,
        o: &Observation,
        file: &str,
        state: &mut Context,
    ) -> Result<()> {
        if strv(&o.event, "type") == "session_meta" {
            ingest(db, &o.source_id, file, &o.event, state, None)?;
        }
        Ok(())
    }
    fn identity_policy(&self) -> IdentityPolicy {
        IdentityPolicy {
            namespace: "codex-thread-response-v1".into(),
            scope: "source+original-thread+response".into(),
            revisions: false,
        }
    }
    fn build_chat_target(&self, thread: &str) -> Option<String> {
        uuid::Uuid::parse_str(thread)
            .ok()
            .filter(|id| id.hyphenated().to_string().eq_ignore_ascii_case(thread))
            .map(|id| format!("codex://threads/{}", id.hyphenated()))
    }
    fn capabilities(&self) -> Value {
        // Evidence describes the fixed verification corpus, not the version or
        // current RPC availability of whichever source registered this report.
        // Registration and rebuilds must never advance verifiedAt.
        let verified = |support: &str,
                        channel: &str,
                        granularity: &str,
                        prerequisites: &[&str],
                        evidence: &[&str],
                        scope: &str| {
            json!({
                "support": support,
                "runtime": null,
                "version": "0.147.0",
                "supportedVersion": "0.147.0",
                "testedVersion": "0.147.0",
                "channel": channel,
                "granularity": granularity,
                "prerequisites": prerequisites,
                "evidenceRefs": evidence,
                "verifiedAt": "2026-10-15",
                "verificationScope": scope,
                "versionPolicy": "仅上述版本及证据范围；其他版本未验证"
            })
        };
        let jsonl = [
            "来源已启用且 sessions / archived_sessions 中的 JSONL 可读",
            "字段与身份符合 Codex 0.147.0 已验证样本；当前来源版本未自动探测",
        ];
        let account = [
            "仅当前启用的 Codex 主账户来源；与 sourceId 精确绑定",
            "对应 Codex 可执行文件可用且已有登录；允许只读 app-server stdio",
            "以账户快照的状态、采集时间和窗口重置时间判断当前可用性",
        ];
        json!({
            "usage": verified("本机已验证", "jsonl", "response", &jsonl,
                &["docs/acceptance/ledger-replay-double-cursor-2026-10-15.json"],
                "六份脱敏原生日志与独立 oracle 对账；3004 个响应。不是所有版本或全部日志格式资格"),
            "cacheRead": verified("本机已验证", "jsonl", "response", &jsonl,
                &["docs/acceptance/ledger-replay-double-cursor-2026-10-15.json", "src-tauri/src/core/tests.rs#cache_ratio_uses_only_valid_subset_and_reports_input_coverage"],
                "缓存读取是输入子集；加权比例仅覆盖有效字段，不把缺失补零"),
            "cacheWrite": verified("仅文档支持", "jsonl", "response", &jsonl,
                &["docs/acceptance/ledger-replay-double-cursor-2026-10-15.json", "src-tauri/src/core/tests.rs#missing_zero_invalid_and_unknown_are_distinct", "src-tauri/src/core/tests.rs#fixture_native_and_cumulative_are_not_added_twice"],
                "本机原生字段及零值已观察；非零仅固定合成样本验证，未宣称真实非零写缓存场景通过"),
            "reasoning": verified("本机已验证", "jsonl", "response", &jsonl,
                &["docs/acceptance/ledger-replay-double-cursor-2026-10-15.json"],
                "推理输出是输出子集；未提供字段保留未知"),
            "turnCompletion": verified("本机已验证", "jsonl", "turn", &jsonl,
                &["docs/acceptance/work-care-2026-10-15.json", "src-tauri/src/core/tests.rs#completion_refinement_updates_one_record_and_preserves_read_and_delivery"],
                "本机字段探测及固定样本；无明确结果的 task_complete 只表示结果未知，不推定成功"),
            "turnTiming": verified("本机已验证", "jsonl", "turn", &jsonl,
                &["docs/acceptance/M0-环境与发布门槛.md", "tests/fixtures/codex-native.jsonl"],
                "仅有明确起止及计时字段的回合；回合总耗时不等于单次请求或纯模型生成耗时"),
            "requestTiming": {
                "support": "明确不支持", "runtime": "不可用",
                "supportedVersion": null, "testedVersion": "0.147.0",
                "channel": "jsonl", "granularity": "request",
                "prerequisites": ["需要可验证的逐请求边界及输出计时，当前适配器未提供"],
                "evidenceRefs": ["docs/acceptance/M0-环境与发布门槛.md"],
                "verifiedAt": "2026-10-15",
                "verificationScope": "当前 JSONL 适配器明确不提供逐请求生成速度；不能由回合耗时倒推",
                "versionPolicy": "本机 0.147.0 探测范围；未验证其他版本"
            },
            "subagents": {
                "support": "未知", "runtime": null,
                "supportedVersion": null, "testedVersion": null,
                "channel": "jsonl", "granularity": "thread-relationship",
                "prerequisites": ["来源已启用且 JSONL 可读", "来源明确提供可核对的父线程与子线程身份；未提供时不推断关系"],
                "evidenceRefs": ["src-tauri/src/core/tests.rs#child_inherits_parent_history_without_reassigning_parent_usage_or_notifying_child"],
                "verifiedAt": null,
                "verificationScope": "仅固定合成样本验证父子身份契约；尚无可核对的真实 Codex 0.147.0 父子关系验证收据，不宣称真实版本已验证",
                "versionPolicy": "真实子代理关系未验证任何版本"
            },
            "context": {
                "support": "未知", "runtime": "未配置",
                "supportedVersion": null, "testedVersion": null,
                "channel": null, "granularity": "context-snapshot",
                "prerequisites": ["需要来源提供已确认语义的上下文快照及容量；当前未配置采集通道"],
                "evidenceRefs": [], "verifiedAt": null,
                "verificationScope": "未验证；不将 token 使用量当作上下文占用",
                "versionPolicy": "未验证任何版本"
            },
            "chatTarget": {
                "support": "仅文档支持", "runtime": null,
                "supportedVersion": null, "testedVersion": null,
                "channel": "codex-uri", "granularity": "thread",
                "prerequisites": ["已登记来源中存在该会话且线程 ID 是规范 UUID", "操作系统已安装并注册 Codex 协议处理器"],
                "evidenceRefs": ["src-tauri/src/core/tests.rs#chat_target_requires_exact_source_membership_registered_adapter_and_plain_uuid"],
                "verifiedAt": null,
                "verificationScope": "已验证受限目标构造与来源隔离；外部 Codex 真实定位未验证，失败保留复制 ID 降级",
                "versionPolicy": "外部聊天定位未验证任何版本"
            },
            "quota": verified("本机已验证", "app-server", "quota-window", &account,
                &["docs/acceptance/live-account-bridge-2026-10-15.json", "docs/acceptance/account-bridge-contract-2026-10-15.json"],
                "2026-10-15 只读实测一桶一窗口及协议 mock；独立 account/rateLimits/read，不是 JSONL 用量或当前账户成功声明"),
            "accountUsage": verified("本机已验证", "app-server", "account-day", &account,
                &["docs/acceptance/live-account-bridge-2026-10-15.json", "docs/acceptance/account-bridge-contract-2026-10-15.json"],
                "2026-10-15 只读实测 77 个日桶；独立 account/usage/read，保留官方日期口径，不与本机日量相加")
        })
    }
    fn normalize(&self, o: &Observation) -> Result<Vec<StandardEvent>> {
        let v = &o.event;
        if strv(v, "type") != "token_usage_record" {
            return Ok(vec![]);
        }
        let p = v.get("payload").unwrap_or(&Value::Null);
        let u = p.get("usage").ok_or("顶层原生用量缺少 usage")?;
        let thread = strv(p, "thread_id");
        let response = strv(p, "response_id");
        if thread.is_empty() || response.is_empty() {
            return Err("原生响应缺少可信线程或响应身份".into());
        }
        let native_counts = read_native(
            u,
            &[
                "input_tokens",
                "output_tokens",
                "cached_input_tokens",
                "cache_write_input_tokens",
                "reasoning_output_tokens",
                "total_tokens",
            ],
        );
        let counts = codex_counts(&native_counts);
        let time = normalize_time(&strv(v, "timestamp")).ok_or("原生响应时间无效")?;
        Ok(vec![StandardEvent::ResponseUsage(ResponseUsage {
            source_id: o.source_id.clone(),
            channel_id: o.channel_id.clone(),
            namespace: self.identity_policy().namespace,
            thread_id: thread,
            response_id: response,
            turn_id: optional(p, "turn_id"),
            root_turn_id: optional(p, "root_turn_id"),
            event_time: time,
            collected_at: now(),
            requested_model: optional(p, "model").or_else(|| optional(p, "requested_model")),
            observed_model: optional(p, "observed_model"),
            provider_id: optional(p, "provider_id"),
            account_scope_id: None,
            native_counts,
            counts,
            semantics_version: SEMANTICS.into(),
            adapter_version: "codex-0.147-v1".into(),
            completeness: "source-recorded".into(),
            phase: "final".into(),
            revision: None,
            live: false,
        })])
    }
}
fn discover_codex(
    dir: &Path,
    result: &mut Discovery,
    recursive: bool,
    cancel: &std::sync::atomic::AtomicBool,
) {
    discover_codex_with(dir, result, recursive, cancel, &|path| fs::read_dir(path));
}
fn discover_codex_with<F>(
    dir: &Path,
    result: &mut Discovery,
    recursive: bool,
    cancel: &std::sync::atomic::AtomicBool,
    read_dir: &F,
) where
    F: Fn(&Path) -> std::io::Result<fs::ReadDir>,
{
    use std::sync::atomic::Ordering;
    if cancel.load(Ordering::Relaxed) {
        result.cancelled = true;
        return;
    }
    // The explicitly selected sessions directory can itself be a link.
    // Inspect its identity without following it before starting recursion.
    match fs::symlink_metadata(dir) {
        Ok(metadata) if metadata.file_type().is_symlink() => return,
        Ok(_) => {}
        Err(_) => {
            result.errors.push(dir.to_path_buf());
            return;
        }
    }
    let entries = match read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => {
            result.errors.push(dir.to_path_buf());
            return;
        }
    };
    for entry in entries {
        if cancel.load(Ordering::Relaxed) {
            result.cancelled = true;
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                result.errors.push(dir.to_path_buf());
                continue;
            }
        };
        let kind = match entry.file_type() {
            Ok(kind) => kind,
            Err(_) => {
                result.errors.push(entry.path());
                continue;
            }
        };
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            if recursive {
                discover_codex_with(&entry.path(), result, true, cancel, read_dir);
            }
        } else {
            let path = entry.path();
            match path.extension().and_then(|e| e.to_str()) {
                Some("jsonl") => result.files.push(path),
                Some("gz" | "zip" | "zst") => result.unsupported += 1,
                _ => {}
            }
        }
    }
    result.cancelled |= cancel.load(Ordering::Relaxed);
}

#[cfg(test)]
#[path = "discovery_reliability_tests.rs"]
mod discovery_reliability_tests;

#[cfg(test)]
mod capability_report_tests {
    use super::*;

    const FIELDS: [&str; 12] = [
        "usage",
        "cacheRead",
        "cacheWrite",
        "reasoning",
        "turnCompletion",
        "turnTiming",
        "requestTiming",
        "subagents",
        "context",
        "chatTarget",
        "quota",
        "accountUsage",
    ];

    #[test]
    fn capability_metadata_is_version_scoped_and_does_not_claim_live_rpc_success() {
        let report = CodexAdapter.capabilities();
        for key in FIELDS {
            let field = &report[key];
            for property in [
                "support",
                "runtime",
                "supportedVersion",
                "testedVersion",
                "channel",
                "granularity",
                "prerequisites",
                "evidenceRefs",
                "verifiedAt",
                "verificationScope",
                "versionPolicy",
            ] {
                assert!(
                    field.get(property).is_some(),
                    "{key}.{property} is required"
                );
            }
            assert!(!field["prerequisites"].as_array().unwrap().is_empty());
            assert!(field["evidenceRefs"].is_array());
            for reference in field["evidenceRefs"].as_array().unwrap() {
                let (file, fragment) = reference
                    .as_str()
                    .unwrap()
                    .split_once('#')
                    .map_or((reference.as_str().unwrap(), None), |(file, name)| {
                        (file, Some(name))
                    });
                let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .unwrap()
                    .join(file);
                assert!(path.is_file(), "{key}: missing evidence {file}");
                if let Some(name) = fragment {
                    assert!(
                        fs::read_to_string(&path)
                            .unwrap()
                            .contains(&format!("fn {name}(")),
                        "{key}: missing case {name}"
                    );
                }
            }
            assert!(field["versionPolicy"].as_str().unwrap().contains("未验证"));
            if !field["verifiedAt"].is_null() {
                assert_eq!(field["verifiedAt"], "2026-10-15");
                assert!(!field["evidenceRefs"].as_array().unwrap().is_empty());
            }
            if !field["supportedVersion"].is_null() {
                assert_eq!(field["supportedVersion"], "0.147.0");
                assert_eq!(field["testedVersion"], "0.147.0");
            }
        }
        assert_eq!(report["requestTiming"]["runtime"], "不可用");
        assert_eq!(report["context"]["runtime"], "未配置");
        assert!(report["context"]["verifiedAt"].is_null());
        assert!(report["chatTarget"]["verifiedAt"].is_null());
        assert_eq!(report["subagents"]["support"], "未知");
        assert!(report["subagents"]["supportedVersion"].is_null());
        assert!(report["subagents"]["testedVersion"].is_null());
        assert!(report["subagents"]["verifiedAt"].is_null());
        for key in ["quota", "accountUsage"] {
            assert_eq!(report[key]["channel"], "app-server");
            assert!(report[key]["runtime"].is_null());
        }
        assert_eq!(
            report,
            CodexAdapter.capabilities(),
            "registration must not advance verification dates"
        );
    }

    #[test]
    fn registered_source_snapshot_preserves_declaration_and_runtime_boundaries() {
        let fixture = tempfile::tempdir().unwrap();
        let source = fixture.path().join("source");
        fs::create_dir(&source).unwrap();
        let mut core = Core::open(&fixture.path().join("data"), &source).unwrap();
        // A directory alone has unknown health; complete discovery before asserting availability.
        core.scan().unwrap();
        let snapshot = core.snapshot(json!({})).unwrap();
        let report = snapshot["sourceCapabilities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["sourceId"] == core.source_id)
            .unwrap();
        assert_eq!(report["usage"]["supportedVersion"], "0.147.0");
        assert!(report["usage"]["runtime"].is_null());
        assert_eq!(report["requestTiming"]["runtime"], "不可用");
        assert_eq!(report["requestTiming"]["sourceRuntime"], "可用");
        assert_eq!(report["context"]["runtime"], "未配置");
        assert_eq!(report["quota"]["channel"], "app-server");
        assert!(report["quota"]["runtime"].is_null());
        assert_eq!(report["quota"]["verifiedAt"], "2026-10-15");
        core.shutdown().unwrap();
        let mut reopened = Core::open(&fixture.path().join("data"), &source).unwrap();
        let again = reopened.snapshot(json!({})).unwrap();
        let updated = again["sourceCapabilities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["sourceId"] == reopened.source_id)
            .unwrap();
        assert_eq!(
            report["quota"]["verifiedAt"],
            updated["quota"]["verifiedAt"]
        );
        reopened.shutdown().unwrap();
    }
}
#[cfg(test)]
mod lifecycle_order_regression_tests {
    use super::*;

    const THREAD: &str = "65000000-0000-4000-8000-000000000001";
    const TURN: &str = "arrival-order-root";
    const T0: &str = "2026-10-15T00:00:00.000Z";
    const T2: &str = "2026-10-15T00:00:02.000Z";
    const T3: &str = "2026-10-15T00:00:03.000Z";

    fn observed_order(terminal: &str, usage_first: bool, generic_after: bool) -> Value {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        fs::create_dir(&source).unwrap();
        let mut core = Core::open(&tmp.path().join("data"), &source).unwrap();
        let source_id = core.source_id.clone();
        let mut context = Context::default();
        let event =
            |time, kind, payload: Value| json!({"timestamp":time,"type":kind,"payload":payload});
        let meta = event(
            T0,
            "session_meta",
            json!({"id":THREAD,"cli_version":"0.147.0"}),
        );
        let start = event(
            T0,
            "event_msg",
            json!({"type":"task_started","thread_id":THREAD,"turn_id":TURN,"root_turn_id":TURN}),
        );
        let usage = event(
            T3,
            "token_usage_record",
            json!({"thread_id":THREAD,"response_id":"arrival-order-response","turn_id":TURN,"root_turn_id":TURN,"model":"synthetic-model","usage":{"input_tokens":100,"output_tokens":20,"cached_input_tokens":0,"cache_write_input_tokens":0,"reasoning_output_tokens":0,"total_tokens":120}}),
        );
        let end = event(
            T2,
            "event_msg",
            if terminal == "failed" {
                json!({"type":"turn_failed","thread_id":THREAD,"turn_id":TURN,"root_turn_id":TURN,"duration_ms":2000})
            } else {
                json!({"type":"task_complete","thread_id":THREAD,"turn_id":TURN,"root_turn_id":TURN,"status":"completed","duration_ms":2000})
            },
        );
        // The generic observation describes that same t2 end and adds no result
        // evidence. It must never erase a known failed outcome.
        let generic = event(
            T2,
            "event_msg",
            json!({"type":"task_complete","thread_id":THREAD,"turn_id":TURN,"root_turn_id":TURN}),
        );
        let mut arrivals = vec![meta, start];
        if usage_first {
            arrivals.extend([usage, end]);
        } else {
            arrivals.extend([end, usage]);
        }
        if generic_after {
            arrivals.push(generic);
        }
        for record in &arrivals {
            // Use the real adapter entry, not SQL-created turn or usage facts.
            ingest(
                &core.db,
                &source_id,
                "synthetic-arrival-order",
                record,
                &mut context,
                None,
            )
            .unwrap();
        }
        let fact = load_fact(&core.db, "turns", &format!("{source_id}|{TURN}"))
            .unwrap()
            .unwrap();
        let snapshot = core.snapshot(json!({})).unwrap();
        let inbox = snapshot["inbox"].as_array().unwrap();
        let result = json!({
            "terminal":terminal,"usageFirst":usage_first,"genericAfter":generic_after,
            "arrivals":arrivals,
            "turn":{"status":fact["status"],"startedAt":fact["startedAt"],"completedAt":fact["completedAt"],"lastEventAt":fact["lastEventAt"],"eventTime":fact["eventTime"]},
            "inboxStatuses":inbox.iter().map(|item|item["status"].clone()).collect::<Vec<_>>(),
            "activity":snapshot["activity"]
        });
        core.shutdown().unwrap();
        result
    }

    #[test]
    fn explicit_terminal_result_is_order_independent_of_later_usage_metadata() {
        let observations = [
            observed_order("completed", false, false),
            observed_order("completed", true, false),
            observed_order("failed", false, true),
            observed_order("failed", true, true),
        ];
        // Gather every case before asserting so the original failure receipt
        // preserves both legal arrival orders and the failed/unknown guard.
        eprintln!("arrival-order-observations={}", json!(observations));
        let actual: Vec<Value> = observations
            .iter()
            .map(|value| {
                json!({
                    "turn":value["turn"], "inboxStatuses":value["inboxStatuses"]
                })
            })
            .collect();
        let expected = ["completed", "completed", "failed", "failed"].map(|status| json!({
            "turn":{"status":status,"startedAt":T0,"completedAt":T2,"lastEventAt":T3,"eventTime":T3},
            "inboxStatuses":[status]
        }));
        assert_eq!(actual, expected, "same legal thread/turn and event times must produce one consistent terminal fact independent of arrival order; generic unknown cannot overwrite known failure");
    }

    #[test]
    fn newer_known_result_ignores_older_explicit_and_later_generic_without_refreshing_delivery() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        fs::create_dir(&source).unwrap();
        let mut core = Core::open(&tmp.path().join("data"), &source).unwrap();
        let mut context = Context::default();
        let event =
            |time, kind, payload: Value| json!({"timestamp":time,"type":kind,"payload":payload});
        let t4 = "2026-10-15T00:00:04.000Z";
        let t5 = "2026-10-15T00:00:05.000Z";
        let initial = [
            event(T0, "session_meta", json!({"id":THREAD})),
            event(
                T0,
                "event_msg",
                json!({"type":"task_started","thread_id":THREAD,"turn_id":TURN,"root_turn_id":TURN}),
            ),
            event(
                T3,
                "event_msg",
                json!({"type":"turn_failed","thread_id":THREAD,"turn_id":TURN,"root_turn_id":TURN,"duration_ms":3000}),
            ),
            event(
                t4,
                "token_usage_record",
                json!({"thread_id":THREAD,"response_id":"known-newer-response","turn_id":TURN,"root_turn_id":TURN,"usage":{"input_tokens":100,"output_tokens":20,"cached_input_tokens":0,"cache_write_input_tokens":0,"reasoning_output_tokens":0,"total_tokens":120}}),
            ),
        ];
        for record in &initial {
            ingest(
                &core.db,
                &core.source_id,
                "synthetic-known-newer",
                record,
                &mut context,
                None,
            )
            .unwrap();
        }
        let before = core.snapshot(json!({})).unwrap();
        let id = before["inbox"][0]["id"].as_str().unwrap().to_owned();
        core.action("mark_read", &json!({"id":id})).unwrap();
        core.action("acknowledge_delivery", &json!({"id":id}))
            .unwrap();
        for record in [
            event(
                T2,
                "event_msg",
                json!({"type":"task_complete","thread_id":THREAD,"turn_id":TURN,"root_turn_id":TURN,"status":"completed","duration_ms":2000,"time_to_first_token_ms":999}),
            ),
            event(
                t5,
                "event_msg",
                json!({"type":"task_complete","thread_id":THREAD,"turn_id":TURN,"root_turn_id":TURN,"duration_ms":5000,"time_to_first_token_ms":888}),
            ),
        ] {
            ingest(
                &core.db,
                &core.source_id,
                "synthetic-known-newer",
                &record,
                &mut context,
                None,
            )
            .unwrap();
        }
        let fact = load_fact(&core.db, "turns", &format!("{}|{TURN}", core.source_id))
            .unwrap()
            .unwrap();
        assert_eq!(fact["status"], "failed");
        assert_eq!(fact["completedAt"], T3);
        assert_eq!(fact["lifecycleEventAt"], T3);
        assert_eq!(fact["resultEvidence"], "explicit-terminal");
        assert_eq!(fact["lastEventAt"], t5);
        assert_eq!(fact["eventTime"], t5);
        assert_eq!(fact["durationMs"], 3000);
        assert!(
            fact["timeToFirstTokenMs"].is_null(),
            "ignored results cannot supply another result's timing"
        );
        let after = core.snapshot(json!({})).unwrap();
        assert_eq!(after["inbox"].as_array().unwrap().len(), 1);
        let reminder = &after["inbox"][0];
        assert_eq!(reminder["id"], id);
        assert_eq!(reminder["status"], "failed");
        assert_eq!(reminder["createdAt"], T3);
        assert_eq!(reminder["read"], true);
        assert_eq!(reminder["animationAcknowledged"], true);
        for field in [
            "notifyEligible",
            "notified",
            "notificationState",
            "eligibleRuntime",
        ] {
            assert_eq!(
                reminder[field], before["inbox"][0][field],
                "delivery field {field}"
            );
        }
        core.shutdown().unwrap();
    }
}

fn optional(v: &Value, k: &str) -> Option<String> {
    v.get(k)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}
pub fn normalize_time(s: &str) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|t| {
        t.with_timezone(&Utc)
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    })
}
fn eligible_time(time: &str, start: &str) -> bool {
    match (
        chrono::DateTime::parse_from_rfc3339(time),
        chrono::DateTime::parse_from_rfc3339(start),
    ) {
        (Ok(time), Ok(start)) => {
            time >= start && time.with_timezone(&Utc) <= crate::budget_clock::now().utc + Duration::seconds(5)
        }
        _ => false,
    }
}
fn read_native(v: &Value, keys: &[&str]) -> BTreeMap<String, Metric> {
    keys.iter()
        .map(|k| {
            (
                (*k).into(),
                match v.get(k) {
                    None | Some(Value::Null) => Metric::Missing,
                    Some(n) => match n.as_i64() {
                        Some(n) if n >= 0 => Metric::Valid(n),
                        _ => Metric::Invalid,
                    },
                },
            )
        })
        .collect()
}
fn metric(m: &BTreeMap<String, Metric>, k: &str) -> Metric {
    m.get(k).cloned().unwrap_or(Metric::Missing)
}
fn codex_counts(m: &BTreeMap<String, Metric>) -> Counts {
    let i = metric(m, "input_tokens");
    let o = metric(m, "output_tokens");
    let mut c = metric(m, "cached_input_tokens");
    let mut w = metric(m, "cache_write_input_tokens");
    let mut r = metric(m, "reasoning_output_tokens");
    if let (Some(cn), Some(n)) = (c.value(), i.value()) {
        if cn > n {
            c = Metric::Invalid;
        }
    }
    if let (Some(wn), Some(n)) = (w.value(), i.value()) {
        if wn > n {
            w = Metric::Invalid;
        }
    }
    if let (Some(rn), Some(n)) = (r.value(), o.value()) {
        if rn > n {
            r = Metric::Invalid;
        }
    }
    let total = match (i.value(), o.value()) {
        (Some(i), Some(o)) => match i.checked_add(o) {
            Some(n) => {
                let supplied = metric(m, "total_tokens");
                if supplied
                    .value()
                    .map(|v| v != n)
                    .unwrap_or(matches!(supplied, Metric::Invalid))
                {
                    Metric::Invalid
                } else {
                    Metric::Valid(n)
                }
            }
            None => Metric::Invalid,
        },
        _ => Metric::Missing,
    };
    Counts {
        input: i,
        output: o,
        cache_read: c,
        cache_write: w,
        reasoning: r,
        total,
    }
}
fn canonical_digest(r: &ResponseUsage) -> String {
    hash(
        json!({"native":r.native_counts,"counts":r.counts,"semantics":r.semantics_version})
            .to_string()
            .as_bytes(),
    )
}
pub fn response_key(r: &ResponseUsage) -> String {
    hash(
        format!(
            "{}|{}|{}|{}",
            r.source_id, r.namespace, r.thread_id, r.response_id
        )
        .as_bytes(),
    )
}

pub(super) fn store_standard_event(
    db: &Connection,
    event: &StandardEvent,
    allow_revision: bool,
) -> Result<usize> {
    match event {
        StandardEvent::ResponseUsage(r) => store_response(db, r, allow_revision),
        StandardEvent::UsageAggregate(a) => {
            db.execute("INSERT OR IGNORE INTO usage_aggregates(id,source_id,event_time,data) VALUES(?1,?2,?3,?4)",params![a.id,a.source_id,a.interval_end,serde_json::to_string(a).map_err(err)?]).map_err(err)?;
            Ok(0)
        }
        StandardEvent::ContextSnapshot(s) => {
            db.execute("INSERT OR IGNORE INTO context_snapshots(id,source_id,event_time,data) VALUES(?1,?2,?3,?4)",params![s.id,s.source_id,s.observed_at,serde_json::to_string(s).map_err(err)?]).map_err(err)?;
            Ok(0)
        }
        StandardEvent::QuotaSnapshot(s) => {
            db.execute(
                "INSERT INTO account_snapshots(collected_at,data) VALUES(?1,?2)",
                params![s.observed_at, serde_json::to_string(s).map_err(err)?],
            )
            .map_err(err)?;
            Ok(0)
        }
        StandardEvent::SourceHealth(h) => {
            put_setting(
                db,
                &format!("sourceHealth:{}:{}", h.source_id, h.channel_id),
                &serde_json::to_value(h).map_err(err)?,
            )?;
            Ok(0)
        }
        StandardEvent::TurnLifecycle(t) | StandardEvent::ToolLifecycle(t) => {
            if t.thread_id.is_none() {
                return Err("生命周期缺少已验证会话身份，不伪造关系".into());
            }
            let mut v = serde_json::to_value(t).map_err(err)?;
            v["id"] = json!(t.native_id);
            let id = format!("{}|{}|{}", t.source_id, t.namespace, t.native_id);
            match event {
                StandardEvent::TurnLifecycle(_) => save_turn(db, &id, &v, &t.event_time)?,
                _ => save_item(db, &id, &v, &t.event_time)?,
            }
            Ok(0)
        }
    }
}
pub(super) fn mark_suspected_overlap(
    db: &Connection,
    facts: &[String],
    evidence: &str,
) -> Result<()> {
    if facts.len() < 2 || evidence.is_empty() {
        return Err("重叠疑点需要事实范围与证据引用".into());
    }
    for id in facts {
        db.execute("INSERT OR IGNORE INTO excluded_facts(fact_id,reason) VALUES(?1,'unresolved-cross-source-overlap')",[id]).map_err(err)?;
    }
    let id = hash(format!("overlap|{}", facts.join("|")).as_bytes());
    db.execute("INSERT OR IGNORE INTO identity_conflicts(id,kind,event_time,details) VALUES(?1,'cross_source_suspected_overlap',?2,?3)",params![id,now(),json!({"factIds":facts,"evidence":evidence}).to_string()]).map_err(err)?;
    Ok(())
}
pub(super) fn merge_verified_copy(
    db: &Connection,
    canonical: &str,
    duplicate: &str,
    evidence: &str,
) -> Result<()> {
    if evidence.is_empty() {
        return Err("合并需要已验证的原始身份与复制关系证据".into());
    }
    let a: ResponseUsage =
        serde_json::from_value(load_fact(db, "usage_responses", canonical)?.ok_or("原事实不存在")?)
            .map_err(err)?;
    let b: ResponseUsage = serde_json::from_value(
        load_fact(db, "usage_responses", duplicate)?.ok_or("复制事实不存在")?,
    )
    .map_err(err)?;
    if a.namespace != b.namespace
        || a.thread_id != b.thread_id
        || a.response_id != b.response_id
        || canonical_digest(&a) != canonical_digest(&b)
    {
        return Err("身份及计量证据不一致，禁止合并".into());
    }
    db.execute("INSERT INTO fact_aliases(alias,fact_id,evidence) VALUES(?1,?2,?3) ON CONFLICT(alias) DO UPDATE SET fact_id=excluded.fact_id,evidence=excluded.evidence",params![duplicate,canonical,evidence]).map_err(err)?;
    db.execute("DELETE FROM excluded_facts WHERE (fact_id=?1 OR fact_id=?2) AND reason='unresolved-cross-source-overlap'",params![canonical,duplicate]).map_err(err)?;
    db.execute("UPDATE identity_conflicts SET details=json_set(details,'$.resolved',json('true')) WHERE kind='cross_source_suspected_overlap' AND EXISTS(SELECT 1 FROM json_each(details,'$.factIds') WHERE value=?1) AND EXISTS(SELECT 1 FROM json_each(details,'$.factIds') WHERE value=?2)",params![canonical,duplicate]).map_err(err)?;
    Ok(())
}

/// The identity digest excludes late model/account metadata. Revisions require adapter policy plus sequence evidence.
pub(super) fn store_response(
    db: &Connection,
    r: &ResponseUsage,
    allow_revision: bool,
) -> Result<usize> {
    let key = response_key(r);
    let digest = canonical_digest(r);
    let obs = hash(format!("{}|{}|{}", r.channel_id, key, digest).as_bytes());
    db.execute("INSERT OR IGNORE INTO fact_observations(id,fact_id,source_id,channel_id,event_time,digest) VALUES(?1,?2,?3,?4,?5,?6)",params![obs,key,r.source_id,r.channel_id,r.event_time,digest]).map_err(err)?;
    let old: Option<(String, String)> = db
        .query_row(
            "SELECT digest,data FROM usage_responses WHERE id=?1",
            [&key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(err)?;
    if let Some((old_digest, old_data)) = old {
        if old_digest != digest {
            let old: ResponseUsage = serde_json::from_str(&old_data).map_err(err)?;
            let permitted = allow_revision
                && r.revision
                    .zip(old.revision)
                    .map(|(n, o)| n > o)
                    .unwrap_or(false)
                && !(old.phase == "final" && r.phase != "final");
            if permitted {
                db.execute(
                    "UPDATE usage_responses SET digest=?1,data=?2,event_time=?3 WHERE id=?4",
                    params![
                        digest,
                        serde_json::to_string(r).map_err(err)?,
                        r.event_time,
                        key
                    ],
                )
                .map_err(err)?;
            } else if allow_revision
                && r.revision
                    .zip(old.revision)
                    .map(|(n, o)| n < o)
                    .unwrap_or(false)
            {
                return Ok(0);
            } else {
                let conflict = hash(format!("{key}|{digest}").as_bytes());
                db.execute("INSERT OR IGNORE INTO identity_conflicts(id,kind,event_time,details) VALUES(?1,'same_identity_different_counts',?2,?3)",params![conflict,now(),json!({"factId":key,"oldDigest":old_digest,"newDigest":digest,"sourceId":r.source_id}).to_string()]).map_err(err)?;
                db.execute("INSERT OR IGNORE INTO excluded_facts(fact_id,reason) VALUES(?1,'unresolved-metering-conflict')",[&key]).map_err(err)?;
            }
        } else {
            let mut old: ResponseUsage = serde_json::from_str(&old_data).map_err(err)?;
            if old.requested_model.is_none() {
                old.requested_model = r.requested_model.clone();
            }
            if old.observed_model.is_none() {
                old.observed_model = r.observed_model.clone();
            }
            db.execute(
                "UPDATE usage_responses SET data=?1 WHERE id=?2",
                params![serde_json::to_string(&old).map_err(err)?, key],
            )
            .map_err(err)?;
        }
        return Ok(0);
    }
    db.execute("INSERT INTO usage_responses(id,source_id,thread_id,response_id,turn_id,event_time,digest,data) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![key,r.source_id,r.thread_id,r.response_id,r.turn_id,r.event_time,digest,serde_json::to_string(r).map_err(err)?]).map_err(err)?;
    // The complete original Codex UUID thread + response pair is copying evidence to investigate,
    // never proof permitting a merge. Independent tools and short-ID domains remain independent.
    if r.namespace == "codex-thread-response-v1" && uuid::Uuid::parse_str(&r.thread_id).is_ok() {
        let mut stmt=db.prepare("SELECT id,data FROM usage_responses WHERE source_id<>?1 AND thread_id=?2 AND response_id=?3 AND NOT EXISTS(SELECT 1 FROM fact_aliases WHERE alias=usage_responses.id)").map_err(err)?;
        let rows = stmt
            .query_map(params![r.source_id, r.thread_id, r.response_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(err)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(err)?;
        drop(stmt);
        for (other, data) in rows {
            let previous: ResponseUsage = serde_json::from_str(&data).map_err(err)?;
            if previous.namespace == r.namespace {
                let mut ids = vec![key.clone(), other];
                ids.sort();
                mark_suspected_overlap(db,&ids,"matching original Codex UUID thread and response across roots; copy relation unresolved")?;
            }
        }
    }
    Ok(1)
}
pub(super) fn ingest(
    db: &Connection,
    source: &str,
    file: &str,
    v: &Value,
    ctx: &mut Context,
    cutoff: Option<&chrono::DateTime<Utc>>,
) -> Result<usize> {
    let kind = strv(v, "type");
    let p = v.get("payload").unwrap_or(&Value::Null);
    let Some(time) = normalize_time(&strv(v, "timestamp")) else {
        if [
            "session_meta",
            "turn_context",
            "event_msg",
            "response_item",
            "token_usage_record",
        ]
        .contains(&kind.as_str())
        {
            issue(
                db,
                "invalid_event_time",
                file,
                &hash(v.to_string().as_bytes()),
                "事件缺少有效 UTC 时间，未用采集时间伪造来源事实",
            )?;
        }
        return Ok(0);
    };
    if normalize_time(&strv(v, "timestamp")).is_some() {
        let watermark = format!("sourceLastEvent:{source}");
        let previous = get_setting(db, &watermark)?.and_then(|v| v.as_str().map(str::to_owned));
        if previous.as_ref().map(|p| time > *p).unwrap_or(true) {
            put_setting(db, &watermark, &json!(time))?;
        }
    }
    let within = cutoff
        .map(|c| {
            chrono::DateTime::parse_from_rfc3339(&time)
                .map(|t| t.with_timezone(&Utc) >= *c)
                .unwrap_or(false)
        })
        .unwrap_or(true);
    if kind == "session_meta" {
        let id = strv(p, "id");
        if id.is_empty() {
            return Ok(0);
        }
        if ctx.owner.is_empty() {
            ctx.owner = id.clone();
            ctx.own_start = p
                .get("subagent_history_start_ordinal")
                .and_then(Value::as_u64);
            ctx.parent = strv(p, "parent_thread_id");
        }
        ctx.active_thread = id.clone();
        let key = format!("{source}|{id}");
        let mut data = load_fact(db, "thread_metadata", &key)?.unwrap_or_else(|| json!({"id":id,"sourceId":source,"title":format!("会话 {}",id.chars().take(8).collect::<String>()),"cwd":"","parentThreadId":Value::Null,"rootThreadId":Value::Null,"model":Value::Null,"accountScopeId":Value::Null}));
        if let Some(cwd) = p
            .get("cwd")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            data["cwd"] = json!(cwd);
        }
        super::agent_kind::merge(
            &mut data,
            p,
            "jsonl-session-meta",
            p.get("cli_version").and_then(Value::as_str),
        );
        db.execute(
            "INSERT INTO thread_metadata(id,data) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET data=excluded.data",
            params![key, data.to_string()],
        )
        .map_err(err)?;
        return Ok(0);
    }
    let mut thread = ctx.thread(v);
    let explicit = optional(p, "thread_id");
    if let Some(t) = explicit {
        thread = t;
    }
    if kind == "turn_context" {
        let turn = strv(p, "turn_id");
        if turn.is_empty() {
            return Ok(0);
        }
        ctx.current_turn = turn.clone();
        ctx.model = strv(p, "model");
        let id = format!("{source}|{turn}");
        let mut t=load_fact(db,"turns",&id)?.unwrap_or_else(||json!({"id":turn,"sourceId":source,"threadId":thread,"rootTurnId":optional(p,"root_turn_id"),"status":"unknown","startedAt":Value::Null,"completedAt":Value::Null,"durationMs":Value::Null,"timeToFirstTokenMs":Value::Null}));
        if !ctx.model.is_empty() {
            t["model"] = json!(ctx.model);
            t["modelSource"] = json!("turn_context");
        }
        t["lastEventAt"] = json!(time);
        save_turn(db, &id, &t, &time)?;
        return Ok(0);
    }
    if kind == "token_usage_record" {
        if !within {
            return Ok(0);
        }
        let o = Observation {
            source_id: source.into(),
            channel_id: format!("{source}:jsonl"),
            event: v.clone(),
        };
        match CodexAdapter.normalize(&o) {
            Ok(events) => {
                let mut count = 0;
                for e in events {
                    if let StandardEvent::ResponseUsage(mut r) = e {
                        r.live = ctx.notify_eligible && eligible_time(&time, &ctx.runtime_start);
                        if r.requested_model.is_none() {
                            if let Some(turn) = &r.turn_id {
                                r.requested_model =
                                    load_fact(db, "turns", &format!("{source}|{turn}"))?
                                        .and_then(|t| optional(&t, "model"));
                            }
                            if r.requested_model.is_none()
                                && r.turn_id.as_deref() == Some(ctx.current_turn.as_str())
                                && !ctx.model.is_empty()
                            {
                                r.requested_model = Some(ctx.model.clone());
                            }
                        }
                        let data = json!({"id":r.thread_id,"sourceId":source,"title":format!("会话 {}",r.thread_id.chars().take(8).collect::<String>()),"cwd":"","parentThreadId":Value::Null,"rootThreadId":Value::Null,"model":Value::Null});
                        db.execute(
                            "INSERT OR IGNORE INTO thread_metadata(id,data) VALUES(?1,?2)",
                            params![format!("{source}|{}", r.thread_id), data.to_string()],
                        )
                        .map_err(err)?;
                        count += store_standard_event(
                            db,
                            &StandardEvent::ResponseUsage(r.clone()),
                            CodexAdapter.allows_revisions(),
                        )?;
                        // Ownership is evaluated after this native fact is stored,
                        // within the reader's same transaction. A late response
                        // can refine an existing turn/inbox without replaying it.
                        if let Some(turn) = &r.turn_id {
                            let id = format!("{source}|{turn}");
                            if let Some(mut t) = load_fact(db, "turns", &id)? {
                                t["lastEventAt"] = json!(time);
                                save_turn(db, &id, &t, &time)?;
                            }
                        }
                        if r.counts.total.value().is_none()
                            || [
                                &r.counts.cache_read,
                                &r.counts.cache_write,
                                &r.counts.reasoning,
                            ]
                            .iter()
                            .any(|m| matches!(m, Metric::Invalid))
                        {
                            issue(
                                db,
                                "metric_contract",
                                file,
                                &canonical_digest(&r),
                                "计数字段不满足计量契约，相关派生值已关闭",
                            )?;
                        }
                    }
                }
                return Ok(count);
            }
            Err(_) => {
                issue(
                    db,
                    "unsupported_native_usage",
                    file,
                    &hash(v.to_string().as_bytes()),
                    "原生响应身份、时间或字段缺失；未进入精确账本",
                )?;
                return Ok(0);
            }
        }
    }
    if kind == "event_msg" {
        let event = strv(p, "type");
        if event == "token_count" {
            legacy(db, source, file, p, &thread, &time, ctx, within)?;
            return Ok(0);
        }
        if [
            "task_started",
            "task_complete",
            "turn_completed",
            "turn_aborted",
            "turn_failed",
        ]
        .contains(&event.as_str())
        {
            let turn = strv(p, "turn_id");
            if turn.is_empty() {
                issue(
                    db,
                    "missing_turn_id",
                    file,
                    "",
                    "回合事件没有原生回合身份，未伪造关系",
                )?;
                return Ok(0);
            }
            let attribution = p.get("turn_attribution").unwrap_or(&Value::Null);
            if !ctx.parent.is_empty()
                && attribution
                    .get("parent_turn_id")
                    .is_some_and(|v| !v.is_null())
                && ctx.own_start.is_none()
            {
                thread = ctx.owner.clone();
            }
            // Row order is not attribution evidence. An index can change the
            // first row for a turn that has contradictory thread ownership.
            let owners = native_turn_owners(db, source, &turn)?;
            if owners.len() > 1 {
                issue(
                    db,
                    "ambiguous_turn_thread",
                    "",
                    &hash(format!("{source}|{turn}").as_bytes()),
                    "同一来源回合关联了多个线程，未猜测归属或生成完成提醒",
                )?;
                if let Some(existing) = load_fact(db, "turns", &format!("{source}|{turn}"))? {
                    save_turn(
                        db,
                        &format!("{source}|{turn}"),
                        &existing,
                        &strv(&existing, "eventTime"),
                    )?;
                }
                ctx.current_turn.clear();
                return Ok(0);
            }
            if let Some(t) = owners.into_iter().next() {
                thread = t;
            }
            ctx.current_turn = turn.clone();
            let id = format!("{source}|{turn}");
            let mut t=load_fact(db,"turns",&id)?.unwrap_or_else(||json!({"id":turn,"sourceId":source,"threadId":thread,"status":"unknown","startedAt":Value::Null,"completedAt":Value::Null,"durationMs":Value::Null,"timeToFirstTokenMs":Value::Null,"model":Value::Null}));
            t["rootTurnId"] = json!(
                optional(p, "root_turn_id").or_else(|| optional(attribution, "root_turn_id"))
            );
            t["parentTurnId"] =
                json!(optional(attribution, "parent_turn_id")
                    .or_else(|| optional(&t, "parentTurnId")));
            t["lastEventAt"] = json!(time);
            if event == "task_started" {
                if t["completedAt"].is_null() {
                    t["status"] = json!("active");
                }
                if t["startedAt"].is_null() {
                    t["startedAt"] = json!(optional(p, "started_at")
                        .and_then(|s| normalize_time(&s))
                        .unwrap_or_else(|| time.clone()));
                }
            } else if within {
                let status = if event == "turn_aborted" {
                    "interrupted"
                } else if event == "turn_failed" {
                    "failed"
                } else {
                    match p.get("status").and_then(Value::as_str) {
                        Some("completed") | Some("success") => "completed",
                        Some("failed") => "failed",
                        Some("interrupted") | Some("cancelled") => "interrupted",
                        _ => "unknown",
                    }
                };
                // Only lifecycle records supply result evidence. Metadata may
                // have a later global eventTime without carrying a later result.
                t["status"] = json!(status);
                t["lifecycleEventAt"] = json!(time);
                t["resultEvidence"] = json!(if status == "unknown" {
                    "unknown-end"
                } else {
                    "explicit-terminal"
                });
                t["completedAt"] = json!(optional(p, "completed_at")
                    .and_then(|s| normalize_time(&s))
                    .unwrap_or_else(|| time.clone()));
                if let Some(n) = p
                    .get("duration_ms")
                    .and_then(Value::as_i64)
                    .filter(|n| *n >= 0)
                {
                    t["durationMs"] = json!(n);
                }
                if let Some(n) = p
                    .get("time_to_first_token_ms")
                    .and_then(Value::as_i64)
                    .filter(|n| *n >= 0)
                {
                    t["timeToFirstTokenMs"] = json!(n);
                }
            }
            save_turn(db, &id, &t, &time)?;
            if event != "task_started" && within {
                let accepted = load_fact(db, "turns", &id)?.ok_or("保存后的回合事实缺失")?;
                let result_time = turn_result_time(&accepted);
                // An ignored generic/older result cannot re-publish an old
                // terminal as a fresh completion. Use the accepted result clock.
                let incoming_accepted = result_time.as_deref() == Some(time.as_str())
                    && accepted["status"] == t["status"];
                let root = accepted["rootTurnId"].as_str().unwrap_or(&turn);
                if incoming_accepted && root == turn {
                    let result_time = result_time.as_deref().unwrap();
                    completion(
                        db,
                        source,
                        &accepted,
                        result_time,
                        ctx.notify_eligible && eligible_time(result_time, &ctx.runtime_start),
                        &ctx.runtime_id,
                    )?;
                }
            }
            return Ok(0);
        }
        if [
            "exec_command_begin",
            "exec_command_end",
            "mcp_tool_call_begin",
            "mcp_tool_call_end",
            "tool_call_begin",
            "tool_call_end",
        ]
        .contains(&event.as_str())
            && within
        {
            tool_event(db, source, p, &thread, &ctx.current_turn, &time)?;
            if let Some(mut t) = load_fact(db, "turns", &format!("{source}|{}", ctx.current_turn))?
            {
                t["lastEventAt"] = json!(time);
                save_turn(db, &format!("{source}|{}", ctx.current_turn), &t, &time)?;
            }
        }
    }
    if kind == "response_item" && within {
        let event = strv(p, "type");
        if event == "function_call" || event == "custom_tool_call" {
            let call = strv(p, "call_id");
            if !call.is_empty() {
                let name = safe_name(&strv(p, "name"));
                let data = json!({"id":call,"sourceId":source,"threadId":thread,"turnId":if ctx.current_turn.is_empty(){None}else{Some(&ctx.current_turn)},"name":name,"kind":if name.starts_with("mcp__"){"mcp"}else{"function"},"status":"started","exitCode":Value::Null,"isError":Value::Null,"durationMs":Value::Null,"startedAt":time,"eventTime":time});
                save_item(db, &format!("{source}|{thread}|{call}"), &data, &time)?;
            }
        } else if event == "function_call_output" || event == "custom_tool_call_output" {
            let call = strv(p, "call_id");
            if call.is_empty() || thread.is_empty() {
                return Ok(0);
            }
            let id = format!("{source}|{thread}|{call}");
            // Only this event's recorded fields are incoming evidence. An
            // output may precede its call; a later call can fill the name.
            let mut data = json!({"id":call,"sourceId":source,"threadId":thread,"turnId":if ctx.current_turn.is_empty(){None}else{Some(&ctx.current_turn)},"name":"unknown_tool","kind":"unknown","status":"returned_unknown","exitCode":Value::Null,"isError":Value::Null,"durationMs":Value::Null,"eventTime":time});
            let metadata = result_metadata(p);
            if let Some(code) = metadata.get("exitCode").and_then(Value::as_i64) {
                data["exitCode"] = json!(code);
                data["status"] = json!(if code == 0 { "completed" } else { "failed" });
            }
            if let Some(error) = metadata.get("isError").and_then(Value::as_bool) {
                data["isError"] = json!(error);
                if error {
                    data["status"] = json!("failed");
                }
            }
            if let Some(ms) = metadata.get("durationMs").and_then(Value::as_i64) {
                data["durationMs"] = json!(ms);
                data["durationSource"] = json!("tool-result-recorded-wall-time");
            }
            save_item(db, &id, &data, &time)?;
        }
    }
    Ok(0)
}
pub(super) fn load_fact(db: &Connection, table: &str, id: &str) -> Result<Option<Value>> {
    db.query_row(
        &format!("SELECT data FROM {table} WHERE id=?1"),
        [id],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .map_err(err)?
    .map(|s| serde_json::from_str(&s).map_err(err))
    .transpose()
}
fn known_turn_result(v: &Value) -> bool {
    matches!(
        v["status"].as_str(),
        Some("completed" | "failed" | "interrupted")
    )
}

fn turn_result_time(v: &Value) -> Option<String> {
    if !known_turn_result(v) && v["status"] != "unknown" {
        return None;
    }
    optional(v, "lifecycleEventAt")
        .and_then(|t| normalize_time(&t))
        // Old stored JSON has no result provenance. A recorded terminal end
        // supplies its conservative baseline; metadata eventTime never does.
        .or_else(|| optional(v, "completedAt").and_then(|t| normalize_time(&t)))
}

fn incoming_turn_result_wins(old: &Value, incoming: &Value) -> bool {
    if known_turn_result(old) && !known_turn_result(incoming) {
        return false;
    }
    if known_turn_result(incoming) && !known_turn_result(old) {
        return true;
    }
    match (turn_result_time(old), turn_result_time(incoming)) {
        (Some(old), Some(incoming)) => incoming >= old,
        (None, Some(_)) => true,
        _ => false,
    }
}

fn native_turn_owners(db: &Connection, source: &str, turn: &str) -> Result<Vec<String>> {
    if source.is_empty() || turn.is_empty() {
        return Ok(vec![]);
    }
    let mut statement = db.prepare("SELECT DISTINCT thread_id FROM usage_responses WHERE source_id=?1 AND turn_id=?2 LIMIT 2").map_err(err)?;
    let rows = statement
        .query_map(params![source, turn], |row| row.get::<_, String>(0))
        .map_err(err)?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(err)
}

fn save_turn(db: &Connection, id: &str, v: &Value, time: &str) -> Result<()> {
    let old = load_fact(db, "turns", id)?;
    let mut fact = v.clone();
    if let Some(old) = old.as_ref() {
        if strv(&old, "eventTime").as_str() > time {
            fact = old.clone();
            if let (Some(target), Some(incoming)) = (fact.as_object_mut(), v.as_object()) {
                for (key, value) in incoming {
                    if target.get(key).is_none_or(|v| v.is_null()) && !value.is_null() {
                        target.insert(key.clone(), value.clone());
                    }
                }
            }
        }
    }
    let result = if turn_result_time(v).is_some() {
        Some(match old.as_ref() {
            Some(old) if !incoming_turn_result_wins(old, v) => old,
            _ => v,
        })
    } else {
        old.as_ref().filter(|old| turn_result_time(old).is_some())
    };
    if let Some(result) = result {
        // Keep fields belonging to accepted result evidence together even when
        // the global metadata clock has advanced past that lifecycle event.
        for key in ["status", "completedAt", "durationMs", "timeToFirstTokenMs"] {
            if let Some(value) = result.get(key) {
                fact[key] = value.clone();
            } else {
                fact.as_object_mut().unwrap().remove(key);
            }
        }
        if let Some(at) = turn_result_time(result) {
            fact["lifecycleEventAt"] = json!(at);
            fact["resultEvidence"] = result.get("resultEvidence").cloned().unwrap_or_else(|| {
                json!(if known_turn_result(result) {
                    "legacy-terminal-completed-at"
                } else {
                    "legacy-unknown-completed-at"
                })
            });
        } else {
            fact.as_object_mut().unwrap().remove("lifecycleEventAt");
            fact.as_object_mut().unwrap().remove("resultEvidence");
        }
    }
    let time = [
        Some(time),
        v["eventTime"].as_str(),
        old.as_ref().and_then(|old| old["eventTime"].as_str()),
    ]
    .into_iter()
    .flatten()
    .max()
    .unwrap()
    .to_owned();
    if fact.get("lastEventAt").is_some() {
        fact["lastEventAt"] = json!([
            v["lastEventAt"].as_str(),
            old.as_ref().and_then(|old| old["lastEventAt"].as_str()),
            Some(time.as_str()),
        ]
        .into_iter()
        .flatten()
        .max());
    }
    fact["channelId"] = v
        .get("channelId")
        .cloned()
        .unwrap_or(json!(format!("{}:jsonl", strv(v, "sourceId"))));
    fact["parserVersion"] = json!(SEMANTICS);
    fact["collectedAt"] = json!(now());
    fact["eventTime"] = json!(time);
    fact["granularity"] = json!("turn");
    fact["timingUnit"] = json!("ms");
    fact["completeness"] = json!("source-recorded-fields-only");
    // Metadata time is not ownership authority. Only an unambiguous native
    // source+turn domain may override the thread selected by metadata merging.
    let source = strv(&fact, "sourceId");
    let turn = strv(&fact, "id");
    let owners = native_turn_owners(db, &source, &turn)?;
    if !owners.is_empty() {
        let ambiguous = owners.len() > 1;
        let owner = if ambiguous { "" } else { owners[0].as_str() };
        fact["threadId"] = json!(owner);
        fact["threadAttribution"] = json!(if ambiguous {
            "ambiguous-native-owner"
        } else {
            "unique-native-owner"
        });
        if ambiguous {
            issue(
                db,
                "ambiguous_turn_thread",
                "",
                &hash(format!("{source}|{turn}").as_bytes()),
                "同一来源回合关联了多个线程，未猜测归属或生成完成提醒",
            )?;
        }
        let inbox_id = hash(format!("completion|{source}|{turn}").as_bytes());
        if let Some(mut inbox) = load_fact(db, "inbox_events", &inbox_id)? {
            let before = inbox.clone();
            inbox["threadId"] = json!(owner);
            if ambiguous {
                inbox["threadAttribution"] = json!("ambiguous-native-owner");
                inbox["notifyEligible"] = json!(false);
                // Prior OS delivery remains historical truth; never turn sent
                // into unsent or replay a notification on metadata refinement.
            } else if inbox.get("threadAttribution").is_some() {
                inbox["threadAttribution"] = json!("unique-native-owner");
            }
            if inbox != before {
                db.execute(
                    "UPDATE inbox_events SET thread_id=?1,data=?2 WHERE id=?3",
                    params![owner, inbox.to_string(), inbox_id],
                )
                .map_err(err)?;
            }
        }
    }
    let v = &fact;
    db.execute("INSERT INTO turns(id,thread_id,turn_id,event_time,data) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(id) DO UPDATE SET thread_id=excluded.thread_id,event_time=excluded.event_time,data=excluded.data",params![id,strv(v,"threadId"),strv(v,"id"),time,v.to_string()]).map_err(err)?;
    Ok(())
}
fn tool_result_class(value: &Value) -> Option<&'static str> {
    match value["status"].as_str()? {
        "completed" | "success" | "succeeded" | "normal" | "ok" => Some("completed"),
        "failed" | "error" => Some("failed"),
        "interrupted" => Some("interrupted"),
        "cancelled" | "canceled" => Some("cancelled"),
        _ => None,
    }
}

fn tool_state_rank(value: &Value) -> u8 {
    if tool_result_class(value).is_some() {
        2
    } else if matches!(value["status"].as_str(), Some("started" | "active" | "running")) {
        0
    } else {
        1
    }
}

fn tool_results_conflict(old: &Value, incoming: &Value) -> bool {
    let (Some(old_class), Some(incoming_class)) =
        (tool_result_class(old), tool_result_class(incoming))
    else {
        return false;
    };
    old_class != incoming_class
        || matches!((old["exitCode"].as_i64(), incoming["exitCode"].as_i64()), (Some(a), Some(b)) if a != b)
        || matches!((old["isError"].as_bool(), incoming["isError"].as_bool()), (Some(a), Some(b)) if a != b)
}

fn tool_result_evidence(value: &Value) -> Value {
    json!({"status":value["status"],"exitCode":value["exitCode"],"isError":value["isError"]})
}

fn save_item(db: &Connection, id: &str, v: &Value, time: &str) -> Result<()> {
    let mut fact = v.clone();
    if let Some(old) = load_fact(db, "items", id)? {
        let incoming_newer = strv(&old, "eventTime").as_str() <= time;
        let incoming_result_wins = tool_state_rank(v) > tool_state_rank(&old)
            || tool_state_rank(v) == tool_state_rank(&old) && incoming_newer;
        fact = old.clone();
        if let (Some(target), Some(incoming)) = (fact.as_object_mut(), v.as_object()) {
            for (key, value) in incoming {
                if value.is_null()
                    || ["status", "exitCode", "isError", "eventTime", "resultConflict", "resultEvidence"]
                        .contains(&key.as_str())
                {
                    continue;
                }
                let missing = target.get(key).is_none_or(|old| old.is_null());
                let placeholder = matches!(key.as_str(), "name" | "kind")
                    && matches!(value.as_str(), Some("unknown_tool" | "unknown"));
                let old_placeholder = target.get(key).and_then(Value::as_str)
                    .is_some_and(|s| matches!(s, "unknown_tool" | "unknown"));
                let identity_detail = matches!(key.as_str(), "name" | "kind");
                if key == "startedAt" && target.get(key).and_then(Value::as_str)
                    .zip(value.as_str()).is_some_and(|(old, incoming)| old <= incoming)
                {
                    continue;
                }
                if missing || old_placeholder || incoming_newer && !placeholder && !identity_detail {
                    target.insert(key.clone(), value.clone());
                }
            }
        }
        if old["resultConflict"].as_bool() == Some(true) || tool_results_conflict(&old, v) {
            // Contradictory explicit results have no revision contract. Preserve
            // the first contradictory pair, not a guessed last/failure winner.
            if old["resultConflict"].as_bool() != Some(true) {
                let mut evidence = vec![tool_result_evidence(&old), tool_result_evidence(v)];
                evidence.sort_by_key(|value| value.to_string());
                fact["resultEvidence"] = json!(evidence);
                issue(
                    db,
                    "conflicting_tool_result",
                    "",
                    &hash(id.as_bytes()),
                    "同一工具调用记录了矛盾的明确结果，已保留最小结果证据并显示结果未知",
                )?;
            }
            fact["resultConflict"] = json!(true);
            fact["status"] = json!("returned_unknown");
            fact["exitCode"] = Value::Null;
            fact["isError"] = Value::Null;
        } else {
            if incoming_result_wins {
                fact["status"] = v["status"].clone();
            }
            for key in ["exitCode", "isError"] {
                if !v[key].is_null() && (incoming_result_wins || fact[key].is_null()) {
                    fact[key] = v[key].clone();
                }
            }
        }
    }
    let time = fact
        .get("eventTime")
        .and_then(Value::as_str)
        .filter(|old| *old > time)
        .unwrap_or(time)
        .to_owned();
    fact["channelId"] = v
        .get("channelId")
        .cloned()
        .unwrap_or(json!(format!("{}:jsonl", strv(v, "sourceId"))));
    fact["parserVersion"] = json!(SEMANTICS);
    fact["collectedAt"] = json!(now());
    fact["eventTime"] = json!(time);
    fact["granularity"] = json!("tool");
    fact["timingUnit"] = json!("ms");
    fact["completeness"] = json!("source-recorded-fields-only");
    let v = &fact;
    db.execute("INSERT INTO items(id,thread_id,turn_id,event_time,data) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(id) DO UPDATE SET turn_id=excluded.turn_id,event_time=excluded.event_time,data=excluded.data",params![id,strv(v,"threadId"),v.get("turnId").and_then(Value::as_str),time,v.to_string()]).map_err(err)?;
    Ok(())
}

#[cfg(test)]
#[path = "tool_state_tests.rs"]
mod tool_state_tests;
fn completion(
    db: &Connection,
    source: &str,
    t: &Value,
    time: &str,
    live: bool,
    runtime: &str,
) -> Result<()> {
    if get_setting(db, "inboxWatermark")?
        .and_then(|v| v.as_str().map(str::to_owned))
        .is_some_and(|s| s.as_str() >= time)
    {
        return Ok(());
    }
    let id = hash(format!("completion|{source}|{}", strv(t, "id")).as_bytes());
    // The caller has already saved and accepted lifecycle evidence. In
    // particular, an explicit result can refine an earlier-arriving generic
    // unknown even when its own source timestamp is older. Preserve the first
    // createdAt and delivery state below instead of re-timing that reminder.
    let old = load_fact(db, "inbox_events", &id)?;
    let status = strv(t, "status");
    let title = match status.as_str() {
        "completed" => "回合正常结束",
        "failed" => "回合失败",
        "interrupted" => "回合已中断",
        _ => "已结束，结果未知",
    };
    let eligible = old
        .as_ref()
        .and_then(|v| v["notifyEligible"].as_bool())
        .unwrap_or(live);
    let data = json!({"id":id,"sourceId":source,"kind":"completion","threadId":t["threadId"],"turnId":t["id"],"title":title,"status":status,"createdAt":old.as_ref().and_then(|v|v.get("createdAt")).cloned().unwrap_or(json!(time)),"read":old.as_ref().and_then(|v|v["read"].as_bool()).unwrap_or(false),"notifyEligible":eligible,"animationAcknowledged":old.as_ref().and_then(|v|v["animationAcknowledged"].as_bool()).unwrap_or(false),"eligibleRuntime":old.as_ref().and_then(|v|v.get("eligibleRuntime")).cloned().unwrap_or(json!(runtime)),"notificationState":old.as_ref().and_then(|v|v.get("notificationState")).cloned().unwrap_or(json!(if live{"pending"}else{"history-only"})),"notified":old.as_ref().and_then(|v|v["notified"].as_bool()).unwrap_or(false)});
    db.execute("INSERT INTO inbox_events(id,thread_id,turn_id,event_time,data) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(id) DO UPDATE SET data=excluded.data",params![id,strv(t,"threadId"),strv(t,"id"),time,data.to_string()]).map_err(err)?;
    Ok(())
}
fn safe_name(s: &str) -> String {
    if !s.is_empty()
        && s.len() <= 120
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_.:/-".contains(c))
    {
        s.to_owned()
    } else {
        "unknown_tool".into()
    }
}
fn result_metadata(payload: &Value) -> Value {
    let output = payload.get("output").unwrap_or(&Value::Null);
    let mut values = Vec::new();
    let mut texts = Vec::new();
    if let Some(s) = output.as_str() {
        texts.push(s);
    }
    if let Some(a) = output.as_array() {
        for block in a.iter().take(16) {
            if block.get("type").and_then(Value::as_str) == Some("text") {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    texts.push(text);
                }
            }
        }
    }
    if output.is_object() {
        values.push(output.clone());
    }
    let mut metadata = json!({});
    for text in texts {
        if let Ok(value) = serde_json::from_str::<Value>(text) {
            if value.is_object() {
                values.push(value);
            }
        }
        // The source's execution envelope header precedes Output:. Never search command stdout.
        if text.starts_with("Chunk ID:") {
            for line in text.split("Output:").next().unwrap_or("").lines().take(12) {
                if let Some(s) = line.strip_prefix("Wall time:") {
                    if let Some(n) = s
                        .split_whitespace()
                        .next()
                        .and_then(|s| s.parse::<f64>().ok())
                        .filter(|n| n.is_finite() && *n >= 0.0 && *n < 1e9)
                    {
                        metadata["durationMs"] = json!((n * 1000.0).round() as i64);
                    }
                }
                for prefix in ["Process exited with code ", "Exit code: "] {
                    if let Some(code) = line
                        .strip_prefix(prefix)
                        .and_then(|s| s.trim().parse::<i64>().ok())
                    {
                        metadata["exitCode"] = json!(code);
                    }
                }
            }
        }
    }
    for v in values {
        if let Some(code) = v.get("exit_code").and_then(Value::as_i64) {
            metadata["exitCode"] = json!(code);
        }
        if let Some(error) = v
            .get("isError")
            .or_else(|| v.get("is_error"))
            .and_then(Value::as_bool)
        {
            metadata["isError"] = json!(error);
        }
        if let Some(ms) = v
            .get("duration_ms")
            .and_then(Value::as_i64)
            .filter(|n| *n >= 0)
        {
            metadata["durationMs"] = json!(ms);
        } else if let Some(s) = v
            .get("wall_time_seconds")
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite() && *n >= 0.0 && *n < 1e9)
        {
            metadata["durationMs"] = json!((s * 1000.0).round() as i64);
        }
    }
    metadata
}
fn duration(p: &Value) -> Option<i64> {
    p.get("duration_ms")
        .and_then(Value::as_i64)
        .filter(|n| *n >= 0)
        .or_else(|| {
            let d = p.get("duration")?;
            let secs = d.get("secs")?.as_i64()?;
            let nanos = d.get("nanos").and_then(Value::as_i64).unwrap_or(0);
            if secs < 0 || !(0..1_000_000_000).contains(&nanos) {
                None
            } else {
                secs.checked_mul(1000)?.checked_add(nanos / 1_000_000)
            }
        })
}
fn tool_event(
    db: &Connection,
    source: &str,
    p: &Value,
    thread: &str,
    turn: &str,
    time: &str,
) -> Result<()> {
    let call = optional(p, "call_id").or_else(|| optional(p, "item_id"));
    let Some(call) = call else { return Ok(()) };
    let id = format!("{source}|{thread}|{call}");
    let kind = strv(p, "type");
    let mut v=json!({"id":call,"sourceId":source,"threadId":thread,"turnId":if turn.is_empty(){None}else{Some(turn)},"name":safe_name(&optional(p,"tool_name").or_else(||optional(p,"tool")).unwrap_or_else(||if kind.starts_with("exec_"){"exec_command".into()}else{"unknown_tool".into()})),"kind":if kind.starts_with("mcp_"){"mcp"}else{"command"},"exitCode":Value::Null,"isError":Value::Null,"durationMs":Value::Null});
    let end = kind.ends_with("_end");
    v["status"] = json!(if end {
        p.get("status")
            .and_then(Value::as_str)
            .unwrap_or("returned_unknown")
    } else {
        "started"
    });
    v["eventTime"] = json!(time);
    if let Some(code) = p.get("exit_code").and_then(Value::as_i64) {
        v["exitCode"] = json!(code);
        if end {
            v["status"] = json!(if code == 0 { "completed" } else { "failed" });
        }
    }
    if let Some(error) = p
        .get("is_error")
        .and_then(Value::as_bool)
        .or_else(|| p.pointer("/result/isError").and_then(Value::as_bool))
    {
        v["isError"] = json!(error);
        if error {
            v["status"] = json!("failed");
        }
    }
    v["durationMs"] = json!(duration(p));
    save_item(db, &id, &v, time)
}
fn legacy_allowed_counts(u: &Value) -> Value {
    let mut allowed = serde_json::Map::new();
    for key in [
        "input_tokens",
        "output_tokens",
        "cached_input_tokens",
        "cache_write_input_tokens",
        "reasoning_output_tokens",
        "total_tokens",
    ] {
        if let Some(value) = u.get(key) {
            allowed.insert(
                key.into(),
                value
                    .as_i64()
                    .filter(|n| *n >= 0)
                    .map(Value::from)
                    .unwrap_or(Value::Null),
            );
        }
    }
    Value::Object(allowed)
}
// Bootstrap only the metadata required to decode a live tail. No historical
// response, lifecycle, tool or inbox fact is emitted by this operation.
fn advance_codex_context(v: &Value, ctx: &mut Context) {
    let p = &v["payload"];
    match strv(v, "type").as_str() {
        "session_meta" => {
            let id = strv(p, "id");
            if !id.is_empty() {
                if ctx.owner.is_empty() {
                    ctx.owner = id.clone();
                    ctx.own_start = p["subagent_history_start_ordinal"].as_u64();
                    ctx.parent = strv(p, "parent_thread_id");
                }
                ctx.active_thread = id;
            }
        }
        "turn_context" => {
            ctx.current_turn = strv(p, "turn_id");
            ctx.model = strv(p, "model");
        }
        "event_msg" => {
            let event = strv(p, "type");
            if [
                "task_started",
                "task_complete",
                "turn_completed",
                "turn_aborted",
                "turn_failed",
            ]
            .contains(&event.as_str())
            {
                ctx.current_turn = strv(p, "turn_id");
            } else if event == "token_count" {
                let thread = optional(p, "thread_id").unwrap_or_else(|| ctx.thread(v));
                if let (Some(counts), Some(time)) = (
                    p.pointer("/info/total_token_usage"),
                    normalize_time(&strv(v, "timestamp")),
                ) {
                    if !thread.is_empty() {
                        ctx.legacy.insert(thread, json!({"counts":legacy_allowed_counts(counts),"time":time,"turn":ctx.current_turn,"model":ctx.model}));
                    }
                }
            }
        }
        _ => {}
    }
}
fn legacy(
    db: &Connection,
    source: &str,
    file: &str,
    p: &Value,
    thread: &str,
    time: &str,
    ctx: &mut Context,
    within: bool,
) -> Result<()> {
    let Some(u) = p.pointer("/info/total_token_usage") else {
        return Ok(());
    };
    if thread.is_empty() {
        return Ok(());
    }
    let filtered = legacy_allowed_counts(u);
    let u = &filtered;
    let old = ctx.legacy.insert(
        thread.into(),
        json!({"counts":u,"time":time,"turn":ctx.current_turn,"model":ctx.model}),
    );
    let Some(old) = old else {
        if within {
            issue(
                db,
                "legacy_missing_baseline",
                file,
                &hash(format!("{thread}|{time}").as_bytes()),
                "旧格式首个累计快照仅建立基线，不计为新增用量",
            )?;
        }
        return Ok(());
    };
    if !within {
        return Ok(());
    }
    let mut delta = serde_json::Map::new();
    let mut reset = false;
    for key in [
        "input_tokens",
        "output_tokens",
        "cached_input_tokens",
        "cache_write_input_tokens",
        "reasoning_output_tokens",
        "total_tokens",
    ] {
        if let (Some(a), Some(b)) = (
            u.get(key).and_then(Value::as_i64),
            old["counts"].get(key).and_then(Value::as_i64),
        ) {
            if a < b {
                reset = true;
            } else {
                delta.insert(key.into(), json!(a - b));
            }
        }
    }
    if reset {
        issue(
            db,
            "legacy_counter_reset",
            file,
            &hash(format!("{thread}|{time}").as_bytes()),
            "累计回退，开启新基线；未将首快照视为新增",
        )?;
        return Ok(());
    }
    if strv(&old, "model") != ctx.model {
        issue(
            db,
            "legacy_model_change",
            file,
            &hash(time.as_bytes()),
            "模型切换期间的累计差值归属不明，已保留缺口",
        )?;
        return Ok(());
    }
    let counts = codex_counts(&read_native(
        &Value::Object(delta),
        &[
            "input_tokens",
            "output_tokens",
            "cached_input_tokens",
            "cache_write_input_tokens",
            "reasoning_output_tokens",
            "total_tokens",
        ],
    ));
    if counts.total.value().unwrap_or(0) == 0 {
        return Ok(());
    }
    let id = hash(format!("legacy-v1|{source}|{thread}|{}|{time}", strv(&old, "time")).as_bytes());
    let data = json!({"id":id,"sourceId":source,"threadId":thread,"turnId":if ctx.current_turn.is_empty(){None}else{Some(&ctx.current_turn)},"from":old["time"],"eventTime":time,"model":if ctx.model.is_empty(){None}else{Some(&ctx.model)},"algorithmVersion":"cumulative-difference-v1","quality":"derived-limited","counts":counts});
    db.execute("INSERT OR IGNORE INTO legacy_usage_segments(id,source_id,thread_id,turn_id,event_time,data) VALUES(?1,?2,?3,?4,?5,?6)",params![id,source,thread,ctx.current_turn,time,data.to_string()]).map_err(err)?;
    Ok(())
}
