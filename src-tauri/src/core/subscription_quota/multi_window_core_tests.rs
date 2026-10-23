//! One vendor read containing two pools; proofs/settlements remain per window.
use super::*;

fn observe_both(rig: &mut Rig, sequence: u64, offset: i64, index: u64) -> Value {
    rig.clock.set(reading(offset));
    if index > 0 {
        rig.write_usage(offset - 90);
    }
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
    let used = [20.0 + index as f64 * 10.0, 20.0 + index as f64 * 5.0];
    let native = AccountSnapshot {
        status: "connected".into(),
        collected_at: Some(stamp.clone()),
        auth_mode: Some("chatgpt".into()),
        account_scope: Some("typed-fixture-account".into()),
        plan_type: Some("fixture-plan-is-not-rights-proof".into()),
        identity_epoch: "1".into(),
        identity_consistency: "proven".into(),
        identity_consistency_reason: "typed-fixture-only".into(),
        observation_sequence: Some(sequence.to_string()),
        observation_outcome: Some("success".into()),
        read_started_at: Some(stamp.clone()),
        read_finished_at: Some(stamp),
        account_read: Some(read.clone()),
        quota_read: Some(read),
        buckets: vec![QuotaBucket {
            id: "shared".into(),
            name: Some("two synthetic limits".into()),
            primary: Some(QuotaWindow {
                used_percent: used[0],
                window_duration_mins: Some(10080),
                resets_at: Some(at(1800).timestamp()),
            }),
            secondary: Some(QuotaWindow {
                used_percent: used[1],
                window_duration_mins: Some(10080),
                resets_at: Some(at(3600).timestamp()),
            }),
        }],
        ..Default::default()
    };
    let check = json!({"checkedAt":at(offset).to_rfc3339(),"discoveryComplete":true,"cancelled":false,"pendingFiles":0,"issues":0,"discoveryErrors":0});
    put_setting(
        &rig.core().db,
        &format!("lastSourceCheck:{}", rig.core().source_id),
        &check,
    )
    .unwrap();
    let revision = get_setting(&rig.core().db, "subscriptionRevision")
        .unwrap()
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let mut domains = Vec::new();
    for (i, kind) in ["primary", "secondary"].into_iter().enumerate() {
        let mut scope = rig.scope();
        scope.pool_id = format!("shared:{kind}");
        scope.window_instance_id = format!("two-window-{kind}-period");
        domains.push(Domain::from_scope(&scope).unwrap());
        rig.proofs.push(AdapterQuotaProof {
            adapter_id: "typed-fixture-adapter".into(),
            reference: "two-window-atomic-read".into(),
            native_digest: native_observation_digest(&native),
            scope,
            sequence,
            percent: PercentReading {
                used_percent: used[i],
                rounding_step: Some(1.0),
                saturated: false,
                saturation_headroom_bounded: false,
            },
            fact_time: Interval::exact(seconds(offset)).unwrap(),
            ttl_start: seconds(offset),
            clock_epoch: "typed-fixture-clock".into(),
            reset_time: Interval::exact(seconds(if i == 0 { 1800 } else { 3600 })).unwrap(),
            recovery: RecoverySemantics::FixedReset,
            stable_identity_reference: "typed-fixture-stable-identity".into(),
            entitlement_reference: "typed-fixture-rights-evidence".into(),
            rule_reference: "typed-fixture-rules-evidence".into(),
            period_reference: format!("official-{kind}-fixture-period"),
            period_stable_across_runtimes: true,
        });
    }
    let proofs: Vec<Vec<_>> = domains
        .iter()
        .map(|domain| {
            rig.proofs
                .iter()
                .filter(|p| p.scope.pool_id == domain.pool_id)
                .cloned()
                .collect()
        })
        .collect();
    let settlements: Vec<_> = domains
        .iter()
        .map(|domain| LedgerSettlement {
            domain: domain.clone(),
            revision,
            complete_interval: Interval::new(seconds(0), seconds(offset)).unwrap(),
            captured_at: seconds(offset),
            source_check_digest: source_check_digest(&check),
            metering_version: adapter::SEMANTICS.into(),
            billing_clock_epoch: "typed-fixture-clock".into(),
            billing_alignment_reference: "both-pools-charge-the-same-fixture-activity".into(),
        })
        .collect();
    let coverages: Vec<_> = domains
        .iter()
        .map(|domain| {
            vec![CoverageAttestation {
                domain: domain.clone(),
                interval: Interval::new(seconds(0), seconds(offset)).unwrap(),
                kind: CoverageKind::VerifiedComplete {
                    adapter_id: "typed-fixture-independent-coverage".into(),
                    reference: format!("independent-complete-{}", domain.pool_id),
                },
            }]
        })
        .collect();
    let modes: Vec<_> = domains
        .iter()
        .map(|domain| {
            vec![UsageModeAttestation {
                domain: domain.clone(),
                interval: Interval::new(seconds(0), seconds(offset)).unwrap(),
                effort: ExplicitMode::Known("medium".into()),
                speed: ExplicitMode::Known("normal".into()),
                adapter_id: "typed-fixture-modes".into(),
                reference: "same-recorded-model-modes".into(),
            }]
        })
        .collect();
    let batches: Vec<_> = (0..2)
        .map(|i| CollectorBatch {
            proofs: &proofs[i],
            settlement: Some(&settlements[i]),
            coverage: &coverages[i],
            modes: &modes[i],
            reset_transitions: &[],
        })
        .collect();
    let result = rig
        .core
        .as_mut()
        .unwrap()
        .settle_subscription_observation_with_evidence(1, json!(native), &batches)
        .unwrap();
    assert_eq!(result["persisted"], true, "{result}");
    assert!(result["persistenceError"].is_null(), "{result}");
    result
}

