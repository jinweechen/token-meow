use super::super::model::fixtures;
use super::*;
use chrono::Duration;

fn at(seconds: i64) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-22T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
        + Duration::seconds(seconds)
}
fn profile(model: &str) -> UsageProfile {
    UsageProfile {
        models: vec![super::super::model::ModelShare {
            model: model.into(),
            share: 1.0,
        }],
        all_models_attributed: true,
        effort: Some("medium".into()),
        speed_mode: Some("normal".into()),
        output_share: 0.2,
        cache_share: 0.3,
        median_input_tokens: 1000.0,
    }
}
fn selection() -> Selection<'static> {
    Selection {
        source_id: "source-a",
        window_id: "shared:primary",
        model: Some("model-a"),
        model_dimension: "response",
        reserve_percent: 20.0,
    }
}
fn fixture_remaining_lower(card: &Value) -> u64 {
    // Projector range endpoints are outward-rounded f64 JSON values, while a
    // saved plan target is an integer JSON value. Read their numeric value
    // without requiring the range to use serde_json's integer representation.
    let lower = card["remainingLower"].as_f64().unwrap();
    assert!(lower.is_finite() && lower >= 0.0 && lower <= MAX_SAFE as f64);
    assert_eq!(lower.fract(), 0.0);
    lower as u64
}
struct Fixture {
    current: Value,
    windows: Vec<Value>,
    cards: Vec<Value>,
    now: DateTime<Utc>,
}
impl Fixture {
    fn new(specs: &[(&str, u64, i64)]) -> Self {
        let now = at(23_301);
        let anchor = at(23_300);
        let mut buckets = BTreeMap::<String, Value>::new();
        let mut windows = Vec::new();
        let mut cards = Vec::new();
        for (window_id, tokens, until) in specs {
            let (pool, kind) = window_id.split_once(':').unwrap();
            let mut pairs = Vec::new();
            for (period, train_count) in [7, 7, 6].into_iter().enumerate() {
                let rows = train_count + if period == 2 { 5 } else { 0 };
                for step in 0..rows {
                    let index = pairs.len() as u64;
                    let start_at = at(period as i64 * 10_000 + step as i64 * 300);
                    let start_seconds = start_at.timestamp() as f64;
                    let used = 10.0 + step as f64 * 5.0;
                    let mut pair = fixtures::pair(index, start_seconds, used);
                    pair.tokens = *tokens;
                    pair.profile = profile("model-a");
                    pair.holdout = step >= train_count;
                    pair.end.percent.used_percent = used + 5.0;
                    pair.start.scope = EvaluationScope {
                        runtime_id: "runtime-a".into(),
                        source_id: "source-a".into(),
                        binding_generation: 1,
                        identity_epoch: 1,
                        clear_generation: "null".into(),
                        account_scope: Some("account-a".into()),
                        entitlement_segment: "rights-a".into(),
                        rule_version: "rule-a".into(),
                        pool_id: (*window_id).into(),
                        window_instance_id: format!("{window_id}:period-{period}"),
                    };
                    pair.end.scope = pair.start.scope.clone();
                    let reset = if period == 2 {
                        at(*until)
                    } else {
                        at(period as i64 * 10_000 + 9000)
                    };
                    for sample in [&mut pair.start, &mut pair.end] {
                        sample.reset_time = Interval::exact(reset.timestamp() as f64);
                        sample.clock_epoch = "clock-a".into();
                        sample.observation_id =
                            format!("runtime-a:{}:{window_id}", sample.sequence);
                    }
                    pairs.push(pair);
                }
            }
            let last = pairs.last().unwrap();
            assert_eq!(last.end.fact_time.unwrap().lower, anchor.timestamp() as f64);
            let result = calibrate(&pairs, &profile("model-a"));
            assert_eq!(result.quality, CalibrationQuality::Validated, "{result:?}");
            let capacity = result.capacity.unwrap();
            let remaining = remaining_tokens(capacity, &last.end.percent).unwrap();
            let period = last.end.scope.window_instance_id.clone();
            let reset = at(*until);
            let raw = json!({"usedPercent":last.end.percent.used_percent,"roundingStep":1,"saturated":false,"saturationHeadroomBounded":false,
                "resetsAt":reset.timestamp(),"recovery":"fixed_reset","ruleVersion":"rule-a","windowInstanceId":period,
                "windowInstanceEvidence":{"verified":true,"source":"official","windowInstanceId":period,"reference":"synthetic-period-proof"},
                "usageStructureEvidence":{"verified":true,"source":"trusted-ledger","reference":"synthetic-current-pair-only",
                    "sourceId":"source-a","accountScope":"account-a","windowId":window_id,"windowInstanceId":period,"model":"model-a","modelDimension":"response","profile":profile("model-a")}});
            let bucket = buckets
                .entry(pool.into())
                .or_insert_with(|| json!({"id":pool}));
            bucket[kind] = raw;
            windows.push(json!({"id":window_id,"bucketId":pool,"periodInstanceId":period,"usedPercent":last.end.percent.used_percent,
                "resetsAt":timestamp(reset),"resetState":"current","freshness":"fresh","canBindPlan":true,"reasons":[]}));
            cards.push(json!({"id":window_id,"windowId":window_id,"model":"model-a","quality":"validated","coverage":"verified_complete",
                "capacityLower":capacity.lower.floor(),"capacityUpper":capacity.upper.ceil(),"remainingLower":remaining.lower.floor(),"remainingUpper":remaining.upper.ceil(),
                "sampleCount":result.train_pairs,"cycleCount":result.independent_periods,"profileEvidence":profile("model-a"),"reasons":[],"evidence":result,"inputPairs":pairs}));
        }
        let current = json!({"runtimeId":"runtime-a","sourceId":"source-a","bindingGeneration":"1","identityEpoch":"1","clearGeneration":null,
            "accountScope":"account-a","entitlementSegment":"rights-a","entitlementEvidence":{"verified":true,"source":"official","segmentId":"rights-a","ruleVersion":"rule-a","reference":"synthetic-rights-proof"},
            "identityConsistency":"proven","identityEvidence":{"reference":"synthetic-identity-proof"},"status":"connected","observationOutcome":"success","observationSequence":"50","persisted":true,"discontinuity":false,
            "accountRead":{"outcome":"success","freshnessAnchor":timestamp(anchor)},
            "quotaRead":{"outcome":"success","freshnessAnchor":timestamp(anchor),"factTimeLower":timestamp(anchor),"factTimeUpper":timestamp(anchor),"timeBasis":"provider-fact-time","clockEpoch":"clock-a"},
            "buckets":buckets.into_values().collect::<Vec<_>>()});
        Self {
            current,
            windows,
            cards,
            now,
        }
    }
    fn one() -> Self {
        Self::new(&[("shared:primary", 50_000, 30_500)])
    }
    fn draft(&self, choice: &Selection<'_>) -> Option<Value> {
        token_draft(
            choice,
            &self.current,
            &self.windows,
            &self.cards,
            "runtime-a",
            self.now,
        )
    }
    fn raw_mut(&mut self, id: &str) -> &mut Value {
        let (pool, kind) = id.split_once(':').unwrap();
        let bucket = self.current["buckets"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|b| b["id"] == pool)
            .unwrap();
        &mut bucket[kind]
    }
}

