use super::*;
use std::collections::VecDeque;

struct Rpc {
    values: VecDeque<Result<Value, String>>,
    calls: Vec<String>,
    timeouts: Vec<Duration>,
}
impl Rpc {
    fn values(values: Vec<Result<Value, String>>) -> Self {
        Self {
            values: values.into(),
            calls: vec![],
            timeouts: vec![],
        }
    }
    fn success(usage: Result<Value, String>) -> Self {
        Self::values(vec![
            Ok(json!({"account":{"type":"chatgpt","email":"PRIVATE_EMAIL", "planType":"pro"}})),
            Ok(json!({"rateLimits":{"primary":{"usedPercent":17,"resetsAt":1900000000}}})),
            usage,
        ])
    }
}
impl ReadRpc for Rpc {
    fn read(&mut self, method: &str, _: Value) -> Result<Value, String> {
        assert!(ALLOWED.contains(&method));
        self.calls.push(method.into());
        self.values.pop_front().expect("unexpected request")
    }
    fn read_with_timeout(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        self.timeouts.push(timeout);
        self.read(method, params)
    }
}
fn run(bridge: &mut AccountBridge, rpc: &mut Rpc) -> AccountSnapshot {
    bridge.begin_account_read().unwrap();
    let result = read_account_snapshot(rpc, &mut bridge.snapshot, &mut bridge.identity_observation);
    bridge.complete_account_read(result)
}

#[test]
fn bridge_m0_returns_only_unproven_sanitized_membership_and_unbounded_times() {
    let mut bridge = AccountBridge::new();
    let mut rpc = Rpc::success(Ok(json!({"summary":{"lifetimeTokens":42}})));
    let snapshot = run(&mut bridge, &mut rpc);
    assert_eq!(snapshot.plan_type.as_deref(), Some("pro"));
    assert_eq!(snapshot.observation_sequence.as_deref(), Some("1"));
    assert_eq!(snapshot.observation_outcome.as_deref(), Some("success"));
    assert_eq!(snapshot.identity_epoch, "1");
    assert_eq!(snapshot.identity_consistency, "unproven");
    assert!(snapshot.identity_consistency_reason.contains("待确认"));
    assert!(snapshot.account_scope.is_none());
    assert_eq!(
        rpc.calls,
        [
            "account/read",
            "account/rateLimits/read",
            "account/usage/read"
        ]
    );
    for read in [
        &snapshot.account_read,
        &snapshot.quota_read,
        &snapshot.usage_read,
    ] {
        let read = read.as_ref().unwrap();
        assert!(read.fact_time_lower.is_none() && read.fact_time_upper.is_none());
        assert_eq!(read.time_basis, "unbounded-upstream");
        let started = chrono::DateTime::parse_from_rfc3339(&read.request_started_at).unwrap();
        let returned = chrono::DateTime::parse_from_rfc3339(&read.response_received_at).unwrap();
        assert!(started <= returned);
        assert_eq!(
            read.freshness_anchor.as_ref(),
            Some(&read.request_started_at)
        );
    }
    assert_eq!(
        snapshot.collected_at,
        snapshot.quota_read.as_ref().unwrap().freshness_anchor
    );
    assert!(snapshot.read_started_at.is_some() && snapshot.read_finished_at.is_some());
    assert!(!serde_json::to_string(&snapshot)
        .unwrap()
        .contains("PRIVATE_EMAIL"));
    assert!(rpc
        .timeouts
        .iter()
        .all(|timeout| *timeout <= REQUEST_TIMEOUT));
}

#[test]
fn bridge_m0_identical_reads_are_separate_sequences_and_isolated_identity_segments() {
    let mut bridge = AccountBridge::new();
    let first = run(&mut bridge, &mut Rpc::success(Err(UNSUPPORTED.into())));
    let second = run(&mut bridge, &mut Rpc::success(Err(UNSUPPORTED.into())));
    assert_eq!(first.observation_sequence.as_deref(), Some("1"));
    assert_eq!(second.observation_sequence.as_deref(), Some("2"));
    assert_ne!(first.identity_epoch, second.identity_epoch);
    assert_eq!(second.identity_consistency, "unproven");
}

#[test]
fn bridge_m0_read_failure_keeps_old_facts_but_has_new_failed_sequence_and_backoff() {
    let mut bridge = AccountBridge::new();
    let first = run(&mut bridge, &mut Rpc::success(Err(UNSUPPORTED.into())));
    let failed = run(
        &mut bridge,
        &mut Rpc::values(vec![Err("账户读取连接中断".into())]),
    );
    assert_eq!(failed.observation_sequence.as_deref(), Some("2"));
    assert_eq!(failed.observation_outcome.as_deref(), Some("failure"));
    assert_eq!(failed.status, "stale");
    assert_eq!(failed.collected_at, first.collected_at);
    assert_eq!(
        failed.buckets[0].primary.as_ref().unwrap().used_percent,
        17.0
    );
    assert_eq!(failed.account_read.as_ref().unwrap().outcome, "failure");
    assert!(failed.quota_read.is_none() && failed.usage_read.is_none());
    assert_eq!(bridge.failure_count, 1);
    assert!(bridge.next_attempt.is_some());
}

