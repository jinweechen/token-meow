import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { isDesktop } from "../../shared/bridge";

export function TemporaryInteractionSettings({ enabled = false }: { enabled?: boolean }) {
  const [deadline, setDeadline] = useState(0);
  const [remaining, setRemaining] = useState(0);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  useEffect(() => {
    const update = () => setRemaining(Math.max(0, Math.ceil((deadline - Date.now()) / 1000)));
    update();
    if (!deadline) return;
    const timer = window.setInterval(update, 250);
    return () => window.clearInterval(timer);
  }, [deadline]);
  const change = async (active: boolean) => {
    setPending(true); setError("");
    try {
      if (active) {
        const result = await invoke<{ remainingMs: number }>("begin_pet_temporary_interaction");
        setDeadline(Date.now() + result.remainingMs);
        if (!result.remainingMs) setError("请先显示桌面伙伴");
      } else {
        await invoke("cancel_pet_temporary_interaction"); setDeadline(0);
      }
    } catch (error) { setError(String(error)); }
    finally { setPending(false); }
  };
  return <div className="setting-row">
    <div><strong>临时交互</strong><p>通过托盘或 Ctrl + Alt + I 暂停整体穿透 15 秒，到期恢复当前设置。关闭弹层、隐藏或切换来源也会结束。</p>
      {error && <small role="alert">{error}</small>}</div>
    <button className="button secondary" disabled={!isDesktop || !enabled || pending} onClick={() => void change(!remaining)}>
      {remaining ? `结束临时交互（约 ${remaining} 秒）` : "临时交互 15 秒"}
    </button>
  </div>;
}
