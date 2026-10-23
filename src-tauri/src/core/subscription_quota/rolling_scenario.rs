//! 显式手动滚动窗口情景。进入与到期同步扫描，未知历史或未知扣额从不补零。
use serde::Serialize;
use std::collections::BTreeSet;
use std::fmt;

const MAX_CHARGES: usize = 128;
const MAX_POINTS: f64 = 1_000_000_000.0;
const MAX_WINDOW_MS: i64 = 10_080 * 60_000;
const MAX_OUTPUT_POINTS: usize = 260;

#[derive(Clone, Debug)]
pub(crate) struct RollingCharge {
    pub id: String,
    pub occurred_at_ms: i64,
    pub points: Option<f64>,
}

#[derive(Clone, Debug)]
pub(crate) struct RollingInput {
    pub as_of_ms: i64,
    pub history_complete_from_ms: Option<i64>,
    pub window_ms: i64,
    pub horizon_ms: i64,
    pub capacity_points: f64,
    pub required_points: f64,
    pub charges: Vec<RollingCharge>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RollingPoint {
    pub at_ms: i64,
    pub used_points: Option<f64>,
    pub remaining_points: Option<f64>,
    pub available: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AvailabilityWindow {
    pub from_ms: i64,
    pub to_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RollingResult {
    pub as_of_ms: i64,
    pub horizon_ms: i64,
    pub points: Vec<RollingPoint>,
    pub availability_windows: Vec<AvailabilityWindow>,
    pub next_available_ms: Option<i64>,
    pub reasons: Vec<String>,
    pub quality: &'static str,
    pub constraints_verified: bool,
    pub notify_eligible: bool,
    pub uncertainty_bounds: Option<[f64; 2]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RollingError(pub &'static str);
impl fmt::Display for RollingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}
impl std::error::Error for RollingError {}

fn windows(points: &[RollingPoint]) -> Vec<AvailabilityWindow> {
    let mut result: Vec<AvailabilityWindow> = Vec::new();
    for pair in points.windows(2) {
        if pair[0].available == Some(true) && pair[0].at_ms < pair[1].at_ms {
            if let Some(previous) = result
                .last_mut()
                .filter(|previous| previous.to_ms == pair[0].at_ms)
            {
                previous.to_ms = pair[1].at_ms;
            } else {
                result.push(AvailabilityWindow {
                    from_ms: pair[0].at_ms,
                    to_ms: pair[1].at_ms,
                });
            }
        }
    }
    result
}

pub(crate) fn simulate(input: &RollingInput) -> Result<RollingResult, RollingError> {
    if input.window_ms <= 0
        || input.window_ms > MAX_WINDOW_MS
        || input.horizon_ms <= input.as_of_ms
        || input
            .history_complete_from_ms
            .is_some_and(|time| time > input.as_of_ms)
        || !input.capacity_points.is_finite()
        || !(0.0..=MAX_POINTS).contains(&input.capacity_points)
        || input.capacity_points == 0.0
        || !input.required_points.is_finite()
        || input.required_points <= 0.0
        || input.required_points > input.capacity_points
    {
        return Err(RollingError("invalid_number"));
    }
    if input.charges.len() > MAX_CHARGES {
        return Err(RollingError("charge_limit"));
    }
    let latest_allowed = input
        .as_of_ms
        .checked_add(input.window_ms)
        .ok_or(RollingError("time_overflow"))?;
    let latest_horizon = latest_allowed
        .checked_add(input.window_ms)
        .ok_or(RollingError("time_overflow"))?;
    if input.horizon_ms > latest_horizon {
        return Err(RollingError("horizon_limit"));
    }
    let known_from = input
        .history_complete_from_ms
        .unwrap_or(input.as_of_ms)
        .checked_add(input.window_ms)
        .ok_or(RollingError("time_overflow"))?;
    let mut ids = BTreeSet::new();
    let mut events = BTreeSet::from([input.as_of_ms, input.horizon_ms]);
    if known_from > input.as_of_ms && known_from <= input.horizon_ms {
        events.insert(known_from);
    }
    let mut charges = Vec::with_capacity(input.charges.len());
    for charge in &input.charges {
        if charge.id.trim().is_empty()
            || charge.id.encode_utf16().count() > 512
            || charge.id.chars().any(char::is_control)
            || !ids.insert(&charge.id)
        {
            return Err(RollingError("invalid_charge_id"));
        }
        if charge.occurred_at_ms > latest_allowed
            || charge
                .points
                .is_some_and(|points| !points.is_finite() || points <= 0.0 || points > MAX_POINTS)
        {
            return Err(RollingError("invalid_number"));
        }
        // 映射超过单笔上限的模型扣额由上层显式转为 None，不能伪装成零。
        let expires = charge
            .occurred_at_ms
            .checked_add(input.window_ms)
            .ok_or(RollingError("time_overflow"))?;
        for time in [charge.occurred_at_ms, expires] {
            if time > input.as_of_ms && time <= input.horizon_ms {
                events.insert(time);
            }
        }
        charges.push((charge, expires));
    }
    if events.len() > MAX_OUTPUT_POINTS {
        return Err(RollingError("point_limit"));
    }
    let mut history_unknown = false;
    let mut charge_unknown = false;
    let mut points = Vec::with_capacity(events.len());
    for at_ms in events {
        let mut unknown = at_ms < known_from;
        history_unknown |= unknown;
        let mut used = 0.0;
        // 有界输入每个事件重新求活动集合；不累积浮点减法，避免释放后产生负占用。
        for (charge, expires) in &charges {
            if charge.occurred_at_ms <= at_ms && *expires > at_ms {
                if let Some(amount) = charge.points {
                    used += amount;
                } else {
                    unknown = true;
                    charge_unknown = true;
                }
            }
        }
        if !used.is_finite() {
            return Err(RollingError("arithmetic_overflow"));
        }
        points.push(RollingPoint {
            at_ms,
            used_points: (!unknown).then_some(used),
            remaining_points: (!unknown).then_some((input.capacity_points - used).max(0.0)),
            available: (!unknown).then_some(used <= input.capacity_points - input.required_points),
        });
    }
    let mut reasons = Vec::new();
    if history_unknown {
        reasons.push("history_incomplete".into());
    }
    if charge_unknown {
        reasons.push("charge_unknown".into());
    }
    Ok(RollingResult {
        as_of_ms: input.as_of_ms,
        horizon_ms: input.horizon_ms,
        next_available_ms: points
            .iter()
            .find(|point| point.available == Some(true))
            .map(|point| point.at_ms),
        availability_windows: windows(&points),
        points,
        reasons,
        quality: "conditional-manual",
        constraints_verified: false,
        notify_eligible: false,
        uncertainty_bounds: None,
    })
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommonAvailability {
    pub as_of_ms: i64,
    pub horizon_ms: i64,
    pub availability_windows: Vec<AvailabilityWindow>,
    pub next_available_ms: Option<i64>,
}

/// 多池只组合各自完整情景在每个事件后的状态，不取各池首次恢复的最大值。
pub(crate) fn common_availability(
    inputs: &[RollingInput],
) -> Result<CommonAvailability, RollingError> {
    if inputs.is_empty() || inputs.len() > 8 {
        return Err(RollingError("pool_limit"));
    }
    let first = &inputs[0];
    if inputs
        .iter()
        .any(|input| input.as_of_ms != first.as_of_ms || input.horizon_ms != first.horizon_ms)
    {
        return Err(RollingError("scope_changed"));
    }
    let simulations = inputs.iter().map(simulate).collect::<Result<Vec<_>, _>>()?;
    let events: BTreeSet<_> = simulations
        .iter()
        .flat_map(|result| result.points.iter().map(|point| point.at_ms))
        .collect();
    let mut cursors = vec![0; simulations.len()];
    let mut common = Vec::with_capacity(events.len());
    for at_ms in events {
        let mut available = true;
        let mut unknown = false;
        for (index, simulation) in simulations.iter().enumerate() {
            while cursors[index] + 1 < simulation.points.len()
                && simulation.points[cursors[index] + 1].at_ms <= at_ms
            {
                cursors[index] += 1;
            }
            match simulation.points[cursors[index]].available {
                Some(false) => available = false,
                None => unknown = true,
                _ => {}
            }
        }
        common.push(RollingPoint {
            at_ms,
            used_points: None,
            remaining_points: None,
            available: if !available {
                Some(false)
            } else if unknown {
                None
            } else {
                Some(true)
            },
        });
    }
    Ok(CommonAvailability {
        as_of_ms: first.as_of_ms,
        horizon_ms: first.horizon_ms,
        next_available_ms: common
            .iter()
            .find(|point| point.available == Some(true))
            .map(|point| point.at_ms),
        availability_windows: windows(&common),
    })
}

#[cfg(test)]
#[path = "rolling_scenario_tests.rs"]
mod tests;
