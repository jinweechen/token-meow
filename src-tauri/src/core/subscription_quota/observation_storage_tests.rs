use super::*;

fn at(seconds: i64) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-22T09:30:00Z")
        .unwrap()
        .with_timezone(&Utc)
        + Duration::seconds(seconds)
}
fn database() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE TABLE account_snapshots(id INTEGER PRIMARY KEY AUTOINCREMENT,collected_at TEXT NOT NULL,data TEXT NOT NULL);CREATE TABLE settings(key TEXT PRIMARY KEY,value TEXT NOT NULL);CREATE TABLE quota_calibrations(id TEXT PRIMARY KEY,source_id TEXT NOT NULL,data TEXT NOT NULL);").unwrap();
    db
}
fn receipt(sequence: u64, seconds: i64, percent: f64) -> Value {
    let sub = json!({"requestStartedAt":timestamp(at(seconds)),"responseReceivedAt":timestamp(at(seconds)+Duration::milliseconds(20)),"freshnessAnchor":timestamp(at(seconds)),"factTimeLower":timestamp(at(seconds)),"factTimeUpper":timestamp(at(seconds)+Duration::milliseconds(20)),"timeBasis":"synthetic-fixture-only","outcome":"success"});
    let buckets=(0..8).map(|id|json!({"id":format!("bucket-{id}-{}","界".repeat(100)),"name":"界".repeat(120),"primary":{"usedPercent":percent,"windowDurationMins":300,"resetsAt":1_800_000_000},"secondary":{"usedPercent":10.0,"windowDurationMins":10080,"resetsAt":1_900_000_000}})).collect::<Vec<_>>();
    let snapshot = json!({"status":"connected","runtimeId":"runtime-a","sourceId":"source-a","bindingGeneration":"1","identityEpoch":"1","clearGeneration":null,"accountScope":"synthetic-account","identityConsistency":"proven","identityConsistencyReason":"fixture-only","identityEvidence":{"kind":"synthetic-fixture-only-not-live-proof"},"authMode":"chatgpt","planType":"pro","collectedAt":timestamp(at(seconds)),"lastAttemptAt":timestamp(at(seconds)),"observationSequence":sequence.to_string(),"observationOutcome":"success","readStartedAt":timestamp(at(seconds)),"readFinishedAt":timestamp(at(seconds)+Duration::milliseconds(20)),"accountRead":sub,"quotaRead":sub,"usageRead":sub,"buckets":buckets,"usage":null,"persisted":false});
    json!({"documentKind":"subscription_observation","schemaVersion":1,"runtimeId":"runtime-a","sourceId":"source-a","bindingGeneration":"1","identityEpoch":"1","clearGeneration":null,"observationSequence":sequence.to_string(),"lastSettledSequence":sequence.to_string(),"lastPersistedSequence":sequence.to_string(),"outcome":"success","identityConsistency":"proven","identityConsistencyReason":"fixture-only","readStartedAt":snapshot["readStartedAt"],"readFinishedAt":snapshot["readFinishedAt"],"accountRead":sub,"quotaRead":sub,"usageRead":sub,"observation":snapshot})
}
fn insert(db: &Connection, value: &Value, seconds: i64) -> i64 {
    db.execute(
        "INSERT INTO account_snapshots(collected_at,data) VALUES(?1,?2)",
        params![timestamp(at(seconds)), value.to_string()],
    )
    .unwrap();
    db.last_insert_rowid()
}
fn write(db: &Connection, value: &Value, seconds: i64) -> (i64, Value) {
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    let encoded = encode(db, value, at(seconds)).unwrap();
    let id = insert(db, &encoded, seconds);
    db.execute_batch("COMMIT").unwrap();
    (id, encoded)
}
fn raw(db: &Connection, id: i64) -> Value {
    parse_raw(db, id).unwrap().unwrap()
}
fn policy(db: &Connection, id: &str, created: DateTime<Utc>, assignments: Value, source: &str) {
    db.execute("INSERT INTO settings VALUES(?1,?2)",params![format!("{HOLDOUT_PREFIX}{id}"),json!({"id":id,"createdAt":timestamp(created),"sourceId":source,"assignments":assignments}).to_string()]).unwrap();
}

