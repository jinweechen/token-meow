import type { Settings } from "../../shared/contracts";
import { companionText, type CompanionCapabilities } from "../../shared/desktopCompanion";
import * as bridge from "../../shared/bridge";
import { Icon, Toggle } from "../../shared/UI";
import { useCompanionLaunchers, useDesktopCompanion } from "./useDesktopCompanion";
import { LauncherManager } from "./LauncherManager";

export function CompanionSettings({ settings, save, pending }: { settings: Settings; save: (settings: Settings) => Promise<void>; pending: boolean }) {
  const companion = useDesktopCompanion();
  const capabilities = companion.error ? undefined : companion.state?.capabilities;
  const items = useCompanionLaunchers(capabilities?.launchers === true);
  const native = bridge.isDesktop;
  const row = (key: "idlePlayEnabled" | "idleRoamingEnabled" | "workAppLinkEnabled", label: string, detail: string, available: boolean) => <div className="setting-row">
    <div><strong>{label}</strong><p>{detail}</p></div>
    <Toggle label={label} checked={available && (key === "idleRoamingEnabled" ? settings[key] === true : settings[key] !== false)} disabled={pending || !native || !available} onChange={(value) => void save({ [key]: value })} />
  </div>;
  return <section className="card companion-settings">
    <div className="section-heading"><h2>桌面活动与应用</h2><Icon name="sparkles" size={20} /></div>
    {!native && <p className="notice inset">这些功能在 Windows 桌面应用中可用。浏览器预览不会读取活动应用或启动程序。</p>}
    <p className="companion-context" role="status">{native ? companionText(companion.state, !!settings.privacyMode) : "浏览器演示预览"}</p>
    {row("idlePlayEnabled", "空闲时原地玩耍", "鼠标键盘空闲 2 分钟后，小柚会伸懒腰、做小动作；恢复操作即停。菜单、拖动、免打扰或系统暂停时保持安静。", capabilities?.systemIdle === true)}
    {capabilities?.roaming === true && <>
      {row("idleRoamingEnabled", "空闲时走动", "默认关闭。开启后，空闲 2 分钟且任务已空闲时，在选定范围内短暂散步；操作、菜单和换肤会停止移动。本地皮肤只原地播放。", true)}
      <div className="setting-row"><div><strong>走动范围</strong><p>范围按完整角色窗口计算，保持在当前屏幕工作区内。</p></div>
        <select aria-label="走动范围" value={settings.idleRoamRange === "workArea" ? "workArea" : "nearby"} disabled={pending || !native || settings.idleRoamingEnabled !== true} onChange={(event) => void save({ idleRoamRange: event.target.value === "nearby" ? "nearby" : "workArea" })}>
          <option value="nearby">附近（左右各 160 逻辑像素）</option><option value="workArea">当前屏幕工作区</option>
        </select>
      </div>
    </>}
    {row("workAppLinkEnabled", "工作应用陪伴", "识别你登记的工作应用，陪你一起工作。不读取窗口标题、文件内容或输入内容。", capabilities?.workApps === true)}
    <div className="section-heading companion-launcher-heading"><h3>快捷启动</h3>
      <button className="button secondary small" disabled={!native || capabilities?.launchers !== true || items.pending} onClick={() => void items.perform(() => bridge.chooseCompanionLauncher())}><Icon name="plus" size={14} />添加应用</button>
    </div>
    <LauncherManager launchers={items.launchers} pending={items.pending} disabled={!native || capabilities?.launchers !== true} perform={items.perform} />
    {native && capabilities?.fileDrop === true && <p className="subtle-note">也可以把一个 .exe 拖入此窗口，确认后加入快捷启动。把最多 8 个文件拖给桌宠，可选择登记的应用打开。</p>}
    {native && companion.state && capabilities?.launchers === false && <p className="companion-error" role="alert">快捷启动登记暂时不可用，已有记录会保留。</p>}
    {(companion.error || items.error) && <p className="companion-error" role="alert">{companion.error ?? items.error}</p>}
  </section>;
}

export function CompanionMenu({ capabilities }: { capabilities?: CompanionCapabilities }) {
  const items = useCompanionLaunchers(capabilities?.launchers === true);
  if (!bridge.isDesktop || capabilities?.launchers !== true) return null;
  return <div className="companion-menu-section">
    <h3>快捷启动</h3>
    {items.launchers.length ? items.launchers.map((app) => <button key={app.id} disabled={items.pending} onClick={() => void items.perform(() => bridge.launchCompanionApp(app.id))}><Icon name="external" size={16} />{app.label}</button>) : <p className="subtle-note">在“偏好设置 → 桌面伙伴”添加常用应用。</p>}
    {items.error && <p className="companion-error" role="alert">{items.error}</p>}
  </div>;
}
