use super::super::projector::projections;
use super::*;
use crate::account_bridge::{AccountSubRead, QuotaBucket, QuotaWindow};

fn at(offset: i64) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-22T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
        + chrono::Duration::seconds(offset)
}
fn clock(offset: i64) -> Reading {
    Reading {
        utc: at(offset),
        mono_ms: 1_000_000 + offset as u64 * 1000,
    }
}
fn database(db: &Connection) {
    // Use the real migrations and real typed ResponseUsage writer, rather than
    // constructing a parallel mock fact schema or handwritten calibration JSON.
    for sql in [
        include_str!("../../../migrations/001_initial.sql"),
        include_str!("../../../migrations/002_multi_source.sql"),
        include_str!("../../../migrations/003_fact_references.sql"),
        include_str!("../../../migrations/004_usage_lookup_index.sql"),
        include_str!("../../../migrations/005_pet_read_models.sql"),
        include_str!("../../../migrations/006_retention_time_indexes.sql"),
        include_str!("../../../migrations/007_subscription_plans.sql"),
        include_str!("../../../migrations/008_subscription_learning.sql"),
    ] {
        db.execute_batch(&sql.replace("PRAGMA journal_mode=WAL;", ""))
            .unwrap();
    }
    db.execute(
        "INSERT INTO source_instances(id,root,created_at) VALUES('source-a','fixture-only',?1)",
        [at(0).to_rfc3339()],
    )
    .unwrap();
}
fn memory() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    database(&db);
    db
}
fn put(db: &Connection, key: &str, value: Value) {
    db.execute("INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key,value.to_string()]).unwrap();
}
fn scope(runtime: &str, period: &str) -> EvaluationScope {
    EvaluationScope {
        runtime_id: runtime.into(),
        source_id: "source-a".into(),
        binding_generation: 1,
        identity_epoch: 1,
        clear_generation: "null".into(),
        account_scope: Some("account-a".into()),
        entitlement_segment: "rights-a".into(),
        rule_version: "rules-a".into(),
        pool_id: "shared:primary".into(),
        window_instance_id: period.into(),
    }
}
fn native(sequence: u64, offset: i64, used: f64, reset: i64) -> AccountSnapshot {
    let stamp = at(offset).to_rfc3339();
    let read = AccountSubRead {
        request_started_at: stamp.clone(),
        response_received_at: stamp.clone(),
        freshness_anchor: Some(stamp.clone()),
        fact_time_lower: Some(stamp.clone()),
        fact_time_upper: Some(stamp.clone()),
        time_basis: "verified_on_demand_bounds".into(),
        outcome: "success".into(),
    };
    let mut snapshot = AccountSnapshot::default();
    snapshot.status = "connected".into();
    snapshot.collected_at = Some(stamp.clone());
    snapshot.auth_mode = Some("chatgpt".into());
    snapshot.account_scope = Some("account-a".into());
    snapshot.plan_type = Some("fixture-plan-only".into());
    snapshot.identity_epoch = "1".into();
    snapshot.identity_consistency = "proven".into();
    snapshot.identity_consistency_reason = "typed-fixture-only".into();
    snapshot.observation_sequence = Some(sequence.to_string());
    snapshot.observation_outcome = Some("success".into());
    snapshot.read_started_at = Some(stamp.clone());
    snapshot.read_finished_at = Some(stamp);
    snapshot.account_read = Some(read.clone());
    snapshot.quota_read = Some(read);
    snapshot.buckets = vec![QuotaBucket {
        id: "shared".into(),
        name: Some("fixture pool".into()),
        primary: Some(QuotaWindow {
            used_percent: used,
            window_duration_mins: Some(10080),
            resets_at: Some(at(reset).timestamp()),
        }),
        secondary: None,
    }];
    snapshot
}
fn store_native(
    db: &Connection,
    snapshot: &AccountSnapshot,
    runtime: &str,
    period: &str,
) -> AdapterQuotaProof {
    store_native_with_encoding(db, snapshot, runtime, period, false)
}
fn store_native_with_encoding(
    db: &Connection,
    snapshot: &AccountSnapshot,
    runtime: &str,
    period: &str,
    encoded: bool,
) -> AdapterQuotaProof {
    let sequence = snapshot
        .observation_sequence
        .as_deref()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    let mut value = serde_json::to_value(snapshot).unwrap();
    value["runtimeId"] = json!(runtime);
    value["sourceId"] = json!("source-a");
    value["bindingGeneration"] = json!("1");
    value["clearGeneration"] = Value::Null;
    value["persisted"] = json!(false);
    let mut receipt = json!({"documentKind":"subscription_observation","schemaVersion":1,"runtimeId":runtime,"sourceId":"source-a","bindingGeneration":"1",
        "identityEpoch":"1","clearGeneration":null,"observationSequence":sequence.to_string(),"lastPersistedSequence":sequence.to_string(),"outcome":"success","observation":value});
    if encoded {
        // The compression case uses the actual production receipt header.
        // Missing these duplicate time fields would correctly make a reference
        // larger than the fixture's artificially abbreviated full envelope.
        for field in [
            "identityConsistency",
            "identityConsistencyReason",
            "readStartedAt",
            "readFinishedAt",
            "accountRead",
            "quotaRead",
            "usageRead",
        ] {
            receipt[field] = receipt["observation"][field].clone();
        }
        receipt["lastSettledSequence"] = json!(sequence.to_string());
        receipt["observation"]
            .as_object_mut()
            .unwrap()
            .remove("usage");
    }
    let receipt = if encoded {
        super::super::observation_storage::encode(
            db,
            &receipt,
            DateTime::parse_from_rfc3339(snapshot.collected_at.as_deref().unwrap())
                .unwrap()
                .with_timezone(&Utc),
        )
        .unwrap()
    } else {
        receipt
    };
    db.execute(
        "INSERT INTO account_snapshots(collected_at,data) VALUES(?1,?2)",
        params![snapshot.collected_at, receipt.to_string()],
    )
    .unwrap();
    AdapterQuotaProof {
        adapter_id: "typed-fixture-adapter".into(),
        reference: "fixed-fixture-only".into(),
        native_digest: native_observation_digest(snapshot),
        scope: scope(runtime, period),
        sequence,
        percent: PercentReading {
            used_percent: snapshot.buckets[0].primary.as_ref().unwrap().used_percent,
            rounding_step: Some(1.0),
            saturated: false,
            saturation_headroom_bounded: false,
        },
        fact_time: Interval::exact(
            instant(&json!(
                snapshot.quota_read.as_ref().unwrap().fact_time_lower
            ))
            .unwrap(),
        )
        .unwrap(),
        ttl_start: instant(&json!(
            snapshot.quota_read.as_ref().unwrap().freshness_anchor
        ))
        .unwrap(),
        clock_epoch: "fixture-billing-clock".into(),
        reset_time: Interval::exact(
            snapshot.buckets[0]
                .primary
                .as_ref()
                .unwrap()
                .resets_at
                .unwrap() as f64,
        )
        .unwrap(),
        recovery: RecoverySemantics::FixedReset,
        stable_identity_reference: "typed-stable-account-fixture-only".into(),
        entitlement_reference: "typed-rights-fixture-only".into(),
        rule_reference: "typed-rules-fixture-only".into(),
        period_reference: "typed-period-fixture-only".into(),
        period_stable_across_runtimes: true,
    }
}
fn response(id: &str, offset: i64, total: i64) -> ResponseUsage {
    let n = adapter::Metric::Valid;
    ResponseUsage {
        source_id: "source-a".into(),
        channel_id: "fixture-native".into(),
        namespace: "fixture".into(),
        thread_id: "thread-a".into(),
        response_id: id.into(),
        turn_id: None,
        root_turn_id: None,
        event_time: at(offset).to_rfc3339(),
        collected_at: at(offset).to_rfc3339(),
        requested_model: Some("requested-other".into()),
        observed_model: Some("actual-model".into()),
        provider_id: None,
        account_scope_id: Some("account-a".into()),
        native_counts: BTreeMap::new(),
        counts: adapter::Counts {
            input: n(total * 4 / 5),
            output: n(total / 5),
            cache_read: n(total / 5),
            cache_write: n(0),
            reasoning: n(total / 25),
            total: n(total),
        },
        semantics_version: adapter::SEMANTICS.into(),
        adapter_version: "typed-fixture-adapter".into(),
        completeness: "source-recorded".into(),
        phase: "final".into(),
        revision: None,
        live: false,
    }
}
fn write_response(db: &Connection, row: &ResponseUsage) {
    adapter::store_response(db, row, false).unwrap();
}
fn observations(
    db: &Connection,
    proofs: &[(i64, AdapterQuotaProof)],
) -> Vec<CommittedQuotaObservation> {
    proofs
        .iter()
        .map(|(id, proof)| load_observation(db, *id, Some(proof)).unwrap())
        .collect()
}
fn settlement(db: &Connection, now: i64) -> LedgerSettlement {
    let check = json!({"checkedAt":at(now).to_rfc3339(),"discoveryComplete":true,"cancelled":false,"pendingFiles":0,"issues":0,"discoveryErrors":0});
    put(db, "lastSourceCheck:source-a", check.clone());
    put(db, "subscriptionRevision", json!(1));
    LedgerSettlement {
        domain: Domain::from_scope(&scope("runtime-a", "period-a")).unwrap(),
        revision: 1,
        complete_interval: Interval::new(seconds(at(0)), seconds(at(now))).unwrap(),
        captured_at: seconds(at(now)),
        source_check_digest: source_check_digest(&check),
        metering_version: adapter::SEMANTICS.into(),
        billing_clock_epoch: "fixture-billing-clock".into(),
        billing_alignment_reference: "typed-exact-event-billing-fixture-only".into(),
    }
}
fn coverage(now: i64, kind: CoverageKind) -> CoverageAttestation {
    CoverageAttestation {
        domain: Domain::from_scope(&scope("runtime-a", "period-a")).unwrap(),
        interval: Interval::new(seconds(at(0)), seconds(at(now))).unwrap(),
        kind,
    }
}
fn complete_coverage(now: i64) -> CoverageAttestation {
    coverage(
        now,
        CoverageKind::VerifiedComplete {
            adapter_id: "independent-coverage-fixture".into(),
            reference: "all-pool-activity-fixture-only".into(),
        },
    )
}
fn modes(now: i64) -> UsageModeAttestation {
    UsageModeAttestation {
        domain: Domain::from_scope(&scope("runtime-a", "period-a")).unwrap(),
        interval: Interval::new(seconds(at(0)), seconds(at(now))).unwrap(),
        effort: ExplicitMode::Known("medium".into()),
        speed: ExplicitMode::Unknown,
        adapter_id: "typed-mode-fixture".into(),
        reference: "fixed-mode-contract-fixture-only".into(),
    }
}
fn history(db: &Connection) -> Vec<Value> {
    let mut q = db
        .prepare("SELECT data FROM account_snapshots ORDER BY id")
        .unwrap();
    let values = q
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| {
            let raw: Value = serde_json::from_str(&r.unwrap()).unwrap();
            super::super::observation_storage::resolve(db, &raw).unwrap()
        })
        .collect();
    values
}
fn projected(db: &Connection, usage: &[ResponseUsage], runtime: &str, now: i64) -> Value {
    let history = history(db);
    let mut current = history.last().unwrap()["observation"].clone();
    current["persisted"] = json!(true);
    projections(&current, &history, usage, runtime, seconds(at(now)))
}
fn run(
    db: &Connection,
    proofs: &[(i64, AdapterQuotaProof)],
    usage: &[ResponseUsage],
    now: i64,
    kind: CoverageKind,
) -> Assembly {
    let observations = observations(db, proofs);
    let settled = settlement(db, now);
    let coverage = [coverage(now, kind)];
    let modes = [modes(now)];
    db.execute_batch("BEGIN").unwrap();
    let out = assemble(
        db,
        &EvidenceBatch {
            observations: &observations,
            usage,
            settlement: Some(&settled),
            coverage: &coverage,
            modes: &modes,
        },
        &clock(now),
    )
    .unwrap();
    super::apply(db, &out).unwrap();
    let derived = projected(
        db,
        usage,
        proofs.last().unwrap().1.scope.runtime_id.as_str(),
        now,
    );
    super::super::calibration_archive::advance(db, "source-a", &derived, &history(db), 1, at(now))
        .unwrap();
    db.execute_batch("COMMIT").unwrap();
    out
}
fn verified() -> CoverageKind {
    CoverageKind::VerifiedComplete {
        adapter_id: "independent-coverage-fixture".into(),
        reference: "all-pool-activity-fixture-only".into(),
    }
}
fn simple(db: &Connection) -> (Vec<(i64, AdapterQuotaProof)>, Vec<ResponseUsage>) {
    let mut proofs = Vec::new();
    let mut usage = Vec::new();
    for i in 0..4 {
        let proof = store_native(
            db,
            &native(i + 1, i as i64 * 300, 10.0 + i as f64 * 10.0, 9000),
            "runtime-a",
            "period-a",
        );
        proofs.push((db.last_insert_rowid(), proof));
        if i > 0 {
            let row = response(&format!("r{i}"), i as i64 * 300 - 150, 100000);
            write_response(db, &row);
            usage.push(row);
            run(db, &proofs, &usage, i as i64 * 300, verified());
        }
    }
    (proofs, usage)
}
fn longitudinal(
    db: &Connection,
    bad_holdout: bool,
    incremental: bool,
    conditional: bool,
) -> (Vec<(i64, AdapterQuotaProof)>, Vec<ResponseUsage>) {
    let mut proofs = Vec::new();
    let mut usage = Vec::new();
    for (period, trains, held) in [(0, 7, 0), (1, 7, 0), (2, 6, 5)] {
        let base = period * 10000;
        for i in 0..=trains + held {
            let proof = store_native(
                db,
                &native(
                    i as u64 + 1,
                    base + i * 300,
                    10.0 + i as f64 * 5.0,
                    base + 9000,
                ),
                &format!("runtime-{period}"),
                &format!("period-{period}"),
            );
            proofs.push((db.last_insert_rowid(), proof));
            if i > 0 {
                for duplicate in 0..if bad_holdout && period == 2 && i == 8 {
                    3
                } else {
                    1
                } {
                    let row = response(
                        &format!("r{period}-{i}-{duplicate}"),
                        base + i * 300 - 150 + duplicate,
                        50000,
                    );
                    write_response(db, &row);
                    usage.push(row);
                }
                if incremental {
                    run(
                        db,
                        &proofs,
                        &usage,
                        base + i * 300,
                        if conditional {
                            CoverageKind::FutureUserAssumption {
                                declared_at: seconds(at(0)),
                                reference: "future-declaration-fixture-only".into(),
                            }
                        } else {
                            verified()
                        },
                    );
                }
            }
        }
    }
    (proofs, usage)
}
fn reason(out: &Assembly, reason: Reason) -> bool {
    out.missing.iter().any(|r| r.reason == reason)
}

