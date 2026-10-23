use super::*;
use chrono::Duration;

fn at() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-22T10:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}
fn snapshot() -> Value {
    let mut s = json!({"runtimeId":"runtime-a","sourceId":"source-a","bindingGeneration":"1","identityEpoch":"1","clearGeneration":null,"accountScope":"account-a","observationSequence":"1",
        "identityConsistency":"proven","identityEvidence":{"kind":"synthetic_fixture_only"},"observationOutcome":"success","status":"connected","persisted":true,
        "accountRead":{"outcome":"success","freshnessAnchor":at().to_rfc3339()},"quotaRead":{"outcome":"success","freshnessAnchor":at().to_rfc3339()}});
    s["entitlementSegment"] = json!("rights-a");
    s["entitlementEvidence"] = json!({"verified":true,"source":"official","segmentId":"rights-a","ruleVersion":"rights-rule-a","reference":"synthetic_fixture_only"});
    let mut w = json!({"windowInstanceId":"period-a","ruleVersion":"window-rule-a","recovery":"fixed_reset","resetsAt":(at()+Duration::hours(1)).timestamp(),"usedPercent":60,"roundingStep":1.0,"saturated":false,"saturationHeadroomBounded":false});
    w["windowInstanceEvidence"] = json!({"verified":true,"source":"official","windowInstanceId":"period-a","reference":"synthetic_fixture_only"});
    w["usageStructureEvidence"] = json!({"verified":true,"source":"trusted-ledger","reference":"synthetic_fixture_only","sourceId":"source-a","accountScope":"account-a","windowId":"shared:primary","windowInstanceId":"period-a","model":"model-a","modelDimension":"response",
        "profile":{"models":[{"model":"model-a","share":1.0}],"allModelsAttributed":true,"effort":"medium","speedMode":"normal","outputShare":0.2,"cacheShare":0.3,"medianInputTokens":1000}});
    s["buckets"] = json!([{"id":"shared","primary":w}]);
    s
}
fn assumption(s: &Value) -> Value {
    json!({"id":"assumption-a","revision":1,"sourceId":"source-a","windowId":"shared:primary","model":"model-a","capacityTokens":2000000,"effectiveFrom":at().to_rfc3339(),"validUntil":(at()+Duration::hours(2)).to_rfc3339(),"note":"手动条件；synthetic fixture only","provenance":"manual-assumption","quality":"conditional","manualScope":freeze(Some(s),"source-a","shared:primary",Some("model-a"),at())})
}
fn project(a: &Value, s: &Value) -> Value {
    scenario(a, Some(s), "source-a", at() + Duration::seconds(1))
}

#[test]
fn explicit_manual_scenario_propagates_percent_quantum_without_upgrading_calibration() {
    let s = snapshot();
    let a = assumption(&s);
    let p = project(&a, &s);
    assert_eq!(p["active"], true);
    assert_eq!(p["remainingEstimate"], 800000.0);
    assert_eq!(p["remainingLower"], 790000.0);
    assert_eq!(p["remainingUpper"], 810000.0);
    assert_eq!(p["quality"], "conditional");
    assert_eq!(p["provenance"], "manual-assumption");
    assert_eq!(p["constraintsVerified"], false);
    assert_eq!(p["notifyEligible"], false);
    assert_eq!(p["mutuallyExclusive"], true);
    assert!(p.get("calibrationVersion").is_none());
    assert!(p.get("plan").is_none());
    assert_eq!(a, assumption(&s));
}

#[test]
fn unproven_or_legacy_premise_can_be_saved_but_cannot_acquire_scope_later() {
    let s = snapshot();
    let mut a = assumption(&s);
    a.as_object_mut().unwrap().remove("manualScope");
    let p = project(&a, &s);
    assert_eq!(p["active"], false);
    assert_eq!(p["reasons"], json!(["manual_scope_unverified"]));
    assert!(p["remainingEstimate"].is_null());
    a["manualScope"] = freeze(None, "source-a", "shared:primary", Some("model-a"), at());
    assert!(a["manualScope"]["scope"].is_null());
    assert_eq!(project(&a, &s)["active"], false);
    assert_eq!(a["capacityTokens"], 2000000);
}

#[test]
fn freeze_requires_stable_account_official_rights_and_explicit_window_identity() {
    for field in [
        "accountScope",
        "identityEvidence",
        "entitlementSegment",
        "entitlementEvidence",
        "persisted",
    ] {
        let mut s = snapshot();
        s[field] = Value::Null;
        assert!(
            freeze(
                Some(&s),
                "source-a",
                "shared:primary",
                Some("model-a"),
                at()
            )["scope"]
                .is_null(),
            "{field}"
        );
    }
    for field in [
        "windowInstanceId",
        "windowInstanceEvidence",
        "ruleVersion",
        "recovery",
    ] {
        let mut s = snapshot();
        s["buckets"][0]["primary"][field] = Value::Null;
        assert!(
            freeze(
                Some(&s),
                "source-a",
                "shared:primary",
                Some("model-a"),
                at()
            )["scope"]
                .is_null(),
            "{field}"
        );
    }
}

