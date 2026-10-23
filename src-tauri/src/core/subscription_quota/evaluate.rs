//! Deterministic interval arithmetic and native-window evaluation.
use super::model::*;

const MAX_EXACT_INTEGER: u64 = 9_007_199_254_740_991;

fn valid_percent(value: f64) -> bool {
    value.is_finite() && (0.0..=100.0).contains(&value)
}

pub fn percentage_bounds(reading: &PercentReading) -> Result<Interval, Reason> {
    if !valid_percent(reading.used_percent) {
        return Err(Reason::InvalidNumber);
    }
    if (reading.saturated || reading.used_percent == 100.0) && !reading.saturation_headroom_bounded
    {
        return Err(Reason::Saturated);
    }
    let q = reading.rounding_step.ok_or(Reason::PrecisionUnknown)?;
    if !q.is_finite() || q <= 0.0 || q > 100.0 {
        return Err(Reason::InvalidNumber);
    }
    Interval::new(
        (reading.used_percent - q / 2.0).max(0.0),
        (reading.used_percent + q / 2.0).min(100.0),
    )
    .ok_or(Reason::InvalidNumber)
}

pub fn remaining_percentage(reading: &PercentReading) -> Result<Interval, Reason> {
    let p = percentage_bounds(reading)?;
    Interval::new((100.0 - p.upper).max(0.0), (100.0 - p.lower).max(0.0))
        .ok_or(Reason::InvalidNumber)
}

pub fn percentage_delta(a: &PercentReading, b: &PercentReading) -> Result<Interval, Reason> {
    if a.saturated || b.saturated || a.used_percent == 100.0 || b.used_percent == 100.0 {
        return Err(Reason::Saturated);
    }
    if b.used_percent < a.used_percent {
        return Err(Reason::PercentDecreased);
    }
    let (a, b) = (percentage_bounds(a)?, percentage_bounds(b)?);
    let delta = Interval::new(b.lower - a.upper, b.upper - a.lower).ok_or(Reason::InvalidNumber)?;
    if delta.lower <= 0.0 {
        return Err(Reason::ZeroDenominator);
    }
    Ok(delta)
}

/// Formula only. Calibration admission (coverage, identities, samples) is separate.
pub fn capacity_interval(
    a: &PercentReading,
    b: &PercentReading,
    tokens: u64,
) -> Result<Interval, Reason> {
    if tokens == 0 || tokens > MAX_EXACT_INTEGER {
        return Err(Reason::InvalidNumber);
    }
    let delta = percentage_delta(a, b)?;
    Interval::new(
        100.0 * tokens as f64 / delta.upper,
        100.0 * tokens as f64 / delta.lower,
    )
    .ok_or(Reason::InvalidNumber)
}

pub fn remaining_tokens(capacity: Interval, current: &PercentReading) -> Result<Interval, Reason> {
    if !capacity.is_valid() || capacity.lower < 0.0 {
        return Err(Reason::InvalidNumber);
    }
    let remaining = remaining_percentage(current)?;
    Interval::new(
        capacity.lower * remaining.lower / 100.0,
        capacity.upper * remaining.upper / 100.0,
    )
    .ok_or(Reason::InvalidNumber)
}

pub fn compare_exhaustion_to_reset(exhaustion: Interval, reset: Interval) -> Option<bool> {
    if !exhaustion.is_valid() || !reset.is_valid() {
        return None;
    }
    // Binary arithmetic / Unix-second conversion must not turn a decimal
    // boundary equality into an apparently strict ordering. This only widens
    // the indeterminate region; it never repairs a zero rate or time divisor.
    let scale = exhaustion
        .lower
        .abs()
        .max(exhaustion.upper.abs())
        .max(reset.lower.abs())
        .max(reset.upper.abs())
        .max(1.0);
    let representation_guard = scale * f64::EPSILON * 32.0;
    if reset.lower - exhaustion.upper > representation_guard {
        Some(true)
    } else if exhaustion.lower - reset.upper > representation_guard {
        Some(false)
    } else {
        None
    }
}

fn params_valid(p: &NativeParams) -> bool {
    [
        p.ttl_seconds,
        p.min_span_seconds,
        p.max_gap_seconds,
        p.preferred_history_seconds,
        p.max_history_seconds,
        p.max_change_age_seconds,
    ]
    .into_iter()
    .all(|n| n.is_finite() && n > 0.0)
        && p.min_samples >= 2
        && p.min_distinct_readings >= 2
        && p.preferred_history_seconds <= p.max_history_seconds
}

