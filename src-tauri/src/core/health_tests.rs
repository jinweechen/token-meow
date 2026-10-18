//! Temporary-source regressions for health knowledge and history completeness.
use super::*;

struct Fixture {
    temp: tempfile::TempDir,
    core: Core,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let core = Core::open(&temp.path().join("pet"), &source).unwrap();
        Self { temp, core }
    }

    fn snapshot(&self) -> Value {
        self.core.snapshot(json!({"from":"1900-01-01","to":"1900-01-01","model":"not-present"})).unwrap()
    }

    fn add_source(&mut self) -> String {
        let path = self.temp.path().join("second-source");
        fs::create_dir(&path).unwrap();
        self.core.add_source(&path).unwrap()
    }

    fn check(&self, id: &str) -> Value {
        get_setting(&self.core.db, &format!("lastSourceCheck:{id}")).unwrap().unwrap()
    }

    fn set_check(&self, id: &str, check: &Value) {
        put_setting(&self.core.db, &format!("lastSourceCheck:{id}"), check).unwrap();
    }

    fn complete_source(&self, id: &str, pending: u64) {
        self.set_check(id, &json!({"checkedAt":now(),"pendingFiles":pending,"issues":0,
            "files":0,"unsupportedFiles":0,"discoveryComplete":true,"discoveryErrors":0,"cancelled":false}));
        put_setting(&self.core.db, &format!("historyPending:{id}"), &json!(pending)).unwrap();
    }
}

fn source<'a>(snapshot: &'a Value, id: &str) -> &'a Value {
    snapshot["sources"].as_array().unwrap().iter().find(|row| row["id"] == id).unwrap()
}

#[test]
fn fresh_source_is_unknown_until_a_completed_check() {
    let mut fixture = Fixture::new();
    let first = fixture.snapshot();
    let primary = source(&first, &fixture.core.source_id);
    assert!(primary["health"]["checkedAt"].is_null());
    assert_eq!(primary["runtime"], "未知", "directory existence cannot establish a successful collector check");
    fixture.core.scan().unwrap();
    let checked = fixture.snapshot();
    assert_eq!(source(&checked, &fixture.core.source_id)["runtime"], "可用");
    assert_eq!(checked["health"]["history"]["status"], "idle");
    assert_eq!(checked["health"]["history"]["pendingFiles"], 0);
}

#[test]
fn mixed_known_and_unchecked_history_never_claims_global_idle() {
    let mut fixture = Fixture::new();
    fixture.core.scan().unwrap();
    let second = fixture.add_source();
    let snapshot = fixture.snapshot();
    assert!(snapshot["health"]["history"]["pendingFiles"].is_null(), "one zero-pending source does not complete an unexamined second source");
    assert_ne!(snapshot["health"]["history"]["status"], "idle");
    assert_eq!(snapshot["health"]["history"]["knownPendingFiles"], 0);
    assert_eq!(source(&snapshot, &second)["runtime"], "未知");
    assert!(source(&snapshot, &second)["health"]["lastSuccessfulCheck"].is_null());
}

#[test]
fn partial_discovery_retains_history_watermark_but_cannot_publish_completion() {
    let mut fixture = Fixture::new();
    fixture.core.scan().unwrap();
    let id = fixture.core.source_id.clone();
    let mut check = fixture.check(&id);
    check["discoveryComplete"] = json!(false);
    check["discoveryErrors"] = json!(1);
    check["issues"] = json!(1);
    fixture.set_check(&id, &check);
    let snapshot = fixture.snapshot();
    assert!(snapshot["health"]["history"]["pendingFiles"].is_null());
    assert_ne!(snapshot["health"]["history"]["status"], "idle");
    assert_eq!(source(&snapshot, &id)["runtime"], "不可用");
    assert_eq!(get_setting(&fixture.core.db, &format!("historyPending:{id}")).unwrap(), Some(json!(0)), "projection must retain the collector's earlier watermark");
    fixture.complete_source(&id, 0);
    let complete = fixture.check(&id);
    for (field, value) in [("discoveryComplete", json!(false)), ("discoveryErrors", json!(1)), ("cancelled", json!(true))] {
        let mut check = complete.clone();
        check[field] = value;
        fixture.set_check(&id, &check);
        let snapshot = fixture.snapshot();
        assert_eq!(source(&snapshot, &id)["runtime"], "不可用", "known unsuccessful {field}");
        assert!(snapshot["health"]["history"]["pendingFiles"].is_null(), "incomplete {field}");
        assert_ne!(snapshot["health"]["history"]["status"], "idle");
    }
}