#[test]
fn manual_note_or_membership_label_does_not_supply_missing_structure_or_rights() {
    let mut s = snapshot();
    s["buckets"][0]["primary"]["usageStructureEvidence"] = Value::Null;
    let mut a = assumption(&s);
    a["note"] = json!("会员pro，effort medium，假设全部model-a");
    assert!(a["manualScope"]["scope"].is_null());
    assert_eq!(project(&a, &s)["active"], false);
    s["planType"] = json!("pro");
    s["entitlementEvidence"] = Value::Null;
    assert!(freeze(
        Some(&s),
        "source-a",
        "shared:primary",
        Some("model-a"),
        at()
    )["scope"]
        .is_null());
}

#[test]
fn membership_account_runtime_binding_and_clear_changes_invalidate_frozen_scope() {
    let original = snapshot();
    let a = assumption(&original);
    for (field, value) in [
        ("accountScope", json!("account-b")),
        ("runtimeId", json!("runtime-b")),
        ("bindingGeneration", json!("2")),
        ("identityEpoch", json!("2")),
        ("clearGeneration", json!("new-clear")),
        ("entitlementSegment", json!("rights-b")),
    ] {
        let mut s = original.clone();
        s[field] = value;
        let p = project(&a, &s);
        assert_eq!(p["active"], false, "{field}");
        assert!(p["remainingLower"].is_null(), "{field}");
    }
}

#[test]
fn rule_period_pool_reset_and_usage_structure_are_not_silently_rebound() {
    let original = snapshot();
    let a = assumption(&original);
    for field in ["ruleVersion", "windowInstanceId", "resetsAt"] {
        let mut s = original.clone();
        s["buckets"][0]["primary"][field] = if field == "resetsAt" {
            json!((at() + Duration::hours(2)).timestamp())
        } else {
            json!("changed")
        };
        assert_eq!(project(&a, &s)["active"], false, "{field}");
    }
    let mut s = original.clone();
    s["buckets"][0]["id"] = json!("another-pool");
    assert_eq!(project(&a, &s)["active"], false);
    for (field, value) in [
        ("effort", json!("high")),
        ("speedMode", json!("fast")),
        ("outputShare", json!(0.3)),
        ("cacheShare", json!(0.4)),
        ("medianInputTokens", json!(1200)),
    ] {
        let mut s = original.clone();
        s["buckets"][0]["primary"]["usageStructureEvidence"]["profile"][field] = value;
        let p = project(&a, &s);
        assert_eq!(p["active"], false, "{field}");
        assert_eq!(p["reasons"], json!(["manual_scope_changed"]));
    }
}

#[test]
fn missing_or_unattributed_or_forged_structure_is_never_certified_by_a_flag() {
    for mutation in 0..8 {
        let mut s = snapshot();
        let e = &mut s["buckets"][0]["primary"]["usageStructureEvidence"];
        match mutation {
            0 => e["source"] = json!("manual"),
            1 => e["profile"]["allModelsAttributed"] = json!(false),
            2 => e["profile"]["models"][0]["share"] = json!(0.5),
            3 => e["profile"]["effort"] = Value::Null,
            4 => e["accountScope"] = json!("account-b"),
            5 => e["windowInstanceId"] = json!("period-b"),
            6 => e["profile"]["models"][0]["model"] = json!("model-b"),
            _ => e["profile"]["cacheShare"] = json!(-0.1),
        };
        assert!(
            freeze(
                Some(&s),
                "source-a",
                "shared:primary",
                Some("model-a"),
                at()
            )["scope"]
                .is_null(),
            "{mutation}"
        );
    }
}

#[test]
fn unknown_percent_precision_is_only_an_explicit_manual_point_estimate() {
    let mut s = snapshot();
    s["buckets"][0]["primary"]["roundingStep"] = Value::Null;
    let a = assumption(&s);
    let p = project(&a, &s);
    assert_eq!(p["active"], true);
    assert_eq!(p["remainingEstimate"], 800000.0);
    assert!(p["remainingLower"].is_null());
    assert!(p["remainingUpper"].is_null());
    assert_eq!(p["estimatePrecision"], "unknown");
    assert_eq!(p["reasons"], json!(["precision_unknown"]));
    assert_eq!(p["constraintsVerified"], false);
}

#[test]
fn missing_percent_and_unbounded_saturation_never_become_zero_remaining() {
    for used in [Value::Null, json!(100.0), json!(101.0)] {
        let mut s = snapshot();
        s["buckets"][0]["primary"]["usedPercent"] = used;
        let p = project(&assumption(&s), &s);
        assert!(p["remainingEstimate"].is_null());
        assert!(p["remainingLower"].is_null());
        assert!(p["remainingUpper"].is_null());
        assert_eq!(p["active"], false);
    }
}

