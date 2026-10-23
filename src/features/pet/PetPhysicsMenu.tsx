import type { PetPhysicsController } from "./usePetPhysics";

export function PetPhysicsMenu({ physics, onBeforeRequest }: { physics: PetPhysicsController; onBeforeRequest?: () => Promise<boolean> }) {
  const disabled = !physics.available || physics.pending;
  const start = async (operation: () => Promise<boolean>) => {
    if (onBeforeRequest && !await onBeforeRequest()) return;
    await operation();
  };
  return <div className="pet-physics-menu">
    <p>先把桌宠拖近普通前台窗口的上沿或侧边，再选择动作。</p>
    <button type="button" disabled={disabled} onClick={() => { void start(physics.controls.dock); }}>停在窗口上沿</button>
    <button type="button" disabled={disabled || !physics.climbAvailable} onClick={() => { void start(() => physics.controls.climb("left", "up")); }}>沿左侧向上爬</button>
    <button type="button" disabled={disabled || !physics.climbAvailable} onClick={() => { void start(() => physics.controls.climb("right", "up")); }}>沿右侧向上爬</button>
    {!physics.climbAvailable && physics.available && <p>攀爬需要 VRM 角色。</p>}
    <label><input type="checkbox" checked={physics.state.throwEnabled} disabled={physics.pending || (!physics.available && !physics.state.throwEnabled)}
      onChange={event => { const enabled = event.target.checked; void start(() => physics.controls.setThrowEnabled(enabled)); }} />拖动松手时轻抛（仅本次运行）</label>
    {physics.state.request && <button type="button" disabled={physics.pending} onClick={() => { void start(physics.controls.cancel); }}>停止这次移动</button>}
    {(physics.error || physics.blockedReason) && <p role={physics.error ? "status" : undefined}>{physics.error ?? physics.blockedReason}</p>}
  </div>;
}
