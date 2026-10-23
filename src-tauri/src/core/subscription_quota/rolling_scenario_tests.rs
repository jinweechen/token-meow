use super::super::mixture_fit::{fit, predict_tokens, FitKind, MixtureSample};
use super::*;
const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;

fn input() -> RollingInput {
    RollingInput {
        as_of_ms: 10 * HOUR,
        history_complete_from_ms: Some(0),
        window_ms: 5 * HOUR,
        horizon_ms: 20 * HOUR,
        capacity_points: 100.0,
        required_points: 60.0,
        charges: Vec::new(),
    }
}
fn charge(id: &str, at: i64, points: Option<f64>) -> RollingCharge {
    RollingCharge {
        id: id.into(),
        occurred_at_ms: at,
        points,
    }
}
fn at(result: &RollingResult, at_ms: i64) -> &RollingPoint {
    result
        .points
        .iter()
        .find(|point| point.at_ms == at_ms)
        .unwrap()
}
fn close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-8, "{actual} != {expected}");
}

#[test]
fn actual_svd_to_predicted_charge_to_step_release_closed_loop() {
    let models = vec!["a".into(), "b".into()];
    let vectors = [
        [1, 0],
        [0, 1],
        [1, 1],
        [2, 1],
        [1, 2],
        [3, 2],
        [2, 3],
        [4, 1],
    ];
    let samples: Vec<_> = vectors
        .iter()
        .enumerate()
        .map(|(i, v)| MixtureSample {
            id: format!("s-{i}"),
            from_ms: i as i64 * MINUTE,
            to_ms: (i as i64 + 1) * MINUTE,
            charge_points: (v[0] * 2 + v[1] * 5) as f64,
            tokens: v.iter().map(|n| n * 1_000_000).collect(),
            holdout: i >= 6,
        })
        .collect();
    let fit = fit(&models, &samples);
    assert_eq!(fit.kind, FitKind::Separated, "{fit:?}");
    let predicted = predict_tokens(&fit, &[3_000_000, 2_000_000]).unwrap();
    close(predicted, 16.0);
    let mut data = input();
    data.charges = vec![
        charge("modeled-history", 9 * HOUR, Some(predicted)),
        charge("manual-current", 10 * HOUR, Some(20.0)),
        charge("future-manual", 12 * HOUR, Some(6.0)),
    ];
    let result = simulate(&data).unwrap();
    for (time, expected) in [
        (10, 36.0),
        (12, 42.0),
        (14, 26.0),
        (15, 6.0),
        (17, 0.0),
        (20, 0.0),
    ] {
        close(at(&result, time * HOUR).used_points.unwrap(), expected);
    }
    assert_eq!(
        result.availability_windows,
        vec![
            AvailabilityWindow {
                from_ms: 10 * HOUR,
                to_ms: 12 * HOUR
            },
            AvailabilityWindow {
                from_ms: 14 * HOUR,
                to_ms: 20 * HOUR
            }
        ]
    );
    println!(
        "MIXTURE_ROLLING_NUMERIC_FIXTURE={}",
        serde_json::json!({"fit":fit,"rolling":result})
    );
}

#[test]
fn half_open_window_merges_same_time_expiry_and_new_entry_before_evaluation() {
    let mut data = input();
    data.charges = vec![
        charge("already-expired", 5 * HOUR, Some(90.0)),
        charge("current", 10 * HOUR, Some(80.0)),
        charge("simultaneous", 15 * HOUR, Some(30.0)),
    ];
    let result = simulate(&data).unwrap();
    assert_eq!(at(&result, 10 * HOUR).used_points, Some(80.0));
    assert_eq!(at(&result, 15 * HOUR).used_points, Some(30.0));
    assert_eq!(
        result
            .points
            .iter()
            .filter(|point| point.at_ms == 15 * HOUR)
            .count(),
        1
    );
    assert_eq!(result.next_available_ms, Some(15 * HOUR));
}

#[test]
fn unknown_initial_history_stays_null_until_its_entire_window_has_expired() {
    let mut data = input();
    data.history_complete_from_ms = None;
    data.charges = vec![charge("future", 12 * HOUR, Some(10.0))];
    let result = simulate(&data).unwrap();
    for time in [10, 12] {
        let point = at(&result, time * HOUR);
        assert!(
            point.used_points.is_none()
                && point.remaining_points.is_none()
                && point.available.is_none()
        );
    }
    assert_eq!(at(&result, 15 * HOUR).used_points, Some(10.0));
    assert_eq!(at(&result, 17 * HOUR).used_points, Some(0.0));
    assert_eq!(result.next_available_ms, Some(15 * HOUR));
    assert!(result.reasons.contains(&"history_incomplete".into()));
}

#[test]
fn partial_history_recovers_at_declared_coverage_boundary_and_unknown_charges_have_local_lifetime()
{
    let mut data = input();
    data.history_complete_from_ms = Some(6 * HOUR);
    data.charges = vec![
        charge("expired-unknown", 0, None),
        charge("unknown-model", 12 * HOUR, None),
        charge("known-manual", 13 * HOUR, Some(7.0)),
    ];
    let result = simulate(&data).unwrap();
    assert_eq!(at(&result, 10 * HOUR).used_points, None);
    assert_eq!(at(&result, 11 * HOUR).used_points, Some(0.0));
    assert_eq!(at(&result, 12 * HOUR).used_points, None);
    assert_eq!(at(&result, 13 * HOUR).used_points, None);
    assert_eq!(at(&result, 17 * HOUR).used_points, Some(7.0));
    assert_eq!(at(&result, 18 * HOUR).used_points, Some(0.0));
    assert!(result.reasons.contains(&"charge_unknown".into()));
}

