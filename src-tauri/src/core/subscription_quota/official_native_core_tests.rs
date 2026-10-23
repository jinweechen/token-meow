//! Real Core/SQLite with synthetic typed provider proofs; no OS qualification.
use super::*;
use crate::budget_clock;
use crate::notification_gate::{self, QueuedNotification};

fn target_rig(qualified: bool) -> (Rig, Value) {
    target_at(qualified, Some(1.0), 240)
}
fn target_at(qualified: bool, quantum: Option<f64>, final_offset: i64) -> (Rig, Value) {
    let mut rig = Rig::new();
    rig.rounding_step = quantum;
    if qualified {
        rig.core
            .as_mut()
            .unwrap()
            .qualify_subscription_native_for_test();
    }
    rig.observe(1, 0, 60.0, false);
    let source = rig.core().source_id.clone();
    let input = json!({"expectedRevision":null,"title":"synthetic official target","type":"percent",
        "sourceId":source,"model":null,"modelDimension":"response","target":80.0,
        "endPolicy":"window","endsAt":at(1800).to_rfc3339(),"windowId":"shared:primary","remindersEnabled":true});
    rig.core.as_mut().unwrap().save_usage_plan(&input).unwrap();
    rig.observe(2, 120, 70.0, false);
    let cursor = rig.core().pet_pulse(None).unwrap()["promptFeed"]["cursor"].clone();
    if final_offset >= 0 {
        rig.observe(3, final_offset, 80.0, false);
    }
    (rig, cursor)
}
fn risk_rig(qualified: bool) -> (Rig, Value) {
    risk_quantum(qualified, Some(1.0))
}
fn risk_quantum(qualified: bool, quantum: Option<f64>) -> (Rig, Value) {
    let mut rig = Rig::new();
    rig.rounding_step = quantum;
    if qualified {
        rig.core
            .as_mut()
            .unwrap()
            .qualify_subscription_native_for_test();
    }
    for i in 0..6 {
        rig.observe(i + 1, i as i64 * 180, 20.0 + i as f64 * 10.0, i > 0);
    }
    let cursor = rig.core().pet_pulse(None).unwrap()["promptFeed"]["cursor"].clone();
    rig.observe(7, 1080, 80.0, true);
    (rig, cursor)
}
fn events(rig: &Rig) -> Vec<Value> {
    rig.core()
        .db
        .prepare("SELECT data FROM inbox_events ORDER BY rowid")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
        .collect()
}
fn claim(rig: &mut Rig) -> (Value, crate::notification_gate::NotificationPermit) {
    let claimed = rig.core.as_mut().unwrap().claim_notifications().unwrap();
    assert_eq!(claimed.len(), 1, "events={:?}", events(rig));
    let event = claimed[0].clone();
    let permit = rig
        .core()
        .notification_permit(&event)
        .expect("new committed official transition must have a RAM permit");
    (event, permit)
}

#[test]
fn official_target_commits_then_enters_claim_queue_and_independent_pet_channel() {
    let (mut rig, cursor) = target_rig(true);
    let (event, toast) = claim(&mut rig);
    assert_eq!(event["kind"], "subscription_plan");
    assert_eq!(event["reminderKind"], "official_target");
    assert_eq!(event["officialTargetPercent"], 80.0);
    assert_eq!(event["inputObservationSequences"], json!(["2", "3"]));
    let mut queue = Vec::new();
    let admitted = notification_gate::admit_notifications(
        &mut queue,
        vec![QueuedNotification {
            event: event.clone(),
            permit: Some(toast.clone()),
        }],
        &reading(240),
    );
    assert!(admitted.suppressed.is_empty());
    assert_eq!(queue.len(), 1);
    rig.core
        .as_mut()
        .unwrap()
        .finish_notification(
            &[event["id"].as_str().unwrap().into()],
            "failed",
            Some("synthetic COM failure"),
        )
        .unwrap();
    assert!(!toast.is_current());
    rig.clock.set(reading(243));
    let pulse = rig.core().pet_pulse(Some(cursor)).unwrap();
    assert_eq!(pulse["promptFeed"]["baseline"], false);
    assert_eq!(pulse["prompts"].as_array().unwrap().len(), 1, "{pulse}");
    assert_eq!(pulse["prompts"][0]["id"], event["id"]);
    assert!(rig
        .core()
        .domain_prompt_is_current("subscription_plan", event["id"].as_str().unwrap()));
    let next = rig
        .core()
        .pet_pulse(Some(pulse["promptFeed"]["cursor"].clone()))
        .unwrap();
    assert!(next["prompts"].as_array().unwrap().is_empty());
}

