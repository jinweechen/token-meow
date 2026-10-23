import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isDesktop, listCompanionLaunchers } from "../../shared/bridge";
import { parseCompanionDrop, type CompanionDrop, type CompanionDropClient } from "../../shared/companionDrop";
import type { CompanionLauncher } from "../../shared/desktopCompanion";
import "./companionDrop.css";

export const companionDropClient: CompanionDropClient = {
  current: async () => parseCompanionDrop(await invoke("get_companion_drop")),
  launchers: listCompanionLaunchers,
  register: batchId => invoke("register_companion_drop", { options: { batchId } }),
  open: (batchId, launcherId) => invoke("open_companion_drop", { options: { batchId, launcherId } }),
  cancel: batchId => invoke("cancel_companion_drop", { options: { batchId } }),
  async subscribe(changed, failed) {
    const removeChanged = await listen("companion-drop-changed", changed);
    try {
      const removeFailed = await listen<{ message: string }>("companion-drop-error", event => failed(message(event.payload?.message)));
      return () => { removeChanged(); removeFailed(); };
    } catch (error) { removeChanged(); throw error; }
  },
};
const message = (error: unknown) => typeof error === "string" ? error : error instanceof Error ? error.message : "拖放操作未完成，请重新拖入。";

/** 文件路径由桌面端持有，界面只确认当前这批文件和登记应用。 */
export function CompanionDropDialog({ client = companionDropClient, enabled = isDesktop }: {
  client?: CompanionDropClient; enabled?: boolean;
}) {
  const [drop, setDrop] = useState<CompanionDrop | null>(null);
  const [apps, setApps] = useState<CompanionLauncher[]>([]);
  const [selected, setSelected] = useState("");
  const [remaining, setRemaining] = useState(0);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState("");
  const current = useRef<CompanionDrop | null>(null);
  const deadline = useRef(0);
  const version = useRef(0);
  const live = useRef(false);
  const working = useRef(false);
  const ended = useRef(new Set<string>());
  const nativeVisible = useRef(true);
  const nativePaused = useRef(false);
  const confirm = useRef<HTMLButtonElement>(null);
  const dialog = useRef<HTMLDialogElement>(null);
  current.current = drop;
  const retire = (batchId: string) => {
    ended.current.add(batchId);
    if (ended.current.size > 64) ended.current.delete(ended.current.values().next().value!);
  };
  useEffect(() => {
    if (!enabled) { setDrop(null); return; }
    live.current = true;
    let active = true;
    let remove: (() => void) | undefined;
    const available = () => !document.hidden && nativeVisible.current && !nativePaused.current;
    const refresh = async () => {
      const request = ++version.current;
      const requestedAt = performance.now();
      try {
        let incoming = parseCompanionDrop(await client.current());
        const candidateDeadline = incoming ? requestedAt + incoming.expiresInMs : 0;
        const launchers = incoming?.kind === "openFiles" ? await client.launchers() : [];
        if (!active || request !== version.current) return;
        if (incoming && ended.current.has(incoming.batchId)) incoming = null;
        if (incoming && (!available() || performance.now() >= candidateDeadline)) {
          retire(incoming.batchId);
          void client.cancel(incoming.batchId).catch(() => {});
          incoming = null;
        }
        if (incoming) {
          const nextDeadline = candidateDeadline;
          deadline.current = current.current?.batchId === incoming.batchId ? Math.min(deadline.current, nextDeadline) : nextDeadline;
          setRemaining(Math.ceil((deadline.current - performance.now()) / 1000));
          setSelected(value => launchers.some(app => app.id === value) ? value : launchers[0]?.id ?? "");
        }
        current.current = incoming;
        setDrop(incoming); setApps(launchers);
      } catch (error) {
        if (active && request === version.current) { current.current = null; setDrop(null); setNotice(message(error)); }
      }
    };
    const restrict = () => {
      if (!available()) {
        ++version.current;
        const value = current.current; current.current = null; setDrop(null);
        if (value) { retire(value.batchId); void client.cancel(value.batchId).catch(() => {}); }
      } else void refresh();
    };
    const hide = (event: Event) => {
      const value = (event as CustomEvent).detail;
      if (typeof value === "boolean") nativeVisible.current = value;
      if (typeof value?.visible === "boolean") nativeVisible.current = value.visible;
      if (typeof value?.paused === "boolean") nativePaused.current = value.paused;
      restrict();
    };
    document.addEventListener("visibilitychange", restrict);
    window.addEventListener("view-visibility", hide);
    window.addEventListener("desktop-runtime-state", hide);
    void client.subscribe(() => { if (active) void refresh(); }, error => {
      if (!active) return;
      ++version.current;
      if (current.current) retire(current.current.batchId);
      current.current = null; setDrop(null); setNotice(error);
    }).then(unsubscribe => {
      if (!active) unsubscribe(); else { remove = unsubscribe; void refresh(); }
    }).catch(error => { if (active) setNotice(message(error)); });
    return () => {
      active = false; live.current = false; ++version.current; remove?.();
      const abandoned = current.current; current.current = null;
      if (abandoned) { retire(abandoned.batchId); void client.cancel(abandoned.batchId).catch(() => {}); }
      document.removeEventListener("visibilitychange", restrict);
      window.removeEventListener("view-visibility", hide);
      window.removeEventListener("desktop-runtime-state", hide);
    };
  }, [client, enabled]);
  useEffect(() => {
    if (!drop) return;
    const modal = dialog.current;
    if (modal && !modal.open) modal.showModal();
    confirm.current?.focus({ preventScroll: true });
    const timer = window.setInterval(() => {
      const seconds = Math.max(0, Math.ceil((deadline.current - performance.now()) / 1000));
      setRemaining(seconds);
      if (!seconds && current.current?.batchId === drop.batchId) {
        retire(drop.batchId);
        ++version.current; current.current = null; setDrop(null); setNotice("拖放确认已到期，请重新拖入。");
        void client.cancel(drop.batchId).catch(() => {});
      }
    }, 250);
    return () => { window.clearInterval(timer); modal?.close(); };
  }, [drop?.batchId, client]);
  const cancel = async () => {
    if (!drop || working.current) return;
    retire(drop.batchId);
    ++version.current; current.current = null; setDrop(null);
    try { await client.cancel(drop.batchId); } catch (error) { if (live.current) setNotice(message(error)); }
  };
  const accept = async () => {
    const active = current.current;
    if (!active || working.current || deadline.current <= performance.now() || (active.kind === "openFiles" && !selected)) return;
    retire(active.batchId);
    working.current = true; setBusy(true); ++version.current;
    try {
      const result = active.kind === "registerExecutable" ? await client.register(active.batchId) : await client.open(active.batchId, selected);
      if (!result || typeof result !== "object" || !("accepted" in result) || result.accepted !== true) throw Error("应用未接受这次操作，请重新拖入。");
      if (live.current) setNotice(active.kind === "registerExecutable" ? "已加入快捷启动。" : "已交给所选应用打开。");
    } catch (error) { if (live.current) setNotice(message(error)); }
    finally {
      working.current = false;
      if (live.current) {
        setBusy(false);
        if (current.current?.batchId === active.batchId) { current.current = null; setDrop(null); }
      }
    }
  };
  return <>
    {notice && <div className="companion-drop-notice" role="status"><span>{notice}</span><button type="button" aria-label="关闭拖放结果" onClick={() => setNotice("")}>关闭</button></div>}
    {drop && <dialog ref={dialog} className="companion-drop-dialog" aria-labelledby="companion-drop-title"
        onCancel={event => { event.preventDefault(); void cancel(); }}>
        <h2 id="companion-drop-title">{drop.kind === "registerExecutable" ? "加入快捷启动" : "选择打开文件的应用"}</h2>
        <ul>{drop.fileNames.map((name, index) => <li key={index}>{name}</li>)}</ul>
        {drop.kind === "registerExecutable" ? <p>确认后，将此应用加入快捷启动。</p> : <>
          <label>使用应用<select aria-label="打开文件的应用" value={selected} disabled={busy || !apps.length} onChange={event => setSelected(event.target.value)}>
            {apps.map(app => <option key={app.id} value={app.id}>{app.label}</option>)}
          </select></label>
          {!apps.length && <p role="status">请先在“桌面伙伴 → 快捷启动”添加应用，再重新拖入文件。</p>}
        </>}
        <small>本次确认剩余约 {remaining} 秒</small>
        <div className="companion-drop-actions"><button type="button" className="button secondary" disabled={busy} onClick={() => void cancel()}>取消</button>
          <button ref={confirm} type="button" className="button primary" disabled={busy || !remaining || (drop.kind === "openFiles" && !selected)} onClick={() => void accept()}>{busy ? "正在处理…" : drop.kind === "registerExecutable" ? "确认添加" : "打开文件"}</button></div>
    </dialog>}
  </>;
}
