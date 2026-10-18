//! Finite pet-card contracts: owned temporary JSONL/SQLite only, no shell or bridge.
use super::*;
use std::{cell::Cell, io::{BufRead, BufReader}};

const FIRST: &str = "00000000-0000-4000-8000-000000000081";
const SECOND: &str = "00000000-0000-4000-8000-000000000082";
const CHILD: &str = "00000000-0000-4000-8000-000000000083";

struct Fixture {
    temp: tempfile::TempDir,
    source: PathBuf,
    core: Core,
    at: DateTime<Utc>,
    next_file: Cell<u64>,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let core = Core::open(&temp.path().join("pet"), &source).unwrap();
        // Keep fixture records away from midnight and sub-millisecond clipping;
        // the day still matches the public wrappers' current Shanghai day.
        let at = parsed(&format!("{}T12:00:00+08:00", day_at(Utc::now()))).unwrap();
        Self { temp, source, core, at, next_file: Cell::new(0) }
    }

    fn ingest(&self, source: &str, values: &[Value]) {
        let number = self.next_file.get();
        self.next_file.set(number + 1);
        let file = self.temp.path().join(format!("fixture-{number}.jsonl"));
        let text = values.iter().map(|value| value.to_string() + "\n").collect::<String>();
        fs::write(&file, text).unwrap();
        let mut context = reader::Context::default();
        for line in BufReader::new(fs::File::open(&file).unwrap()).lines() {
            let value: Value = serde_json::from_str(&line.unwrap()).unwrap();
            adapter::ingest(&self.core.db, source, file.to_str().unwrap(), &value, &mut context, None)
                .unwrap();
        }
    }

    fn timed(&self, source: &str, thread: &str, turn: &str, at: DateTime<Utc>, ttft: Value) {
        self.ingest(source, &[
            event(at, "session_meta", json!({"id":thread,"cwd":"D:/fixture/project"})),
            event(at, "event_msg", json!({"type":"task_started","turn_id":turn,"root_turn_id":turn})),
            event(at, "event_msg", json!({"type":"task_complete","turn_id":turn,"root_turn_id":turn,
                "status":"completed","duration_ms":1000,"time_to_first_token_ms":ttft})),
        ]);
    }

    fn domain(&self) -> Value {
        let pulse = pulse_at(&self.core, None, self.at).unwrap();
        json!({"day":pulse["day"],"runtimeEpoch":pulse["runtimeEpoch"],"sourceScopeRevision":pulse["sourceScopeRevision"]})
    }

    fn details(&self) -> Value {
        details_at(&self.core, &self.domain(), self.at).unwrap()
    }

    fn second_source(&mut self) -> String {
        let root = self.temp.path().join("second-source");
        fs::create_dir(&root).unwrap();
        self.core.add_source(&root).unwrap()
    }

    fn populated() -> Self {
        let fixture = Self::new();
        fixture.timed(&fixture.core.source_id, FIRST, "first-turn", fixture.at, json!(125));
        fixture
    }
}

fn event(at: DateTime<Utc>, kind: &str, payload: Value) -> Value {
    json!({"timestamp":at.to_rfc3339_opts(chrono::SecondsFormat::Millis,true),"type":kind,"payload":payload})
}

fn action(domain: &Value, source: &str, thread: &str) -> Value {
    json!({"petDomain":domain,"chat":{"sourceId":source,"threadId":thread}})
}

fn assert_open(result: Result<PetChatTarget>, expected: &str) {
    match result.unwrap() {
        PetChatTarget::Open { target, thread_id } => {
            assert_eq!(thread_id, expected);
            assert_eq!(target.as_str(), format!("codex://threads/{expected}"));
        }
        PetChatTarget::CopyRequired { .. } => panic!("verified plain UUID should remain openable"),
    }
}

fn total_changes(core: &Core) -> i64 {
    core.db.query_row("SELECT total_changes()", [], |row| row.get(0)).unwrap()
}

