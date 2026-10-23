//! Passive, profile-specific calibration. A declaration of local-only use never
//! becomes verified account coverage, even after a good numerical backtest.
use super::evaluate::{capacity_interval, percentage_bounds};
use super::model::*;
use std::collections::{BTreeMap, BTreeSet};

const MAX_EXACT_INTEGER: u64 = 9_007_199_254_740_991;

fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() || values.iter().any(|n| !n.is_finite()) {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    Some(if sorted.len().is_multiple_of(2) {
        sorted[middle - 1] / 2.0 + sorted[middle] / 2.0
    } else {
        sorted[middle]
    })
}

fn normalized_models(profile: &UsageProfile) -> Option<BTreeMap<&str, f64>> {
    if !profile.all_models_attributed
        || profile.models.is_empty()
        || !profile.output_share.is_finite()
        || !(0.0..=1.0).contains(&profile.output_share)
        || !profile.cache_share.is_finite()
        || !(0.0..=1.0).contains(&profile.cache_share)
        || !profile.median_input_tokens.is_finite()
        || profile.median_input_tokens <= 0.0
    {
        return None;
    }
    let mut models = BTreeMap::new();
    let mut total = 0.0;
    for model in &profile.models {
        if model.model.trim().is_empty()
            || !model.share.is_finite()
            || model.share <= 0.0
            || model.share > 1.0
            || models.insert(model.model.as_str(), model.share).is_some()
        {
            return None;
        }
        total += model.share;
    }
    ((total - 1.0).abs() <= 1e-9).then_some(models)
}

/// Model mixture weights are part of the stratum. No unverified tolerance is
/// invented for a change in model mixture or unknown effort/speed mode.
pub fn profiles_compatible(a: &UsageProfile, b: &UsageProfile) -> bool {
    let (Some(a_models), Some(b_models)) = (normalized_models(a), normalized_models(b)) else {
        return false;
    };
    a_models.len() == b_models.len()
        && a_models.iter().all(|(model, share)| {
            b_models
                .get(model)
                .is_some_and(|other| (share - other).abs() <= 1e-9)
        })
        && a.effort == b.effort
        && a.speed_mode == b.speed_mode
        && (a.output_share - b.output_share).abs() <= 0.1 + 1e-12
        && (a.cache_share - b.cache_share).abs() <= 0.1 + 1e-12
        && (0.8..=1.25).contains(&(b.median_input_tokens / a.median_input_tokens))
}

fn pair_time(pair: &CalibrationPair) -> Option<(Interval, Interval)> {
    let (a, b) = (pair.start.fact_time?, pair.end.fact_time?);
    (a.is_valid() && b.is_valid() && a.lower >= 0.0 && b.lower - a.upper > 0.0).then_some((a, b))
}

