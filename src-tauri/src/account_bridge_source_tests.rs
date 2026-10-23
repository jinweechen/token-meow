//! 真实自有 Job/管道运输闭环；只启动当前 Rust 测试二进制，不启动 Codex 或账户服务。
use super::*;
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use windows_sys::Win32::{
    Foundation::WAIT_OBJECT_0,
    System::{JobObjects::IsProcessInJob, Threading::WaitForSingleObject},
};

const CHILD_TEST: &str = "account_bridge::source_transport_tests::source_bound_rpc_child";
const MARKER: &str = "BRIDGE_SOURCE_JSON=";
const HOME_PROOF: &str = "TOKEN_MEOW_PRIVATE_SOURCE_EXPECTED_HOME";
const TAG: &str = "TOKEN_MEOW_PRIVATE_SOURCE_TAG";
const PERCENT: &str = "TOKEN_MEOW_PRIVATE_SOURCE_PERCENT";
const FAIL_ACCOUNT: &str = "TOKEN_MEOW_PRIVATE_SOURCE_FAIL_ACCOUNT";

#[test]
fn source_bound_rpc_child() {
    let Some(expected) = std::env::var_os(HOME_PROOF) else {
        return;
    };
    // 只看测试明确注入的目录字符串，不打开目录、不读取任何 HOME、认证或日志文件。
    assert_eq!(std::env::var_os("CODEX_HOME"), Some(expected));
    let tag = std::env::var(TAG).expect("合成 fixture 缺少代次");
    let percent: u64 = std::env::var(PERCENT).unwrap().parse().unwrap();
    let fail_account = std::env::var(FAIL_ACCOUNT).as_deref() == Ok("true");
    let mut output = std::io::stdout().lock();
    let input = std::io::stdin();
    let account_id = if tag.starts_with('a') {
        "synthetic-account-a"
    } else {
        "synthetic-account-b"
    };
    for line in input.lock().lines() {
        let line = line.unwrap();
        let request: Value = serde_json::from_str(&line).unwrap();
        let Some(id) = request.get("id") else {
            continue;
        };
        let method = request["method"].as_str().unwrap_or_default();
        assert!(ALLOWED.contains(&method), "fixture 不接受写入或推理方法");
        let result = match method {
            "initialize" => {
                json!({"userAgent":"owned-source-fixture","platformFamily":"windows","selectedHomeExact":true,"generation":tag})
            }
            "account/read" => {
                assert_eq!(request["params"]["refreshToken"], false);
                if fail_account {
                    writeln!(
                        output,
                        "{MARKER}{}",
                        json!({"id":id,"error":{"code":-32000,"message":"合成账户拒绝"}})
                    )
                    .unwrap();
                    output.flush().unwrap();
                    continue;
                }
                json!({"account":{"type":"chatgpt","accountId":account_id,"organizationId":"synthetic-org","subscriptionId":"synthetic-subscription"}})
            }
            "account/rateLimits/read" => {
                json!({"rateLimits":{"primary":{"usedPercent":percent,"windowDurationMins":300,"resetsAt":1900000000}}})
            }
            "account/usage/read" => json!({"summary":{"totalTokens":percent}}),
            "thread/list" => {
                assert_eq!(request["params"]["useStateDbOnly"], true);
                json!({"data":[{"id":"019c6e27-e55b-73d1-87d8-4e01f1f75043","name":tag}],"nextCursor":null})
            }
            _ => unreachable!(),
        };
        // libtest 自身有状态行；只由测试侧剥掉这个固定标记，生产 Transport 仍处理完整 JSON RPC。
        writeln!(output, "{MARKER}{}", json!({"id":id,"result":result})).unwrap();
        output.flush().unwrap();
    }
}