fn typed_snapshot(
    sequence: u64,
    seconds: i64,
    change: &str,
) -> (
    Value,
    super::super::calibration_assembler::AdapterQuotaProof,
) {
    use super::super::{calibration_assembler::*, model::*};
    use crate::account_bridge::{AccountSnapshot, AccountSubRead, QuotaBucket, QuotaWindow};
    let stamp = timestamp(at(seconds));
    let read = AccountSubRead {
        request_started_at: stamp.clone(),
        response_received_at: stamp.clone(),
        freshness_anchor: Some(stamp.clone()),
        fact_time_lower: Some(stamp.clone()),
        fact_time_upper: Some(stamp.clone()),
        time_basis: "verified_on_demand_bounds".into(),
        outcome: "success".into(),
    };
    let native = AccountSnapshot {
        status: "connected".into(),
        collected_at: Some(stamp.clone()),
        last_attempt_at: Some(stamp.clone()),
        account_scope: Some(
            if change == "account" {
                "other-account"
            } else {
                "synthetic-account"
            }
            .into(),
        ),
        identity_epoch: "1".into(),
        identity_consistency: "proven".into(),
        identity_consistency_reason: "synthetic typed storage fixture only".into(),
        observation_sequence: Some(sequence.to_string()),
        observation_outcome: Some("success".into()),
        read_started_at: Some(stamp.clone()),
        read_finished_at: Some(stamp),
        account_read: Some(read.clone()),
        quota_read: Some(read),
        buckets: vec![QuotaBucket {
            id: "shared".into(),
            name: None,
            primary: Some(QuotaWindow {
                used_percent: 20.0,
                window_duration_mins: Some(300),
                resets_at: Some(at(1800).timestamp()),
            }),
            secondary: None,
        }],
        ..Default::default()
    };
    let mut proof = AdapterQuotaProof {
        adapter_id: "fixture-adapter".into(),
        reference: "synthetic-only".into(),
        native_digest: native_observation_digest(&native),
        scope: EvaluationScope {
            runtime_id: "runtime-a".into(),
            source_id: "source-a".into(),
            binding_generation: 1,
            identity_epoch: 1,
            clear_generation: "null".into(),
            account_scope: native.account_scope.clone(),
            entitlement_segment: "fixture-rights".into(),
            rule_version: "fixture-rule".into(),
            pool_id: "shared:primary".into(),
            window_instance_id: "fixture-period".into(),
        },
        sequence,
        percent: PercentReading {
            used_percent: 20.0,
            rounding_step: Some(1.0),
            saturated: false,
            saturation_headroom_bounded: false,
        },
        fact_time: Interval::exact(at(seconds).timestamp() as f64).unwrap(),
        ttl_start: at(seconds).timestamp() as f64,
        clock_epoch: "fixture-clock".into(),
        reset_time: Interval::exact(at(1800).timestamp() as f64).unwrap(),
        recovery: RecoverySemantics::FixedReset,
        stable_identity_reference: "fixture-identity".into(),
        entitlement_reference: "fixture-entitlement".into(),
        rule_reference: "fixture-rule-reference".into(),
        period_reference: "fixture-period-reference".into(),
        period_stable_across_runtimes: true,
    };
    match change {
        "identity-reference" => proof.stable_identity_reference = "new-identity-reference".into(),
        "adapter" => proof.adapter_id = "new-adapter".into(),
        "entitlement" => proof.scope.entitlement_segment = "new-entitlement".into(),
        "entitlement-reference" => proof.entitlement_reference = "new-entitlement-reference".into(),
        "rule" => proof.scope.rule_version = "new-rule".into(),
        "period" => proof.scope.window_instance_id = "new-period".into(),
        "period-reference" => proof.period_reference = "new-period-reference".into(),
        "precision" => proof.percent.rounding_step = Some(0.5),
        "clock" => proof.clock_epoch = "new-clock".into(),
        _ => {}
    }
    let mut value = serde_json::to_value(native).unwrap();
    value["runtimeId"] = json!("runtime-a");
    value["sourceId"] = json!("source-a");
    value["bindingGeneration"] = json!("1");
    value["clearGeneration"] = Value::Null;
    value["persisted"] = json!(false);
    (value, proof)
}
fn typed_receipt(sequence: u64, seconds: i64, change: &str) -> Value {
    use super::super::calibration_assembler::observation_proof;
    let (mut value, proof) = typed_snapshot(sequence, seconds, change);
    let admission = observation_proof::admit(
        &mut value,
        None,
        &[&proof],
        &[],
        &crate::budget_clock::Reading {
            utc: at(seconds),
            mono_ms: 1000 + seconds as u64 * 1000,
        },
    );
    assert_eq!(admission["status"], "admitted", "{admission}");
    let mut r = receipt(sequence, seconds, 20.0);
    for key in [
        "runtimeId",
        "sourceId",
        "bindingGeneration",
        "identityEpoch",
        "clearGeneration",
        "observationSequence",
        "identityConsistency",
        "identityConsistencyReason",
        "readStartedAt",
        "readFinishedAt",
        "accountRead",
        "quotaRead",
        "usageRead",
    ] {
        r[key] = value[key].clone();
    }
    r["proofAdmission"] = admission;
    r["observation"] = value;
    observation_proof::seal_receipt(&mut r).unwrap();
    r
}

#[test]
fn typed_same_value_rounds_keep_complete_proofs_without_extra_training_samples() {
    let db = database();
    let first = typed_receipt(1, 0, "");
    let (_, stored_first) = write(&db, &first, 0);
    for (sequence, seconds) in [(2, 120), (3, 240), (4, 599), (5, 600)] {
        let next = typed_receipt(sequence, seconds, "");
        assert_ne!(
            first["derivedObservationProof"],
            next["derivedObservationProof"]
        );
        let (_, stored) = write(&db, &next, seconds);
        assert_eq!(
            stored[STORAGE]["encoding"],
            if seconds == 600 { "full" } else { "reference" }
        );
        assert_eq!(stored[STORAGE]["independentTrainingSample"], false);
        assert_eq!(stored[STORAGE]["originalReceiptDigest"], digest(&next));
        let resolved = resolve(&db, &stored).unwrap();
        assert_eq!(original(&resolved), next);
        assert_eq!(
            resolved["derivedObservationProof"],
            next["derivedObservationProof"]
        );
        let hydrated =
            super::super::calibration_assembler::hydrated_observation(&resolved).unwrap();
        assert_eq!(hydrated["observationSequence"], sequence.to_string());
        assert_eq!(
            hydrated["identityEvidence"]["reference"],
            "fixture-identity"
        );
        if sequence == 2 {
            assert!(stored.to_string().len() < stored_first.to_string().len());
            println!(
                "typed single-bucket stored_full={} stored_reference={}",
                stored_first.to_string().len(),
                stored.to_string().len()
            );
        }
    }
}

