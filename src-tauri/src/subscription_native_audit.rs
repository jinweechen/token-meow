//! Debug-feature-only admission for an owned, credential-free Native attempt.
//! This admission permits collecting Native evidence; it is not that evidence.
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

const MARKER: &str = "subscription-native-audit.marker.json";
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Marker {
    schema_version: u32,
    kind: String,
    attempt_id: String,
    allow_native_plan_delivery: bool,
    source_root: PathBuf,
    data_directory: PathBuf,
}
fn ordinary(path: &Path, directory: bool) -> bool {
    let Ok(m) = fs::symlink_metadata(path) else {
        return false;
    };
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if m.file_attributes() & 0x400 != 0 {
            return false;
        }
    }
    !m.file_type().is_symlink() && if directory { m.is_dir() } else { m.is_file() }
}
fn canonical(path: &Path, directory: bool) -> Option<PathBuf> {
    if !path.is_absolute() || !ordinary(path, directory) {
        return None;
    }
    let mut cursor = path.parent();
    while let Some(p) = cursor {
        if !ordinary(p, true) {
            return None;
        }
        cursor = p.parent();
    }
    fs::canonicalize(path).ok()
}
fn equal(a: &Path, b: &Path) -> bool {
    #[cfg(windows)]
    {
        a.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&b.as_os_str().to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        a == b
    }
}
fn env_path(env: &BTreeMap<String, OsString>, key: &str, expected: &Path) -> bool {
    env.get(key)
        .and_then(|p| canonical(Path::new(p), true))
        .is_some_and(|p| equal(&p, expected))
}
fn inspect(data_dir: &Path, source_root: &Path, env: &BTreeMap<String, OsString>) -> bool {
    let Some(root) = env
        .get("TOKEN_MEOW_SUBSCRIPTION_AUDIT_ROOT")
        .and_then(|p| canonical(Path::new(p), true))
    else {
        return false;
    };
    let Some(data) = canonical(data_dir, true) else {
        return false;
    };
    let Some(source) = canonical(source_root, true) else {
        return false;
    };
    if !equal(&data, &root.join("data")) || !equal(&source, &root.join("source")) {
        return false;
    }
    let marker_path = root.join(MARKER);
    if canonical(&marker_path, false).is_none() {
        return false;
    }
    let Ok(file) = fs::File::open(marker_path) else {
        return false;
    };
    let mut bytes = Vec::new();
    if file.take(4097).read_to_end(&mut bytes).is_err() || bytes.len() > 4096 {
        return false;
    }
    let Ok(marker) = serde_json::from_slice::<Marker>(&bytes) else {
        return false;
    };
    if marker.schema_version != 1
        || marker.kind != "subscription-native-audit"
        || !marker.allow_native_plan_delivery
        || marker.attempt_id.len() != 32
        || !marker.attempt_id.bytes().all(|c| c.is_ascii_hexdigit())
        || canonical(&marker.data_directory, true).is_none_or(|p| !equal(&p, &data))
        || canonical(&marker.source_root, true).is_none_or(|p| !equal(&p, &source))
    {
        return false;
    }
    // Inspect names against a complete allowlist; inherited auth, proxy, task,
    // arbitrary PATH and unrecognized provider inputs are never accepted.
    const ALLOWED: &[&str] = &[
        "OS",
        "NUMBER_OF_PROCESSORS",
        "PROCESSOR_ARCHITECTURE",
        "PROCESSOR_IDENTIFIER",
        "PROCESSOR_LEVEL",
        "PROCESSOR_REVISION",
        "PROGRAMFILES",
        "PROGRAMFILES(X86)",
        "PROGRAMW6432",
        "SYSTEMROOT",
        "WINDIR",
        "SYSTEMDRIVE",
        "COMSPEC",
        "PATH",
        "LOCALAPPDATA",
        "APPDATA",
        "USERPROFILE",
        "TEMP",
        "TMP",
        "USERNAME",
        "USERDOMAIN",
        "CODEX_HOME",
        "TOKEN_MEOW_DATA_DIR",
        "WEBVIEW2_USER_DATA_FOLDER",
        "TOKEN_MEOW_DEBUG_CDP_PORT",
        "TOKEN_MEOW_SUBSCRIPTION_AUDIT_ROOT",
    ];
    if env.keys().any(|key| !ALLOWED.contains(&key.as_str())) {
        return false;
    }
    let profile = root.join("profile");
    let local = profile.join("AppData/Local");
    let bin = root.join("bin");
    for (key, path) in [
        ("USERPROFILE", profile),
        ("LOCALAPPDATA", local.clone()),
        ("APPDATA", root.join("profile/AppData/Roaming")),
        ("TEMP", root.join("temp")),
        ("TMP", root.join("temp")),
        ("WEBVIEW2_USER_DATA_FOLDER", root.join("webview")),
        ("CODEX_HOME", source.clone()),
        ("TOKEN_MEOW_DATA_DIR", data.clone()),
    ] {
        if !env_path(env, key, &path) {
            return false;
        }
    }
    let Some(windows) = env
        .get("SYSTEMROOT")
        .and_then(|p| canonical(Path::new(p), true))
    else {
        return false;
    };
    if !env_path(env, "WINDIR", &windows) {
        return false;
    }
    let expected = [bin, windows.join("System32"), windows];
    let Some(path) = env.get("PATH") else {
        return false;
    };
    let paths = std::env::split_paths(path).collect::<Vec<_>>();
    if paths.len() != expected.len() {
        return false;
    }
    for (path, expected) in paths.iter().zip(&expected) {
        if canonical(path, true).is_none_or(|p| !equal(&p, expected))
            || path.join("codex.exe").try_exists().ok() != Some(false)
        {
            return false;
        }
    }
    if local
        .join("Programs/OpenAI/Codex/bin/codex.exe")
        .try_exists()
        .ok()
        != Some(false)
    {
        return false;
    }
    if ["auth.json", "config.toml", "credentials.json"]
        .iter()
        .any(|name| source.join(name).try_exists().ok() != Some(false))
    {
        return false;
    }
    if env.get("TOKEN_MEOW_DEBUG_CDP_PORT").is_some_and(|port| {
        port.to_str()
            .and_then(|p| p.parse::<u16>().ok())
            .is_none_or(|n| n < 1024)
    }) {
        return false;
    }
    true
}

