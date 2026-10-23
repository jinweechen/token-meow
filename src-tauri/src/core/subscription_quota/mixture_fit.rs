//! 手动同条件样本的非负扣额拟合。数值可辨识不构成厂商计费、身份或额度证明。
use nalgebra::{linalg::SVD, DMatrix, DVector};
use serde::Serialize;
use std::collections::BTreeSet;

const MAX_SAMPLES: usize = 128;
const MAX_TOKEN: u64 = 9_007_199_254_740_991;
const MAX_POINTS: f64 = 1_000_000_000.0;
const MILLION: f64 = 1_000_000.0;
const SVD_EPS: f64 = 1e-12;
const RANK_EPS: f64 = 1e-8;
const MAX_CONDITION: f64 = 1e6;
const MAX_SVD_ITERATIONS: usize = 256;
const MAX_WAPE: f64 = 0.20;

#[derive(Clone, Debug)]
pub(crate) struct MixtureSample {
    pub id: String,
    pub from_ms: i64,
    pub to_ms: i64,
    pub charge_points: f64,
    pub tokens: Vec<u64>,
    pub holdout: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FitKind {
    Separated,
    FixedMix,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelWeight {
    pub model: String,
    pub points_per_million_tokens: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FixedMix {
    pub shares: Vec<f64>,
    pub points_per_million_tokens: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MixtureFitResult {
    pub kind: FitKind,
    pub weights: Option<Vec<ModelWeight>>,
    pub fixed_mix: Option<FixedMix>,
    pub rank: usize,
    pub condition_number: Option<f64>,
    pub train_samples: usize,
    pub holdout_samples: usize,
    pub train_wape: Option<f64>,
    pub holdout_wape: Option<f64>,
    pub reasons: Vec<String>,
    pub quality: &'static str,
    pub constraints_verified: bool,
    pub notify_eligible: bool,
    pub uncertainty_bounds: Option<[f64; 2]>,
}

impl MixtureFitResult {
    fn empty() -> Self {
        Self {
            kind: FitKind::Unavailable,
            weights: None,
            fixed_mix: None,
            rank: 0,
            condition_number: None,
            train_samples: 0,
            holdout_samples: 0,
            train_wape: None,
            holdout_wape: None,
            reasons: Vec::new(),
            quality: "conditional-manual",
            constraints_verified: false,
            notify_eligible: false,
            uncertainty_bounds: None,
        }
    }
    fn reason(mut self, reason: &str) -> Self {
        if !self.reasons.iter().any(|value| value == reason) {
            self.reasons.push(reason.into());
        }
        self
    }
}

fn text_valid(value: &str) -> bool {
    !value.trim().is_empty()
        && value.encode_utf16().count() <= 512
        && !value.chars().any(|c| {
            c.is_control() || matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
}

fn total_tokens(sample: &MixtureSample) -> f64 {
    sample.tokens.iter().map(|value| *value as f64).sum()
}

fn wape(samples: &[&MixtureSample], coefficients: &[f64]) -> Option<f64> {
    let denominator: f64 = samples.iter().map(|sample| sample.charge_points).sum();
    let numerator: f64 = samples
        .iter()
        .map(|sample| {
            let predicted: f64 = sample
                .tokens
                .iter()
                .zip(coefficients)
                .map(|(tokens, weight)| *tokens as f64 / MILLION * weight)
                .sum();
            (predicted - sample.charge_points).abs()
        })
        .sum();
    // 全零留出不能证明费率为零；分母为零保持不可评估。
    (denominator > 0.0 && denominator.is_finite() && numerator.is_finite())
        .then_some(numerator / denominator)
}

fn fixed_shares(samples: &[&MixtureSample]) -> Option<Vec<f64>> {
    let first = samples.first()?;
    let total = total_tokens(first);
    let shares: Vec<_> = first
        .tokens
        .iter()
        .map(|value| *value as f64 / total)
        .collect();
    samples
        .iter()
        .all(|sample| {
            let total = total_tokens(sample);
            sample
                .tokens
                .iter()
                .zip(&shares)
                .all(|(value, share)| (*value as f64 / total - share).abs() <= 1e-9)
        })
        .then_some(shares)
}

fn fixed_mix_result(
    mut result: MixtureFitResult,
    train: &[&MixtureSample],
    holdout: &[&MixtureSample],
) -> MixtureFitResult {
    let all: Vec<_> = train.iter().chain(holdout).copied().collect();
    let Some(shares) = fixed_shares(&all) else {
        return result.reason("rank_deficient");
    };
    let numerator: f64 = train
        .iter()
        .map(|sample| total_tokens(sample) / MILLION * sample.charge_points)
        .sum();
    let denominator: f64 = train
        .iter()
        .map(|sample| (total_tokens(sample) / MILLION).powi(2))
        .sum();
    let coefficient = numerator / denominator;
    if !coefficient.is_finite() || coefficient <= 0.0 {
        return result.reason("low_signal");
    }
    // 固定比例只辨识总 Token 的整体斜率；不把它拆成各模型独立系数。
    let repeated = vec![coefficient; shares.len()];
    result.train_wape = wape(train, &repeated);
    result.holdout_wape = wape(holdout, &repeated);
    if result.train_wape.is_none_or(|value| value > MAX_WAPE)
        || result.holdout_wape.is_none_or(|value| value > MAX_WAPE)
    {
        return result.reason("backtest_failed");
    }
    result.kind = FitKind::FixedMix;
    result.fixed_mix = Some(FixedMix {
        shares,
        points_per_million_tokens: coefficient,
    });
    result.reason("rank_deficient").reason("fixed_mixture_only")
}

/// 最多三列的 NNLS：枚举全部非空 support，再用有界 SVD 求各支持集的最小二乘。
/// 不形成 XᵀX，不使用无界迭代或把负系数直接截断成一个未经优化的解。
fn nnls(matrix: &DMatrix<f64>, target: &DVector<f64>) -> Option<Vec<f64>> {
    let columns = matrix.ncols();
    let mut best = vec![0.0; columns];
    let mut best_loss = target.norm_squared();
    for mask in 1..(1usize << columns) {
        let support: Vec<_> = (0..columns)
            .filter(|column| mask & (1 << column) != 0)
            .collect();
        let restricted = DMatrix::from_fn(matrix.nrows(), support.len(), |row, column| {
            matrix[(row, support[column])]
        });
        let svd = SVD::try_new(restricted, true, true, SVD_EPS, MAX_SVD_ITERATIONS)?;
        let solved = svd.solve(target, SVD_EPS).ok()?;
        let tolerance = 1e-10 * (1.0 + solved.iter().copied().map(f64::abs).fold(0.0, f64::max));
        if solved
            .iter()
            .any(|value| !value.is_finite() || *value < -tolerance)
        {
            continue;
        }
        let mut candidate = vec![0.0; columns];
        for (column, value) in support.iter().zip(solved.iter()) {
            candidate[*column] = value.max(0.0);
        }
        let loss = (matrix * DVector::from_vec(candidate.clone()) - target).norm_squared();
        if !loss.is_finite() {
            return None;
        }
        if loss < best_loss {
            best_loss = loss;
            best = candidate;
        }
    }
    Some(best)
}

pub(crate) fn fit(models: &[String], samples: &[MixtureSample]) -> MixtureFitResult {
    let mut result = MixtureFitResult::empty();
    let mut names = BTreeSet::new();
    if models.is_empty()
        || models.len() > 3
        || models
            .iter()
            .any(|model| !text_valid(model) || !names.insert(model))
    {
        return result.reason("invalid_models");
    }
    if samples.len() > MAX_SAMPLES {
        return result.reason("sample_limit");
    }
    let mut ordered: Vec<_> = samples.iter().collect();
    ordered.sort_by(|a, b| {
        a.from_ms
            .cmp(&b.from_ms)
            .then(a.to_ms.cmp(&b.to_ms))
            .then(a.id.cmp(&b.id))
    });
    let mut ids = BTreeSet::new();
    let mut previous_end = None;
    let mut seen_holdout = false;
    for sample in &ordered {
        if !text_valid(&sample.id) || !ids.insert(&sample.id) {
            return result.reason("invalid_sample_id");
        }
        if sample.from_ms >= sample.to_ms || previous_end.is_some_and(|end| sample.from_ms < end) {
            return result.reason("overlapping_samples");
        }
        previous_end = Some(sample.to_ms);
        if sample.tokens.len() != models.len()
            || sample.tokens.iter().any(|value| *value > MAX_TOKEN)
            || !sample.tokens.iter().any(|value| *value > 0)
            || !sample.charge_points.is_finite()
            || !(0.0..=MAX_POINTS).contains(&sample.charge_points)
        {
            return result.reason("invalid_number");
        }
        if seen_holdout && !sample.holdout {
            return result.reason("holdout_invalid");
        }
        seen_holdout |= sample.holdout;
    }
    let train: Vec<_> = ordered
        .iter()
        .copied()
        .filter(|sample| !sample.holdout)
        .collect();
    let holdout: Vec<_> = ordered
        .iter()
        .copied()
        .filter(|sample| sample.holdout)
        .collect();
    result.train_samples = train.len();
    result.holdout_samples = holdout.len();
    if train.len() < models.len() + 2 || holdout.len() < 2 {
        return result.reason("insufficient_samples");
    }
    if train.iter().map(|sample| sample.charge_points).sum::<f64>() <= 0.0 {
        return result.reason("low_signal");
    }
    let matrix = DMatrix::from_fn(train.len(), models.len(), |row, column| {
        train[row].tokens[column] as f64 / MILLION
    });
    let scales: Vec<_> = (0..models.len())
        .map(|column| matrix.column(column).norm())
        .collect();
    let normalized = DMatrix::from_fn(train.len(), models.len(), |row, column| {
        if scales[column] > 0.0 {
            matrix[(row, column)] / scales[column]
        } else {
            0.0
        }
    });
    let Some(svd) = SVD::try_new(
        normalized.clone(),
        false,
        false,
        SVD_EPS,
        MAX_SVD_ITERATIONS,
    ) else {
        return result.reason("svd_failed");
    };
    let largest = svd.singular_values.iter().copied().fold(0.0, f64::max);
    result.rank = svd.rank(largest * RANK_EPS);
    if result.rank < models.len() {
        return if result.rank == 1 && models.len() > 1 {
            fixed_mix_result(result, &train, &holdout)
        } else {
            result.reason("rank_deficient")
        };
    }
    let smallest = svd
        .singular_values
        .iter()
        .copied()
        .fold(f64::INFINITY, f64::min);
    let condition = largest / smallest;
    if !condition.is_finite() {
        return result.reason("invalid_number");
    }
    result.condition_number = Some(condition);
    if condition > MAX_CONDITION {
        return result.reason("ill_conditioned");
    }
    let target =
        DVector::from_iterator(train.len(), train.iter().map(|sample| sample.charge_points));
    let Some(normalized_weights) = nnls(&normalized, &target) else {
        return result.reason("svd_failed");
    };
    let weights: Vec<_> = normalized_weights
        .iter()
        .zip(&scales)
        .map(|(weight, scale)| weight / scale)
        .collect();
    if weights
        .iter()
        .any(|value| !value.is_finite() || *value < 0.0)
    {
        return result.reason("invalid_number");
    }
    result.train_wape = wape(&train, &weights);
    result.holdout_wape = wape(&holdout, &weights);
    if result.train_wape.is_none_or(|value| value > MAX_WAPE)
        || result.holdout_wape.is_none_or(|value| value > MAX_WAPE)
    {
        return result.reason("backtest_failed");
    }
    result.kind = FitKind::Separated;
    result.weights = Some(
        models
            .iter()
            .zip(weights)
            .map(|(model, value)| ModelWeight {
                model: model.clone(),
                points_per_million_tokens: value,
            })
            .collect(),
    );
    result
}

/// 仅使用回测通过的独立系数。零系数是 NNLS 边界诊断，不证明任何模型免费。
pub(crate) fn predict_tokens(
    result: &MixtureFitResult,
    tokens: &[u64],
) -> Result<f64, &'static str> {
    let weights = result
        .weights
        .as_ref()
        .filter(|_| result.kind == FitKind::Separated)
        .ok_or("fit_unavailable")?;
    if weights.len() != tokens.len()
        || tokens.iter().any(|value| *value > MAX_TOKEN)
        || !tokens.iter().any(|value| *value > 0)
    {
        return Err("invalid_number");
    }
    let mut amount = 0.0;
    for (tokens, weight) in tokens.iter().zip(weights) {
        if !weight.points_per_million_tokens.is_finite() || weight.points_per_million_tokens < 0.0 {
            return Err("invalid_number");
        }
        if *tokens > 0 && weight.points_per_million_tokens <= 0.0 {
            return Err("zero_weight_unpriced");
        }
        amount += *tokens as f64 / MILLION * weight.points_per_million_tokens;
    }
    if !amount.is_finite() || amount <= 0.0 {
        return Err("invalid_number");
    }
    if amount > MAX_POINTS {
        return Err("prediction_out_of_range");
    }
    Ok(amount)
}

#[cfg(test)]
#[path = "mixture_fit_tests.rs"]
mod tests;
