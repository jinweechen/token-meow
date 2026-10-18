//! Read-only work permission for desktop movement, independent of panel filters.
//!
//! This is an admission observation, not a lease or proof of OS/renderer readiness.
//! The desktop owner must additionally close permission on collector/watcher errors,
//! unavailable locks, input, visibility, interaction, and stale native observations.
use super::*;
use chrono::{DateTime, Utc};

// Bound movement admission to 15 seconds of collector evidence; never reuse a
// prior-process zero-pending record or future/stale observation as permission.
const MAX_OBSERVATION_AGE_MS: i64 = 15_000;

// pet::activity reads json_extract through ValueRef::Text, not json_type='text'.
// JSON arrays/objects are SQL TEXT too; numeric/boolean/null ids become "" there.
// A TEXT root different from that id is a child; every other active row is a root.
// No freshness clause: both fresh activity and stale/unknown activity block motion.
const ACTIVE_ROOT_EXISTS: &str = "SELECT EXISTS(SELECT 1 FROM turns \
    WHERE json_extract(data,'$.status')='active' AND (\
    typeof(json_extract(data,'$.rootTurnId'))<>'text' OR \
    json_extract(data,'$.rootTurnId')=CASE \
    WHEN typeof(json_extract(data,'$.id'))='text' THEN json_extract(data,'$.id') \
    ELSE '' END))";

fn timestamp(value: &Value) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.as_str()?)
        .ok()
        .map(|time| time.with_timezone(&Utc))
}

fn deadline(value: &Value, booted_at: DateTime<Utc>, at: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let time = timestamp(value)?;
    let until = time.checked_add_signed(Duration::milliseconds(MAX_OBSERVATION_AGE_MS))?;
    (time >= booted_at && time <= at && at <= until).then_some(until)
}

struct Admission {
    valid_until: DateTime<Utc>,
    source_roots: Vec<PathBuf>,
}

struct CachedAdmission {
    changes: u64,
    idle: bool,
    observation: Admission,
}

pub(super) struct DesktopMotionCache {
    booted_at: DateTime<Utc>,
    last_observed_at: Option<DateTime<Utc>>,
    last_checked_at: Option<DateTime<Utc>>,
    entry: Option<CachedAdmission>,
    #[cfg(test)]
    root_queries: u64,
}

impl DesktopMotionCache {
    pub(super) fn new() -> Self {
        Self {
            booted_at: Utc::now(),
            last_observed_at: None,
            last_checked_at: None,
            entry: None,
            #[cfg(test)]
            root_queries: 0,
        }
    }

    fn check_clock(&mut self, at: DateTime<Utc>) -> bool {
        if at < self.booted_at || self.last_checked_at.is_some_and(|previous| at < previous) {
            self.entry = None;
            return false;
        }
        self.last_checked_at = Some(at);
        true
    }
}

impl Core {
    /// Return true only for a fresh, completely observed local collection with
    /// no active root turn anywhere in the ledger. No account or usage query.
    ///
    /// schema6 has no status expression index: an uncached EXISTS may scan turns.
    /// Within the single-writer Core, total_changes invalidates that verdict on
    /// every data mutation; repeated native checks only reuse it before the
    /// earliest collector expiry. Source path existence is checked even on a hit.
    /// Do not treat this bounded observation as a renderer/native movement lease.
    pub(crate) fn desktop_motion_idle(&mut self) -> Result<bool> {
        self.desktop_motion_idle_at(Utc::now())
    }

    /// UI-thread execution check: no filesystem access and no SQL query. The
    /// maintenance worker must have refreshed the full observation <500ms ago.
    /// Core's mutex fences mutations through the synchronous native move.
    pub(crate) fn desktop_motion_cached_idle(&mut self) -> bool {
        self.desktop_motion_cached_idle_at(Utc::now())
    }