#[test]
fn typed_adapter_to_real_sqlite_to_assembled_patches_to_projector_survives_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("real-ledger.sqlite3");
    let (proofs, usage) = {
        let db = Connection::open(&path).unwrap();
        database(&db);
        let fixture = simple(&db);
        let value = projected(&db, &fixture.1, "runtime-a", 900);
        let c = &value["calibrations"][0];
        assert_eq!(c["quality"], "conditional");
        assert_eq!(c["sampleCount"], 3);
        assert_eq!(c["capacityLower"], 909090.0);
        assert_eq!(c["coverage"], "verified_complete");
        assert_eq!(c["inputPairs"].as_array().unwrap().len(), 3);
        fixture
    };
    let db = Connection::open(&path).unwrap();
    let before = setting(
        &db,
        &format!(
            "{PREFIX}{}",
            projected(&db, &usage, "runtime-a", 900)["calibrations"][0]["inputPairs"][0]["pairId"]
                .as_str()
                .unwrap()
        ),
    )
    .unwrap();
    assert!(before.is_none()); // IDs are policy IDs, never pair-id string parsing.
    let out = run(&db, &proofs, &usage, 900, verified());
    assert!(out.pairs.is_empty());
    assert!(out.patches.is_empty());
    assert_eq!(
        projected(&db, &usage, "runtime-a", 900)["calibrations"][0]["sampleCount"],
        3
    );
    let count: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM settings WHERE key LIKE 'subscriptionHoldout:%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
    let history = history(&db);
    let current = hydrated_observation(history.last().unwrap()).unwrap();
    let cards = super::super::calibration_archive::cards(&db, "source-a", &history, Some(&current))
        .unwrap();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0]["quality"], "conditional");
    assert!(cards[0]["remainingLower"].is_null());
    assert!(history
        .iter()
        .all(|r| r["observation"]["entitlementSegment"].is_null()
            && r["observation"]["identityEvidence"].is_null()));
}

