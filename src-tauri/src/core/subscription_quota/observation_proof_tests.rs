use super::super::super::model::RecoverySemantics;
use super::*;
use crate::account_bridge::{AccountSnapshot, AccountSubRead, QuotaBucket, QuotaWindow};
use chrono::{DateTime, Duration, Utc};

fn at(n: i64) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-22T08:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
        + Duration::seconds(n)
}
fn seconds(n: i64) -> f64 {
    at(n).timestamp() as f64
}
fn clock(n: i64) -> Reading {
    Reading {
        utc: at(n),
        mono_ms: 1000 + n as u64 * 1000,
    }
}
fn fixture(sequence: u64, n: i64) -> (Value, AdapterQuotaProof) {
    let stamp = at(n).to_rfc3339();
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
        account_scope: Some("fixture-account".into()),
        identity_epoch: "1".into(),
        identity_consistency: "proven".into(),
        identity_consistency_reason: "synthetic fixture only".into(),
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
                used_percent: 60.0,
                window_duration_mins: Some(300),
                resets_at: Some(at(1800).timestamp()),
            }),
            secondary: None,
        }],
        ..Default::default()
    };
    let digest = super::super::native_observation_digest(&native);
    let mut value = serde_json::to_value(native).unwrap();
    value["runtimeId"] = json!("fixture-runtime");
    value["sourceId"] = json!("fixture-source");
    value["bindingGeneration"] = json!("1");
    value["clearGeneration"] = Value::Null;
    let proof = AdapterQuotaProof {
        adapter_id: "fixture-adapter".into(),
        reference: "synthetic-only".into(),
        native_digest: digest,
        scope: EvaluationScope {
            runtime_id: "fixture-runtime".into(),
            source_id: "fixture-source".into(),
            binding_generation: 1,
            identity_epoch: 1,
            clear_generation: "null".into(),
            account_scope: Some("fixture-account".into()),
            entitlement_segment: "fixture-rights".into(),
            rule_version: "fixture-rule".into(),
            pool_id: "shared:primary".into(),
            window_instance_id: "fixture-period".into(),
        },
        sequence,
        percent: PercentReading {
            used_percent: 60.0,
            rounding_step: Some(1.0),
            saturated: false,
            saturation_headroom_bounded: false,
        },
        fact_time: Interval::exact(seconds(n)).unwrap(),
        ttl_start: seconds(n),
        clock_epoch: "fixture-clock".into(),
        reset_time: Interval::exact(seconds(1800)).unwrap(),
        recovery: RecoverySemantics::FixedReset,
        stable_identity_reference: "fixture-stable-identity".into(),
        entitlement_reference: "fixture-rights-proof".into(),
        rule_reference: "fixture-rule-proof".into(),
        period_reference: "fixture-period-proof".into(),
        period_stable_across_runtimes: true,
    };
    (value, proof)
}
fn receipt(value: &Value) -> Value {
    let mut r = json!({"documentKind":"subscription_observation","schemaVersion":1,"outcome":"success","lastPersistedSequence":value["observationSequence"],"observation":value});
    for key in [
        "runtimeId",
        "sourceId",
        "bindingGeneration",
        "identityEpoch",
        "clearGeneration",
        "observationSequence",
    ] {
        r[key] = value[key].clone();
    }
    r
}

#[test]
fn typed_native_dto_admits_without_json_self_attestation_and_seals_original_body() {
    let (mut value, proof) = fixture(1, 0);
    let original = value.clone();
    assert!(value.get("identityEvidence").is_none());
    assert_eq!(
        admit(&mut value, None, &[&proof], &[], &clock(0))["status"],
        "admitted"
    );
    let mut r = receipt(&value);
    seal_receipt(&mut r).unwrap();
    assert!(r["observation"].get("identityEvidence").is_none());
    assert!(r["observation"]["buckets"][0]["primary"]
        .get("windowInstanceId")
        .is_none());
    assert_eq!(
        native_snapshot_digest(&r["observation"]),
        native_snapshot_digest(&original)
    );
    let hydrated = super::super::hydrated_observation(&r).unwrap();
    assert_eq!(
        hydrated["identityEvidence"]["reference"],
        proof.stable_identity_reference
    );
    assert_eq!(
        hydrated["buckets"][0]["primary"]["windowInstanceId"],
        proof.scope.window_instance_id
    );
    assert_eq!(hydrated["persisted"], true);
}

#[test]
fn no_proof_or_any_digest_scope_sequence_or_read_mismatch_cannot_keep_payload_metadata() {
    let (original, proof) = fixture(1, 0);
    for field in [
        "missing",
        "digest",
        "scope",
        "sequence",
        "percent",
        "time",
        "provenance",
    ] {
        let mut value = original.clone();
        value["identityEvidence"] = json!({"verified":true});
        value["buckets"][0]["primary"]["resetEvidence"] = json!({"confirmedNewWindow":true});
        value["buckets"][0]["primary"]["usageStructureEvidence"] = json!({"verified":true});
        let mut p = proof.clone();
        match field {
            "digest" => p.native_digest = "wrong".into(),
            "scope" => p.scope.source_id = "other".into(),
            "sequence" => p.sequence = 2,
            "percent" => p.percent.used_percent = 61.0,
            "time" => p.fact_time = Interval::exact(seconds(1)).unwrap(),
            "provenance" => p.reference.clear(),
            _ => {}
        }
        let proofs = if field == "missing" { vec![] } else { vec![&p] };
        assert_eq!(
            admit(&mut value, None, &proofs, &[], &clock(0))["status"],
            "unavailable",
            "{field}"
        );
        assert!(value.get("identityEvidence").is_none(), "{field}");
        assert!(
            value["buckets"][0]["primary"]
                .get("resetEvidence")
                .is_none(),
            "{field}"
        );
        assert!(
            value["buckets"][0]["primary"]
                .get("usageStructureEvidence")
                .is_none(),
            "{field}"
        );
    }
}