    fn desktop_motion_cached_idle_at(&mut self, at: DateTime<Utc>) -> bool {
        if !self.desktop_motion_cache.check_clock(at) { return false; }
        let recent = self.desktop_motion_cache.last_observed_at.is_some_and(|observed|
            at >= observed && at.signed_duration_since(observed) < Duration::milliseconds(500));
        let valid = recent && self.desktop_motion_cache.entry.as_ref().is_some_and(|cached|
            cached.idle && cached.changes == self.db.total_changes() && at <= cached.observation.valid_until);
        if !valid { self.desktop_motion_cache.entry = None; }
        valid
    }

    fn desktop_motion_idle_at(&mut self, at: DateTime<Utc>) -> Result<bool> {
        if !self.desktop_motion_cache.check_clock(at) { return Ok(false); }
        self.desktop_motion_cache.last_observed_at = Some(at);
        let changes = self.db.total_changes();
        if let Some(cached) = &self.desktop_motion_cache.entry {
            if cached.changes == changes && at <= cached.observation.valid_until {
                let paths_available = cached.observation.source_roots.iter().all(|root| root.is_dir());
                if paths_available {
                    return Ok(cached.idle);
                }
                self.desktop_motion_cache.entry = None;
                return Ok(false);
            }
        }
        // Any failed refresh/error leaves no previous true verdict behind.
        self.desktop_motion_cache.entry = None;
        let Some(observation) = self.desktop_motion_admission_at(at)? else {
            return Ok(false);
        };
        #[cfg(test)]
        { self.desktop_motion_cache.root_queries += 1; }
        let active: bool = self.db.query_row(ACTIVE_ROOT_EXISTS, [], |row| row.get(0)).map_err(err)?;
        let idle = !active;
        self.desktop_motion_cache.entry = Some(CachedAdmission { changes, idle, observation });
        Ok(idle)
    }

