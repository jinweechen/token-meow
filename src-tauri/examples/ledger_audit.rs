//! Offline product-core replay, without a desktop window or account subprocess.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, Metadata, OpenOptions},
    io::{ErrorKind, Write},
    path::{Component, Path, PathBuf},
    time::Instant,
};
use token_meow_lib::Core;

fn main() {
    if let Err(error) = audit() {
        eprintln!("ledger audit failed: {error}");
        std::process::exit(1);
    }
}
fn audit() -> Result<(), String> {
    let raw_args: Vec<_> = std::env::args().skip(1).collect();
    if raw_args
        .iter()
        .any(|arg| arg == "--prepare-only" || arg.starts_with("--prepare-only="))
    {
        return prepare_only(&raw_args);
    }
    let args: HashMap<_, _> = raw_args
        .iter()
        .filter_map(|arg| {
            arg.split_once('=')
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
        })
        .collect();
    let source = PathBuf::from(
        args.get("--source")
            .ok_or("--source=<absolute Codex fixture root> required")?,
    );
    let data = PathBuf::from(
        args.get("--data")
            .ok_or("--data=<absolute isolated app data directory> required")?,
    );
    if !source.is_absolute() || !data.is_absolute() {
        return Err("source/data paths must be absolute".into());
    }
    if let Some(path) = args.get("--out") {
        let output = PathBuf::from(path);
        let output_root = std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join("output")
            .canonicalize()
            .map_err(|_| "run from the workspace with an existing output directory")?;
        let parent = output
            .parent()
            .ok_or("output must name a file")?
            .canonicalize()
            .map_err(|_| "output parent must already exist")?;
        let source_root = source.canonicalize().map_err(|e| e.to_string())?;
        let data_root = data.canonicalize().unwrap_or_else(|_| data.clone());
        let absolute_output = parent.join(output.file_name().ok_or("output must name a file")?);
        if !parent.starts_with(output_root)
            || output.is_symlink()
            || absolute_output.starts_with(source_root)
            || absolute_output.starts_with(data_root)
        {
            return Err(
                "audit evidence must remain in workspace output, outside source/data directories"
                    .into(),
            );
        }
    }
    let maximum = args
        .get("--scans")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(200)
        .clamp(1, 10000);
    let mut ledger = Core::open(&data, &source)?;
    let started = Instant::now();
    let mut scans = 0;
    let mut last = Value::Null;
    let mut stable = 0;
    let mut last_lines = None;
    for _ in 0..maximum {
        let report = ledger.scan()?;
        scans += 1;
        let snapshot = ledger.snapshot(json!({}))?;
        let health = &snapshot["health"];
        let progress = json!([
            health["files"],
            snapshot["overview"]["observedResponseCount"],
            health["pendingFiles"]
        ]);
        if report.lines == 0 && last_lines.as_ref() == Some(&progress) {
            stable += 1
        } else {
            stable = 0
        }
        last_lines = Some(progress);
        last = json!({"overview":snapshot["overview"],"health":{"files":health["files"],"pendingFiles":health["pendingFiles"],"lastSuccessfulScan":health["lastSuccessfulScan"],"lastSourceEvent":health["lastSourceEvent"],"coverageFrom":health["coverageFrom"],"coverageTo":health["coverageTo"],"conflicts":health["conflicts"],"excludedResponses":health["excludedResponses"]},"counts":{"sessions":snapshot["sessions"].as_array().map(Vec::len),"turns":snapshot["turns"].as_array().map(Vec::len),"tools":snapshot["tools"].as_array().map(Vec::len),"inbox":snapshot["inbox"].as_array().map(Vec::len)},"activity":snapshot["activity"]});
        // Multiple no-progress rounds are evidence of a stable cursor, not proof that every
        // input is supported. Pending/coverage/issues remain explicit in the result.
        if stable >= 3 && health["pendingFiles"].as_u64() == Some(0) {
            break;
        }
    }
    ledger.shutdown()?;
    let output = json!({"observedAt":chrono::Utc::now().to_rfc3339(),"scans":scans,"elapsedMs":started.elapsed().as_millis(),"result":last,"scope":"offline production-core replay; no desktop/account/inference qualification"});
    let text = serde_json::to_string_pretty(&output).map_err(|e| e.to_string())?;
    if let Some(path) = args.get("--out") {
        std::fs::write(path, &text).map_err(|e| e.to_string())?;
    }
    println!("{text}");
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn is_link(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        return metadata.file_type().is_symlink()
            || metadata.file_attributes()
                & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
                != 0;
    }
    #[cfg(not(windows))]
    metadata.file_type().is_symlink()
}

fn reject_links(path: &Path, missing_leaf: bool) -> Result<(), String> {
    if !path.is_absolute()
        || path.as_os_str().len() > 4096
        || path.components().count() > 64
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(
            "prepare-only paths must be bounded absolute paths without dot components".into(),
        );
    }
    for ancestor in path.ancestors().filter(|part| !part.as_os_str().is_empty()) {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if is_link(&metadata) => {
                return Err("prepare-only rejects symlink/reparse-point paths".into())
            }
            Ok(_) => {}
            Err(error)
                if missing_leaf && ancestor == path && error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(format!("prepare-only path metadata failed: {error}")),
        }
    }
    Ok(())
}