fn sample_time(sample: &NativeSample, now: f64) -> Result<Interval, Reason> {
    if !sample.scope.is_valid() || sample.observation_id.trim().is_empty() {
        return Err(Reason::InvalidScope);
    }
    if !sample.identity_consistent {
        return Err(Reason::IdentityUnproven);
    }
    if !sample.persisted {
        return Err(Reason::NotPersisted);
    }
    if sample.sequence == 0 {
        return Err(Reason::OutOfOrder);
    }
    if !valid_percent(sample.percent.used_percent) {
        return Err(Reason::InvalidNumber);
    }
    if sample.clock_epoch.is_empty() {
        return Err(Reason::ClockChanged);
    }
    if !sample.ttl_start.is_finite() || sample.ttl_start < 0.0 || sample.ttl_start > now {
        return Err(Reason::TimeInvalid);
    }
    let t = sample.fact_time.ok_or(Reason::TimeUnknown)?;
    if !t.is_valid() || t.lower < 0.0 || t.upper > now {
        return Err(Reason::TimeInvalid);
    }
    Ok(t)
}

fn baseline_admitted(samples: &[&NativeSample], p: &NativeParams) -> Result<(), Reason> {
    if samples.len() < p.min_samples {
        return Err(Reason::InsufficientSamples);
    }
    let first = samples[0];
    let last = samples[samples.len() - 1];
    let distinct = samples
        .iter()
        .fold(Vec::<f64>::new(), |mut values, s| {
            if !values.contains(&s.percent.used_percent) {
                values.push(s.percent.used_percent);
            }
            values
        })
        .len();
    if distinct < p.min_distinct_readings {
        return Err(Reason::LowSignal);
    }
    let a = first.fact_time.ok_or(Reason::TimeUnknown)?;
    let b = last.fact_time.ok_or(Reason::TimeUnknown)?;
    if b.lower - a.upper < p.min_span_seconds {
        return Err(Reason::LowSignal);
    }
    let max_q = samples.iter().try_fold(0.0_f64, |q, s| {
        let next = s.percent.rounding_step.ok_or(Reason::PrecisionUnknown)?;
        percentage_bounds(&s.percent)?;
        Ok::<f64, Reason>(q.max(next))
    })?;
    if last.percent.used_percent - first.percent.used_percent < (5.0 * max_q).max(3.0) {
        return Err(Reason::LowSignal);
    }
    Ok(())
}

/// Conservative rectangular envelope; shared b/c variables are never assumed
/// statistically independent. This function may widen but never narrow them.
fn project(
    samples: &[&NativeSample],
    current: &NativeSample,
) -> Result<(Interval, Interval, Interval), Reason> {
    let first = samples[0];
    let last = samples[samples.len() - 1];
    let (a, b, c) = (
        first.fact_time.ok_or(Reason::TimeUnknown)?,
        last.fact_time.ok_or(Reason::TimeUnknown)?,
        current.fact_time.ok_or(Reason::TimeUnknown)?,
    );
    let time = Interval::new(b.lower - a.upper, b.upper - a.lower).ok_or(Reason::TimeInvalid)?;
    if time.lower <= 0.0 {
        return Err(Reason::ZeroDenominator);
    }
    let delta = percentage_delta(&first.percent, &last.percent)?;
    let speed = Interval::new(delta.lower / time.upper, delta.upper / time.lower)
        .ok_or(Reason::InvalidNumber)?;
    let remaining = remaining_percentage(&current.percent)?;
    let eta = Interval::new(remaining.lower / speed.upper, remaining.upper / speed.lower)
        .ok_or(Reason::InvalidNumber)?;
    let exhaustion =
        Interval::new(c.lower + eta.lower, c.upper + eta.upper).ok_or(Reason::InvalidNumber)?;
    Ok((speed, eta, exhaustion))
}