#[test]
fn bridge_m0_admission_refusal_does_not_allocate_or_replay_a_read_sequence() {
    let mut bridge = AccountBridge::new();
    run(&mut bridge, &mut Rpc::success(Err(UNSUPPORTED.into())));
    bridge.next_attempt = Some(Instant::now() + Duration::from_secs(60));
    let snapshot = bridge.refresh_with(|_, _, _, _| panic!("backoff must not start transport"));
    assert_eq!(bridge.observation_sequence, 1);
    assert_eq!(bridge.identity_epoch, 1);
    assert!(snapshot.observation_sequence.is_none());
    assert!(snapshot.observation_outcome.is_none());
    assert!(snapshot.read_started_at.is_none() && snapshot.read_finished_at.is_none());
    assert_eq!(snapshot.error.as_deref(), Some(DEFERRED));
}

#[test]
fn bridge_m0_exhaustion_rejects_before_transport_or_counter_wrap() {
    for identity_exhausted in [false, true] {
        let mut bridge = AccountBridge::new();
        if identity_exhausted {
            bridge.identity_epoch = u64::MAX;
        } else {
            bridge.observation_sequence = u64::MAX;
        }
        let snapshot =
            bridge.refresh_with(|_, _, _, _| panic!("exhaustion must not start transport"));
        assert!(snapshot.error.unwrap().contains("耗尽"));
        assert!(snapshot.observation_sequence.is_none());
        assert!(bridge.begin_account_read().is_err());
    }
}

#[test]
fn bridge_m0_clear_preserves_sequence_and_advances_identity_without_reading() {
    let mut bridge = AccountBridge::new();
    run(&mut bridge, &mut Rpc::success(Err(UNSUPPORTED.into())));
    bridge.clear_cached_data();
    assert_eq!(bridge.observation_sequence, 1);
    assert_eq!(bridge.identity_epoch, 2);
    assert_eq!(bridge.snapshot.identity_epoch, "2");
    assert!(bridge.snapshot.observation_sequence.is_none());
    let next = run(&mut bridge, &mut Rpc::success(Err(UNSUPPORTED.into())));
    assert_eq!(next.observation_sequence.as_deref(), Some("2"));
    assert_eq!(next.identity_epoch, "3");
}

#[test]
fn bridge_m0_usage_rejection_is_local_but_transport_timeout_cancel_are_whole_failures() {
    for error in [
        UNSUPPORTED,
        RPC_REJECTED,
        "账户读取连接中断",
        "账户读取超时，稍后重试",
        "桥接正在退出",
    ] {
        let mut bridge = AccountBridge::new();
        let snapshot = run(&mut bridge, &mut Rpc::success(Err(error.into())));
        let local = error == UNSUPPORTED || error == RPC_REJECTED;
        assert_eq!(
            snapshot.observation_outcome.as_deref(),
            Some(if local { "success" } else { "failure" })
        );
        assert_eq!(
            snapshot.usage_read.as_ref().unwrap().outcome,
            if local { "unavailable" } else { "failure" }
        );
        assert_eq!(bridge.failure_count, u32::from(!local));
        if local {
            assert_eq!(snapshot.buckets.len(), 1);
        } else {
            assert!(snapshot.buckets.is_empty());
        }
    }
}

#[test]
fn bridge_m0_budget_expiry_stops_before_additional_rpc_and_does_not_invent_timing() {
    let mut rpc = Rpc::values(vec![]);
    let mut metadata = None;
    let error = account_sub_read(
        &mut rpc,
        "account/read",
        json!({"refreshToken":false}),
        Instant::now() - Duration::from_millis(1),
        &mut metadata,
    )
    .unwrap_err();
    assert!(error.contains("整轮读取超时"));
    assert!(rpc.calls.is_empty());
    assert!(metadata.is_none());
}

#[test]
fn bridge_m0_extra_untrusted_fields_and_identical_endpoints_never_prove_identity_or_time() {
    let mut bridge = AccountBridge::new();
    let mut rpc = Rpc::values(vec![
        Ok(json!({"account":{"type":"chatgpt","email":"A", "planType":"pro"},"revision":42})),
        Ok(
            json!({"accountId":"B", "identityConsistency":"proved", "observedAt":"2026-10-22T00:00:00Z", "rateLimits":{"primary":{"usedPercent":17}}}),
        ),
        Err(UNSUPPORTED.into()),
    ]);
    let snapshot = run(&mut bridge, &mut rpc);
    assert_eq!(snapshot.identity_consistency, "unproven");
    assert!(snapshot.account_scope.is_none());
    assert!(snapshot.quota_read.unwrap().fact_time_lower.is_none());
    assert_eq!(
        rpc.calls.len(),
        3,
        "no unqualified endpoint sandwich is added"
    );
}

#[test]
fn bridge_m0_cached_capability_rejection_has_no_fabricated_rpc_timestamps() {
    struct Unsupported;
    impl ReadRpc for Unsupported {
        fn read(&mut self, _: &str, _: Value) -> Result<Value, String> {
            panic!("cached unsupported capability must not issue an RPC")
        }
        fn known_unsupported(&self, _: &str) -> bool {
            true
        }
    }
    let mut metadata = None;
    let result = account_sub_read(
        &mut Unsupported,
        "account/usage/read",
        json!({}),
        Instant::now() + ACCOUNT_READ_BUDGET,
        &mut metadata,
    );
    assert_eq!(result.unwrap_err(), UNSUPPORTED);
    assert!(metadata.is_none());
}