#[test]
fn validated_current_remaining_produces_only_an_editable_time_limited_token_draft() {
    let fixture = Fixture::one();
    let draft = fixture.draft(&selection()).unwrap();
    let lower = fixture_remaining_lower(&fixture.cards[0]);
    assert_eq!(draft["plan"]["type"], "tokens");
    assert_eq!(draft["plan"]["target"], lower * 8 / 10);
    assert_eq!(draft["plan"]["endPolicy"], "time");
    assert!(draft["plan"]["windowId"].is_null());
    assert_eq!(draft["constraintsVerified"], true);
    assert_eq!(draft["notifyEligible"], false);
    assert!(
        draft["plan"].get("effectiveFrom").is_none(),
        "only saving starts the plan"
    );
    assert_eq!(draft["sustainability"]["conditional"], true);
}

#[test]
fn all_related_pools_use_the_minimum_remaining_and_earliest_reset_without_adding_models() {
    let fixture = Fixture::new(&[
        ("shared:primary", 50_000, 30_500),
        ("shared:secondary", 25_000, 29_000),
        ("extra:primary", 75_000, 28_000),
    ]);
    let draft = fixture.draft(&selection()).unwrap();
    let minimum = fixture
        .cards
        .iter()
        .map(fixture_remaining_lower)
        .min()
        .unwrap();
    assert_eq!(draft["plan"]["target"], minimum * 8 / 10);
    assert_eq!(draft["plan"]["endsAt"], timestamp(at(28_000)));
    assert_eq!(
        draft["sustainability"]["constraints"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert!(
        draft["plan"]["target"].as_u64().unwrap()
            < fixture
                .cards
                .iter()
                .map(fixture_remaining_lower)
                .sum::<u64>()
    );
}

#[test]
fn a_single_unknown_related_window_prevents_a_token_verdict() {
    for mutation in [
        "stale",
        "unbindable",
        "reset",
        "missing-card",
        "missing-window",
        "duplicate",
        "extra-raw",
    ] {
        let mut fixture = Fixture::new(&[
            ("shared:primary", 50_000, 30_500),
            ("shared:secondary", 25_000, 29_000),
        ]);
        match mutation {
            "stale" => fixture.windows[1]["freshness"] = json!("stale"),
            "unbindable" => fixture.windows[1]["canBindPlan"] = json!(false),
            "reset" => fixture.windows[1]["resetsAt"] = Value::Null,
            "missing-card" => {
                fixture.cards.pop();
            }
            "missing-window" => {
                fixture.windows.pop();
            }
            "duplicate" => fixture.windows[1] = fixture.windows[0].clone(),
            "extra-raw" => fixture.current["buckets"][0]["other"] = json!({"unknown":true}),
            _ => unreachable!(),
        }
        if mutation == "extra-raw" {
            fixture.current["buckets"]
                .as_array_mut()
                .unwrap()
                .push(json!({"id":"unknown-extra","primary":{"unknown":true}}));
        }
        assert!(fixture.draft(&selection()).is_none(), "{mutation}");
    }
}

#[test]
fn quality_strings_manual_scenarios_archives_and_budget_omissions_cannot_open_g4b() {
    for mutation in [
        "conditional",
        "invalidated",
        "learning",
        "manual",
        "archive",
        "budget",
        "forged-quality",
        "no-pairs",
        "no-evidence",
        "coverage",
        "user-coverage",
    ] {
        let mut fixture = Fixture::one();
        match mutation {
            "conditional" | "invalidated" | "learning" => {
                fixture.cards[0]["quality"] = json!(mutation)
            }
            "manual" => fixture.cards[0]["provenance"] = json!("manual-assumption"),
            "archive" => fixture.cards[0]["archived"] = json!(true),
            "budget" => fixture.cards[0]["reasons"] = json!(["projection_budget_exceeded"]),
            "forged-quality" => {
                fixture.cards[0]["evidence"]["quality"] = json!("conditional");
            }
            "no-pairs" => fixture.cards[0]["inputPairs"] = json!([]),
            "no-evidence" => fixture.cards[0]["evidence"] = Value::Null,
            "coverage" => fixture.cards[0]["coverage"] = json!("conditional_user_assumption"),
            "user-coverage" => {
                fixture.cards[0]["inputPairs"][0]["coverage"] = json!("conditional_user_assumption")
            }
            _ => unreachable!(),
        }
        assert!(fixture.draft(&selection()).is_none(), "{mutation}");
    }
}

#[test]
fn unsafe_unknown_zero_or_forged_current_amounts_never_create_target_zero() {
    for lower in [
        Value::Null,
        json!(-1),
        json!(0),
        json!(0.5),
        json!(MAX_SAFE + 1),
        json!(1),
    ] {
        let mut fixture = Fixture::one();
        fixture.cards[0]["remainingLower"] = lower.clone();
        assert!(fixture.draft(&selection()).is_none(), "{lower}");
    }
    for reserve in [-1.0, 100.0, f64::NAN, f64::INFINITY] {
        let fixture = Fixture::one();
        let mut choice = selection();
        choice.reserve_percent = reserve;
        assert!(fixture.draft(&choice).is_none());
    }
    assert_eq!(reserved_target(1, 99.0), None);
    assert_eq!(reserved_target(MAX_SAFE, 0.0), Some(MAX_SAFE));
    assert_eq!(reserved_target(1000, 20.1), Some(799));
}

#[test]
fn every_current_identity_and_period_dimension_is_bound_to_the_actual_pair_endpoint() {
    for field in [
        "runtimeId",
        "sourceId",
        "bindingGeneration",
        "identityEpoch",
        "clearGeneration",
        "accountScope",
        "entitlementSegment",
        "observationSequence",
        "persisted",
        "discontinuity",
    ] {
        let mut fixture = Fixture::one();
        fixture.current[field] = match field {
            "bindingGeneration" | "identityEpoch" => json!("2"),
            "observationSequence" => json!("51"),
            "persisted" => json!(false),
            "discontinuity" => json!(true),
            "clearGeneration" => json!("new-clear"),
            _ => json!("different"),
        };
        assert!(fixture.draft(&selection()).is_none(), "{field}");
    }
    for field in [
        "windowInstanceId",
        "ruleVersion",
        "usedPercent",
        "roundingStep",
    ] {
        let mut fixture = Fixture::one();
        fixture.raw_mut("shared:primary")[field] = match field {
            "usedPercent" => json!(64),
            "roundingStep" => json!(2),
            _ => json!("different"),
        };
        assert!(fixture.draft(&selection()).is_none(), "{field}");
    }
}

#[test]
fn old_fact_time_clock_or_uncertain_reset_cannot_supply_current_remaining() {
    for mutation in [
        "stale",
        "future",
        "clock",
        "reset-interval",
        "unknown-time",
        "read-failed",
    ] {
        let mut fixture = Fixture::one();
        match mutation {
            "stale" => fixture.now += Duration::seconds(240),
            "future" => {
                fixture.current["quotaRead"]["factTimeUpper"] =
                    json!(timestamp(fixture.now + Duration::seconds(1)))
            }
            "clock" => fixture.current["quotaRead"]["clockEpoch"] = json!("new-clock"),
            "reset-interval" => {
                fixture.raw_mut("shared:primary")["resetTimeLower"] = json!(at(30_499).timestamp());
                fixture.raw_mut("shared:primary")["resetTimeUpper"] = json!(at(30_501).timestamp());
            }
            "unknown-time" => fixture.current["quotaRead"]["timeBasis"] = json!("unknown"),
            "read-failed" => fixture.current["quotaRead"]["outcome"] = json!("failure"),
            _ => unreachable!(),
        }
        assert!(fixture.draft(&selection()).is_none(), "{mutation}");
    }
}

#[test]
fn request_model_and_a_different_actual_model_never_share_response_capacity() {
    let fixture = Fixture::one();
    let mut choice = selection();
    choice.model_dimension = "request";
    assert!(fixture.draft(&choice).is_none());
    choice = selection();
    choice.model = Some("model-b");
    assert!(fixture.draft(&choice).is_none());
    choice.model = None;
    assert!(
        fixture.draft(&choice).is_some(),
        "the actual current combination may contain one model"
    );
}

#[test]
fn common_usage_structure_must_match_all_pools_and_actual_calibration_profiles() {
    for mutation in [
        "effort",
        "speed",
        "cache",
        "median",
        "model-shares",
        "unknown-effort",
        "untrusted",
    ] {
        let mut fixture = Fixture::new(&[
            ("shared:primary", 50_000, 30_500),
            ("shared:secondary", 25_000, 29_000),
        ]);
        let evidence = &mut fixture.raw_mut("shared:secondary")["usageStructureEvidence"];
        match mutation {
            "effort" => evidence["profile"]["effort"] = json!("high"),
            "speed" => evidence["profile"]["speedMode"] = json!("fast"),
            "cache" => evidence["profile"]["cacheShare"] = json!(0.8),
            "median" => evidence["profile"]["medianInputTokens"] = json!(10_000),
            "model-shares" => {
                evidence["model"] = Value::Null;
                evidence["profile"]["models"] =
                    json!([{"model":"model-a","share":0.5},{"model":"model-b","share":0.5}]);
            }
            "unknown-effort" => evidence["profile"]["effort"] = Value::Null,
            "untrusted" => evidence["source"] = json!("user-specified"),
            _ => unreachable!(),
        }
        let mut choice = selection();
        choice.model = None;
        assert!(fixture.draft(&choice).is_none(), "{mutation}");
    }
}

#[test]
fn current_mixed_profile_does_not_authorize_a_single_model_or_sum_hypotheticals() {
    let mut fixture = Fixture::one();
    let mixed = json!({"models":[{"model":"model-a","share":0.5},{"model":"model-b","share":0.5}],"allModelsAttributed":true,"effort":"medium","speedMode":"normal","outputShare":0.2,"cacheShare":0.3,"medianInputTokens":1000});
    {
        let evidence = &mut fixture.raw_mut("shared:primary")["usageStructureEvidence"];
        evidence["model"] = Value::Null;
        evidence["profile"] = mixed.clone();
    }
    fixture.cards[0]["model"] = Value::Null;
    fixture.cards[0]["profileEvidence"] = mixed.clone();
    for pair in fixture.cards[0]["inputPairs"].as_array_mut().unwrap() {
        pair["profile"] = mixed.clone();
    }
    assert!(
        fixture.draft(&selection()).is_none(),
        "a mixed remainder cannot become model-a capacity"
    );
    let mut choice = selection();
    choice.model = None;
    assert!(
        fixture.draft(&choice).is_some(),
        "the same covered combination can have one mutually exclusive scenario"
    );
    fixture.cards.push(fixture.cards[0].clone());
    assert!(
        fixture.draft(&choice).is_none(),
        "multiple scenarios of one pool cannot be added as separate capacities"
    );
}

#[test]
fn standalone_card_metadata_cannot_replace_the_current_input_or_recomputed_validation() {
    for mutation in [
        "pair-scope",
        "pair-sequence",
        "pair-profile",
        "holdout",
        "capacity",
        "remaining-upper",
        "structure-proof",
    ] {
        let mut fixture = Fixture::one();
        match mutation {
            "pair-scope" => {
                fixture.cards[0]["inputPairs"][24]["end"]["scope"]["accountScope"] =
                    json!("old-account")
            }
            "pair-sequence" => fixture.cards[0]["inputPairs"][24]["end"]["sequence"] = json!("49"),
            "pair-profile" => {
                fixture.cards[0]["inputPairs"][24]["profile"]["effort"] = json!("high")
            }
            "holdout" => {
                for pair in fixture.cards[0]["inputPairs"].as_array_mut().unwrap() {
                    pair["holdout"] = json!(false);
                }
            }
            "capacity" => fixture.cards[0]["evidence"]["capacity"]["lower"] = json!(1),
            "remaining-upper" => fixture.cards[0]["remainingUpper"] = Value::Null,
            "structure-proof" => {
                fixture.raw_mut("shared:primary")["usageStructureEvidence"] = Value::Null
            }
            _ => unreachable!(),
        }
        assert!(fixture.draft(&selection()).is_none(), "{mutation}");
    }
}

#[test]
fn draft_projection_never_mutates_inputs_or_mints_reminder_authority() {
    let fixture = Fixture::one();
    let before = json!([fixture.current, fixture.windows, fixture.cards]);
    let first = fixture.draft(&selection()).unwrap();
    let again = fixture.draft(&selection()).unwrap();
    assert_eq!(first, again);
    assert_eq!(
        json!([fixture.current, fixture.windows, fixture.cards]),
        before
    );
    assert_eq!(first["notifyEligible"], false);
    assert!(first.get("permit").is_none());
    assert!(first.get("savedPlanId").is_none());
}
