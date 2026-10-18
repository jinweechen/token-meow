//! Read-only, owned Codex app-server transport. No login, inference or task control API.
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
#[cfg(any(windows, test))]
use std::ffi::OsString;
use std::{
    collections::HashSet,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc::{self, Receiver},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(4);
const MAX_MESSAGE: usize = 2 * 1024 * 1024;
const UNSUPPORTED: &str = "当前 Codex 不支持该读取能力";
const RPC_REJECTED: &str = "账户读取被拒绝或服务暂不可用";
const DEFERRED: &str = "桥接失败退避中，尚未发起新的读取；可手动重试";
const SOURCE_UNAVAILABLE: &str = "所选 Codex 来源目录不存在或已不再是目录；未发起新的桥接读取";
const RESOURCE_WARNING: u8 = 1;
const RESOURCE_HARD_LIMIT: u8 = 2;
const RESOURCE_MONITOR_FAILED: u8 = 3;

fn resource_error(reason: u8) -> Option<&'static str> {
    match reason {
        RESOURCE_WARNING => {
            Some("桥接触发 96 MiB 提交内存预算预警，已停止全部自有桥接进程；请手动重试")
        }
        RESOURCE_HARD_LIMIT => {
            Some("已观察到桥接 128 MiB 提交内存硬上限事件，已停止全部自有桥接进程；请手动重试")
        }
        RESOURCE_MONITOR_FAILED => Some("桥接预算监视不可用，已停止全部自有桥接进程；请手动重试"),
        _ => None,
    }
}
const ALLOWED: &[&str] = &[
    "initialize",
    "account/read",
    "account/rateLimits/read",
    "account/usage/read",
    "thread/list",
];

#[cfg(any(windows, test))]
fn merge_bridge_environment(
    inherited: impl IntoIterator<Item = (OsString, OsString)>,
    overrides: &[(OsString, OsString)],
) -> Vec<(OsString, OsString)> {
    // Only remove these inherited credential/task inputs. Keep security policy,
    // certificates, proxies and unknown variables; explicit source/test overrides
    // retain their existing precedence. This is not all-provider auth isolation.
    const DENIED_INHERITED: &[&str] = &[
        "CODEX_HOME",
        "CODEX_ACCESS_TOKEN",
        "CODEX_API_KEY",
        "CODEX_APP_TOOLS_PIPE_PATH",
        "CODEX_INTERNAL_ORIGINATOR_OVERRIDE",
        "CODEX_THREAD_ID",
        "CODEX_SESSION_ID",
        "CODEX_TOOL_CALL_ID",
        "CODEX_TASK_WORKSPACE_VERIFYING_IDENTITY",
    ];
    let mut vars: Vec<_> = inherited
        .into_iter()
        .filter(|(key, _)| {
            let key = key.to_string_lossy();
            !DENIED_INHERITED
                .iter()
                .any(|denied| key.eq_ignore_ascii_case(denied))
                && !overrides.iter().any(|(override_key, _)| {
                    override_key.to_string_lossy().eq_ignore_ascii_case(&key)
                })
        })
        .collect();
    vars.extend_from_slice(overrides);
    vars
}

// Internal, bounded evidence only. No RPC params, responses, paths or account facts
// enter this record, and public account/directory errors remain unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[repr(u8)]
enum BridgeStage {
    #[serde(rename = "startup")]
    Startup,
    #[serde(rename = "initialize")]
    Initialize,
    #[serde(rename = "idle")]
    Idle,
    #[serde(rename = "account/read")]
    AccountRead,
    #[serde(rename = "account/rateLimits/read")]
    RateLimitsRead,
    #[serde(rename = "account/usage/read")]
    UsageRead,
    #[serde(rename = "thread/list")]
    ThreadList,
}

impl BridgeStage {
    fn for_method(method: &str) -> Option<Self> {
        match method {
            "initialize" => Some(Self::Initialize),
            "account/read" => Some(Self::AccountRead),
            "account/rateLimits/read" => Some(Self::RateLimitsRead),
            "account/usage/read" => Some(Self::UsageRead),
            "thread/list" => Some(Self::ThreadList),
            _ => None,
        }
    }