fn fixture_transport(
    cancel: Arc<AtomicBool>,
    selected: Option<&Path>,
    resource: Arc<AtomicU8>,
    diagnostics: Arc<BridgeDiagnostics>,
    expected: &Path,
    tag: &str,
    percent: u64,
    fail: bool,
) -> (Transport, OwnedHandle) {
    assert_eq!(selected, Some(expected));
    let args = ["--exact", CHILD_TEST, "--nocapture", "--test-threads=1"].map(OsString::from);
    let overrides = vec![
        (
            OsString::from("CODEX_HOME"),
            expected.as_os_str().to_owned(),
        ),
        (HOME_PROOF.into(), expected.as_os_str().to_owned()),
        (TAG.into(), tag.into()),
        (PERCENT.into(), percent.to_string().into()),
        (FAIL_ACCOUNT.into(), fail.to_string().into()),
    ];
    let (mut child, input, output) = win_process::OwnedProcess::test_suspended(
        &args,
        &overrides,
        cancel.clone(),
        resource.clone(),
        Some(win_process::WARNING_COMMIT),
    )
    .unwrap();
    let witness = child.process.try_clone().unwrap();
    unsafe {
        let mut inside = 0;
        assert_ne!(
            IsProcessInJob(
                child.process.as_raw_handle(),
                child.job_handle(),
                &mut inside
            ),
            0
        );
        assert_eq!(inside, 1, "测试子进程必须先绑定自有 Job 再执行");
    }
    let (tx, messages) = mpsc::sync_channel(64);
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            let line = match line {
                Ok(line) => line,
                Err(_) => {
                    let _ = tx.send(Err("合成管道读取失败".into()));
                    break;
                }
            };
            let Some(index) = line.find(MARKER) else {
                continue;
            };
            let value = serde_json::from_str(&line[index + MARKER.len()..])
                .map_err(|_| "合成 RPC 格式无效".to_owned());
            if tx.send(value).is_err() {
                break;
            }
        }
    });
    child.resume().unwrap();
    let mut transport = Transport {
        child,
        stdin: Some(input),
        messages: Some(messages),
        reader: Some(reader),
        next_id: 0,
        cancel,
        resource_stop: resource,
        diagnostics,
        unsupported: HashSet::new(),
    };
    let handshake = transport
        .request(
            "initialize",
            json!({"clientInfo":{"name":"source-isolation-test"}}),
        )
        .unwrap();
    assert_eq!(handshake["selectedHomeExact"], true);
    assert_eq!(handshake["generation"], tag);
    transport
        .write(&json!({"method":"initialized","params":{}}))
        .unwrap();
    (transport, witness)
}

fn read_fixture(
    bridge: &mut AccountBridge,
    root: &Path,
    tag: &str,
    percent: u64,
    fail: bool,
) -> (AccountSnapshot, OwnedHandle) {
    let mut witness = None;
    let snapshot = bridge.refresh_with(|cancel, selected, resource, diagnostics| {
        let (transport, handle) = fixture_transport(
            cancel,
            selected,
            resource,
            diagnostics,
            root,
            tag,
            percent,
            fail,
        );
        witness = Some(handle);
        Ok(transport)
    });
    (snapshot, witness.expect("必须启动独立合成运输"))
}
fn assert_exited(witness: &OwnedHandle) {
    unsafe {
        assert_eq!(
            WaitForSingleObject(witness.as_raw_handle(), 1000),
            WAIT_OBJECT_0,
            "旧自有运输进程应已退出"
        );
    }
}
fn assert_snapshot(snapshot: &AccountSnapshot, percent: f64, sequence: &str) {
    assert_eq!(snapshot.status, "connected");
    assert_eq!(snapshot.buckets.len(), 1);
    assert_eq!(
        snapshot.buckets[0].primary.as_ref().unwrap().used_percent,
        percent
    );
    assert_eq!(snapshot.observation_sequence.as_deref(), Some(sequence));
    assert_eq!(snapshot.identity_consistency, "unproven");
    assert!(snapshot.collected_at.is_some());
}
fn roots() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("source-a 空格&$");
    let b = temp.path().join("source-b 中文&$");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    (temp, a, b)
}

