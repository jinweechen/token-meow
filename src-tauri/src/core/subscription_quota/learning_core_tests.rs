//! Production Core/SQLite wiring with explicitly synthetic typed attestations.
//! These fixtures do not establish any live provider capability or OS permit.
use super::calibration_assembler::{
    native_observation_digest, source_check_digest, AdapterQuotaProof, CoverageAttestation,
    CoverageKind, Domain, ExplicitMode, LedgerSettlement, UsageModeAttestation,
};
use super::learning::{AdapterResetTransitionProof, CollectorBatch, ResetTransition};
use super::model::{EvaluationScope, Interval, PercentReading, RecoverySemantics};
use crate::account_bridge::{AccountSnapshot, AccountSubRead, QuotaBucket, QuotaWindow};
use crate::budget_clock::{test_support::TestClock, Reading};
use crate::core::{adapter, get_setting, put_setting, Core};
use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[path = "multi_window_core_tests.rs"]
mod multi_window;
#[path = "official_native_core_tests.rs"]
mod official_native;

fn at(offset: i64) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-22T08:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
        + Duration::seconds(offset)
}
fn reading(offset: i64) -> Reading {
    Reading {
        utc: at(offset),
        mono_ms: 1_000_000 + offset as u64 * 1000,
    }
}
fn seconds(offset: i64) -> f64 {
    at(offset).timestamp() as f64
}