fn admit_pair(pair: &CalibrationPair) -> Result<(f64, Option<Interval>, Option<f64>), Reason> {
    if pair.evidence_invalidated {
        return Err(Reason::EvidenceInvalidated);
    }
    if pair.pair_id.trim().is_empty() || !pair.start.scope.is_valid() || !pair.end.scope.is_valid()
    {
        return Err(Reason::InvalidScope);
    }
    if pair.start.scope != pair.end.scope
        || pair.start.recovery != pair.end.recovery
        || pair.start.reset_time != pair.end.reset_time
    {
        return Err(Reason::ScopeChanged);
    }
    if !pair.start.identity_consistent
        || !pair.end.identity_consistent
        || pair.identity_evidence == IdentityEvidence::Unknown
    {
        return Err(Reason::IdentityUnproven);
    }
    if pair.identity_evidence == IdentityEvidence::VerifiedAccount
        && pair.start.scope.account_scope.is_none()
    {
        return Err(Reason::IdentityUnproven);
    }
    if !pair.start.persisted || !pair.end.persisted {
        return Err(Reason::NotPersisted);
    }
    if pair.start.sequence == 0
        || pair.end.sequence <= pair.start.sequence
        || pair.start.observation_id.is_empty()
        || pair.end.observation_id.is_empty()
        || pair.start.observation_id == pair.end.observation_id
    {
        return Err(Reason::OutOfOrder);
    }
    if pair.start.clock_epoch.is_empty() || pair.start.clock_epoch != pair.end.clock_epoch {
        return Err(Reason::ClockChanged);
    }
    if pair.start.discontinuity || pair.end.discontinuity {
        return Err(Reason::Discontinuous);
    }
    let (_, end_time) = pair_time(pair).ok_or(Reason::TimeUnknown)?;
    if pair
        .start
        .reset_time
        .is_some_and(|reset| !reset.is_valid() || end_time.upper >= reset.lower)
    {
        return Err(Reason::WaitingForReset);
    }
    if !pair.start.recovery.supports_single_reset_projection() {
        return Err(Reason::RecoveryUnknown);
    }
    if !pair.settled {
        return Err(Reason::Unsettled);
    }
    if pair.conflicts_or_gaps {
        return Err(Reason::ConflictsOrGaps);
    }
    match pair.coverage {
        CoverageEvidence::KnownIncomplete => return Err(Reason::ExternalUsageUncovered),
        CoverageEvidence::Unknown => return Err(Reason::CoverageUnknown),
        _ => {}
    }
    if normalized_models(&pair.profile).is_none() {
        return Err(Reason::ModelCoverageUnknown);
    }
    if pair.tokens == 0 || pair.tokens > MAX_EXACT_INTEGER {
        return Err(Reason::InvalidNumber);
    }
    let (a, b) = (&pair.start.percent, &pair.end.percent);
    if !a.used_percent.is_finite()
        || !b.used_percent.is_finite()
        || !(0.0..100.0).contains(&a.used_percent)
        || !(0.0..100.0).contains(&b.used_percent)
        || a.saturated
        || b.saturated
    {
        return Err(Reason::Saturated);
    }
    let delta = b.used_percent - a.used_percent;
    if delta <= 0.0 {
        return Err(Reason::LowSignal);
    }
    let precision = match (a.rounding_step, b.rounding_step) {
        (Some(a_q), Some(b_q)) => {
            percentage_bounds(a)?;
            percentage_bounds(b)?;
            Some(a_q.max(b_q))
        }
        _ => {
            // A present but invalid step must not be laundered into "unknown".
            for reading in [a, b] {
                if reading.rounding_step.is_some() {
                    percentage_bounds(reading)?;
                }
            }
            None
        }
    };
    if delta < precision.map_or(3.0, |q| (5.0 * q).max(3.0)) {
        return Err(Reason::LowSignal);
    }
    let point = 100.0 * pair.tokens as f64 / delta;
    if !point.is_finite() || point <= 0.0 {
        return Err(Reason::InvalidNumber);
    }
    let bounds = precision
        .map(|_| capacity_interval(a, b, pair.tokens))
        .transpose()?;
    Ok((point, bounds, precision))
}

struct Admitted<'a> {
    pair: &'a CalibrationPair,
    point: f64,
    bounds: Option<Interval>,
    q: Option<f64>,
}