#[test]
fn pet_card_current_source_chat_preserves_all_source_ttft_and_native_thread() {
    let mut fixture = Fixture::new();
    let other = fixture.second_source();
    let primary = fixture.core.source_id.clone();
    fixture.timed(&primary, FIRST, "turn-is-not-a-thread", fixture.at - Duration::seconds(2), json!(125));
    fixture.timed(&other, SECOND, "other-turn", fixture.at, Value::Null);
    let details = fixture.details();
    assert_eq!(details["latestTurn"], json!({"timeToFirstTokenMs":null}),
        "the globally latest missing TTFT cannot borrow the primary source's value");
    assert_eq!(details["recentChat"], json!({"sourceId":primary,"threadId":FIRST}));
    assert!(details["overview"]["totalTokens"].is_null(), "navigation does not need a usage response");
    assert_open(chat_target_at(&fixture.core, &action(&fixture.domain(), &primary, FIRST), fixture.at), FIRST);
}

#[test]
fn pet_card_newest_incomplete_identity_never_borrows_an_older_chat() {
    let fixture = Fixture::populated();
    let primary = fixture.core.source_id.clone();
    fixture.timed(&primary, SECOND, "newest", fixture.at + Duration::milliseconds(1), json!(250));
    let key = format!("{primary}|newest");
    for thread in [Value::Null, json!(17), json!(false), json!({"id":SECOND}), json!("")] {
        fixture.core.db.execute("UPDATE turns SET data=json_set(data,'$.threadId',json(?1)) WHERE id=?2",
            params![thread.to_string(), key]).unwrap();
        assert!(fixture.details()["recentChat"].is_null(), "bad newest identity must not select FIRST");
    }
    fixture.core.db.execute("UPDATE turns SET data=json_set(data,'$.threadId',?1) WHERE id=?2",
        params![SECOND, key]).unwrap();
    fixture.core.db.execute("DELETE FROM thread_metadata WHERE id=?1", [format!("{primary}|{SECOND}")]).unwrap();
    assert!(fixture.details()["recentChat"].is_null(), "missing newest member must not select FIRST");
}

#[test]
fn pet_card_child_and_equal_time_selection_keep_original_thread_and_stable_ttft() {
    let fixture = Fixture::new();
    let primary = &fixture.core.source_id;
    fixture.timed(primary, FIRST, "first", fixture.at, Value::Null);
    fixture.ingest(primary, &[
        event(fixture.at, "session_meta", json!({"id":CHILD,"parent_thread_id":FIRST})),
        event(fixture.at, "event_msg", json!({"type":"task_complete","turn_id":"child-turn",
            "root_turn_id":"root-turn-not-thread","status":"completed","time_to_first_token_ms":900})),
    ]);
    let tied = fixture.details();
    assert_eq!(tied["latestTurn"], json!({"timeToFirstTokenMs":null}));
    assert_eq!(tied["recentChat"]["threadId"], FIRST, "an exact tie keeps the earlier physical row");
    let later = (fixture.at + Duration::milliseconds(1)).to_rfc3339_opts(chrono::SecondsFormat::Millis,true);
    fixture.core.db.execute("UPDATE turns SET data=json_set(data,'$.completedAt',?1,'$.lastEventAt',?1) WHERE turn_id='child-turn'", [later]).unwrap();
    let details = fixture.details();
    assert_eq!(details["latestTurn"], json!({"timeToFirstTokenMs":900}));
    assert_eq!(details["recentChat"]["threadId"], CHILD, "never substitute the parent or root turn ID");
    assert_open(chat_target_at(&fixture.core, &action(&fixture.domain(), primary, CHILD), fixture.at), CHILD);
}

#[test]
fn pet_card_today_selection_and_action_reject_yesterday_and_metadata_only() {
    let fixture = Fixture::new();
    let primary = &fixture.core.source_id;
    fixture.timed(primary, FIRST, "yesterday", fixture.at - Duration::days(1), json!(50));
    fixture.ingest(primary, &[event(fixture.at, "session_meta", json!({"id":SECOND}))]);
    let details = fixture.details();
    assert!(details["latestTurn"].is_null());
    assert!(details["recentChat"].is_null());
    for thread in [FIRST, SECOND] {
        assert!(chat_target_at(&fixture.core, &action(&fixture.domain(), primary, thread), fixture.at).is_err());
        assert!(fixture.core.chat_target(primary, thread).is_ok(), "public explicit navigation retains its existing wider contract");
    }
}