#[test]
fn independent_periods_and_pre_frozen_roles_really_unlock_typed_longitudinal_validation() {
    let db = memory();
    let (_, usage) = longitudinal(&db, false, true, false);
    let value = projected(&db, &usage, "runtime-2", 23300);
    let c = &value["calibrations"][0];
    assert_eq!(c["quality"], "validated");
    assert_eq!(c["sampleCount"], 20);
    assert_eq!(c["cycleCount"], 3);
    assert_eq!(c["evidence"]["holdoutPairs"], 5);
    let mut q = db
        .prepare("SELECT value FROM settings WHERE key LIKE 'subscriptionHoldout:%'")
        .unwrap();
    let raw: String = q.query_row([], |r| r.get(0)).unwrap();
    let policy: FrozenHoldoutPolicy = serde_json::from_str(&raw).unwrap();
    assert_eq!(policy.assignments.len(), 25);
    assert_eq!(policy.assignments.iter().filter(|a| a.holdout).count(), 5);
    assert!(policy
        .assignments
        .iter()
        .filter(|a| a.holdout)
        .all(|a| instant(&json!(policy.created_at)).unwrap() < a.start));
}

#[test]
fn building_after_the_history_cannot_backdate_holdout_freezing() {
    let db = memory();
    let (proofs, usage) = longitudinal(&db, false, false, false);
    run(&db, &proofs, &usage, 23300, verified());
    let c = projected(&db, &usage, "runtime-2", 23300)["calibrations"][0].clone();
    assert_eq!(c["quality"], "conditional");
    assert_eq!(c["evidence"]["holdoutPairs"], 0);
}

