use super::*;
use crate::core::adapter::{CodexAdapter, Metric, Observation, SourceAdapter, StandardEvent};

struct Rig {
    _tmp: tempfile::TempDir,
    source: PathBuf,
    data: PathBuf,
    core: Core,
}

impl Rig {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        let data = tmp.path().join("pet");
        fs::create_dir_all(&source).unwrap();
        let core = Core::open(&data, &source).unwrap();
        Self {
            _tmp: tmp,
            source,
            data,
            core,
        }
    }
}

fn at() -> DateTime<Utc> {
    parsed("2026-10-16T02:00:00.000Z").unwrap()
}

fn iso(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn cursor(pulse: &Value) -> &Value {
    &pulse["promptFeed"]["cursor"]
}

fn request(pulse: &Value) -> Value {
    json!({"day":pulse["day"],"runtimeEpoch":pulse["runtimeEpoch"],"sourceScopeRevision":pulse["sourceScopeRevision"]})
}

fn inbox(core: &Core, id: &str, time: &str, status: &str, kind: &str) {
    let v = json!({"id":id,"sourceId":core.source_id,"threadId":"thread","turnId":id,
        "kind":kind,"status":status,"createdAt":time,"read":false,
        "eligibleRuntime":core.runtime_id,"notifyEligible":true,"animationAcknowledged":false,
        "notified":false,"notificationState":"pending"});
    core.db.execute("INSERT INTO inbox_events(id,thread_id,turn_id,event_time,data) VALUES(?1,'thread',?1,?2,?3) ON CONFLICT(id) DO UPDATE SET data=excluded.data",params![id,time,v.to_string()]).unwrap();
}

fn turn(core: &Core, id: &str, time: &str, status: &str, root: Option<&str>, ttft: Value) {
    let v = json!({"id":id,"sourceId":core.source_id,"threadId":"thread","rootTurnId":root,
        "status":status,"startedAt":time,"completedAt":if status=="active"{Value::Null}else{json!(time)},
        "lastEventAt":time,"timeToFirstTokenMs":ttft});
    core.db
        .execute(
            "INSERT INTO turns(id,thread_id,turn_id,event_time,data) VALUES(?1,'thread',?2,?3,?4)",
            params![format!("{}|{id}", core.source_id), id, time, v.to_string()],
        )
        .unwrap();
}

fn native(core: &Core, id: &str, time: DateTime<Utc>, input: i64, cached: Option<i64>) -> String {
    native_with_output(core, id, time, input, 20, cached)
}

fn native_with_output(
    core: &Core,
    id: &str,
    time: DateTime<Utc>,
    input: i64,
    output: i64,
    cached: Option<i64>,
) -> String {
    let event = json!({"timestamp":iso(time),"type":"token_usage_record","payload":{
        "thread_id":"thread","response_id":id,"turn_id":id,"root_turn_id":id,
        "usage":{"input_tokens":input,"output_tokens":output,"cached_input_tokens":cached,
        "cache_write_input_tokens":0,"reasoning_output_tokens":0,"total_tokens":input+output}}});
    let observation = Observation {
        source_id: core.source_id.clone(),
        channel_id: format!("{}:jsonl", core.source_id),
        event,
    };
    let value = CodexAdapter.normalize(&observation).unwrap().remove(0);
    let StandardEvent::ResponseUsage(r) = &value else {
        panic!()
    };
    let key = crate::core::adapter::response_key(r);
    crate::core::adapter::store_standard_event(&core.db, &value, false).unwrap();
    key
}

fn changes(core: &Core) -> i64 {
    core.db
        .query_row("SELECT total_changes()", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn pet_pulse_matches_snapshot_and_never_reads_unused_fact_tables() {
    let r = Rig::new();
    let time = Utc::now();
    let day = day_at(time);
    turn(
        &r.core,
        "root",
        &iso(time - chrono::Duration::seconds(20)),
        "active",
        None,
        Value::Null,
    );
    turn(
        &r.core,
        "child",
        &iso(time),
        "active",
        Some("root"),
        Value::Null,
    );
    turn(
        &r.core,
        "stale",
        &iso(time - chrono::Duration::seconds(601)),
        "active",
        None,
        Value::Null,
    );
    inbox(&r.core, "today", &iso(time), "completed", "completion");
    inbox(
        &r.core,
        "yesterday",
        &iso(time - chrono::Duration::days(1)),
        "unknown",
        "completion",
    );
    for file in ["one", "two"] {
        issue(&r.core.db, "fixture", file, file, "same grouped issue").unwrap();
    }
    r.core.db.execute("INSERT INTO identity_conflicts VALUES('open','fixture',?1,'{}'),('resolved','fixture',?1,'{\"resolved\":true}')",[iso(time)]).unwrap();
    put_setting(&r.core.db, "lastSuccessfulScan", &json!(iso(time))).unwrap();
    native(&r.core, "usage", time, 100, Some(50));
    let full = r.core.snapshot(json!({"from":day,"to":day})).unwrap();
    let before = changes(&r.core);
    let pulse = pulse_at(&r.core, None, time).unwrap();
    assert_eq!(
        pulse["activity"],
        Value::Object(
            full["activity"]
                .as_object()
                .unwrap()
                .iter()
                .filter(|(k, _)| ["activeTurns", "uncertainTurns", "state"].contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<serde_json::Map<_, _>>()
        )
    );
    assert_eq!(
        pulse["unreadCount"],
        full["inbox"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| v["read"] != true)
            .count()
    );
    assert_eq!(
        pulse["health"]["issues"],
        full["health"]["issues"].as_array().unwrap().len()
    );
    assert_eq!(pulse["health"]["conflicts"], full["health"]["conflicts"]);
    assert_eq!(pulse["health"]["issues"], 1);
    assert_eq!(
        changes(&r.core),
        before,
        "reads do not mark delivery or mutate facts"
    );
    // The production endpoint must remain independent of these complete fact collections.
    r.core.db.execute_batch("DROP TABLE items; DROP TABLE thread_metadata; DROP TABLE usage_responses; DROP TABLE legacy_usage_segments;").unwrap();
    assert!(r.core.snapshot(json!({})).is_err());
    let next = pulse_at(&r.core, Some(cursor(&pulse)), time).unwrap();
    assert_eq!(next["activity"], pulse["activity"]);
    assert_eq!(next["unreadCount"], pulse["unreadCount"]);
    assert!(next["settings"].get("projectMappings").is_none());
    assert_eq!(next["settings"]["muted"], true);
}

#[test]
fn pet_feed_priority_overflow_and_every_observed_sequence_are_consumed_once() {
    let time = at();
    let _clock = crate::budget_clock::test_support::TestClock::new(crate::budget_clock::Reading {
        utc: time,
        mono_ms: 1_000,
    });
    let mut r = Rig::new();
    r.core.notification_gate().publish(true, false);
    inbox(&r.core, "initial", &iso(time), "completed", "completion");
    let baseline = pulse_at(&r.core, None, time).unwrap();
    assert_eq!(baseline["promptFeed"]["baseline"], true);
    assert_eq!(baseline["prompts"], json!([]));
    for i in 0..10 {
        inbox(
            &r.core,
            &format!("complete-{i:02}"),
            &iso(time - chrono::Duration::milliseconds(i)),
            "completed",
            "completion",
        );
    }
    for (id, status, kind) in [
        ("failed", "failed", "completion"),
        ("interrupted", "interrupted", "completion"),
        ("unknown", "unknown", "completion"),
        ("budget", "threshold", "budget"),
    ] {
        inbox(
            &r.core,
            id,
            &iso(time - chrono::Duration::seconds(9)),
            status,
            kind,
        );
    }
    // This ordering fixture supplies a host permit explicitly. Production
    // budget eligibility is exercised separately through real JSONL scans.
    r.core.grant_budget_test_permit("budget", 10_000);
    for (id, status, millis) in [
        ("normal", "Normal", 9500),
        ("ok", "OK", 9500),
        ("done", "done", 8000),
    ] {
        inbox(
            &r.core,
            id,
            &iso(time - chrono::Duration::milliseconds(millis)),
            status,
            "completion",
        );
    }
    inbox(
        &r.core,
        "numeric-notify",
        &iso(time),
        "failed",
        "completion",
    );
    r.core.db.execute("UPDATE inbox_events SET data=json_set(data,'$.notifyEligible',1) WHERE id='numeric-notify'",[]).unwrap();
    inbox(
        &r.core,
        "numeric-ack",
        &iso(time - chrono::Duration::milliseconds(9500)),
        "completed",
        "completion",
    );
    r.core.db.execute("UPDATE inbox_events SET data=json_set(data,'$.animationAcknowledged',1,'$.read',json('true'),'$.notified',json('true')) WHERE id='numeric-ack'",[]).unwrap();
    inbox(&r.core, "old-runtime", &iso(time), "failed", "completion");
    r.core.db.execute("UPDATE inbox_events SET data=json_set(data,'$.eligibleRuntime','old-runtime') WHERE id='old-runtime'",[]).unwrap();
    inbox(
        &r.core,
        "future",
        &iso(time + chrono::Duration::seconds(2)),
        "failed",
        "completion",
    );
    inbox(&r.core, "invalid", "not-a-time", "failed", "completion");
    inbox(
        &r.core,
        "expired",
        &iso(time - chrono::Duration::milliseconds(10001)),
        "failed",
        "completion",
    );
    inbox(
        &r.core,
        "other-day",
        &iso(time - chrono::Duration::days(1)),
        "failed",
        "completion",
    );
    inbox(&r.core, "ineligible", &iso(time), "failed", "completion");
    r.core.db.execute("UPDATE inbox_events SET data=json_set(data,'$.animationAcknowledged',json('true')) WHERE id='ineligible'",[]).unwrap();
    let before = changes(&r.core);
    let got = pulse_at(&r.core, Some(cursor(&baseline)), time).unwrap();
    assert_eq!(got["prompts"].as_array().unwrap().len(), 8);
    let ids: Vec<_> = got["prompts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        &ids[..5],
        ["done", "budget", "failed", "interrupted", "unknown"]
    );
    assert_eq!(&ids[5..], ["complete-00", "complete-01", "complete-02"]);
    assert_eq!(
        got["promptFeed"]["dropped"],
        json!({"overflow":10,"expired":1,"invalidTime":1,"future":1,"ineligible":3,"otherDay":1})
    );
    assert_eq!(changes(&r.core), before);
    // Updates, ACKs, and a future timestamp becoming current do not allocate another arrival.
    r.core
        .action("acknowledge_delivery", &json!({"id":"budget"}))
        .unwrap();
    r.core.action("mark_read", &json!({"id":"failed"})).unwrap();
    inbox(&r.core, "unknown", &iso(time), "completed", "completion");
    let again = pulse_at(
        &r.core,
        Some(cursor(&got)),
        time + chrono::Duration::seconds(2),
    )
    .unwrap();
    assert_eq!(again["prompts"], json!([]));
    assert_eq!(cursor(&again)["sequence"], cursor(&got)["sequence"]);
    inbox(
        &r.core,
        "brand-new",
        &iso(time + chrono::Duration::seconds(2)),
        "completed",
        "completion",
    );
    let fresh = pulse_at(
        &r.core,
        Some(cursor(&again)),
        time + chrono::Duration::seconds(2),
    )
    .unwrap();
    assert_eq!(fresh["prompts"][0]["id"], "brand-new");
    assert_eq!(fresh["prompts"].as_array().unwrap().len(), 1);
}

#[test]
fn budget_pet_prompts_require_a_current_permit_and_consume_invalid_arrivals_without_writes() {
    use crate::budget_clock::{test_support::TestClock, Reading};

    for mode in ["valid", "missing", "expired", "revoked", "native-aba"] {
        let start = Reading {
            utc: at(),
            mono_ms: 1_000,
        };
        let clock = TestClock::new(start);
        let mut rig = Rig::new();
        let gate = rig.core.notification_gate();
        gate.publish(true, false);
        let baseline = pulse_at(&rig.core, None, start.utc).unwrap();
        inbox(&rig.core, "budget", &iso(start.utc), "threshold", "budget");
        inbox(
            &rig.core,
            "complete",
            &iso(start.utc),
            "completed",
            "completion",
        );
        let permit = (mode != "missing").then(|| rig.core.grant_budget_test_permit("budget", 500));
        let read_at = if mode == "expired" {
            start.checked_deadline(501).unwrap()
        } else {
            start.checked_deadline(1).unwrap()
        };
        clock.set(read_at);
        if mode == "revoked" {
            permit.as_ref().unwrap().revoke();
        }
        if mode == "native-aba" {
            gate.publish(true, true);
            gate.publish(true, false);
        }

        let before = changes(&rig.core);
        let value = pulse_at(&rig.core, Some(cursor(&baseline)), read_at.utc).unwrap();
        let ids: Vec<_> = value["prompts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["id"].as_str().unwrap())
            .collect();
        let expected = if mode == "valid" {
            vec!["budget", "complete"]
        } else {
            vec!["complete"]
        };
        assert_eq!(ids, expected, "{mode}");
        assert_eq!(
            value["promptFeed"]["dropped"]["ineligible"],
            if mode == "valid" { 0 } else { 1 },
            "{mode}"
        );
        assert_eq!(
            changes(&rig.core),
            before,
            "read remains database read-only: {mode}"
        );
        assert_ne!(cursor(&value)["sequence"], cursor(&baseline)["sequence"]);

        // Restoring a host does not re-enqueue an already consumed arrival.
        let again = pulse_at(&rig.core, Some(cursor(&value)), read_at.utc).unwrap();
        assert_eq!(again["prompts"], json!([]), "{mode}");
        assert_eq!(cursor(&again)["sequence"], cursor(&value)["sequence"]);
    }
}

#[test]
fn pet_cursor_scope_cleanup_restart_and_decimal_precision_are_explicit() {
    let mut r = Rig::new();
    let time = at();
    // Establish a real completed source check before testing harmless metrics
    // churn. Malformed partial checks correctly remain unknown in production.
    r.core.scan().unwrap();
    let source_key = format!("lastSourceCheck:{}", r.core.source_id);
    let mut check = get_setting(&r.core.db, &source_key).unwrap().unwrap();
    assert_eq!(super::super::queries::source_runtime(&r.source, true, Some(&check)), "可用");
    let first = pulse_at(&r.core, None, time).unwrap();
    let revision = first["revision"].as_str().unwrap().parse::<u64>().unwrap();
    for value in [
        json!(false),
        json!({}),
        json!({"runtimeEpoch":"x","sourceScopeRevision":"x","day":"x","sequence":0}),
        json!({"runtimeEpoch":"x","sourceScopeRevision":"x","day":"x","sequence":"-1"}),
        json!({"runtimeEpoch":"x","sourceScopeRevision":"x","day":"x","sequence":"9223372036854775808"}),
    ] {
        assert!(pulse_at(&r.core, Some(&value), time).is_err(), "{value}");
    }
    let mut ahead = cursor(&first).clone();
    ahead["sequence"] = json!("1");
    assert!(pulse_at(&r.core, Some(&ahead), time).is_err());
    let next = pulse_at(&r.core, Some(cursor(&first)), time).unwrap();
    assert_eq!(
        next["revision"].as_str().unwrap().parse::<u64>().unwrap(),
        revision + 1
    );
    let mut old_runtime = cursor(&first).clone();
    old_runtime["runtimeEpoch"] = json!("old-runtime");
    old_runtime["sequence"] = json!("9007199254740993");
    let restored = pulse_at(&r.core, Some(&old_runtime), time).unwrap();
    assert_eq!(restored["promptFeed"]["baseline"], true);
    assert_eq!(cursor(&restored)["sequence"], "0");
    assert_eq!(restored["prompts"], json!([]));
    check["checkedAt"] = json!(iso(time + chrono::Duration::seconds(1)));
    put_setting(&r.core.db, &source_key, &check).unwrap();
    let unchanged = pulse_at(&r.core, Some(cursor(&next)), time).unwrap();
    assert_eq!(
        unchanged["sourceScopeRevision"],
        next["sourceScopeRevision"]
    );
    check["issues"] = json!(1);
    put_setting(&r.core.db, &source_key, &check).unwrap();
    let changed = pulse_at(&r.core, Some(cursor(&unchanged)), time).unwrap();
    assert_eq!(changed["promptFeed"]["baseline"], true);
    let other = r.source.with_file_name("other-source");
    fs::create_dir_all(&other).unwrap();
    r.core.add_source(&other).unwrap();
    let added = pulse_at(&r.core, Some(cursor(&changed)), time).unwrap();
    assert_eq!(added["promptFeed"]["baseline"], true);
    assert_ne!(added["sourceScopeRevision"], changed["sourceScopeRevision"]);
    inbox(&r.core, "before-clear", &iso(time), "failed", "completion");
    let before_clear = pulse_at(&r.core, Some(cursor(&added)), time).unwrap();
    let high = cursor(&before_clear)["sequence"]
        .as_str()
        .unwrap()
        .parse::<i64>()
        .unwrap();
    let inbox_before: String = r
        .core
        .db
        .query_row(
            "SELECT data FROM inbox_events WHERE id='before-clear'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    r.core.db.execute_batch("CREATE TRIGGER owned_pet_clear_failure BEFORE INSERT ON settings WHEN NEW.key='inboxWatermark' BEGIN SELECT RAISE(ABORT,'owned failure'); END;").unwrap();
    assert!(r.core.action("clear_inbox", &json!({})).is_err());
    assert_eq!(
        r.core
            .db
            .query_row(
                "SELECT data FROM inbox_events WHERE id='before-clear'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        inbox_before
    );
    assert_eq!(
        r.core
            .db
            .query_row("SELECT COUNT(*) FROM pet_inbox_feed", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(feed_high_watermark(&r.core).unwrap(), high);
    assert!(get_setting(&r.core.db, "inboxWatermark").unwrap().is_none());
    r.core
        .db
        .execute_batch("DROP TRIGGER owned_pet_clear_failure;")
        .unwrap();
    r.core.action("clear_inbox", &json!({})).unwrap();
    assert_eq!(feed_high_watermark(&r.core).unwrap(), high);
    let empty: i64 = r
        .core
        .db
        .query_row("SELECT COUNT(*) FROM pet_inbox_feed", [], |r| r.get(0))
        .unwrap();
    assert_eq!(empty, 0);
    let cleared = pulse_at(&r.core, Some(cursor(&before_clear)), time).unwrap();
    assert_eq!(cleared["promptFeed"]["baseline"], true);
    r.core
        .db
        .execute(
            "UPDATE sqlite_sequence SET seq=9007199254740993 WHERE name='pet_inbox_feed'",
            [],
        )
        .unwrap();
    inbox(
        &r.core,
        "large-sequence",
        &iso(time),
        "failed",
        "completion",
    );
    let precise = pulse_at(&r.core, Some(cursor(&cleared)), time).unwrap();
    assert_eq!(cursor(&precise)["sequence"], "9007199254740994");
    assert_eq!(precise["prompts"][0]["id"], "large-sequence");
    let tomorrow = pulse_at(
        &r.core,
        Some(cursor(&precise)),
        time + chrono::Duration::days(1),
    )
    .unwrap();
    assert_eq!(tomorrow["promptFeed"]["baseline"], true);
    assert_eq!(
        tomorrow["asOf"],
        iso(time + chrono::Duration::days(1)),
        "no clock clamping"
    );
    r.core.shutdown().unwrap();
    drop(r.core);
    let mut reopened = Core::open(&r.data, &r.source).unwrap();
    let restarted = pulse_at(&reopened, Some(cursor(&precise)), time).unwrap();
    assert_eq!(restarted["promptFeed"]["baseline"], true);
    assert_ne!(restarted["runtimeEpoch"], precise["runtimeEpoch"]);
    assert_eq!(restarted["prompts"], json!([]));
    let high = feed_high_watermark(&reopened).unwrap();
    reopened.action("clear_data", &json!({})).unwrap();
    assert_eq!(feed_high_watermark(&reopened).unwrap(), high);
    let cleared = pulse_at(&reopened, Some(cursor(&restarted)), time).unwrap();
    assert_eq!(cleared["promptFeed"]["baseline"], true);
    inbox(
        &reopened,
        "after-clear-data",
        &iso(time),
        "failed",
        "completion",
    );
    let new = pulse_at(&reopened, Some(cursor(&cleared)), time).unwrap();
    assert_eq!(new["prompts"][0]["id"], "after-clear-data");
    assert_eq!(feed_high_watermark(&reopened).unwrap(), high + 1);
}

#[test]
fn pet_pulse_chrono_freshness_midnight_and_unknown_watermarks_match_existing_rules() {
    let r = Rig::new();
    let time = parsed("2026-10-16T15:59:59.999Z").unwrap();
    for (id, delta) in [
        ("fresh", 599999),
        ("stale", 600000),
        ("future-allowed", -5999),
        ("future-uncertain", -6000),
    ] {
        turn(
            &r.core,
            id,
            &iso(time - chrono::Duration::milliseconds(delta)),
            "active",
            None,
            Value::Null,
        );
    }
    turn(
        &r.core,
        "invalid",
        "not-rfc3339",
        "active",
        None,
        Value::Null,
    );
    turn(
        &r.core,
        "child",
        &iso(time),
        "active",
        Some("fresh"),
        Value::Null,
    );
    inbox(
        &r.core,
        "before-midnight",
        &iso(time),
        "unknown",
        "completion",
    );
    let before = pulse_at(&r.core, None, time).unwrap();
    assert_eq!(
        before["activity"],
        json!({"activeTurns":2,"uncertainTurns":3,"state":"active"})
    );
    assert_eq!(before["day"], "2026-10-16");
    assert_eq!(before["unreadCount"], 1);
    assert_eq!(before["health"]["lastSuccessfulScan"], Value::Null);
    assert_eq!(before["health"]["lastSourceEvent"], Value::Null);
    let after = pulse_at(
        &r.core,
        Some(cursor(&before)),
        time + chrono::Duration::milliseconds(1),
    )
    .unwrap();
    assert_eq!(after["day"], "2026-10-17");
    assert_eq!(after["unreadCount"], 0);
    assert_eq!(after["promptFeed"]["baseline"], true);
    let before_error = r.core.pet_read_revision.get();
    r.core.db.execute_batch("DROP TABLE turns;").unwrap();
    assert!(
        pulse_at(&r.core, None, time).is_err(),
        "query failure must not masquerade as zero activity"
    );
    assert_eq!(r.core.pet_read_revision.get(), before_error);
}

#[test]
fn pet_details_trust_cache_latest_null_and_scope_rejection_keep_snapshot_semantics() {
    let r = Rig::new();
    let time = at();
    let pulse = pulse_at(&r.core, None, time).unwrap();
    let empty = details_at(&r.core, &request(&pulse), time).unwrap();
    assert_eq!(
        empty["overview"],
        json!({"totalTokens":null,"cacheHitRate":null})
    );
    assert_eq!(empty["latestTurn"], Value::Null);
    native(&r.core, "pair-a", time, 100, Some(50));
    native(&r.core, "missing-cache", time, 200, None);
    native(&r.core, "pair-b", time, 300, Some(0));
    native(
        &r.core,
        "yesterday",
        time - chrono::Duration::days(1),
        10000,
        Some(1),
    );
    let alias = native(&r.core, "alias", time, 5000, Some(1));
    r.core
        .db
        .execute(
            "INSERT INTO fact_aliases(alias,fact_id,evidence) VALUES(?1,'canonical','fixture')",
            [alias],
        )
        .unwrap();
    let excluded = native(&r.core, "excluded", time, 7000, Some(1));
    r.core
        .db
        .execute(
            "INSERT INTO excluded_facts VALUES(?1,'fixture-conflict')",
            [excluded],
        )
        .unwrap();
    let missing = native(&r.core, "missing-total", time, 9000, Some(1));
    r.core
        .db
        .execute(
            "UPDATE usage_responses SET data=json_set(data,'$.counts.total',json(?1)) WHERE id=?2",
            params![serde_json::to_string(&Metric::Missing).unwrap(), missing],
        )
        .unwrap();
    turn(
        &r.core,
        "older",
        &iso(time - chrono::Duration::seconds(60)),
        "completed",
        None,
        json!(50),
    );
    turn(
        &r.core,
        "latest",
        &iso(time - chrono::Duration::seconds(1)),
        "completed",
        None,
        Value::Null,
    );
    let details = details_at(&r.core, &request(&pulse), time).unwrap();
    assert_eq!(
        details["overview"],
        json!({"totalTokens":660,"cacheHitRate":0.125})
    );
    assert_eq!(details["latestTurn"], json!({"timeToFirstTokenMs":null}));
    let snapshot = r
        .core
        .snapshot(json!({"from":pulse["day"],"to":pulse["day"]}))
        .unwrap();
    assert_eq!(
        details["overview"]["totalTokens"],
        snapshot["overview"]["totalTokens"]
    );
    assert_eq!(
        details["overview"]["cacheHitRate"],
        snapshot["overview"]["cacheHitRate"]
    );
    // Equal completed/start order is then ordered by the snapshot's lastEventAt;
    // an exact tie keeps the earlier physical insertion instead of borrowing a non-null value.
    r.core
        .db
        .execute(
            "UPDATE turns SET data=json_set(data,'$.completedAt',?1) WHERE turn_id='older'",
            [iso(time - chrono::Duration::seconds(1))],
        )
        .unwrap();
    turn(
        &r.core,
        "later-tied-row",
        &iso(time - chrono::Duration::seconds(1)),
        "completed",
        None,
        json!(900),
    );
    assert_eq!(
        details_at(&r.core, &request(&pulse), time).unwrap()["latestTurn"],
        json!({"timeToFirstTokenMs":null})
    );
    let mut wrong = request(&pulse);
    wrong["runtimeEpoch"] = json!("old");
    assert!(details_at(&r.core, &wrong, time).is_err());
    let mut injected = request(&pulse);
    injected["query"] = json!({"sourceId":"other","agentKind":"primary"});
    assert!(details_at(&r.core, &injected, time).is_err());
    let mut injected = cursor(&pulse).clone();
    injected["query"] = json!({"sourceId":"other"});
    assert!(pulse_at(&r.core, Some(&injected), time).is_err());
    assert!(details_at(&r.core, &request(&pulse), time + chrono::Duration::days(1)).is_err());
    put_setting(&r.core.db, "inboxWatermark", &json!(iso(time))).unwrap();
    assert!(details_at(&r.core, &request(&pulse), time).is_err());
    assert!(details_at(&r.core, &json!({"day":false}), time).is_err());
    // A zero is recorded; it is not substituted for an empty trusted subset.
    r.core
        .db
        .execute_batch("DELETE FROM usage_responses;")
        .unwrap();
    native_with_output(&r.core, "zero", time, 0, 0, Some(0));
    let current = pulse_at(&r.core, None, time).unwrap();
    assert_eq!(
        details_at(&r.core, &request(&current), time).unwrap()["overview"],
        json!({"totalTokens":0,"cacheHitRate":null})
    );
    r.core.db.execute_batch("DELETE FROM turns;").unwrap();
    turn(
        &r.core,
        "midnight",
        "2026-10-15T15:59:59.999Z",
        "completed",
        None,
        json!(250),
    );
    r.core.db.execute("UPDATE turns SET data=json_set(data,'$.completedAt','2026-10-16T00:00:00.000+08:00','$.lastEventAt','2026-10-15T16:00:00.000Z') WHERE turn_id='midnight'",[]).unwrap();
    assert_eq!(
        details_at(&r.core, &request(&current), time).unwrap()["latestTurn"],
        json!({"timeToFirstTokenMs":250})
    );
}

#[test]
fn pet_v5_backfill_preserves_inbox_and_last_ddl_failure_rolls_back_and_recovers() {
    for collide in [false, true] {
        let mut r = Rig::new();
        let time = at();
        inbox(&r.core, "old-one", &iso(time), "unknown", "completion");
        inbox(&r.core, "old-two", &iso(time), "completed", "completion");
        let original: Vec<String> = r
            .core
            .db
            .prepare("SELECT data FROM inbox_events ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        // Construct a genuine pre-v5 fixture: remove v6-only DDL as well as the feed.
        // Otherwise a successful 005 followed by 006 would collide with indexes
        // left over from Rig's fresh current schema rather than test v5 backfill.
        r.core.db.execute_batch("DROP INDEX legacy_usage_event_time; DROP INDEX usage_aggregate_event_time; DROP INDEX context_snapshot_event_time; DROP INDEX timing_observation_event_time; DROP INDEX fact_observation_event_time; DROP TRIGGER pet_inbox_feed_after_insert; DROP TABLE pet_inbox_feed; PRAGMA user_version=4;").unwrap();
        if collide {
            r.core.db.execute_batch("CREATE TRIGGER pet_inbox_feed_after_insert AFTER INSERT ON source_instances BEGIN SELECT 1; END;").unwrap();
        }
        r.core.shutdown().unwrap();
        drop(r.core);
        let opened = Core::open(&r.data, &r.source);
        if collide {
            assert!(opened.is_err());
            let old = Connection::open(r.data.join("ledger.sqlite3")).unwrap();
            assert_eq!(
                old.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                    .unwrap(),
                4
            );
            assert_eq!(
                old.query_row(
                    "SELECT COUNT(*) FROM sqlite_schema WHERE name='pet_inbox_feed'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
                0,
                "table and backfilled feed must roll back"
            );
            let rows: Vec<String> = old
                .prepare("SELECT data FROM inbox_events ORDER BY id")
                .unwrap()
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<std::result::Result<_, _>>()
                .unwrap();
            assert_eq!(rows, original);
            drop(old);
            let backup = fs::read_dir(&r.data)
                .unwrap()
                .map(|e| e.unwrap().path())
                .find(|p| {
                    p.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("ledger-before-v6-")
                })
                .unwrap();
            let recovery = r.data.with_file_name("recovery");
            fs::create_dir(&recovery).unwrap();
            fs::copy(backup, recovery.join("ledger.sqlite3")).unwrap();
            let backup = Connection::open(recovery.join("ledger.sqlite3")).unwrap();
            assert_eq!(
                backup
                    .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                    .unwrap(),
                "ok"
            );
            backup
                .execute_batch("DROP TRIGGER pet_inbox_feed_after_insert;")
                .unwrap();
            drop(backup);
            let recovered = Core::open(&recovery, &r.source).unwrap();
            assert_eq!(feed_high_watermark(&recovered).unwrap(), 2);
        } else {
            let mut core = opened.unwrap();
            assert_eq!(feed_high_watermark(&core).unwrap(), 2);
            let rows: Vec<String> = core
                .db
                .prepare("SELECT data FROM inbox_events ORDER BY id")
                .unwrap()
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<std::result::Result<_, _>>()
                .unwrap();
            assert_eq!(rows, original);
            inbox(&core, "old-one", &iso(time), "completed", "completion");
            assert_eq!(
                feed_high_watermark(&core).unwrap(),
                2,
                "same-id refinement is not a new arrival"
            );
            inbox(&core, "new-three", &iso(time), "failed", "completion");
            assert_eq!(feed_high_watermark(&core).unwrap(), 3);
            core.shutdown().unwrap();
            drop(core);
            let reopened = Core::open(&r.data, &r.source).unwrap();
            assert_eq!(feed_high_watermark(&reopened).unwrap(), 3);
        }
    }
}