#[test]
fn risk_without_user_plan_uses_two_real_rounds_and_its_own_owner_permit() {
    let (mut rig, cursor) = risk_rig(true);
    assert_eq!(
        rig.core()
            .db
            .query_row("SELECT COUNT(*) FROM usage_plans", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let (event, toast) = claim(&mut rig);
    assert_eq!(event["kind"], "quota_eta");
    assert_eq!(event["ownerKind"], "quota_watch");
    assert_eq!(event["inputObservationSequences"], json!(["6", "7"]));
    assert!(event["planId"].is_null());
    assert_eq!(event["forecastInputSequences"].as_array().unwrap().len(), 7);
    rig.core
        .as_mut()
        .unwrap()
        .finish_notification(&[event["id"].as_str().unwrap().into()], "sent", None)
        .unwrap();
    assert!(!toast.is_current());
    rig.clock.set(reading(1083));
    let pulse = rig.core().pet_pulse(Some(cursor)).unwrap();
    assert_eq!(pulse["prompts"][0]["kind"], "quota_eta", "{pulse}");
    assert!(rig
        .core
        .as_mut()
        .unwrap()
        .claim_notifications()
        .unwrap()
        .is_empty());
}

#[test]
fn official_pet_acknowledgement_keeps_the_queued_toast_but_cannot_be_replayed() {
    let (mut rig, cursor) = target_rig(true);
    let (event, toast) = claim(&mut rig);
    let pulse = rig.core().pet_pulse(Some(cursor)).unwrap();
    assert_eq!(pulse["prompts"][0]["id"], event["id"]);
    rig.core
        .as_mut()
        .unwrap()
        .action("acknowledge_delivery", &json!({"id":event["id"]}))
        .unwrap();
    assert!(toast.is_current());
    assert!(rig.core().notification_permit(&event).is_some());
    assert!(!rig
        .core()
        .domain_prompt_is_current("subscription_plan", event["id"].as_str().unwrap()));
    rig.core()
        .db
        .execute(
            "UPDATE inbox_events SET data=json_set(data,'$.animationAcknowledged',json('false'))",
            [],
        )
        .unwrap();
    assert!(
        !rig.core()
            .domain_prompt_is_current("subscription_plan", event["id"].as_str().unwrap()),
        "DB rollback cannot revive a consumed prompt channel"
    );
}

#[test]
fn official_permit_expires_during_preparation_without_submitting_or_renewing() {
    let (mut rig, _) = target_rig(true);
    let (event, permit) = claim(&mut rig);
    let submitted = std::cell::Cell::new(false);
    let queued = QueuedNotification {
        event: event.clone(),
        permit: Some(permit.clone()),
    };
    let delivery = notification_gate::deliver_batch(
        &[queued],
        |_| {
            rig.clock.set(reading(255));
            Ok(())
        },
        |_| {
            submitted.set(true);
            Ok(())
        },
        || true,
        budget_clock::now,
    );
    assert!(!submitted.get());
    assert_eq!(
        delivery.suppressed,
        vec![event["id"].as_str().unwrap().to_string()]
    );
    assert!(!permit.is_current());
    assert!(rig.core().notification_permit(&event).is_none());
}

#[test]
fn official_host_aba_clock_skew_and_unknown_current_revoke_queued_clones() {
    for boundary in ["host", "clock", "unavailable"] {
        let (mut rig, _) = target_rig(true);
        let (event, permit) = claim(&mut rig);
        match boundary {
            "host" => {
                rig.core().notification_gate.publish(false, true);
                rig.core().notification_gate.publish(true, false);
            }
            "clock" => rig.clock.set(Reading {
                utc: at(241),
                mono_ms: reading(245).mono_ms,
            }),
            "unavailable" => rig.core.as_mut().unwrap().subscription_unavailable(),
            _ => unreachable!(),
        }
        assert!(
            rig.core().notification_permit(&event).is_none(),
            "{boundary}"
        );
        assert!(!permit.is_current(), "{boundary}");
    }
}

#[test]
fn official_receipt_correction_missing_base_and_plan_revision_close_delivery() {
    for correction in ["change", "remove", "plan"] {
        let (mut rig, _) = target_rig(true);
        let (event, permit) = claim(&mut rig);
        match correction {
            "change" => {
                rig.core().db.execute("UPDATE account_snapshots SET data=json_set(data,'$.observation.buckets[0].primary.usedPercent',69) WHERE json_extract(data,'$.observationSequence')='2'",[]).unwrap();
            }
            "remove" => {
                rig.core().db.execute("DELETE FROM account_snapshots WHERE json_extract(data,'$.observationSequence')='2'",[]).unwrap();
            }
            "plan" => {
                rig.core()
                    .db
                    .execute(
                        "UPDATE usage_plans SET data=json_set(data,'$.revision',2)",
                        [],
                    )
                    .unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            rig.core().notification_permit(&event).is_none(),
            "{correction}"
        );
        assert!(!permit.is_current());
    }
}

#[test]
fn native_official_events_cannot_be_forged_by_kind_threshold_or_identity_payload() {
    let (mut rig, _) = target_rig(true);
    let (event, permit) = claim(&mut rig);
    for (key, value) in [
        ("kind", json!("quota_eta")),
        ("reminderKind", json!("native_quota_eta")),
        ("planRevision", json!(8)),
        ("officialTargetPercent", json!(90)),
        ("eligibleRuntime", json!("other")),
    ] {
        let mut forged = event.clone();
        forged[key] = value;
        assert!(rig.core().notification_permit(&forged).is_none(), "{key}");
    }
    assert!(
        permit.is_current(),
        "a bad payload cannot take authority from the valid event"
    );
    assert!(rig.core().notification_permit(&event).is_some());
}

#[test]
fn production_gate_and_restart_never_recreate_native_authority_from_panel_history() {
    for risk in [false, true] {
        let (mut rig, _) = if risk {
            risk_rig(false)
        } else {
            target_rig(false)
        };
        let event = events(&rig).pop().unwrap();
        assert_eq!(event["notifyEligible"], false);
        rig.core
            .as_mut()
            .unwrap()
            .qualify_subscription_native_for_test();
        assert!(rig.core().notification_permit(&event).is_none());
        assert!(rig
            .core
            .as_mut()
            .unwrap()
            .claim_notifications()
            .unwrap()
            .is_empty());
        rig.reopen();
        assert!(rig.core().notification_permit(&event).is_none());
    }
}

#[test]
fn new_official_round_revokes_old_authority_and_duplicate_does_not_renew_lease() {
    let (mut rig, _) = target_rig(true);
    let (event, permit) = claim(&mut rig);
    rig.observe(3, 249, 80.0, false);
    assert!(permit.is_current());
    rig.clock.set(reading(255));
    assert!(!permit.is_current());
    assert!(rig.core().notification_permit(&event).is_none());
    let (mut rig, _) = target_rig(true);
    let (event, permit) = claim(&mut rig);
    rig.observe(4, 245, 81.0, false);
    assert!(!permit.is_current());
    assert!(rig.core().notification_permit(&event).is_none());
    assert_eq!(
        events(&rig).len(),
        1,
        "same window target is already latched"
    );
}

#[test]
fn official_confirmation_ttl_can_expire_before_the_fifteen_second_lease() {
    let (mut rig, _) = target_at(true, Some(1.0), 359);
    let (event, permit) = claim(&mut rig);
    assert_eq!(
        permit.expires_utc(),
        at(360),
        "the older confirmation expires one second after this transition"
    );
    rig.clock.set(reading(360));
    assert!(!permit.is_current());
    assert!(rig.core().notification_permit(&event).is_none());
}

#[test]
fn unknown_precision_allows_displayed_official_target_but_cannot_authorize_eta() {
    let (mut target, _) = target_at(true, None, 240);
    let (event, permit) = claim(&mut target);
    assert_eq!(event["reminderKind"], "official_target");
    assert!(permit.is_current());
    drop(target);
    let (mut risk, _) = risk_quantum(true, None);
    assert!(events(&risk).is_empty());
    assert!(risk
        .core
        .as_mut()
        .unwrap()
        .claim_notifications()
        .unwrap()
        .is_empty());
    assert_eq!(
        risk.overview()["windows"][0]["risk"]["notifyCandidate"],
        false
    );
}

#[test]
fn correcting_an_old_rate_baseline_revokes_risk_even_when_current_percent_is_unchanged() {
    let (mut rig, _) = risk_rig(true);
    let (event, permit) = claim(&mut rig);
    rig.core().db.execute("UPDATE account_snapshots SET data=json_set(data,'$.observation.buckets[0].primary.usedPercent',21) WHERE json_extract(data,'$.observationSequence')='1'",[]).unwrap();
    assert!(rig.core().notification_permit(&event).is_none());
    assert!(!permit.is_current());
}

#[test]
fn changing_persisted_forecast_evidence_cannot_borrow_the_unchanged_input_digest() {
    let (mut rig, _) = risk_rig(true);
    let (event, permit) = claim(&mut rig);
    let mut changed = event.clone();
    let upper = changed["evidence"]["exhaustionTime"]["upper"]
        .as_f64()
        .unwrap();
    changed["evidence"]["exhaustionTime"]["upper"] = json!(upper + 1.0);
    assert_eq!(changed["forecastInputDigest"], event["forecastInputDigest"]);
    rig.core()
        .db
        .execute(
            "UPDATE inbox_events SET data=?1 WHERE id=?2",
            rusqlite::params![changed.to_string(), event["id"].as_str()],
        )
        .unwrap();
    assert!(rig.core().notification_permit(&event).is_none());
    assert!(
        !permit.is_current(),
        "the persisted evidence remains part of the exact immutable binding"
    );
}

#[test]
fn failed_event_transaction_rolls_back_receipt_and_cannot_issue_or_replay_a_permit() {
    let (mut rig, _) = target_at(true, Some(1.0), -1);
    rig.clock.set(reading(240));
    let stamp = at(240).to_rfc3339();
    let read = AccountSubRead {
        request_started_at: stamp.clone(),
        response_received_at: stamp.clone(),
        freshness_anchor: Some(stamp.clone()),
        fact_time_lower: Some(stamp.clone()),
        fact_time_upper: Some(stamp.clone()),
        time_basis: "verified_on_demand_bounds".into(),
        outcome: "success".into(),
    };
    // AccountSnapshot intentionally has no Deserialize: construct the same
    // typed synthetic adapter input rather than reopening a JSON trust path.
    let native = AccountSnapshot {
        status: "connected".into(),
        collected_at: Some(stamp.clone()),
        auth_mode: Some("chatgpt".into()),
        account_scope: Some("typed-fixture-account".into()),
        plan_type: Some("fixture-plan-is-not-rights-proof".into()),
        identity_epoch: "1".into(),
        identity_consistency: "proven".into(),
        identity_consistency_reason: "typed-fixture-only".into(),
        observation_sequence: Some("3".into()),
        observation_outcome: Some("success".into()),
        read_started_at: Some(stamp.clone()),
        read_finished_at: Some(stamp),
        account_read: Some(read.clone()),
        quota_read: Some(read),
        buckets: vec![QuotaBucket {
            id: "shared".into(),
            name: Some("fixture pool".into()),
            primary: Some(QuotaWindow {
                used_percent: 80.0,
                window_duration_mins: Some(10080),
                resets_at: Some(at(1800).timestamp()),
            }),
            secondary: None,
        }],
        ..Default::default()
    };
    let mut proof = rig.proofs.last().unwrap().clone();
    proof.sequence = 3;
    proof.native_digest = native_observation_digest(&native);
    proof.percent.used_percent = 80.0;
    proof.fact_time = Interval::exact(seconds(240)).unwrap();
    proof.ttl_start = seconds(240);
    let mut proofs = rig.proofs.clone();
    proofs.push(proof);
    let batches = [CollectorBatch {
        proofs: &proofs,
        settlement: None,
        coverage: &[],
        modes: &[],
        reset_transitions: &[],
    }];
    let carrier = json!(native);
    rig.core().db.execute_batch("CREATE TEMP TRIGGER reject_official_event BEFORE INSERT ON inbox_events BEGIN SELECT RAISE(ABORT,'synthetic rollback'); END;").unwrap();
    let failed = rig
        .core
        .as_mut()
        .unwrap()
        .settle_subscription_observation_with_evidence(1, carrier.clone(), &batches)
        .unwrap();
    assert_eq!(failed["persisted"], false);
    assert_eq!(failed["persistenceError"], "observation_persistence_failed");
    assert!(events(&rig).is_empty());
    assert_eq!(
        rig.core()
            .db
            .query_row("SELECT COUNT(*) FROM account_snapshots", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    rig.core()
        .db
        .execute_batch("DROP TRIGGER reject_official_event;")
        .unwrap();
    let replay = rig
        .core
        .as_mut()
        .unwrap()
        .settle_subscription_observation_with_evidence(1, carrier, &batches)
        .unwrap();
    assert_eq!(
        replay["persisted"], false,
        "RAM sequence watermark must not roll back with SQLite"
    );
    assert!(rig
        .core
        .as_mut()
        .unwrap()
        .claim_notifications()
        .unwrap()
        .is_empty());
    rig.observe(4, 360, 81.0, false);
    assert!(
        events(&rig).is_empty(),
        "recovery records a new baseline rather than replaying the failed crossing"
    );
}