#[test]
fn over_capacity_clamps_remaining_without_clamping_spent_or_subtracting_independent_intervals() {
    let mut data = input();
    data.charges = vec![
        charge("a", 9 * HOUR, Some(90.0)),
        charge("b", 10 * HOUR, Some(30.0)),
    ];
    let result = simulate(&data).unwrap();
    assert_eq!(at(&result, 10 * HOUR).used_points, Some(120.0));
    assert_eq!(at(&result, 10 * HOUR).remaining_points, Some(0.0));
    assert_eq!(at(&result, 10 * HOUR).available, Some(false));
    assert_eq!(at(&result, 14 * HOUR).used_points, Some(30.0));
}

#[test]
fn common_availability_scans_future_recharges_instead_of_maximum_of_first_recoveries() {
    let base = RollingInput {
        as_of_ms: 0,
        history_complete_from_ms: Some(-10 * MINUTE),
        window_ms: 10 * MINUTE,
        horizon_ms: 20 * MINUTE,
        capacity_points: 10.0,
        required_points: 5.0,
        charges: vec![],
    };
    let mut a = base.clone();
    a.charges = vec![
        charge("old-a", -5 * MINUTE, Some(8.0)),
        charge("new-a", 7 * MINUTE, Some(8.0)),
    ];
    let mut b = base;
    b.charges = vec![charge("old-b", -MINUTE, Some(8.0))];
    assert_eq!(simulate(&a).unwrap().next_available_ms, Some(5 * MINUTE));
    assert_eq!(simulate(&b).unwrap().next_available_ms, Some(9 * MINUTE));
    let common = common_availability(&[a, b]).unwrap();
    assert_eq!(common.next_available_ms, Some(17 * MINUTE));
    assert_eq!(
        common.availability_windows,
        vec![AvailabilityWindow {
            from_ms: 17 * MINUTE,
            to_ms: 20 * MINUTE
        }]
    );
}

#[test]
fn maximum_events_include_frozen_end_and_history_boundary_within_260_points() {
    let data = RollingInput {
        as_of_ms: 0,
        history_complete_from_ms: None,
        window_ms: 1000,
        horizon_ms: 2000,
        capacity_points: 1000.0,
        required_points: 1.0,
        charges: (1..=128)
            .map(|i| charge(&format!("c-{i}"), i, Some(1.0)))
            .collect(),
    };
    let result = simulate(&data).unwrap();
    assert_eq!(result.points.len(), 259);
    assert_eq!(result.points.first().unwrap().at_ms, 0);
    assert_eq!(result.points.last().unwrap().at_ms, 2000);
    assert!(result
        .points
        .windows(2)
        .all(|pair| pair[0].at_ms < pair[1].at_ms));
    assert_eq!(simulate(&data).unwrap(), result, "重复读取不随墙钟推进情景");
}

#[test]
fn horizon_endpoint_can_be_first_available_without_claiming_a_future_open_interval() {
    let mut data = input();
    data.horizon_ms = 15 * HOUR;
    data.charges = vec![charge("current", 10 * HOUR, Some(80.0))];
    let result = simulate(&data).unwrap();
    assert_eq!(result.next_available_ms, Some(15 * HOUR));
    assert!(result.availability_windows.is_empty());
}

#[test]
fn invalid_inputs_duplicate_ids_and_overflow_return_errors_without_partial_zero_results() {
    let mut data = input();
    data.capacity_points = f64::NAN;
    assert!(simulate(&data).is_err());
    data = input();
    data.charges = vec![
        charge("same", 10 * HOUR, Some(1.0)),
        charge("same", 11 * HOUR, Some(2.0)),
    ];
    assert_eq!(simulate(&data).unwrap_err().0, "invalid_charge_id");
    data = input();
    data.charges = vec![charge("zero", 10 * HOUR, Some(0.0))];
    assert!(simulate(&data).is_err());
    data = input();
    data.charges = vec![charge("too-late", 15 * HOUR + 1, Some(1.0))];
    assert!(simulate(&data).is_err());
    data = input();
    data.as_of_ms = i64::MAX - 1;
    data.horizon_ms = i64::MAX;
    assert_eq!(simulate(&data).unwrap_err().0, "time_overflow");
    data = input();
    data.charges = (0..129)
        .map(|i| charge(&i.to_string(), 10 * HOUR, Some(1.0)))
        .collect();
    assert_eq!(simulate(&data).unwrap_err().0, "charge_limit");
    data = input();
    data.charges = vec![
        charge("a", 10 * HOUR, Some(f64::MAX)),
        charge("b", 10 * HOUR, Some(f64::MAX)),
    ];
    assert_eq!(simulate(&data).unwrap_err().0, "invalid_number");
}

#[test]
fn serialized_manual_result_and_unknown_values_cannot_grant_quota_proof() {
    let mut data = input();
    data.history_complete_from_ms = None;
    let value = serde_json::to_value(simulate(&data).unwrap()).unwrap();
    assert_eq!(value["quality"], "conditional-manual");
    assert_eq!(value["constraintsVerified"], false);
    assert_eq!(value["notifyEligible"], false);
    assert!(value["uncertaintyBounds"].is_null());
    assert!(
        value["points"][0]["usedPoints"].is_null() && value["points"][0]["available"].is_null()
    );
}
