import type { Query } from "./contracts";

const scopeFilterKeys = [
  "project",
  "threadId",
  "rootTurnId",
  "model",
  "sourceId",
  "clientKind",
  "providerId",
  "agentKind",
] as const;

export type ScopeFilterKey = (typeof scopeFilterKeys)[number];

function hasScopeValue(query: Query, key: ScopeFilterKey): boolean {
  const value = query[key];
  if (typeof value !== "string" || value.trim().length === 0) return false;
  return (
    key !== "agentKind" ||
    value === "primary" ||
    value === "subagent" ||
    value === "unknown"
  );
}

/** Removing a source also removes identities that only make sense in that source. */
export function removeScopeFilter(query: Query, key: ScopeFilterKey): Query {
  const next = { ...query };
  delete next[key];
  if (key === "sourceId") {
    delete next.threadId;
    delete next.rootTurnId;
  }
  return next;
}

/** Date and legacy visibility remain selected when clearing the scope. */
export function clearScopeFilters(query: Query): Query {
  const { from, to, includeLegacy } = query;
  return {
    ...(from !== undefined ? { from } : {}),
    ...(to !== undefined ? { to } : {}),
    ...(includeLegacy !== undefined ? { includeLegacy } : {}),
  };
}

/** Broaden one detail level within the current intersection, without inventing history. */
export function parentScopeQuery(query: Query): Query | null {
  for (const key of ["threadId", "rootTurnId", "project"] as const) {
    if (hasScopeValue(query, key)) return removeScopeFilter(query, key);
  }
  return null;
}

export function activeScopeFilterKeys(query: Query): ScopeFilterKey[] {
  return scopeFilterKeys.filter((key) => hasScopeValue(query, key));
}