#[test]
fn failed_holdout_stays_in_the_assembled_dataset_and_blocks_validation() {
    let db = memory();
    let (_, usage) = longitudinal(&db, true, true, false);
    let c = projected(&db, &usage, "runtime-2", 23300)["calibrations"][0].clone();
    assert_eq!(c["quality"], "conditional");
    assert_eq!(c["evidence"]["holdoutPairs"], 5);
    assert!(c["reasons"]
        .as_array()
        .unwrap()
        .contains(&json!("backtest_failed")));
}

#[test]
fn user_future_coverage_declaration_never_becomes_verified_even_after_good_backtest() {
    let db = memory();
    let (_, usage) = longitudinal(&db, false, true, true);
    let c = projected(&db, &usage, "runtime-2", 23300)["calibrations"][0].clone();
    assert_eq!(c["quality"], "conditional");
    assert_eq!(c["coverage"], "conditional_user_assumption");
    assert_eq!(c["evidence"]["holdoutPairs"], 5);
}

#[test]
fn real_bridge_unknown_and_json_verified_flags_do_not_manufacture_adapter_proofs() {
    let db = memory();
    let mut raw = native(1, 0, 10.0, 9000);
    raw.identity_consistency = "unproven".into();
    raw.account_scope = None;
    raw.quota_read.as_mut().unwrap().fact_time_lower = None;
    raw.quota_read.as_mut().unwrap().fact_time_upper = None;
    let mut proof = store_native_unproven(&db, &raw);
    let id = db.last_insert_rowid();
    assert_eq!(
        load_observation(&db, id, None).unwrap_err().detail,
        "adapter_quota_proof_missing"
    );
    proof.scope.account_scope = None;
    assert_eq!(
        load_observation(&db, id, Some(&proof)).unwrap_err().reason,
        Reason::IdentityUnproven
    );
}
fn store_native_unproven(db: &Connection, snapshot: &AccountSnapshot) -> AdapterQuotaProof {
    // Use a valid typed carrier only to create the envelope, then replace its
    // native payload with the actual unproven DTO. JSON flags remain untrusted.
    let proof = store_native(db, &native(1, 0, 10.0, 9000), "runtime-a", "period-a");
    let id = db.last_insert_rowid();
    let mut r = history(db).last().unwrap().clone();
    let mut value = serde_json::to_value(snapshot).unwrap();
    for key in [
        "runtimeId",
        "sourceId",
        "bindingGeneration",
        "clearGeneration",
        "persisted",
    ] {
        value[key] = r["observation"][key].clone();
    }
    value["identityEvidence"] = json!({"verified":true});
    value["entitlementEvidence"] = json!({"verified":true});
    r["observation"] = value;
    db.execute(
        "UPDATE account_snapshots SET data=?1 WHERE id=?2",
        params![r.to_string(), id],
    )
    .unwrap();
    proof
}

#[test]
fn invalid_precision_time_or_changed_native_receipt_is_rejected() {
    for mode in ["precision", "time", "native", "request_time"] {
        let db = memory();
        let mut n = native(1, 0, 10.0, 9000);
        if mode == "request_time" {
            n.quota_read.as_mut().unwrap().time_basis = "request_start_only".into();
        }
        let mut proof = store_native(&db, &n, "runtime-a", "period-a");
        let id = db.last_insert_rowid();
        if mode == "precision" {
            proof.percent.rounding_step = Some(0.0);
        }
        if mode == "time" {
            proof.fact_time.upper += 1.0;
        }
        if mode == "native" {
            let mut r = history(&db)[0].clone();
            r["observation"]["buckets"][0]["primary"]["usedPercent"] = json!(11.0);
            db.execute(
                "UPDATE account_snapshots SET data=?1 WHERE id=?2",
                params![r.to_string(), id],
            )
            .unwrap();
        }
        assert!(load_observation(&db, id, Some(&proof)).is_err(), "{mode}");
    }
}

#[test]
fn trusted_unknown_quantum_loads_and_freezes_explicit_null_without_accepting_missing_precision() {
    let db = memory();
    let mut proof = store_native(&db, &native(1, 0, 10.0, 9000), "runtime-a", "period-a");
    proof.percent.rounding_step = None;
    let observation = load_observation(&db, db.last_insert_rowid(), Some(&proof)).unwrap();
    assert!(observation.sample.percent.rounding_step.is_none());
    let mut patch = make_patch(&observation, None);
    assert_eq!(patch.window_fields.get("roundingStep"), Some(&Value::Null));
    assert!(patch_shape(&patch));
    patch
        .window_fields
        .as_object_mut()
        .unwrap()
        .remove("roundingStep");
    assert!(
        !patch_shape(&patch),
        "absence cannot silently become an attested unknown quantum"
    );
}

fn two_unassembled(db: &Connection) -> (Vec<(i64, AdapterQuotaProof)>, Vec<ResponseUsage>) {
    let mut proofs = Vec::new();
    for i in 0..2 {
        let p = store_native(
            db,
            &native(i + 1, i as i64 * 300, 10.0 + i as f64 * 10.0, 9000),
            "runtime-a",
            "period-a",
        );
        proofs.push((db.last_insert_rowid(), p));
    }
    let row = response("r1", 150, 100000);
    write_response(db, &row);
    (proofs, vec![row])
}