#[test]
fn owned_rpc_source_a_b_a_uses_exact_home_and_reaps_previous_processes() {
    let (_temp, a, b) = roots();
    let mut bridge = AccountBridge::with_source_root(&a);
    let cancel = bridge.cancel_handle();
    let (first, first_process) = read_fixture(&mut bridge, &a, "a-first", 11, false);
    assert_snapshot(&first, 11.0, "1");
    assert_eq!(bridge.list_sessions().unwrap()[0]["title"], "a-first");
    bridge.set_source_root(&b);
    assert_exited(&first_process);
    assert!(bridge.snapshot.collected_at.is_none() && bridge.snapshot.buckets.is_empty());
    let (second, second_process) = read_fixture(&mut bridge, &b, "b-middle", 22, false);
    assert_snapshot(&second, 22.0, "2");
    assert_ne!(first.account_scope, second.account_scope);
    assert_eq!(bridge.list_sessions().unwrap()[0]["title"], "b-middle");
    bridge.set_source_root(&a);
    assert_exited(&second_process);
    let (third, third_process) = read_fixture(&mut bridge, &a, "a-return", 33, false);
    assert_snapshot(&third, 33.0, "3");
    assert_eq!(first.account_scope, third.account_scope);
    assert_ne!(first.identity_epoch, third.identity_epoch);
    assert!(Arc::ptr_eq(&cancel, &bridge.cancel_handle()));
    assert_eq!(bridge.list_sessions().unwrap()[0]["title"], "a-return");
    bridge.shutdown();
    assert_exited(&third_process);
}

#[test]
fn owned_rpc_new_source_failure_cannot_reuse_previous_quota_or_directory() {
    let (_temp, a, b) = roots();
    let mut bridge = AccountBridge::with_source_root(&a);
    let (first, first_process) = read_fixture(&mut bridge, &a, "a-first", 11, false);
    assert_snapshot(&first, 11.0, "1");
    bridge.set_source_root(&b);
    assert_exited(&first_process);
    let (failed, failed_process) = read_fixture(&mut bridge, &b, "b-rejected", 22, true);
    assert_eq!(failed.status, "unavailable");
    assert!(
        failed.collected_at.is_none()
            && failed.buckets.is_empty()
            && failed.account_scope.is_none()
    );
    assert_eq!(failed.observation_sequence.as_deref(), Some("2"));
    assert_eq!(failed.observation_outcome.as_deref(), Some("failure"));
    assert_exited(&failed_process);
    assert!(bridge.transport.is_none());
    assert_eq!(bridge.list_sessions().unwrap_err(), DEFERRED);
    bridge.set_source_root(&a);
    bridge.retry_manually();
    let (fresh, fresh_process) = read_fixture(&mut bridge, &a, "a-fresh", 33, false);
    assert_snapshot(&fresh, 33.0, "3");
    bridge.clear_cached_data();
    assert_exited(&fresh_process);
    assert!(
        bridge.snapshot.collected_at.is_none()
            && bridge.snapshot.buckets.is_empty()
            && bridge.identity_observation.is_none()
    );
}

#[test]
fn owned_rpc_resource_stop_survives_source_switch_and_cancel_blocks_restart() {
    let (_temp, a, b) = roots();
    let mut bridge = AccountBridge::with_source_root(&a);
    let (_, process) = read_fixture(&mut bridge, &a, "a-first", 11, false);
    bridge
        .resource_stop
        .store(RESOURCE_WARNING, Ordering::Release);
    assert_eq!(
        bridge.observe_resource_stop(),
        resource_error(RESOURCE_WARNING)
    );
    assert_exited(&process);
    bridge.set_source_root(&b);
    let stopped = bridge.refresh_with(|_, _, _, _| panic!("资源停止不能因切换来源而重启"));
    assert_eq!(stopped.status, "unavailable");
    assert!(stopped.buckets.is_empty() && stopped.collected_at.is_none());
    bridge.retry_manually();
    let (fresh, process) = read_fixture(&mut bridge, &b, "b-retried", 22, false);
    assert_snapshot(&fresh, 22.0, "2");
    let cancel = bridge.cancel_handle();
    cancel.store(true, Ordering::Release);
    let cancelled = bridge.refresh_with(|_, _, _, _| panic!("退出后的桥接不能重启"));
    assert_ne!(cancelled.status, "connected");
    assert_exited(&process);
    bridge.clear_cached_data();
    assert!(Arc::ptr_eq(&cancel, &bridge.cancel_handle()));
    assert!(bridge.cancel.load(Ordering::Acquire));
}