    fn desktop_motion_admission_at(&self, at: DateTime<Utc>) -> Result<Option<Admission>> {
        let booted_at = self.desktop_motion_cache.booted_at;
        let Some(scan) = get_setting(&self.db, "lastSuccessfulScan")? else {
            return Ok(None);
        };
        let Some(mut valid_until) = deadline(&scan, booted_at, at) else {
            return Ok(None);
        };
        if get_setting(&self.db, "pendingFiles")?.and_then(|v| v.as_u64()) != Some(0) {
            return Ok(None);
        }
        for key in ["historyPaused", "historyCancelled"] {
            // These flags are absent until first used in the existing collector.
            // An explicit false is clear; malformed values and true close admission.
            if get_setting(&self.db, key)?.is_some_and(|v| v.as_bool() != Some(false)) {
                return Ok(None);
            }
        }
        let mut statement = self.db.prepare(
            "SELECT id,root,client_kind,environment FROM source_instances WHERE enabled=1",
        ).map_err(err)?;
        let mut rows = statement.query([]).map_err(err)?;
        let mut source_roots = Vec::new();
        while let Some(row) = rows.next().map_err(err)? {
            let id: String = row.get(0).map_err(err)?;
            let root: String = row.get(1).map_err(err)?;
            let kind: String = row.get(2).map_err(err)?;
            let environment: String = row.get(3).map_err(err)?;
            if kind != "codex" || environment != "windows-local" || !Path::new(&root).is_dir() {
                return Ok(None);
            }
            let Some(check) = get_setting(&self.db, &format!("lastSourceCheck:{id}"))? else {
                return Ok(None);
            };
            let Some(check_deadline) = deadline(&check["checkedAt"], booted_at, at) else {
                return Ok(None);
            };
            if !check.is_object()
                || check["discoveryComplete"].as_bool() != Some(true)
                || check["cancelled"].as_bool() != Some(false)
                || ["pendingFiles", "issues", "unsupportedFiles", "discoveryErrors"]
                    .into_iter().any(|key| check[key].as_u64() != Some(0))
                || check["files"].as_u64().is_none()
                || get_setting(&self.db, &format!("historyPending:{id}"))?
                    .and_then(|v| v.as_u64()) != Some(0)
            {
                return Ok(None);
            }
            let Some(success) = get_setting(&self.db, &format!("lastSourceSuccess:{id}"))? else {
                return Ok(None);
            };
            let Some(success_deadline) = deadline(&success, booted_at, at) else {
                return Ok(None);
            };
            valid_until = valid_until.min(check_deadline).min(success_deadline);
            source_roots.push(PathBuf::from(root));
        }
        if source_roots.is_empty() {
            return Ok(None);
        }
        Ok(Some(Admission { valid_until, source_roots }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        _temp: tempfile::TempDir,
        core: Core,
        at: DateTime<Utc>,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join("source");
            fs::create_dir(&source).unwrap();
            let core = Core::open(&temp.path().join("pet"), &source).unwrap();
            let at = Utc::now() + Duration::seconds(1);
            Self { _temp: temp, core, at }
        }

        fn ready(&self) {
            let time = json!(self.at.to_rfc3339());
            for key in ["lastSuccessfulScan", &format!("lastSourceSuccess:{}", self.core.source_id)] {
                put_setting(&self.core.db, key, &time).unwrap();
            }
            put_setting(&self.core.db, "pendingFiles", &json!(0)).unwrap();
            put_setting(&self.core.db, &format!("historyPending:{}", self.core.source_id), &json!(0)).unwrap();
            put_setting(&self.core.db, &format!("lastSourceCheck:{}", self.core.source_id), &json!({
                "checkedAt":time,"pendingFiles":0,"issues":0,"files":0,"unsupportedFiles":0,
                "discoveryComplete":true,"discoveryErrors":0,"cancelled":false,
            })).unwrap();
        }

        fn idle(&mut self) -> bool {
            self.core.desktop_motion_idle_at(self.at).unwrap()
        }

        fn turn(&self, name: &str, data: Value) {
            self.core.db.execute(
                "INSERT INTO turns(id,thread_id,turn_id,event_time,data) VALUES(?1,'thread',?1,?2,?3)",
                params![name, self.at.to_rfc3339(), data.to_string()],
            ).unwrap();
        }

        fn check(&self) -> Value {
            get_setting(&self.core.db, &format!("lastSourceCheck:{}", self.core.source_id)).unwrap().unwrap()
        }

        fn set_check(&self, check: &Value) {
            put_setting(&self.core.db, &format!("lastSourceCheck:{}", self.core.source_id), check).unwrap();
        }
    }

    #[test]
    fn motion_idle_requires_current_complete_known_collection_and_does_not_write() {
        let mut fixture = Fixture::new();
        assert!(!fixture.core.desktop_motion_idle().unwrap());
        fixture.ready();
        let changes = fixture.core.db.total_changes();
        assert!(fixture.idle());
        assert_eq!(fixture.core.db.total_changes(), changes);
        // Optional usage, inbox, account and full-snapshot tables are unrelated.
        fixture.core.db.execute_batch("DROP TABLE account_snapshots; DROP TABLE account_usage_snapshots; DROP TABLE usage_responses;").unwrap();
        fixture.core.desktop_motion_cache.entry = None;
        assert!(fixture.idle());
    }

    #[test]
    fn motion_idle_blocks_fresh_stale_and_missing_time_active_roots_globally() {
        for case in 0..4 {
            let mut fixture = Fixture::new();
            fixture.ready();
            let time = match case {
                0 => json!(fixture.at.to_rfc3339()),
                1 => json!("2000-01-01T00:00:00Z"),
                2 => Value::Null,
                _ => json!("not-time"),
            };
            fixture.turn("active-root", json!({"id":"root","rootTurnId":"root","sourceId":"other","status":"active","lastEventAt":time}));
            assert!(!fixture.idle());
        }
    }

    #[test]
    fn motion_idle_matches_pet_sql_text_root_semantics_including_unknown_ids() {
        let cases = [
            (json!({"id":"child","rootTurnId":"parent","status":"active"}), true),
            (json!({"id":"root","rootTurnId":"root","status":"active"}), false),
            (json!({"id":"root","rootTurnId":null,"status":"active"}), false),
            (json!({"id":"root","status":"active"}), false),
            (json!({"id":7,"rootTurnId":null,"status":"active"}), false),
            (json!({"id":7,"rootTurnId":"","status":"active"}), false),
            (json!({"id":7,"rootTurnId":"parent","status":"active"}), true),
            (json!({"id":"root","rootTurnId":false,"status":"active"}), false),
            (json!({"id":{"x":1},"rootTurnId":{"x":1},"status":"active"}), false),
            (json!({"id":"child","rootTurnId":["parent"],"status":"active"}), true),
        ];
        for (turn, idle) in cases {
            let mut fixture = Fixture::new();
            fixture.ready();
            fixture.turn("one", turn);
            assert_eq!(fixture.idle(), idle);
        }
    }

    #[test]
    fn motion_idle_does_not_reinterpret_ended_unknown_failed_or_completed_as_running() {
        let mut fixture = Fixture::new();
        fixture.ready();
        for status in ["completed", "failed", "interrupted", "unknown"] {
            fixture.turn(status, json!({"id":status,"rootTurnId":status,"status":status}));
        }
        assert!(fixture.idle());
    }

    #[test]
    fn motion_idle_unknown_or_partial_collector_metrics_never_mean_zero() {
        let mut fixture = Fixture::new();
        for key in ["pendingFiles", "issues", "files", "unsupportedFiles", "discoveryErrors", "discoveryComplete", "cancelled", "checkedAt"] {
            fixture.ready();
            let mut check = fixture.check();
            check.as_object_mut().unwrap().remove(key);
            fixture.set_check(&check);
            assert!(!fixture.idle(), "missing {key}");
        }
        for (key, value) in [
            ("pendingFiles", json!(1)), ("issues", json!(1)),
            ("unsupportedFiles", json!(1)), ("discoveryErrors", json!(1)),
            ("discoveryComplete", json!(false)), ("cancelled", json!(true)),
            ("pendingFiles", json!("0")), ("files", json!(-1)),
        ] {
            fixture.ready();
            let mut check = fixture.check();
            check[key] = value;
            fixture.set_check(&check);
            assert!(!fixture.idle(), "invalid {key}");
        }
    }

    #[test]
    fn motion_idle_requires_each_enabled_source_and_known_history_completion() {
        let mut fixture = Fixture::new();
        fixture.ready();
        for key in ["pendingFiles".to_owned(), format!("historyPending:{}", fixture.core.source_id), format!("lastSourceSuccess:{}", fixture.core.source_id)] {
            fixture.ready();
            fixture.core.db.execute("DELETE FROM settings WHERE key=?1", [&key]).unwrap();
            assert!(!fixture.idle(), "missing {key}");
        }
        for key in ["pendingFiles".to_owned(), format!("historyPending:{}", fixture.core.source_id)] {
            fixture.ready();
            put_setting(&fixture.core.db, &key, &json!(1)).unwrap();
            assert!(!fixture.idle(), "pending {key}");
        }
        fixture.ready();
        fixture.core.db.execute("INSERT INTO source_instances(id,root,created_at) VALUES('unchecked',?1,?2)", params![fixture.core.source_root.to_string_lossy().to_string(), fixture.at.to_rfc3339()]).unwrap();
        assert!(!fixture.idle());
        fixture.core.db.execute("UPDATE source_instances SET enabled=0 WHERE id='unchecked'", []).unwrap();
        assert!(fixture.idle());
        fixture.core.db.execute("UPDATE source_instances SET enabled=0", []).unwrap();
        assert!(!fixture.idle());
    }

    #[test]
    fn motion_idle_closes_on_paused_cancelled_missing_or_unsupported_sources() {
        let mut fixture = Fixture::new();
        fixture.ready();
        for key in ["historyPaused", "historyCancelled"] {
            put_setting(&fixture.core.db, key, &json!(true)).unwrap();
            assert!(!fixture.idle());
            put_setting(&fixture.core.db, key, &json!("false")).unwrap();
            assert!(!fixture.idle());
            put_setting(&fixture.core.db, key, &json!(false)).unwrap();
        }
        assert!(fixture.idle());
        let missing = fixture._temp.path().join("missing-source").to_string_lossy().to_string();
        for (column, value) in [("root", missing.as_str()), ("client_kind", "unknown"), ("environment", "wsl")] {
            let original: String = fixture.core.db.query_row(&format!("SELECT {column} FROM source_instances WHERE id=?1"), [&fixture.core.source_id], |row| row.get(0)).unwrap();
            fixture.core.db.execute(&format!("UPDATE source_instances SET {column}=?1 WHERE id=?2"), params![value, fixture.core.source_id]).unwrap();
            assert!(!fixture.idle());
            fixture.core.db.execute(&format!("UPDATE source_instances SET {column}=?1 WHERE id=?2"), params![original, fixture.core.source_id]).unwrap();
        }
        assert!(fixture.idle());
    }

    #[test]
    fn motion_idle_rejects_stale_future_and_previous_process_success() {
        let mut fixture = Fixture::new();
        fixture.ready();
        assert!(fixture.core.desktop_motion_idle_at(fixture.at + Duration::milliseconds(15_000)).unwrap());
        assert!(!fixture.core.desktop_motion_idle_at(fixture.at + Duration::milliseconds(15_001)).unwrap());
        assert!(!fixture.core.desktop_motion_idle_at(fixture.at - Duration::milliseconds(1)).unwrap());
        put_setting(&fixture.core.db, "lastSuccessfulScan", &json!("2000-01-01T00:00:00Z")).unwrap();
        assert!(!fixture.idle());
    }

    #[test]
    fn motion_idle_cached_verdict_reuses_same_data_but_invalidates_every_mutation() {
        let mut fixture = Fixture::new();
        fixture.ready();
        assert!(fixture.idle());
        assert_eq!(fixture.core.desktop_motion_cache.root_queries, 1);
        fixture.at += Duration::milliseconds(50);
        assert!(fixture.idle());
        assert_eq!(fixture.core.desktop_motion_cache.root_queries, 1);
        fixture.turn("new-root", json!({"id":"new-root","status":"active"}));
        assert!(!fixture.idle());
        assert_eq!(fixture.core.desktop_motion_cache.root_queries, 2);
        assert!(!fixture.idle());
        assert_eq!(fixture.core.desktop_motion_cache.root_queries, 2);
        fixture.core.db.execute("DELETE FROM turns WHERE id='new-root'", []).unwrap();
        assert!(fixture.idle());
        assert_eq!(fixture.core.desktop_motion_cache.root_queries, 3);
        put_setting(&fixture.core.db, "pendingFiles", &json!(1)).unwrap();
        assert!(!fixture.idle());
        assert!(fixture.core.desktop_motion_cache.entry.is_none());
        assert_eq!(fixture.core.desktop_motion_cache.root_queries, 3);
    }

    #[test]
    fn motion_idle_cache_uses_earliest_observation_deadline_and_closes_on_rollback() {
        let mut fixture = Fixture::new();
        fixture.ready();
        // One older (but current-process) observation is the permission deadline.
        fixture.at += Duration::seconds(1);
        put_setting(&fixture.core.db, "lastSuccessfulScan", &json!(fixture.at.to_rfc3339())).unwrap();
        assert!(fixture.idle());
        let until = fixture.core.desktop_motion_cache.entry.as_ref().unwrap().observation.valid_until;
        assert_eq!(until, fixture.at + Duration::seconds(14));
        fixture.at = until;
        assert!(fixture.idle());
        assert_eq!(fixture.core.desktop_motion_cache.root_queries, 1);
        fixture.at += Duration::milliseconds(1);
        assert!(!fixture.idle());
        assert!(fixture.core.desktop_motion_cache.entry.is_none());
        assert_eq!(fixture.core.desktop_motion_cache.root_queries, 1);
        fixture.ready();
        assert!(fixture.idle());
        fixture.at -= Duration::milliseconds(1);
        assert!(!fixture.idle());
        assert!(fixture.core.desktop_motion_cache.entry.is_none());
    }

    #[test]
    fn motion_idle_reopened_core_cannot_reuse_persisted_prior_process_admission() {
        let mut fixture = Fixture::new();
        fixture.at = Utc::now();
        fixture.ready();
        assert!(fixture.idle());
        let data = fixture.core.data_dir.clone();
        let source = fixture.core.source_root.clone();
        fixture.core = Core::open(&data, &source).unwrap();
        fixture.at = Utc::now();
        assert!(!fixture.idle());
        assert_eq!(fixture.core.desktop_motion_cache.root_queries, 0);
        fixture.ready();
        assert!(fixture.idle());
    }

    #[test]
    fn motion_idle_cache_checks_source_path_and_cannot_hide_query_errors_after_data_change() {
        let mut fixture = Fixture::new();
        fixture.ready();
        assert!(fixture.idle());
        let source = fixture.core.source_root.clone();
        fs::remove_dir(&source).unwrap();
        assert!(!fixture.idle());
        assert!(fixture.core.desktop_motion_cache.entry.is_none());
        fs::create_dir(&source).unwrap();
        assert!(fixture.idle());
        fixture.core.db.execute_batch("DROP TABLE turns;").unwrap();
        // Schema changes are outside normal collector operation and do not
        // increment total_changes; a data write explicitly invalidates this hit.
        put_setting(&fixture.core.db, "testCacheInvalidation", &json!(true)).unwrap();
        assert!(fixture.core.desktop_motion_idle_at(fixture.at).is_err());
        assert!(fixture.core.desktop_motion_cache.entry.is_none());
    }

    #[test]
    fn motion_idle_database_failures_are_errors_not_permission() {
        let mut fixture = Fixture::new();
        fixture.ready();
        fixture.core.db.execute_batch("DROP TABLE turns;").unwrap();
        assert!(fixture.core.desktop_motion_idle_at(fixture.at).is_err());
    }

    #[test]
    fn motion_settings_validate_atomically_and_project_both_keys() {
        let mut fixture = Fixture::new();
        assert_eq!(fixture.core.settings().unwrap()["idleRoamingEnabled"], true);
        assert_eq!(fixture.core.settings().unwrap()["idleRoamRange"], "workArea");
        fixture.core.action("update_settings", &json!({"idleRoamingEnabled":false,"idleRoamRange":"nearby"})).unwrap();
        let baseline = get_setting(&fixture.core.db, "preferences").unwrap();
        for patch in [
            json!({"idleRoamingEnabled":"true","idleRoamRange":"workArea","muted":false}),
            json!({"idleRoamingEnabled":null,"muted":false}),
            json!({"idleRoamingEnabled":true,"idleRoamRange":"all-monitors","muted":false}),
            json!({"idleRoamingEnabled":true,"idleRoamRange":0,"muted":false}),
            json!({"idleRoamRange":null,"muted":false}),
        ] {
            assert!(fixture.core.action("update_settings", &patch).is_err());
            assert_eq!(get_setting(&fixture.core.db, "preferences").unwrap(), baseline);
        }
        let pulse = super::super::pet::pulse_at(&fixture.core, None, fixture.at).unwrap();
        assert_eq!(pulse["settings"]["idleRoamingEnabled"], false);
        assert_eq!(pulse["settings"]["idleRoamRange"], "nearby");
        fixture.core.action("save_settings", &json!({"settings":{"idleRoamingEnabled":true,"idleRoamRange":"workArea"}})).unwrap();
        assert_eq!(fixture.core.settings().unwrap()["idleRoamingEnabled"], true);
        assert_eq!(fixture.core.settings().unwrap()["idleRoamRange"], "workArea");
    }

    #[test]
    fn motion_cached_ui_check_needs_worker_verdict_and_expires_at_500ms_without_renewal() {
        let mut fixture = Fixture::new();
        fixture.ready();
        assert!(!fixture.core.desktop_motion_cached_idle_at(fixture.at));
        assert_eq!(fixture.core.desktop_motion_cache.root_queries, 0);
        assert!(fixture.idle());
        assert!(fixture.core.desktop_motion_cached_idle_at(fixture.at + Duration::milliseconds(499)));
        assert_eq!(fixture.core.desktop_motion_cache.root_queries, 1);
        assert!(!fixture.core.desktop_motion_cached_idle_at(fixture.at + Duration::milliseconds(500)));
        assert!(fixture.core.desktop_motion_cache.entry.is_none());
        assert_eq!(fixture.core.desktop_motion_cache.root_queries, 1);
    }

    #[test]
    fn motion_cached_ui_check_cannot_refresh_after_mutation_rollback_or_collector_expiry() {
        let mut fixture = Fixture::new();
        fixture.ready();
        assert!(fixture.idle());
        put_setting(&fixture.core.db, "motion-cache-mutation", &json!(true)).unwrap();
        assert!(!fixture.core.desktop_motion_cached_idle_at(fixture.at));
        assert!(fixture.idle());
        assert!(!fixture.core.desktop_motion_cached_idle_at(fixture.at - Duration::milliseconds(1)));
        fixture.at += Duration::milliseconds(MAX_OBSERVATION_AGE_MS - 1);
        assert!(fixture.idle());
        assert!(!fixture.core.desktop_motion_cached_idle_at(fixture.at + Duration::milliseconds(2)));
    }

    #[test]
    fn motion_cached_ui_check_uses_bounded_worker_path_observation_without_filesystem_access() {
        let mut fixture = Fixture::new();
        fixture.ready();
        assert!(fixture.idle());
        fs::remove_dir(&fixture.core.source_root).unwrap();
        // Deliberately retain the worker's bounded observation on the UI path;
        // the next worker refresh closes it without waiting for the 500ms cap.
        assert!(fixture.core.desktop_motion_cached_idle_at(fixture.at));
        assert!(!fixture.idle());
        assert!(!fixture.core.desktop_motion_cached_idle_at(fixture.at));
        assert_eq!(fixture.core.desktop_motion_cache.root_queries, 1);
    }

    #[test]
    fn motion_cached_ui_clock_high_watermark_closes_rollback_without_renewing_worker_age() {
        let mut fixture = Fixture::new();
        fixture.ready();
        assert!(fixture.idle());
        assert!(fixture.core.desktop_motion_cached_idle_at(fixture.at + Duration::milliseconds(400)));
        assert!(!fixture.core.desktop_motion_cached_idle_at(fixture.at + Duration::milliseconds(399)));
        assert!(!fixture.core.desktop_motion_idle_at(fixture.at + Duration::milliseconds(399)).unwrap());
        assert!(fixture.core.desktop_motion_idle_at(fixture.at + Duration::milliseconds(400)).unwrap());
        assert!(fixture.core.desktop_motion_cached_idle_at(fixture.at + Duration::milliseconds(899)));
        assert!(!fixture.core.desktop_motion_cached_idle_at(fixture.at + Duration::milliseconds(900)));
        assert_eq!(fixture.core.desktop_motion_cache.root_queries, 2);
    }
}
