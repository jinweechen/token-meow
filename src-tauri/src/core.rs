//! Local, read-only Codex log collector. All writes belong to this application's database.
pub mod adapter;
mod agent_kind;
mod motion;
mod panel_read;
mod pet;
pub(super) mod pricing;
mod pricing_sync;
mod queries;
mod reader;

use panel_read::QueryReadContext;

use adapter::SourceAdapter;
use chrono::{Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(crate) type Result<T> = std::result::Result<T, String>;
pub(crate) fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
pub(crate) fn now() -> String {
    crate::budget_clock::now().utc.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
pub(crate) fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(crate) fn strv(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_owned()
}
pub(crate) fn get_setting(db: &Connection, key: &str) -> Result<Option<Value>> {
    db.query_row("SELECT value FROM settings WHERE key=?1", [key], |r| {
        r.get::<_, String>(0)
    })
    .optional()
    .map_err(err)?
    .map(|s| serde_json::from_str(&s).map_err(err))
    .transpose()
}
pub(crate) fn put_setting(db: &Connection, key: &str, v: &Value) -> Result<()> {
    db.execute("INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key,v.to_string()]).map_err(err)?;
    Ok(())
}
pub(crate) fn issue(
    db: &Connection,
    kind: &str,
    file: &str,
    digest: &str,
    message: &str,
) -> Result<()> {
    let id = hash(format!("{kind}|{file}|{digest}").as_bytes());
    db.execute("INSERT OR IGNORE INTO ingestion_issues(id,kind,file,event_time,digest,message) VALUES(?1,?2,?3,?4,?5,?6)",params![id,kind,file,now(),digest,message]).map_err(err)?;
    Ok(())
}
pub(crate) fn json_rows(db: &Connection, table: &str) -> Result<Vec<Value>> {
    // Table is always a static identifier supplied by this module, never user input.
    let mut s = db
        .prepare(&format!("SELECT data FROM {table}"))
        .map_err(err)?;
    let rows = s.query_map([], |r| r.get::<_, String>(0)).map_err(err)?;
    rows.map(|r| serde_json::from_str(&r.map_err(err)?).map_err(err))
        .collect()
}

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    pub files: usize,
    pub lines: usize,
    pub new_responses: usize,
    pub pending_files: usize,
    pub issues: usize,
}

pub struct Core {
    pub(crate) db: Connection,
    pub(crate) data_dir: PathBuf,
    pub(crate) source_root: PathBuf,
    pub(crate) source_id: String,
    pub(crate) runtime_start: String,
    pub(crate) runtime_id: String,
    pet_read_revision: std::cell::Cell<u64>,
    desktop_motion_cache: motion::DesktopMotionCache,
    notification_gate: std::sync::Arc<crate::notification_gate::Gate>,
    budget_observer: queries::budget::Observer,
}
pub struct ValidatedChatTarget {
    uri: String,
}
pub(crate) enum PetChatTarget {
    Open { target: ValidatedChatTarget, thread_id: String },
    CopyRequired { thread_id: String, message: String },
}

// Preserve the public chat error text while allowing the pet's finite action to
// distinguish a verified but unlocatable member from a failed database read.
enum ChatTargetFailure {
    Unverified(String),
    Unlocatable(String),
    Database(String),
}
impl ChatTargetFailure {
    fn message(self) -> String {
        match self {
            Self::Unverified(message) | Self::Unlocatable(message) | Self::Database(message) => message,
        }
    }
}
impl ValidatedChatTarget {
    pub fn as_str(&self) -> &str {
        &self.uri
    }
}

impl Core {
    pub fn open(data_dir: &Path, source_root: &Path) -> Result<Self> {
        validate_path_pair(data_dir, source_root)?;
        fs::create_dir_all(data_dir).map_err(err)?;
        let db_path = data_dir.join("ledger.sqlite3");
        let existed = db_path.exists();
        let mut db = Connection::open(&db_path).map_err(err)?;
        db.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(err)?;
        let version: i64 = db
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(err)?;
        if version > 6 {
            return Err("账本版本比应用更新；保留原数据库，请升级应用".into());
        }
        if existed && version < 6 {
            // VACUUM INTO takes a consistent SQLite snapshot, including committed WAL content.
            let backup =
                data_dir.join(format!("ledger-before-v6-{}.sqlite3", uuid::Uuid::new_v4()));
            db.execute("VACUUM INTO ?1", [backup.to_string_lossy().to_string()])
                .map_err(err)?;
        }
        db.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;",
        )
        .map_err(err)?;
        if version < 6 {
            // All pending generations commit together. A later index failure
            // must not leave an older ledger half upgraded.
            let tx = db
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(err)?;
            if version == 0 {
                tx.execute_batch(
                    &include_str!("../migrations/001_initial.sql")
                        .replace("PRAGMA journal_mode=WAL;", ""),
                )
                .map_err(err)?;
            }
            if version < 2 {
                tx.execute_batch(include_str!("../migrations/002_multi_source.sql"))
                    .map_err(err)?;
            }
            if version < 3 {
                tx.execute_batch(include_str!("../migrations/003_fact_references.sql"))
                    .map_err(err)?;
            }
            if version < 4 {
                tx.execute_batch(include_str!("../migrations/004_usage_lookup_index.sql"))
                    .map_err(err)?;
            }
            if version < 5 {
                tx.execute_batch(include_str!("../migrations/005_pet_read_models.sql"))
                    .map_err(err)?;
            }
            tx.execute_batch(include_str!("../migrations/006_retention_time_indexes.sql"))
                .map_err(err)?;
            tx.commit().map_err(err)?;
        }
        let saved_primary =
            get_setting(&db, "primarySourceId")?.and_then(|v| v.as_str().map(str::to_owned));
        let selected: Option<(String, String)> = if let Some(id) = saved_primary {
            db.query_row(
                "SELECT id,root FROM source_instances WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(err)?
        } else {
            None
        };
        let selected_root = selected
            .as_ref()
            .map(|(_, root)| PathBuf::from(root))
            .unwrap_or_else(|| source_root.to_path_buf());
        let source_root = selected_root.as_path();
        let existing: Option<String> = selected.map(|(id, _)| id).or(db
            .query_row(
                "SELECT id FROM source_instances WHERE root=?1",
                [source_root.to_string_lossy().to_string()],
                |r| r.get(0),
            )
            .optional()
            .map_err(err)?);
        // A directory change is not evidence of migration; explicit relocation retains identity.
        let source_id = existing
            .unwrap_or_else(|| hash(format!("{}|{}", data_dir.display(), now()).as_bytes()));
        db.execute("INSERT INTO source_instances(id,root,created_at) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET root=excluded.root",params![source_id,source_root.to_string_lossy(),now()]).map_err(err)?;
        db.execute(
            "UPDATE source_files SET source_id=?1,channel_id=?2 WHERE source_id=''",
            params![source_id, format!("{source_id}:jsonl")],
        )
        .map_err(err)?;
        register_channel(&db, &source_id)?;
        put_setting(&db, "primarySourceId", &json!(source_id))?;
        if get_setting(&db, "importSince")?.is_none() {
            put_setting(
                &db,
                "importSince",
                &json!((crate::budget_clock::now().utc - Duration::days(30)).to_rfc3339()),
            )?;
        }
        // Fill only absent settings, preserving every explicit historical preference.
        if get_setting(&db, "preferences")?.is_none() {
            put_setting(&db, "preferences", &json!({}))?;
        }
        validate_path_pair(data_dir, source_root)?;
        Ok(Self {
            db,
            data_dir: data_dir.to_path_buf(),
            source_root: source_root.to_path_buf(),
            source_id,
            runtime_start: now(),
            runtime_id: uuid::Uuid::new_v4().to_string(),
            pet_read_revision: std::cell::Cell::new(0),
            desktop_motion_cache: motion::DesktopMotionCache::new(),
            notification_gate: std::sync::Arc::new(crate::notification_gate::Gate::new()),
            budget_observer: queries::budget::Observer::default(),
        })
    }
    pub fn scan(&mut self) -> Result<ScanReport> {
        self.scan_with_cancel(&std::sync::atomic::AtomicBool::new(false))
    }
    pub fn scan_with_cancel(
        &mut self,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<ScanReport> {
        let host = self.notification_gate.snapshot().map(|epoch|(epoch,crate::budget_clock::now()));
        self.scan_with_cancel_host(cancel,host)
    }
    pub(crate) fn scan_with_cancel_host(
        &mut self,
        cancel: &std::sync::atomic::AtomicBool,
        host: Option<(u64,crate::budget_clock::Reading)>,
    ) -> Result<ScanReport> {
        let observation = queries::budget::begin(self,host)?;
        let result = reader::scan(self,cancel,observation);
        if result.is_err() { self.budget_observer.invalidate(); }
        result
    }
    pub(crate) fn notification_gate(&self) -> std::sync::Arc<crate::notification_gate::Gate> {
        std::sync::Arc::clone(&self.notification_gate)
    }
    pub(crate) fn budget_notification_permit(&self, event: &Value) -> Option<crate::notification_gate::NotificationPermit> {
        (event["kind"] == "budget").then(||self.budget_observer.permit(&self.db,&strv(event,"id"))).flatten()
    }
    pub(super) fn budget_prompt_is_current(&self, id: &str) -> bool {
        self.budget_observer.permit(&self.db,id).is_some()
    }
    #[cfg(test)]
    pub(super) fn grant_budget_test_permit(&mut self, id: &str, lifetime_ms: u64) -> crate::notification_gate::NotificationPermit {
        self.budget_observer.test_permit(&self.db,std::sync::Arc::clone(&self.notification_gate),id,lifetime_ms)
    }
    pub fn shutdown(&mut self) -> Result<()> {
        self.db
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(err)
    }
    pub fn settings(&self) -> Result<Value> {
        self.settings_at(crate::budget_clock::now().utc)
    }
    pub(crate) fn settings_at(&self, at: chrono::DateTime<Utc>) -> Result<Value> {
        Self::settings_from_db_at(&self.db, at)
    }
    fn settings_from_db_at(db: &Connection, at: chrono::DateTime<Utc>) -> Result<Value> {
        let mut projected = default_settings();
        if let Some(saved) = get_setting(db, "preferences")? {
            if let Some(m) = saved.as_object() {
                for (k, v) in m {
                    projected[k] = v.clone();
                }
            }
        }
        let time = at
            .with_timezone(&chrono::FixedOffset::east_opt(8 * 3600).unwrap())
            .time();
        let start = projected["dndStart"]
            .as_str()
            .and_then(|s| chrono::NaiveTime::parse_from_str(s, "%H:%M").ok());
        let end = projected["dndEnd"]
            .as_str()
            .and_then(|s| chrono::NaiveTime::parse_from_str(s, "%H:%M").ok());
        let scheduled = match (start, end) {
            (Some(a), Some(b)) if a < b => time >= a && time < b,
            (Some(a), Some(b)) if a > b => time >= a || time < b,
            _ => false,
        };
        projected["effectiveDoNotDisturb"] =
            json!(projected["doNotDisturb"].as_bool().unwrap_or(false) || scheduled);
        Ok(projected)
    }
    pub fn primary_source(&self) -> Result<Value> {
        self.query_read_context().primary_source()
    }
    pub fn sources(&self) -> Result<Vec<Value>> {
        queries::source_rows(self)
    }
    /// Watch configuration only; filesystem checks and health queries stay outside this read.
    pub(crate) fn enabled_codex_source_roots(&self) -> Result<Vec<PathBuf>> {
        let mut statement = self
            .db
            .prepare("SELECT root FROM source_instances WHERE enabled=1 AND client_kind='codex'")
            .map_err(err)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0).map(PathBuf::from))
            .map_err(err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(err)
    }
    /// Configuration roots only, including disabled sources; no fact/health queries.
    pub(crate) fn registered_source_roots(&self) -> Result<Vec<PathBuf>> {
        let mut statement = self
            .db
            .prepare("SELECT root FROM source_instances")
            .map_err(err)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0).map(PathBuf::from))
            .map_err(err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(err)
    }
    pub fn claim_notifications(&mut self) -> Result<Vec<Value>> {
        let dnd = self.settings()?["effectiveDoNotDisturb"]
            .as_bool()
            .unwrap_or(false);
        let runtime = self.runtime_id.clone();
        let tx = self.db.transaction().map_err(err)?;
        let mut statement=tx.prepare("SELECT data FROM inbox_events WHERE json_extract(data,'$.notifyEligible')=1 AND json_extract(data,'$.eligibleRuntime')=?1 AND COALESCE(json_extract(data,'$.notified'),0)=0 ORDER BY event_time LIMIT 100").map_err(err)?;
        let rows = statement
            .query_map([&runtime], |r| r.get::<_, String>(0))
            .map_err(err)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(err)?;
        drop(statement);
        let mut claimed = Vec::new();
        for row in rows {
            let mut value: Value = serde_json::from_str(&row).map_err(err)?;
            let budget_invalid = value["kind"] == "budget"
                && self.budget_observer.permit(&tx,&strv(&value,"id")).is_none();
            let suppressed = dnd || budget_invalid;
            value["notified"] = json!(true);
            value["notificationState"] = json!(if suppressed { "suppressed" } else { "claimed" });
            if suppressed {
                value["animationAcknowledged"] = json!(true);
                if value["kind"] == "budget" { self.budget_observer.finish(&strv(&value,"id")); }
            }
            value["claimedAt"] = json!(now());
            tx.execute(
                "UPDATE inbox_events SET data=?1 WHERE id=?2",
                params![value.to_string(), strv(&value, "id")],
            )
            .map_err(err)?;
            if !suppressed {
                claimed.push(value);
            }
        }
        tx.commit().map_err(err)?;
        Ok(claimed)
    }
    pub fn finish_notification(
        &mut self,
        ids: &[String],
        status: &str,
        error: Option<&str>,
    ) -> Result<()> {
        if !["shown", "sent", "failed", "suppressed"].contains(&status) {
            return Err("未知通知送达状态".into());
        }
        let tx = self.db.transaction().map_err(err)?;
        for id in ids.iter().take(100) {
            if let Some(mut v) = adapter::load_fact(&tx, "inbox_events", id)? {
                v["notificationState"] = json!(status);
                if status == "suppressed" {
                    v["animationAcknowledged"] = json!(true);
                }
                v["deliveryError"] = json!(error.map(|_| "notification_delivery_failed"));
                v["deliveryUpdatedAt"] = json!(now());
                tx.execute(
                    "UPDATE inbox_events SET data=?1 WHERE id=?2",
                    params![v.to_string(), id],
                )
                .map_err(err)?;
            }
        }
        tx.commit().map_err(err)?;
        for id in ids.iter().take(100) { self.budget_observer.finish(id); }
        Ok(())
    }
    pub fn migrate_desktop_settings(&mut self, legacy: Option<Value>) -> Result<Value> {
        if get_setting(&self.db, "desktopMigration")?.is_some() {
            return self.settings();
        }
        let mut saved = get_setting(&self.db, "preferences")?.unwrap_or(json!({}));
        // Existing Core values are authoritative. Only previously absent keys may migrate.
        if let Some(mut old) = legacy {
            if old.get("petScale").is_none() {
                if let Some(scale) = old.get("scale").cloned() {
                    old["petScale"] = scale;
                }
            }
            if let Some(m) = old.as_object() {
                for (k, v) in m {
                    if saved.get(k).is_none() {
                        saved[k] = v.clone();
                    }
                }
            }
        }
        put_setting(&self.db, "preferences", &saved)?;
        put_setting(&self.db, "desktopMigration", &json!(true))?;
        self.settings()
    }
    pub fn snapshot(&self, query: Value) -> Result<Value> {
        queries::snapshot(self, &query)
    }
    pub fn pet_pulse(&self, cursor: Option<Value>) -> Result<Value> {
        pet::pulse(self, cursor.as_ref())
    }
    pub fn pet_details(&self, request: Value) -> Result<Value> {
        pet::details(self, &request)
    }
    pub(crate) fn pet_validate_domain(&self, request: &Value) -> Result<()> {
        pet::validate_domain_at(self, request, Utc::now()).map(|_| ())
    }
    pub(crate) fn pet_chat_target(&self, request: &Value) -> Result<PetChatTarget> {
        pet::chat_target_at(self, request, Utc::now())
    }
    pub(crate) fn next_pet_revision(&self) -> Result<String> {
        let next = self
            .pet_read_revision
            .get()
            .checked_add(1)
            .ok_or("宠物读取序号已耗尽，请重新启动应用")?;
        self.pet_read_revision.set(next);
        Ok(next.to_string())
    }
    pub fn has_thread(&self, thread_id: &str) -> Result<bool> {
        self.db.query_row("SELECT EXISTS(SELECT 1 FROM thread_metadata t JOIN source_instances s ON json_extract(t.data,'$.sourceId')=s.id WHERE json_extract(t.data,'$.id')=?1 AND s.client_kind='codex')",[thread_id],|r|r.get(0)).map_err(err)
    }
    pub fn chat_target(&self, source_id: &str, thread_id: &str) -> Result<ValidatedChatTarget> {
        self.classified_chat_target(source_id, thread_id)
            .map_err(ChatTargetFailure::message)
    }
    fn classified_chat_target(
        &self,
        source_id: &str,
        thread_id: &str,
    ) -> std::result::Result<ValidatedChatTarget, ChatTargetFailure> {
        let kind: Option<String> = self
            .db
            .query_row(
                "SELECT client_kind FROM source_instances WHERE id=?1",
                [source_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|error| ChatTargetFailure::Database(err(error)))?;
        let kind = kind.ok_or_else(|| ChatTargetFailure::Unverified("未知来源实例".into()))?;
        let adapter = adapter::adapter_for(&kind)
            .ok_or_else(|| ChatTargetFailure::Unverified("此来源尚未发布聊天跳转适配器".into()))?;
        let owned: bool = self
            .db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM thread_metadata WHERE id=?1)",
                [format!("{source_id}|{thread_id}")],
                |r| r.get(0),
            )
            .map_err(|error| ChatTargetFailure::Database(err(error)))?;
        if !owned {
            return Err(ChatTargetFailure::Unverified("此会话未在指定来源中验证，请复制会话标识".into()));
        }
        let domains:i64=self.db.query_row("SELECT COUNT(DISTINCT json_extract(data,'$.sourceId')) FROM thread_metadata WHERE json_extract(data,'$.id')=?1",[thread_id],|r|r.get(0))
            .map_err(|error| ChatTargetFailure::Database(err(error)))?;
        if domains > 1 {
            return Err(ChatTargetFailure::Unlocatable("会话标识出现在多个来源，当前聊天协议无法区分来源，请复制会话标识".into()));
        }
        let target = adapter
            .build_chat_target(thread_id)
            .ok_or_else(|| ChatTargetFailure::Unlocatable("会话身份或跳转协议未通过适配器校验".into()))?;
        if kind != "codex"
            || !target.starts_with("codex://threads/")
            || target.contains(['?', '#', '\\', '\n', '\r'])
        {
            return Err(ChatTargetFailure::Unlocatable("来源适配器返回了未允许的聊天目标".into()));
        }
        Ok(ValidatedChatTarget { uri: target })
    }
    pub fn export_csv(&self, query: Value) -> Result<String> {
        queries::export_csv(self, &query)
    }
    pub fn add_source(&mut self, root: &Path) -> Result<String> {
        validate_path_pair(&self.data_dir, root)?;
        let path = root.to_string_lossy().to_string();
        if let Some(id) = self
            .db
            .query_row(
                "SELECT id FROM source_instances WHERE root=?1",
                [&path],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(err)?
        {
            return Ok(id);
        }
        let id = hash(format!("source|{path}|{}", now()).as_bytes());
        self.budget_observer.invalidate();
        let tx = self.db.transaction().map_err(err)?;
        tx.execute(
                "INSERT INTO source_instances(id,root,created_at) VALUES(?1,?2,?3)",
                params![id, path, now()],
            )
            .map_err(err)?;
        register_channel(&tx, &id)?;
        tx.commit().map_err(err)?;
        Ok(id)
    }
    pub fn action(&mut self, name: &str, payload: &Value) -> Result<Value> {
        // Invalidate before a destructive/rebinding operation, even if a partial
        // failure follows. Ordinary preferences and inbox clearing keep carry.
        if ["set_primary_source","update_source","relocate_source","set_project_mapping",
            "pause_history","resume_history","cancel_history","import_history","clear_data","apply_retention"].contains(&name) {
            self.budget_observer.invalidate();
        }
        if ["save_budget","delete_budget"].contains(&name) {
            self.budget_observer.remove_rule(&strv(payload,"id"));
        }
        if name == "clear_inbox" { self.budget_observer.revoke_all(); }
        if ["save_settings","update_settings"].contains(&name)
            && ["retentionDays","doNotDisturb","dndStart","dndEnd"].iter()
                .any(|key| payload.get("settings").unwrap_or(payload).get(*key).is_some()) {
            self.budget_observer.invalidate();
        }
        let directory_before = if name == "merge_session_directory" && queries::budget::has_enabled(self)? {
            Some(queries::budget::configuration_fingerprint(self)?)
        } else { None };
        let result = self.action_inner(name,payload);
        if let Some(before) = directory_before {
            if queries::budget::configuration_fingerprint(self).as_ref() != Ok(&before) {
                self.budget_observer.invalidate();
            }
        }
        result
    }
    fn action_inner(&mut self, name: &str, payload: &Value) -> Result<Value> {
        match name {
            "set_primary_source" => {
                let id = strv(payload, "id");
                let source: Option<Value> = self
                    .db
                    .query_row(
                        "SELECT id,root,enabled,client_kind,environment FROM source_instances WHERE id=?1",
                        [&id],
                        |r| Ok(json!({"id":r.get::<_,String>(0)?,"root":r.get::<_,String>(1)?,"enabled":r.get::<_,bool>(2)?,"clientKind":r.get::<_,String>(3)?,"environment":r.get::<_,String>(4)?})),
                    )
                    .optional()
                    .map_err(err)?;
                let source = source.ok_or("未知来源实例")?;
                let root = PathBuf::from(strv(&source, "root"));
                put_setting(&self.db, "primarySourceId", &json!(id))?;
                // Publish the in-memory binding only after its durable write succeeds.
                self.source_id = id;
                self.source_root = root;
                Ok(source)
            }
            "add_source" => {
                let path = strv(payload, "path");
                if path.is_empty() || !Path::new(&path).is_dir() {
                    return Err("请选择存在的 Codex 来源目录".into());
                }
                let id = self.add_source(Path::new(&path))?;
                Ok(json!({"ok":true,"id":id}))
            }
            "update_source" => {
                let id = strv(payload, "id");
                let exists: bool = self
                    .db
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM source_instances WHERE id=?1)",
                        [&id],
                        |r| r.get(0),
                    )
                    .map_err(err)?;
                if !exists {
                    return Err("未知来源实例".into());
                }
                let path = payload.get("path").and_then(Value::as_str);
                if let Some(path) = path {
                    validate_path_pair(&self.data_dir, Path::new(path))?;
                    if !Path::new(path).is_dir() {
                        return Err("来源目录不存在".into());
                    }
                    let duplicate:bool=self.db.query_row("SELECT EXISTS(SELECT 1 FROM source_instances WHERE root=?1 AND id<>?2)",params![path,id],|r|r.get(0)).map_err(err)?;
                    if duplicate {
                        return Err("此目录已经登记为另一来源，请先停用重复来源".into());
                    }
                }
                let tx = self.db.transaction().map_err(err)?;
                if let Some(path) = path {
                    tx.execute(
                            "UPDATE source_instances SET root=?1 WHERE id=?2",
                            params![path, id],
                        )
                        .map_err(err)?;
                }
                if let Some(enabled) = payload.get("enabled").and_then(Value::as_bool) {
                    tx.execute(
                            "UPDATE source_instances SET enabled=?1 WHERE id=?2",
                            params![enabled, id],
                        )
                        .map_err(err)?;
                }
                tx.commit().map_err(err)?;
                if id == self.source_id {
                    if let Some(path) = path {
                        self.source_root = PathBuf::from(path);
                    }
                }
                Ok(json!({"ok":true}))
            }
            "merge_session_directory" => {
                let rows = payload
                    .get("sessions")
                    .and_then(Value::as_array)
                    .ok_or("sessions 必须是数组")?;
                if rows.len() > 10_000 {
                    return Err(
                        "会话目录超过 10,000 条本地安全上限；未声明完整覆盖，原目录快照保持不变"
                            .into(),
                    );
                }
                let directory_source = payload
                    .get("sourceId")
                    .and_then(Value::as_str)
                    .unwrap_or(&self.source_id)
                    .to_owned();
                if directory_source != self.source_id {
                    return Err("来源已切换，丢弃旧会话目录快照".into());
                }
                for row in rows {
                    let id = strv(row, "id");
                    if id.is_empty() {
                        continue;
                    }
                    let key = format!("{}|{id}", self.source_id);
                    let mut data=adapter::load_fact(&self.db,"thread_metadata",&key)?.unwrap_or_else(||json!({"id":id,"sourceId":self.source_id,"title":format!("会话 {}",id.chars().take(8).collect::<String>()),"cwd":"","parentThreadId":Value::Null,"rootThreadId":Value::Null,"model":Value::Null}));
                    for field in [
                        "title",
                        "cwd",
                        "provider",
                        "cliVersion",
                        "archived",
                        "sessionId",
                    ] {
                        if let Some(value) = row.get(field) {
                            if !value.is_null() && value.as_str() != Some("") {
                                data[field] = value.clone();
                            }
                        }
                    }
                    agent_kind::merge(
                        &mut data,
                        row,
                        "app-server-directory",
                        row.get("directoryProtocolVersion").and_then(Value::as_str),
                    );
                    self.db.execute("INSERT INTO thread_metadata(id,data) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET data=excluded.data",params![key,data.to_string()]).map_err(err)?;
                }
                Ok(json!({"ok":true}))
            }
            "relocate_source" => self.action("update_source", payload),
            "save_settings" | "update_settings" => {
                let mut v = get_setting(&self.db, "preferences")?.unwrap_or(json!({}));
                let p = payload.get("settings").unwrap_or(payload);
                for key in ["idlePlayEnabled", "workAppLinkEnabled", "idleRoamingEnabled"] {
                    if p.get(key).is_some_and(|value| !value.is_boolean()) {
                        return Err(format!("{key} 必须是布尔值"));
                    }
                }
                if let Some(range) = p.get("idleRoamRange") {
                    if !matches!(range.as_str(), Some("nearby" | "workArea")) {
                        return Err("idleRoamRange 必须为 nearby 或 workArea".into());
                    }
                }
                if let Some(enabled) = p.get("workCareEnabled") {
                    if !enabled.is_boolean() {
                        return Err("workCareEnabled 必须是布尔值".into());
                    }
                }
                if let Some(tone) = p.get("careTone") {
                    if !matches!(tone.as_str(), Some("gentle" | "playful" | "mixed")) {
                        return Err("careTone 必须为 gentle、playful 或 mixed".into());
                    }
                }
                if let Some(interval) = p.get("careIntervalMinutes") {
                    if !matches!(interval.as_u64(), Some(25 | 45 | 60)) {
                        return Err("careIntervalMinutes 必须为整数 25、45 或 60".into());
                    }
                }
                for key in ["idlePlayEnabled", "workAppLinkEnabled", "idleRoamingEnabled", "idleRoamRange", "workCareEnabled", "careTone", "careIntervalMinutes"] {
                    if let Some(value) = p.get(key) {
                        v[key] = value.clone();
                    }
                }
                for key in [
                    "muted",
                    "privacyMode",
                    "alwaysOnTop",
                    "clickThrough",
                    "alphaHitTest",
                    "doNotDisturb",
                    "startAtLogin",
                    "reducedMotion",
                    "petVisible",
                ] {
                    if let Some(b) = p.get(key).and_then(Value::as_bool) {
                        v[key] = json!(b);
                    }
                }
                if let Some(n) = p.get("petScale").and_then(Value::as_f64) {
                    v["petScale"] = json!(n.clamp(0.5, 2.0));
                }
                if let Some(n) = p.get("retentionDays").and_then(Value::as_u64) {
                    v["retentionDays"] = json!(n.clamp(1, 3650));
                }
                if let Some(pos) = p.get("petPosition") {
                    if let (Some(x), Some(y), Some(scale)) = (
                        pos.get("logicalX").and_then(Value::as_f64),
                        pos.get("logicalY").and_then(Value::as_f64),
                        pos.get("scaleFactor").and_then(Value::as_f64),
                    ) {
                        if x.is_finite() && y.is_finite() && (0.5..=8.0).contains(&scale) {
                            v["petPosition"] = json!({"monitorId":pos.get("monitorId").and_then(Value::as_str),"logicalX":x,"logicalY":y,"scaleFactor":scale});
                        }
                    }
                }
                for key in ["dndStart", "dndEnd"] {
                    if p.get(key)
                        .is_some_and(|v| v.is_null() || v.as_str() == Some(""))
                    {
                        if let Some(m) = v.as_object_mut() {
                            m.remove(key);
                        }
                        continue;
                    }
                    if let Some(s) = p.get(key).and_then(Value::as_str) {
                        if s.len() == 5 && chrono::NaiveTime::parse_from_str(s, "%H:%M").is_ok() {
                            v[key] = json!(s);
                        }
                    }
                }
                put_setting(&self.db, "preferences", &v)?;
                self.settings()
            }
            "mark_read" => {
                let id = payload.get("id").and_then(Value::as_str);
                let tx = self.db.transaction().map_err(err)?;
                for mut v in json_rows(&tx, "inbox_events")? {
                    if id.is_none() || v.get("id").and_then(Value::as_str) == id {
                        v["read"] = json!(true);
                        tx.execute(
                            "UPDATE inbox_events SET data=?1 WHERE id=?2",
                            params![v.to_string(), strv(&v, "id")],
                        )
                        .map_err(err)?;
                    }
                }
                tx.commit().map_err(err)?;
                Ok(json!({"ok":true}))
            }
            "save_budget" => {
                let scope = payload
                    .get("scope")
                    .and_then(Value::as_str)
                    .unwrap_or("daily");
                if !["daily", "project", "thread", "rootTurn"].contains(&scope) {
                    return Err("不支持的预算范围".into());
                }
                let threshold = payload
                    .get("threshold")
                    .and_then(Value::as_i64)
                    .filter(|n| *n > 0)
                    .ok_or("预算阈值必须是正整数")?;
                let target = payload.get("target").and_then(Value::as_str).unwrap_or("");
                if scope != "daily" && target.is_empty() {
                    return Err("此预算需要指定项目、会话或根任务".into());
                }
                let mut source_id = payload
                    .get("sourceId")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned);
                if source_id.is_none() && ["thread", "rootTurn"].contains(&scope) {
                    let sql = if scope == "thread" {
                        "SELECT DISTINCT json_extract(data,'$.sourceId') FROM thread_metadata WHERE json_extract(data,'$.id')=?1"
                    } else {
                        "SELECT DISTINCT json_extract(data,'$.sourceId') FROM turns WHERE turn_id=?1"
                    };
                    let mut s = self.db.prepare(sql).map_err(err)?;
                    let candidates = s
                        .query_map([target], |r| r.get::<_, String>(0))
                        .map_err(err)?
                        .collect::<std::result::Result<Vec<_>, _>>()
                        .map_err(err)?;
                    if candidates.len() != 1 {
                        return Err("任务或会话的来源不明确，请选择具体来源".into());
                    }
                    source_id = candidates.first().cloned();
                }
                if let Some(source) = &source_id {
                    let exists: bool = self
                        .db
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM source_instances WHERE id=?1)",
                            [source],
                            |r| r.get(0),
                        )
                        .map_err(err)?;
                    if !exists {
                        return Err("预算指定了未知来源".into());
                    }
                }
                let id = payload
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| hash(format!("{scope}|{target}|{}", now()).as_bytes()));
                let previous: Option<String> = self
                    .db
                    .query_row("SELECT data FROM budgets WHERE id=?1", [&id], |r| r.get(0))
                    .optional()
                    .map_err(err)?;
                let version = previous
                    .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                    .and_then(|v| v["version"].as_u64())
                    .unwrap_or(0)
                    + 1;
                let v = json!({"id":id,"version":version,"scope":scope,"sourceId":source_id,"target":if target.is_empty(){None}else{Some(target)},"threshold":threshold,"enabled":payload.get("enabled").and_then(Value::as_bool).unwrap_or(true)});
                self.db.execute("INSERT INTO budgets(id,data) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET data=excluded.data",params![id,v.to_string()]).map_err(err)?;
                queries::initialize_budget(self, &id)?;
                Ok(v)
            }
            "delete_budget" => {
                self.db
                    .execute("DELETE FROM budgets WHERE id=?1", [strv(payload, "id")])
                    .map_err(err)?;
                Ok(json!({"ok":true}))
            }
            "set_project_mapping" => {
                let path = strv(payload, "path");
                let project = strv(payload, "project");
                if path.is_empty() || project.is_empty() {
                    return Err("需要目录和项目名称".into());
                }
                let mut m = get_setting(&self.db, "projectMappings")?.unwrap_or(json!({}));
                m[path] = json!(project);
                put_setting(&self.db, "projectMappings", &m)?;
                Ok(m)
            }
            "pause_history" => {
                if get_setting(&self.db, "historyCancelled")?.and_then(|v| v.as_bool())
                    == Some(true)
                {
                    return Err("本次补导已取消，请明确选择日期重新补导".into());
                }
                put_setting(&self.db, "historyPaused", &json!(true))?;
                Ok(json!({"ok":true}))
            }
            "resume_history" => {
                if get_setting(&self.db, "historyCancelled")?.and_then(|v| v.as_bool())
                    == Some(true)
                {
                    return Err("取消的补导不能恢复，请明确选择日期重新补导".into());
                }
                put_setting(&self.db, "historyPaused", &json!(false))?;
                Ok(json!({"ok":true}))
            }
            "cancel_history" => {
                let tx = self.db.transaction().map_err(err)?;
                put_setting(&tx, "historyCancelled", &json!(true))?;
                put_setting(&tx, "historyPaused", &json!(false))?;
                put_setting(&tx, "historyCancelledAt", &json!(now()))?;
                tx.commit().map_err(err)?;
                Ok(json!({"ok":true,"status":"cancelled"}))
            }
            "import_history" => {
                let since = payload
                    .get("from")
                    .and_then(Value::as_str)
                    .unwrap_or("1970-01-01T00:00:00Z");
                let normalized = if since.len() == 10 {
                    format!("{since}T00:00:00+08:00")
                } else {
                    since.into()
                };
                chrono::DateTime::parse_from_rfc3339(&normalized).map_err(|_| "补导日期无效")?;
                let days = self.settings()?["retentionDays"].as_i64().unwrap_or(90);
                let tx = self.db.transaction().map_err(err)?;
                put_setting(&tx, "historyReplayAt", &json!(now()))?;
                put_setting(&tx, "importSince", &json!(normalized))?;
                put_setting(&tx, "historyPaused", &json!(false))?;
                put_setting(&tx, "historyCancelled", &json!(false))?;
                // Explicit user history import removes the clear watermark and rebuilds physical cursors.
                tx.execute("DELETE FROM settings WHERE key='clearWatermark'", [])
                    .map_err(err)?;
                let mut statement = tx
                    .prepare("SELECT id,context FROM source_files")
                    .map_err(err)?;
                let files = statement
                    .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
                    .map_err(err)?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(err)?;
                drop(statement);
                for (id, context) in files {
                    let old: reader::Context = serde_json::from_str(&context).unwrap_or_default();
                    let fresh = reader::Context {
                        live_cursor: old.live_cursor,
                        ..reader::Context::default()
                    };
                    tx.execute("UPDATE source_files SET offset=0,last_start=0,last_hash='',context=?1,generation=generation+1 WHERE id=?2",params![serde_json::to_string(&fresh).map_err(err)?,id]).map_err(err)?;
                }
                // A larger explicit import cannot retain a stale, narrower retention boundary.
                put_setting(
                    &tx,
                    "retentionWatermark",
                    &json!((crate::budget_clock::now().utc - Duration::days(days))
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
                )?;
                tx.commit().map_err(err)?;
                Ok(json!({"ok":true,"status":"queued"}))
            }
            "clear_data" => {
                let watermark = now();
                let tx = self.db.transaction().map_err(err)?;
                for table in [
                    "usage_responses",
                    "legacy_usage_segments",
                    "turns",
                    "items",
                    "thread_metadata",
                    "inbox_events",
                    "identity_conflicts",
                    "ingestion_issues",
                    "ingested_records",
                    "account_snapshots",
                    "account_usage_snapshots",
                    "usage_aggregates",
                    "context_snapshots",
                    "timing_observations",
                    "fact_observations",
                    "fact_aliases",
                    "excluded_facts",
                    "account_bindings",
                ] {
                    tx.execute(&format!("DELETE FROM {table}"), [])
                        .map_err(err)?;
                }
                put_setting(&tx, "clearWatermark", &json!(watermark))?;
                tx.execute(
                    "UPDATE source_files SET context=json_set(context,'$.legacy',json('{}'))",
                    [],
                )
                .map_err(err)?;
                tx.commit().map_err(err)?;
                Ok(json!({"ok":true,"watermark":watermark}))
            }
            "clear_inbox" => {
                let tx = self.db.transaction().map_err(err)?;
                tx.execute("DELETE FROM inbox_events", []).map_err(err)?;
                put_setting(&tx, "inboxWatermark", &json!(now()))?;
                tx.commit().map_err(err)?;
                Ok(json!({"ok":true}))
            }
            "acknowledge_delivery" => {
                let id = strv(payload, "id");
                if let Some(mut v) = adapter::load_fact(&self.db, "inbox_events", &id)? {
                    v["animationAcknowledged"] = json!(true);
                    self.db
                        .execute(
                            "UPDATE inbox_events SET data=?1 WHERE id=?2",
                            params![v.to_string(), id],
                        )
                        .map_err(err)?;
                }
                Ok(json!({"ok":true}))
            }
            "apply_retention" => {
                queries::apply_retention(self)?;
                Ok(json!({"ok":true}))
            }
            _ => Err(format!("不支持的本地操作：{name}")),
        }
    }
}

