use base64::{engine::general_purpose::STANDARD, Engine};
use minisign_verify::{PublicKey, Signature};
use serde::{Deserialize, Serialize};
use std::io::Read;

pub const REPOSITORY: &str = "https://github.com/jinweechen/token-meow";
pub const ENDPOINT: &str = "https://github.com/jinweechen/token-meow/releases/latest/download/latest.json";
pub const MAX_INSTALLER_BYTES: u64 = 512 * 1024 * 1024;
pub const PUBLIC_KEY: &str = include_str!("../../release/update-public-key.pub");

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Release {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    pub platforms: std::collections::BTreeMap<String, Artifact>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Artifact { pub url: String, pub signature: String }

pub fn validate_release(release: &Release, current: &str) -> Result<bool, String> {
    let next = semver::Version::parse(&release.version).map_err(|_| "更新版本号无效")?;
    let installed = semver::Version::parse(current).map_err(|_| "当前版本号无效")?;
    if !next.pre.is_empty() || !next.build.is_empty() { return Err("稳定渠道不接受预发布版本".into()); }
    let artifact = release.platforms.get("windows-x86_64").ok_or("更新缺少 Windows x64 安装包")?;
    let expected = format!("{REPOSITORY}/releases/download/v{next}/TokenMeow_{next}_x64-custom-setup.exe");
    if artifact.url != expected || artifact.signature.len() > 4096 || release.notes.len() > 16384 {
        return Err("更新包来源、名称或发布信息无效".into());
    }
    Ok(next > installed)
}

pub fn verify_reader(mut reader: impl Read, signature: &str, version: &str, key: &str) -> Result<(), String> {
    let decode = |text: &str| -> Result<String, String> {
        String::from_utf8(STANDARD.decode(text.trim()).map_err(|_| "更新签名编码无效")?).map_err(|_| "更新签名文本无效".into())
    };
    let public = PublicKey::decode(&decode(key)?).map_err(|_| "更新公钥无效")?;
    let signature = Signature::decode(&decode(signature)?).map_err(|_| "更新签名无效")?;
    let mut verifier = public.verify_stream(&signature).map_err(|_| "更新签名不受信任")?;
    let mut buf = [0u8; 65536];
    let mut count = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|_| "无法读取更新文件")?;
        if n == 0 { break; }
        count += n as u64;
        if count > MAX_INSTALLER_BYTES { return Err("更新安装包超过大小限制".into()); }
        verifier.update(&buf[..n]);
    }
    verifier.finalize().map_err(|_| "更新包签名校验失败，未启动安装")?;
    // The comment becomes trustworthy only after the global signature verifies.
    let versions: Vec<_> = signature.trusted_comment().split('\t').filter_map(|s| s.strip_prefix("version:")).collect();
    if versions != [version] { return Err("安装包签名绑定的版本与发布版本不一致".into()); }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn release(v: &str) -> Release {
        Release { version:v.into(), notes:String::new(), platforms:[("windows-x86_64".into(), Artifact { url:format!("{REPOSITORY}/releases/download/v{v}/TokenMeow_{v}_x64-custom-setup.exe"), signature:"placeholder".into() })].into() }
    }
    #[test] fn stable_upgrade_only() {
        assert!(validate_release(&release("0.2.0"), "0.1.1").unwrap());
        assert!(!validate_release(&release("0.1.1"), "0.1.1").unwrap());
        assert!(!validate_release(&release("0.1.0"), "0.1.1").unwrap());
        assert!(validate_release(&release("0.2.0-beta.1"), "0.1.1").is_err());
    }
    #[test] fn untrusted_url_missing_platform_and_oversized_metadata_fail_closed() {
        let mut r=release("0.2.0"); r.platforms.get_mut("windows-x86_64").unwrap().url="https://example.com/evil.exe".into();
        assert!(validate_release(&r,"0.1.1").is_err());
        r=release("0.2.0");r.platforms.clear();assert!(validate_release(&r,"0.1.1").is_err());
        r=release("0.2.0");r.notes="x".repeat(16385);assert!(validate_release(&r,"0.1.1").is_err());
    }
    #[test] fn invalid_signatures_cannot_launch() { assert!(verify_reader(&b"MZ"[..],"bad","0.1.1",PUBLIC_KEY).is_err()); }
}