#[test]
fn independent_coverage_settlement_and_explicit_mode_evidence_are_all_required() {
    for absent in [
        "settlement",
        "coverage",
        "mode",
        "incomplete",
        "late_declaration",
    ] {
        let db = memory();
        let (proofs, usage) = two_unassembled(&db);
        let observations = observations(&db, &proofs);
        let settled = settlement(&db, 300);
        let cov = coverage(
            300,
            if absent == "incomplete" {
                CoverageKind::KnownIncomplete
            } else if absent == "late_declaration" {
                CoverageKind::FutureUserAssumption {
                    declared_at: seconds(at(1)),
                    reference: "late".into(),
                }
            } else {
                verified()
            },
        );
        let mode = modes(300);
        db.execute_batch("BEGIN").unwrap();
        let out = assemble(
            &db,
            &EvidenceBatch {
                observations: &observations,
                usage: &usage,
                settlement: if absent == "settlement" {
                    None
                } else {
                    Some(&settled)
                },
                coverage: if absent == "coverage" {
                    &[]
                } else {
                    std::slice::from_ref(&cov)
                },
                modes: if absent == "mode" {
                    &[]
                } else {
                    std::slice::from_ref(&mode)
                },
            },
            &clock(300),
        )
        .unwrap();
        db.execute_batch("COMMIT").unwrap();
        assert!(out.pairs.is_empty(), "{absent}");
        assert!(!out.missing.is_empty(), "{absent}");
    }
}

#[test]
fn explicit_unknown_modes_form_a_separate_usable_profile() {
    let db = memory();
    let (proofs, usage) = two_unassembled(&db);
    let observations = observations(&db, &proofs);
    let settled = settlement(&db, 300);
    let cov = complete_coverage(300);
    let mut mode = modes(300);
    mode.effort = ExplicitMode::Unknown;
    db.execute_batch("BEGIN").unwrap();
    let out = assemble(
        &db,
        &EvidenceBatch {
            observations: &observations,
            usage: &usage,
            settlement: Some(&settled),
            coverage: &[cov],
            modes: &[mode],
        },
        &clock(300),
    )
    .unwrap();
    db.execute_batch("COMMIT").unwrap();
    assert_eq!(out.pairs.len(), 1);
    assert!(out.pairs[0].profile.effort.is_none());
    assert!(out.pairs[0].profile.speed_mode.is_none());
    let mut known = out.pairs[0].profile.clone();
    known.effort = Some("medium".into());
    assert!(!profiles_compatible(&known, &out.pairs[0].profile));
}

#[test]
fn denominator_classification_and_actual_model_cannot_be_filled_by_request_metadata() {
    for bad in [
        "cache",
        "input",
        "observed_model",
        "account",
        "normalization",
    ] {
        let db = memory();
        let (proofs, mut usage) = two_unassembled(&db);
        match bad {
            "cache" => usage[0].counts.cache_read = adapter::Metric::Unknown,
            "input" => {
                usage[0].counts.input = adapter::Metric::Valid(0);
                usage[0].counts.output = adapter::Metric::Valid(100000);
                usage[0].counts.cache_read = adapter::Metric::Valid(0);
            }
            "observed_model" => usage[0].observed_model = None,
            "account" => usage[0].account_scope_id = None,
            "normalization" => usage[0].counts.total = adapter::Metric::Valid(99999),
            _ => unreachable!(),
        }
        db.execute(
            "UPDATE usage_responses SET data=?1",
            [serde_json::to_string(&usage[0]).unwrap()],
        )
        .unwrap();
        let out = run(&db, &proofs, &usage, 300, verified());
        assert!(out.pairs.is_empty(), "{bad}");
        assert!(!out.missing.is_empty(), "{bad}");
    }
}

#[test]
fn database_exclusions_or_uncommitted_projection_cannot_be_marked_settled() {
    for bad in ["excluded", "projection", "revision", "source_check"] {
        let db = memory();
        let (proofs, mut usage) = two_unassembled(&db);
        let obs = observations(&db, &proofs);
        let mut settled = settlement(&db, 300);
        if bad == "excluded" {
            db.execute(
                "INSERT INTO excluded_facts VALUES(?1,'conflict')",
                [adapter::response_key(&usage[0])],
            )
            .unwrap();
        }
        if bad == "projection" {
            usage.clear();
        }
        if bad == "revision" {
            settled.revision = 2;
        }
        if bad == "source_check" {
            put(
                &db,
                "lastSourceCheck:source-a",
                json!({"discoveryComplete":false}),
            );
        }
        db.execute_batch("BEGIN").unwrap();
        let out = assemble(
            &db,
            &EvidenceBatch {
                observations: &obs,
                usage: &usage,
                settlement: Some(&settled),
                coverage: &[complete_coverage(300)],
                modes: &[modes(300)],
            },
            &clock(300),
        )
        .unwrap();
        db.execute_batch("COMMIT").unwrap();
        assert!(out.pairs.is_empty(), "{bad}");
        assert!(!out.missing.is_empty(), "{bad}");
    }
}

#[test]
fn failed_receipt_between_typed_endpoints_breaks_the_pair() {
    let db = memory();
    let first = store_native(&db, &native(1, 0, 10.0, 9000), "runtime-a", "period-a");
    let first_id = db.last_insert_rowid();
    db.execute("INSERT INTO account_snapshots(collected_at,data) VALUES(?1,?2)",params![at(150).to_rfc3339(),json!({"documentKind":"subscription_observation","schemaVersion":1,
        "sourceId":"source-a","runtimeId":"runtime-a","outcome":"failure","observationSequence":"2"}).to_string()]).unwrap();
    let last = store_native(&db, &native(3, 300, 20.0, 9000), "runtime-a", "period-a");
    let last_id = db.last_insert_rowid();
    let row = response("r1", 100, 100000);
    write_response(&db, &row);
    let out = run(
        &db,
        &[(first_id, first), (last_id, last)],
        &[row],
        300,
        verified(),
    );
    assert!(out.pairs.is_empty());
    assert!(reason(&out, Reason::Discontinuous) || reason(&out, Reason::OutOfOrder));
}

