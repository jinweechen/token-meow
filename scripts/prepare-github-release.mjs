import { readFileSync, writeFileSync, existsSync, lstatSync, openSync, closeSync, readSync, realpathSync } from "node:fs";
import { resolve, dirname, basename } from "node:path";
import { fileURLToPath } from "node:url";
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { checkVersions } from "./release-version.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const args = process.argv.slice(2);
if (args.length !== 2 && !(args.length === 4 && args[2] === "--application-build-receipt")) throw new Error("Usage: node scripts/prepare-github-release.mjs <release-directory> <private-key-path> [--application-build-receipt <original-full-build-receipt>]");
const version = checkVersions();
const release = realpathSync(resolve(args[0]));
const key = realpathSync(resolve(args[1]));
const prefix = release.toLowerCase() + "\\";
if (key.toLowerCase().startsWith(prefix)) throw new Error("Signing keys must never be inside the distributable release directory.");
const receipt = JSON.parse(readFileSync(resolve(release, "receipt.json"), "utf8"));
const name = `TokenMeow_${version}_x64-custom-setup.exe`;
const setup = resolve(release, name);
const hash = (path) => {
  const digest = createHash("sha256"), fd = openSync(path, "r"), buffer = Buffer.alloc(1024 * 1024);
  try { for (;;) { const n = readSync(fd, buffer, 0, buffer.length, null); if (!n) break; digest.update(buffer.subarray(0, n)); } }
  finally { closeSync(fd); }
  return digest.digest("hex").toUpperCase();
};
let upstreamPath;
if (receipt.productVersion !== version) throw new Error("The production receipt version differs.");
if (!receipt.applicationRecompiledByThisRun || receipt.mode !== "Build") {
  if (args.length !== 4 || receipt.mode !== "ReuseEngine") throw new Error("A reused engine requires an explicit original full application build receipt.");
  upstreamPath = realpathSync(resolve(args[3]));
  const upstream = JSON.parse(readFileSync(upstreamPath, "utf8"));
  if (upstream.mode !== "Build" || !upstream.applicationRecompiledByThisRun || upstream.productVersion !== version || upstream.actualEnginePayload.sha256 !== receipt.actualEnginePayload.sha256) throw new Error("The upstream full build does not bind the exact reused application/version.");
}
if (basename(receipt.executable.path) !== name || hash(setup) !== receipt.executable.sha256) throw new Error("Installer differs from its build receipt.");
for (const row of receipt.heldInputs) {
  const path = resolve(row.path);
  if (path.startsWith(root) && !path.startsWith(resolve(root, "output") + "\\") && !path.includes("\\node_modules\\") && hash(path) !== row.sha256) throw new Error(`Build source has changed: ${path}`);
}
if (existsSync(resolve(release, "latest.json"))) throw new Error("Preserve the previous release preparation; latest.json already exists.");
if (!lstatSync(setup).isFile() || !lstatSync(key).isFile()) throw new Error("Ordinary files are required.");
// Verify using the same compiled production verifier, not just the signer exit code.
const verifier = process.env.TOKEN_MEOW_SIGNATURE_VERIFIER;
if (!verifier || !existsSync(verifier)) throw new Error("Set TOKEN_MEOW_SIGNATURE_VERIFIER to the compiled update_verify example; no manifest created.");
if (!existsSync(`${setup}.sig`)) {
  const cli = resolve(root, "node_modules/@tauri-apps/cli/tauri.js");
  const sign = spawnSync(process.execPath, [cli, "signer", "sign", "--private-key-path", key, "--app-version", version, setup], { cwd: root, encoding: "utf8", windowsHide: true });
  if (sign.status !== 0) throw new Error("Update signing failed. Verify the private key and TAURI_SIGNING_PRIVATE_KEY_PASSWORD; no update manifest was created.");
}
const signature = readFileSync(`${setup}.sig`, "utf8").trim();
const verify = spawnSync(verifier, [setup, `${setup}.sig`, version], { cwd: root, encoding: "utf8", windowsHide: true });
if (verify.status !== 0) throw new Error("Release signature does not match the application's pinned update key/version. No manifest created.");
const manifest = {
  version, notes: "安装、升级与卸载流程更新。详细变更和已知限制请见本次发布说明。", pub_date: new Date().toISOString(),
  platforms: { "windows-x86_64": { signature, url: `https://github.com/jinweechen/token-meow/releases/download/v${version}/${name}` } },
};
writeFileSync(resolve(release, "latest.json"), JSON.stringify(manifest, null, 2) + "\n", { flag: "wx" });
const files = [name, "receipt.json", `${name}.sig`, "latest.json"];
if (upstreamPath) {
  writeFileSync(resolve(release, "application-build-receipt.json"), readFileSync(upstreamPath), { flag: "wx" });
  files.push("application-build-receipt.json");
}
writeFileSync(resolve(release, "SHA256SUMS.txt"), files.map((f) => `${hash(resolve(release, f))}  ${f}`).join("\n") + "\n");
console.log(JSON.stringify({ status: "SIGNED_UPDATE_PREPARED_NOT_PUBLISHED", version, directory: release, assets: [...files, "SHA256SUMS.txt"], published: false }, null, 2));