#[test]
fn future_effective_time_or_captured_scope_cannot_take_effect_early() {
    let s = snapshot();
    let mut a = assumption(&s);
    a["effectiveFrom"] = json!((at() + Duration::minutes(1)).to_rfc3339());
    assert_eq!(project(&a, &s)["active"], false);
    assert_eq!(
        project(&a, &s)["reasons"],
        json!(["manual_assumption_not_started"])
    );
    let mut a = assumption(&s);
    a["manualScope"]["capturedAt"] = json!((at() + Duration::minutes(1)).to_rfc3339());
    assert_eq!(project(&a, &s)["active"], false);
    assert!(project(&a, &s)["remainingEstimate"].is_null());
}

#[test]
fn expired_assumption_stale_reads_and_reset_waiting_have_no_remaining_scenario() {
    let s = snapshot();
    let mut a = assumption(&s);
    a["validUntil"] = json!(at().to_rfc3339());
    assert_eq!(project(&a, &s)["active"], false);
    let a = assumption(&s);
    for field in ["accountRead", "quotaRead"] {
        let mut stale = s.clone();
        stale[field]["freshnessAnchor"] = json!((at() - Duration::seconds(240)).to_rfc3339());
        assert_eq!(project(&a, &stale)["active"], false);
    }
    let p = scenario(&a, Some(&s), "source-a", at() + Duration::hours(1));
    assert_eq!(p["active"], false);
    assert!(p["remainingEstimate"].is_null());
}

#[test]
fn source_model_and_invalid_capacity_cannot_reuse_a_different_manual_premise() {
    let s = snapshot();
    let a = assumption(&s);
    assert_eq!(scenario(&a, Some(&s), "source-b", at())["active"], false);
    let mut other = a.clone();
    other["model"] = json!("model-b");
    assert_eq!(project(&other, &s)["active"], false);
    for capacity in [json!(0), json!(-1), json!(1.5), json!(9007199254740992u64)] {
        let mut other = a.clone();
        other["capacityTokens"] = capacity;
        assert_eq!(project(&other, &s)["active"], false);
    }
    let mut other = a;
    other["quality"] = json!("validated");
    let p = project(&other, &s);
    assert_eq!(p["quality"], "conditional");
    assert_eq!(p["constraintsVerified"], false);
    assert_eq!(p["notifyEligible"], false);
}

#[test]
fn shared_pool_model_scenarios_remain_independent_and_mutually_exclusive() {
    let s = snapshot();
    let a = assumption(&s);
    let before = a.clone();
    let p = project(&a, &s);
    let mut mixed = s.clone();
    let e = &mut mixed["buckets"][0]["primary"]["usageStructureEvidence"];
    e["model"] = Value::Null;
    e["profile"]["models"] =
        json!([{"model":"model-a","share":0.5},{"model":"model-b","share":0.5}]);
    let mut b = a.clone();
    b["id"] = json!("assumption-b");
    b["model"] = Value::Null;
    b["capacityTokens"] = json!(1000000);
    b["manualScope"] = freeze(Some(&mixed), "source-a", "shared:primary", None, at());
    let q = project(&b, &mixed);
    assert_eq!(p["remainingEstimate"], 800000.0);
    assert_eq!(q["remainingEstimate"], 400000.0);
    assert_eq!(p["mutuallyExclusive"], true);
    assert_eq!(q["mutuallyExclusive"], true);
    assert_eq!(a, before);
    assert!(p.get("combinedRemaining").is_none());
}

#[test]
fn one_attested_pure_profile_can_freeze_actual_mixture_or_the_same_selected_model_only() {
    let s = snapshot();
    for model in [None, Some("model-a")] {
        let frozen = freeze(Some(&s), "source-a", "shared:primary", model, at());
        assert!(frozen["scope"].is_object(), "{frozen}");
        assert_eq!(
            frozen["scope"]["usageStructure"]["models"][0]["model"],
            "model-a"
        );
        assert_eq!(frozen["scope"]["model"], json!(model));
    }
    assert!(freeze(
        Some(&s),
        "source-a",
        "shared:primary",
        Some("model-b"),
        at()
    )["scope"]
        .is_null());
    let mut mixed = s.clone();
    let evidence = &mut mixed["buckets"][0]["primary"]["usageStructureEvidence"];
    evidence["model"] = Value::Null;
    evidence["profile"]["models"] =
        json!([{"model":"model-a","share":0.5},{"model":"model-b","share":0.5}]);
    assert!(freeze(Some(&mixed), "source-a", "shared:primary", None, at())["scope"].is_object());
    assert!(freeze(
        Some(&mixed),
        "source-a",
        "shared:primary",
        Some("model-a"),
        at()
    )["scope"]
        .is_null());
    mixed["buckets"][0]["primary"]["usageStructureEvidence"]["model"] = json!("model-a");
    assert!(
        freeze(Some(&mixed), "source-a", "shared:primary", None, at())["scope"].is_null(),
        "attested model must describe the profile itself"
    );
}