#[test]
fn core_discontinuity_marker_is_preserved_by_real_load_and_breaks_assembly() {
    for intermediate in [false, true] {
        let db = memory();
        let mut proofs = Vec::new();
        let first = store_native(&db, &native(1, 0, 10.0, 9000), "runtime-a", "period-a");
        proofs.push((db.last_insert_rowid(), first));
        if intermediate {
            store_native(&db, &native(2, 150, 15.0, 9000), "runtime-a", "period-a");
            let mut receipt = history(&db).last().unwrap().clone();
            receipt["observation"]["discontinuity"] = json!(true);
            db.execute(
                "UPDATE account_snapshots SET data=?1 WHERE id=?2",
                params![receipt.to_string(), db.last_insert_rowid()],
            )
            .unwrap();
        }
        let last = store_native(
            &db,
            &native(if intermediate { 3 } else { 2 }, 300, 20.0, 9000),
            "runtime-a",
            "period-a",
        );
        let id = db.last_insert_rowid();
        proofs.push((id, last));
        if !intermediate {
            let mut receipt = history(&db).last().unwrap().clone();
            receipt["observation"]["discontinuity"] = json!(true);
            db.execute(
                "UPDATE account_snapshots SET data=?1 WHERE id=?2",
                params![receipt.to_string(), id],
            )
            .unwrap();
            assert!(
                load_observation(&db, id, Some(&proofs.last().unwrap().1))
                    .unwrap()
                    .sample()
                    .discontinuity
            );
        }
        let row = response("r1", 100, 100000);
        write_response(&db, &row);
        let out = run(&db, &proofs, &[row], 300, verified());
        assert!(out.pairs.is_empty());
        assert!(reason(&out, Reason::Discontinuous));
    }
}

#[test]
fn small_changes_accumulate_into_one_nonoverlapping_signal_pair() {
    let db = memory();
    let mut proofs = Vec::new();
    let mut usage = Vec::new();
    for (i, used) in [10.0, 11.0, 12.0, 15.0].into_iter().enumerate() {
        let p = store_native(
            &db,
            &native(i as u64 + 1, i as i64 * 300, used, 9000),
            "runtime-a",
            "period-a",
        );
        proofs.push((db.last_insert_rowid(), p));
        if i > 0 {
            let row = response(&format!("r{i}"), i as i64 * 300 - 150, 100000);
            write_response(&db, &row);
            usage.push(row);
        }
    }
    let out = run(&db, &proofs, &usage, 900, verified());
    assert_eq!(out.pairs.len(), 1);
    assert_eq!(out.pairs[0].start.sequence, 1);
    assert_eq!(out.pairs[0].end.sequence, 4);
    assert_eq!(out.pairs[0].tokens, 300000);
}

#[test]
fn independent_structure_needs_private_fresh_seal_and_cannot_create_a_capacity_role() {
    let db = memory();
    let proofs: Vec<_> = [(1, 0), (2, 180)]
        .into_iter()
        .map(|(sequence, offset)| {
            let proof = store_native(
                &db,
                &native(sequence, offset, 20.0, 1800),
                "runtime-a",
                "period-a",
            );
            (db.last_insert_rowid(), proof)
        })
        .collect();
    let rows = [response("profile-only", 90, 100_000)];
    write_response(&db, &rows[0]);
    let settled = settlement(&db, 180);
    let observations = observations(&db, &proofs);
    let coverage = [complete_coverage(180)];
    let mut mode = modes(180);
    mode.speed = ExplicitMode::Known("normal".into());
    let modes = [mode];
    db.execute_batch("BEGIN").unwrap();
    let mut output = assemble(
        &db,
        &EvidenceBatch {
            observations: &observations,
            usage: &rows,
            settlement: Some(&settled),
            coverage: &coverage,
            modes: &modes,
        },
        &clock(180),
    )
    .unwrap();
    assert!(output.pairs.is_empty());
    assert!(output.manifests.is_empty());
    let patch = output.patches.last_mut().unwrap();
    assert!(patch.window_fields["usageStructureEvidence"]["basis"].is_object());
    let original = patch.window_fields.clone();
    patch.window_fields["usageStructureEvidence"]["profile"]["outputShare"] = json!(0.8);
    patch.window_fields["usageStructureEvidence"]["basis"]["coverageReference"] = json!("changed");
    assert!(apply(&db, &output).is_err());
    output.patches.last_mut().unwrap().window_fields = original;
    let seal = output.patches.last_mut().unwrap().structure_seal.take();
    assert!(
        apply(&db, &output).is_err(),
        "JSON metadata cannot supply its own typed seal"
    );
    output.patches.last_mut().unwrap().structure_seal = seal;
    apply(&db, &output).unwrap();
    let receipt = history(&db).last().unwrap().clone();
    let snapshot = hydrated_observation(&receipt).unwrap();
    assert!(usage_structure_current(&snapshot, "shared:primary", &rows));
    assert!(receipt["observation"]["buckets"][0]["primary"]
        .get("usageStructureEvidence")
        .is_none());
    let mut changed = receipt;
    changed["derivedCalibrationPatches"]["shared:primary"]["windowFields"]
        ["usageStructureEvidence"]["basis"]["coverageReference"] = json!("changed");
    assert!(hydrated_observation(&changed).is_none());
    db.execute_batch("COMMIT").unwrap();
}