#[test]
fn typed_stable_identity_account_entitlement_rule_period_precision_and_clock_changes_stay_full() {
    for change in [
        "identity-reference",
        "adapter",
        "account",
        "entitlement",
        "entitlement-reference",
        "rule",
        "period",
        "period-reference",
        "precision",
        "clock",
    ] {
        let db = database();
        write(&db, &typed_receipt(1, 0, ""), 0);
        let next = typed_receipt(2, 120, change);
        let (_, stored) = write(&db, &next, 120);
        assert_eq!(stored[STORAGE]["encoding"], "full", "{change}");
        assert_eq!(
            stored[STORAGE]["independentTrainingSample"], true,
            "{change}"
        );
        assert_eq!(original(&resolve(&db, &stored).unwrap()), next);
    }
}

#[test]
fn typed_reset_transition_and_unknown_proof_shapes_cannot_be_normalized_away() {
    use super::super::calibration_assembler::observation_proof::{
        self, AdapterResetTransitionProof, ResetTransition,
    };
    for change in [
        "reset",
        "unknown-root",
        "unknown-nested",
        "unknown-window",
        "future-version",
    ] {
        let db = database();
        let alter_unknown = |receipt: &mut Value| {
            let namespace = &mut receipt["derivedObservationProof"];
            match change {
                "unknown-root" => namespace["futureEvidence"] = json!(true),
                "unknown-nested" => {
                    namespace["snapshotFields"]["identityEvidence"]["futureEvidence"] = json!(true)
                }
                "unknown-window" => {
                    namespace["windowFields"]["shared:primary"]["windowInstanceEvidence"]
                        ["futureEvidence"] = json!(true)
                }
                _ => namespace["algorithmVersion"] = json!("adapter-observation-proof-v2"),
            }
            namespace.as_object_mut().unwrap().remove("metadataDigest");
            namespace["metadataDigest"] = json!(digest(namespace));
        };
        let mut first = typed_receipt(1, 0, "");
        if change != "reset" {
            alter_unknown(&mut first);
        }
        write(&db, &first, 0);
        let mut next = typed_receipt(2, 120, "");
        if change == "reset" {
            let previous =
                super::super::calibration_assembler::hydrated_observation(&first).unwrap();
            let (mut current, proof) = typed_snapshot(2, 120, "");
            let reset = AdapterResetTransitionProof {
                adapter_id: "fixture-reset".into(),
                reference: "official-fixture-only".into(),
                scope: proof.scope.clone(),
                sequence: proof.sequence,
                native_digest: proof.native_digest.clone(),
                previous_sequence: 1,
                previous_native_digest: first["derivedObservationProof"]["nativeDigest"]
                    .as_str()
                    .unwrap()
                    .into(),
                previous_window_instance_id: proof.scope.window_instance_id.clone(),
                current_window_instance_id: proof.scope.window_instance_id.clone(),
                transition: ResetTransition::SameWindow,
                next_reset: Some(proof.reset_time),
                actual_boundary: None,
                final_used_percent: None,
            };
            assert_eq!(
                observation_proof::admit(
                    &mut current,
                    Some(&previous),
                    &[&proof],
                    &[&reset],
                    &crate::budget_clock::Reading {
                        utc: at(120),
                        mono_ms: 121_000
                    }
                )["status"],
                "admitted"
            );
            next.as_object_mut()
                .unwrap()
                .remove("derivedObservationProof");
            next["observation"] = current;
            observation_proof::seal_receipt(&mut next).unwrap();
        } else {
            alter_unknown(&mut next);
        }
        let (_, stored) = write(&db, &next, 120);
        assert_eq!(stored[STORAGE]["encoding"], "full", "{change}");
        assert_eq!(
            stored[STORAGE]["independentTrainingSample"], true,
            "{change}"
        );
    }
}

#[test]
fn typed_namespace_corruption_still_fails_storage_and_hydration_integrity() {
    for encoding in ["full", "reference"] {
        let db = database();
        let (_, first) = write(&db, &typed_receipt(1, 0, ""), 0);
        let (_, second) = write(&db, &typed_receipt(2, 120, ""), 120);
        let mut stored = if encoding == "full" { first } else { second };
        assert_eq!(stored[STORAGE]["encoding"], encoding);
        let mut resolved = resolve(&db, &stored).unwrap();
        resolved["derivedObservationProof"]["snapshotFields"]["identityEvidence"]["reference"] =
            json!("tampered");
        assert!(super::super::calibration_assembler::hydrated_observation(&resolved).is_none());
        stored["derivedObservationProof"]["scope"]["observationSequence"] = json!("900");
        let rejected = resolve(&db, &stored).unwrap();
        assert_eq!(rejected[EVIDENCE]["available"], false);
        assert_eq!(rejected[EVIDENCE]["reason"], "storage_digest_mismatch");
        assert!(super::super::calibration_assembler::hydrated_observation(&rejected).is_none());
    }
}

