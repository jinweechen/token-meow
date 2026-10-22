import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isDesktop } from "../../shared/bridge";
import { Modal } from "../../shared/UI";
import { version } from "../../../package.json";

type Release = { currentVersion: string; available: boolean; version: string; notes: string };
type Progress = { received: number; total: number | null };

export function AppUpdatePanel() {
  const [release, setRelease] = useState<Release | null>(null);
  const [busy, setBusy] = useState<"check" | "download" | "launch" | null>(null);
  const [error, setError] = useState("");
  const [progress, setProgress] = useState<Progress | null>(null);
  const [ready, setReady] = useState(false);
  const [confirm, setConfirm] = useState(false);
  useEffect(() => {
    if (!isDesktop) return;
    let active = true;
    const unlisten = listen<Progress>("app-update-progress", (event) => { if (active) setProgress(event.payload); });
    return () => { active = false; void unlisten.then((dispose) => dispose()).catch(() => {}); };
  }, []);
  async function check() {
    setBusy("check"); setError(""); setRelease(null); setReady(false); setProgress(null);
    try { setRelease(await invoke<Release>("check_app_update")); }
    catch (cause) { setError(String(cause)); }
    finally { setBusy(null); }
  }
  async function download() {
    setBusy("download"); setError(""); setReady(false); setProgress(null);
    try { await invoke("download_app_update"); setReady(true); }
    catch (cause) { setError(String(cause)); }
    finally { setBusy(null); }
  }
  async function install() {
    setBusy("launch"); setError("");
    try { await invoke("launch_app_update"); setConfirm(false); }
    catch (cause) { setError(String(cause)); setBusy(null); setConfirm(false); }
  }
  const downloaded = progress ? `${(progress.received / 1048576).toFixed(1)} MB${progress.total ? ` / ${(progress.total / 1048576).toFixed(1)} MB` : ""}` : "正在连接下载服务器…";
  return <section className="card" aria-label="应用更新">
    <div className="section-heading"><h2>应用更新</h2><span>当前版本 {version}</span></div>
    <p>从 GitHub Releases 获取稳定版更新。下载安装包后校验发布签名，安装时正常退出应用并保留本地账本。</p>
    {!isDesktop && <p>请在 Windows 应用内检查和安装更新。</p>}
    <div className="modal-actions">
      <button className="button secondary" disabled={!!busy || !isDesktop} onClick={() => void check()}>{busy === "check" ? "正在检查…" : "检查更新"}</button>
      {release?.available && !ready && <button className="button primary" disabled={!!busy} onClick={() => void download()}>{busy === "download" ? "正在下载并校验…" : `下载 ${release.version}`}</button>}
      {ready && <button className="button primary" disabled={!!busy} onClick={() => setConfirm(true)}>退出并打开更新安装器</button>}
    </div>
    <div role="status" aria-live="polite">
      {release && <p>{release.available ? `发现新版本 ${release.version}` : "当前已是最新稳定版"}</p>}
      {release?.available && release.notes && <p style={{ whiteSpace: "pre-wrap" }}>{release.notes}</p>}
      {busy === "download" && <p>{downloaded}</p>}
      {ready && <p>安装包签名和版本校验通过，可以开始更新。</p>}
    </div>
    {error && <p role="alert">{error}</p>}
    {confirm && <Modal title="安装更新" onClose={() => { if (!busy) setConfirm(false); }}>
      <p>将打开已校验的安装器，然后正常退出 Token 喵。请在安装器中确认更新；本地账本与偏好设置会保留。</p>
      <div className="modal-actions"><button className="button secondary" disabled={!!busy} onClick={() => setConfirm(false)}>取消</button><button className="button primary" disabled={!!busy} onClick={() => void install()}>{busy === "launch" ? "正在打开…" : "退出并更新"}</button></div>
    </Modal>}
  </section>;
}