pub(crate) fn enabled(data_dir: &Path, source_root: &Path) -> bool {
    // lib.rs compiles this module only for debug + subscription-native-audit.
    if std::env::var_os("TOKEN_MEOW_SUBSCRIPTION_AUDIT_ROOT").is_none() {
        return false;
    }
    let environment = std::env::vars_os()
        .map(|(key, value)| (key.to_string_lossy().to_ascii_uppercase(), value))
        .collect();
    inspect(data_dir, source_root, &environment)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, BTreeMap<String, OsString>) {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        for p in [
            "data",
            "source",
            "bin",
            "profile/AppData/Local",
            "profile/AppData/Roaming",
            "temp",
            "webview",
            "windows/System32",
        ] {
            fs::create_dir_all(root.join(p)).unwrap();
        }
        let mut env = BTreeMap::new();
        for (key, path) in [
            ("TOKEN_MEOW_SUBSCRIPTION_AUDIT_ROOT", root.clone()),
            ("TOKEN_MEOW_DATA_DIR", root.join("data")),
            ("CODEX_HOME", root.join("source")),
            ("USERPROFILE", root.join("profile")),
            ("LOCALAPPDATA", root.join("profile/AppData/Local")),
            ("APPDATA", root.join("profile/AppData/Roaming")),
            ("TEMP", root.join("temp")),
            ("TMP", root.join("temp")),
            ("WEBVIEW2_USER_DATA_FOLDER", root.join("webview")),
            ("SYSTEMROOT", root.join("windows")),
            ("WINDIR", root.join("windows")),
        ] {
            env.insert(key.into(), path.into_os_string());
        }
        env.insert(
            "PATH".into(),
            std::env::join_paths([
                root.join("bin"),
                root.join("windows/System32"),
                root.join("windows"),
            ])
            .unwrap(),
        );
        fs::write(root.join(MARKER),serde_json::to_vec(&serde_json::json!({"schemaVersion":1,"kind":"subscription-native-audit","attemptId":"1234567890abcdef1234567890abcdef","allowNativePlanDelivery":true,"sourceRoot":root.join("source"),"dataDirectory":root.join("data")})).unwrap()).unwrap();
        (dir, env)
    }
    fn valid(root: &Path, env: &BTreeMap<String, OsString>) -> bool {
        inspect(&root.join("data"), &root.join("source"), env)
    }
    #[test]
    fn dedicated_fixture_is_admitted_but_p1_or_cdp_alone_never_are() {
        let (dir, env) = fixture();
        assert!(valid(dir.path(), &env));
        let mut bad = env.clone();
        bad.remove("TOKEN_MEOW_SUBSCRIPTION_AUDIT_ROOT");
        bad.insert("TOKEN_MEOW_DEBUG_CDP_PORT".into(), "9441".into());
        bad.insert("TOKEN_MEOW_P1_DEVELOPMENT".into(), "1".into());
        assert!(!valid(dir.path(), &bad));
    }
    #[test]
    fn marker_is_required_bounded_and_explicit() {
        for mutation in 0..4 {
            let (dir, env) = fixture();
            let path = dir.path().join(MARKER);
            let mut v: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            match mutation {
                0 => {
                    fs::remove_file(&path).unwrap();
                }
                1 => {
                    v["allowNativePlanDelivery"] = false.into();
                    fs::write(&path, v.to_string()).unwrap();
                }
                2 => {
                    v["attemptId"] = "bad".into();
                    fs::write(&path, v.to_string()).unwrap();
                }
                _ => {
                    fs::write(&path, vec![b' '; 4097]).unwrap();
                }
            }
            assert!(!valid(dir.path(), &env));
        }
    }
    #[test]
    fn different_data_source_or_profile_is_rejected() {
        let (dir, env) = fixture();
        let other = dir.path().join("other");
        fs::create_dir(&other).unwrap();
        assert!(!inspect(&other, &dir.path().join("source"), &env));
        let mut bad = env.clone();
        bad.insert("USERPROFILE".into(), other.into_os_string());
        assert!(!valid(dir.path(), &bad));
    }
    #[test]
    fn arbitrary_inherited_credentials_task_or_proxy_names_are_rejected() {
        for name in [
            "OPENAI_API_KEY",
            "CODEX_ACCESS_TOKEN",
            "HTTP_PROXY",
            "CODEX_THREAD_ID",
            "TOKEN_MEOW_P1_DEVELOPMENT",
        ] {
            let (dir, mut env) = fixture();
            env.insert(name.into(), "synthetic-only".into());
            assert!(!valid(dir.path(), &env), "{name}");
        }
    }
    #[test]
    fn every_real_codex_candidate_and_source_auth_file_is_rejected() {
        for file in [
            "bin/codex.exe",
            "windows/System32/codex.exe",
            "windows/codex.exe",
            "profile/AppData/Local/Programs/OpenAI/Codex/bin/codex.exe",
            "source/auth.json",
            "source/config.toml",
        ] {
            let (dir, env) = fixture();
            let path = dir.path().join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"synthetic only").unwrap();
            assert!(!valid(dir.path(), &env), "{file}");
        }
    }
    #[test]
    fn extra_path_or_unknown_profile_directory_is_rejected() {
        let (dir, mut env) = fixture();
        env.insert(
            "PATH".into(),
            std::env::join_paths([
                dir.path().join("bin"),
                dir.path().join("windows/System32"),
                dir.path().join("windows"),
                dir.path().join("source"),
            ])
            .unwrap(),
        );
        assert!(!valid(dir.path(), &env));
    }
    #[cfg(windows)]
    #[test]
    #[ignore = "explicit symlink permission is required; not Native evidence"]
    fn reparse_data_or_marker_cannot_enable_audit() {
        use std::os::windows::fs::symlink_dir;
        let (dir, env) = fixture();
        let target = dir.path().join("elsewhere");
        fs::rename(dir.path().join("data"), &target).unwrap();
        symlink_dir(&target, dir.path().join("data")).unwrap();
        assert!(!valid(dir.path(), &env));
    }
}