#[test]
fn old_full_static_digest_remains_valid_as_a_base_for_new_typed_references() {
    let db = database();
    let first = typed_receipt(1, 0, "");
    let (id, mut stored) = write(&db, &first, 0);
    stored[STORAGE]["staticDigest"] = json!(digest(&legacy_static_value(&first)));
    assert_ne!(
        stored[STORAGE]["staticDigest"],
        digest(&static_value(&first))
    );
    db.execute(
        "UPDATE account_snapshots SET data=?1 WHERE id=?2",
        params![stored.to_string(), id],
    )
    .unwrap();
    assert_eq!(original(&resolve(&db, &stored).unwrap()), first);
    let next = typed_receipt(2, 120, "");
    let (_, reference) = write(&db, &next, 120);
    assert_eq!(reference[STORAGE]["encoding"], "reference");
    assert_eq!(reference[STORAGE]["baseReceiptId"], id);
    assert_eq!(reference[STORAGE]["independentTrainingSample"], false);
    assert_eq!(original(&resolve(&db, &reference).unwrap()), next);
}

#[test]
fn sqlite_reference_round_trip_preserves_every_original_dynamic_time_and_sequence() {
    let db = database();
    let (_, first) = write(&db, &receipt(1, 0, 20.0), 0);
    assert_eq!(first[STORAGE]["encoding"], "full");
    let current = receipt(2, 120, 20.0);
    let (_, second) = write(&db, &current, 120);
    assert_eq!(second[STORAGE]["encoding"], "reference");
    assert!(second.get("observation").is_none());
    let reconstructed = resolve(&db, &second).unwrap();
    assert_eq!(original(&reconstructed), current);
    assert_eq!(
        reconstructed["quotaRead"]["freshnessAnchor"],
        timestamp(at(120))
    );
    assert_eq!(
        reconstructed["observation"]["quotaRead"]["freshnessAnchor"],
        timestamp(at(120))
    );
    assert_eq!(reconstructed[EVIDENCE]["independentTrainingSample"], false);
    assert!(second.to_string().len() < first.to_string().len());
}

#[test]
fn reference_uses_only_the_full_base_and_ten_minute_heartbeat_is_not_training_data() {
    let db = database();
    let (base, _) = write(&db, &receipt(1, 0, 20.0), 0);
    for (sequence, seconds) in [(2, 120), (3, 240), (4, 599)] {
        let (_, stored) = write(&db, &receipt(sequence, seconds, 20.0), seconds);
        assert_eq!(stored[STORAGE]["baseReceiptId"], base);
    }
    let (new_base, heartbeat) = write(&db, &receipt(5, 600, 20.0), 600);
    assert_eq!(heartbeat[STORAGE]["encoding"], "full");
    assert_eq!(heartbeat[STORAGE]["independentTrainingSample"], false);
    let (_, after) = write(&db, &receipt(6, 720, 20.0), 720);
    assert_eq!(after[STORAGE]["baseReceiptId"], new_base);
}

#[test]
fn ordinary_single_bucket_real_rounds_compress_via_exact_current_header_projection() {
    let db = database();
    let single = |sequence, seconds, percent| {
        let mut r = receipt(sequence, seconds, percent);
        r["observation"]["buckets"] = json!([{"id":"codex","name":null,"primary":{"usedPercent":percent,"windowDurationMins":300,"resetsAt":1_800_000_000},"secondary":{"usedPercent":10.0,"windowDurationMins":10080,"resetsAt":1_900_000_000}}]);
        r
    };
    let (_, first) = write(&db, &single(1, 0, 20.0), 0);
    for (seq, seconds) in [(2, 120), (3, 240), (4, 360), (5, 480)] {
        let original = single(seq, seconds, 20.0);
        let (_, stored) = write(&db, &original, seconds);
        assert_eq!(stored[STORAGE]["encoding"], "reference");
        assert!(stored.to_string().len() < first.to_string().len());
        assert!(stored[STORAGE]["headerProjection"]
            .as_array()
            .unwrap()
            .contains(&json!("quotaRead")));
        assert_eq!(original, super::original(&resolve(&db, &stored).unwrap()));
        assert_eq!(stored[STORAGE]["independentTrainingSample"], false);
        if seq == 2 {
            println!(
                "normal single-bucket stored_full={} stored_reference={} original={}",
                first.to_string().len(),
                stored.to_string().len(),
                original.to_string().len()
            );
        }
    }
    let (_, heartbeat) = write(&db, &single(6, 600, 20.0), 600);
    assert_eq!(heartbeat[STORAGE]["encoding"], "full");
    assert_eq!(heartbeat[STORAGE]["independentTrainingSample"], false);
    let (_, changed) = write(&db, &single(7, 720, 21.0), 720);
    assert_eq!(changed[STORAGE]["encoding"], "full");
    assert_eq!(changed[STORAGE]["independentTrainingSample"], true);
}

