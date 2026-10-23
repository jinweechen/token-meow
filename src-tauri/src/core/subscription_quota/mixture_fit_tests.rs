use super::*;

fn sample(index: usize, tokens: Vec<u64>, charge_points: f64, holdout: bool) -> MixtureSample {
    MixtureSample {
        id: format!("sample-{index}"),
        from_ms: index as i64 * 1000,
        to_ms: (index as i64 + 1) * 1000,
        charge_points,
        tokens,
        holdout,
    }
}
fn three_models() -> (Vec<String>, Vec<MixtureSample>) {
    let models = vec!["model-a".into(), "model-b".into(), "model-c".into()];
    let vectors = [
        [1, 0, 0],
        [0, 1, 0],
        [0, 0, 1],
        [1, 1, 0],
        [0, 1, 1],
        [1, 0, 1],
        [3, 2, 1],
        [1, 4, 2],
        [2, 1, 3],
        [2, 3, 1],
        [1, 1, 2],
        [5, 2, 4],
    ];
    let samples = vectors
        .iter()
        .enumerate()
        .map(|(index, vector)| {
            sample(
                index,
                vector.iter().map(|value| value * 1_000_000).collect(),
                vector
                    .iter()
                    .zip([2, 5, 9])
                    .map(|(value, weight)| (value * weight) as f64)
                    .sum(),
                index >= 9,
            )
        })
        .collect();
    (models, samples)
}
fn close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() <= 1e-8 * (1.0 + expected.abs()),
        "actual={actual} expected={expected}"
    );
}

#[test]
fn full_rank_three_model_synthetic_input_recovers_independent_weights_and_prices_vector() {
    let (models, samples) = three_models();
    let result = fit(&models, &samples);
    assert_eq!(result.kind, FitKind::Separated, "{result:?}");
    assert_eq!(result.rank, 3);
    assert_eq!((result.train_samples, result.holdout_samples), (9, 3));
    for (weight, expected) in result.weights.as_ref().unwrap().iter().zip([2.0, 5.0, 9.0]) {
        close(weight.points_per_million_tokens, expected);
    }
    close(
        predict_tokens(&result, &[2_000_000, 1_000_000, 0]).unwrap(),
        9.0,
    );
    assert_eq!(
        predict_tokens(&result, &[1_000_000_000_000_000, 0, 0]),
        Err("prediction_out_of_range")
    );
    assert!(result.train_wape.unwrap() < 1e-12 && result.holdout_wape.unwrap() < 1e-12);
}

#[test]
fn column_normalization_handles_several_orders_of_token_magnitude() {
    let models = vec!["small".into(), "large".into()];
    let vectors = [
        [1, 0],
        [0, 1_000_000_000],
        [3, 1_000_000_000],
        [1, 3_000_000_000],
        [5, 2_000_000_000],
        [2, 4_000_000_000],
    ];
    let samples: Vec<_> = vectors
        .iter()
        .enumerate()
        .map(|(i, v)| {
            sample(
                i,
                v.to_vec(),
                v[0] as f64 * 2.0 + v[1] as f64 / 1_000_000.0 * 0.000005,
                i >= 4,
            )
        })
        .collect();
    let result = fit(&models, &samples);
    assert_eq!(result.kind, FitKind::Separated, "{result:?}");
    let weights = result.weights.unwrap();
    close(weights[0].points_per_million_tokens, 2_000_000.0);
    close(weights[1].points_per_million_tokens, 0.000005);
}

#[test]
fn frozen_holdout_is_never_used_to_fit_and_failed_backtest_withholds_weights() {
    let (models, mut samples) = three_models();
    let baseline = fit(&models, &samples);
    for sample in samples.iter_mut().filter(|sample| sample.holdout) {
        sample.charge_points *= 1.1;
    }
    let nearby = fit(&models, &samples);
    assert_eq!(nearby.kind, FitKind::Separated);
    assert_eq!(
        nearby.weights, baseline.weights,
        "留出变化不得重新拟合训练权重"
    );
    assert!(nearby.holdout_wape.unwrap() > 0.05);
    for sample in samples.iter_mut().filter(|sample| sample.holdout) {
        sample.charge_points *= 2.0;
    }
    let failed = fit(&models, &samples);
    assert_eq!(failed.kind, FitKind::Unavailable);
    assert!(failed.weights.is_none() && failed.fixed_mix.is_none());
    assert!(failed
        .reasons
        .iter()
        .any(|value| value == "backtest_failed"));
    assert!(failed.holdout_wape.unwrap() > 0.2);
}

