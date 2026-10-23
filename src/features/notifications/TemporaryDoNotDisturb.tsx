import type { Settings } from "../../shared/contracts";
import { isDesktop } from "../../shared/bridge";
import { temporaryDndState, temporaryDndStatus } from "../../shared/temporaryDnd";
import "./temporaryDnd.css";

interface Props {
  settings: Settings;
  pending: boolean;
  onRequest: (minutes: 0 | 30 | 60) => void;
}

export function TemporaryDoNotDisturb({ settings, pending, onRequest }: Props) {
  const state = temporaryDndState(settings);
  const disabled = pending || !isDesktop || !state;
  return <div className="temporary-dnd-setting">
    <strong>临时免打扰</strong>
    <p className="temporary-dnd-status" role="status">{temporaryDndStatus(settings)}</p>
    <div className="temporary-dnd-actions">
      <button className="button secondary small" disabled={disabled} onClick={() => onRequest(30)}>
        {state?.active ? "重新计时 30 分钟" : "安静 30 分钟"}
      </button>
      <button className="button secondary small" disabled={disabled} onClick={() => onRequest(60)}>
        {state?.active ? "重新计时 60 分钟" : "安静 60 分钟"}
      </button>
      {state?.active && <button className="button secondary small" disabled={disabled} onClick={() => onRequest(0)}>结束临时免打扰</button>}
    </div>
    <p className="subtle-note">本次运行有效；到期或重启后结束。手动免打扰和每日时段继续按各自设置生效。</p>
    {!isDesktop && <p className="subtle-note">请在 Windows 桌面应用中使用。</p>}
  </div>;
}

export function TemporaryDoNotDisturbMenu({ settings, pending, onRequest }: Props) {
  const state = temporaryDndState(settings);
  if (!isDesktop || !state) return null;
  return <div className="temporary-dnd-menu">
    <h3>临时免打扰</h3>
    <p className="subtle-note" role="status">{temporaryDndStatus(settings)}</p>
    <button disabled={pending} onClick={() => onRequest(30)}>{state.active ? "重新计时 30 分钟" : "安静 30 分钟"}</button>
    <button disabled={pending} onClick={() => onRequest(60)}>{state.active ? "重新计时 60 分钟" : "安静 60 分钟"}</button>
    {state.active && <button disabled={pending} onClick={() => onRequest(0)}>结束临时免打扰</button>}
    <p className="subtle-note">到期或重启结束，不改变手动和每日设置。</p>
  </div>;
}