    fn from_code(code: u8) -> Self {
        match code {
            1 => Self::Initialize,
            2 => Self::Idle,
            3 => Self::AccountRead,
            4 => Self::RateLimitsRead,
            5 => Self::UsageRead,
            6 => Self::ThreadList,
            _ => Self::Startup,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct BridgeRootIdentity {
    pid: u32,
    #[serde(serialize_with = "serialize_optional_filetime")]
    creation_filetime: Option<u64>,
    creation_query_error: Option<i32>,
}

fn serialize_optional_filetime<S>(value: &Option<u64>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match value {
        Some(value) => serializer.serialize_some(&value.to_string()),
        None => serializer.serialize_none(),
    }
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct BridgeResourceCleanup {
    monitor_terminate_job_succeeded: Option<bool>,
    owner_terminate_job_succeeded: Option<bool>,
    owner_terminate_process_succeeded: Option<bool>,
    owner_wait_result: Option<u32>,
    owner_cleanup_elapsed_ms: Option<u64>,
}

#[derive(Clone, Copy)]
struct BridgeLimitObservation {
    job_memory: Option<u64>,
    job_memory_limit: Option<u64>,
    limit_flags: u32,
    violation_limit_flags: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BridgeResourceDiagnostic {
    reason: u8,
    // UTC, elapsed time and stage are observed at IOCP dequeue. Delivery is
    // asynchronous, so none proves that this RPC caused the committed memory.
    // The stage is the last selected stage, not proof that a method was sent.
    event_utc: String,
    elapsed_ms: u64,
    observed_stage: BridgeStage,
    root: Option<BridgeRootIdentity>,
    completion_message: Option<u32>,
    notification_query_succeeded: Option<bool>,
    monitor_error: Option<i32>,
    job_memory: Option<u64>,
    job_memory_limit: Option<u64>,
    limit_flags: Option<u32>,
    violation_limit_flags: Option<u32>,
    cleanup: BridgeResourceCleanup,
}

struct BridgeDiagnosticState {
    started: Instant,
    root: Option<BridgeRootIdentity>,
    first_event: Option<BridgeResourceDiagnostic>,
}

struct BridgeDiagnostics {
    stage: AtomicU8,
    state: Mutex<BridgeDiagnosticState>,
}

impl BridgeDiagnostics {
    fn new() -> Self {
        Self {
            stage: AtomicU8::new(BridgeStage::Startup as u8),
            state: Mutex::new(BridgeDiagnosticState {
                started: Instant::now(),
                root: None,
                first_event: None,
            }),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, BridgeDiagnosticState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn begin_startup(&self) {
        let mut state = self.state();
        if state.first_event.is_none() {
            state.started = Instant::now();
            self.set_stage(BridgeStage::Startup);
            state.root = None;
        }
    }

    fn set_stage(&self, stage: BridgeStage) {
        self.stage.store(stage as u8, Ordering::Release);
    }

    fn observed_stage(&self) -> BridgeStage {
        BridgeStage::from_code(self.stage.load(Ordering::Acquire))
    }

    fn set_root(&self, root: BridgeRootIdentity) {
        let mut state = self.state();
        state.root = Some(root);
        // A monitor failure can precede suspended process creation. Attach only
        // this owner's subsequently available identity, before it can run.
        if let Some(event) = state.first_event.as_mut() {
            if event.root.is_none() {
                event.root = Some(root);
            }
        }
    }

    fn record_resource_event(
        &self,
        reason: u8,
        observed_stage: BridgeStage,
        observed_at: chrono::DateTime<chrono::Utc>,
        observed_instant: Instant,
        completion_message: Option<u32>,
        notification_query_succeeded: Option<bool>,
        monitor_error: Option<i32>,
        violation: Option<BridgeLimitObservation>,
    ) {
        let mut state = self.state();
        if state.first_event.is_some() {
            return;
        }
        state.first_event = Some(BridgeResourceDiagnostic {
            reason,
            event_utc: observed_at.to_rfc3339(),
            elapsed_ms: elapsed_ms_at(state.started, observed_instant),
            observed_stage,
            root: state.root,
            completion_message,
            notification_query_succeeded,
            monitor_error,
            // Windows supplies memory at notification, not a current sample.
            job_memory: violation.and_then(|value| value.job_memory),
            job_memory_limit: violation.and_then(|value| value.job_memory_limit),
            limit_flags: violation.map(|value| value.limit_flags),
            violation_limit_flags: violation.map(|value| value.violation_limit_flags),
            cleanup: BridgeResourceCleanup::default(),
        });
    }

    fn record_monitor_cleanup(&self, succeeded: bool) {
        if let Some(event) = self.state().first_event.as_mut() {
            event
                .cleanup
                .monitor_terminate_job_succeeded
                .get_or_insert(succeeded);
        }
    }

    fn record_owner_cleanup(&self, root: BridgeRootIdentity, job: bool, process: bool, wait: u32) {
        let mut state = self.state();
        let elapsed = elapsed_ms(state.started);
        if let Some(event) = state.first_event.as_mut() {
            // Never label a different PID/creation pair as this event's cleanup.
            if event.root == Some(root) && event.cleanup.owner_wait_result.is_none() {
                event.cleanup.owner_terminate_job_succeeded = Some(job);
                event.cleanup.owner_terminate_process_succeeded = Some(process);
                event.cleanup.owner_wait_result = Some(wait);
                event.cleanup.owner_cleanup_elapsed_ms = Some(elapsed);
            }
        }
    }

    fn snapshot(&self) -> Option<BridgeResourceDiagnostic> {
        self.state().first_event.clone()
    }

    fn reset(&self) {
        *self.state() = BridgeDiagnosticState {
            started: Instant::now(),
            root: None,
            first_event: None,
        };
        self.set_stage(BridgeStage::Startup);
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    elapsed_ms_at(started, Instant::now())
}

fn elapsed_ms_at(started: Instant, observed: Instant) -> u64 {
    u64::try_from(observed.saturating_duration_since(started).as_millis()).unwrap_or(u64::MAX)
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSnapshot {
    pub status: String,
    pub collected_at: Option<String>,
    pub last_attempt_at: Option<String>,
    pub error: Option<String>,
    pub auth_mode: Option<String>,
    pub account_scope: Option<String>,
    pub buckets: Vec<QuotaBucket>,
    pub usage: Option<Value>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaBucket {
    pub id: String,
    pub name: Option<String>,
    pub primary: Option<QuotaWindow>,
    pub secondary: Option<QuotaWindow>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindow {
    pub used_percent: f64,
    pub window_duration_mins: Option<u64>,
    pub resets_at: Option<i64>,
}

impl Default for AccountSnapshot {
    fn default() -> Self {
        Self {
            status: "unavailable".into(),
            collected_at: None,
            last_attempt_at: None,
            error: Some("账户尚未采集".into()),
            auth_mode: None,
            account_scope: None,
            buckets: vec![],
            usage: None,
        }
    }
}

pub struct AccountBridge {
    transport: Option<Transport>,
    snapshot: AccountSnapshot,
    failure_count: u32,
    next_attempt: Option<Instant>,
    cancel: Arc<AtomicBool>,
    identity_observation: Option<String>,
    source_root: Option<PathBuf>,
    // Process protection is global to this bridge, not owned by cached account data.
    // Neither changing source nor clearing observations may revive a stopped service.
    resource_stop: Arc<AtomicU8>,
    diagnostics: Arc<BridgeDiagnostics>,
}

struct AdmissionError {
    message: String,
    attempted_at: Option<String>,
}

impl Default for AccountBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl AccountBridge {
    pub fn new() -> Self {
        Self {
            transport: None,
            snapshot: AccountSnapshot::default(),
            failure_count: 0,
            next_attempt: None,
            cancel: Arc::new(AtomicBool::new(false)),
            identity_observation: None,
            source_root: None,
            resource_stop: Arc::new(AtomicU8::new(0)),
            diagnostics: Arc::new(BridgeDiagnostics::new()),
        }
    }

    pub fn with_source_root(root: &Path) -> Self {
        let mut bridge = Self::new();
        bridge.source_root = Some(root.to_path_buf());
        bridge
    }

    /// Switch only between explicitly configured source roots. Any prior snapshots and
    /// transport belong to the old source and must be discarded before the next request.
    pub fn set_source_root(&mut self, root: &Path) {
        if self.source_root.as_deref() == Some(root) {
            return;
        }
        self.clear_cached_data();
        self.source_root = Some(root.to_path_buf());
    }

    /// Discard only this application's cached observations. Keep the selected source
    /// and the existing cancellation handle so shutdown remains effective afterwards.
    /// No RPC is sent; a later explicit refresh starts from an empty snapshot.
    pub fn clear_cached_data(&mut self) {
        self.shutdown();
        self.snapshot = AccountSnapshot::default();
        self.identity_observation = None;
    }

    /// Only an explicit user retry clears admission failure gates. No account facts,
    /// identity, collection timestamps, authentication, or source files are changed.
    pub fn retry_manually(&mut self) {
        if !self.cancel.load(Ordering::Relaxed) {
            // Preserve a healthy transport and its per-method capability results.
            // Only a latched resource stop needs its old monitor joined first.
            if let Some(error) = resource_error(self.resource_stop.load(Ordering::Acquire)) {
                self.shutdown();
                self.mark_unavailable(error.into());
                self.resource_stop.store(0, Ordering::Release);
                self.diagnostics.reset();
            }
            self.failure_count = 0;
            self.next_attempt = None;
        }
    }

    pub fn cancel_handle(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    pub(crate) fn resource_diagnostic(&self) -> Option<BridgeResourceDiagnostic> {
        self.diagnostics.snapshot()
    }

    /// Observe an asynchronous resource stop without starting or reading the service.
    /// Reap only our stopped transport; only an explicit manual retry clears the latch.
    pub(crate) fn observe_resource_stop(&mut self) -> Option<&'static str> {
        let error = resource_error(self.resource_stop.load(Ordering::Acquire))?;
        self.shutdown();
        Some(error)
    }

    pub fn refresh(&mut self) -> AccountSnapshot {
        self.refresh_with(Transport::start)
    }

    fn refresh_with(
        &mut self,
        start: impl FnOnce(
            Arc<AtomicBool>,
            Option<&Path>,
            Arc<AtomicU8>,
            Arc<BridgeDiagnostics>,
        ) -> Result<Transport, String>,
    ) -> AccountSnapshot {
        if let Err(error) = self.ensure_transport_with(start) {
            if let Some(at) = error.attempted_at {
                self.snapshot.last_attempt_at = Some(at);
            }
            self.mark_unavailable(error.message);
            return self.snapshot.clone();
        }
        self.snapshot.last_attempt_at = Some(chrono::Utc::now().to_rfc3339());
        let result = self.fetch();
        match self.finish_read(result) {
            Ok(mut current) => {
                current.last_attempt_at = self.snapshot.last_attempt_at.clone();
                self.snapshot = current;
            }
            Err(error) => {
                self.mark_unavailable(error);
            }
        }
        self.snapshot.clone()
    }

    fn fetch(&mut self) -> Result<AccountSnapshot, String> {
        let transport = self.transport.as_mut().ok_or("账户桥接未连接")?;
        read_account_snapshot(
            transport,
            &mut self.snapshot,
            &mut self.identity_observation,
        )
    }

    /// Directory data never supplies runtime activity and never copies `preview` or turns.
    pub fn list_sessions(&mut self) -> Result<Vec<Value>, String> {
        self.ensure_transport().map_err(|error| error.message)?;
        let transport = self.transport.as_mut().ok_or("会话目录桥接未连接")?;
        let result = read_directory(transport);
        self.finish_read(result)
    }

    /// Both public readers share exactly one startup, backoff and resource gate.
    fn ensure_transport(&mut self) -> Result<(), AdmissionError> {
        self.ensure_transport_with(Transport::start)
    }

    fn ensure_transport_with(
        &mut self,
        start: impl FnOnce(
            Arc<AtomicBool>,
            Option<&Path>,
            Arc<AtomicU8>,
            Arc<BridgeDiagnostics>,
        ) -> Result<Transport, String>,
    ) -> Result<(), AdmissionError> {
        let deferred = |message: &str| AdmissionError {
            message: message.into(),
            attempted_at: None,
        };
        if self.cancel.load(Ordering::Relaxed) {
            self.shutdown();
            return Err(deferred("桥接正在退出"));
        }
        if let Some(error) = resource_error(self.resource_stop.load(Ordering::Acquire)) {
            self.shutdown();
            return Err(deferred(error));
        }
        let invalid_source = self
            .source_root
            .as_deref()
            .is_some_and(|root| !root.is_dir());
        if invalid_source {
            // An already-open service also belongs to this source. Reap it even
            // when a backoff gate takes precedence in the returned error.
            self.shutdown();
        }
        if self.next_attempt.is_some_and(|at| Instant::now() < at) {
            return Err(deferred(DEFERRED));
        }
        if invalid_source {
            return Err(deferred(SOURCE_UNAVAILABLE));
        }
        if self.transport.is_none() {
            let attempted_at = chrono::Utc::now().to_rfc3339();
            self.diagnostics.begin_startup();
            let started = start(
                self.cancel.clone(),
                self.source_root.as_deref(),
                self.resource_stop.clone(),
                self.diagnostics.clone(),
            );
            match started {
                Ok(transport) => self.transport = Some(transport),
                Err(error) => {
                    return Err(AdmissionError {
                        message: self.finish_read::<()>(Err(error)).unwrap_err(),
                        attempted_at: Some(attempted_at),
                    })
                }
            }
        }
        Ok(())
    }

    fn finish_read<T>(&mut self, result: Result<T, String>) -> Result<T, String> {
        // The monitor can stop a job between receipt of a response and this boundary.
        let result = match resource_error(self.resource_stop.load(Ordering::Acquire)) {
            Some(error) => Err(error.into()),
            None => result,
        };
        match result {
            Ok(value) => {
                self.failure_count = 0;
                self.next_attempt = None;
                Ok(value)
            }
            Err(error) => {
                if error != UNSUPPORTED {
                    self.mark_unavailable(error.clone());
                    self.shutdown();
                    self.failure_count = self.failure_count.saturating_add(1);
                    let seconds = 15u64
                        .saturating_mul(1 << self.failure_count.min(6))
                        .min(900);
                    self.next_attempt = Some(Instant::now() + Duration::from_secs(seconds));
                }
                Err(error)
            }
        }
    }

    fn mark_unavailable(&mut self, error: String) {
        self.snapshot.status = if self.snapshot.collected_at.is_some() {
            "stale"
        } else {
            "unavailable"
        }
        .into();
        self.snapshot.error = Some(error);
    }

    pub fn shutdown(&mut self) {
        self.transport.take();
    }
}

impl Drop for AccountBridge {
    fn drop(&mut self) {
        self.shutdown();
    }
}

trait ReadRpc {
    fn read(&mut self, method: &str, params: Value) -> Result<Value, String>;
}

fn read_account_snapshot(
    rpc: &mut impl ReadRpc,
    previous: &mut AccountSnapshot,
    observation: &mut Option<String>,
) -> Result<AccountSnapshot, String> {
    let account = rpc.read("account/read", json!({"refreshToken":false}))?;
    let Some(identity) = account.get("account").filter(|v| !v.is_null()) else {
        *observation = None;
        *previous = AccountSnapshot::default();
        return Ok(AccountSnapshot {
            collected_at: Some(chrono::Utc::now().to_rfc3339()),
            error: Some("Codex 未登录或来源未提供账户".into()),
            ..AccountSnapshot::default()
        });
    };
    // An email/plan observation can invalidate cached values. It cannot prove an account,
    // organization or subscription identity and is never exported as account_scope.
    let seen = format!("{:x}", Sha256::digest(json!({"type":identity.get("type"),"id":identity.get("id"),"accountId":identity.get("accountId"),"email":identity.get("email"),"planType":identity.get("planType"),"organizationId":identity.get("organizationId"),"subscriptionId":identity.get("subscriptionId")}).to_string().as_bytes()));
    let scope = account_scope(identity);
    if observation.as_deref() != Some(&seen) {
        *previous = AccountSnapshot {
            last_attempt_at: previous.last_attempt_at.clone(),
            account_scope: scope.clone(),
            ..AccountSnapshot::default()
        };
        *observation = Some(seen);
    }
    let limits = rpc.read("account/rateLimits/read", json!({}))?;
    let (usage, mut warning) = match rpc.read("account/usage/read", json!({})) {
        Ok(value) => (Some(sanitize_usage(&value)), None),
        Err(error) if error == UNSUPPORTED => (
            None,
            Some("当前 Codex 不支持账户日趋势；额度快照已采集".into()),
        ),
        Err(error) if error == RPC_REJECTED => (
            None,
            Some("账户日趋势被拒绝或暂不可用；本次有效额度快照已采集".into()),
        ),
        Err(error) => return Err(error),
    };
    let buckets = sanitize_limits(&limits);
    if buckets.is_empty()
        || buckets
            .iter()
            .all(|b| b.primary.is_none() && b.secondary.is_none())
    {
        warning = Some("来源未提供有效额度窗口；未将缺失值补为零".into());
    }
    Ok(AccountSnapshot {
        status: "connected".into(),
        collected_at: Some(chrono::Utc::now().to_rfc3339()),
        last_attempt_at: None,
        error: warning,
        auth_mode: identity
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_owned),
        account_scope: scope,
        buckets,
        usage,
    })
}

fn account_scope(account: &Value) -> Option<String> {
    let id = account
        .get("accountId")
        .or_else(|| account.get("id"))?
        .as_str()
        .filter(|s| !s.is_empty())?;
    let organization = account.get("organizationId").and_then(Value::as_str);
    let subscription = account.get("subscriptionId").and_then(Value::as_str);
    if organization.is_none() && subscription.is_none() {
        return None;
    }
    let identity = json!({"type":account.get("type"),"id":id,"organization":organization,"subscription":subscription});
    Some(format!(
        "account-{:x}",
        Sha256::digest(identity.to_string().as_bytes())
    ))
}

fn read_directory(rpc: &mut impl ReadRpc) -> Result<Vec<Value>, String> {
    let mut sessions = Vec::new();
    for archived in [false, true] {
        let mut cursor: Option<String> = None;
        let mut seen_cursors = std::collections::HashSet::new();
        for page_index in 0..2000 {
            let page = rpc.read("thread/list", json!({
                "useStateDbOnly":true,"archived":archived,"cursor":cursor,"limit":50,
                "sourceKinds":["cli","vscode","exec","appServer","subAgent","subAgentReview","subAgentCompact","subAgentThreadSpawn","subAgentOther","unknown"],
                "modelProviders":[],"sortKey":"created_at","sortDirection":"desc"
            }))?;
            let rows = page
                .get("data")
                .and_then(Value::as_array)
                .ok_or("会话目录格式不支持")?;
            for row in rows {
                let Some(id) = row
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| uuid::Uuid::parse_str(id).is_ok())
                else {
                    continue;
                };
                sessions.push(json!({
                    "id":id,"title":safe_text(row.get("name"),240),
                    "cwd":safe_text(row.get("cwd"),4096),"parentThreadId":row.get("parentThreadId").and_then(Value::as_str).filter(|id|uuid::Uuid::parse_str(id).is_ok()),
                    "provider":safe_text(row.get("modelProvider"),120),"cliVersion":safe_text(row.get("cliVersion"),80),
                    "source":directory_source(row.get("source")),
                    "threadSource":directory_thread_source(row.get("threadSource")),
                    // cliVersion describes thread creation. The free-form initialize
                    // userAgent is not a verified RPC protocol version binding.
                    "directoryProtocolVersion":Value::Null,
                    "sessionId":safe_text(row.get("sessionId"),120),"archived":archived
                }));
                if sessions.len() > 100_000 {
                    return Err("会话目录超出本地安全上限，未声明完整覆盖".into());
                }
            }
            cursor = page
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_owned);
            match &cursor {
                None => break,
                Some(value) if !seen_cursors.insert(value.clone()) => {
                    return Err("会话目录分页游标重复，未声明完整覆盖".into())
                }
                Some(_) if page_index == 1999 => {
                    return Err("会话目录分页超过安全上限，未声明完整覆盖".into())
                }
                _ => {}
            }
        }
    }
    Ok(sessions)
}

fn directory_thread_source(value: Option<&Value>) -> Value {
    match value.and_then(Value::as_str) {
        Some(source @ ("user" | "subagent" | "memory_consolidation")) => json!(source),
        Some(_) => json!("unsupported"),
        None => Value::Null,
    }
}

// Keep only relationship evidence. Nicknames, paths, custom strings and any
// unknown object members never enter the persisted directory projection.
fn directory_source(value: Option<&Value>) -> Value {
    let Some(value) = value else {
        return Value::Null;
    };
    if let Some(source) = value.as_str() {
        return match source {
            "cli" | "vscode" | "exec" | "appServer" | "unknown" => json!(source),
            _ => Value::Null,
        };
    }
    let Some(object) = value.as_object().filter(|object| object.len() == 1) else {
        return Value::Null;
    };
    let Some(subagent) = object.get("subAgent") else {
        return Value::Null;
    };
    if let Some(kind @ ("review" | "compact" | "memory_consolidation")) = subagent.as_str() {
        return json!({"subAgent":kind});
    }
    let Some(kind) = subagent.as_object().filter(|object| object.len() == 1) else {
        return Value::Null;
    };
    if kind.get("other").and_then(Value::as_str).is_some() {
        return json!({"subAgent":{"other":"observed"}});
    }
    let Some(spawn) = kind.get("thread_spawn").and_then(Value::as_object) else {
        return Value::Null;
    };
    let parent = spawn
        .get("parent_thread_id")
        .and_then(Value::as_str)
        .filter(|id| uuid::Uuid::parse_str(id).is_ok());
    let depth = spawn
        .get("depth")
        .and_then(Value::as_i64)
        .filter(|depth| i32::try_from(*depth).is_ok());
    match (parent, depth) {
        (Some(parent), Some(depth)) => json!({"subAgent":{"thread_spawn":{
            "parent_thread_id":parent,"depth":depth
        }}}),
        _ => Value::Null,
    }
}

fn safe_text(value: Option<&Value>, max: usize) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(|s| s.chars().filter(|c| !c.is_control()).take(max).collect())
}

fn quota_window(value: Option<&Value>) -> Option<QuotaWindow> {
    let value = value?;
    let used_percent = value.get("usedPercent")?.as_f64()?;
    if !used_percent.is_finite() || !(0.0..=100.0).contains(&used_percent) {
        return None;
    }
    Some(QuotaWindow {
        used_percent,
        window_duration_mins: value
            .get("windowDurationMins")
            .and_then(Value::as_u64)
            .filter(|n| *n > 0),
        resets_at: value
            .get("resetsAt")
            .and_then(Value::as_i64)
            .filter(|n| *n >= 0),
    })
}

fn quota_bucket(id: &str, value: &Value) -> QuotaBucket {
    QuotaBucket {
        id: id.chars().take(120).collect(),
        name: value
            .get("limitName")
            .and_then(Value::as_str)
            .map(|s| s.chars().take(120).collect()),
        primary: quota_window(value.get("primary")),
        secondary: quota_window(value.get("secondary")),
    }
}

fn sanitize_limits(value: &Value) -> Vec<QuotaBucket> {
    if let Some(buckets) = value.get("rateLimitsByLimitId").and_then(Value::as_object) {
        return buckets
            .iter()
            .take(100)
            .map(|(id, value)| quota_bucket(id, value))
            .collect();
    }
    value
        .get("rateLimits")
        .filter(|v| !v.is_null())
        .map(|v| {
            vec![quota_bucket(
                v.get("limitId").and_then(Value::as_str).unwrap_or("codex"),
                v,
            )]
        })
        .unwrap_or_default()
}

fn sanitize_usage(value: &Value) -> Value {
    let mut summary = serde_json::Map::new();
    for key in [
        "lifetimeTokens",
        "peakDailyTokens",
        "longestRunningTurnSec",
        "currentStreakDays",
        "longestStreakDays",
    ] {
        summary.insert(
            key.into(),
            value
                .pointer(&format!("/summary/{key}"))
                .and_then(Value::as_u64)
                .map_or(Value::Null, Value::from),
        );
    }
    let buckets = value
        .get("dailyUsageBuckets")
        .and_then(Value::as_array)
        .map(|items| {
            items.iter().take(4000).filter_map(|item| {
            let date = item.get("startDate")?.as_str()?;
            chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
            Some(json!({"startDate":date,"tokens":item.get("tokens").and_then(Value::as_u64)}))
        }).collect::<Vec<_>>()
        });
    json!({"summary":summary,"dailyUsageBuckets":buckets})
}

type BridgeStdin = std::fs::File;

struct Transport {
    #[cfg(windows)]
    child: win_process::OwnedProcess,
    stdin: Option<BridgeStdin>,
    messages: Option<Receiver<Result<Value, String>>>,
    reader: Option<std::thread::JoinHandle<()>>,
    next_id: u64,
    cancel: Arc<AtomicBool>,
    resource_stop: Arc<AtomicU8>,
    diagnostics: Arc<BridgeDiagnostics>,
    unsupported: HashSet<String>,
}

impl Transport {
    #[cfg(not(windows))]
    fn start(
        _cancel: Arc<AtomicBool>,
        _source_root: Option<&Path>,
        _resource_stop: Arc<AtomicU8>,
        _diagnostics: Arc<BridgeDiagnostics>,
    ) -> Result<Self, String> {
        Err("本版本账户桥接仅支持具有 Job 提交内存保护的 Windows；该能力暂不可用".into())
    }

    #[cfg(windows)]
    fn start(
        cancel: Arc<AtomicBool>,
        source_root: Option<&Path>,
        resource_stop: Arc<AtomicU8>,
        diagnostics: Arc<BridgeDiagnostics>,
    ) -> Result<Self, String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("桥接正在退出".into());
        }
        let executable = find_codex().ok_or("未找到 codex.exe；请安装 Codex 或将其加入 PATH")?;
        let (child, stdin, stdout) = win_process::OwnedProcess::start(
            &executable,
            source_root,
            cancel.clone(),
            resource_stop.clone(),
            diagnostics.clone(),
        )?;
        let (tx, messages) = mpsc::sync_channel(64);
        let reader = std::thread::Builder::new()
            .name("bridge-stdout".into())
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                loop {
                    let result = read_message(&mut reader);
                    let done = result.is_err();
                    if tx.send(result).is_err() || done {
                        break;
                    }
                }
            })
            .map_err(|_| "无法建立桥接读取线程")?;
        let mut transport = Self {
            child,
            stdin: Some(stdin),
            messages: Some(messages),
            reader: Some(reader),
            next_id: 0,
            cancel,
            resource_stop,
            diagnostics,
            unsupported: HashSet::new(),
        };
        let initialized = transport
            .request(
                "initialize",
                json!({
                    "clientInfo":{"name":"token_miao","title":"Token 喵","version":"0.1.0"},
                    "capabilities":{"experimentalApi":true}
                }),
            )
            .map_err(|error| {
                if error == UNSUPPORTED {
                    "当前 Codex 不支持初始化协议".into()
                } else {
                    error
                }
            })?;
        if initialized
            .get("userAgent")
            .and_then(Value::as_str)
            .is_none()
            || initialized
                .get("platformFamily")
                .and_then(Value::as_str)
                .is_none()
        {
            return Err("Codex 握手版本或能力格式不受支持".into());
        }
        transport.write(&json!({"method":"initialized","params":{}}))?;
        transport.diagnostics.set_stage(BridgeStage::Idle);
        Ok(transport)
    }

    fn write(&mut self, message: &Value) -> Result<(), String> {
        let stdin = self.stdin.as_mut().ok_or("账户桥接连接已关闭")?;
        serde_json::to_writer(&mut *stdin, message).map_err(|_| "账户协议写入失败")?;
        stdin
            .write_all(b"\n")
            .and_then(|_| stdin.flush())
            .map_err(|_| "账户桥接连接中断".into())
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        if !ALLOWED.contains(&method) {
            return Err("拒绝非只读账户方法".into());
        }
        if self.unsupported.contains(method) {
            return Err(UNSUPPORTED.into());
        }
        if self.cancel.load(Ordering::Relaxed) {
            return Err("桥接正在退出".into());
        }
        if let Some(error) = resource_error(self.resource_stop.load(Ordering::Acquire)) {
            return Err(error.into());
        }
        if method == "account/read" && params.get("refreshToken") != Some(&Value::Bool(false)) {
            return Err("拒绝刷新认证信息".into());
        }
        if method == "thread/list" && params.get("useStateDbOnly") != Some(&Value::Bool(true)) {
            return Err("拒绝扫描修复会话目录".into());
        }
        if let Some(stage) = BridgeStage::for_method(method) {
            self.diagnostics.set_stage(stage);
        }
        self.next_id += 1;
        let id = self.next_id;
        self.write(&json!({"id":id,"method":method,"params":params}))?;
        let result = await_response_with_resource(
            self.messages.as_ref().ok_or("桥接读取已关闭")?,
            id,
            &self.cancel,
            REQUEST_TIMEOUT,
            Some(&self.resource_stop),
        );
        if method != "initialize" {
            self.diagnostics.set_stage(BridgeStage::Idle);
        }
        if result
            .as_ref()
            .err()
            .is_some_and(|error| error == UNSUPPORTED)
        {
            self.unsupported.insert(method.into());
        }
        result
    }
}

fn await_response(
    messages: &Receiver<Result<Value, String>>,
    id: u64,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<Value, String> {
    await_response_with_resource(messages, id, cancel, timeout, None)
}

fn await_response_with_resource(
    messages: &Receiver<Result<Value, String>>,
    id: u64,
    cancel: &AtomicBool,
    timeout: Duration,
    resource: Option<&AtomicU8>,
) -> Result<Value, String> {
    let deadline = Instant::now() + timeout;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("桥接正在退出".into());
        }
        if let Some(error) = resource.and_then(|r| resource_error(r.load(Ordering::Acquire))) {
            return Err(error.into());
        }
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or("账户读取超时，稍后重试")?;
        let message = match messages.recv_timeout(remaining.min(Duration::from_millis(100))) {
            Ok(message) => message?,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => return Err("账户读取连接中断".into()),
        };
        if message.get("id").and_then(Value::as_u64) != Some(id) {
            continue;
        }
        if message.get("error").is_some() {
            let code = message.pointer("/error/code").and_then(Value::as_i64);
            return Err(match code {
                Some(-32601) => UNSUPPORTED,
                _ => RPC_REJECTED,
            }
            .into());
        }
        return message
            .get("result")
            .cloned()
            .ok_or("账户响应缺少结果".into());
    }
}

impl ReadRpc for Transport {
    fn read(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.request(method, params)
    }
}

impl Drop for Transport {
    fn drop(&mut self) {
        self.stdin.take();
        // Close the receiving endpoint before joining: a full bounded channel must
        // never keep the stdout reader alive during shutdown.
        self.messages.take();
        #[cfg(windows)]
        self.child.terminate();
        if let Some(reader) = self.reader.take() {
            #[cfg(windows)]
            unsafe {
                use std::os::windows::io::AsRawHandle;
                // Interrupt any synchronous pipe read; the thread owns and closes
                // its read handle on return. No foreign thread is targeted.
                windows_sys::Win32::System::IO::CancelSynchronousIo(reader.as_raw_handle());
            }
            let _ = reader.join();
        }
    }
}

fn find_codex() -> Option<PathBuf> {
    let binary = if cfg!(windows) { "codex.exe" } else { "codex" };
    let mut candidates = std::env::var_os("PATH")
        .map(|p| {
            std::env::split_paths(&p)
                .map(|p| p.join(binary))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local).join("Programs/OpenAI/Codex/bin/codex.exe"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn read_message(reader: &mut impl BufRead) -> Result<Value, String> {
    let mut bytes = Vec::new();
    loop {
        let buf = reader.fill_buf().map_err(|_| "账户协议读取失败")?;
        if buf.is_empty() {
            return Err("账户桥接已退出".into());
        }
        let count = buf
            .iter()
            .position(|b| *b == b'\n')
            .map_or(buf.len(), |n| n + 1);
        if bytes.len() + count > MAX_MESSAGE {
            return Err("账户协议消息超过限制".into());
        }
        bytes.extend_from_slice(&buf[..count]);
        reader.consume(count);
        if bytes.last() == Some(&b'\n') {
            break;
        }
    }
    serde_json::from_slice(&bytes).map_err(|_| "账户协议格式不受支持".into())
}

// Windows is the supported desktop target. The process cannot execute user code
// until the configured job, IOCP and handle inheritance allowlist are installed.
#[cfg(windows)]
mod win_process {
    use super::*;
    use std::{
        ffi::{OsStr, OsString},
        fs::File,
        os::windows::{
            ffi::OsStrExt,
            io::{AsRawHandle, FromRawHandle, OwnedHandle},
        },
        ptr,
    };
    use windows_sys::Win32::{
        Foundation::{
            SetHandleInformation, FILETIME, GENERIC_WRITE, HANDLE, HANDLE_FLAG_INHERIT,
            INVALID_HANDLE_VALUE,
        },
        Security::SECURITY_ATTRIBUTES,
        Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        },
        System::{
            JobObjects::*,
            Pipes::CreatePipe,
            SystemServices::{JOB_OBJECT_MSG_JOB_MEMORY_LIMIT, JOB_OBJECT_MSG_NOTIFICATION_LIMIT},
            Threading::*,
            IO::*,
        },
    };
    pub(super) const HARD_COMMIT: usize = 128 * 1024 * 1024;
    pub(super) const WARNING_COMMIT: usize = 96 * 1024 * 1024;

    pub(super) fn memory_observation(
        violation: &JOBOBJECT_LIMIT_VIOLATION_INFORMATION,
    ) -> BridgeLimitObservation {
        BridgeLimitObservation {
            job_memory: (violation.ViolationLimitFlags & JOB_OBJECT_LIMIT_JOB_MEMORY != 0)
                .then_some(violation.JobMemory),
            job_memory_limit: (violation.LimitFlags & JOB_OBJECT_LIMIT_JOB_MEMORY != 0)
                .then_some(violation.JobMemoryLimit),
            limit_flags: violation.LimitFlags,
            violation_limit_flags: violation.ViolationLimitFlags,
        }
    }

    fn failure(action: &str) -> String {
        format!(
            "{action}（Windows 错误 {}）",
            std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
        )
    }
    unsafe fn own(raw: HANDLE, action: &str) -> Result<OwnedHandle, String> {
        if raw.is_null() || raw == INVALID_HANDLE_VALUE {
            Err(failure(action))
        } else {
            Ok(OwnedHandle::from_raw_handle(raw))
        }
    }
    fn wide(value: &OsStr) -> Result<Vec<u16>, String> {
        let mut result: Vec<u16> = value.encode_wide().collect();
        if result.contains(&0) {
            return Err("桥接启动参数含无效字符".into());
        }
        result.push(0);
        Ok(result)
    }
    fn quote(value: &OsStr) -> Result<Vec<u16>, String> {
        let value = wide(value)?;
        let mut result = vec![b'"' as u16];
        let mut slashes = 0;
        for &unit in &value[..value.len() - 1] {
            if unit == b'\\' as u16 {
                slashes += 1;
                continue;
            }
            result.extend(std::iter::repeat_n(
                b'\\' as u16,
                if unit == b'"' as u16 {
                    slashes * 2 + 1
                } else {
                    slashes
                },
            ));
            slashes = 0;
            result.push(unit);
        }
        result.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
        result.push(b'"' as u16);
        Ok(result)
    }
    fn environment(overrides: &[(OsString, OsString)]) -> Result<Vec<u16>, String> {
        let mut vars = merge_bridge_environment(std::env::vars_os(), overrides);
        vars.sort_by_cached_key(|(key, _)| key.to_string_lossy().to_uppercase());
        let mut block = Vec::new();
        for (key, value) in vars {
            let key = wide(&key)?;
            let value = wide(&value)?;
            block.extend_from_slice(&key[..key.len() - 1]);
            block.push(b'=' as u16);
            block.extend(value);
        }
        block.push(0);
        if block.len() == 1 {
            block.push(0);
        }
        Ok(block)
    }
    fn pipe() -> Result<(OwnedHandle, OwnedHandle), String> {
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: ptr::null_mut(),
            bInheritHandle: 1,
        };
        let (mut read, mut write) = (ptr::null_mut(), ptr::null_mut());
        unsafe {
            if CreatePipe(&mut read, &mut write, &attributes, 0) == 0 {
                return Err(failure("无法建立桥接管道"));
            }
            // Both handles exist whenever CreatePipe succeeds, and are immediately RAII owned.
            Ok((
                own(read, "桥接管道读取句柄无效")?,
                own(write, "桥接管道写入句柄无效")?,
            ))
        }
    }
    struct Attributes {
        storage: Vec<usize>,
        initialized: bool,
    }
    impl Attributes {
        fn handles(handles: &[HANDLE]) -> Result<Self, String> {
            let mut bytes = 0;
            unsafe {
                InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut bytes);
            }
            if bytes == 0 || bytes > 65536 {
                return Err("桥接句柄白名单大小无效".into());
            }
            let mut value = Self {
                storage: vec![0; bytes.div_ceil(std::mem::size_of::<usize>())],
                initialized: false,
            };
            unsafe {
                if InitializeProcThreadAttributeList(value.ptr(), 1, 0, &mut bytes) == 0 {
                    return Err(failure("无法建立桥接句柄白名单"));
                }
                value.initialized = true;
                if UpdateProcThreadAttribute(
                    value.ptr(),
                    0,
                    PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                    handles.as_ptr().cast(),
                    std::mem::size_of_val(handles),
                    ptr::null_mut(),
                    ptr::null(),
                ) == 0
                {
                    return Err(failure("无法设置桥接句柄白名单"));
                }
            }
            Ok(value)
        }
        fn ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
            self.storage.as_mut_ptr().cast()
        }
    }
    impl Drop for Attributes {
        fn drop(&mut self) {
            if self.initialized {
                unsafe {
                    DeleteProcThreadAttributeList(self.ptr());
                }
            }
        }
    }

    pub(super) struct Job {
        handle: Arc<OwnedHandle>,
        port: Arc<OwnedHandle>,
        stop: Arc<AtomicBool>,
        monitor: Option<std::thread::JoinHandle<()>>,
    }
    impl Job {
        fn new(
            cancel: Arc<AtomicBool>,
            resource: Arc<AtomicU8>,
            diagnostics: Arc<BridgeDiagnostics>,
            hard: usize,
            warning: Option<usize>,
        ) -> Result<Self, String> {
            unsafe {
                let handle = Arc::new(own(
                    CreateJobObjectW(ptr::null(), ptr::null()),
                    "无法建立桥接 Job",
                )?);
                let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                // Omitting BREAKAWAY_OK and SILENT_BREAKAWAY_OK also fences descendants.
                limits.BasicLimitInformation.LimitFlags =
                    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_JOB_MEMORY;
                limits.JobMemoryLimit = hard;
                if SetInformationJobObject(
                    handle.as_raw_handle(),
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of_val(&limits) as u32,
                ) == 0
                {
                    return Err(failure("无法设置桥接硬提交内存预算"));
                }
                let port = Arc::new(own(
                    CreateIoCompletionPort(INVALID_HANDLE_VALUE, ptr::null_mut(), 0, 1),
                    "无法建立桥接预算通知端口",
                )?);
                let association = JOBOBJECT_ASSOCIATE_COMPLETION_PORT {
                    CompletionKey: 1usize as *mut _,
                    CompletionPort: port.as_raw_handle(),
                };
                if SetInformationJobObject(
                    handle.as_raw_handle(),
                    JobObjectAssociateCompletionPortInformation,
                    (&association as *const JOBOBJECT_ASSOCIATE_COMPLETION_PORT).cast(),
                    std::mem::size_of_val(&association) as u32,
                ) == 0
                {
                    return Err(failure("无法绑定桥接预算通知"));
                }
                if let Some(warning) = warning {
                    let notice = JOBOBJECT_NOTIFICATION_LIMIT_INFORMATION {
                        JobMemoryLimit: warning as u64,
                        LimitFlags: JOB_OBJECT_LIMIT_JOB_MEMORY,
                        ..Default::default()
                    };
                    if SetInformationJobObject(
                        handle.as_raw_handle(),
                        JobObjectNotificationLimitInformation,
                        (&notice as *const JOBOBJECT_NOTIFICATION_LIMIT_INFORMATION).cast(),
                        std::mem::size_of_val(&notice) as u32,
                    ) == 0
                    {
                        return Err(failure("无法设置桥接提交内存预警"));
                    }
                }
                let stop = Arc::new(AtomicBool::new(false));
                let (h, p, done) = (handle.clone(), port.clone(), stop.clone());
                let monitor = std::thread::Builder::new()
                    .name("bridge-budget".into())
                    .spawn(move || {
                        while !done.load(Ordering::Acquire) {
                            if cancel.load(Ordering::Relaxed) {
                                TerminateJobObject(h.as_raw_handle(), 1);
                                break;
                            }
                            let (mut message, mut key, mut process) =
                                (0u32, 0usize, ptr::null_mut());
                            let ok = GetQueuedCompletionStatus(
                                p.as_raw_handle(),
                                &mut message,
                                &mut key,
                                &mut process,
                                100,
                            );
                            if done.load(Ordering::Acquire) {
                                break;
                            }
                            let dequeue_error = (ok == 0).then(|| {
                                std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
                            });
                            if dequeue_error == Some(258) {
                                continue;
                            }
                            if ok != 0 && key != 1 {
                                continue;
                            }
                            let observed_stage = diagnostics.observed_stage();
                            let observed_at = chrono::Utc::now();
                            let observed_instant = Instant::now();
                            let mut query_succeeded = None;
                            let mut monitor_error = dequeue_error;
                            let mut observed_violation = None;
                            let reason = if ok == 0 {
                                RESOURCE_MONITOR_FAILED
                            } else if message == JOB_OBJECT_MSG_JOB_MEMORY_LIMIT {
                                RESOURCE_HARD_LIMIT
                            } else if message == JOB_OBJECT_MSG_NOTIFICATION_LIMIT {
                                let mut violation: JOBOBJECT_LIMIT_VIOLATION_INFORMATION =
                                    std::mem::zeroed();
                                if QueryInformationJobObject(
                                    h.as_raw_handle(),
                                    JobObjectLimitViolationInformation,
                                    (&mut violation as *mut JOBOBJECT_LIMIT_VIOLATION_INFORMATION)
                                        .cast(),
                                    std::mem::size_of_val(&violation) as u32,
                                    ptr::null_mut(),
                                ) == 0
                                {
                                    query_succeeded = Some(false);
                                    monitor_error = Some(
                                        std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
                                    );
                                    RESOURCE_MONITOR_FAILED
                                } else {
                                    query_succeeded = Some(true);
                                    observed_violation = Some(memory_observation(&violation));
                                    if violation.ViolationLimitFlags & JOB_OBJECT_LIMIT_JOB_MEMORY
                                        != 0
                                    {
                                        RESOURCE_WARNING
                                    } else {
                                        continue;
                                    }
                                }
                            } else {
                                continue;
                            };
                            let first = resource
                                .compare_exchange(0, reason, Ordering::AcqRel, Ordering::Acquire)
                                .is_ok();
                            // Allocation denial is not a kill policy. Explicitly stop the entire owned job.
                            let terminated = TerminateJobObject(h.as_raw_handle(), 1) != 0;
                            // The stop and existing termination precede all diagnostic locking.
                            if first {
                                diagnostics.record_resource_event(
                                    reason,
                                    observed_stage,
                                    observed_at,
                                    observed_instant,
                                    (ok != 0).then_some(message),
                                    query_succeeded,
                                    monitor_error,
                                    observed_violation,
                                );
                                diagnostics.record_monitor_cleanup(terminated);
                            }
                            break;
                        }
                    })
                    .map_err(|_| "无法建立桥接预算监视线程")?;
                Ok(Self {
                    handle,
                    port,
                    stop,
                    monitor: Some(monitor),
                })
            }
        }
        fn terminate(&mut self) -> bool {
            let terminated = unsafe { TerminateJobObject(self.handle.as_raw_handle(), 1) != 0 };
            self.stop.store(true, Ordering::Release);
            unsafe {
                PostQueuedCompletionStatus(self.port.as_raw_handle(), 0, 0, ptr::null());
            }
            if let Some(monitor) = self.monitor.take() {
                let _ = monitor.join();
            }
            terminated
        }
    }
    impl Drop for Job {
        fn drop(&mut self) {
            self.terminate();
        }
    }

    pub(super) struct OwnedProcess {
        pub(super) process: OwnedHandle,
        pub(super) primary: Option<OwnedHandle>,
        pub(super) job: Job,
        cancel: Arc<AtomicBool>,
        resource: Arc<AtomicU8>,
        diagnostics: Arc<BridgeDiagnostics>,
        identity: BridgeRootIdentity,
    }
    impl OwnedProcess {
        pub(super) fn start(
            executable: &Path,
            root: Option<&Path>,
            cancel: Arc<AtomicBool>,
            resource: Arc<AtomicU8>,
            diagnostics: Arc<BridgeDiagnostics>,
        ) -> Result<(Self, File, File), String> {
            let overrides = root
                .map(|r| vec![(OsString::from("CODEX_HOME"), r.as_os_str().to_owned())])
                .unwrap_or_default();
            let (mut process, stdin, stdout) = Self::suspended(
                executable,
                &[
                    OsString::from("app-server"),
                    OsString::from("--listen"),
                    OsString::from("stdio://"),
                ],
                &overrides,
                cancel,
                resource,
                diagnostics,
                HARD_COMMIT,
                Some(WARNING_COMMIT),
            )?;
            process.resume()?;
            Ok((process, stdin, stdout))
        }
        fn suspended(
            executable: &Path,
            args: &[OsString],
            overrides: &[(OsString, OsString)],
            cancel: Arc<AtomicBool>,
            resource: Arc<AtomicU8>,
            diagnostics: Arc<BridgeDiagnostics>,
            hard: usize,
            warning: Option<usize>,
        ) -> Result<(Self, File, File), String> {
            if cancel.load(Ordering::Relaxed) {
                return Err("桥接正在退出".into());
            }
            let job = Job::new(
                cancel.clone(),
                resource.clone(),
                diagnostics.clone(),
                hard,
                warning,
            )?;
            let (stdin_read, stdin_write) = pipe()?;
            let (stdout_read, stdout_write) = pipe()?;
            unsafe {
                if SetHandleInformation(stdin_write.as_raw_handle(), HANDLE_FLAG_INHERIT, 0) == 0
                    || SetHandleInformation(stdout_read.as_raw_handle(), HANDLE_FLAG_INHERIT, 0)
                        == 0
                {
                    return Err(failure("无法隔离桥接父端管道"));
                }
            }
            let attributes = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: ptr::null_mut(),
                bInheritHandle: 1,
            };
            let null_path = wide(OsStr::new("NUL"))?;
            let stderr = unsafe {
                own(
                    CreateFileW(
                        null_path.as_ptr(),
                        GENERIC_WRITE,
                        FILE_SHARE_READ | FILE_SHARE_WRITE,
                        &attributes,
                        OPEN_EXISTING,
                        FILE_ATTRIBUTE_NORMAL,
                        ptr::null_mut(),
                    ),
                    "无法建立桥接错误输出",
                )?
            };
            let handles = [
                stdin_read.as_raw_handle(),
                stdout_write.as_raw_handle(),
                stderr.as_raw_handle(),
            ];
            let mut allowlist = Attributes::handles(&handles)?;
            let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
            startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
            startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            startup.StartupInfo.hStdInput = handles[0];
            startup.StartupInfo.hStdOutput = handles[1];
            startup.StartupInfo.hStdError = handles[2];
            startup.lpAttributeList = allowlist.ptr();
            let application = wide(executable.as_os_str())?;
            let mut command = quote(executable.as_os_str())?;
            for arg in args {
                command.push(b' ' as u16);
                command.extend(quote(arg)?);
            }
            command.push(0);
            let environment = environment(overrides)?;
            let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
            unsafe {
                if CreateProcessW(
                    application.as_ptr(),
                    command.as_mut_ptr(),
                    ptr::null(),
                    ptr::null(),
                    1,
                    CREATE_SUSPENDED
                        | CREATE_NO_WINDOW
                        | CREATE_UNICODE_ENVIRONMENT
                        | EXTENDED_STARTUPINFO_PRESENT,
                    environment.as_ptr().cast(),
                    ptr::null(),
                    &startup.StartupInfo,
                    &mut info,
                ) == 0
                {
                    return Err(failure("无法挂起创建桥接进程"));
                }
                if info.hProcess.is_null() || info.hThread.is_null() {
                    // A process handle alone does not terminate an unassigned child.
                    if !info.hProcess.is_null() {
                        TerminateProcess(info.hProcess, 1);
                        windows_sys::Win32::Foundation::CloseHandle(info.hProcess);
                    }
                    if !info.hThread.is_null() {
                        windows_sys::Win32::Foundation::CloseHandle(info.hThread);
                    }
                    return Err("挂起桥接进程返回了无效句柄，已取消启动".into());
                }
                let process_handle = own(info.hProcess, "桥接进程句柄无效")?;
                let primary_handle = own(info.hThread, "桥接主线程句柄无效")?;
                let mut created: FILETIME = std::mem::zeroed();
                let mut exited: FILETIME = std::mem::zeroed();
                let mut kernel: FILETIME = std::mem::zeroed();
                let mut user: FILETIME = std::mem::zeroed();
                let creation_query_succeeded = GetProcessTimes(
                    process_handle.as_raw_handle(),
                    &mut created,
                    &mut exited,
                    &mut kernel,
                    &mut user,
                ) != 0;
                let identity = BridgeRootIdentity {
                    pid: info.dwProcessId,
                    creation_filetime: creation_query_succeeded.then_some(
                        ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64,
                    ),
                    creation_query_error: (!creation_query_succeeded)
                        .then(|| std::io::Error::last_os_error().raw_os_error().unwrap_or(0)),
                };
                diagnostics.set_root(identity);
                let mut process = Self {
                    process: process_handle,
                    primary: Some(primary_handle),
                    job,
                    cancel,
                    resource,
                    diagnostics,
                    identity,
                };
                if AssignProcessToJobObject(
                    process.job.handle.as_raw_handle(),
                    process.process.as_raw_handle(),
                ) == 0
                {
                    let error = failure("无法在运行前将桥接绑定预算 Job");
                    process.terminate();
                    return Err(error);
                }
                // Parent copies of inheritable child endpoints close before resume.
                drop(stdin_read);
                drop(stdout_write);
                drop(stderr);
                drop(allowlist);
                Ok((process, File::from(stdin_write), File::from(stdout_read)))
            }
        }
        pub(super) fn resume(&mut self) -> Result<(), String> {
            if self.cancel.load(Ordering::Relaxed) {
                self.terminate();
                return Err("桥接正在退出".into());
            }
            if let Some(error) = resource_error(self.resource.load(Ordering::Acquire)) {
                self.terminate();
                return Err(error.into());
            }
            let thread = self.primary.take().ok_or("桥接主线程已恢复")?;
            unsafe {
                if ResumeThread(thread.as_raw_handle()) == u32::MAX {
                    let error = failure("无法恢复桥接主线程");
                    self.terminate();
                    return Err(error);
                }
            }
            Ok(())
        }
        pub(super) fn terminate(&mut self) {
            let job = self.job.terminate();
            let (process, wait) = unsafe {
                let process = TerminateProcess(self.process.as_raw_handle(), 1) != 0;
                let wait = WaitForSingleObject(self.process.as_raw_handle(), 1000);
                (process, wait)
            };
            self.primary.take();
            self.diagnostics
                .record_owner_cleanup(self.identity, job, process, wait);
        }
        #[cfg(test)]
        pub(super) fn test_suspended(
            args: &[OsString],
            overrides: &[(OsString, OsString)],
            cancel: Arc<AtomicBool>,
            resource: Arc<AtomicU8>,
            warning: Option<usize>,
        ) -> Result<(Self, File, File), String> {
            Self::suspended(
                &std::env::current_exe().map_err(|_| "测试进程路径不可用")?,
                args,
                overrides,
                cancel,
                resource,
                Arc::new(BridgeDiagnostics::new()),
                HARD_COMMIT,
                warning,
            )
        }
        #[cfg(test)]
        pub(super) fn job_handle(&self) -> HANDLE {
            self.job.handle.as_raw_handle()
        }
    }
    impl Drop for OwnedProcess {
        fn drop(&mut self) {
            self.terminate();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_environment_strips_only_listed_inherited_auth_and_task_keys() {
        let inherited = [
            ("codex_Home", "parent-source"),
            ("CodeX_ACCESS_token", "synthetic-token"),
            ("codex_api_KEY", "synthetic-key"),
            ("CODEX_app_TOOLS_pipe_PATH", "synthetic-pipe"),
            ("codex_INTERNAL_originator_OVERRIDE", "synthetic-originator"),
            ("CodeX_THREAD_id", "synthetic-thread"),
            ("codeX_session_ID", "synthetic-session"),
            ("CODEX_tool_CALL_id", "synthetic-call"),
            (
                "codex_TASK_workspace_VERIFYING_identity",
                "synthetic-identity",
            ),
            ("SystemRoot", "synthetic-windows"),
            ("PATH", "synthetic-path"),
            ("USERPROFILE", "synthetic-profile"),
            ("HOME", "synthetic-home"),
            ("TEMP", "synthetic-temp"),
            ("HTTPS_PROXY", "synthetic-proxy"),
            ("NO_PROXY", "synthetic-no-proxy"),
            ("SSL_CERT_FILE", "synthetic-ca-file"),
            ("CODEX_CA_CERTIFICATE", "synthetic-codex-ca"),
            ("CODEX_PERMISSION_PROFILE", "synthetic-policy"),
            ("CODEX_WINDOWS_SANDBOX_PACKAGE_FAMILY", "synthetic-sandbox"),
            (
                "CODEX_UNKNOWN_FUTURE_SECURITY_SETTING",
                "synthetic-security",
            ),
            ("CODEX_ACCESS_TOKEN_SUFFIX", "synthetic-near-match"),
        ]
        .map(|(key, value)| (key.into(), value.into()));
        let expected: Vec<(OsString, OsString)> = [
            ("SystemRoot", "synthetic-windows"),
            ("PATH", "synthetic-path"),
            ("USERPROFILE", "synthetic-profile"),
            ("HOME", "synthetic-home"),
            ("TEMP", "synthetic-temp"),
            ("HTTPS_PROXY", "synthetic-proxy"),
            ("NO_PROXY", "synthetic-no-proxy"),
            ("SSL_CERT_FILE", "synthetic-ca-file"),
            ("CODEX_CA_CERTIFICATE", "synthetic-codex-ca"),
            ("CODEX_PERMISSION_PROFILE", "synthetic-policy"),
            ("CODEX_WINDOWS_SANDBOX_PACKAGE_FAMILY", "synthetic-sandbox"),
            (
                "CODEX_UNKNOWN_FUTURE_SECURITY_SETTING",
                "synthetic-security",
            ),
            ("CODEX_ACCESS_TOKEN_SUFFIX", "synthetic-near-match"),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect();
        assert_eq!(merge_bridge_environment(inherited, &[]), expected);
    }

    #[test]
    fn bridge_environment_applies_selected_home_and_fixture_overrides_exactly() {
        let inherited = [
            ("codex_home", "parent-source-one"),
            ("CODEX_HOME", "parent-source-two"),
            ("PaTh", "parent-path"),
            ("token_meow_private_bridge_test_role", "parent-role"),
            ("CODEX_PERMISSION_PROFILE", "synthetic-policy"),
        ]
        .map(|(key, value)| (key.into(), value.into()));
        let overrides = [
            ("CODEX_HOME", "D:/synthetic-selected-source"),
            ("PATH", "synthetic-fixture-path"),
            ("TOKEN_MEOW_PRIVATE_BRIDGE_TEST_ROLE", "allocate"),
        ]
        .map(|(key, value)| (key.into(), value.into()));
        let expected: Vec<(OsString, OsString)> = [
            ("CODEX_PERMISSION_PROFILE", "synthetic-policy"),
            ("CODEX_HOME", "D:/synthetic-selected-source"),
            ("PATH", "synthetic-fixture-path"),
            ("TOKEN_MEOW_PRIVATE_BRIDGE_TEST_ROLE", "allocate"),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect();
        assert_eq!(merge_bridge_environment(inherited, &overrides), expected);
    }

    fn record_diagnostic(diagnostics: &BridgeDiagnostics, reason: u8) {
        diagnostics.record_resource_event(
            reason,
            diagnostics.observed_stage(),
            chrono::Utc::now(),
            Instant::now(),
            None,
            None,
            None,
            None,
        );
    }

    #[test]
    fn resource_stop_observation_is_authoritative_without_reads_or_fact_changes() {
        for reason in [
            RESOURCE_WARNING,
            RESOURCE_HARD_LIMIT,
            RESOURCE_MONITOR_FAILED,
        ] {
            let mut bridge = AccountBridge::with_source_root(Path::new("D:/fixture/source"));
            bridge.snapshot.status = "connected".into();
            bridge.snapshot.collected_at = Some("old-success".into());
            bridge.snapshot.last_attempt_at = Some("old-attempt".into());
            bridge.snapshot.auth_mode = Some("old-auth-mode".into());
            bridge.snapshot.account_scope = Some("old-scope".into());
            bridge.snapshot.buckets.push(QuotaBucket {
                id: "old-bucket".into(),
                name: None,
                primary: None,
                secondary: None,
            });
            bridge.snapshot.usage = Some(json!({"summary":{"totalTokens":42}}));
            bridge.identity_observation = Some("old-identity".into());
            bridge.failure_count = 4;
            bridge.next_attempt = Some(Instant::now() + Duration::from_secs(900));
            let before = serde_json::to_value(&bridge.snapshot).unwrap();
            let deadline = bridge.next_attempt;
            assert!(bridge.observe_resource_stop().is_none());
            // A monitor can latch asynchronously while no reader is active, and
            // publication of its diagnostic is not required for enforcement.
            let latch = bridge.resource_stop.clone();
            std::thread::spawn(move || latch.store(reason, Ordering::Release))
                .join()
                .unwrap();
            for _ in 0..2 {
                assert_eq!(bridge.observe_resource_stop(), resource_error(reason));
                assert!(bridge.transport.is_none());
                assert_eq!(serde_json::to_value(&bridge.snapshot).unwrap(), before);
                assert_eq!(bridge.identity_observation.as_deref(), Some("old-identity"));
                assert_eq!(bridge.failure_count, 4);
                assert_eq!(bridge.next_attempt, deadline);
                assert!(bridge.resource_diagnostic().is_none());
                assert_eq!(bridge.resource_stop.load(Ordering::Acquire), reason);
            }
            record_diagnostic(&bridge.diagnostics, reason);
            assert_eq!(bridge.observe_resource_stop(), resource_error(reason));
            assert!(bridge.resource_diagnostic().is_some());
            assert!(bridge
                .ensure_transport_with(|_, _, _, _| panic!("poll must not reopen the service"))
                .is_err());
            bridge.retry_manually();
            assert!(bridge.observe_resource_stop().is_none());
            assert!(bridge.resource_diagnostic().is_none());
            assert_eq!(bridge.resource_stop.load(Ordering::Acquire), 0);
            assert_eq!(bridge.failure_count, 0);
            assert!(bridge.next_attempt.is_none());
            assert_eq!(bridge.snapshot.collected_at.as_deref(), Some("old-success"));
            assert_eq!(
                bridge.snapshot.last_attempt_at.as_deref(),
                Some("old-attempt")
            );
            assert_eq!(bridge.snapshot.buckets[0].id, "old-bucket");
            assert_eq!(bridge.identity_observation.as_deref(), Some("old-identity"));
            // Conversely, a diagnostic without a latch is not a stop signal.
            record_diagnostic(&bridge.diagnostics, reason);
            assert!(bridge.observe_resource_stop().is_none());
            assert!(bridge.resource_diagnostic().is_some());
        }
    }

    #[test]
    fn resource_diagnostic_is_first_event_only_bounded_and_allowlisted() {
        let bridge = AccountBridge::with_source_root(Path::new("D:/SECRET-source"));
        let diagnostics = &bridge.diagnostics;
        for method in ALLOWED {
            let stage = BridgeStage::for_method(method).expect("every admitted method has a stage");
            assert_eq!(BridgeStage::from_code(stage as u8), stage);
            assert_eq!(serde_json::to_value(stage).unwrap(), *method);
        }
        assert!(BridgeStage::for_method("thread/list?token=SECRET-content").is_none());
        diagnostics.set_stage(BridgeStage::ThreadList);
        let identity = BridgeRootIdentity {
            pid: 42,
            creation_filetime: Some(132_456_789_012_345_678),
            creation_query_error: None,
        };
        diagnostics.set_root(identity);
        diagnostics.record_resource_event(
            RESOURCE_WARNING,
            diagnostics.observed_stage(),
            chrono::Utc::now(),
            Instant::now(),
            Some(11),
            Some(true),
            None,
            Some(BridgeLimitObservation {
                job_memory: Some(100_663_297),
                job_memory_limit: None,
                limit_flags: 0,
                violation_limit_flags: 512,
            }),
        );
        for _ in 0..1000 {
            diagnostics.set_stage(BridgeStage::AccountRead);
            record_diagnostic(diagnostics, RESOURCE_HARD_LIMIT);
        }
        let event = bridge.resource_diagnostic().unwrap();
        assert_eq!(event.reason, RESOURCE_WARNING);
        assert_eq!(event.observed_stage, BridgeStage::ThreadList);
        assert_eq!(event.root, Some(identity));
        assert_eq!(event.job_memory, Some(100_663_297));
        assert_eq!(event.job_memory_limit, None);
        assert_eq!(event.limit_flags, Some(0));
        assert_eq!(event.violation_limit_flags, Some(512));
        assert!(chrono::DateTime::parse_from_rfc3339(&event.event_utc).is_ok());
        let serialized_event = serde_json::to_value(&event).unwrap();
        assert_eq!(
            serialized_event["root"]["creationFiletime"],
            "132456789012345678"
        );
        let unknown_creation = BridgeRootIdentity {
            creation_filetime: None,
            creation_query_error: Some(5),
            ..identity
        };
        assert!(serde_json::to_value(unknown_creation).unwrap()["creationFiletime"].is_null());
        let serialized = serde_json::to_string(&event).unwrap();
        assert!(serialized.len() < 1024);
        assert!(!serialized.contains("SECRET"));
        assert!(!serialized.contains("source"));
        assert!(!serialized.contains("params"));
        assert!(!serialized.contains("accountScope"));
        assert!(serde_json::to_value(&bridge.snapshot)
            .unwrap()
            .get("resourceDiagnostic")
            .is_none());
    }

    #[test]
    fn resource_diagnostic_keeps_unknown_values_and_exact_owner_cleanup_facts() {
        let diagnostics = BridgeDiagnostics::new();
        let identity = BridgeRootIdentity {
            pid: 42,
            creation_filetime: Some(123456),
            creation_query_error: None,
        };
        diagnostics.set_root(identity);
        record_diagnostic(&diagnostics, RESOURCE_HARD_LIMIT);
        let event = diagnostics.snapshot().unwrap();
        assert!(event.notification_query_succeeded.is_none());
        assert!(event.job_memory.is_none());
        assert!(event.job_memory_limit.is_none());
        assert!(event.limit_flags.is_none());
        assert!(event.violation_limit_flags.is_none());
        assert!(event.cleanup.owner_wait_result.is_none());
        diagnostics.record_owner_cleanup(
            BridgeRootIdentity {
                creation_filetime: Some(654321),
                ..identity
            },
            true,
            true,
            0,
        );
        assert!(diagnostics
            .snapshot()
            .unwrap()
            .cleanup
            .owner_wait_result
            .is_none());
        diagnostics.record_monitor_cleanup(false);
        diagnostics.record_monitor_cleanup(true);
        diagnostics.record_owner_cleanup(identity, false, false, 258);
        diagnostics.record_owner_cleanup(identity, true, true, 0);
        let event = diagnostics.snapshot().unwrap();
        assert_eq!(event.cleanup.monitor_terminate_job_succeeded, Some(false));
        assert_eq!(event.cleanup.owner_terminate_job_succeeded, Some(false));
        assert_eq!(event.cleanup.owner_terminate_process_succeeded, Some(false));
        assert_eq!(event.cleanup.owner_wait_result, Some(258));
        diagnostics.reset();
        diagnostics.record_resource_event(
            RESOURCE_MONITOR_FAILED,
            BridgeStage::Startup,
            chrono::Utc::now(),
            Instant::now(),
            Some(11),
            Some(false),
            Some(5),
            None,
        );
        let failed_query = diagnostics.snapshot().unwrap();
        assert_eq!(failed_query.notification_query_succeeded, Some(false));
        assert_eq!(failed_query.monitor_error, Some(5));
        assert!(failed_query.job_memory.is_none());
        assert!(failed_query.job_memory_limit.is_none());
        assert!(failed_query.limit_flags.is_none());
        assert!(failed_query.violation_limit_flags.is_none());
    }

    #[test]
    fn resource_diagnostic_survives_cache_source_and_shared_gate_until_resource_retry() {
        let mut bridge = AccountBridge::with_source_root(Path::new("D:/SECRET-source-one"));
        bridge.diagnostics.set_stage(BridgeStage::AccountRead);
        record_diagnostic(&bridge.diagnostics, RESOURCE_WARNING);
        bridge.retry_manually();
        assert!(
            bridge.resource_diagnostic().is_some(),
            "healthy manual refresh does not reset diagnostics"
        );
        bridge
            .resource_stop
            .store(RESOURCE_WARNING, Ordering::Release);
        bridge.clear_cached_data();
        bridge.set_source_root(Path::new("D:/SECRET-source-two"));
        let directory_error = bridge.list_sessions().unwrap_err();
        assert_eq!(directory_error, resource_error(RESOURCE_WARNING).unwrap());
        let account =
            bridge.refresh_with(|_, _, _, _| panic!("latched account gate must not restart"));
        assert_eq!(account.error.as_deref(), Some(directory_error.as_str()));
        assert_eq!(
            bridge.resource_diagnostic().unwrap().observed_stage,
            BridgeStage::AccountRead
        );
        assert_eq!(
            bridge.resource_stop.load(Ordering::Acquire),
            RESOURCE_WARNING
        );
        bridge.cancel.store(true, Ordering::Release);
        bridge.retry_manually();
        assert!(
            bridge.resource_diagnostic().is_some(),
            "cancelled retry does not clear evidence"
        );
        bridge.cancel.store(false, Ordering::Release);
        bridge.retry_manually();
        assert!(bridge.resource_diagnostic().is_none());
        assert_eq!(bridge.resource_stop.load(Ordering::Acquire), 0);
        assert!(bridge.transport.is_none());
        assert_eq!(bridge.diagnostics.observed_stage(), BridgeStage::Startup);
        record_diagnostic(&bridge.diagnostics, RESOURCE_MONITOR_FAILED);
        assert_eq!(
            bridge.resource_diagnostic().unwrap().reason,
            RESOURCE_MONITOR_FAILED
        );
    }

    #[test]
    fn poisoned_resource_diagnostic_lock_remains_available_after_stop() {
        let diagnostics = BridgeDiagnostics::new();
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = diagnostics.state.lock().unwrap();
            panic!("test-only diagnostic lock poison");
        }));
        assert!(poisoned.is_err());
        record_diagnostic(&diagnostics, RESOURCE_MONITOR_FAILED);
        assert_eq!(
            diagnostics.snapshot().unwrap().reason,
            RESOURCE_MONITOR_FAILED
        );
        diagnostics.reset();
        assert!(diagnostics.snapshot().is_none());
    }

