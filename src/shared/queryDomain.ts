import type { Settings, Snapshot } from "./contracts";
import { emptySnapshot } from "./demo";

const domainActions = new Set([
  "clear_data",
  "clear_inbox",
  "set_primary_source",
  "add_source",
  "update_source",
  "relocate_source",
]);

export function changesQueryDomain(action: string): boolean {
  return domainActions.has(action);
}

export function isQueryDomainChange(payload: unknown): boolean {
  return (
    typeof payload === "object" &&
    payload != null &&
    "petDomainChanged" in payload &&
    payload.petDomainChanged === true
  );
}
export function enablesQueryPrivacy(settings: unknown): boolean {
  return (
    typeof settings === "object" &&
    settings != null &&
    "privacyMode" in settings &&
    settings.privacyMode === true
  );
}

/** Never present previously accepted identities while awaiting a private read. */
export function revokeQueryFacts(
  previous: Snapshot,
  settings: Settings = {},
): Snapshot {
  return {
    ...emptySnapshot(),
    settings: { ...previous.settings, ...settings },
  };
}