fn default_settings() -> Value {
    json!({"muted":true,"privacyMode":false,"alwaysOnTop":true,"clickThrough":false,"alphaHitTest":false,"petScale":1.0,"doNotDisturb":false,"reducedMotion":false,"startAtLogin":false,"petVisible":true,"retentionDays":90,"skin":"anime-catgirl","idlePlayEnabled":true,"workAppLinkEnabled":true,"idleRoamingEnabled":true,"idleRoamRange":"workArea","workCareEnabled":true,"careTone":"mixed","careIntervalMinutes":25})
}
fn register_channel(db: &Connection, source: &str) -> Result<()> {
    let channel = format!("{source}:jsonl");
    let mut capabilities = adapter::CodexAdapter.capabilities();
    capabilities["sourceId"] = json!(source);
    capabilities["channelId"] = json!(channel);
    let root: String = db
        .query_row(
            "SELECT root FROM source_instances WHERE id=?1",
            [source],
            |r| r.get(0),
        )
        .map_err(err)?;
    let config = adapter::SourceConfig {
        id: source.into(),
        root: PathBuf::from(root),
        environment: "windows-local".into(),
    };
    capabilities["probe"] = adapter::CodexAdapter.probe(&config);
    capabilities["identityPolicy"] =
        serde_json::to_value(adapter::CodexAdapter.identity_policy()).map_err(err)?;
    db.execute("INSERT OR IGNORE INTO collection_channels(id,source_id,kind,adapter_version,data) VALUES(?1,?2,'jsonl','codex-0.147-v1',?3)",params![channel,source,json!({"id":channel,"sourceId":source,"kind":"jsonl","enabled":true}).to_string()]).map_err(err)?;
    db.execute("INSERT INTO source_capabilities(id,source_id,data) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET data=excluded.data",params![channel,source,capabilities.to_string()]).map_err(err)?;
    Ok(())
}

fn resolved_path(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_err(err)?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    let mut existing = normalized.as_path();
    let mut tail = Vec::new();
    while !existing.exists() {
        let name = existing.file_name().ok_or("路径不能解析到有效目录")?;
        tail.push(name.to_os_string());
        existing = existing.parent().ok_or("路径没有可解析父目录")?;
    }
    let mut resolved = fs::canonicalize(existing).map_err(err)?;
    for part in tail.into_iter().rev() {
        resolved.push(part);
    }
    Ok(resolved)
}
fn validate_path_pair(data: &Path, source: &Path) -> Result<()> {
    let data = resolved_path(data)?;
    let source = resolved_path(source)?;
    let key = |p: &Path| {
        let s = p
            .to_string_lossy()
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_owned();
        #[cfg(windows)]
        {
            s.to_lowercase()
        }
        #[cfg(not(windows))]
        {
            s
        }
    };
    let d = key(&data);
    let s = key(&source);
    if d == s || d.starts_with(&format!("{s}/")) || s.starts_with(&format!("{d}/")) {
        return Err("宠物数据目录与 Codex 来源目录不得相同或互相包含；源文件始终只读".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod source_watch_tests;
