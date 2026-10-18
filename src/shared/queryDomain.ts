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