struct Rig {
    _temp: tempfile::TempDir,
    data: std::path::PathBuf,
    source: std::path::PathBuf,
    core: Option<Core>,
    clock: TestClock,
    proofs: Vec<AdapterQuotaProof>,
    rounding_step: Option<f64>,
}
impl Rig {
    fn new() -> Self {
        let clock = TestClock::new(reading(0));
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        let source = temp.path().join("source");
        std::fs::create_dir_all(&source).unwrap();
        let core = Core::open(&data, &source).unwrap();
        Self {
            _temp: temp,
            data,
            source,
            core: Some(core),
            clock,
            proofs: Vec::new(),
            rounding_step: Some(1.0),
        }
    }
    fn core(&self) -> &Core {
        self.core.as_ref().unwrap()
    }
    fn scope(&self) -> EvaluationScope {
        EvaluationScope {
            runtime_id: self.core().runtime_id.clone(),
            source_id: self.core().source_id.clone(),
            binding_generation: 1,
            identity_epoch: 1,
            clear_generation: "null".into(),
            account_scope: Some("typed-fixture-account".into()),
            entitlement_segment: "typed-fixture-rights".into(),
            rule_version: "typed-fixture-rules".into(),
            pool_id: "shared:primary".into(),
            window_instance_id: "typed-fixture-provider-period".into(),
        }
    }
    fn write_usage(&self, offset: i64) {
        let n = adapter::Metric::Valid;
        let row = adapter::ResponseUsage {
            source_id: self.core().source_id.clone(),
            channel_id: "typed-fixture-native".into(),
            namespace: "typed-fixture".into(),
            thread_id: "typed-fixture-thread".into(),
            response_id: format!("response-{offset}"),
            turn_id: None,
            root_turn_id: None,
            event_time: at(offset).to_rfc3339(),
            collected_at: at(offset).to_rfc3339(),
            requested_model: Some("requested-other".into()),
            observed_model: Some("actual-model".into()),
            provider_id: None,
            account_scope_id: Some("typed-fixture-account".into()),
            native_counts: BTreeMap::new(),
            counts: adapter::Counts {
                input: n(80_000),
                output: n(20_000),
                cache_read: n(20_000),
                cache_write: n(0),
                reasoning: n(4_000),
                total: n(100_000),
            },
            semantics_version: adapter::SEMANTICS.into(),
            adapter_version: "typed-fixture-adapter".into(),
            completeness: "source-recorded".into(),
            phase: "final".into(),
            revision: None,
            live: false,
        };
        adapter::store_response(&self.core().db, &row, false).unwrap();
    }
    fn observe(&mut self, sequence: u64, offset: i64, used: f64, usage: bool) -> Value {
        self.observe_window(
            sequence,
            offset,
            used,
            usage,
            "typed-fixture-provider-period",
            1800,
            None,
            None,
            false,
        )
    }
    fn observe_window(
        &mut self,
        sequence: u64,
        offset: i64,
        used: f64,
        usage: bool,
        period: &str,
        next_reset: i64,
        transition: Option<ResetTransition>,
        boundary: Option<Interval>,
        known_speed: bool,
    ) -> Value {
        self.clock.set(reading(offset));
        if usage {
            self.write_usage(offset - 90);
        }
        let stamp = at(offset).to_rfc3339();
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
            auth_mode: Some("chatgpt".into()),
            account_scope: Some("typed-fixture-account".into()),
            plan_type: Some("fixture-plan-is-not-rights-proof".into()),
            identity_epoch: "1".into(),
            identity_consistency: "proven".into(),
            identity_consistency_reason: "typed-fixture-only".into(),
            observation_sequence: Some(sequence.to_string()),
            observation_outcome: Some("success".into()),
            read_started_at: Some(stamp.clone()),
            read_finished_at: Some(stamp),
            account_read: Some(read.clone()),
            quota_read: Some(read),
            buckets: vec![QuotaBucket {
                id: "shared".into(),
                name: Some("fixture pool".into()),
                primary: Some(QuotaWindow {
                    used_percent: used,
                    window_duration_mins: Some(10_080),
                    resets_at: Some(at(next_reset).timestamp()),
                }),
                secondary: None,
            }],
            ..Default::default()
        };
        let mut current_scope = self.scope();
        current_scope.window_instance_id = period.into();
        let proof = AdapterQuotaProof {
            adapter_id: "typed-fixture-adapter".into(),
            reference: "typed-fixture-only".into(),
            native_digest: native_observation_digest(&native),
            scope: current_scope,
            sequence,
            percent: PercentReading {
                used_percent: used,
                rounding_step: self.rounding_step,
                saturated: false,
                saturation_headroom_bounded: false,
            },
            fact_time: Interval::exact(seconds(offset)).unwrap(),
            ttl_start: seconds(offset),
            clock_epoch: "typed-fixture-clock".into(),
            reset_time: Interval::exact(seconds(next_reset)).unwrap(),
            recovery: RecoverySemantics::FixedReset,
            stable_identity_reference: "typed-fixture-stable-identity".into(),
            entitlement_reference: "typed-fixture-rights-evidence".into(),
            rule_reference: "typed-fixture-rules-evidence".into(),
            period_reference: "typed-fixture-period-evidence".into(),
            period_stable_across_runtimes: true,
        };
        let domain = Domain::from_scope(&proof.scope).unwrap();
        let carrier = serde_json::to_value(native).unwrap();
        assert!(carrier.get("identityEvidence").is_none());
        let resets: Vec<_> = transition
            .map(|transition| {
                let previous = self
                    .proofs
                    .last()
                    .expect("transition requires a genuine previous typed observation");
                AdapterResetTransitionProof {
                    adapter_id: "typed-fixture-reset-adapter".into(),
                    reference: "typed-fixture-official-transition".into(),
                    scope: proof.scope.clone(),
                    sequence,
                    native_digest: proof.native_digest.clone(),
                    previous_sequence: previous.sequence,
                    previous_native_digest: previous.native_digest.clone(),
                    previous_window_instance_id: previous.scope.window_instance_id.clone(),
                    current_window_instance_id: period.into(),
                    final_used_percent: (transition == ResetTransition::NewWindow
                        && boundary.is_some())
                    .then_some(88.0),
                    transition,
                    next_reset: Some(proof.reset_time),
                    actual_boundary: boundary,
                }
            })
            .into_iter()
            .collect();
        self.proofs.push(proof);
        let check = json!({"checkedAt":at(offset).to_rfc3339(),"discoveryComplete":true,"cancelled":false,"pendingFiles":0,"issues":0,"discoveryErrors":0});
        put_setting(
            &self.core().db,
            &format!("lastSourceCheck:{}", self.core().source_id),
            &check,
        )
        .unwrap();
        let revision = get_setting(&self.core().db, "subscriptionRevision")
            .unwrap()
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let settled = LedgerSettlement {
            domain: domain.clone(),
            revision,
            complete_interval: Interval::new(seconds(0), seconds(offset)).unwrap(),
            captured_at: seconds(offset),
            source_check_digest: source_check_digest(&check),
            metering_version: adapter::SEMANTICS.into(),
            billing_clock_epoch: "typed-fixture-clock".into(),
            billing_alignment_reference: "typed-fixture-independent-billing-alignment".into(),
        };
        let coverage = [CoverageAttestation {
            domain: domain.clone(),
            interval: settled.complete_interval,
            kind: CoverageKind::VerifiedComplete {
                adapter_id: "typed-fixture-independent-coverage".into(),
                reference: "typed-fixture-all-pool-activity".into(),
            },
        }];
        let modes = [UsageModeAttestation {
            domain,
            interval: settled.complete_interval,
            effort: ExplicitMode::Known("medium".into()),
            speed: if known_speed {
                ExplicitMode::Known("normal".into())
            } else {
                ExplicitMode::Unknown
            },
            adapter_id: "typed-fixture-modes".into(),
            reference: "typed-fixture-explicit-unknown-speed".into(),
        }];
        let batches = [CollectorBatch {
            proofs: &self.proofs,
            settlement: Some(&settled),
            coverage: &coverage,
            modes: &modes,
            reset_transitions: &resets,
        }];
        let result = self
            .core
            .as_mut()
            .unwrap()
            .settle_subscription_observation_with_evidence(1, carrier, &batches)
            .unwrap();
        assert_eq!(
            result["persisted"], true,
            "writer must commit the actual production path: {result}"
        );
        assert!(result["persistenceError"].is_null(), "{result}");
        result
    }
    fn reopen(&mut self) {
        let runtime = self.core().runtime_id.clone();
        drop(self.core.take());
        self.core = Some(Core::open(&self.data, &self.source).unwrap());
        assert_ne!(self.core().runtime_id, runtime);
    }
    fn overview(&self) -> Value {
        self.core().read_subscription_overview().unwrap()
    }
    fn receipts(&self) -> Vec<Value> {
        self.core()
            .db
            .prepare("SELECT data FROM account_snapshots ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
            .collect()
    }
    fn risk_events(&self) -> Vec<Value> {
        self.core().db.prepare("SELECT data FROM inbox_events WHERE json_extract(data,'$.reminderKind')='native_quota_eta'").unwrap()
            .query_map([], |r| r.get::<_, String>(0)).unwrap()
            .map(|r| serde_json::from_str(&r.unwrap()).unwrap()).collect()
    }
}