pub fn evaluate_native(
    samples: &[NativeSample],
    current: &NativeSample,
    now: f64,
    params: &NativeParams,
) -> NativeForecast {
    let mut result = NativeForecast::default();
    macro_rules! reject {
        ($reason:expr) => {{
            result.reasons.push($reason);
            return result;
        }};
    }
    if !now.is_finite() || now < 0.0 || !params_valid(params) {
        reject!(Reason::InvalidNumber);
    }
    let current_time = match sample_time(current, now) {
        Ok(t) => t,
        Err(reason) => reject!(reason),
    };
    if now - current.ttl_start > params.ttl_seconds || now - current_time.lower > params.ttl_seconds
    {
        reject!(Reason::Stale);
    }
    if current.discontinuity {
        reject!(Reason::Discontinuous);
    }
    if let Some(reset) = current.reset_time {
        if !reset.is_valid() || reset.lower < 0.0 {
            reject!(Reason::TimeInvalid);
        }
        result.reset_time = Some(reset);
        if now >= reset.lower {
            reject!(Reason::WaitingForReset);
        }
    }
    if !current.recovery.supports_single_reset_projection() || current.reset_time.is_none() {
        reject!(Reason::RecoveryUnknown);
    }

    let mut continuous: Vec<&NativeSample> = Vec::new();
    let mut last_sequence = None;
    for sample in samples {
        // Input order is collector order. Sorting would silently repair replay.
        if let Some(sequence) = last_sequence {
            if sample.scope.runtime_id == current.scope.runtime_id && sample.sequence <= sequence {
                reject!(Reason::OutOfOrder);
            }
        }
        if sample.scope.runtime_id == current.scope.runtime_id {
            last_sequence = Some(sample.sequence);
        }
        let t = match sample_time(sample, now) {
            Ok(t) => t,
            Err(_) => {
                continuous.clear();
                result.excluded_count += 1;
                continue;
            }
        };
        if sample.scope != current.scope
            || sample.clock_epoch != current.clock_epoch
            || sample.recovery != current.recovery
            || sample.reset_time != current.reset_time
        {
            continuous.clear();
            result.excluded_count += 1;
            continue;
        }
        if now - t.lower > params.max_history_seconds {
            result.excluded_count += 1;
            continue;
        }
        if sample.discontinuity {
            continuous.clear();
        }
        if let Some(previous) = continuous.last() {
            let previous_time = previous.fact_time.expect("validated sample");
            if sample.percent.used_percent < previous.percent.used_percent
                || t.lower < previous_time.lower
                || t.upper < previous_time.upper
                || t.upper - previous_time.lower > params.max_gap_seconds
            {
                continuous.clear();
            }
        }
        continuous.push(sample);
    }
    let last = match continuous.last() {
        Some(last) => *last,
        None => reject!(Reason::InsufficientSamples),
    };
    result.shared_endpoint = last.observation_id == current.observation_id;
    if result.shared_endpoint {
        if last != current {
            reject!(Reason::OutOfOrder);
        }
    } else {
        if current.sequence <= last.sequence {
            reject!(Reason::OutOfOrder);
        }
        let last_time = last.fact_time.expect("validated sample");
        if current_time.upper - last_time.lower > params.max_gap_seconds
            || current_time.upper < last_time.lower
        {
            reject!(Reason::Discontinuous);
        }
        if current.percent.used_percent < last.percent.used_percent {
            reject!(Reason::PercentDecreased);
        }
    }
    let preferred: Vec<_> = continuous
        .iter()
        .copied()
        .filter(|s| {
            now - s.fact_time.expect("validated sample").lower <= params.preferred_history_seconds
        })
        .collect();
    let chosen = if baseline_admitted(&preferred, params).is_ok() {
        preferred
    } else {
        if let Err(reason) = baseline_admitted(&continuous, params) {
            reject!(reason);
        }
        result.longer_baseline = continuous.first().is_some_and(|s| {
            now - s.fact_time.expect("validated sample").lower > params.preferred_history_seconds
        });
        continuous
    };
    let (speed, eta, exhaustion) = match project(&chosen, current) {
        Ok(values) => values,
        Err(reason) => reject!(reason),
    };
    result.sample_count = chosen.len();
    result.input_observation_ids = chosen.iter().map(|s| s.observation_id.clone()).collect();
    if !result.shared_endpoint {
        result
            .input_observation_ids
            .push(current.observation_id.clone());
    }
    result.speed_percent_per_second = Some(speed);
    result.eta_seconds = Some(eta);
    result.exhaustion_time = Some(exhaustion);
    let reset = current.reset_time.expect("checked reset");
    result.will_exhaust_before_reset = compare_exhaustion_to_reset(exhaustion, reset);
    let last_change_time =
        if !result.shared_endpoint && current.percent.used_percent > last.percent.used_percent {
            Some(current_time.lower)
        } else {
            chosen
                .windows(2)
                .rev()
                .find(|pair| pair[1].percent.used_percent > pair[0].percent.used_percent)
                .map(|pair| pair[1].fact_time.expect("validated sample").lower)
        };
    if last_change_time.is_none_or(|time| now - time > params.max_change_age_seconds) {
        result.reasons.push(Reason::ActivityStale);
        return result;
    }
    // The reported primary interval uses the longest admitted baseline. A
    // shorter admitted scenario must not be silently ignored by notification.
    let scenarios: Vec<_> = (0..chosen.len())
        .filter_map(|start| {
            let segment = &chosen[start..];
            baseline_admitted(segment, params).ok()?;
            // A scenario which passes the sampling gate but cannot be bounded
            // is uncertain; dropping it would manufacture consensus.
            Some(
                project(segment, current)
                    .ok()
                    .and_then(|(_, _, e)| compare_exhaustion_to_reset(e, reset)),
            )
        })
        .collect();
    let all_scenarios_before_reset =
        !scenarios.is_empty() && scenarios.iter().all(|before| *before == Some(true));
    let all_scenarios_after_reset =
        !scenarios.is_empty() && scenarios.iter().all(|before| *before == Some(false));
    result.notify_candidate = result.will_exhaust_before_reset == Some(true)
        && all_scenarios_before_reset
        && exhaustion.upper > now;
    result.safe_candidate = result.will_exhaust_before_reset == Some(false)
        && all_scenarios_after_reset
        && exhaustion.lower > now;
    if !result.notify_candidate && !result.safe_candidate {
        result.reasons.push(Reason::ResetUncertain);
    }
    if exhaustion.upper <= now {
        result.reasons.push(Reason::Stale);
    }
    result
}