#[test]
fn profile_only_rejects_assumed_coverage_unknown_or_conflicting_modes_and_changed_ledger() {
    for variant in [
        "assumed",
        "unknown",
        "conflicting",
        "changed",
        "billing",
        "incomplete",
    ] {
        let db = memory();
        let proofs: Vec<_> = [(1, 0), (2, 180)]
            .into_iter()
            .map(|(sequence, offset)| {
                let proof = store_native(
                    &db,
                    &native(sequence, offset, 20.0, 1800),
                    "runtime-a",
                    "period-a",
                );
                (db.last_insert_rowid(), proof)
            })
            .collect();
        let rows = [response("profile-only", 90, 100_000)];
        write_response(&db, &rows[0]);
        let mut settled = settlement(&db, 180);
        let observations = observations(&db, &proofs);
        let mut coverage = vec![complete_coverage(180)];
        let mut mode = modes(180);
        mode.speed = ExplicitMode::Known("normal".into());
        let mut modes = vec![mode];
        match variant {
            "assumed" => {
                coverage[0].kind = CoverageKind::FutureUserAssumption {
                    declared_at: seconds(at(0)),
                    reference: "user-declaration".into(),
                }
            }
            "unknown" => modes[0].speed = ExplicitMode::Unknown,
            "conflicting" => {
                let mut other = modes[0].clone();
                other.speed = ExplicitMode::Known("fast".into());
                modes.push(other);
            }
            "changed" => {
                db.execute(
                    "UPDATE usage_responses SET data=json_set(data,'$.observedModel','corrected')",
                    [],
                )
                .unwrap();
            }
            "billing" => settled.billing_clock_epoch = "other-clock".into(),
            "incomplete" => {
                coverage.push(super::tests::coverage(180, CoverageKind::KnownIncomplete))
            }
            _ => unreachable!(),
        }
        db.execute_batch("BEGIN").unwrap();
        let output = assemble(
            &db,
            &EvidenceBatch {
                observations: &observations,
                usage: &rows,
                settlement: Some(&settled),
                coverage: &coverage,
                modes: &modes,
            },
            &clock(180),
        )
        .unwrap();
        assert!(output.pairs.is_empty());
        assert!(
            output
                .patches
                .iter()
                .all(|patch| patch.window_fields.get("usageStructureEvidence").is_none()),
            "{variant}"
        );
        apply(&db, &output).unwrap();
        db.execute_batch("COMMIT").unwrap();
    }
}

#[test]
fn replay_after_fact_deletion_keeps_original_digest_and_does_not_retrain_a_smaller_numerator() {
    let db = memory();
    let (proofs, mut usage) = simple(&db);
    let old = history(&db)[1]["derivedCalibrationPatches"]["shared:primary"]["windowFields"]
        ["calibrationEvidence"]["responseLedgerDigest"]
        .clone();
    db.execute(
        "DELETE FROM usage_responses WHERE id=?1",
        [adapter::response_key(&usage[0])],
    )
    .unwrap();
    usage.remove(0);
    let out = run(&db, &proofs, &usage, 900, verified());
    assert!(out.patches.is_empty());
    assert!(out.pairs.is_empty());
    assert_eq!(
        history(&db)[1]["derivedCalibrationPatches"]["shared:primary"]["windowFields"]
            ["calibrationEvidence"]["responseLedgerDigest"],
        old
    );
    assert_eq!(
        projected(&db, &usage, "runtime-a", 900)["calibrations"][0]["quality"],
        "invalidated"
    );
}

#[test]
fn policy_and_evidence_changes_roll_back_with_the_callers_observation_transaction() {
    let db = memory();
    let (proofs, usage) = two_unassembled(&db);
    let observations = observations(&db, &proofs);
    let settled = settlement(&db, 300);
    let cov = [complete_coverage(300)];
    let modes = [modes(300)];
    let batch = EvidenceBatch {
        observations: &observations,
        usage: &usage,
        settlement: Some(&settled),
        coverage: &cov,
        modes: &modes,
    };
    assert!(assemble(&db, &batch, &clock(300)).is_err());
    db.execute_batch("BEGIN").unwrap();
    let out = assemble(&db, &batch, &clock(300)).unwrap();
    super::apply(&db, &out).unwrap();
    db.execute_batch("ROLLBACK").unwrap();
    let count: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM settings WHERE key LIKE 'subscriptionHoldout:%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    assert!(
        history(&db)[1]["observation"]["buckets"][0]["primary"]["calibrationEvidence"].is_null()
    );
}

#[test]
fn large_input_is_rejected_before_unbounded_history_or_usage_work() {
    let db = memory();
    let p = store_native(&db, &native(1, 0, 10.0, 9000), "runtime-a", "period-a");
    let obs = load_observation(&db, db.last_insert_rowid(), Some(&p)).unwrap();
    let many = vec![obs; MAX_OBSERVATIONS + 1];
    db.execute_batch("BEGIN").unwrap();
    let out = assemble(
        &db,
        &EvidenceBatch {
            observations: &many,
            usage: &[],
            settlement: None,
            coverage: &[],
            modes: &[],
        },
        &clock(0),
    )
    .unwrap();
    db.execute_batch("COMMIT").unwrap();
    assert!(reason(&out, Reason::InvalidNumber));
    assert!(out.patches.is_empty());
}

#[test]
fn production_apply_keeps_native_facts_immutable_and_replays_do_not_extend_evidence() {
    let db = memory();
    let (proofs, usage) = two_unassembled(&db);
    let originals: Vec<_> = history(&db)
        .iter()
        .map(|r| r["observation"].clone())
        .collect();
    let out = run(&db, &proofs, &usage, 300, verified());
    let frozen = history(&db);
    assert!(frozen
        .iter()
        .zip(&originals)
        .all(|(receipt, original)| &receipt["observation"] == original));
    assert!(frozen[1]["observation"]["buckets"][0]["primary"]["calibrationEvidence"].is_null());
    assert!(
        frozen[1]["derivedCalibrationPatches"]["shared:primary"]["windowFields"]
            ["calibrationEvidence"]
            .is_object()
    );
    db.execute_batch("BEGIN").unwrap();
    super::apply(&db, &out).unwrap();
    db.execute_batch("COMMIT").unwrap();
    assert_eq!(history(&db), frozen);
    assert!(super::apply(&db, &out).is_err());
}

