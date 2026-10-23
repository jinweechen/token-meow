import type { PetPresentation, Snapshot } from "../../shared/contracts";

/** The panel preview projects real panel facts, without inventing a Snapshot. */
export function projectSnapshotPresentation(snapshot: Snapshot): PetPresentation {
  return {
    settings: snapshot.settings,
    activity: snapshot.activity,
    health: {
      lastSuccessfulScan: snapshot.health.lastSuccessfulScan,
      lastSourceEvent: snapshot.health.lastSourceEvent,
      issues: snapshot.health.issues,
      conflicts: snapshot.health.conflicts,
    },
    unreadCount: snapshot.inbox.filter((event) => !event.read).length,
    prompts: snapshot.inbox.map((event) => ({
      id: event.id,
      kind: event.kind,
      status: event.status ?? "unknown",
      createdAt: event.createdAt,
      animationEligible: event.animationEligible === true,
    })),
    sourceScopeRevision: JSON.stringify([
      snapshot.primarySourceId ?? null,
      [...(snapshot.sources ?? [])]
        .sort((a, b) => a.id.localeCompare(b.id))
        .map((source) => [
          source.id,
          source.root,
          source.enabled,
          source.runtime ?? null,
        ]),
    ]),
  };
}