    #[cfg(windows)]
    #[test]
    fn resource_diagnostic_masks_memory_fields_by_their_respective_flags() {
        use windows_sys::Win32::System::JobObjects::{
            JOBOBJECT_LIMIT_VIOLATION_INFORMATION, JOB_OBJECT_LIMIT_JOB_MEMORY,
        };
        let mut raw: JOBOBJECT_LIMIT_VIOLATION_INFORMATION = unsafe { std::mem::zeroed() };
        raw.JobMemory = 111;
        raw.JobMemoryLimit = 222;
        let absent = win_process::memory_observation(&raw);
        assert!(absent.job_memory.is_none());
        assert!(absent.job_memory_limit.is_none());
        raw.ViolationLimitFlags = JOB_OBJECT_LIMIT_JOB_MEMORY;
        let notification = win_process::memory_observation(&raw);
        assert_eq!(notification.job_memory, Some(111));
        assert!(notification.job_memory_limit.is_none());
        raw.LimitFlags = JOB_OBJECT_LIMIT_JOB_MEMORY;
        let limit = win_process::memory_observation(&raw);
        assert_eq!(limit.job_memory_limit, Some(222));
    }

    #[test]
    fn quota_multi_bucket_missing_is_not_zero() {
        let result = sanitize_limits(&json!({"rateLimitsByLimitId":{
            "codex":{"primary":{"usedPercent":25.5,"resetsAt":1700000000},"secondary":null},
            "other":{"primary":{"usedPercent":120},"secondary":{"usedPercent":0,"windowDurationMins":60}}
        }}));
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].primary.as_ref().unwrap().used_percent, 25.5);
        assert!(result[0].secondary.is_none());
        assert!(result[1].primary.is_none());
        assert_eq!(result[1].secondary.as_ref().unwrap().used_percent, 0.0);
    }
    #[test]
    fn account_scope_excludes_plan_and_separates_accounts() {
        assert!(
            account_scope(&json!({"type":"chatgpt","email":"a@example.com","planType":"pro"}))
                .is_none()
        );
        assert!(account_scope(&json!({"type":"apiKey"})).is_none());
        assert_ne!(
            account_scope(&json!({"accountId":"a","organizationId":"one"})),
            account_scope(&json!({"accountId":"a","organizationId":"two"}))
        );
        assert_eq!(
            account_scope(&json!({"accountId":"a","subscriptionId":"one","planType":"pro"})),
            account_scope(&json!({"accountId":"a","subscriptionId":"one","planType":"plus"}))
        );
    }
    #[test]
    fn usage_whitelist_drops_sensitive_and_invalid_fields() {
        let usage = sanitize_usage(
            &json!({"token":"secret","summary":{"lifetimeTokens":5,"email":"secret"},"dailyUsageBuckets":[{"startDate":"2026-10-15","tokens":9},{"startDate":"invalid","tokens":2}]}),
        );
        assert_eq!(usage["summary"]["lifetimeTokens"], 5);
        assert!(usage["summary"]["peakDailyTokens"].is_null());
        assert_eq!(usage["dailyUsageBuckets"].as_array().unwrap().len(), 1);
        assert!(!usage.to_string().contains("secret"));
    }
    #[test]
    fn protocol_bounds_and_split_utf8() {
        let mut reader = BufReader::with_capacity(2, std::io::Cursor::new("{\"value\":\"喵\"}\n"));
        assert_eq!(read_message(&mut reader).unwrap()["value"], "喵");
        let mut reader = std::io::Cursor::new(vec![b'x'; MAX_MESSAGE + 1]);
        assert!(read_message(&mut reader).is_err());
        assert!(!ALLOWED.contains(&"turn/start"));
        assert!(!ALLOWED.contains(&"account/login/start"));
    }

    struct FakeRpc {
        results: std::collections::VecDeque<Result<Value, String>>,
        calls: Vec<(String, Value)>,
    }
    impl FakeRpc {
        fn new(values: Vec<Result<Value, String>>) -> Self {
            Self {
                results: values.into(),
                calls: vec![],
            }
        }
    }
    impl ReadRpc for FakeRpc {
        fn read(&mut self, method: &str, params: Value) -> Result<Value, String> {
            assert!(ALLOWED.contains(&method));
            self.calls.push((method.into(), params));
            self.results.pop_front().expect("unexpected RPC")
        }
    }
    #[test]
    fn missing_login_invalidates_current_values_without_requesting_login() {
        let mut rpc = FakeRpc::new(vec![Ok(json!({"account":null}))]);
        let mut previous = AccountSnapshot {
            collected_at: Some("old".into()),
            buckets: vec![quota_bucket("old", &json!({"primary":{"usedPercent":80}}))],
            ..AccountSnapshot::default()
        };
        let mut observation = Some("old".into());
        let result = read_account_snapshot(&mut rpc, &mut previous, &mut observation).unwrap();
        assert_eq!(result.status, "unavailable");
        assert!(result.buckets.is_empty());
        assert!(previous.buckets.is_empty());
        assert!(observation.is_none());
        assert_eq!(rpc.calls.len(), 1);
        assert_eq!(rpc.calls[0].1["refreshToken"], false);
    }
    #[test]
    fn changed_account_drops_cached_values_even_when_quota_read_fails() {
        let mut previous = AccountSnapshot::default();
        let mut observation = None;
        let mut first = FakeRpc::new(vec![
            Ok(json!({"account":{"type":"chatgpt","email":"one@example.com","planType":"pro"}})),
            Ok(json!({"rateLimits":{"primary":{"usedPercent":80}}})),
            Err(UNSUPPORTED.into()),
        ]);
        previous = read_account_snapshot(&mut first, &mut previous, &mut observation).unwrap();
        assert_eq!(previous.status, "connected");
        assert_eq!(previous.buckets.len(), 1);
        assert!(previous.usage.is_none());
        assert!(previous.account_scope.is_none());
        let mut next = FakeRpc::new(vec![
            Ok(json!({"account":{"type":"chatgpt","email":"two@example.com","planType":"pro"}})),
            Err("offline".into()),
        ]);
        assert!(read_account_snapshot(&mut next, &mut previous, &mut observation).is_err());
        assert!(previous.collected_at.is_none());
        assert!(previous.buckets.is_empty());
    }
    #[test]
    fn offline_login_read_preserves_last_collected_data() {
        let mut previous = AccountSnapshot {
            collected_at: Some("2026-10-15T00:00:00Z".into()),
            buckets: vec![quota_bucket(
                "codex",
                &json!({"primary":{"usedPercent":15}}),
            )],
            ..AccountSnapshot::default()
        };
        let mut rpc = FakeRpc::new(vec![Err("offline".into())]);
        assert!(read_account_snapshot(&mut rpc, &mut previous, &mut None).is_err());
        assert_eq!(
            previous.buckets[0].primary.as_ref().unwrap().used_percent,
            15.0
        );
        assert!(previous.collected_at.is_some());
    }
    #[test]
    fn directory_paginates_all_sources_and_archives_without_content() {
        let mut rpc = FakeRpc::new(vec![
            Ok(
                json!({"data":[{"id":"019c6e27-e55b-73d1-87d8-4e01f1f75043","name":"可展示标题","cwd":"D:/test","preview":"SECRET MESSAGE","turns":["SECRET"],"status":{"type":"idle"}}],"nextCursor":"next"}),
            ),
            Ok(json!({"data":[],"nextCursor":null})),
            Ok(json!({"data":[{"id":"019c7714-3b77-74d1-9866-e1f484aae2ab","preview":"SECRET"}]})),
        ]);
        let rows = read_directory(&mut rpc).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows[1]["title"].is_null());
        assert!(!serde_json::to_string(&rows).unwrap().contains("SECRET"));
        assert!(rows[0].get("status").is_none());
        assert_eq!(rpc.calls[0].1["useStateDbOnly"], true);
        assert_eq!(rpc.calls[0].1["sourceKinds"].as_array().unwrap().len(), 10);
        assert_eq!(rpc.calls[1].1["cursor"], "next");
        assert_eq!(rpc.calls[2].1["archived"], true);
    }
    #[test]
    fn directory_relationship_projection_preserves_only_role_evidence() {
        let parent = "019c6e27-e55b-73d1-87d8-4e01f1f75043";
        let source = json!({"subAgent":{"thread_spawn":{
            "parent_thread_id":parent,"depth":2,"agent_nickname":"SECRET",
            "agent_role":"SECRET","agent_path":"SECRET","command":"SECRET"
        }}});
        assert_eq!(
            directory_source(Some(&source)),
            json!({"subAgent":{"thread_spawn":{
                "parent_thread_id":parent,"depth":2
            }}})
        );
        for kind in ["review", "compact", "memory_consolidation"] {
            assert_eq!(
                directory_source(Some(&json!({"subAgent":kind}))),
                json!({"subAgent":kind})
            );
        }
        assert_eq!(
            directory_source(Some(&json!({"subAgent":{"other":"SECRET"}}))),
            json!({"subAgent":{"other":"observed"}})
        );
        for invalid in [
            json!({"custom":"SECRET"}),
            json!({"subAgent":"future"}),
            json!({"subAgent":"review","extra":"SECRET"}),
            json!({"subAgent":{"thread_spawn":{"parent_thread_id":"invalid","depth":1}}}),
            json!({"subAgent":{"thread_spawn":{"parent_thread_id":parent,"depth":2147483648i64}}}),
        ] {
            assert!(directory_source(Some(&invalid)).is_null());
        }
        assert_eq!(directory_thread_source(Some(&json!("user"))), "user");
        assert_eq!(
            directory_thread_source(Some(&json!("subagent"))),
            "subagent"
        );
        assert_eq!(
            directory_thread_source(Some(&json!("SECRET future feature"))),
            "unsupported"
        );
        assert!(directory_thread_source(None).is_null());
    }
    #[test]
    fn directory_creation_version_does_not_claim_a_verified_rpc_protocol() {
        let mut rpc = FakeRpc::new(vec![
            Ok(json!({"data":[{
                "id":"019c6e27-e55b-73d1-87d8-4e01f1f75043",
                "cliVersion":"0.147.0","source":"vscode","threadSource":"user",
                "directoryProtocolVersion":"0.147.0","preview":"SECRET"
            }]})),
            Ok(json!({"data":[]})),
        ]);
        let rows = read_directory(&mut rpc).unwrap();
        assert_eq!(rows[0]["cliVersion"], "0.147.0");
        assert_eq!(rows[0]["source"], "vscode");
        assert_eq!(rows[0]["threadSource"], "user");
        assert!(rows[0]["directoryProtocolVersion"].is_null());
        assert!(!serde_json::to_string(&rows).unwrap().contains("SECRET"));
    }
    #[test]
    fn repeated_directory_cursor_fails_instead_of_claiming_complete() {
        let mut rpc = FakeRpc::new(vec![
            Ok(json!({"data":[],"nextCursor":"same"})),
            Ok(json!({"data":[],"nextCursor":"same"})),
        ]);
        assert!(read_directory(&mut rpc).unwrap_err().contains("游标重复"));
    }
    #[test]
    fn protocol_correlates_only_requested_id_and_filters_server_error_text() {
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(
            json!({"method":"untrusted/notification","params":{"token":"SECRET"}}),
        ))
        .unwrap();
        tx.send(Ok(json!({"id":2,"result":{"ignored":true}})))
            .unwrap();
        tx.send(Ok(json!({"id":1,"result":{"safe":true}}))).unwrap();
        let cancel = AtomicBool::new(false);
        assert_eq!(
            await_response(&rx, 1, &cancel, Duration::from_millis(100)).unwrap(),
            json!({"safe":true})
        );
        tx.send(Ok(
            json!({"id":3,"error":{"code":-32601,"message":"SECRET token and local path"}}),
        ))
        .unwrap();
        let error = await_response(&rx, 3, &cancel, Duration::from_millis(100)).unwrap_err();
        assert!(!error.contains("SECRET"));
        assert!(error.contains("不支持"));
    }
    #[test]
    fn protocol_cancellation_and_timeout_are_bounded() {
        let (_tx, rx) = mpsc::channel();
        let cancel = AtomicBool::new(true);
        let start = Instant::now();
        assert!(await_response(&rx, 1, &cancel, Duration::from_secs(4))
            .unwrap_err()
            .contains("退出"));
        assert!(start.elapsed() < Duration::from_millis(100));
        cancel.store(false, Ordering::Relaxed);
        let start = Instant::now();
        assert!(await_response(&rx, 1, &cancel, Duration::from_millis(30))
            .unwrap_err()
            .contains("超时"));
        assert!(start.elapsed() < Duration::from_millis(200));
    }
    #[test]
    fn quota_reset_timestamps_are_source_values_not_token_estimates() {
        let window = quota_window(Some(
            &json!({"usedPercent":100,"resetsAt":1,"windowDurationMins":300}),
        ))
        .unwrap();
        assert_eq!(window.used_percent, 100.0);
        assert_eq!(window.resets_at, Some(1));
        assert!(quota_window(Some(&json!({"resetsAt":1}))).is_none());
        assert!(quota_window(Some(&json!({"usedPercent":-1}))).is_none());
    }

    #[test]
    fn clearing_cached_data_drops_account_facts_without_switching_source_or_cancel_handle() {
        let source = Path::new("D:/source-one");
        let mut bridge = AccountBridge::with_source_root(source);
        let cancel = bridge.cancel_handle();
        bridge.snapshot = AccountSnapshot {
            status: "connected".into(),
            collected_at: Some("old".into()),
            account_scope: Some("old-account".into()),
            usage: Some(json!({"dailyUsageBuckets": [{"date":"old","totalTokens":50}]})),
            ..AccountSnapshot::default()
        };
        bridge.identity_observation = Some("old-identity".into());
        bridge.failure_count = 4;
        bridge.next_attempt = Some(Instant::now() + Duration::from_secs(900));
        cancel.store(true, Ordering::Relaxed);
        bridge.clear_cached_data();
        assert_eq!(bridge.source_root.as_deref(), Some(source));
        assert!(Arc::ptr_eq(&cancel, &bridge.cancel_handle()));
        assert!(bridge.cancel.load(Ordering::Relaxed));
        assert_eq!(bridge.snapshot.status, "unavailable");
        assert!(bridge.snapshot.collected_at.is_none());
        assert!(bridge.snapshot.account_scope.is_none());
        assert!(bridge.snapshot.buckets.is_empty());
        assert!(bridge.snapshot.usage.is_none());
        assert!(bridge.identity_observation.is_none());
        assert_eq!(bridge.failure_count, 4);
        assert!(bridge.next_attempt.is_some());
        assert_eq!(bridge.refresh().status, "unavailable");
        assert!(bridge.transport.is_none());
    }

    #[test]
    fn source_switch_invalidates_all_cached_account_state_but_keeps_cancel_handle() {
        let mut bridge = AccountBridge::with_source_root(Path::new("D:/source-one"));
        let cancel = bridge.cancel_handle();
        bridge.snapshot.collected_at = Some("old".into());
        bridge.identity_observation = Some("old".into());
        bridge.failure_count = 4;
        bridge.next_attempt = Some(Instant::now() + Duration::from_secs(900));
        bridge.set_source_root(Path::new("D:/source-two"));
        assert!(bridge.snapshot.collected_at.is_none());
        assert!(bridge.identity_observation.is_none());
        assert_eq!(bridge.failure_count, 4);
        assert!(bridge.next_attempt.is_some());
        assert!(Arc::ptr_eq(&cancel, &bridge.cancel_handle()));
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(bridge.refresh().status, "unavailable");
        assert!(bridge.transport.is_none());
    }

    #[test]
    fn directory_failure_shares_backoff_and_resource_fuse_survives_cache_and_source_changes() {
        let mut bridge = AccountBridge::with_source_root(Path::new("D:/private-source-one"));
        bridge.snapshot.collected_at = Some("old-success".into());
        bridge.snapshot.last_attempt_at = Some("old-attempt".into());
        bridge.snapshot.account_scope = Some("old-scope".into());
        bridge
            .finish_read::<()>(Err("账户读取连接中断".into()))
            .unwrap_err();
        let deadline = bridge.next_attempt;
        assert_eq!(bridge.failure_count, 1);
        assert_eq!(bridge.list_sessions().unwrap_err(), DEFERRED);
        let snapshot = bridge.refresh();
        assert_eq!(snapshot.status, "stale");
        assert_eq!(snapshot.collected_at.as_deref(), Some("old-success"));
        assert_eq!(snapshot.last_attempt_at.as_deref(), Some("old-attempt"));
        assert_eq!(bridge.failure_count, 1);
        assert_eq!(bridge.next_attempt, deadline);
        assert_eq!(bridge.resource_stop.load(Ordering::Acquire), 0); // ordinary EOF is not resource evidence
        bridge
            .resource_stop
            .store(RESOURCE_WARNING, Ordering::Release);
        bridge.clear_cached_data();
        bridge.set_source_root(Path::new("D:/private-source-two"));
        assert!(bridge.list_sessions().unwrap_err().contains("预算预警"));
        assert!(bridge.transport.is_none());
        assert_eq!(
            bridge.resource_stop.load(Ordering::Acquire),
            RESOURCE_WARNING
        );
        bridge.retry_manually();
        assert_eq!(bridge.resource_stop.load(Ordering::Acquire), 0);
        assert_eq!(bridge.failure_count, 0);
        assert!(bridge.next_attempt.is_none());
        assert!(bridge.snapshot.collected_at.is_none());
        assert!(bridge.snapshot.account_scope.is_none());
        assert_eq!(
            bridge.source_root.as_deref(),
            Some(Path::new("D:/private-source-two"))
        );
    }

    #[test]
    fn unsupported_capability_is_local_but_usage_transport_error_is_not_success() {
        let mut bridge = AccountBridge::new();
        assert_eq!(
            bridge
                .finish_read::<()>(Err(UNSUPPORTED.into()))
                .unwrap_err(),
            UNSUPPORTED
        );
        assert_eq!(bridge.failure_count, 0);
        assert!(bridge.next_attempt.is_none());
        let mut previous = AccountSnapshot::default();
        let mut observation = None;
        let mut rpc = FakeRpc::new(vec![
            Ok(json!({"account":{"type":"chatgpt","id":"private-fixture"}})),
            Ok(json!({"rateLimits":{"primary":{"usedPercent":17}}})),
            Err(UNSUPPORTED.into()),
        ]);
        let result = read_account_snapshot(&mut rpc, &mut previous, &mut observation).unwrap();
        assert_eq!(result.status, "connected");
        assert_eq!(
            result.buckets[0].primary.as_ref().unwrap().used_percent,
            17.0
        );
        assert!(result.usage.is_none());
        let mut rpc = FakeRpc::new(vec![
            Ok(json!({"account":{"type":"chatgpt","id":"private-fixture"}})),
            Ok(json!({"rateLimits":{"primary":{"usedPercent":17}}})),
            Err("账户读取连接中断".into()),
        ]);
        assert!(
            read_account_snapshot(&mut rpc, &mut previous, &mut observation)
                .unwrap_err()
                .contains("连接中断")
        );
    }

    #[test]
    fn usage_rejection_preserves_current_quota_but_rate_limit_rejection_fails() {
        let account = json!({"account":{"type":"chatgpt","id":"private-fixture"}});
        let mut previous = AccountSnapshot::default();
        let mut observation = None;
        let mut rpc = FakeRpc::new(vec![
            Ok(account.clone()),
            Ok(json!({"rateLimits":{"primary":{"usedPercent":17}}})),
            Err(RPC_REJECTED.into()),
        ]);
        let snapshot = read_account_snapshot(&mut rpc, &mut previous, &mut observation).unwrap();
        assert_eq!(snapshot.status, "connected");
        assert_eq!(
            snapshot.buckets[0].primary.as_ref().unwrap().used_percent,
            17.0
        );
        assert!(snapshot.usage.is_none());
        assert!(snapshot.error.unwrap().contains("日趋势被拒绝"));
        let mut rpc = FakeRpc::new(vec![Ok(account), Err(RPC_REJECTED.into())]);
        assert_eq!(
            read_account_snapshot(&mut rpc, &mut previous, &mut observation).unwrap_err(),
            RPC_REJECTED
        );
    }

    #[test]
    fn invalid_source_admission_rejects_missing_or_file_without_attempt_or_fact_changes() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("source-file");
        std::fs::write(&file, b"not-a-directory").unwrap();
        for root in [temp.path().join("missing-source"), file] {
            let mut bridge = AccountBridge::with_source_root(&root);
            bridge.snapshot.status = "connected".into();
            bridge.snapshot.collected_at = Some("old-success".into());
            bridge.snapshot.last_attempt_at = Some("old-attempt".into());
            bridge.snapshot.usage = Some(json!({"summary":{"totalTokens":42}}));
            bridge.identity_observation = Some("old-identity".into());
            bridge.failure_count = 3;
            bridge.diagnostics.set_stage(BridgeStage::AccountRead);
            let snapshot = bridge.refresh_with(|_, _, _, _| {
                panic!("an invalid source must not invoke the transport factory")
            });
            assert_eq!(snapshot.status, "stale");
            assert_eq!(snapshot.error.as_deref(), Some(SOURCE_UNAVAILABLE));
            assert_eq!(snapshot.collected_at.as_deref(), Some("old-success"));
            assert_eq!(snapshot.last_attempt_at.as_deref(), Some("old-attempt"));
            assert_eq!(snapshot.usage.unwrap()["summary"]["totalTokens"], 42);
            assert_eq!(bridge.identity_observation.as_deref(), Some("old-identity"));
            assert_eq!(bridge.failure_count, 3);
            assert!(bridge.next_attempt.is_none());
            assert!(bridge.transport.is_none());
            assert_eq!(
                bridge.diagnostics.observed_stage(),
                BridgeStage::AccountRead
            );
        }
    }

    #[test]
    fn invalid_source_admission_preserves_gate_priority_and_manual_retry_semantics() {
        for (cancelled, resource, expected) in [
            (true, RESOURCE_WARNING, "桥接正在退出"),
            (
                false,
                RESOURCE_WARNING,
                resource_error(RESOURCE_WARNING).unwrap(),
            ),
            (false, 0, DEFERRED),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("missing-source");
            let mut bridge = AccountBridge::with_source_root(&root);
            bridge.snapshot.collected_at = Some("old-success".into());
            bridge.snapshot.last_attempt_at = Some("old-attempt".into());
            bridge.failure_count = 4;
            let deadline = Some(Instant::now() + Duration::from_secs(900));
            bridge.next_attempt = deadline;
            bridge.cancel.store(cancelled, Ordering::Release);
            bridge.resource_stop.store(resource, Ordering::Release);
            let error = bridge
                .ensure_transport_with(|_, _, _, _| {
                    panic!("an admission gate must not invoke the factory")
                })
                .err()
                .unwrap();
            assert_eq!(error.message, expected);
            assert!(error.attempted_at.is_none());
            assert_eq!(bridge.failure_count, 4);
            assert_eq!(bridge.next_attempt, deadline);
            bridge.retry_manually();
            if cancelled {
                assert_eq!(bridge.failure_count, 4);
                assert_eq!(bridge.next_attempt, deadline);
                assert_eq!(bridge.resource_stop.load(Ordering::Acquire), resource);
                bridge.cancel.store(false, Ordering::Release);
                bridge.retry_manually();
            }
            assert_eq!(bridge.failure_count, 0);
            assert!(bridge.next_attempt.is_none());
            assert_eq!(bridge.resource_stop.load(Ordering::Acquire), 0);
            let missing = bridge.refresh_with(|_, _, _, _| {
                panic!("manual retry cannot make a missing source valid")
            });
            assert_eq!(missing.error.as_deref(), Some(SOURCE_UNAVAILABLE));
            assert_eq!(missing.collected_at.as_deref(), Some("old-success"));
            assert_eq!(missing.last_attempt_at.as_deref(), Some("old-attempt"));
            assert_eq!(bridge.failure_count, 0);
            assert!(bridge.next_attempt.is_none());
            std::fs::create_dir(&root).unwrap();
            let mut factory_calls = 0;
            let admitted = bridge.refresh_with(|_, selected, _, _| {
                factory_calls += 1;
                assert_eq!(selected, Some(root.as_path()));
                Err("injected admitted startup failure".into())
            });
            assert_eq!(factory_calls, 1);
            assert!(chrono::DateTime::parse_from_rfc3339(
                admitted.last_attempt_at.as_deref().unwrap()
            )
            .is_ok());
            assert_eq!(admitted.collected_at.as_deref(), Some("old-success"));
            assert_eq!(bridge.failure_count, 1);
            assert!(bridge.next_attempt.is_some());
        }
    }

    #[cfg(windows)]
    #[test]
    fn invalid_source_admission_reaps_existing_owned_transport_even_during_backoff() {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{
            Foundation::WAIT_OBJECT_0, System::Threading::WaitForSingleObject,
        };
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("source");
        std::fs::create_dir(&root).unwrap();
        // Reuse only the existing suspended Rust test-child fixture. No Codex or
        // application process is started, and it need not run to prove cleanup.
        let (child, input, output, cancel, resource) =
            start_fixture("hold", Some(win_process::WARNING_COMMIT));
        let witness = child.process.try_clone().unwrap();
        let (_tx, messages) = mpsc::channel();
        let mut bridge = AccountBridge::with_source_root(&root);
        bridge.cancel = cancel;
        bridge.resource_stop = resource;
        bridge.snapshot.collected_at = Some("old-success".into());
        bridge.snapshot.last_attempt_at = Some("old-attempt".into());
        bridge.failure_count = 2;
        let deadline = Some(Instant::now() + Duration::from_secs(900));
        bridge.next_attempt = deadline;
        bridge.transport = Some(Transport {
            child,
            stdin: Some(input),
            messages: Some(messages),
            reader: None,
            next_id: 0,
            cancel: bridge.cancel.clone(),
            resource_stop: bridge.resource_stop.clone(),
            diagnostics: bridge.diagnostics.clone(),
            unsupported: HashSet::new(),
        });
        std::fs::remove_dir(&root).unwrap();
        let error = bridge
            .ensure_transport_with(|_, _, _, _| panic!("an invalidated source must not reconnect"))
            .err()
            .unwrap();
        assert_eq!(error.message, DEFERRED);
        assert!(error.attempted_at.is_none());
        assert!(bridge.transport.is_none());
        assert_eq!(bridge.failure_count, 2);
        assert_eq!(bridge.next_attempt, deadline);
        assert_eq!(bridge.snapshot.collected_at.as_deref(), Some("old-success"));
        assert_eq!(
            bridge.snapshot.last_attempt_at.as_deref(),
            Some("old-attempt")
        );
        unsafe {
            assert_eq!(
                WaitForSingleObject(witness.as_raw_handle(), 1000),
                WAIT_OBJECT_0
            );
        }
        drop(output);
    }

    #[test]
    fn initialization_failure_records_attempt_but_shared_deferred_readers_do_not_attempt() {
        let mut bridge = AccountBridge::new();
        let failed = bridge.refresh_with(|_, _, _, _| Err("初始化读取超时".into()));
        let attempted = failed
            .last_attempt_at
            .expect("a real admitted startup must have a timestamp");
        assert!(chrono::DateTime::parse_from_rfc3339(&attempted).is_ok());
        assert!(failed.collected_at.is_none());
        assert_eq!(bridge.failure_count, 1);
        let deferred = bridge
            .ensure_transport_with(|_, _, _, _| panic!("deferred startup must not invoke factory"))
            .err()
            .unwrap();
        assert!(deferred.attempted_at.is_none());
        assert_eq!(bridge.list_sessions().unwrap_err(), DEFERRED);
        assert_eq!(
            bridge.refresh().last_attempt_at.as_deref(),
            Some(attempted.as_str())
        );
        assert_eq!(bridge.failure_count, 1);
    }

    #[cfg(windows)]
    fn child_args() -> Vec<std::ffi::OsString> {
        [
            "--exact",
            "account_bridge::tests::owned_bridge_test_child",
            "--nocapture",
        ]
        .into_iter()
        .map(Into::into)
        .collect()
    }
    #[cfg(windows)]
    fn start_fixture(
        role: &str,
        warning: Option<usize>,
    ) -> (
        win_process::OwnedProcess,
        std::fs::File,
        std::fs::File,
        Arc<AtomicBool>,
        Arc<AtomicU8>,
    ) {
        let cancel = Arc::new(AtomicBool::new(false));
        let resource = Arc::new(AtomicU8::new(0));
        let overrides = vec![("TOKEN_MEOW_PRIVATE_BRIDGE_TEST_ROLE".into(), role.into())];
        let (child, input, output) = win_process::OwnedProcess::test_suspended(
            &child_args(),
            &overrides,
            cancel.clone(),
            resource.clone(),
            warning,
        )
        .unwrap();
        (child, input, output, cancel, resource)
    }
    #[cfg(windows)]
    fn markers(output: std::fs::File) -> (Receiver<String>, std::thread::JoinHandle<()>) {
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let mut reader = BufReader::new(output);
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 0) {
                if let Some(i) = line.find("BRIDGE_") {
                    if tx.send(line[i..].trim().into()).is_err() {
                        break;
                    }
                }
                line.clear();
            }
        });
        (rx, thread)
    }
    #[cfg(windows)]
    fn marker(rx: &Receiver<String>, prefix: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let line = rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("owned fixture marker timeout");
            if line.starts_with(prefix) {
                return line;
            }
        }
    }
    #[cfg(windows)]
    #[test]
    fn owned_bridge_test_child() {
        let Ok(role) = std::env::var("TOKEN_MEOW_PRIVATE_BRIDGE_TEST_ROLE") else {
            return;
        };
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Memory::*;
        if role == "deny" {
            let allocation = unsafe {
                VirtualAlloc(
                    std::ptr::null(),
                    256 * 1024 * 1024,
                    MEM_COMMIT | MEM_RESERVE,
                    PAGE_READWRITE,
                )
            };
            println!("BRIDGE_DENIED={}", allocation.is_null());
        } else if role == "warning-tree" || role == "allocate" {
            let allocation = unsafe {
                VirtualAlloc(
                    std::ptr::null(),
                    48 * 1024 * 1024,
                    MEM_COMMIT | MEM_RESERVE,
                    PAGE_READWRITE,
                )
            };
            assert!(!allocation.is_null());
            println!("BRIDGE_ALLOCATED=48");
        }
        if role == "tree" || role == "warning-tree" {
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
                .args(child_args())
                .env(
                    "TOKEN_MEOW_PRIVATE_BRIDGE_TEST_ROLE",
                    if role == "tree" { "hold" } else { "allocate" },
                )
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap();
            println!("BRIDGE_DESCENDANT={}", child.id());
        }
        std::io::stdout().flush().unwrap();
        loop {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_bridge_is_budget_bound_before_resume_and_cancel_reclaims_descendants() {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::{
            Foundation::{GetHandleInformation, HANDLE_FLAG_INHERIT, WAIT_OBJECT_0},
            System::{JobObjects::*, Pipes::PeekNamedPipe, Threading::*},
        };
        let (mut child, input, output, cancel, _) =
            start_fixture("tree", Some(win_process::WARNING_COMMIT));
        unsafe {
            let mut inside = 0;
            assert_ne!(
                IsProcessInJob(
                    child.process.as_raw_handle(),
                    child.job_handle(),
                    &mut inside
                ),
                0
            );
            assert_eq!(inside, 1);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            assert_ne!(
                QueryInformationJobObject(
                    child.job_handle(),
                    JobObjectExtendedLimitInformation,
                    (&mut info as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of_val(&info) as u32,
                    std::ptr::null_mut()
                ),
                0
            );
            assert_eq!(info.JobMemoryLimit, win_process::HARD_COMMIT);
            assert_eq!(
                info.BasicLimitInformation.LimitFlags
                    & (JOB_OBJECT_LIMIT_BREAKAWAY_OK | JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK),
                0
            );
            let mut bytes = 99;
            assert_ne!(
                PeekNamedPipe(
                    output.as_raw_handle(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    &mut bytes,
                    std::ptr::null_mut()
                ),
                0
            );
            assert_eq!(bytes, 0); // no child code ran
            for file in [&input, &output] {
                let mut flags = 0;
                assert_ne!(GetHandleInformation(file.as_raw_handle(), &mut flags), 0);
                assert_eq!(flags & HANDLE_FLAG_INHERIT, 0);
            }
            child.resume().unwrap();
            let (rx, reader) = markers(output);
            let pid: u32 = marker(&rx, "BRIDGE_DESCENDANT=")
                .split('=')
                .nth(1)
                .unwrap()
                .parse()
                .unwrap();
            let handle = OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            );
            assert!(!handle.is_null());
            let descendant = OwnedHandle::from_raw_handle(handle);
            assert_ne!(
                IsProcessInJob(descendant.as_raw_handle(), child.job_handle(), &mut inside),
                0
            );
            assert_eq!(inside, 1);
            cancel.store(true, Ordering::Release);
            assert_eq!(
                WaitForSingleObject(child.process.as_raw_handle(), 3000),
                WAIT_OBJECT_0
            );
            assert_eq!(
                WaitForSingleObject(descendant.as_raw_handle(), 3000),
                WAIT_OBJECT_0
            );
            drop(input);
            drop(child);
            reader.join().unwrap();
        }
        // Cancellation after CreateProcess but before Resume never runs the primary thread.
        let (mut child, _input, _output, cancel, _) =
            start_fixture("hold", Some(win_process::WARNING_COMMIT));
        cancel.store(true, Ordering::Release);
        assert!(child.resume().unwrap_err().contains("退出"));
        unsafe {
            assert_eq!(
                WaitForSingleObject(child.process.as_raw_handle(), 1000),
                WAIT_OBJECT_0
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn manual_retry_keeps_healthy_transport_and_unsupported_method_cache() {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{
            Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT},
            System::Threading::WaitForSingleObject,
        };
        let (mut child, input, output, cancel, resource) =
            start_fixture("hold", Some(win_process::WARNING_COMMIT));
        child.resume().unwrap();
        let witness = child.process.try_clone().unwrap();
        let (_tx, rx) = mpsc::channel();
        let mut bridge = AccountBridge::new();
        bridge.cancel = cancel;
        bridge.resource_stop = resource;
        bridge.transport = Some(Transport {
            child,
            stdin: Some(input),
            messages: Some(rx),
            reader: None,
            next_id: 0,
            cancel: bridge.cancel.clone(),
            resource_stop: bridge.resource_stop.clone(),
            diagnostics: bridge.diagnostics.clone(),
            unsupported: HashSet::from(["account/usage/read".into()]),
        });
        bridge.snapshot.collected_at = Some("old-success".into());
        bridge.retry_manually();
        unsafe {
            assert_eq!(
                WaitForSingleObject(witness.as_raw_handle(), 0),
                WAIT_TIMEOUT
            );
        }
        let transport = bridge
            .transport
            .as_mut()
            .expect("manual refresh must keep a healthy connection");
        assert_eq!(
            transport
                .request("account/usage/read", json!({}))
                .unwrap_err(),
            UNSUPPORTED
        );
        assert_eq!(
            transport.next_id, 0,
            "unsupported cached method must not send another RPC"
        );
        assert_eq!(bridge.snapshot.collected_at.as_deref(), Some("old-success"));
        bridge
            .resource_stop
            .store(RESOURCE_WARNING, Ordering::Release);
        bridge.retry_manually();
        assert!(bridge.transport.is_none());
        assert_eq!(bridge.resource_stop.load(Ordering::Acquire), 0);
        unsafe {
            assert_eq!(
                WaitForSingleObject(witness.as_raw_handle(), 1000),
                WAIT_OBJECT_0
            );
        }
        assert_eq!(bridge.snapshot.collected_at.as_deref(), Some("old-success"));
        assert_eq!(bridge.snapshot.status, "stale");
        drop(output);
    }

    #[cfg(windows)]
    #[test]
    fn windows_bridge_hard_denial_is_distinct_from_warning_tree_fuse() {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{
            Foundation::WAIT_OBJECT_0, System::Threading::WaitForSingleObject,
        };
        // Test-only notification omission lets the child attempt one allocation
        // larger than the real hard cap. Production always installs the 96 MiB warning.
        let (mut child, input, output, _, resource) = start_fixture("deny", None);
        child.resume().unwrap();
        let (rx, reader) = markers(output);
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut denied = false;
        while let Ok(line) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            if line == "BRIDGE_DENIED=true" {
                denied = true;
                break;
            }
        }
        let hard_event = resource.load(Ordering::Acquire) == RESOURCE_HARD_LIMIT;
        assert!(
            denied || hard_event,
            "need an allocation denial marker or actual hard-limit event, never generic EOF"
        );
        eprintln!("owned hard-limit fixture: denied_marker={denied}, observed_hard_limit_event={hard_event}");
        drop(input);
        drop(child);
        reader.join().unwrap();
        let (mut child, input, output, _, resource) =
            start_fixture("warning-tree", Some(win_process::WARNING_COMMIT));
        child.resume().unwrap();
        let (rx, reader) = markers(output);
        let _pid = marker(&rx, "BRIDGE_DESCENDANT=");
        unsafe {
            assert_eq!(
                WaitForSingleObject(child.process.as_raw_handle(), 5000),
                WAIT_OBJECT_0
            );
        }
        assert_eq!(resource.load(Ordering::Acquire), RESOURCE_WARNING);
        // The monitor already terminated this job while the transport was idle.
        // Observing the latch must still release the transport and stdout reader
        // without another request, retry, or collection timestamp.
        let (_tx, messages) = mpsc::channel();
        let mut bridge = AccountBridge::new();
        bridge.resource_stop = resource;
        bridge.snapshot.collected_at = Some("old-success".into());
        bridge.snapshot.last_attempt_at = Some("old-attempt".into());
        bridge.transport = Some(Transport {
            child,
            stdin: Some(input),
            messages: Some(messages),
            reader: Some(reader),
            next_id: 7,
            cancel: bridge.cancel.clone(),
            resource_stop: bridge.resource_stop.clone(),
            diagnostics: bridge.diagnostics.clone(),
            unsupported: HashSet::new(),
        });
        assert_eq!(bridge.transport.as_ref().unwrap().next_id, 7);
        assert_eq!(
            bridge.observe_resource_stop(),
            resource_error(RESOURCE_WARNING)
        );
        assert!(bridge.transport.is_none());
        assert_eq!(
            bridge.observe_resource_stop(),
            resource_error(RESOURCE_WARNING)
        );
        assert_eq!(
            bridge.resource_stop.load(Ordering::Acquire),
            RESOURCE_WARNING
        );
        assert_eq!(bridge.snapshot.collected_at.as_deref(), Some("old-success"));
        assert_eq!(
            bridge.snapshot.last_attempt_at.as_deref(),
            Some("old-attempt")
        );
    }
}