#[test]
fn pet_card_new_turn_does_not_replace_the_displayed_verified_chat_action() {
    let fixture = Fixture::populated();
    let primary = &fixture.core.source_id;
    let shown = fixture.details();
    let domain = fixture.domain();
    let request = json!({"petDomain":domain,"chat":shown["recentChat"]});
    fixture.timed(primary, SECOND, "later", fixture.at + Duration::milliseconds(1), json!(250));
    assert_eq!(fixture.details()["recentChat"]["threadId"], SECOND);
    assert_open(chat_target_at(&fixture.core, &request, fixture.at), FIRST);
}

#[test]
fn pet_card_domain_and_target_reject_source_changes_and_cleanup_boundaries() {
    for change in ["primary", "root", "disabled", "clear_data", "clear_inbox"] {
        let mut fixture = Fixture::populated();
        let other = fixture.second_source();
        let primary = fixture.core.source_id.clone();
        let domain = fixture.domain();
        let request = action(&domain, &primary, FIRST);
        match change {
            "primary" => { fixture.core.action("set_primary_source", &json!({"id":other})).unwrap(); }
            "root" => {
                let replacement = fixture.temp.path().join("replacement-source");
                fs::create_dir(&replacement).unwrap();
                fixture.core.action("update_source", &json!({"id":primary,"path":replacement.to_string_lossy()})).unwrap();
            }
            "disabled" => { fixture.core.action("update_source", &json!({"id":primary,"enabled":false})).unwrap(); }
            name => { fixture.core.action(name, &json!({})).unwrap(); }
        }
        assert!(validate_domain_at(&fixture.core, &domain, fixture.at).is_err(), "{change}");
        assert!(chat_target_at(&fixture.core, &request, fixture.at).is_err(), "{change}");
        if change == "clear_data" {
            let fresh = fixture.domain();
            assert!(chat_target_at(&fixture.core, &action(&fresh, &primary, FIRST), fixture.at).is_err(),
                "a fresh domain cannot revive a deleted member");
        }
    }
}

#[test]
fn pet_card_strict_domain_and_request_shapes_reject_midnight_and_injection() {
    let fixture = Fixture::populated();
    let primary = &fixture.core.source_id;
    let domain = fixture.domain();
    let valid = action(&domain, primary, FIRST);
    assert!(validate_domain_at(&fixture.core, &domain, fixture.at + Duration::days(1)).is_err());
    assert!(chat_target_at(&fixture.core, &valid, fixture.at + Duration::days(1)).is_err());
    for invalid in [Value::Null, json!([]), json!({}), json!({"day":false}),
        json!({"day":"2026-02-30","runtimeEpoch":"x","sourceScopeRevision":"x"})] {
        assert!(validate_domain_at(&fixture.core, &invalid, fixture.at).is_err());
    }
    for (key, value) in [("runtimeEpoch", json!("old-runtime")), ("day", json!("")),
        ("sourceScopeRevision", Value::Null), ("query", json!({"sourceId":"injected"}))] {
        let mut invalid = domain.clone();
        invalid[key] = value;
        assert!(validate_domain_at(&fixture.core, &invalid, fixture.at).is_err());
    }
    for (path, value) in [("extra", json!(true)), ("chat", json!(null)), ("petDomain", json!({}))] {
        let mut invalid = valid.clone();
        invalid[path] = value;
        assert!(chat_target_at(&fixture.core, &invalid, fixture.at).is_err());
    }
    for (key, value) in [("threadId", json!(false)), ("sourceId", json!(17)),
        ("threadId", json!("")), ("uri", json!("https://example.invalid"))] {
        let mut invalid = valid.clone();
        invalid["chat"][key] = value;
        assert!(chat_target_at(&fixture.core, &invalid, fixture.at).is_err());
    }
}

