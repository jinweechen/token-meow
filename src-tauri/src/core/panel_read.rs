//! A panel read borrows facts and the existing runtime identity, never a second Core.
use super::*;
use rusqlite::OpenFlags;

pub(super) struct QueryReadContext<'a> {
    pub(super) db: &'a Connection,
    pub(super) source_root: &'a Path,
    pub(super) source_id: &'a str,
    pub(super) runtime_id: &'a str,
}

impl QueryReadContext<'_> {
    pub(super) fn settings(&self) -> Result<Value> {
        Core::settings_from_db_at(self.db, crate::budget_clock::now().utc)
    }
    pub(super) fn primary_source(&self) -> Result<Value> {
        self.db.query_row("SELECT id,root,enabled,client_kind,environment FROM source_instances WHERE id=?1",[self.source_id],|r|Ok(json!({"id":r.get::<_,String>(0)?,"root":r.get::<_,String>(1)?,"enabled":r.get::<_,bool>(2)?,"clientKind":r.get::<_,String>(3)?,"environment":r.get::<_,String>(4)?}))).map_err(err)
    }
    pub(super) fn usage_price_sync(&self) -> Result<Value> {
        pricing_sync::read_usage_price_sync(self.db)
    }
}

/// Owns one pinned WAL read transaction. Its caller releases the Core mutex
/// after construction and drops/finishes this session before returning a reply.
/// A read can legitimately outlive a settings/domain change: admission and
/// response epoch checks belong to the host, not to this historical SQL view.
pub(crate) struct PanelReadSession {
    db: Option<Connection>,
    source_root: PathBuf,
    source_id: String,
    runtime_id: String,
}

impl Core {
    pub(super) fn query_read_context(&self) -> QueryReadContext<'_> {
        QueryReadContext {
            db: &self.db,
            source_root: &self.source_root,
            source_id: &self.source_id,
            runtime_id: &self.runtime_id,
        }
    }

    /// Call while holding the writer's Core mutex. BEGIN alone is deferred;
    /// the source-table SELECT below must finish before releasing that mutex.
    pub(crate) fn begin_panel_read(&self) -> Result<PanelReadSession> {
        let db = Connection::open_with_flags(
            self.data_dir.join("ledger.sqlite3"),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ).map_err(err)?;
        db.busy_timeout(std::time::Duration::from_secs(5)).map_err(err)?;
        let mode: String = db.query_row("PRAGMA journal_mode", [], |row| row.get(0)).map_err(err)?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err("面板只读连接要求 WAL 账本".into());
        }
        let session = PanelReadSession {
            db: Some(db),
            source_root: self.source_root.clone(),
            source_id: self.source_id.clone(),
            runtime_id: self.runtime_id.clone(),
        };
        session.connection().execute_batch("PRAGMA query_only=ON; BEGIN DEFERRED;").map_err(err)?;
        // SELECT 1 would not establish a database snapshot. Read a real table.
        let _: String = session.connection().query_row(
            "SELECT root FROM source_instances WHERE id=?1",
            [&session.source_id],
            |row| row.get(0),
        ).map_err(err)?;
        Ok(session)
    }
}

impl PanelReadSession {
    fn connection(&self) -> &Connection {
        self.db.as_ref().expect("a live panel read owns its connection")
    }
    fn context(&self) -> QueryReadContext<'_> {
        QueryReadContext {
            db: self.connection(),
            source_root: &self.source_root,
            source_id: &self.source_id,
            runtime_id: &self.runtime_id,
        }
    }
    pub(crate) fn snapshot(&self, query: Value) -> Result<Value> {
        queries::snapshot_read(&self.context(), &query)
    }
    pub(crate) fn usage_dashboard_snapshot(&self, query: Value, request: Value) -> Result<Value> {
        queries::usage_dashboard_snapshot_read(&self.context(), query, request)
    }
    pub(crate) fn snapshot_for_panel(&self, query: Value, include_tools: bool) -> Result<Value> {
        queries::snapshot_for_panel_read(&self.context(), &query, include_tools)
    }
    pub(crate) fn usage_dashboard_for_panel(&self, query: Value, request: Value, include_tools: bool) -> Result<Value> {
        queries::usage_dashboard_for_panel_read(&self.context(), query, request, include_tools)
    }
    pub(crate) fn primary_source(&self) -> Result<Value> {
        self.context().primary_source()
    }
    pub(crate) fn usage_price_sync(&self) -> Result<Value> {
        self.context().usage_price_sync()
    }
    /// The host may interrupt active SQL on exit or policy invalidation. This
    /// does not turn an older SQL view into authority for a newer UI domain.
    pub(crate) fn interrupt_handle(&self) -> rusqlite::InterruptHandle {
        self.connection().get_interrupt_handle()
    }
    pub(crate) fn finish(mut self) -> Result<()> {
        let db = self.db.take().expect("a live panel read owns its connection");
        let rollback = if db.is_autocommit() { Ok(()) } else { db.execute_batch("ROLLBACK").map_err(err) };
        let close = db.close().map_err(|(db, error)| {
            drop(db);
            err(error)
        });
        rollback.and(close)
    }
}

impl Drop for PanelReadSession {
    fn drop(&mut self) {
        if let Some(db) = self.db.take() {
            if !db.is_autocommit() { let _ = db.execute_batch("ROLLBACK"); }
            drop(db);
        }
    }
}

#[cfg(test)]
#[path = "panel_read_tests.rs"]
mod tests;
