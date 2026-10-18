import type { AgentKind, Query, Session, Turn } from "./contracts";

/** Missing metadata is not positive evidence of a primary agent. */
export const sessionAgentKind = (session: Session): AgentKind =>
  session.agentKind === "primary" || session.agentKind === "subagent"
    ? session.agentKind
    : "unknown";

export const agentKindLabel = (kind: AgentKind): string =>
  ({ primary: "主代理", subagent: "子代理", unknown: "归属未知" })[kind];

export function applyAgentKindFilter(query: Query, value: string): Query {
  if (!value) return { ...query, agentKind: undefined };
  return ["primary", "subagent", "unknown"].includes(value)
    ? { ...query, agentKind: value as AgentKind }
    : query;
}

export interface ScopedIdentity {
  sourceId: string;
  id: string;
}
export const identityValue = ({ sourceId, id }: ScopedIdentity) =>
  JSON.stringify([sourceId, id]);
export function parseIdentity(value: string): ScopedIdentity | null {
  try {
    const parsed: unknown = JSON.parse(value);
    if (
      Array.isArray(parsed) &&
      parsed.length === 2 &&
      parsed.every((item) => typeof item === "string" && item.length > 0)
    )
      return { sourceId: parsed[0], id: parsed[1] };
  } catch {
    /* Invalid selection is never inferred from a short ID. */
  }
  return null;
}
export function rootTurnOptions(
  turns: Turn[],
  sessions: Session[],
): ScopedIdentity[] {
  const roots = new Map<string, ScopedIdentity>();
  for (const turn of turns) {
    if (!turn.sourceId || (turn.rootTurnId && turn.rootTurnId !== turn.id))
      continue;
    const session = sessions.find(
      (item) => item.sourceId === turn.sourceId && item.id === turn.threadId,
    );
    if (!turn.rootTurnId && session?.parentThreadId) continue;
    const option = { sourceId: turn.sourceId, id: turn.id };
    roots.set(identityValue(option), option);
  }
  return [...roots.values()];
}
export function applyRootFilter(query: Query, value: string): Query {
  if (!value) return { ...query, rootTurnId: undefined };
  const identity = parseIdentity(value);
  return identity
    ? {
        ...withoutDetailScope(query),
        sourceId: identity.sourceId,
        rootTurnId: identity.id,
      }
    : query;
}
export const applySourceFilter = (query: Query, sourceId: string): Query => ({
  ...withoutDetailScope(query),
  sourceId: sourceId || undefined,
});

/** Identity changes preserve dates, but do not carry another target's filters. */
function withoutDetailScope(query: Query): Query {
  const {
    threadId: _thread,
    rootTurnId: _root,
    model: _model,
    project: _project,
    providerId: _provider,
    clientKind: _client,
    ...rangeAndSource
  } = query;
  return rangeAndSource;
}

export function applySessionFilter(query: Query, value: string): Query {
  if (!value) return { ...query, threadId: undefined };
  const identity = parseIdentity(value);
  return identity
    ? {
        ...withoutDetailScope(query),
        sourceId: identity.sourceId,
        threadId: identity.id,
      }
    : query;
}

export interface SessionFilterOption extends ScopedIdentity {
  title?: string;
  selectedOnly?: boolean;
}

/** Candidates are only observed sessions, never a fabricated full directory. */
export function sessionFilterOptions(
  sessions: Session[],
  query: Query,
): SessionFilterOption[] {
  const options = new Map<string, SessionFilterOption>();
  for (const session of sessions) {
    if (!session.sourceId || !session.id) continue;
    const option = {
      sourceId: session.sourceId,
      id: session.id,
      title: session.title,
    };
    options.set(identityValue(option), option);
  }
  if (query.threadId && query.sourceId) {
    const selected = { id: query.threadId, sourceId: query.sourceId };
    const key = identityValue(selected);
    if (!options.has(key))
      options.set(key, { ...selected, selectedOnly: true });
  }
  return [...options.values()];
}

export function sessionFilterLabel(
  option: SessionFilterOption,
  index: number,
  privateMode: boolean,
  sources: { id: string }[] = [],
): string {
  const name = privateMode
    ? option.selectedOnly
      ? "已选会话"
      : `会话 ${index + 1}`
    : option.selectedOnly
      ? `已选会话 ${option.id.slice(0, 12)}`
      : option.title || `会话 ${option.id.slice(0, 12)}`;
  return `${name} · ${sourceLabel(option.sourceId, sources)}`;
}

export function turnsForSession(turns: Turn[], session: Session): Turn[] {
  return turns.filter(
    (turn) =>
      turn.threadId === session.id &&
      !!session.sourceId &&
      turn.sourceId === session.sourceId,
  );
}
export function projectLabel(
  session: Session,
  projects: { id: string; name: string }[],
): string {
  return (
    session.projectName ||
    projects.find((project) => project.id === session.project)?.name ||
    "项目未知"
  );
}
export function sourceLabel(
  sourceId: string | undefined,
  sources: { id: string }[] = [],
): string {
  const index = sources.findIndex((source) => source.id === sourceId);
  return index >= 0 ? `来源 ${index + 1}` : "来源未确认";
}