#[test]
fn pet_card_copy_permission_requires_verified_membership_and_known_protocol_failure() {
    let mut fixture = Fixture::populated();
    let primary = fixture.core.source_id.clone();
    let other = fixture.second_source();
    fixture.timed(&other, FIRST, "other-domain", fixture.at, json!(50));
    let domain = fixture.domain();
    match chat_target_at(&fixture.core, &action(&domain, &primary, FIRST), fixture.at).unwrap() {
        PetChatTarget::CopyRequired { thread_id, message } => {
            assert_eq!(thread_id, FIRST);
            assert!(message.contains("多个来源"));
        }
        PetChatTarget::Open { .. } => panic!("ambiguous native URI must not launch"),
    }
    let non_uuid = "known-local-thread";
    fixture.timed(&primary, non_uuid, "legacy", fixture.at, json!(50));
    match chat_target_at(&fixture.core, &action(&domain, &primary, non_uuid), fixture.at).unwrap() {
        PetChatTarget::CopyRequired { thread_id, message } => {
            assert_eq!(thread_id, non_uuid);
            assert!(message.contains("适配器校验"));
        }
        PetChatTarget::Open { .. } => panic!("non-UUID member must only offer its exact ID"),
    }
    assert!(chat_target_at(&fixture.core, &action(&domain, &primary, "https://example.invalid"), fixture.at).is_err());
    assert!(chat_target_at(&fixture.core, &action(&domain, &other, FIRST), fixture.at).is_err());
}

#[test]
fn pet_card_nontext_or_mismatched_metadata_never_verifies_an_identity() {
    for replacement in [json!({"sourceId":"wrong","id":FIRST}),
        json!({"sourceId":true,"id":FIRST}), json!({"id":FIRST}),
        json!({"sourceId":"placeholder","id":17}), json!({"sourceId":"placeholder","id":null})] {
        let fixture = Fixture::populated();
        let primary = &fixture.core.source_id;
        let domain = fixture.domain();
        let mut metadata = replacement;
        if metadata["sourceId"] == "placeholder" { metadata["sourceId"] = json!(primary); }
        fixture.core.db.execute("UPDATE thread_metadata SET data=?1 WHERE id=?2",
            params![metadata.to_string(), format!("{primary}|{FIRST}")]).unwrap();
        assert!(fixture.details()["recentChat"].is_null());
        assert!(chat_target_at(&fixture.core, &action(&domain, primary, FIRST), fixture.at).is_err());
    }
    // SQLite json_extract returns objects as TEXT too: require JSON type=text,
    // rather than confusing serialized object bytes with a native string ID.
    let fixture = Fixture::new();
    let primary = &fixture.core.source_id;
    let serialized = "{\"v\":1}";
    fixture.timed(primary, serialized, "object-id", fixture.at, Value::Null);
    fixture.core.db.execute("UPDATE thread_metadata SET data=json_set(data,'$.id',json('{\"v\":1}')) WHERE id=?1",
        [format!("{primary}|{serialized}")]).unwrap();
    assert!(fixture.details()["recentChat"].is_null());
    assert!(chat_target_at(&fixture.core, &action(&fixture.domain(), primary, serialized), fixture.at).is_err());
}

#[test]
fn pet_card_database_and_malformed_json_fail_without_copy_permission() {
    for corrupt in ["member", "unrelated_member", "turn", "check", "drop_metadata", "drop_turns"] {
        let fixture = Fixture::populated();
        let primary = &fixture.core.source_id;
        let domain = fixture.domain();
        let request = action(&domain, primary, FIRST);
        match corrupt {
            "member" => { fixture.core.db.execute("UPDATE thread_metadata SET data='{' WHERE id=?1", [format!("{primary}|{FIRST}")]).unwrap(); }
            "unrelated_member" => { fixture.core.db.execute("INSERT INTO thread_metadata(id,data) VALUES('corrupt-other','{')", []).unwrap(); }
            "turn" => { fixture.core.db.execute("UPDATE turns SET data='{'", []).unwrap(); }
            "check" => { fixture.core.db.execute("INSERT OR REPLACE INTO settings(key,value) VALUES(?1,'{')", [format!("lastSourceCheck:{primary}")]).unwrap(); }
            "drop_metadata" => fixture.core.db.execute_batch("DROP TABLE thread_metadata;").unwrap(),
            "drop_turns" => fixture.core.db.execute_batch("DROP TABLE turns;").unwrap(),
            _ => unreachable!(),
        }
        assert!(chat_target_at(&fixture.core, &request, fixture.at).is_err(),
            "{corrupt} cannot be downgraded into CopyRequired");
        if corrupt != "unrelated_member" {
            assert!(details_at(&fixture.core, &domain, fixture.at).is_err(), "{corrupt}");
        }
    }
}

