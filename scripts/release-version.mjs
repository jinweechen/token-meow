import { readFileSync, writeFileSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const file = (name) => resolve(root, name);
export function checkVersions(directory = root) {
  const read = (name) => readFileSync(resolve(directory, name), "utf8");
  const version = JSON.parse(read("package.json")).version;
  if (!/^\d+\.\d+\.\d+$/.test(version)) throw new Error("Release version must contain three numeric components.");
  const cargo = /^version = "([^"]+)"/m.exec(read("src-tauri/Cargo.toml"))?.[1];
  const locked = /\[\[package\]\]\r?\nname = "token-meow"\r?\nversion = "([^"]+)"/.exec(read("src-tauri/Cargo.lock"))?.[1];
  const tauri = JSON.parse(read("src-tauri/tauri.conf.json")).version;
  if ([cargo, locked, tauri].some((v) => v !== version)) throw new Error(`Version mismatch: package=${version}, Cargo=${cargo}, lock=${locked}, Tauri=${tauri}`);
  return version;
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  if (args.length) {
    if (args.length !== 2 || args[0] !== "--set" || !/^\d+\.\d+\.\d+$/.test(args[1])) throw new Error("Usage: node scripts/release-version.mjs [--set 0.1.1]");
    const version = args[1];
    for (const name of ["package.json", "src-tauri/tauri.conf.json"]) {
      const content = JSON.parse(readFileSync(file(name), "utf8")); content.version = version;
      writeFileSync(file(name), JSON.stringify(content, null, 2) + "\n");
    }
    const cargo = file("src-tauri/Cargo.toml");
    writeFileSync(cargo, readFileSync(cargo, "utf8").replace(/^version = "[^"]+"/m, `version = "${version}"`));
    const lock = file("src-tauri/Cargo.lock");
    writeFileSync(lock, readFileSync(lock, "utf8").replace(/(\[\[package\]\]\r?\nname = "token-meow"\r?\nversion = ")[^"]+("\r?\n)/, `$1${version}$2`));
  }
  console.log(`Release versions consistent: ${checkVersions()}`);
}