#[test]
fn history_totals_require_every_enabled_source_and_ignore_disabled_unknown_sources() {
    let mut fixture = Fixture::new();
    fixture.core.scan().unwrap();
    let second = fixture.add_source();
    put_setting(&fixture.core.db, &format!("historyPending:{second}"), &json!(3)).unwrap();
    assert!(fixture.snapshot()["health"]["history"]["pendingFiles"].is_null());
    fixture.core.action("update_source", &json!({"id":second,"enabled":false})).unwrap();
    let disabled = fixture.snapshot();
    assert_eq!(source(&disabled, &second)["runtime"], "未配置");
    assert_eq!(disabled["health"]["history"]["pendingFiles"], 0);
    assert_eq!(disabled["health"]["history"]["status"], "idle");
    fixture.core.action("update_source", &json!({"id":second,"enabled":true})).unwrap();
    fixture.complete_source(&second, 3);
    let pending = fixture.snapshot();
    assert_eq!(pending["health"]["history"]["pendingFiles"], 3);
    assert_eq!(pending["health"]["history"]["status"], "running");
    fixture.complete_source(&second, 0);
    let complete = fixture.snapshot();
    assert_eq!(complete["health"]["history"]["pendingFiles"], 0);
    assert_eq!(complete["health"]["history"]["status"], "idle");
}

#[test]
fn paused_cancelled_history_preserves_user_states_during_unknown_collection() {
    let mut fixture = Fixture::new();
    fixture.core.action("pause_history", &json!({})).unwrap();
    let paused = fixture.snapshot();
    assert_eq!(paused["health"]["history"]["status"], "paused");
    assert!(paused["health"]["history"]["pendingFiles"].is_null());
    fixture.core.action("cancel_history", &json!({})).unwrap();
    let cancelled = fixture.snapshot();
    assert_eq!(cancelled["health"]["history"]["status"], "cancelled");
    assert!(cancelled["health"]["history"]["pendingFiles"].is_null());
    assert!(fixture.core.action("resume_history", &json!({})).is_err());
}

#[test]
fn malformed_collector_counts_and_timestamps_never_open_source_health() {
    let mut fixture = Fixture::new();
    fixture.core.scan().unwrap();
    let id = fixture.core.source_id.clone();
    let complete = fixture.check(&id);
    for (field, value) in [
        ("issues", Value::Null), ("issues", json!("0")), ("issues", json!(-1)),
        ("checkedAt", Value::Null), ("checkedAt", json!("not-a-time")),
        ("pendingFiles", Value::Null), ("files", Value::Null),
        ("discoveryComplete", json!("true")), ("discoveryErrors", json!("0")),
        ("cancelled", Value::Null),
    ] {
        let mut check = complete.clone();
        check[field] = value;
        fixture.set_check(&id, &check);
        let snapshot = fixture.snapshot();
        assert_eq!(source(&snapshot, &id)["runtime"], "未知", "malformed {field}");
        assert!(snapshot["health"]["history"]["pendingFiles"].is_null(), "malformed {field}");
        assert_ne!(snapshot["health"]["history"]["status"], "idle", "malformed {field}");
    }
    for check in [Value::Null, json!(true), json!(0), json!("invalid-check"), json!([])] {
        fixture.set_check(&id, &check);
        let snapshot = fixture.snapshot();
        assert_eq!(source(&snapshot, &id)["runtime"], "未知");
        assert!(snapshot["health"]["history"]["pendingFiles"].is_null());
    }
}