#[test]
fn pet_card_domain_and_target_reads_need_no_details_tables_or_read_revision() {
    let fixture = Fixture::populated();
    let primary = &fixture.core.source_id;
    let domain = fixture.domain();
    let request = action(&domain, primary, FIRST);
    fixture.core.db.execute_batch("DROP TABLE usage_responses; DROP TABLE items; DROP TABLE legacy_usage_segments;").unwrap();
    let changes = total_changes(&fixture.core);
    let revision = fixture.core.pet_read_revision.get();
    fixture.core.pet_validate_domain(&domain).unwrap();
    assert_open(fixture.core.pet_chat_target(&request), FIRST);
    assert!(details_at(&fixture.core, &domain, fixture.at).is_err(), "Details really is unavailable without usage_responses");
    assert_eq!(total_changes(&fixture.core), changes);
    assert_eq!(fixture.core.pet_read_revision.get(), revision);
    let mut invalid = request;
    invalid["petDomain"]["runtimeEpoch"] = json!("expired");
    assert!(fixture.core.pet_chat_target(&invalid).is_err());
    assert_eq!(total_changes(&fixture.core), changes);
    assert_eq!(fixture.core.pet_read_revision.get(), revision);
}

#[test]
fn pet_card_source_enablement_is_required_but_collector_health_is_independent() {
    let fixture = Fixture::populated();
    let primary = &fixture.core.source_id;
    // No collector scan/check is fabricated: a logged member can still be used
    // while collection health is unknown or its empty directory is unavailable.
    fs::remove_dir(&fixture.source).unwrap();
    let domain = fixture.domain();
    assert_eq!(fixture.details()["recentChat"]["threadId"], FIRST);
    assert_open(chat_target_at(&fixture.core, &action(&domain, primary, FIRST), fixture.at), FIRST);
    for (column, value) in [("enabled", "0"), ("client_kind", "'unreleased'")] {
        fixture.core.db.execute(&format!("UPDATE source_instances SET {column}={value} WHERE id=?1"), [primary]).unwrap();
        let current = fixture.domain();
        assert!(fixture.details()["recentChat"].is_null());
        assert!(chat_target_at(&fixture.core, &action(&current, primary, FIRST), fixture.at).is_err());
        fixture.core.db.execute("UPDATE source_instances SET enabled=1,client_kind='codex' WHERE id=?1", [primary]).unwrap();
    }
}

#[test]
fn pet_card_shared_chat_validation_preserves_public_error_contract() {
    let fixture = Fixture::populated();
    let primary = &fixture.core.source_id;
    assert_eq!(fixture.core.chat_target("missing-source", FIRST).err().unwrap(), "未知来源实例");
    assert_eq!(fixture.core.chat_target(primary, SECOND).err().unwrap(), "此会话未在指定来源中验证，请复制会话标识");
    let non_uuid = "known-plain-thread";
    fixture.timed(primary, non_uuid, "known-invalid-protocol", fixture.at, Value::Null);
    assert_eq!(fixture.core.chat_target(primary, non_uuid).err().unwrap(), "会话身份或跳转协议未通过适配器校验");
    fixture.core.db.execute("UPDATE source_instances SET client_kind='unreleased' WHERE id=?1", [primary]).unwrap();
    assert_eq!(fixture.core.chat_target(primary, FIRST).err().unwrap(), "此来源尚未发布聊天跳转适配器");
}