#[test]
fn changed_native_identity_membership_reset_basis_or_unknown_field_forces_full() {
    for field in ["percent", "member", "reset", "identity", "basis", "unknown"] {
        let db = database();
        write(&db, &receipt(1, 0, 20.0), 0);
        let mut next = receipt(2, 120, 20.0);
        match field {
            "percent" => next["observation"]["buckets"][0]["primary"]["usedPercent"] = json!(21.0),
            "member" => next["observation"]["planType"] = json!("plus"),
            "reset" => {
                next["observation"]["buckets"][0]["secondary"]["resetsAt"] = json!(1_900_000_001)
            }
            "identity" => next["observation"]["identityEvidence"]["kind"] = json!("different"),
            "basis" => next["observation"]["quotaRead"]["timeBasis"] = json!("different"),
            "unknown" => next["observation"]["newNativeField"] = json!(true),
            _ => {}
        }
        let (_, stored) = write(&db, &next, 120);
        assert_eq!(stored[STORAGE]["encoding"], "full", "{field}");
        assert_eq!(
            stored[STORAGE]["independentTrainingSample"], true,
            "{field}"
        );
    }
}

#[test]
fn json_null_and_missing_dynamic_fields_are_different_and_losslessly_restored() {
    let db = database();
    write(&db, &receipt(1, 0, 20.0), 0);
    let mut next = receipt(2, 120, 20.0);
    next["observation"]["collectedAt"] = Value::Null;
    next["observation"]
        .as_object_mut()
        .unwrap()
        .remove("lastAttemptAt");
    let (_, stored) = write(&db, &next, 120);
    assert_eq!(stored[STORAGE]["encoding"], "reference");
    let result = resolve(&db, &stored).unwrap();
    assert_eq!(original(&result), next);
    assert!(result["observation"].get("collectedAt").is_some());
    assert!(result["observation"].get("lastAttemptAt").is_none());
}

#[test]
fn deleted_or_changed_base_is_auditable_unavailable_without_freshness_or_derived_proofs() {
    for deleted in [false, true] {
        let db = database();
        let (base, _) = write(&db, &receipt(1, 0, 20.0), 0);
        let (_, mut stored) = write(&db, &receipt(2, 120, 20.0), 120);
        stored[DERIVED] = json!({"any":{"proof":"must-not-survive"}});
        if deleted {
            db.execute("DELETE FROM account_snapshots WHERE id=?1", [base])
                .unwrap();
        } else {
            let mut b = raw(&db, base);
            b["observation"]["planType"] = json!("tampered");
            db.execute(
                "UPDATE account_snapshots SET data=?1 WHERE id=?2",
                params![b.to_string(), base],
            )
            .unwrap();
        }
        let result = resolve(&db, &stored).unwrap();
        assert!(result["observation"].is_null());
        assert_eq!(result[EVIDENCE]["available"], false);
        assert!(result.get(DERIVED).is_none());
        assert_eq!(result["observationSequence"], "2");
        assert_eq!(result["quotaRead"]["freshnessAnchor"], timestamp(at(120)));
    }
}

#[test]
fn changed_delta_digest_and_reference_chains_fail_closed() {
    for mode in ["delta", "digest", "chain"] {
        let db = database();
        let (base, _) = write(&db, &receipt(1, 0, 20.0), 0);
        let (_, mut stored) = write(&db, &receipt(2, 120, 20.0), 120);
        match mode {
            "delta" => stored[STORAGE]["observationDelta"] = json!([]),
            "digest" => stored[STORAGE]["originalReceiptDigest"] = json!("invalid"),
            "chain" => {
                let mut b = raw(&db, base);
                b[STORAGE]["encoding"] = json!("reference");
                db.execute(
                    "UPDATE account_snapshots SET data=?1 WHERE id=?2",
                    params![b.to_string(), base],
                )
                .unwrap();
            }
            _ => {}
        }
        assert_eq!(
            resolve(&db, &stored).unwrap()[EVIDENCE]["available"],
            false,
            "{mode}"
        );
    }
}

#[test]
fn derived_namespace_is_appendable_without_expanding_refs_or_weakening_original_digest() {
    let db = database();
    write(&db, &receipt(1, 0, 20.0), 0);
    let (_, stored) = write(&db, &receipt(2, 120, 20.0), 120);
    let mut updated = resolve(&db, &stored).unwrap();
    updated[DERIVED] = json!({"bucket:primary":{"syntheticTest":true}});
    let persisted = with_derived(&db, &stored, &updated).unwrap();
    assert_eq!(persisted[STORAGE], stored[STORAGE]);
    assert!(persisted.get("observation").is_none());
    assert_eq!(resolve(&db, &persisted).unwrap(), updated);
    updated["observation"]["buckets"][0]["primary"]["usedPercent"] = json!(99.0);
    assert!(with_derived(&db, &stored, &updated).is_err());
}

#[test]
fn legacy_receipts_do_not_gain_proof_and_failed_or_old_sequence_reads_are_full() {
    let db = database();
    let legacy = receipt(1, 0, 20.0);
    assert_eq!(resolve(&db, &legacy).unwrap(), legacy);
    write(&db, &legacy, 0);
    let (_, old) = write(&db, &receipt(1, 120, 20.0), 120);
    assert_eq!(old[STORAGE]["encoding"], "full");
    let mut failed = receipt(2, 240, 20.0);
    failed["outcome"] = json!("error");
    failed.as_object_mut().unwrap().remove("observation");
    let (_, failed) = write(&db, &failed, 240);
    assert_eq!(failed[STORAGE]["encoding"], "full");
    assert!(resolve(&db, &failed).unwrap()["observation"].is_null());
}

