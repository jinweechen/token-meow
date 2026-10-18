//! Read-only native bridge audit. Does not issue inference, login or task-control calls.
#[allow(dead_code)]
#[path = "../src/account_bridge.rs"]
mod account_bridge;

use account_bridge::AccountBridge;
use serde_json::{json, Value};
use std::{collections::HashMap, path::PathBuf, sync::atomic::Ordering, time::Instant};

fn main() {
    if let Err(error) = audit() {
        eprintln!("account bridge audit failed: {error}");
        std::process::exit(1);
    }
}

fn audit() -> Result<(), String> {
    let args: HashMap<_, _> = std::env::args()
        .skip(1)
        .filter_map(|arg| {
            arg.split_once('=')
                .map(|(key, value)| (key.to_owned(), value.to_owned()))
        })
        .collect();
    let source = PathBuf::from(
        args.get("--source")
            .ok_or("--source=<absolute Codex root> required")?,
    );
    if !source.is_absolute() || !source.is_dir() {
        return Err("source must be an existing absolute directory".into());
    }
    let output = args.get("--out").map(PathBuf::from);
    if let Some(output) = &output {
        let output_root = std::env::current_dir()
            .map_err(|error| error.to_string())?
            .join("output")
            .canonicalize()
            .map_err(|_| "run from the workspace with an existing output directory")?;
        let parent = output
            .parent()
            .ok_or("output must name a file")?
            .canonicalize()
            .map_err(|_| "output parent must already exist")?;
        let source_root = source.canonicalize().map_err(|error| error.to_string())?;
        let absolute_output = parent.join(output.file_name().ok_or("output must name a file")?);
        if !parent.starts_with(&output_root)
            || output.is_symlink()
            || absolute_output.starts_with(source_root)
        {
            return Err(
                "audit evidence must remain in workspace output, outside the source directory"
                    .into(),
            );
        }
    }
    let started = Instant::now();
    let mut bridge = AccountBridge::with_source_root(&source);
    let cancel = bridge.cancel_handle();
    // An unusually large directory must not hold the acceptance process indefinitely.
    let audit_finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let timer_finished = audit_finished.clone();
    let timer_cancel = cancel.clone();
    std::thread::spawn(move || {
        for _ in 0..600 {
            if timer_finished.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        timer_cancel.store(true, Ordering::Relaxed);
    });
    let snapshot = bridge.refresh();
    let account_elapsed_ms = started.elapsed().as_millis();
    let directory = match bridge.list_sessions() {
        Ok(sessions) => json!({
            "status":"complete", "count":sessions.len(),
            "archived":sessions.iter().filter(|s|s["archived"] == true).count(),
            "withVerifiedParent":sessions.iter().filter(|s|s["parentThreadId"].is_string()).count(),
            "withTitle":sessions.iter().filter(|s|s["title"].is_string()).count(),
            "withDirectory":sessions.iter().filter(|s|s["cwd"].is_string()).count(),
            "contentFieldsPresent":sessions.iter().any(|s|["preview","turns","messages","body","status"].iter().any(|key|s.get(key).is_some()))
        }),
        Err(error) => json!({"status":"unavailable","error":error}),
    };
    bridge.shutdown();
    audit_finished.store(true, Ordering::Relaxed);
    let windows: Vec<Value> = snapshot
        .buckets
        .iter()
        .map(|bucket| {
            json!({
                "id":bucket.id,"primary":bucket.primary,"secondary":bucket.secondary
            })
        })
        .collect();
    let result = json!({
        "observedAt":chrono::Utc::now().to_rfc3339(),
        "source":"src-tauri/src/account_bridge.rs",
        "sourceSha256":source_digest(),
        "scope":"live owned native stdio bridge; read-only account and paginated metadata; no model inference",
        "accountElapsedMs":account_elapsed_ms,"elapsedMs":started.elapsed().as_millis(),
        "account":{
            "status":snapshot.status,"collectedAt":snapshot.collected_at,
            "lastAttemptAt":snapshot.last_attempt_at,"error":snapshot.error,
            "authMode":snapshot.auth_mode,"hasVerifiedAccountScope":snapshot.account_scope.is_some(),
            "buckets":windows,
            "dailyUsageBuckets":snapshot.usage.as_ref().and_then(|v|v["dailyUsageBuckets"].as_array()).map(Vec::len)
        },
        "directory":directory,
        "cancelledByAuditDeadline":cancel.load(Ordering::Relaxed),
        "limitations":["Account service and authentication availability reflect this source at observation time.","Directory metadata does not establish runtime activity or historical account attribution.","Native window, notification and installer acceptance are separate."]
    });
    let text = serde_json::to_string_pretty(&result).map_err(|error| error.to_string())?;
    if let Some(output) = output {
        std::fs::write(output, &text).map_err(|error| error.to_string())?;
    }
    println!("{text}");
    Ok(())
}

fn source_digest() -> String {
    use sha2::{Digest, Sha256};
    format!(
        "{:x}",
        Sha256::digest(include_bytes!("../src/account_bridge.rs"))
    )
}