#[test]
fn known_parse_issues_do_not_turn_completed_history_into_running_work() {
    let mut fixture = Fixture::new();
    fixture.core.scan().unwrap();
    let id = fixture.core.source_id.clone();
    let mut check = fixture.check(&id);
    check["issues"] = json!(1);
    fixture.set_check(&id, &check);
    let snapshot = fixture.snapshot();
    assert_eq!(source(&snapshot, &id)["runtime"], "不可用");
    assert_eq!(snapshot["health"]["history"]["pendingFiles"], 0);
    assert_eq!(snapshot["health"]["history"]["status"], "idle", "completed cursor work and data-quality issues are independent");
}

#[test]
fn a_source_directory_replaced_by_a_file_is_unavailable_even_with_old_success() {
    let mut fixture = Fixture::new();
    fixture.core.scan().unwrap();
    let root = fixture.core.source_root.clone();
    fs::remove_dir(&root).unwrap();
    fs::write(&root, b"temporary fixture file, not a source directory").unwrap();
    let snapshot = fixture.snapshot();
    assert_eq!(source(&snapshot, &fixture.core.source_id)["runtime"], "不可用");
    assert!(snapshot["health"]["history"]["pendingFiles"].is_null());
    assert_ne!(snapshot["health"]["history"]["status"], "idle");
}

#[test]
fn pet_scope_tracks_health_transitions_and_stays_stable_for_new_tokens() {
    let mut fixture = Fixture::new();
    let at = chrono::DateTime::parse_from_rfc3339("2026-10-17T02:00:00Z").unwrap().with_timezone(&Utc);
    let pulse = |core: &Core, previous: Option<&Value>| {
        crate::core::pet::pulse_at(core, previous.map(|v| &v["promptFeed"]["cursor"]), at).unwrap()
    };
    let unknown = pulse(&fixture.core, None);
    fixture.core.scan().unwrap();
    let checked = pulse(&fixture.core, Some(&unknown));
    assert_ne!(checked["sourceScopeRevision"], unknown["sourceScopeRevision"]);
    assert_eq!(checked["promptFeed"]["baseline"], true);

    put_setting(&fixture.core.db, "importSince", &json!("1970-01-01T00:00:00Z")).unwrap();
    let token = json!({"timestamp":now(),"type":"token_usage_record","payload":{
        "thread_id":"88000000-0000-4000-8000-000000000001","response_id":"health-scope-token",
        "usage":{"input_tokens":100,"output_tokens":20,"cached_input_tokens":0,
            "cache_write_input_tokens":0,"reasoning_output_tokens":0,"total_tokens":120}}});
    fs::write(fixture.core.source_root.join("token.jsonl"), token.to_string() + "\n").unwrap();
    fixture.core.scan().unwrap();
    let count: i64 = fixture.core.db.query_row("SELECT COUNT(*) FROM usage_responses", [], |row| row.get(0)).unwrap();
    assert_eq!(count, 1, "the stability check must observe a real temporary token record");
    let with_token = pulse(&fixture.core, Some(&checked));
    assert_eq!(with_token["sourceScopeRevision"], checked["sourceScopeRevision"]);
    assert_eq!(with_token["promptFeed"]["baseline"], false);

    let id = fixture.core.source_id.clone();
    let valid = fixture.check(&id);
    let mut malformed = valid.clone();
    malformed["issues"] = Value::Null;
    fixture.set_check(&id, &malformed);
    let invalid = pulse(&fixture.core, Some(&with_token));
    assert_eq!(source(&fixture.snapshot(), &id)["runtime"], "未知");
    assert_ne!(invalid["sourceScopeRevision"], with_token["sourceScopeRevision"]);
    assert_eq!(invalid["sourceScopeRevision"], unknown["sourceScopeRevision"]);
    assert_eq!(invalid["promptFeed"]["baseline"], true);
    fixture.set_check(&id, &valid);
    assert_eq!(pulse(&fixture.core, Some(&invalid))["sourceScopeRevision"], checked["sourceScopeRevision"]);
}