#[test]
fn encoding_requires_a_writer_transaction_and_participates_in_rollback() {
    let db = database();
    assert!(encode(&db, &receipt(1, 0, 20.0), at(0)).is_err());
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    let value = encode(&db, &receipt(1, 0, 20.0), at(0)).unwrap();
    insert(&db, &value, 0);
    db.execute_batch("ROLLBACK").unwrap();
    assert_eq!(
        availability(&db, None, at(0), 90).unwrap()["receiptCount"],
        0
    );
}

#[test]
fn availability_discloses_current_source_counts_payload_and_global_last_actual_truncation() {
    let db = database();
    write(&db, &receipt(1, 0, 20.0), 0);
    write(&db, &receipt(2, 120, 20.0), 120);
    let mut other = receipt(1, 0, 40.0);
    other["sourceId"] = json!("source-b");
    other["observation"]["sourceId"] = json!("source-b");
    write(&db, &other, 0);
    let first = availability(&db, Some("source-a"), at(120), 365).unwrap();
    assert_eq!(first["receiptCount"], 2);
    assert_eq!(first["fullEnvelopeCount"], 1);
    assert_eq!(first["referenceReceiptCount"], 1);
    assert_eq!(first["retentionDays"], 90);
    assert_eq!(first["earliestAvailableAt"], timestamp(at(0)));
    assert_eq!(first["earliestRecordedFactAt"], timestamp(at(0)));
    assert_eq!(first["payloadIsSqliteDiskSize"], false);
    let sql_bytes:i64=db.query_row("SELECT SUM(length(CAST(data AS BLOB))) FROM account_snapshots WHERE json_extract(data,'$.sourceId')='source-a'",[],|r|r.get(0)).unwrap();
    assert_eq!(first["payloadBytes"], sql_bytes);
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    retain(&db, at(86_401), 1).unwrap();
    db.execute_batch("COMMIT").unwrap();
    let cut = availability(&db, Some("source-a"), at(86_401), 1).unwrap();
    assert_eq!(cut["receiptCount"], 1);
    assert!(cut["earliestAvailableAt"].is_null());
    assert_eq!(cut["unresolvedReferenceCount"], 1);
    assert_eq!(cut["capacityTruncation"]["deletedByAge"], 2);
    assert_eq!(
        cut["capacityTruncation"]["scope"],
        "all-subscription-evidence"
    );
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    retain(&db, at(86_402), 1).unwrap();
    db.execute_batch("COMMIT").unwrap();
    assert_eq!(
        availability(&db, None, at(86_402), 1).unwrap()["capacityTruncation"],
        cut["capacityTruncation"]
    );
}

