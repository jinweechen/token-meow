import type { Snapshot, UsageDashboardRequest } from "./contracts";

export interface SnapshotView {
  readonly enabled: boolean;
  readonly includeTools: boolean;
  readonly request: UsageDashboardRequest;
}

export function usageDashboardRequestKey(request: UsageDashboardRequest): string {
  return JSON.stringify([request.page, request.pageSize, request.quality]);
}

/** Only effective response fields participate; object insertion order does not. */
export function snapshotViewKey(view: SnapshotView): string {
  return JSON.stringify([
    view.enabled ? "dashboard" : "snapshot",
    view.includeTools,
    view.enabled ? usageDashboardRequestKey(view.request) : null,
  ]);
}

/** Older full APIs omitted the marker; an explicit false is never complete. */
export function snapshotViewReady(
  acceptedKey: string | null,
  current: SnapshotView,
  snapshot: Pick<Snapshot, "toolsLoaded">,
): boolean {
  return acceptedKey === snapshotViewKey(current) && (
    !current.includeTools || snapshot.toolsLoaded === undefined ||
    snapshot.toolsLoaded === true
  );
}