fn within(path: &Path, root: &Path) -> bool {
    let path: Vec<_> = path
        .components()
        .map(|part| part.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    let root: Vec<_> = root
        .components()
        .map(|part| part.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    path.starts_with(&root)
}

fn new_performance_path(path: &Path, performance: &Path) -> Result<PathBuf, String> {
    reject_links(path, true)?;
    match fs::symlink_metadata(path) {
        Ok(_) => return Err("prepare-only data and receipt must not already exist".into()),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("prepare-only path must have a UTF-8 leaf name")?;
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    if name.is_empty()
        || name.len() > 120
        || name.ends_with('.')
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
        || ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
    {
        return Err("prepare-only leaf names must be safe ASCII file names".into());
    }
    let parent = path
        .parent()
        .ok_or("prepare-only path must have a parent")?
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if !within(&parent, performance) {
        return Err("prepare-only data/receipt must stay in workspace output/performance".into());
    }
    Ok(parent.join(name))
}

fn prepare_only(raw_args: &[String]) -> Result<(), String> {
    let mut args = HashMap::new();
    for arg in raw_args {
        let (key, value) = arg
            .split_once('=')
            .ok_or("prepare-only accepts only explicit --key=value arguments")?;
        if !["--prepare-only", "--source", "--data", "--out"].contains(&key)
            || args.insert(key, value).is_some()
        {
            return Err("prepare-only rejects unknown or duplicate arguments".into());
        }
    }
    if args.get("--prepare-only") != Some(&"true") {
        return Err("prepare-only must be exactly --prepare-only=true".into());
    }
    let path = |key| {
        args.get(key)
            .map(PathBuf::from)
            .ok_or_else(|| format!("prepare-only requires {key}=<absolute path>"))
    };
    let source = path("--source")?;
    let data = path("--data")?;
    let output = path("--out")?;
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("missing workspace root")?;
    let performance = workspace.join("output/performance");
    reject_links(&performance, false)?;
    let performance = performance
        .canonicalize()
        .map_err(|error| error.to_string())?;
    reject_links(&source, false)?;
    if !source.is_dir() {
        return Err("prepare-only source must be an existing directory".into());
    }
    let source = source.canonicalize().map_err(|error| error.to_string())?;
    let data = new_performance_path(&data, &performance)?;
    let output = new_performance_path(&output, &performance)?;
    if output.extension().and_then(|value| value.to_str()) != Some("json")
        || within(&data, &source)
        || within(&source, &data)
        || within(&output, &source)
        || within(&output, &data)
        || within(&data, &output)
    {
        return Err("prepare-only source/data/JSON receipt must not overlap".into());
    }
    // Reserve evidence first: an existing receipt cannot cause preparation side effects.
    let mut receipt = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)
        .map_err(|error| format!("create-new prepare receipt failed: {error}"))?;
    fs::create_dir(&data)
        .map_err(|error| format!("create-new prepare data directory failed: {error}"))?;
    reject_links(&data, false)?;
    if fs::read_dir(&data)
        .map_err(|error| error.to_string())?
        .next()
        .is_some()
    {
        return Err("prepare-only new data directory is not empty".into());
    }
    let started = Instant::now();
    let mut ledger = Core::open(&data, &source)?;
    let settings = ledger.action(
        "update_settings",
        &json!({"settings":{"reducedMotion":true}}),
    )?;
    ledger.shutdown()?;
    drop(ledger);
    let value = json!({
        "schemaVersion":1,"kind":"production-core-prepare-only","observedAt":chrono::Utc::now().to_rfc3339(),"completed":true,
        "dataDirectory":data,"sourceDirectory":source,"elapsedMs":started.elapsed().as_millis(),
        "coreSha256":digest(include_bytes!("../src/core.rs")),
        "compiledCoreSources":{
            "src/core.rs":digest(include_bytes!("../src/core.rs")),
            "src/core/adapter.rs":digest(include_bytes!("../src/core/adapter.rs")),
            "src/core/agent_kind.rs":digest(include_bytes!("../src/core/agent_kind.rs")),
            "src/core/pet.rs":digest(include_bytes!("../src/core/pet.rs")),
            "src/core/reader.rs":digest(include_bytes!("../src/core/reader.rs")),
            "src/core/queries.rs":digest(include_bytes!("../src/core/queries.rs")),
            "migrations/001_initial.sql":digest(include_bytes!("../migrations/001_initial.sql")),
            "migrations/002_multi_source.sql":digest(include_bytes!("../migrations/002_multi_source.sql")),
            "migrations/003_fact_references.sql":digest(include_bytes!("../migrations/003_fact_references.sql")),
            "migrations/004_usage_lookup_index.sql":digest(include_bytes!("../migrations/004_usage_lookup_index.sql")),
            "migrations/005_pet_read_models.sql":digest(include_bytes!("../migrations/005_pet_read_models.sql")),
            "migrations/006_retention_time_indexes.sql":digest(include_bytes!("../migrations/006_retention_time_indexes.sql"))
        },
        "productionCalls":["Core::open","Core::action(update_settings,{settings:{reducedMotion:true}})","Core::shutdown"],
        "settingsAction":{"name":"update_settings","payload":{"settings":{"reducedMotion":true}}},"settingsResult":settings,
        "scanCalls":0,"snapshotCalls":0,"bridgeCalls":0,"modelRequests":0,
        "safety":{"dataDirectoryCreatedNew":true,"initialDataDirectoryEmpty":true,"receiptCreateNew":true,"symlinkAndReparseChecks":true,"sourceDataOverlap":false},
        "scope":"Production schema and static setting preparation only; source usage was not scanned or snapshotted. Not a first-import, desktop, or process-resource qualification. Path checks assume no hostile concurrent directory replacement."
    });
    let text = serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?;
    receipt
        .write_all(text.as_bytes())
        .and_then(|_| receipt.sync_all())
        .map_err(|error| error.to_string())?;
    println!("{text}");
    Ok(())
}