#[test]
fn retention_selects_newest_in_order_and_does_not_delete_old_generic_vendor_snapshots() {
    let db = database();
    let generic = json!({"sourceId":"vendor-generic","old":true});
    insert(&db, &generic, -10_000_000);
    for seq in 1..=8 {
        insert(&db, &receipt(seq, seq as i64, 20.0), seq as i64);
    }
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    let report = retain_with_limits(&db, at(10), 90, 3, 1_000_000, 1_000_000).unwrap();
    db.execute_batch("COMMIT").unwrap();
    assert_eq!(report["deletedBySourceCount"], 5);
    let sequences=db.prepare("SELECT json_extract(data,'$.observationSequence') FROM account_snapshots WHERE json_extract(data,'$.documentKind')='subscription_observation' ORDER BY id").unwrap().query_map([],|r|r.get::<_,String>(0)).unwrap().collect::<std::result::Result<Vec<_>,_>>().unwrap();
    assert_eq!(sequences, vec!["6", "7", "8"]);
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM account_snapshots WHERE json_extract(data,'$.old')=1",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[test]
fn source_and_global_payload_eviction_use_utf8_bytes_with_bounded_sqlite_test_limits() {
    let db = database();
    for source in ["source-a", "source-b", "source-c"] {
        for seq in 1..=3 {
            let mut r = receipt(seq, seq as i64, 20.0);
            r["sourceId"] = json!(source);
            insert(&db, &r, seq as i64);
        }
    }
    let row_bytes = receipt(1, 1, 20.0).to_string().len() as i64;
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    let report =
        retain_with_limits(&db, at(10), 90, 100, row_bytes * 2 + 10, row_bytes * 3 + 10).unwrap();
    db.execute_batch("COMMIT").unwrap();
    assert_eq!(report["deletedBySourceBytes"], 3);
    assert_eq!(report["deletedByGlobalBytes"], 3);
    let total: i64 = db
        .query_row(
            "SELECT SUM(length(CAST(data AS BLOB))) FROM account_snapshots",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(total <= row_bytes * 3 + 10);
}

#[test]
fn holdout_retention_protects_surviving_receipt_refs_assignments_archives_and_failed_policies() {
    let db = database();
    let mut r = receipt(1, 0, 20.0);
    r[DERIVED] =
        json!({"w":{"calibrationEvidence":{"holdoutManifestId":"receipt-policy","failed":true}}});
    let endpoint = insert(&db, &r, 0);
    policy(
        &db,
        "receipt-policy",
        at(-100 * 86_400),
        json!([]),
        "source-a",
    );
    policy(
        &db,
        "assignment-policy",
        at(-100 * 86_400),
        json!([{"startReceipt":endpoint,"endReceipt":9999}]),
        "source-b",
    );
    policy(
        &db,
        "archive-policy",
        at(-100 * 86_400),
        json!([]),
        "source-c",
    );
    db.execute(
        "INSERT INTO quota_calibrations VALUES('archive','source-c',?1)",
        [
            json!({"result":{"quality":"invalidated","reason":"failed holdout retained"}})
                .to_string(),
        ],
    )
    .unwrap();
    policy(
        &db,
        "expired-unused",
        at(-100 * 86_400),
        json!([]),
        "source-d",
    );
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    let report = retain_holdouts(&db, at(0), 90).unwrap();
    db.execute_batch("COMMIT").unwrap();
    assert_eq!(report["protectedPolicies"], 3);
    assert_eq!(report["deletedPolicies"], 1);
    assert_eq!(report["policyCount"], 3);
    assert!(setting(&db, "subscriptionHoldout:expired-unused")
        .unwrap()
        .is_none());
}

#[test]
fn unreferenced_holdout_settings_are_bounded_but_in_use_manifests_are_never_deleted_for_a_cap() {
    let db = database();
    for n in 0..101 {
        policy(&db, &format!("p{n:03}"), at(n), json!([]), "source-a");
    }
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    let report = retain_holdouts(&db, at(200), 90).unwrap();
    db.execute_batch("COMMIT").unwrap();
    assert_eq!(report["policyCount"], 100);
    assert_eq!(report["deletedPolicies"], 1);
    let db = database();
    let endpoint = insert(&db, &receipt(1, 0, 20.0), 0);
    for n in 0..101 {
        policy(
            &db,
            &format!("p{n:03}"),
            at(n),
            json!([{"startReceipt":endpoint,"endReceipt":endpoint}]),
            "source-a",
        );
    }
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    let report = retain_holdouts(&db, at(200), 90).unwrap();
    db.execute_batch("COMMIT").unwrap();
    assert_eq!(report["deletedPolicies"], 0);
    assert_eq!(report["blockedByInUseEvidence"], true);
}

#[test]
fn malformed_holdout_metadata_stops_deletion_instead_of_guessing_dependency_absence() {
    let db = database();
    db.execute(
        "INSERT INTO settings VALUES('subscriptionHoldout:broken',?1)",
        [json!({"id":"broken","createdAt":"invalid","assignments":null}).to_string()],
    )
    .unwrap();
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert!(retain_holdouts(&db, at(0), 1).is_err());
    db.execute_batch("ROLLBACK").unwrap();
    assert!(setting(&db, "subscriptionHoldout:broken")
        .unwrap()
        .is_some());
}

fn maximum_receipt(escaped: bool, daily: bool) -> Value {
    let mut r = receipt(1, 0, f64::MIN_POSITIVE);
    let label = if escaped {
        "\u{0000}".repeat(120)
    } else {
        "🧮".repeat(120)
    };
    r["observation"]["buckets"]=json!((0..100).map(|_|json!({"id":label,"name":label,"primary":{"usedPercent":f64::MIN_POSITIVE,"windowDurationMins":u64::MAX,"resetsAt":i64::MAX},"secondary":{"usedPercent":f64::MIN_POSITIVE,"windowDurationMins":u64::MAX,"resetsAt":i64::MAX}})).collect::<Vec<_>>());
    if daily {
        r["observation"]["usage"] = json!({"summary":{"lifetimeTokens":u64::MAX,"peakDailyTokens":u64::MAX,"longestRunningTurnSec":u64::MAX,"currentStreakDays":u64::MAX,"longestStreakDays":u64::MAX},"dailyUsageBuckets":(0..4000).map(|_|json!({"startDate":"9999-12-31","tokens":u64::MAX})).collect::<Vec<_>>()});
    } else {
        r["observation"].as_object_mut().unwrap().remove("usage");
    }
    r
}

#[test]
fn g0_maximum_allowed_bucket_fixture_reports_actual_bytes_and_does_not_promise_ninety_days() {
    let db = database();
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    for escaped in [false, true] {
        let full = maximum_receipt(escaped, true);
        let stripped = maximum_receipt(escaped, false);
        let full_bytes = full.to_string().len();
        let stripped_bytes = stripped.to_string().len();
        let encoded = encode(&db, &stripped, at(0)).unwrap();
        let stored_bytes = encoded.to_string().len();
        println!("G0 payload fixture escaped={escaped}: raw_with_4000_daily={full_bytes}, subscription_without_usage={stripped_bytes}, encoded_full={stored_bytes}, source32MiB_full_receipts={}, total128MiB_full_receipts={}, days_at_720_per_day={:.6}",SOURCE_BYTES as usize/stored_bytes,TOTAL_BYTES as usize/stored_bytes,(SOURCE_BYTES as usize/stored_bytes)as f64/720.0);
        assert_eq!(
            stripped["observation"]["buckets"].as_array().unwrap().len(),
            100
        );
        assert!(
            stored_bytes < 256 * 1024,
            "subscription domain must fit assembler typed loader"
        );
        assert!(
            full_bytes > 256 * 1024,
            "fixture shows why daily usage must be stored outside subscription evidence"
        );
        assert_eq!(original(&resolve(&db, &encoded).unwrap()), stripped);
    }
    db.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn oversize_original_receipt_is_rejected_without_truncation_or_a_new_row() {
    let db = database();
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    let mut r = receipt(1, 0, 20.0);
    r["observation"]["oversizeSyntheticField"] = json!("界".repeat(MAX_ROW_BYTES));
    assert!(encode(&db, &r, at(0)).is_err());
    db.execute_batch("ROLLBACK").unwrap();
    assert_eq!(
        availability(&db, None, at(0), 90).unwrap()["receiptCount"],
        0
    );
}

#[test]
fn bounded_projection_expansion_preserves_latest_data_and_defers_old_bodies_without_deleting_evidence(
) {
    let db = database();
    let mut stored = vec![];
    for sequence in 1..=300 {
        let seconds = sequence as i64 * 10;
        let mut r = receipt(sequence, seconds, 20.0);
        r["observation"]["stableSyntheticPayload"] = json!("界".repeat(40_000));
        stored.push(write(&db, &r, seconds).1);
    }
    let projection = resolve_many(&db, &stored).unwrap();
    assert_eq!(projection.len(), stored.len());
    assert!(serde_json::to_vec(&projection).unwrap().len() <= 32 * 1024 * 1024);
    assert_eq!(
        projection.last().unwrap()["observation"]["observationSequence"],
        "300"
    );
    let deferred = projection
        .iter()
        .filter(|r| r[EVIDENCE]["reason"] == "projection_budget_exceeded")
        .collect::<Vec<_>>();
    assert!(!deferred.is_empty());
    for r in &deferred {
        assert_eq!(r[EVIDENCE]["evidenceRemoved"], false);
        assert!(r["observation"].is_null());
    }
    assert_eq!(deferred[0]["observationSequence"], "1");
    // The canonical stored evidence still resolves on demand for an archive.
    assert!(resolve(&db, &stored[0]).unwrap()["observation"].is_object());
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM account_snapshots", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        300
    );
}

#[test]
fn latest_1024_limit_is_temporary_and_missing_base_remains_a_real_evidence_failure() {
    let db = database();
    let mut stored = vec![];
    for sequence in 1..=1030 {
        stored.push(
            write(
                &db,
                &receipt(sequence, sequence as i64, 20.0),
                sequence as i64,
            )
            .1,
        );
    }
    let projection = resolve_many(&db, &stored).unwrap();
    assert_eq!(
        projection[5][EVIDENCE]["reason"],
        "projection_budget_exceeded"
    );
    assert_eq!(projection[5][EVIDENCE]["evidenceRemoved"], false);
    assert!(projection[6]["observation"].is_object());
    assert!(projection.last().unwrap()["observation"].is_object());
    let last = stored.last().unwrap();
    let base = last[STORAGE]["baseReceiptId"].as_i64().unwrap();
    db.execute("DELETE FROM account_snapshots WHERE id=?1", [base])
        .unwrap();
    let result = resolve_many(&db, std::slice::from_ref(last)).unwrap();
    assert_eq!(result[0][EVIDENCE]["reason"], "evidence_removed");
    assert!(result[0]["observation"].is_null());
}

#[test]
fn old_frozen_pair_intervals_remain_resolved_across_weeks_and_keep_intermediate_barriers() {
    let db = database();
    let mut stored = vec![];
    for sequence in 1..=1050 {
        let seconds = if sequence < 10 {
            sequence as i64 * 120
        } else {
            14 * 86_400 + (sequence - 10) as i64 * 120
        };
        let mut r = receipt(sequence, seconds, 20.0);
        if sequence == 9 {
            r[DERIVED] = json!({"bucket:primary":{"windowFields":{"calibrationEvidence":{"fromSequence":"1"}}}});
        }
        stored.push(write(&db, &r, seconds).1);
    }
    let view = resolve_many(&db, &stored).unwrap();
    for row in view.iter().take(9) {
        assert!(
            row["observation"].is_object(),
            "sealed pair's entire historical interval must survive projection selection"
        );
    }
    assert_eq!(
        view[9][EVIDENCE]["reason"], "projection_budget_exceeded",
        "unrelated old heartbeat need not expand"
    );
    assert!(view.last().unwrap()["observation"].is_object());
    assert!(serde_json::to_vec(&view).unwrap().len() <= MAX_PROJECTED_BYTES);
}

#[test]
fn frozen_manifest_assignment_receipt_ids_select_the_complete_old_interval_without_expanding_every_heartbeat(
) {
    let db = database();
    let mut stored = vec![];
    let mut endpoints = vec![];
    for sequence in 1..=1040 {
        let (id, row) = write(
            &db,
            &receipt(sequence, sequence as i64 * 120, 20.0),
            sequence as i64 * 120,
        );
        if sequence == 2 || sequence == 10 {
            endpoints.push(id);
        }
        stored.push(row);
    }
    policy(
        &db,
        "manifest-endpoints",
        at(0),
        json!([{"startReceipt":endpoints[0],"endReceipt":endpoints[1]}]),
        "source-a",
    );
    let projected = resolve_many(&db, &stored).unwrap();
    assert_eq!(
        projected[0][EVIDENCE]["reason"],
        "projection_budget_exceeded"
    );
    for row in projected.iter().skip(1).take(9) {
        assert!(row["observation"].is_object());
    }
    assert_eq!(
        projected[10][EVIDENCE]["reason"],
        "projection_budget_exceeded"
    );
}