#[test]
fn same_official_round_learns_both_windows_and_keeps_their_native_owners_and_leases_separate() {
    let mut rig = Rig::new();
    rig.core
        .as_mut()
        .unwrap()
        .qualify_subscription_native_for_test();
    for i in 0..7 {
        let current = observe_both(&mut rig, i + 1, i as i64 * 180, i);
        if i > 0 {
            assert_eq!(
                current["calibrationAssembly"]["assembledPairs"], 2,
                "each window receives its own newly settled pair: {current}"
            );
        }
    }
    let before = rig.core().db.total_changes();
    let overview = rig.overview();
    assert_eq!(rig.core().db.total_changes(), before);
    assert_eq!(overview["windows"].as_array().unwrap().len(), 2);
    let cards: Vec<_> = overview["calibrations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["archived"] != true)
        .collect();
    assert_eq!(cards.len(), 2, "{overview}");
    for (id, lower, upper) in [
        ("shared:primary", 900_000.0, 1_120_000.0),
        ("shared:secondary", 1_600_000.0, 2_600_000.0),
    ] {
        let card = cards.iter().find(|c| c["windowId"] == id).unwrap();
        assert_eq!(card["sampleCount"], 6);
        assert_eq!(card["quality"], "conditional");
        assert!(card["capacityLower"].as_f64().unwrap() > lower);
        assert!(card["capacityUpper"].as_f64().unwrap() < upper);
    }
    let receipt = rig.receipts().pop().unwrap();
    assert!(receipt["derivedCalibrationPatches"]["shared:primary"].is_object());
    assert!(receipt["derivedCalibrationPatches"]["shared:secondary"].is_object());
    let mut claims = rig.core.as_mut().unwrap().claim_notifications().unwrap();
    let secondary = rig
        .risk_events()
        .into_iter()
        .find(|e| e["windowId"] == "shared:secondary")
        .unwrap();
    let forecast: super::super::model::NativeForecast =
        serde_json::from_value(overview["windows"][1]["risk"]["evidence"].clone()).unwrap();
    let serialized = serde_json::to_string(&forecast).unwrap();
    let mut direct = json!(forecast);
    direct
        .as_object_mut()
        .unwrap()
        .remove("inputObservationIds");
    let mut persisted: Value = serde_json::from_str(&serialized).unwrap();
    persisted
        .as_object_mut()
        .unwrap()
        .remove("inputObservationIds");
    assert_eq!(
        secondary["evidence"], direct,
        "persisted prediction must preserve exact computation values"
    );
    assert_eq!(secondary["evidence"], persisted);
    assert_eq!(
        secondary["forecastInputDigest"],
        crate::core::hash(serialized.as_bytes())
    );

    claims.sort_by_key(|event| event["windowId"].as_str().unwrap().to_string());
    assert_eq!(claims.len(), 2, "{claims:?}");
    assert_ne!(claims[0]["ownerId"], claims[1]["ownerId"]);
    assert!(claims
        .iter()
        .all(|e| e["ownerKind"] == "quota_watch" && e["planId"].is_null()));
    let first = rig.core().notification_permit(&claims[0]).unwrap();
    let second = rig.core().notification_permit(&claims[1]).unwrap();
    rig.core
        .as_mut()
        .unwrap()
        .finish_notification(
            &[claims[0]["id"].as_str().unwrap().into()],
            "failed",
            Some("synthetic COM error"),
        )
        .unwrap();
    assert!(!first.is_current());
    assert!(second.is_current());
    assert!(rig
        .core()
        .domain_prompt_is_current("quota_eta", claims[0]["id"].as_str().unwrap()));
    assert!(rig
        .core()
        .domain_prompt_is_current("quota_eta", claims[1]["id"].as_str().unwrap()));
    rig.reopen();
    assert!(!second.is_current());
    assert!(rig
        .core
        .as_mut()
        .unwrap()
        .claim_notifications()
        .unwrap()
        .is_empty());
    assert!(rig.overview()["windows"].as_array().unwrap().is_empty());
}