pub fn calibrate(pairs: &[CalibrationPair], target_profile: &UsageProfile) -> CalibrationResult {
    let mut result = CalibrationResult {
        algorithm_version: ALGORITHM_VERSION.into(),
        quality: CalibrationQuality::Learning,
        capacity: None,
        empirical_capacity: None,
        train_pairs: 0,
        holdout_pairs: 0,
        independent_periods: 0,
        excluded_pairs: 0,
        relative_mad: None,
        holdout_wape: None,
        included_pair_ids: Vec::new(),
        excluded: Vec::new(),
        reasons: Vec::new(),
    };
    if normalized_models(target_profile).is_none() {
        result.reasons.push(Reason::ModelCoverageUnknown);
        return result;
    }
    let mut ordered: Vec<_> = pairs.iter().collect();
    ordered.sort_by(|a, b| {
        a.start
            .fact_time
            .map_or(f64::INFINITY, |t| t.lower)
            .total_cmp(&b.start.fact_time.map_or(f64::INFINITY, |t| t.lower))
            .then(a.pair_id.cmp(&b.pair_id))
    });
    let mut accepted: Vec<Admitted<'_>> = Vec::new();
    let mut ids = BTreeSet::new();
    for pair in ordered {
        let decision = admit_pair(pair).and_then(|values| {
            if !ids.insert(pair.pair_id.clone()) {
                return Err(Reason::OverlappingPairs);
            }
            if !profiles_compatible(target_profile, &pair.profile) {
                return Err(Reason::ProfileChanged);
            }
            if let Some(first) = accepted.first() {
                if !first
                    .pair
                    .start
                    .scope
                    .same_calibration_domain(&pair.start.scope)
                {
                    return Err(Reason::ScopeChanged);
                }
                if (pair.identity_evidence != IdentityEvidence::VerifiedAccount
                    || first.pair.identity_evidence != IdentityEvidence::VerifiedAccount)
                    && (pair.start.scope.runtime_id != first.pair.start.scope.runtime_id
                        || pair.start.scope.binding_generation
                            != first.pair.start.scope.binding_generation
                        || pair.start.scope.identity_epoch != first.pair.start.scope.identity_epoch
                        || pair.start.scope.clear_generation
                            != first.pair.start.scope.clear_generation)
                {
                    return Err(Reason::IdentityUnproven);
                }
            }
            if let Some(previous) = accepted.last() {
                let (start, _) = pair_time(pair).expect("admitted time");
                let (_, end) = pair_time(previous.pair).expect("admitted time");
                let shared = pair.start == previous.pair.end;
                if !shared && start.lower < end.upper {
                    return Err(Reason::OverlappingPairs);
                }
                if pair.start.scope == previous.pair.end.scope
                    && pair.start.percent.used_percent < previous.pair.end.percent.used_percent
                {
                    return Err(Reason::PercentDecreased);
                }
            }
            Ok(values)
        });
        match decision {
            Ok((point, bounds, q)) => accepted.push(Admitted {
                pair,
                point,
                bounds,
                q,
            }),
            Err(reason) => result.excluded.push((pair.pair_id.clone(), reason)),
        }
    }
    result.excluded_pairs = result.excluded.len();
    if pairs.iter().any(|p| p.evidence_invalidated) {
        result.quality = CalibrationQuality::Invalidated;
        result.reasons.push(Reason::EvidenceInvalidated);
        return result;
    }
    let train: Vec<_> = accepted.iter().filter(|p| !p.pair.holdout).collect();
    let holdout: Vec<_> = accepted.iter().filter(|p| p.pair.holdout).collect();
    result.train_pairs = train.len();
    result.holdout_pairs = holdout.len();
    result.included_pair_ids = accepted.iter().map(|p| p.pair.pair_id.clone()).collect();
    let mut periods: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    for item in &train {
        periods
            .entry(&item.pair.start.scope.window_instance_id)
            .or_default()
            .push(item.point);
    }
    result.independent_periods = periods.len();
    if train.len() < 3 {
        result.reasons.push(Reason::InsufficientSamples);
        return result;
    }
    let earliest_upper = train
        .iter()
        .map(|p| p.pair.start.fact_time.unwrap().upper)
        .fold(f64::INFINITY, f64::min);
    let latest_lower = train
        .iter()
        .map(|p| p.pair.end.fact_time.unwrap().lower)
        .fold(f64::NEG_INFINITY, f64::max);
    let all_precision_known = accepted.iter().all(|p| p.q.is_some());
    let max_q = accepted.iter().filter_map(|p| p.q).fold(0.0_f64, f64::max);
    let total_delta: f64 = train
        .iter()
        .map(|p| p.pair.end.percent.used_percent - p.pair.start.percent.used_percent)
        .sum();
    if latest_lower - earliest_upper < 900.0
        || total_delta
            < if all_precision_known {
                (15.0 * max_q).max(15.0)
            } else {
                15.0
            }
    {
        result.reasons.push(Reason::LowSignal);
        return result;
    }
    let cycle_points: Vec<_> = periods
        .values()
        .filter_map(|points| median(points))
        .collect();
    let Some(capacity) = median(&cycle_points) else {
        result.reasons.push(Reason::InvalidNumber);
        return result;
    };
    let deviations: Vec<_> = cycle_points
        .iter()
        .map(|point| (point - capacity).abs())
        .collect();
    result.relative_mad = median(&deviations).map(|mad| mad / capacity);
    result.empirical_capacity = Some(capacity);
    result.quality = CalibrationQuality::Conditional;
    // Retain an outer envelope, rather than treating empirical dispersion as a
    // confidence interval or shrinking it by counting repeated refreshes.
    if train.iter().all(|p| p.bounds.is_some()) {
        result.capacity = Interval::new(
            train
                .iter()
                .map(|p| p.bounds.unwrap().lower)
                .fold(f64::INFINITY, f64::min),
            train
                .iter()
                .map(|p| p.bounds.unwrap().upper)
                .fold(f64::NEG_INFINITY, f64::max),
        );
    }
    if !all_precision_known {
        result.reasons.push(Reason::PrecisionUnknown);
    }
    if accepted
        .iter()
        .any(|p| p.pair.coverage == CoverageEvidence::ConditionalUserAssumption)
    {
        result.reasons.push(Reason::ConditionalCoverage);
    }

    // Compare against the frozen training estimate. Retain all failed pairs.
    let mut consecutive_deviations = 0;
    for item in &accepted {
        let Some(q) = item.q else {
            consecutive_deviations = 0;
            continue;
        };
        let actual = item.pair.end.percent.used_percent - item.pair.start.percent.used_percent;
        let predicted = 100.0 * item.pair.tokens as f64 / capacity;
        if (predicted - actual).abs() > (2.0 * q).max(actual * 0.3) {
            consecutive_deviations += 1;
            if consecutive_deviations >= 2 {
                result.quality = CalibrationQuality::Invalidated;
                result.capacity = None;
                result.empirical_capacity = None;
                result.reasons.push(Reason::SuspectedChangePoint);
                return result;
            }
        } else {
            consecutive_deviations = 0;
        }
    }
    let max_train_end = train
        .iter()
        .map(|p| p.pair.end.fact_time.unwrap().upper)
        .fold(f64::NEG_INFINITY, f64::max);
    let held_after_train = holdout
        .iter()
        .all(|p| p.pair.start.fact_time.unwrap().lower >= max_train_end);
    if !held_after_train {
        result.reasons.push(Reason::HoldoutInvalid);
    }
    if !holdout.is_empty() {
        let denominator: f64 = holdout
            .iter()
            .map(|p| p.pair.end.percent.used_percent - p.pair.start.percent.used_percent)
            .sum();
        let numerator: f64 = holdout
            .iter()
            .map(|p| {
                let actual = p.pair.end.percent.used_percent - p.pair.start.percent.used_percent;
                (100.0 * p.pair.tokens as f64 / capacity - actual).abs()
            })
            .sum();
        if denominator > 0.0 && numerator.is_finite() && denominator.is_finite() {
            result.holdout_wape = Some(numerator / denominator);
        }
    }
    let verified = accepted.iter().all(|p| {
        p.pair.identity_evidence == IdentityEvidence::VerifiedAccount
            && p.pair.start.scope.account_scope.is_some()
            && p.pair.coverage == CoverageEvidence::VerifiedComplete
    });
    if !verified {
        result.reasons.push(Reason::IdentityUnproven);
    }
    if periods.len() < 3 {
        result.reasons.push(Reason::InsufficientPeriods);
    }
    let backtest_ok = result.relative_mad.is_some_and(|v| v <= 0.2)
        && result.holdout_wape.is_some_and(|v| v <= 0.2);
    if result.holdout_wape.is_some() && !backtest_ok {
        result.reasons.push(Reason::BacktestFailed);
    }
    if all_precision_known
        && verified
        && periods.len() >= 3
        && train.len() >= 20
        && holdout.len() >= 5
        && held_after_train
        && backtest_ok
    {
        result.quality = CalibrationQuality::Validated;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::super::model::fixtures::*;
    use super::*;

    fn conditional_pairs() -> Vec<CalibrationPair> {
        (0..3)
            .map(|i| pair(i, i as f64 * 300.0, 10.0 + i as f64 * 10.0))
            .collect()
    }
    fn longitudinal() -> Vec<CalibrationPair> {
        let mut pairs = Vec::new();
        for (period, train_count) in [7, 7, 6].into_iter().enumerate() {
            let rows = train_count + if period == 2 { 5 } else { 0 };
            for i in 0..rows {
                let index = pairs.len() as u64;
                let base = period as f64 * 10000.0;
                let mut p = pair(index, base + i as f64 * 300.0, 10.0 + i as f64 * 5.0);
                p.end.percent.used_percent = p.start.percent.used_percent + 5.0;
                p.tokens = 50000;
                p.start.scope.window_instance_id = format!("window-{period}");
                p.end.scope = p.start.scope.clone();
                p.start.reset_time = Interval::exact(base + 9000.0);
                p.end.reset_time = p.start.reset_time;
                p.holdout = i >= train_count;
                pairs.push(p);
            }
        }
        pairs
    }

    #[test]
    fn three_nonoverlapping_pairs_only_unlock_conditional_reference() {
        let result = calibrate(&conditional_pairs(), &profile());
        assert_eq!(result.quality, CalibrationQuality::Conditional);
        assert_eq!((result.train_pairs, result.independent_periods), (3, 1));
        close(result.empirical_capacity.unwrap(), 1000000.0);
        let capacity = result.capacity.unwrap();
        close(capacity.lower, 10000000.0 / 11.0);
        close(capacity.upper, 10000000.0 / 9.0);
    }

    #[test]
    fn user_specified_scope_does_not_automatically_join_runtime_starts() {
        let mut pairs = conditional_pairs();
        for (i, pair) in pairs.iter_mut().enumerate() {
            pair.identity_evidence = IdentityEvidence::UserSpecified;
            pair.start.scope.runtime_id = format!("runtime-{i}");
            pair.end.scope.runtime_id = pair.start.scope.runtime_id.clone();
        }
        let result = calibrate(&pairs, &profile());
        assert_eq!(result.quality, CalibrationQuality::Learning);
        assert_eq!(result.train_pairs, 1);
        assert!(result.capacity.is_none());
        assert!(result
            .excluded
            .iter()
            .all(|(_, reason)| *reason == Reason::IdentityUnproven));
    }

    #[test]
    fn validated_requires_real_evidence_flags_three_periods_twenty_train_and_five_heldout() {
        let pairs = longitudinal();
        let result = calibrate(&pairs, &profile());
        assert_eq!(
            (
                result.train_pairs,
                result.holdout_pairs,
                result.independent_periods
            ),
            (20, 5, 3)
        );
        assert_eq!(result.quality, CalibrationQuality::Validated);
        assert_eq!(result.relative_mad, Some(0.0));
        assert_eq!(result.holdout_wape, Some(0.0));
        assert_eq!(
            calibrate(&pairs[..24], &profile()).quality,
            CalibrationQuality::Conditional
        );
    }

    #[test]
    fn local_only_declaration_never_upgrades_account_coverage() {
        let mut pairs = longitudinal();
        for p in &mut pairs {
            p.coverage = CoverageEvidence::ConditionalUserAssumption;
        }
        let result = calibrate(&pairs, &profile());
        assert_eq!(result.quality, CalibrationQuality::Conditional);
        assert!(result.reasons.contains(&Reason::ConditionalCoverage));
        for p in &mut pairs {
            p.coverage = CoverageEvidence::KnownIncomplete;
        }
        let result = calibrate(&pairs, &profile());
        assert_eq!(result.quality, CalibrationQuality::Learning);
        assert!(result.capacity.is_none() && result.empirical_capacity.is_none());
        assert!(result
            .excluded
            .iter()
            .all(|(_, reason)| *reason == Reason::ExternalUsageUncovered));
    }

    #[test]
    fn unknown_precision_is_an_explicit_empirical_scenario_without_bounded_capacity() {
        let mut pairs = longitudinal();
        for p in &mut pairs {
            p.start.percent.rounding_step = None;
            p.end.percent.rounding_step = None;
        }
        let result = calibrate(&pairs, &profile());
        assert_eq!(result.quality, CalibrationQuality::Conditional);
        assert!(result.capacity.is_none());
        assert_eq!(result.empirical_capacity, Some(1000000.0));
        assert!(result.reasons.contains(&Reason::PrecisionUnknown));
    }

    #[test]
    fn overlap_repeated_pairs_and_same_window_declines_do_not_inflate_samples() {
        let mut pairs = conditional_pairs();
        pairs.push(pairs[0].clone());
        let mut overlapping = pair(100, 150.0, 15.0);
        overlapping.end.percent.used_percent = 25.0;
        pairs.push(overlapping);
        let result = calibrate(&pairs, &profile());
        assert_eq!(result.train_pairs, 3);
        assert_eq!(result.excluded_pairs, 2);
        assert!(result
            .excluded
            .iter()
            .all(|(_, r)| *r == Reason::OverlappingPairs));
        pairs = conditional_pairs();
        pairs[2].start.percent.used_percent = 15.0;
        pairs[2].end.percent.used_percent = 25.0;
        assert!(calibrate(&pairs, &profile())
            .excluded
            .iter()
            .any(|(_, r)| *r == Reason::PercentDecreased));
    }

    #[test]
    fn profile_strata_reject_effort_speed_cache_input_and_model_mixture_changes() {
        let original = profile();
        let mut changes = Vec::new();
        let mut p = original.clone();
        p.effort = Some("high".into());
        changes.push(p);
        let mut p = original.clone();
        p.speed_mode = Some("fast".into());
        changes.push(p);
        let mut p = original.clone();
        p.cache_share = 0.41;
        changes.push(p);
        let mut p = original.clone();
        p.output_share = 0.31;
        changes.push(p);
        let mut p = original.clone();
        p.median_input_tokens = 12501.0;
        changes.push(p);
        let mut p = original.clone();
        p.models[0].share = 0.95;
        p.models.push(ModelShare {
            model: "synthetic-model-b".into(),
            share: 0.05,
        });
        changes.push(p);
        for changed in changes {
            assert!(!profiles_compatible(&original, &changed));
        }
        let mut boundary = original.clone();
        boundary.cache_share += 0.1;
        boundary.median_input_tokens = 12500.0;
        assert!(profiles_compatible(&original, &boundary));
        let mut unknown = original.clone();
        unknown.effort = None;
        assert!(!profiles_compatible(&original, &unknown));
        assert!(profiles_compatible(&unknown, &unknown));
    }

    #[test]
    fn unknown_actual_model_or_account_identity_cannot_be_promoted() {
        let mut pairs = longitudinal();
        for p in &mut pairs {
            p.profile.all_models_attributed = false;
        }
        assert_eq!(calibrate(&pairs, &profile()).train_pairs, 0);
        pairs = longitudinal();
        for p in &mut pairs {
            p.start.scope.account_scope = None;
            p.end.scope.account_scope = None;
            p.identity_evidence = IdentityEvidence::RuntimeOnly;
        }
        assert_eq!(
            calibrate(&pairs, &profile()).quality,
            CalibrationQuality::Conditional
        );
        pairs[7].start.scope.runtime_id = "new-runtime".into();
        pairs[7].end.scope.runtime_id = "new-runtime".into();
        let result = calibrate(&pairs, &profile());
        assert!(result
            .excluded
            .iter()
            .any(|(_, reason)| *reason == Reason::IdentityUnproven));
        assert_ne!(result.quality, CalibrationQuality::Validated);
    }

    #[test]
    fn holdout_must_be_later_than_all_training_data() {
        let mut pairs = longitudinal();
        pairs[0].holdout = true;
        pairs[20].holdout = false;
        let result = calibrate(&pairs, &profile());
        assert_eq!(result.quality, CalibrationQuality::Conditional);
        assert!(result.reasons.contains(&Reason::HoldoutInvalid));
    }

    #[test]
    fn failed_holdout_is_retained_and_cannot_qualify_with_a_good_mad() {
        let mut pairs = longitudinal();
        for (i, pair) in pairs.iter_mut().filter(|p| p.holdout).enumerate() {
            pair.start.percent.used_percent = 40.0 + i as f64 * 7.0;
            pair.end.percent.used_percent = pair.start.percent.used_percent + 7.0;
        }
        let result = calibrate(&pairs, &profile());
        assert_eq!(result.quality, CalibrationQuality::Conditional);
        assert_eq!(result.relative_mad, Some(0.0));
        assert!(result.holdout_wape.unwrap() > 0.2);
        assert!(result.reasons.contains(&Reason::BacktestFailed));
        assert_eq!(result.holdout_pairs, 5);
        assert!(result.excluded.is_empty());
    }

    #[test]
    fn two_independent_large_deviations_stop_old_capacity_guidance() {
        let mut pairs = longitudinal();
        for (i, pair) in pairs.iter_mut().filter(|p| p.holdout).enumerate() {
            pair.start.percent.used_percent = 40.0 + i as f64 * 10.0;
            pair.end.percent.used_percent = pair.start.percent.used_percent + 10.0;
        }
        let result = calibrate(&pairs, &profile());
        assert_eq!(result.quality, CalibrationQuality::Invalidated);
        assert!(result.reasons.contains(&Reason::SuspectedChangePoint));
        assert!(result.capacity.is_none() && result.empirical_capacity.is_none());
        assert_eq!(result.holdout_pairs, 5);
    }

    #[test]
    fn invalidated_or_unsettled_evidence_is_not_silently_reused() {
        let mut pairs = conditional_pairs();
        pairs[0].settled = false;
        assert_eq!(
            calibrate(&pairs, &profile()).quality,
            CalibrationQuality::Learning
        );
        pairs[0].settled = true;
        pairs[0].evidence_invalidated = true;
        let result = calibrate(&pairs, &profile());
        assert_eq!(result.quality, CalibrationQuality::Invalidated);
        assert!(result.capacity.is_none());
    }

    #[test]
    fn different_window_within_pair_nan_precision_and_overflow_are_rejected() {
        let mut pairs = conditional_pairs();
        pairs[0].end.scope.window_instance_id = "other-window".into();
        pairs[1].start.percent.rounding_step = Some(f64::NAN);
        pairs[2].tokens = u64::MAX;
        let result = calibrate(&pairs, &profile());
        assert_eq!(result.train_pairs, 0);
        assert!(result.capacity.is_none());
    }
}
