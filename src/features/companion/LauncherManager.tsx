import { useId, useState } from "react";
import type { CompanionLauncher } from "../../shared/desktopCompanion";
import * as bridge from "../../shared/bridge";
import { Toggle } from "../../shared/UI";
import "./launcherManager.css";

interface LauncherManagerProps {
  launchers: CompanionLauncher[];
  pending: boolean;
  disabled: boolean;
  perform: (operation: () => Promise<unknown>) => Promise<boolean>;
}

function LauncherRow({ app, first, last, pending, disabled, perform }: Omit<LauncherManagerProps, "launchers"> & {
  app: CompanionLauncher; first: boolean; last: boolean;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(app.label);
  const [error, setError] = useState("");
  const errorId = useId();
  const busy = disabled || pending;
  const saveName = async () => {
    if (busy) return;
    const label = draft.trim();
    if (/[\u0000-\u001f\u007f-\u009f\u061c\u200e\u200f\u202a-\u202e\u2066-\u2069]/u.test(draft)) {
      setError("名称不能包含控制字符或文字方向控制符。");
      return;
    }
    if (!label || Array.from(label).length > 80) {
      setError("请输入 1～80 个字符的应用名称。");
      return;
    }
    setError("");
    if (await perform(() => bridge.updateCompanionLauncher({ launcherId: app.id, label }))) setEditing(false);
  };
  return <li className="launcher-manager-item">
    <div className="launcher-manager-details">
      <strong className="launcher-manager-name">{app.label}</strong>
      <small>{app.workAppEnabled ? "参与工作应用陪伴" : "仅作快捷启动"}</small>
    </div>
    <div className="companion-actions launcher-manager-actions">
      <button className="button secondary small" disabled={busy || editing} onClick={() => {
        setDraft(app.label); setError(""); setEditing(true);
      }} aria-label={`重命名 ${app.label}`}>重命名</button>
      <button className="button secondary small" disabled={busy || first} aria-label={`上移 ${app.label}`}
        onClick={() => void perform(() => bridge.moveCompanionLauncher(app.id, "up"))}>上移</button>
      <button className="button secondary small" disabled={busy || last} aria-label={`下移 ${app.label}`}
        onClick={() => void perform(() => bridge.moveCompanionLauncher(app.id, "down"))}>下移</button>
      <button className="button secondary small" disabled={busy}
        onClick={() => void perform(() => bridge.launchCompanionApp(app.id))}>启动</button>
      <button className="button secondary small" disabled={busy}
        onClick={() => void perform(() => bridge.removeCompanionLauncher(app.id))}>移除</button>
    </div>
    <div className="launcher-manager-work-app">
      <span>工作陪伴</span>
      <Toggle label={`${app.label} 的工作陪伴`} checked={app.workAppEnabled} disabled={busy}
        onChange={(workAppEnabled) => void perform(() => bridge.updateCompanionLauncher({ launcherId: app.id, workAppEnabled }))} />
    </div>
    {editing && <form className="launcher-manager-editor" onSubmit={(event) => { event.preventDefault(); void saveName(); }}>
      <label className="launcher-manager-input">
        <span>应用名称</span>
        <input autoFocus value={draft} disabled={busy} aria-label={`应用名称：${app.label}`}
          aria-invalid={!!error} aria-describedby={error ? errorId : undefined}
          onChange={(event) => { setDraft(event.target.value); setError(""); }}
          onKeyDown={(event) => {
            if (event.key === "Escape" && !busy) { event.preventDefault(); setEditing(false); setError(""); }
          }} />
      </label>
      <div className="companion-actions">
        <button className="button primary small" type="submit" disabled={busy}>保存名称</button>
        <button className="button secondary small" type="button" disabled={busy} onClick={() => { setEditing(false); setError(""); }}>取消</button>
      </div>
      {error && <p id={errorId} className="companion-error" role="alert">{error}</p>}
    </form>}
  </li>;
}

export function LauncherManager(props: LauncherManagerProps) {
  if (!props.launchers.length) return <p className="subtle-note">添加常用应用后，可从小柚的右键菜单直接启动。</p>;
  return <>
    <p className="subtle-note">上下移动会同步调整右键菜单顺序；关闭某项工作陪伴后，仍可手动启动该应用。</p>
    <ul className="companion-launcher-list" aria-label="快捷启动应用">
      {props.launchers.map((app, index) => <LauncherRow key={app.id} {...props} app={app} first={index === 0} last={index === props.launchers.length - 1} />)}
    </ul>
  </>;
}