/// No boundary is inferred from duration, percentage decrease, or local time.
pub fn classify_reset(
    old_window: &str,
    old_end: Option<Interval>,
    evidence: Option<&ResetEvidence>,
    now: f64,
) -> ResetDecision {
    let mut decision = ResetDecision {
        status: WindowStatus::Unknown,
        window_instance_id: old_window.into(),
        resolved_end: old_end.filter(|t| t.is_valid()),
        ended_previous_window: false,
        previous_usage_partial: false,
    };
    if !now.is_finite()
        || now < 0.0
        || old_window.is_empty()
        || old_end.is_some_and(|t| !t.is_valid() || t.lower < 0.0)
    {
        return decision;
    }
    let expired = old_end.is_some_and(|t| now >= t.lower);
    decision.status = if expired {
        WindowStatus::WaitingForReset
    } else {
        WindowStatus::Ready
    };
    let Some(e) = evidence else {
        return decision;
    };
    if e.confirmed_new_window && e.confirmed_same_window
        || e.next_reset.is_some_and(|t| !t.is_valid() || t.lower < 0.0)
        || e.actual_boundary
            .is_some_and(|t| !t.is_valid() || t.lower < 0.0 || t.upper > now)
    {
        decision.status = WindowStatus::Unknown;
        return decision;
    }
    if e.confirmed_new_window {
        let Some(new_id) = e
            .window_instance_id
            .as_ref()
            .filter(|id| !id.is_empty() && *id != old_window)
        else {
            return decision;
        };
        decision.window_instance_id = new_id.clone();
        decision.resolved_end = e.next_reset;
        decision.ended_previous_window = true;
        decision.previous_usage_partial = e.actual_boundary.is_none_or(|t| t.lower != t.upper);
        decision.status = if e.next_reset.is_some_and(|t| now >= t.lower) {
            WindowStatus::WaitingForReset
        } else {
            WindowStatus::Ready
        };
    } else if e.confirmed_same_window
        && e.window_instance_id
            .as_deref()
            .is_none_or(|id| id == old_window)
    {
        if let Some(end) = e.next_reset {
            decision.resolved_end = Some(end);
            decision.status = if now >= end.lower {
                WindowStatus::WaitingForReset
            } else {
                WindowStatus::Ready
            };
        }
    }
    decision
}

pub fn summarize_windows(windows: &[WindowConstraint]) -> MultiWindowSummary {
    let forecasts: Vec<_> = windows
        .iter()
        .filter(|w| w.required)
        .filter_map(|w| {
            w.forecast
                .exhaustion_time
                .filter(|t| t.is_valid())
                .map(|t| (w, t))
        })
        .collect();
    let earliest = if forecasts.is_empty() {
        None
    } else {
        Interval::new(
            forecasts
                .iter()
                .map(|(_, t)| t.lower)
                .fold(f64::INFINITY, f64::min),
            forecasts
                .iter()
                .map(|(_, t)| t.upper)
                .fold(f64::INFINITY, f64::min),
        )
    };
    let limiting_pools = earliest.map_or_else(Vec::new, |e| {
        forecasts
            .iter()
            .filter(|(_, t)| t.lower <= e.upper)
            .map(|(w, _)| w.pool_id.clone())
            .collect()
    });
    let limited: Vec<_> = windows
        .iter()
        .filter(|w| w.required && w.currently_limited)
        .collect();
    let restoration = if limited.is_empty()
        || limited
            .iter()
            .any(|w| w.restoration_time.is_none_or(|t| !t.is_valid()))
    {
        None
    } else {
        Interval::new(
            limited
                .iter()
                .map(|w| w.restoration_time.unwrap().lower)
                .fold(f64::NEG_INFINITY, f64::max),
            limited
                .iter()
                .map(|w| w.restoration_time.unwrap().upper)
                .fold(f64::NEG_INFINITY, f64::max),
        )
    };
    MultiWindowSummary {
        earliest_limit: earliest,
        limiting_pools,
        all_required_restored_at: restoration,
        constraints_verified: windows.iter().any(|w| w.required)
            && windows
                .iter()
                .filter(|w| w.required)
                .all(|w| w.forecast.exhaustion_time.is_some_and(|t| t.is_valid())),
    }
}