#[test]
fn fixed_proportions_only_identify_overall_mix_and_never_independent_model_prices() {
    let models = vec!["a".into(), "b".into()];
    let mut samples: Vec<_> = (1..=8)
        .map(|i| {
            sample(
                i - 1,
                vec![i as u64 * 1_000_000, i as u64 * 2_000_000],
                i as f64 * 12.0,
                i > 6,
            )
        })
        .collect();
    let result = fit(&models, &samples);
    assert_eq!(result.kind, FitKind::FixedMix, "{result:?}");
    assert_eq!(result.rank, 1);
    assert!(result.weights.is_none());
    let fixed = result.fixed_mix.as_ref().unwrap();
    close(fixed.shares[0], 1.0 / 3.0);
    close(fixed.points_per_million_tokens, 4.0);
    assert_eq!(
        predict_tokens(&result, &[1_000_000, 2_000_000]),
        Err("fit_unavailable")
    );
    samples[7].tokens = vec![1_000_000, 1_000_000];
    assert_eq!(
        fit(&models, &samples).kind,
        FitKind::Unavailable,
        "换比例的留出不能由固定混合斜率解释"
    );
}

#[test]
fn near_collinearity_never_publishes_unstable_individual_weights() {
    let models = vec!["a".into(), "b".into()];
    let samples: Vec<_> = (0..8)
        .map(|i| {
            let a = (i as u64 + 1) * 1_000_000;
            let b = a + if i % 2 == 0 { 1 } else { 0 };
            sample(
                i,
                vec![a, b],
                a as f64 / 1_000_000.0 * 2.0 + b as f64 / 1_000_000.0 * 5.0,
                i >= 6,
            )
        })
        .collect();
    let result = fit(&models, &samples);
    assert_eq!(result.kind, FitKind::Unavailable, "{result:?}");
    assert!(result.weights.is_none());
    assert!(result
        .reasons
        .iter()
        .any(|value| value == "rank_deficient" || value == "ill_conditioned"));
}

#[test]
fn nnls_reoptimizes_support_and_zero_weight_does_not_price_active_tokens_as_free() {
    let models = vec!["a".into(), "b".into()];
    let values = [
        (vec![1_000_000, 0], 10.0),
        (vec![0, 1_000_000], 0.0),
        (vec![1_000_000, 1_000_000], 8.0),
        (vec![2_000_000, 1_000_000], 18.0),
        (vec![3_000_000, 0], 28.0),
        (vec![4_000_000, 0], 37.0),
    ];
    let samples: Vec<_> = values
        .into_iter()
        .enumerate()
        .map(|(i, (tokens, charge))| sample(i, tokens, charge, i >= 4))
        .collect();
    let result = fit(&models, &samples);
    assert_eq!(result.kind, FitKind::Separated, "{result:?}");
    close(
        result.weights.as_ref().unwrap()[0].points_per_million_tokens,
        9.0,
    );
    assert_eq!(
        result.weights.as_ref().unwrap()[1].points_per_million_tokens,
        0.0
    );
    assert_eq!(
        predict_tokens(&result, &[0, 1_000_000]),
        Err("zero_weight_unpriced")
    );
    assert!(predict_tokens(&result, &[1_000_000, 0]).unwrap() > 0.0);
}

#[test]
fn invalid_values_dimensions_duplicate_ids_and_overlaps_fail_closed() {
    let (models, samples) = three_models();
    let mut invalid = samples.clone();
    invalid[0].charge_points = f64::NAN;
    assert!(fit(&models, &invalid)
        .reasons
        .contains(&"invalid_number".into()));
    invalid = samples.clone();
    invalid[0].tokens.pop();
    assert_eq!(fit(&models, &invalid).kind, FitKind::Unavailable);
    invalid = samples.clone();
    invalid[0].tokens[0] = MAX_TOKEN + 1;
    assert_eq!(fit(&models, &invalid).kind, FitKind::Unavailable);
    invalid = samples.clone();
    invalid[1].id = invalid[0].id.clone();
    assert!(fit(&models, &invalid)
        .reasons
        .contains(&"invalid_sample_id".into()));
    invalid = samples.clone();
    invalid[1].from_ms = invalid[0].to_ms - 1;
    assert!(fit(&models, &invalid)
        .reasons
        .contains(&"overlapping_samples".into()));
    assert_eq!(
        fit(&["a".into(), "a".into()], &samples).kind,
        FitKind::Unavailable
    );
}

#[test]
fn all_zero_signal_insufficient_samples_and_interleaved_holdout_do_not_yield_weights() {
    let (models, mut samples) = three_models();
    for sample in &mut samples {
        sample.charge_points = 0.0;
    }
    assert!(fit(&models, &samples)
        .reasons
        .contains(&"low_signal".into()));
    let (_, mut samples) = three_models();
    samples[3].holdout = true;
    assert!(fit(&models, &samples)
        .reasons
        .contains(&"holdout_invalid".into()));
    assert!(fit(&models, &samples[..3])
        .reasons
        .contains(&"insufficient_samples".into()));
}

#[test]
fn result_serialization_never_grants_official_proof_or_confidence_bounds() {
    let (models, samples) = three_models();
    let value = serde_json::to_value(fit(&models, &samples)).unwrap();
    assert_eq!(value["quality"], "conditional-manual");
    assert_eq!(value["constraintsVerified"], false);
    assert_eq!(value["notifyEligible"], false);
    assert!(value["uncertaintyBounds"].is_null());
    assert_eq!(value["weights"][0]["model"], "model-a");
}
