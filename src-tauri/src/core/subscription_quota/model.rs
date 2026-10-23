//! Offline subscription-domain contracts. All times are Unix seconds.
//!
//! These inputs carry evidence supplied by the collector; constructing an input
//! does not prove a live account capability or grant a notification permit.
use serde::{Deserialize, Serialize};

pub const ALGORITHM_VERSION: &str = "subscription-quota-v1";

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Interval {
    pub lower: f64,
    pub upper: f64,
}

impl Interval {
    pub fn new(lower: f64, upper: f64) -> Option<Self> {
        (lower.is_finite() && upper.is_finite() && lower <= upper).then_some(Self { lower, upper })
    }
    pub fn exact(value: f64) -> Option<Self> {
        Self::new(value, value)
    }
    pub fn is_valid(self) -> bool {
        self.lower.is_finite() && self.upper.is_finite() && self.lower <= self.upper
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PercentReading {
    pub used_percent: f64,
    /// Confirmed rounding quantum, never inferred from UI decimal places.
    pub rounding_step: Option<f64>,
    pub saturated: bool,
    pub saturation_headroom_bounded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluationScope {
    pub runtime_id: String,
    pub source_id: String,
    pub binding_generation: u64,
    pub identity_epoch: u64,
    /// Opaque canonical JSON of the Core clear watermark (including null).
    pub clear_generation: String,
    pub account_scope: Option<String>,
    pub entitlement_segment: String,
    pub rule_version: String,
    pub pool_id: String,
    pub window_instance_id: String,
}

impl EvaluationScope {
    pub fn is_valid(&self) -> bool {
        [
            &self.runtime_id,
            &self.source_id,
            &self.entitlement_segment,
            &self.rule_version,
            &self.pool_id,
            &self.window_instance_id,
        ]
        .iter()
        .all(|s| !s.trim().is_empty())
            && self
                .account_scope
                .as_ref()
                .is_none_or(|s| !s.trim().is_empty())
    }
    /// Cross-period grouping still requires separate verified identity evidence.
    pub fn same_calibration_domain(&self, other: &Self) -> bool {
        self.source_id == other.source_id
            && self.account_scope == other.account_scope
            && self.entitlement_segment == other.entitlement_segment
            && self.rule_version == other.rule_version
            && self.pool_id == other.pool_id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoverySemantics {
    FixedReset,
    AnchoredReset,
    Sliding,
    Replenishing,
    NextResetOnly,
    Unknown,
}

impl RecoverySemantics {
    pub fn supports_single_reset_projection(self) -> bool {
        matches!(self, Self::FixedReset | Self::AnchoredReset)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeSample {
    pub observation_id: String,
    #[serde(with = "decimal_u64")]
    pub sequence: u64,
    pub scope: EvaluationScope,
    pub percent: PercentReading,
    /// None means that TTL freshness is known but sampling time is not bounded.
    pub fact_time: Option<Interval>,
    pub ttl_start: f64,
    pub persisted: bool,
    pub identity_consistent: bool,
    pub clock_epoch: String,
    pub reset_time: Option<Interval>,
    pub recovery: RecoverySemantics,
    pub discontinuity: bool,
}

mod decimal_u64 {
    use serde::{de::Error, Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &u64, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&v.to_string())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        let value = String::deserialize(d)?;
        let number = value.parse::<u64>().map_err(D::Error::custom)?;
        if number.to_string() != value {
            return Err(D::Error::custom(
                "sequence must be canonical unsigned decimal",
            ));
        }
        Ok(number)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    InvalidNumber,
    InvalidScope,
    IdentityUnproven,
    ScopeChanged,
    NotPersisted,
    OutOfOrder,
    TimeUnknown,
    TimeInvalid,
    ClockChanged,
    Stale,
    WaitingForReset,
    RecoveryUnknown,
    PrecisionUnknown,
    Saturated,
    ZeroDenominator,
    LowSignal,
    InsufficientSamples,
    Discontinuous,
    PercentDecreased,
    ActivityStale,
    ResetUncertain,
    ExternalUsageUncovered,
    CoverageUnknown,
    ConditionalCoverage,
    ModelCoverageUnknown,
    ProfileChanged,
    Unsettled,
    ConflictsOrGaps,
    OverlappingPairs,
    InsufficientPeriods,
    HoldoutInvalid,
    BacktestFailed,
    SuspectedChangePoint,
    EvidenceInvalidated,
    ConstraintsUnverified,
    BudgetExceeded,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeParams {
    pub ttl_seconds: f64,
    pub min_samples: usize,
    pub min_distinct_readings: usize,
    pub min_span_seconds: f64,
    pub max_gap_seconds: f64,
    pub preferred_history_seconds: f64,
    pub max_history_seconds: f64,
    pub max_change_age_seconds: f64,
}

impl Default for NativeParams {
    fn default() -> Self {
        Self {
            ttl_seconds: 240.0,
            min_samples: 6,
            min_distinct_readings: 3,
            min_span_seconds: 600.0,
            max_gap_seconds: 240.0,
            preferred_history_seconds: 1800.0,
            max_history_seconds: 7200.0,
            max_change_age_seconds: 600.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeForecast {
    /// All admitted recent scenarios must be explicitly after the reset.
    #[serde(default)]
    pub safe_candidate: bool,
    pub algorithm_version: String,
    pub speed_percent_per_second: Option<Interval>,
    pub eta_seconds: Option<Interval>,
    pub exhaustion_time: Option<Interval>,
    pub reset_time: Option<Interval>,
    pub will_exhaust_before_reset: Option<bool>,
    /// A single eligible observation; Core must still confirm two new readings
    /// and apply host, deduplication, freshness and final-submit permissions.
    pub notify_candidate: bool,
    pub sample_count: usize,
    pub excluded_count: usize,
    pub longer_baseline: bool,
    pub shared_endpoint: bool,
    pub input_observation_ids: Vec<String>,
    pub reasons: Vec<Reason>,
}

impl Default for NativeForecast {
    fn default() -> Self {
        Self {
            algorithm_version: ALGORITHM_VERSION.into(),
            speed_percent_per_second: None,
            eta_seconds: None,
            exhaustion_time: None,
            reset_time: None,
            will_exhaust_before_reset: None,
            notify_candidate: false,
            safe_candidate: false,
            sample_count: 0,
            excluded_count: 0,
            longer_baseline: false,
            shared_endpoint: false,
            input_observation_ids: Vec::new(),
            reasons: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowStatus {
    Ready,
    WaitingForReset,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetEvidence {
    pub confirmed_new_window: bool,
    pub confirmed_same_window: bool,
    pub window_instance_id: Option<String>,
    pub next_reset: Option<Interval>,
    pub actual_boundary: Option<Interval>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetDecision {
    pub status: WindowStatus,
    pub window_instance_id: String,
    pub resolved_end: Option<Interval>,
    pub ended_previous_window: bool,
    pub previous_usage_partial: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowConstraint {
    pub pool_id: String,
    pub forecast: NativeForecast,
    pub currently_limited: bool,
    pub required: bool,
    pub restoration_time: Option<Interval>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MultiWindowSummary {
    pub earliest_limit: Option<Interval>,
    pub limiting_pools: Vec<String>,
    pub all_required_restored_at: Option<Interval>,
    pub constraints_verified: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageEvidence {
    VerifiedComplete,
    ConditionalUserAssumption,
    Unknown,
    KnownIncomplete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityEvidence {
    VerifiedAccount,
    UserSpecified,
    RuntimeOnly,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelShare {
    pub model: String,
    pub share: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageProfile {
    pub models: Vec<ModelShare>,
    pub all_models_attributed: bool,
    pub effort: Option<String>,
    pub speed_mode: Option<String>,
    pub output_share: f64,
    pub cache_share: f64,
    pub median_input_tokens: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibrationPair {
    pub pair_id: String,
    pub start: NativeSample,
    pub end: NativeSample,
    pub tokens: u64,
    pub profile: UsageProfile,
    pub identity_evidence: IdentityEvidence,
    pub coverage: CoverageEvidence,
    pub settled: bool,
    pub conflicts_or_gaps: bool,
    pub holdout: bool,
    pub evidence_invalidated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalibrationQuality {
    Learning,
    Conditional,
    Validated,
    Invalidated,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibrationResult {
    pub algorithm_version: String,
    pub quality: CalibrationQuality,
    pub capacity: Option<Interval>,
    /// Explicit point scenario, not a bounded estimate when precision is unknown.
    pub empirical_capacity: Option<f64>,
    pub train_pairs: usize,
    pub holdout_pairs: usize,
    pub independent_periods: usize,
    pub excluded_pairs: usize,
    pub relative_mad: Option<f64>,
    pub holdout_wape: Option<f64>,
    pub included_pair_ids: Vec<String>,
    pub excluded: Vec<(String, Reason)>,
    pub reasons: Vec<Reason>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeBudgetConstraint {
    pub pool_id: String,
    pub remaining: Option<f64>,
    pub expected_consumption: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DemandInterval {
    pub amount: f64,
    pub covered: bool,
    pub comparable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DemandDraft {
    pub target_p50: f64,
    pub reference_p90: f64,
    pub samples: usize,
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;
    pub fn scope() -> EvaluationScope {
        EvaluationScope {
            runtime_id: "runtime-a".into(),
            source_id: "codex-local".into(),
            binding_generation: 1,
            identity_epoch: 1,
            clear_generation: "clear-1".into(),
            account_scope: Some("synthetic-account".into()),
            entitlement_segment: "entitlement-1".into(),
            rule_version: "rule-1".into(),
            pool_id: "shared".into(),
            window_instance_id: "window-1".into(),
        }
    }
    pub fn percent(used_percent: f64) -> PercentReading {
        PercentReading {
            used_percent,
            rounding_step: Some(1.0),
            saturated: false,
            saturation_headroom_bounded: false,
        }
    }
    pub fn sample(sequence: u64, time: Interval, used: f64) -> NativeSample {
        NativeSample {
            observation_id: format!("observation-{sequence}"),
            sequence,
            scope: scope(),
            percent: percent(used),
            fact_time: Some(time),
            ttl_start: time.lower,
            persisted: true,
            identity_consistent: true,
            clock_epoch: "clock-1".into(),
            reset_time: Interval::exact(10000.0),
            recovery: RecoverySemantics::FixedReset,
            discontinuity: false,
        }
    }
    pub fn exact_sample(sequence: u64, time: f64, used: f64) -> NativeSample {
        sample(sequence, Interval::exact(time).unwrap(), used)
    }
    pub fn profile() -> UsageProfile {
        UsageProfile {
            models: vec![ModelShare {
                model: "synthetic-model-a".into(),
                share: 1.0,
            }],
            all_models_attributed: true,
            effort: Some("medium".into()),
            speed_mode: None,
            output_share: 0.2,
            cache_share: 0.3,
            median_input_tokens: 10000.0,
        }
    }
    pub fn pair(index: u64, time: f64, used: f64) -> CalibrationPair {
        CalibrationPair {
            pair_id: format!("pair-{index}"),
            start: exact_sample(index * 2 + 1, time, used),
            end: exact_sample(index * 2 + 2, time + 300.0, used + 10.0),
            tokens: 100000,
            profile: profile(),
            identity_evidence: IdentityEvidence::VerifiedAccount,
            coverage: CoverageEvidence::VerifiedComplete,
            settled: true,
            conflicts_or_gaps: false,
            holdout: false,
            evidence_invalidated: false,
        }
    }
    pub fn close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() <= 1e-8 * expected.abs().max(1.0),
            "{actual} != {expected}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{fixtures::*, *};
    #[test]
    fn old_forecasts_without_safe_candidate_fail_closed_on_deserialization() {
        let mut value = serde_json::to_value(NativeForecast::default()).unwrap();
        value.as_object_mut().unwrap().remove("safeCandidate");
        assert!(
            !serde_json::from_value::<NativeForecast>(value)
                .unwrap()
                .safe_candidate
        );
    }
    #[test]
    fn rejects_non_finite_and_reversed_intervals() {
        assert!(Interval::new(f64::NAN, 1.0).is_none());
        assert!(Interval::new(0.0, f64::INFINITY).is_none());
        assert!(Interval::new(2.0, 1.0).is_none());
    }
    #[test]
    fn observation_sequence_is_lossless_and_canonical_in_json() {
        let mut observation = exact_sample(u64::MAX, 100.0, 20.0);
        observation.scope.account_scope = None;
        let value = serde_json::to_value(&observation).unwrap();
        assert_eq!(value["sequence"], u64::MAX.to_string());
        assert_eq!(
            serde_json::from_value::<NativeSample>(value.clone()).unwrap(),
            observation
        );
        let mut noncanonical = value;
        noncanonical["sequence"] = serde_json::json!("01");
        assert!(serde_json::from_value::<NativeSample>(noncanonical).is_err());
    }
}