pub fn reserved_native_target(used: f64, reserve_fraction: f64) -> Option<f64> {
    if !valid_percent(used)
        || !reserve_fraction.is_finite()
        || !(0.0..=1.0).contains(&reserve_fraction)
    {
        return None;
    }
    Some(used + (100.0 - used) * (1.0 - reserve_fraction))
}

pub fn check_native_constraints(
    constraints: &[NativeBudgetConstraint],
    reserve_fraction: f64,
) -> Result<(), Reason> {
    if !reserve_fraction.is_finite() || !(0.0..=1.0).contains(&reserve_fraction) {
        return Err(Reason::InvalidNumber);
    }
    if constraints.is_empty() {
        return Err(Reason::ConstraintsUnverified);
    }
    let mut totals = std::collections::BTreeMap::<&str, (f64, f64)>::new();
    for pool in constraints {
        let (Some(remaining), Some(expected)) = (pool.remaining, pool.expected_consumption) else {
            return Err(Reason::ConstraintsUnverified);
        };
        if pool.pool_id.trim().is_empty()
            || !remaining.is_finite()
            || !expected.is_finite()
            || remaining < 0.0
            || expected < 0.0
        {
            return Err(Reason::InvalidNumber);
        }
        let entry = totals.entry(&pool.pool_id).or_insert((remaining, 0.0));
        if entry.0 != remaining {
            return Err(Reason::ConstraintsUnverified);
        }
        entry.1 += expected;
        if !entry.1.is_finite() {
            return Err(Reason::InvalidNumber);
        }
    }
    for (remaining, expected) in totals.values() {
        if *expected > *remaining * (1.0 - reserve_fraction) {
            return Err(Reason::BudgetExceeded);
        }
    }
    Ok(())
}

