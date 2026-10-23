use super::super::model::fixtures;
use super::*;

fn at() -> DateTime<Utc> {
    DateTime::from_timestamp(2000, 0).unwrap()
}
fn db() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE TABLE usage_responses(id TEXT PRIMARY KEY,source_id TEXT NOT NULL,event_time TEXT NOT NULL,data TEXT NOT NULL);CREATE TABLE excluded_facts(fact_id TEXT PRIMARY KEY,reason TEXT NOT NULL);CREATE TABLE fact_aliases(alias TEXT PRIMARY KEY,fact_id TEXT NOT NULL,evidence TEXT NOT NULL);").unwrap();
    db.execute_batch(include_str!(
        "../../../migrations/008_subscription_learning.sql"
    ))
    .unwrap();
    db
}
fn inputs() -> (Value, Vec<Value>) {
    let mut pair = fixtures::pair(0, 1000.0, 10.0);
    pair.start.scope.clear_generation = "null".into();
    pair.end.scope.clear_generation = "null".into();
    let receipts=[&pair.start,&pair.end].into_iter().map(|s|json!({"documentKind":"subscription_observation","schemaVersion":1,"outcome":"success","observation":{},"runtimeId":s.scope.runtime_id,"sourceId":s.scope.source_id,"bindingGeneration":s.scope.binding_generation.to_string(),"identityEpoch":s.scope.identity_epoch.to_string(),"clearGeneration":null,"observationSequence":s.sequence.to_string()})).collect::<Vec<_>>();
    let result = json!({"id":"shared","windowId":"shared","model":"synthetic-model-a","quality":"conditional","capacityLower":900000,"capacityUpper":1100000,"remainingLower":100000,"remainingUpper":200000,"profileEvidence":fixtures::profile(),"sampleCount":3,"cycleCount":1,"reasons":[],"inputPairs":[pair]});
    (
        json!({"algorithmVersion":"subscription-quota-v1","calibrations":[result]}),
        receipts,
    )
}
fn current() -> Value {
    json!({"identityConsistency":"proven","accountScope":"synthetic-account","entitlementSegment":"entitlement-1","clearGeneration":null})
}
fn save(db: &mut Connection, derived: &Value, history: &[Value]) {
    let tx = db.transaction().unwrap();
    advance(&tx, "codex-local", derived, history, 1, at()).unwrap();
    tx.commit().unwrap();
}
fn count(db: &Connection) -> i64 {
    db.query_row("SELECT COUNT(*) FROM quota_calibrations", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn archive_versions_retain_parameters_inputs_and_reject_query_side_writes() {
    let mut db = db();
    let (derived, history) = inputs();
    save(&mut db, &derived, &history);
    assert_eq!(count(&db), 1);
    save(&mut db, &derived, &history);
    assert_eq!(count(&db), 1, "same derivation has no new revision");
    let before = db.total_changes();
    let card = cards(&db, "codex-local", &history, Some(&current()))
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(before, db.total_changes());
    assert_eq!(card["quality"], "conditional");
    assert!(card["remainingLower"].is_null());
    assert!(card.get("inputPairs").is_none());
    let saved = latest(&db, None).unwrap().pop().unwrap();
    assert!(saved["parameters"]["native"].is_object());
    assert_eq!(
        saved["dependencies"]["observations"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(advance(&db, "codex-local", &derived, &history, 1, at()).is_err());
}
#[test]
fn archive_missing_evidence_is_visible_invalidated_history_after_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("archive.sqlite");
    let seed = db();
    let sql = seed
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name='quota_calibrations'",
            [],
            |r| r.get::<_, String>(0),
        )
        .unwrap();
    let mut disk = Connection::open(&path).unwrap();
    disk.execute_batch(&format!("{sql};CREATE TABLE usage_responses(id TEXT PRIMARY KEY,source_id TEXT,event_time TEXT,data TEXT);CREATE TABLE excluded_facts(fact_id TEXT,reason TEXT);CREATE TABLE fact_aliases(alias TEXT,fact_id TEXT,evidence TEXT);")).unwrap();
    let (derived, history) = inputs();
    save(&mut disk, &derived, &history);
    drop(disk);
    let mut disk = Connection::open(&path).unwrap();
    let tx = disk.transaction().unwrap();
    reconcile(&tx, "codex-local", &[], at()).unwrap();
    tx.commit().unwrap();
    assert_eq!(count(&disk), 2);
    let card = cards(&disk, "codex-local", &[], Some(&current()))
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(card["quality"], "invalidated");
    assert!(card["capacityLower"].is_null());
    assert_eq!(card["reasons"], json!(["evidence_removed"]));
    let original: String = disk
        .query_row(
            "SELECT data FROM quota_calibrations WHERE revision=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&original).unwrap()["result"]["capacityLower"],
        900000
    );
}
#[test]
fn empirical_archive_retains_scenario_but_revokes_it_when_dependencies_change() {
    let mut db = db();
    let (mut derived, history) = inputs();
    let result = &mut derived["calibrations"][0];
    result["empiricalCapacity"] = json!(1_000_000);
    for field in [
        "capacityLower",
        "capacityUpper",
        "remainingLower",
        "remainingUpper",
    ] {
        result[field] = Value::Null;
    }
    result["reasons"] = json!(["precision_unknown"]);
    save(&mut db, &derived, &history);
    let card = &cards(&db, "codex-local", &history, Some(&current())).unwrap()[0];
    assert_eq!(card["empiricalCapacity"], 1_000_000);
    let before = db.total_changes();
    let revoked = &cards(&db, "codex-local", &[], Some(&current())).unwrap()[0];
    assert_eq!(revoked["quality"], "invalidated");
    assert!(revoked["empiricalCapacity"].is_null());
    assert_eq!(db.total_changes(), before);
    let tx = db.transaction().unwrap();
    reconcile(&tx, "codex-local", &[], at()).unwrap();
    tx.commit().unwrap();
    let revoked = &cards(&db, "codex-local", &[], Some(&current())).unwrap()[0];
    assert!(revoked["empiricalCapacity"].is_null());
    let original: String = db
        .query_row(
            "SELECT data FROM quota_calibrations WHERE revision=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&original).unwrap()["result"]["empiricalCapacity"],
        1_000_000
    );
}
#[test]
fn archive_fact_correction_and_alias_or_exclusion_changes_invalidate_dependencies() {
    for mutation in [
        "UPDATE usage_responses SET data='{}' WHERE id='fact'",
        "INSERT INTO excluded_facts VALUES('fact','conflict')",
        "INSERT INTO fact_aliases VALUES('fact','canonical','verified')",
    ] {
        let mut db = db();
        let (derived, history) = inputs();
        db.execute(
            "INSERT INTO usage_responses VALUES('fact','codex-local',?1,'{\"counts\":1}')",
            [timestamp(DateTime::from_timestamp(1100, 0).unwrap())],
        )
        .unwrap();
        save(&mut db, &derived, &history);
        db.execute_batch(mutation).unwrap();
        let card = cards(&db, "codex-local", &history, Some(&current()))
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(card["quality"], "invalidated");
        assert_eq!(card["reasons"], json!(["historical_evidence_changed"]));
        let tx = db.transaction().unwrap();
        reconcile(&tx, "codex-local", &history, at()).unwrap();
        tx.commit().unwrap();
        assert_eq!(count(&db), 2);
    }
}
#[test]
fn archive_unrelated_future_usage_does_not_invalidate_frozen_ledger_interval() {
    let mut db = db();
    let (derived, history) = inputs();
    save(&mut db, &derived, &history);
    db.execute(
        "INSERT INTO usage_responses VALUES('future','codex-local',?1,'{}')",
        [timestamp(DateTime::from_timestamp(1400, 0).unwrap())],
    )
    .unwrap();
    assert_eq!(
        cards(&db, "codex-local", &history, Some(&current())).unwrap()[0]["quality"],
        "conditional"
    );
}
#[test]
fn archive_scope_change_never_relabels_old_capacity_as_current_guidance() {
    let mut db = db();
    let (derived, history) = inputs();
    save(&mut db, &derived, &history);
    let mut other = current();
    other["accountScope"] = json!("other");
    let card = cards(&db, "codex-local", &history, Some(&other))
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(card["quality"], "invalidated");
    assert_eq!(card["reasons"], json!(["scope_changed"]));
    assert!(card["capacityUpper"].is_null());
    assert!(cards(&db, "other-source", &history, Some(&current()))
        .unwrap()
        .is_empty());
    let tx = db.transaction().unwrap();
    reconcile(&tx, "other-source", &[], at()).unwrap();
    tx.commit().unwrap();
    assert_eq!(
        count(&db),
        1,
        "unrelated source cannot invalidate this archive"
    );
}
#[test]
fn archive_failure_rolls_back_with_owning_observation_transaction() {
    let mut db = db();
    let (derived, history) = inputs();
    let tx = db.transaction().unwrap();
    advance(&tx, "codex-local", &derived, &history, 1, at()).unwrap();
    tx.rollback().unwrap();
    assert_eq!(count(&db), 0);
}
#[test]
fn archive_retention_respects_user_horizon_without_deleting_vendor_facts() {
    let mut db = db();
    let (derived, history) = inputs();
    save(&mut db, &derived, &history);
    db.execute(
        "INSERT INTO usage_responses VALUES('retained-source','codex-local',?1,'{}')",
        [timestamp(at())],
    )
    .unwrap();
    retain(&db, at() + Duration::days(2), 1).unwrap();
    assert_eq!(count(&db), 0);
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM usage_responses", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn archive_window_metadata_isolated_but_related_or_original_changes_invalidate_read_only() {
    let mut db = db();
    let (derived, mut history) = inputs();
    for receipt in &mut history {
        receipt["observation"]["quotaRead"] = json!({"factTimeLower":1000,"factTimeUpper":1000});
        receipt["observation"]["buckets"] = json!([{"id":"shared","primary":{"usedPercent":10}},{"id":"other","primary":{"usedPercent":20}}]);
        receipt["derivedCalibrationPatches"] =
            json!({"shared":{"proof":"frozen-shared"},"other":{"proof":"other-1"}});
    }
    save(&mut db, &derived, &history);
    let original_history = history.clone();
    let before = db.total_changes();
    for receipt in &mut history {
        receipt["derivedCalibrationPatches"]["other"]["proof"] = json!("other-2");
    }
    assert_eq!(
        cards(&db, "codex-local", &history, Some(&current())).unwrap()[0]["quality"],
        "conditional"
    );
    assert_eq!(
        db.total_changes(),
        before,
        "dependency queries must not write"
    );
    let tx = db.transaction().unwrap();
    reconcile(&tx, "codex-local", &history, at()).unwrap();
    tx.commit().unwrap();
    assert_eq!(count(&db), 1);
    history[0]["derivedCalibrationPatches"]["shared"]["proof"] = json!("changed");
    let before = db.total_changes();
    assert_eq!(
        cards(&db, "codex-local", &history, Some(&current())).unwrap()[0]["quality"],
        "invalidated"
    );
    assert_eq!(db.total_changes(), before);
    for original_change in ["scope", "read", "window"] {
        let mut history = original_history.clone();
        match original_change {
            "scope" => history[0]["observation"]["accountScope"] = json!("changed-account"),
            "read" => history[0]["observation"]["quotaRead"]["factTimeUpper"] = json!(1001),
            "window" => {
                history[0]["observation"]["buckets"][0]["primary"]["usedPercent"] = json!(11)
            }
            _ => unreachable!(),
        }
        let before = db.total_changes();
        assert_eq!(
            cards(&db, "codex-local", &history, Some(&current())).unwrap()[0]["quality"],
            "invalidated",
            "{original_change}"
        );
        assert_eq!(db.total_changes(), before);
    }
}

#[test]
fn archive_shared_cache_hashes_only_referenced_receipts_and_each_interval_once() {
    let mut db = db();
    let (mut derived, mut history) = inputs();
    let base = derived["calibrations"][0].clone();
    let mut results = Vec::new();
    for i in 0..3 {
        let mut value = base.clone();
        value["profileEvidence"]["outputShare"] = json!(0.2 + i as f64 * 0.01);
        results.push(value);
    }
    derived["calibrations"] = json!(results);
    save(&mut db, &derived, &history);
    let irrelevant = history[0].clone();
    for seq in 10..1010 {
        let mut receipt = irrelevant.clone();
        receipt["observationSequence"] = json!(seq.to_string());
        history.push(receipt);
    }
    let before = db.total_changes();
    let mut cache = DependencyCache::new(&db, &history, "codex-local").unwrap();
    assert_eq!(cache.receipt_digest_computations, 0);
    for _ in 0..2 {
        for value in latest(&db, Some("codex-local")).unwrap() {
            assert!(dependency_reason(&value, &mut cache).unwrap().is_none());
        }
    }
    assert_eq!(
        cache.receipt_digest_computations, 2,
        "whole retained history is indexed once, never eagerly hashed for each profile"
    );
    assert_eq!(
        cache.ledger_digest_computations, 1,
        "the shared exact interval is read and hashed only once"
    );
    assert_eq!(cache.receipt_digests.len(), 2);
    assert_eq!(cache.ledger_digests.len(), 1);
    assert_eq!(db.total_changes(), before);
    let before = db.total_changes();
    assert_eq!(
        cards(&db, "codex-local", &history, Some(&current()))
            .unwrap()
            .len(),
        3
    );
    assert_eq!(db.total_changes(), before);
}

#[test]
fn archive_lazily_resolves_only_raw_encoded_dependencies_and_still_rejects_a_removed_base() {
    let mut db = db();
    db.execute_batch("CREATE TABLE account_snapshots(id INTEGER PRIMARY KEY AUTOINCREMENT,collected_at TEXT NOT NULL,data TEXT NOT NULL);").unwrap();
    let (derived, receipts) = inputs();
    let mut raw = Vec::new();
    let tx = db.transaction().unwrap();
    for (i, receipt) in receipts.into_iter().enumerate() {
        let mut receipt = receipt;
        receipt["observation"]["typedFixturePayload"] = json!("界".repeat(4096));
        let encoded = super::super::observation_storage::encode(
            &tx,
            &receipt,
            at() + Duration::seconds(i as i64),
        )
        .unwrap();
        tx.execute(
            "INSERT INTO account_snapshots(collected_at,data) VALUES(?1,?2)",
            params![timestamp(at()), encoded.to_string()],
        )
        .unwrap();
        raw.push(encoded);
    }
    assert_eq!(raw[1]["subscriptionStorage"]["encoding"], "reference");
    assert!(raw[1].get("observation").is_none());
    advance(&tx, "codex-local", &derived, &raw, 1, at()).unwrap();
    tx.commit().unwrap();
    // An unrelated invalid encoded header must never be eagerly expanded.
    let mut unrelated = raw[1].clone();
    unrelated["observationSequence"] = json!("999");
    unrelated["subscriptionStorage"]["baseReceiptId"] = json!(999_999);
    raw.push(unrelated);
    let before = db.total_changes();
    let mut cache = DependencyCache::new(&db, &raw, "codex-local").unwrap();
    assert_eq!(cache.receipt_digest_computations, 0);
    for value in latest(&db, Some("codex-local")).unwrap() {
        assert!(dependency_reason(&value, &mut cache).unwrap().is_none());
        assert!(dependency_reason(&value, &mut cache).unwrap().is_none());
    }
    assert_eq!(cache.receipt_digest_computations, 2);
    assert_eq!(
        cache.receipt_digests.len(),
        2,
        "no reconstructed full bodies are retained"
    );
    assert_eq!(
        cards(&db, "codex-local", &raw, Some(&current())).unwrap()[0]["quality"],
        "conditional"
    );
    assert_eq!(db.total_changes(), before);
    db.execute("DELETE FROM account_snapshots WHERE id=1", [])
        .unwrap();
    let before = db.total_changes();
    let card = &cards(&db, "codex-local", &raw, Some(&current())).unwrap()[0];
    assert_eq!(card["quality"], "invalidated");
    assert!(card["capacityLower"].is_null());
    assert_eq!(card["reasons"], json!(["evidence_removed"]));
    assert_eq!(db.total_changes(), before);
}

#[test]
fn projection_budget_tombstones_hide_numbers_without_permanently_invalidating_archive() {
    let mut db = db();
    let (derived, history) = inputs();
    save(&mut db, &derived, &history);
    let mut deferred = history.clone();
    for receipt in &mut deferred {
        receipt["observation"] = Value::Null;
        receipt["storageEvidence"] = json!({"available":false,"evidenceRemoved":false,"reason":"projection_budget_exceeded"});
    }
    let before = db.total_changes();
    let card = &cards(&db, "codex-local", &deferred, Some(&current())).unwrap()[0];
    assert_eq!(card["quality"], "invalidated");
    assert!(card["capacityLower"].is_null());
    assert_eq!(card["reasons"], json!(["projection_budget_exceeded"]));
    assert_eq!(db.total_changes(), before);
    let tx = db.transaction().unwrap();
    reconcile(&tx, "codex-local", &deferred, at()).unwrap();
    advance(&tx, "codex-local", &derived, &deferred, 2, at()).unwrap();
    tx.commit().unwrap();
    assert_eq!(
        count(&db),
        1,
        "transient projection limits cannot append an invalidation version"
    );
    assert_eq!(
        latest(&db, None).unwrap()[0]["result"]["quality"],
        "conditional"
    );
    assert_eq!(
        cards(&db, "codex-local", &history, Some(&current())).unwrap()[0]["quality"],
        "conditional"
    );
}