#[test]
fn contradictory_coverage_or_mode_proofs_do_not_depend_on_input_order() {
    for mode_conflict in [false, true] {
        for reverse in [false, true] {
            let db = memory();
            let (proofs, usage) = two_unassembled(&db);
            let obs = observations(&db, &proofs);
            let settled = settlement(&db, 300);
            let mut coverages = vec![complete_coverage(300)];
            let mut mode_proofs = vec![modes(300)];
            if mode_conflict {
                let mut other = modes(300);
                other.effort = ExplicitMode::Known("high".into());
                mode_proofs.push(other);
            } else {
                coverages.push(coverage(300, CoverageKind::KnownIncomplete));
            }
            if reverse {
                coverages.reverse();
                mode_proofs.reverse();
            }
            db.execute_batch("BEGIN").unwrap();
            let out = assemble(
                &db,
                &EvidenceBatch {
                    observations: &obs,
                    usage: &usage,
                    settlement: Some(&settled),
                    coverage: &coverages,
                    modes: &mode_proofs,
                },
                &clock(300),
            )
            .unwrap();
            db.execute_batch("COMMIT").unwrap();
            assert!(out.pairs.is_empty());
            assert!(reason(
                &out,
                if mode_conflict {
                    Reason::ProfileChanged
                } else {
                    Reason::ExternalUsageUncovered
                }
            ));
        }
    }
}

#[test]
fn production_apply_rejects_native_or_account_field_injection_and_evidence_renewal() {
    let db = memory();
    let (proofs, usage) = two_unassembled(&db);
    let mut out = run(&db, &proofs, &usage, 300, verified());
    let before = history(&db);
    out.patches[0].window_fields["usedPercent"] = json!(99.0);
    db.execute_batch("BEGIN").unwrap();
    assert!(super::apply(&db, &out).is_err());
    db.execute_batch("ROLLBACK").unwrap();
    assert_eq!(history(&db), before);
    out.patches[0]
        .window_fields
        .as_object_mut()
        .unwrap()
        .remove("usedPercent");
    out.patches[0].snapshot_fields["accountScope"] = json!("wrong-account");
    db.execute_batch("BEGIN").unwrap();
    assert!(super::apply(&db, &out).is_err());
    db.execute_batch("ROLLBACK").unwrap();
    assert_eq!(history(&db), before);
    out.patches[0]
        .snapshot_fields
        .as_object_mut()
        .unwrap()
        .remove("accountScope");
    out.patches.last_mut().unwrap().window_fields["calibrationEvidence"]["settledThrough"] =
        json!(seconds(at(900)));
    db.execute_batch("BEGIN").unwrap();
    assert!(super::apply(&db, &out).is_err());
    db.execute_batch("ROLLBACK").unwrap();
    assert_eq!(history(&db), before);
}

#[test]
fn changed_original_observation_cannot_reuse_its_derived_namespace_or_archive_capacity() {
    let db = memory();
    let (_, usage) = simple(&db);
    let mut receipts = history(&db);
    let mut receipt = receipts[1].clone();
    receipt["observation"]["buckets"][0]["primary"]["usedPercent"] = json!(21.0);
    assert!(hydrated_observation(&receipt).is_none());
    db.execute(
        "UPDATE account_snapshots SET data=?1 WHERE id=2",
        [receipt.to_string()],
    )
    .unwrap();
    receipts = history(&db);
    let current = hydrated_observation(receipts.last().unwrap()).unwrap();
    let cards =
        super::super::calibration_archive::cards(&db, "source-a", &receipts, Some(&current))
            .unwrap();
    assert_eq!(cards[0]["quality"], "invalidated");
    assert!(cards[0]["capacityLower"].is_null());
    // The same native mutation also breaks live pairing; hydration never hides
    // a changed vendor field behind an older derived proof.
    assert!(projected(&db, &usage, "runtime-a", 900)["calibrations"][0]["capacityLower"].is_null());
}

#[test]
fn compressed_real_rounds_preserve_native_metadata_without_duplicate_training_or_expanding_storage()
{
    let db = memory();
    let mut proofs = Vec::new();
    db.execute_batch("BEGIN").unwrap();
    for (sequence, offset) in [(1, 0), (2, 180)] {
        let proof = store_native_with_encoding(
            &db,
            &native(sequence, offset, 10.0, 9000),
            "runtime-a",
            "period-a",
            true,
        );
        proofs.push((db.last_insert_rowid(), proof));
    }
    db.execute_batch("COMMIT").unwrap();
    let duplicate = load_observation(&db, proofs[1].0, Some(&proofs[1].1)).unwrap();
    assert!(!duplicate.independent_training_sample);
    let out = run(&db, &proofs, &[], 180, verified());
    assert!(out.pairs.is_empty());
    let stored: Value = serde_json::from_str(
        &db.query_row(
            "SELECT data FROM account_snapshots WHERE id=?1",
            [proofs[1].0],
            |r| r.get::<_, String>(0),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(stored["subscriptionStorage"]["encoding"], "reference");
    assert!(
        stored.get("observation").is_none(),
        "apply must retain the encoded original body reference"
    );
    assert!(stored["derivedCalibrationPatches"]["shared:primary"].is_object());
    let resolved = super::super::observation_storage::resolve(&db, &stored).unwrap();
    let hydrated = hydrated_observation(&resolved).unwrap();
    assert_eq!(hydrated["observationSequence"], "2");
    assert_eq!(hydrated["buckets"][0]["primary"]["roundingStep"], 1.0);
    assert!(hydrated["buckets"][0]["primary"]["calibrationEvidence"].is_null());
    db.execute_batch("BEGIN").unwrap();
    let proof = store_native_with_encoding(
        &db,
        &native(3, 360, 20.0, 9000),
        "runtime-a",
        "period-a",
        true,
    );
    proofs.push((db.last_insert_rowid(), proof));
    db.execute_batch("COMMIT").unwrap();
    let usage = vec![response("after-duplicate", 270, 100_000)];
    write_response(&db, &usage[0]);
    let out = run(&db, &proofs, &usage, 360, verified());
    assert_eq!(out.pairs.len(), 1);
    assert_eq!(
        out.pairs[0].start.sequence, 1,
        "duplicate cannot replace the independent baseline"
    );
    assert_eq!(out.pairs[0].end.sequence, 3);
    assert_eq!(out.pairs[0].tokens, 100_000);
    db.execute("DELETE FROM account_snapshots WHERE id=?1", [proofs[0].0])
        .unwrap();
    assert_eq!(
        load_observation(&db, proofs[1].0, Some(&proofs[1].1))
            .unwrap_err()
            .detail,
        "receipt_storage_evidence_removed"
    );
}