#[test]
fn typed_evidence_reaches_core_apply_hydration_watch_archive_and_reopens_without_replay() {
    let mut rig = Rig::new();
    for i in 0..7 {
        let result = rig.observe(i + 1, i as i64 * 180, 20.0 + i as f64 * 10.0, i > 0);
        assert_eq!(
            result["calibrationAssembly"]["reasonCodes"],
            json!([]),
            "{result}"
        );
        if i > 0 {
            assert_eq!(result["calibrationAssembly"]["status"], "assembled");
        }
    }
    let overview = rig.overview();
    let active = overview["calibrations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["archived"] != true && c["sampleCount"] == 6)
        .expect("Core must expose actual assembled pairs");
    assert_eq!(active["quality"], "conditional", "{overview}");
    assert!(active["capacityLower"].as_f64().unwrap() > 900_000.0);
    assert!(active["capacityUpper"].as_f64().unwrap() < 1_120_000.0);
    assert!(active.get("inputPairs").is_none() && active.get("evidence").is_none());
    assert_eq!(overview["windows"][0]["risk"]["state"], "risk");
    assert_eq!(overview["windows"][0]["risk"]["notifyCandidate"], true);
    let events = rig.risk_events();
    assert_eq!(
        events.len(),
        1,
        "Two production native risk rounds must create one panel latch"
    );
    assert_eq!(events[0]["notifyEligible"], false);
    assert_eq!(events[0]["animationAcknowledged"], true);
    let receipts = rig.receipts();
    let last = receipts.last().unwrap();
    assert!(last["derivedCalibrationPatches"]["shared:primary"].is_object());
    let original = &last["observation"];
    assert!(original["buckets"][0]["primary"]["roundingStep"].is_null());
    assert!(original["entitlementSegment"].is_null());
    assert!(original["buckets"][0]["primary"]["calibrationEvidence"].is_null());
    let hydrated = super::projector::receipt_snapshot(last).unwrap();
    assert_eq!(hydrated["buckets"][0]["primary"]["roundingStep"], 1.0);
    assert_eq!(hydrated["entitlementSegment"], "typed-fixture-rights");
    let archives: i64 = rig
        .core()
        .db
        .query_row("SELECT COUNT(*) FROM quota_calibrations", [], |r| r.get(0))
        .unwrap();
    assert!(archives > 0);
    rig.reopen();
    let before = rig.core().db.total_changes();
    let reopened = rig.overview();
    assert!(reopened["windows"].as_array().unwrap().is_empty());
    assert!(reopened["calibrations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["quality"] == "invalidated" && c["capacityLower"].is_null()));
    assert_eq!(
        rig.core().db.total_changes(),
        before,
        "reopening queries must not mutate calibration or notification state"
    );
    assert_eq!(rig.risk_events().len(), 1);
    rig.observe(1, 1260, 80.0, false);
    rig.observe(2, 1440, 90.0, true);
    let linked = rig.overview();
    assert!(
        linked["calibrations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["quality"] == "conditional" && c["sampleCount"] == 7),
        "stable typed identity may reuse complete old-runtime pairs: {linked}"
    );
    assert_eq!(
        linked["windows"][0]["risk"]["state"], "unknown",
        "two new-runtime points cannot borrow the prior runtime's native ETA"
    );
    assert_eq!(
        rig.risk_events().len(),
        1,
        "reopen and conditional historical linkage must not replay a risk event"
    );
    let duplicate = rig.observe(3, 1620, 90.0, false);
    assert_eq!(duplicate["calibrationAssembly"]["assembledPairs"], 0);
    assert_eq!(duplicate["buckets"][0]["primary"]["roundingStep"], 1.0);
    assert_eq!(duplicate["observationSequence"], "3");
    let raw = rig.receipts().pop().unwrap();
    assert_eq!(raw["subscriptionStorage"]["encoding"], "reference");
    assert_eq!(
        raw["subscriptionStorage"]["independentTrainingSample"],
        false
    );
    assert!(
        raw.get("observation").is_none(),
        "production apply must not expand a body reference"
    );
    let resolved = super::observation_storage::resolve(&rig.core().db, &raw).unwrap();
    let hydrated = super::projector::receipt_snapshot(&resolved).unwrap();
    assert_eq!(hydrated["observationSequence"], "3");
    assert_eq!(hydrated["buckets"][0]["primary"]["roundingStep"], 1.0);
    assert!(hydrated["buckets"][0]["primary"]["calibrationEvidence"].is_null());
    assert!(rig.overview()["calibrations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["quality"] == "conditional" && c["sampleCount"] == 7));
    assert_eq!(rig.risk_events().len(), 1);
}

fn window_plan(rig: &mut Rig) -> Value {
    let input = json!({"id":null,"expectedRevision":null,"title":"typed fixture official window","type":"percent",
        "sourceId":rig.core().source_id,"model":null,"modelDimension":"response","target":80,
        "endPolicy":"window","endsAt":at(1800).to_rfc3339(),"windowId":"shared:primary","remindersEnabled":false});
    rig.core.as_mut().unwrap().save_usage_plan(&input).unwrap()["plans"][0].clone()
}

#[test]
fn typed_reset_proof_flows_through_core_sqlite_same_window_extension_and_exact_new_window() {
    let mut rig = Rig::new();
    let first = rig.observe(1, 0, 60.0, false);
    assert!(first["identityEvidence"].is_object());
    let plan = window_plan(&mut rig);
    let initial_revision = plan["revision"].clone();
    // A changed native timestamp alone is deliberately insufficient.
    rig.observe_window(
        2,
        1810,
        62.0,
        false,
        "typed-fixture-provider-period",
        3600,
        None,
        None,
        false,
    );
    let waiting = super::window_plans::projection(&rig.core().db, &plan, at(1810)).unwrap();
    assert!(!waiting.same_window && !waiting.archived);
    assert_eq!(waiting.resolved_end, Some(at(1800)));
    rig.observe_window(
        3,
        1811,
        63.0,
        false,
        "typed-fixture-provider-period",
        3600,
        Some(ResetTransition::SameWindow),
        None,
        false,
    );
    let resumed = super::window_plans::projection(&rig.core().db, &plan, at(1811)).unwrap();
    assert!(resumed.same_window && !resumed.archived);
    assert_eq!(resumed.resolved_end, Some(at(3600)));
    let result = rig.observe_window(
        4,
        3610,
        3.0,
        false,
        "typed-fixture-next-period",
        5400,
        Some(ResetTransition::NewWindow),
        Interval::exact(seconds(3600)),
        false,
    );
    assert_eq!(
        result["buckets"][0]["primary"]["resetEvidence"]["confirmedNewWindow"],
        true
    );
    let ended = super::window_plans::projection(&rig.core().db, &plan, at(3610)).unwrap();
    assert!(ended.archived && !ended.partial && !ended.same_window);
    assert_eq!(ended.resolved_end, Some(at(3600)));
    assert_eq!(ended.final_percent, Some(88.0));
    let raw = rig.receipts().last().unwrap().clone();
    let resolved = super::observation_storage::resolve(&rig.core().db, &raw).unwrap();
    assert!(resolved["derivedObservationProof"].is_object());
    assert!(resolved["observation"].get("identityEvidence").is_none());
    assert!(resolved["observation"]["buckets"][0]["primary"]
        .get("resetEvidence")
        .is_none());
    assert_eq!(plan["revision"], initial_revision);
    assert_eq!(
        DateTime::parse_from_rfc3339(plan["endsAt"].as_str().unwrap())
            .unwrap()
            .with_timezone(&Utc),
        at(1800)
    );
    rig.reopen();
    let archived = super::window_plans::projection(&rig.core().db, &plan, at(3610)).unwrap();
    assert!(archived.archived && !archived.partial);
    assert_eq!(archived.resolved_end, Some(at(3600)));
    assert!(
        rig.overview()["windows"].as_array().unwrap().is_empty(),
        "stored proof cannot impersonate a fresh current runtime identity"
    );
}

#[test]
fn typed_new_window_without_actual_boundary_archives_partial_through_core() {
    let mut rig = Rig::new();
    rig.observe(1, 0, 60.0, false);
    let plan = window_plan(&mut rig);
    rig.observe_window(
        2,
        1810,
        3.0,
        false,
        "typed-fixture-next-period",
        3600,
        Some(ResetTransition::NewWindow),
        None,
        false,
    );
    let ended = super::window_plans::projection(&rig.core().db, &plan, at(1810)).unwrap();
    assert!(ended.archived && ended.partial && !ended.same_window);
    assert!(ended.resolved_end.is_none() && ended.final_percent.is_none());
}

#[test]
fn core_cannot_admit_native_json_without_matching_typed_proof() {
    let mut rig = Rig::new();
    let first = rig.observe(1, 0, 60.0, false);
    for sequence in [2, 3] {
        rig.clock.set(reading(sequence as i64));
        let mut forged = first.clone();
        forged["observationSequence"] = json!(sequence.to_string());
        forged["identityEvidence"] = json!({"verified":true,"reference":"untrusted-json"});
        let mut wrong = rig.proofs[0].clone();
        wrong.sequence = sequence; // Digest still binds sequence 1, so this is not a proof of the new native DTO.
        let proofs = [wrong];
        let batches = [CollectorBatch {
            proofs: &proofs,
            settlement: None,
            coverage: &[],
            modes: &[],
            reset_transitions: &[],
        }];
        let batches = if sequence == 2 { &[][..] } else { &batches[..] };
        let result = rig
            .core
            .as_mut()
            .unwrap()
            .settle_subscription_observation_with_evidence(1, forged, batches)
            .unwrap();
        assert!(result.get("identityEvidence").is_none(), "{result}");
        let raw = rig.receipts().last().unwrap().clone();
        assert!(
            raw["observation"].is_null(),
            "failure/minimal receipt must not preserve self-attested official data"
        );
        assert!(raw["derivedObservationProof"].is_null());
    }
}

#[test]
fn current_settled_known_modes_generate_manual_structure_through_core_without_capacity_promotion() {
    let mut rig = Rig::new();
    rig.observe_window(
        1,
        0,
        20.0,
        false,
        "typed-fixture-provider-period",
        1800,
        None,
        None,
        true,
    );
    let current = rig.observe_window(
        2,
        180,
        30.0,
        true,
        "typed-fixture-provider-period",
        1800,
        None,
        None,
        true,
    );
    let structure = &current["buckets"][0]["primary"]["usageStructureEvidence"];
    assert_eq!(structure["source"], "trusted-ledger");
    assert_eq!(structure["profile"]["speedMode"], "normal");
    assert_eq!(structure["model"], "actual-model");
    for model in [Value::Null, json!("actual-model")] {
        let payload = json!({"id":null,"expectedRevision":null,"sourceId":rig.core().source_id,"windowId":"shared:primary",
            "model":model,"capacityTokens":2_000_000,"validUntil":at(1700).to_rfc3339(),"note":"synthetic typed manual premise only"});
        let overview = rig
            .core
            .as_mut()
            .unwrap()
            .save_quota_assumption(&payload)
            .unwrap();
        let latest = overview["assumptions"].as_array().unwrap().last().unwrap();
        assert_eq!(latest["manualScenario"]["active"], true, "{overview}");
        assert_eq!(latest["manualScenario"]["quality"], "conditional");
        assert_eq!(latest["manualScenario"]["constraintsVerified"], false);
    }
    let raw = rig.receipts().last().unwrap().clone();
    let resolved = super::observation_storage::resolve(&rig.core().db, &raw).unwrap();
    assert!(resolved["observation"]["buckets"][0]["primary"]
        .get("usageStructureEvidence")
        .is_none());
    assert!(
        resolved["derivedCalibrationPatches"]["shared:primary"]["windowFields"]
            ["usageStructureEvidence"]
            .is_object()
    );
    let rows = crate::core::queries::subscription_responses(rig.core()).unwrap();
    let before = rig.core().db.total_changes();
    assert!(super::calibration_assembler::usage_structure_current(
        &current,
        "shared:primary",
        &rows
    ));
    assert_eq!(
        rig.core().db.total_changes(),
        before,
        "query validation must not renew or write evidence"
    );
    rig.core().db.execute("UPDATE usage_responses SET data=json_set(data,'$.observedModel','same-total-corrected-model') WHERE response_id='response-90'", []).unwrap();
    let rows = crate::core::queries::subscription_responses(rig.core()).unwrap();
    assert!(!super::calibration_assembler::usage_structure_current(
        &current,
        "shared:primary",
        &rows
    ));
    assert!(
        rig.overview()["assumptions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|value| value["manualScenario"]["active"] == false),
        "RAM profile cannot survive a same-total canonical model correction"
    );
    rig.reopen();
    assert!(rig.overview()["assumptions"]
        .as_array()
        .unwrap()
        .iter()
        .all(|value| value["manualScenario"]["active"] == false));
}

#[test]
fn low_signal_and_same_value_reads_can_prove_manual_structure_without_capacity_samples() {
    for (quantum, used) in [(Some(1.0), 20.0), (Some(1.0), 22.0), (None, 20.0)] {
        let mut rig = Rig::new();
        rig.rounding_step = quantum;
        rig.observe_window(
            1,
            0,
            20.0,
            false,
            "typed-fixture-provider-period",
            1800,
            None,
            None,
            true,
        );
        let current = rig.observe_window(
            2,
            180,
            used,
            true,
            "typed-fixture-provider-period",
            1800,
            None,
            None,
            true,
        );
        let window = &current["buckets"][0]["primary"];
        let structure = &window["usageStructureEvidence"];
        assert_eq!(
            structure["basis"]["version"], "settled-usage-profile-v1",
            "{current}"
        );
        assert_eq!(structure["profile"]["speedMode"], "normal");
        assert!(window.get("calibrationEvidence").is_none());
        assert_eq!(
            rig.core()
                .db
                .query_row(
                    "SELECT COUNT(*) FROM settings WHERE key LIKE 'subscriptionHoldout:%'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        let overview = rig.overview();
        assert!(overview["calibrations"]
            .as_array()
            .unwrap()
            .iter()
            .all(|card| card["sampleCount"] == 0));
        let raw = rig.receipts().last().unwrap().clone();
        if used == 20.0 {
            assert_eq!(raw["subscriptionStorage"]["encoding"], "reference");
            let resolved = super::observation_storage::resolve(&rig.core().db, &raw).unwrap();
            assert_eq!(
                resolved["storageEvidence"]["independentTrainingSample"],
                false
            );
        }
        let payload = json!({"id":null,"expectedRevision":null,"sourceId":rig.core().source_id,"windowId":"shared:primary",
            "model":"actual-model","capacityTokens":2_000_000,"validUntil":at(1700).to_rfc3339(),"note":"synthetic manual premise; no capacity training"});
        let overview = rig
            .core
            .as_mut()
            .unwrap()
            .save_quota_assumption(&payload)
            .unwrap();
        let assumption = overview["assumptions"].as_array().unwrap().last().unwrap();
        assert_eq!(assumption["manualScenario"]["active"], true, "{overview}");
        assert_eq!(assumption["manualScenario"]["constraintsVerified"], false);
        let rows = crate::core::queries::subscription_responses(rig.core()).unwrap();
        assert!(super::calibration_assembler::usage_structure_current(
            &current,
            "shared:primary",
            &rows
        ));
        rig.write_usage(181);
        let rows = crate::core::queries::subscription_responses(rig.core()).unwrap();
        assert!(!super::calibration_assembler::usage_structure_current(
            &current,
            "shared:primary",
            &rows
        ));
        assert!(rig.overview()["assumptions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|value| value["manualScenario"]["active"] == false));
    }
}

#[test]
fn manual_structure_is_only_the_settled_half_open_interval_not_newer_unsettled_usage() {
    let mut rig = Rig::new();
    rig.observe_window(
        1,
        0,
        20.0,
        false,
        "typed-fixture-provider-period",
        1800,
        None,
        None,
        true,
    );
    let current = rig.observe_window(
        2,
        180,
        30.0,
        true,
        "typed-fixture-provider-period",
        1800,
        None,
        None,
        true,
    );
    let rows = crate::core::queries::subscription_responses(rig.core()).unwrap();
    assert!(super::calibration_assembler::usage_structure_current(
        &current,
        "shared:primary",
        &rows
    ));
    rig.write_usage(180); // Exactly the measured end is excluded by [start, end).
    let rows = crate::core::queries::subscription_responses(rig.core()).unwrap();
    assert!(!super::calibration_assembler::usage_structure_current(
        &current,
        "shared:primary",
        &rows
    ));
}

#[test]
fn twenty_five_real_core_pairs_open_token_draft_only_after_the_fifth_frozen_holdout() {
    let mut rig = Rig::new();
    let mut sequence = 0u64;
    let mut before_fifth = None;
    for (period, pair_count) in [7, 7, 11].into_iter().enumerate() {
        let base = period as i64 * 10_000;
        let instance = format!("typed-fixture-period-{period}");
        for step in 0..=pair_count {
            sequence += 1;
            let transition = (period > 0 && step == 0).then_some(ResetTransition::NewWindow);
            let boundary = transition
                .as_ref()
                .and_then(|_| Interval::exact(seconds(base - 1000)));
            rig.observe_window(
                sequence,
                base + step as i64 * 180,
                10.0 + step as f64 * 5.0,
                step > 0,
                &instance,
                base + 9000,
                transition,
                boundary,
                true,
            );
            if period == 2 && step == 10 {
                let payload = json!({"expectedScope":rig.core().subscription_scope_token().unwrap(),"intent":"remaining",
                    "sourceId":rig.core().source_id,"windowId":"shared:primary","model":null,"modelDimension":"response","reservePercent":20});
                let changes = rig.core().db.total_changes();
                let draft = rig
                    .core
                    .as_mut()
                    .unwrap()
                    .subscription_command("draft", &payload)
                    .unwrap();
                assert_eq!(
                    rig.core().db.total_changes(),
                    changes,
                    "draft query must not write or renew evidence"
                );
                assert_eq!(
                    draft["plan"]["type"], "percent",
                    "four held-out pairs cannot open G4b: {draft}"
                );
                before_fifth = Some(draft);
            }
        }
    }
    assert!(before_fifth.is_some());
    let payload = json!({"expectedScope":rig.core().subscription_scope_token().unwrap(),"intent":"remaining",
        "sourceId":rig.core().source_id,"windowId":"shared:primary","model":null,"modelDimension":"response","reservePercent":20});
    let changes = rig.core().db.total_changes();
    let draft = rig
        .core
        .as_mut()
        .unwrap()
        .subscription_command("draft", &payload)
        .unwrap();
    assert_eq!(rig.core().db.total_changes(), changes);
    assert_eq!(draft["plan"]["type"], "tokens", "twenty training plus five frozen held-out pairs across three proven fixture periods must reach the real command: {draft}");
    assert_eq!(draft["plan"]["sourceId"], rig.core().source_id);
    assert_eq!(draft["plan"]["modelDimension"], "response");
    assert!(draft["plan"]["model"].is_null());
    assert_eq!(draft["plan"]["endPolicy"], "time");
    assert!(draft["plan"]["windowId"].is_null());
    assert_eq!(draft["constraintsVerified"], true);
    let target = draft["plan"]["target"].as_f64().unwrap();
    // 100k per 5pp gives a 2m empirical capacity. At used65%, known-q
    // propagation and the 20% reserve put the conservative target near460k.
    assert!((450_000.0..470_000.0).contains(&target), "{draft}");
    let end = DateTime::parse_from_rfc3339(draft["plan"]["endsAt"].as_str().unwrap())
        .unwrap()
        .with_timezone(&Utc);
    assert_eq!(end, at(29_000));
    assert_eq!(
        rig.core()
            .db
            .query_row("SELECT COUNT(*) FROM usage_plans", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0,
        "draft must remain editable and unsaved"
    );
    let mut save = draft["plan"].clone();
    save["expectedScope"] = json!(rig.core().subscription_scope_token().unwrap());
    let saved = rig
        .core
        .as_mut()
        .unwrap()
        .subscription_command("save", &save)
        .unwrap();
    let plan = &saved["plans"][0];
    assert_eq!(plan["target"].as_f64(), Some(target));
    assert_eq!(plan["sourceId"], rig.core().source_id);
    assert_eq!(plan["modelDimension"], "response");
    assert!(plan["model"].is_null());
    assert_eq!(plan["endPolicy"], "time");
    assert_eq!(
        DateTime::parse_from_rfc3339(plan["endsAt"].as_str().unwrap())
            .unwrap()
            .with_timezone(&Utc),
        end
    );
}

#[test]
fn three_typed_unknown_quantum_pairs_publish_only_an_empirical_conditional_point() {
    let mut rig = Rig::new();
    rig.rounding_step = None;
    let mut sequence = 0;
    // The repeated native readings keep real sequence/time receipts, without
    // becoming independent training endpoints or adding fictional zero usage.
    for (offset, used, usage) in [
        (0, 10.0, false),
        (60, 10.0, false),
        (120, 10.0, false),
        (300, 15.0, true),
        (420, 15.0, false),
        (600, 20.0, true),
        (720, 20.0, false),
        (900, 25.0, true),
    ] {
        sequence += 1;
        rig.observe_window(
            sequence,
            offset,
            used,
            usage,
            "unknown-q-fixture-period",
            9000,
            None,
            None,
            true,
        );
    }
    let overview = rig.overview();
    let current = overview["calibrations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["archived"] != true)
        .unwrap();
    assert_eq!(current["quality"], "conditional", "{overview}");
    assert_eq!(current["sampleCount"], 3);
    assert_eq!(current["empiricalCapacity"], 2_000_000.0);
    for key in [
        "capacityLower",
        "capacityUpper",
        "remainingLower",
        "remainingUpper",
    ] {
        assert!(
            current[key].is_null(),
            "unknown q has no bounded {key}: {current}"
        );
    }
    assert!(current["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|reason| reason == "precision_unknown"));
    let risk = &overview["windows"][0]["risk"];
    assert_eq!(risk["state"], "unknown");
    assert!(risk["exhaustsAtLower"].is_null() && risk["exhaustsAtUpper"].is_null());
    assert_eq!(risk["notifyCandidate"], false);
    assert_eq!(risk["notifyEligible"], false);
    assert!(risk["reasons"].as_array().unwrap().iter().any(|reason| reason == "precision_unknown"), "eight continuous real native observations must specifically refuse bounded q-unknown ETA: {risk}");
    let payload = json!({"expectedScope":rig.core().subscription_scope_token().unwrap(),"intent":"remaining",
        "sourceId":rig.core().source_id,"windowId":"shared:primary","model":null,"modelDimension":"response","reservePercent":20});
    let changes = rig.core().db.total_changes();
    let draft = rig
        .core
        .as_mut()
        .unwrap()
        .subscription_command("draft", &payload)
        .unwrap();
    assert_eq!(rig.core().db.total_changes(), changes);
    assert_eq!(
        draft["plan"]["type"], "percent",
        "an empirical point must never open the validated Token draft gate: {draft}"
    );
    assert!(draft["sustainability"].is_null());
}