pub fn demand_draft(history: &[DemandInterval]) -> Option<DemandDraft> {
    let mut amounts: Vec<_> = history
        .iter()
        .filter(|d| d.covered && d.comparable)
        .map(|d| d.amount)
        .collect();
    if amounts.len() < 5 || amounts.iter().any(|n| !n.is_finite() || *n < 0.0) {
        return None;
    }
    amounts.sort_by(f64::total_cmp);
    // Nearest-rank quantiles: a zero is admitted only by an explicit covered row.
    let quantile = |q: f64| amounts[(q * amounts.len() as f64).ceil() as usize - 1];
    Some(DemandDraft {
        target_p50: quantile(0.5),
        reference_p90: quantile(0.9),
        samples: amounts.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::super::model::fixtures::*;
    use super::*;

    fn recent() -> Vec<NativeSample> {
        (0..6)
            .map(|i| exact_sample(i + 1, i as f64 * 180.0, 69.0 + i as f64 * 6.0))
            .collect()
    }
    fn forecast(samples: &[NativeSample]) -> NativeForecast {
        let current = samples.last().unwrap();
        evaluate_native(
            samples,
            current,
            current.fact_time.unwrap().upper,
            &NativeParams::default(),
        )
    }

    #[test]
    fn all_admitted_scenarios_after_reset_are_a_safe_candidate_without_false_uncertainty() {
        let samples: Vec<_> = (0..6)
            .map(|i| {
                let mut sample = exact_sample(i + 1, i as f64 * 120.0, 20.0 + i as f64);
                sample.reset_time = Interval::exact(960.0);
                sample
            })
            .collect();
        let result = forecast(&samples);
        assert_eq!(result.will_exhaust_before_reset, Some(false));
        assert!(result.safe_candidate);
        assert!(!result.notify_candidate);
        assert!(!result.reasons.contains(&Reason::ResetUncertain));
    }

    #[test]
    fn primary_safe_does_not_resolve_a_shorter_admitted_risk_scenario() {
        let samples: Vec<_> = [
            10.0, 10.0, 10.0, 10.0, 10.0, 10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0,
        ]
        .into_iter()
        .enumerate()
        .map(|(i, used)| {
            let mut sample = exact_sample(i as u64 + 1, i as f64 * 120.0, used);
            sample.reset_time = Interval::exact(1800.0);
            sample
        })
        .collect();
        let result = forecast(&samples);
        assert_eq!(result.will_exhaust_before_reset, Some(false));
        assert!(!result.safe_candidate);
        assert!(!result.notify_candidate);
        assert!(result.reasons.contains(&Reason::ResetUncertain));
    }

    #[test]
    fn q1_shared_endpoint_preserves_current_quantization_error() {
        let capacity = capacity_interval(&percent(89.0), &percent(99.0), 100000).unwrap();
        close(capacity.lower, 10000000.0 / 11.0);
        close(capacity.upper, 10000000.0 / 9.0);
        let remaining = remaining_tokens(capacity, &percent(99.0)).unwrap();
        close(remaining.lower, 50000.0 / 11.0);
        close(remaining.upper, 150000.0 / 9.0);
        assert!(remaining.lower < 5100.0 && remaining.upper > 5100.0);
        assert_eq!(
            (remaining.lower.floor(), remaining.upper.ceil()),
            (4545.0, 16667.0)
        );
    }

    #[test]
    fn q2_uses_independent_current_snapshot_instead_of_calibration_endpoint() {
        let capacity = capacity_interval(&percent(69.0), &percent(79.0), 100000).unwrap();
        let remaining = remaining_tokens(capacity, &percent(89.0)).unwrap();
        close(remaining.lower, 1050000.0 / 11.0);
        close(remaining.upper, 1150000.0 / 9.0);
        assert_eq!(
            (remaining.lower.floor(), remaining.upper.ceil()),
            (95454.0, 127778.0)
        );
    }

    #[test]
    fn q3_exact_times_reduce_to_documented_eta_and_cross_reset_is_unknown() {
        let mut samples = recent();
        for s in &mut samples {
            s.reset_time = Interval::exact(930.0);
        }
        let result = forecast(&samples);
        let speed = result.speed_percent_per_second.unwrap();
        close(speed.lower, 29.0 / 900.0);
        close(speed.upper, 31.0 / 900.0);
        let eta = result.eta_seconds.unwrap();
        close(eta.lower, 450.0 / 31.0);
        close(eta.upper, 1350.0 / 29.0);
        assert!(eta.lower < 15.3 && eta.upper > 15.3);
        assert_eq!(result.will_exhaust_before_reset, None);
        assert!(!result.notify_candidate);
        assert!(result.shared_endpoint);
    }

    #[test]
    fn q4_zero_denominator_and_weak_positive_signal_are_not_admitted() {
        assert_eq!(
            capacity_interval(&percent(89.0), &percent(90.0), 100000),
            Err(Reason::ZeroDenominator)
        );
        let samples: Vec<_> = (0..6)
            .map(|i| exact_sample(i + 1, i as f64 * 120.0, 89.0 + i as f64 * 0.4))
            .collect();
        let result = forecast(&samples);
        assert!(result.exhaustion_time.is_none());
        assert!(result.reasons.contains(&Reason::LowSignal));
    }

    #[test]
    fn q5_saturation_never_trains_and_headroom_requires_explicit_semantics() {
        assert_eq!(
            capacity_interval(&percent(89.0), &percent(100.0), 100000),
            Err(Reason::Saturated)
        );
        assert_eq!(
            remaining_percentage(&percent(100.0)),
            Err(Reason::Saturated)
        );
        let mut current = percent(100.0);
        current.saturated = true;
        current.saturation_headroom_bounded = true;
        assert_eq!(
            remaining_percentage(&current).unwrap(),
            Interval::new(0.0, 0.5).unwrap()
        );
        assert_eq!(
            remaining_tokens(Interval::new(900000.0, 1100000.0).unwrap(), &current).unwrap(),
            Interval::new(0.0, 5500.0).unwrap()
        );
    }

    fn latency_series(last_index: u64) -> Vec<NativeSample> {
        (0..=last_index)
            .map(|i| {
                let start = i as f64 * 120.0;
                let end = if i == 0 { 0.0 } else { start + 3.8 };
                let used = (73.41 + 0.025 * end).round();
                let mut s = sample(i + 1, Interval::new(start, end).unwrap(), used);
                s.reset_time = Interval::exact(1062.0);
                s
            })
            .collect()
    }

    #[test]
    fn t1_sampling_latency_cannot_create_two_false_before_reset_confirmations() {
        let six = latency_series(5);
        let seven = latency_series(6);
        let first = forecast(&six);
        let second = forecast(&seven);
        let e1 = first.exhaustion_time.unwrap();
        let e2 = second.exhaustion_time.unwrap();
        close(e1.lower, 16500.0 / 17.0);
        close(e1.upper, 160007.0 / 150.0);
        close(e2.lower, 990.0);
        close(e2.upper, 191807.0 / 180.0);
        for result in [&first, &second] {
            let e = result.exhaustion_time.unwrap();
            assert!(e.lower < 1063.6 && e.upper > 1063.6);
            assert_eq!(result.will_exhaust_before_reset, None);
            assert!(!result.notify_candidate);
        }
    }

    #[test]
    fn t2_fresh_ttl_and_short_rpc_do_not_prove_sampling_time() {
        let samples = recent();
        let mut current = samples.last().unwrap().clone();
        current.fact_time = None;
        let result = evaluate_native(&samples, &current, 900.0, &NativeParams::default());
        assert!(result.eta_seconds.is_none());
        assert!(result.reasons.contains(&Reason::TimeUnknown));
        current.fact_time = Interval::exact(900.0);
        current.percent.rounding_step = None;
        let result = evaluate_native(&samples[..5], &current, 900.0, &NativeParams::default());
        assert!(result.eta_seconds.is_none());
        assert!(!result.notify_candidate);
    }

    #[test]
    fn t3_span_lower_bound_and_maximum_possible_gap_control_admission() {
        let mut samples: Vec<_> = (0..6)
            .map(|i| exact_sample(i + 1, i as f64 * 120.0, 20.0 + i as f64 * 3.0))
            .collect();
        samples[0].fact_time = Interval::new(0.0, 3.8);
        assert!(forecast(&samples).eta_seconds.is_none()); // span lower bound 596.2
        let mut samples = recent();
        samples[1].fact_time = Interval::new(180.0, 241.0);
        assert!(forecast(&samples).eta_seconds.is_none()); // first maximum gap 241
    }

    #[test]
    fn t4_independent_current_time_and_ui_refresh_do_not_move_exhaustion() {
        let samples = recent();
        let mut current = exact_sample(7, 930.0, 99.0);
        current.ttl_start = 930.0;
        let first = evaluate_native(&samples, &current, 930.0, &NativeParams::default());
        let refreshed = evaluate_native(&samples, &current, 935.0, &NativeParams::default());
        assert!(!first.shared_endpoint);
        assert_eq!(first.exhaustion_time, refreshed.exhaustion_time);
        close(first.exhaustion_time.unwrap().lower, 930.0 + 450.0 / 31.0);
        assert_eq!(first.input_observation_ids.last().unwrap(), "observation-7");
    }

    #[test]
    fn t5_reset_equality_or_intersection_never_means_deterministic_before() {
        assert_eq!(
            compare_exhaustion_to_reset(
                Interval::exact(0.3 - 0.2).unwrap(),
                Interval::exact(0.1).unwrap()
            ),
            None
        );
        assert_eq!(
            compare_exhaustion_to_reset(
                Interval::new(10.0, 20.0).unwrap(),
                Interval::exact(20.0).unwrap()
            ),
            None
        );
        assert_eq!(
            compare_exhaustion_to_reset(
                Interval::new(10.0, 20.0).unwrap(),
                Interval::new(19.0, 30.0).unwrap()
            ),
            None
        );
        assert_eq!(
            compare_exhaustion_to_reset(
                Interval::new(10.0, 19.0).unwrap(),
                Interval::new(20.0, 30.0).unwrap()
            ),
            Some(true)
        );
        assert_eq!(
            compare_exhaustion_to_reset(
                Interval::new(31.0, 40.0).unwrap(),
                Interval::new(20.0, 30.0).unwrap()
            ),
            Some(false)
        );
    }

    #[test]
    fn stale_replayed_unpersisted_and_unproven_identity_cannot_notify() {
        let samples = recent();
        let current = samples.last().unwrap();
        assert!(forecast(&samples).notify_candidate); // only a candidate, not a permit
        for (mutated, reason) in [
            (0, Reason::IdentityUnproven),
            (1, Reason::NotPersisted),
            (2, Reason::Stale),
        ] {
            let mut c = current.clone();
            if mutated == 0 {
                c.identity_consistent = false;
            }
            if mutated == 1 {
                c.persisted = false;
            }
            if mutated == 2 {
                c.ttl_start = 0.0;
            }
            let result = evaluate_native(&samples, &c, 900.0, &NativeParams::default());
            assert!(!result.notify_candidate);
            assert!(result.reasons.contains(&reason));
        }
        let mut replay = samples.clone();
        replay[3].sequence = 1;
        assert!(forecast(&replay).reasons.contains(&Reason::OutOfOrder));
    }

    #[test]
    fn future_non_finite_reversed_and_clock_changed_times_are_rejected() {
        let samples = recent();
        let original = samples.last().unwrap();
        for time in [
            Interval {
                lower: f64::NAN,
                upper: 900.0,
            },
            Interval {
                lower: 901.0,
                upper: 900.0,
            },
            Interval {
                lower: 900.0,
                upper: 901.0,
            },
        ] {
            let mut current = original.clone();
            current.fact_time = Some(time);
            assert!(
                evaluate_native(&samples, &current, 900.0, &NativeParams::default())
                    .eta_seconds
                    .is_none()
            );
        }
        let mut current = original.clone();
        current.clock_epoch = "after-sleep".into();
        assert!(
            evaluate_native(&samples, &current, 900.0, &NativeParams::default())
                .eta_seconds
                .is_none()
        );
    }

    #[test]
    fn runtime_identity_isolation_allows_unknown_account_but_never_reuses_old_segment() {
        let mut samples = recent();
        for sample in &mut samples {
            sample.scope.account_scope = None;
        }
        assert!(forecast(&samples).eta_seconds.is_some());
        let mut current = samples.last().unwrap().clone();
        current.scope.runtime_id = "new-runtime".into();
        assert!(
            evaluate_native(&samples, &current, 900.0, &NativeParams::default())
                .eta_seconds
                .is_none()
        );
    }

    #[test]
    fn idle_readings_cannot_keep_old_activity_notifications_alive() {
        let mut samples: Vec<_> = (0..6)
            .map(|i| exact_sample(i + 1, i as f64 * 120.0, 69.0 + i as f64 * 6.0))
            .collect();
        for i in 6..=12 {
            samples.push(exact_sample(i + 1, i as f64 * 120.0, 99.0));
        }
        let result = forecast(&samples);
        assert!(result.eta_seconds.is_some());
        assert!(!result.notify_candidate);
        assert!(result.reasons.contains(&Reason::ActivityStale));
    }

    #[test]
    fn reset_requires_positive_new_window_evidence_and_keeps_unknown_boundary_partial() {
        let end = Interval::exact(100.0);
        let pending = classify_reset("old", end, None, 100.0);
        assert_eq!(pending.status, WindowStatus::WaitingForReset);
        let old_reply = ResetEvidence {
            confirmed_new_window: false,
            confirmed_same_window: false,
            window_instance_id: Some("old".into()),
            next_reset: end,
            actual_boundary: None,
        };
        assert_eq!(
            classify_reset("old", end, Some(&old_reply), 101.0).status,
            WindowStatus::WaitingForReset
        );
        let mut proof = old_reply.clone();
        proof.confirmed_new_window = true;
        proof.window_instance_id = Some("new".into());
        proof.next_reset = Interval::exact(200.0);
        let next = classify_reset("old", end, Some(&proof), 101.0);
        assert!(next.ended_previous_window && next.previous_usage_partial);
        proof.actual_boundary = Interval::exact(100.0);
        assert!(!classify_reset("old", end, Some(&proof), 101.0).previous_usage_partial);
        proof.confirmed_new_window = false;
        proof.confirmed_same_window = true;
        proof.window_instance_id = Some("old".into());
        let delayed = classify_reset("old", end, Some(&proof), 101.0);
        assert_eq!(delayed.status, WindowStatus::Ready);
        assert!(!delayed.ended_previous_window);
    }

    #[test]
    fn multi_window_earliest_limit_and_all_restored_are_different_operations() {
        let window = |id: &str, limit: Interval, restoration: Option<Interval>| WindowConstraint {
            pool_id: id.into(),
            forecast: NativeForecast {
                exhaustion_time: Some(limit),
                ..NativeForecast::default()
            },
            currently_limited: true,
            required: true,
            restoration_time: restoration,
        };
        let mut windows = vec![
            window(
                "short",
                Interval::new(10.0, 20.0).unwrap(),
                Interval::exact(100.0),
            ),
            window(
                "week",
                Interval::new(30.0, 40.0).unwrap(),
                Interval::exact(1000.0),
            ),
        ];
        let result = summarize_windows(&windows);
        assert_eq!(result.earliest_limit, Interval::new(10.0, 20.0));
        assert_eq!(result.all_required_restored_at, Interval::exact(1000.0));
        windows[1].restoration_time = None;
        assert!(summarize_windows(&windows)
            .all_required_restored_at
            .is_none());
    }

    #[test]
    fn reserve_applies_to_remaining_amount_and_shared_pool_allocations_are_summed() {
        assert_eq!(reserved_native_target(60.0, 0.2), Some(92.0));
        let row = |expected| NativeBudgetConstraint {
            pool_id: "shared".into(),
            remaining: Some(40.0),
            expected_consumption: expected,
        };
        assert_eq!(
            check_native_constraints(&[row(Some(15.0)), row(Some(20.0))], 0.2),
            Err(Reason::BudgetExceeded)
        );
        assert_eq!(
            check_native_constraints(&[row(None)], 0.2),
            Err(Reason::ConstraintsUnverified)
        );
        assert_eq!(
            check_native_constraints(&[], 0.2),
            Err(Reason::ConstraintsUnverified)
        );
    }

    #[test]
    fn demand_draft_counts_proven_zero_and_excludes_missing_history() {
        let mut rows: Vec<_> = [0.0, 0.0, 10.0, 20.0, 30.0]
            .into_iter()
            .map(|amount| DemandInterval {
                amount,
                covered: true,
                comparable: true,
            })
            .collect();
        rows.push(DemandInterval {
            amount: 100000.0,
            covered: false,
            comparable: true,
        });
        let draft = demand_draft(&rows).unwrap();
        assert_eq!(
            (draft.target_p50, draft.reference_p90, draft.samples),
            (10.0, 30.0, 5)
        );
        rows[0].covered = false;
        assert!(demand_draft(&rows).is_none());
    }
}