#[test]
fn sealed_namespace_is_bound_to_original_scope_native_digest_and_metadata() {
    let (mut value, proof) = fixture(1, 0);
    admit(&mut value, None, &[&proof], &[], &clock(0));
    let mut original = receipt(&value);
    seal_receipt(&mut original).unwrap();
    for field in ["native", "scope", "metadata", "version"] {
        let mut r = original.clone();
        match field {
            "native" => r["observation"]["buckets"][0]["primary"]["usedPercent"] = json!(70),
            "scope" => {
                r["runtimeId"] = json!("other");
                r["observation"]["runtimeId"] = json!("other");
            }
            "metadata" => {
                r[NAMESPACE]["snapshotFields"]["identityEvidence"]["reference"] = json!("changed")
            }
            _ => r[NAMESPACE]["schemaVersion"] = json!(2),
        }
        assert!(super::super::hydrated_observation(&r).is_none(), "{field}");
    }
}

#[test]
fn nonzero_bounded_fact_time_can_admit_native_metadata_without_inventing_exact_billing_pairs() {
    let (mut value, mut proof) = fixture(1, 1);
    value["readStartedAt"] = json!(at(0).to_rfc3339());
    value["quotaRead"]["requestStartedAt"] = json!(at(0).to_rfc3339());
    value["quotaRead"]["factTimeLower"] = json!(at(0).to_rfc3339());
    proof.fact_time = Interval::new(seconds(0), seconds(1)).unwrap();
    proof.native_digest = native_snapshot_digest(&value);
    assert_eq!(
        admit(&mut value, None, &[&proof], &[], &clock(1))["status"],
        "admitted"
    );
    assert_ne!(
        value["quotaRead"]["factTimeLower"],
        value["quotaRead"]["factTimeUpper"]
    );
}

#[test]
fn oversized_typed_metadata_closes_admission_before_storage_without_losing_native_readout() {
    let (mut value, proof) = fixture(1, 0);
    value["error"] = json!("x".repeat(460 * 1024));
    let result = admit(&mut value, None, &[&proof], &[], &clock(0));
    assert_eq!(
        result["reasonCodes"],
        json!(["observation_proof_payload_limit"])
    );
    assert!(value.get("identityEvidence").is_none());
    assert_eq!(value["buckets"][0]["primary"]["usedPercent"], 60.0);
    assert!(value.get(NAMESPACE).is_none());
}

#[test]
fn reset_transition_requires_the_real_previous_endpoint_and_official_contract() {
    let (mut previous, old) = fixture(1, 0);
    admit(&mut previous, None, &[&old], &[], &clock(0));
    previous["persisted"] = json!(true);
    let (mut current, native) = fixture(2, 10);
    let reset = AdapterResetTransitionProof {
        adapter_id: "fixture-reset".into(),
        reference: "official-fixture-only".into(),
        scope: native.scope.clone(),
        sequence: 2,
        native_digest: native.native_digest.clone(),
        previous_sequence: 1,
        previous_native_digest: old.native_digest.clone(),
        previous_window_instance_id: old.scope.window_instance_id.clone(),
        current_window_instance_id: native.scope.window_instance_id.clone(),
        transition: ResetTransition::SameWindow,
        next_reset: Some(native.reset_time),
        actual_boundary: None,
        final_used_percent: None,
    };
    let mut wrong = reset.clone();
    wrong.previous_native_digest = "wrong".into();
    assert_eq!(
        admit(
            &mut current,
            Some(&previous),
            &[&native],
            &[&wrong],
            &clock(10)
        )["status"],
        "unavailable"
    );
    assert!(current.get("identityEvidence").is_none());
    assert_eq!(
        admit(
            &mut current,
            Some(&previous),
            &[&native],
            &[&reset],
            &clock(10)
        )["status"],
        "admitted"
    );
    assert_eq!(
        current["buckets"][0]["primary"]["resetEvidence"]["confirmedSameWindow"],
        true
    );
}

#[test]
fn a_well_formed_calibration_namespace_cannot_override_different_admitted_identity_metadata() {
    let (mut value, proof) = fixture(1, 0);
    admit(&mut value, None, &[&proof], &[], &clock(0));
    let mut r = receipt(&value);
    seal_receipt(&mut r).unwrap();
    let mut fields = r[NAMESPACE]["snapshotFields"].clone();
    fields["identityEvidence"]["reference"] = json!("different-stable-identity");
    let mut patch = json!({"schemaVersion":1,"algorithmVersion":super::super::VERSION,
        "observationId":"fixture-runtime:1:shared:primary","windowId":"shared:primary","nativeDigest":proof.native_digest,
        "snapshotFields":fields,"windowFields":r[NAMESPACE]["windowFields"]["shared:primary"]});
    let digest = canonical_digest(&patch);
    patch["metadataDigest"] = json!(digest);
    r["derivedCalibrationPatches"] = json!({"shared:primary":patch});
    assert!(super::super::hydrated_observation(&r).is_none());
}
