import type { CompanionLauncher } from "./desktopCompanion";

export interface CompanionDrop {
  batchId: string;
  kind: "registerExecutable" | "openFiles";
  fileNames: string[];
  expiresInMs: number;
}
export interface CompanionDropClient {
  current(): Promise<CompanionDrop | null>;
  launchers(): Promise<CompanionLauncher[]>;
  register(batchId: string): Promise<unknown>;
  open(batchId: string, launcherId: string): Promise<unknown>;
  cancel(batchId: string): Promise<unknown>;
  subscribe(changed: () => void, failed: (message: string) => void): Promise<() => void>;
}
export function parseCompanionDrop(value: unknown): CompanionDrop | null {
  if (value === null) return null;
  if (!value || typeof value !== "object") throw Error("拖放信息暂不可用，请重新拖入。");
  const drop = value as CompanionDrop;
  if (Object.keys(drop).length !== 4 || Object.keys(drop).some(key => !["batchId", "kind", "fileNames", "expiresInMs"].includes(key)) ||
      typeof drop.batchId !== "string" || !/^[a-zA-Z0-9-]{16,80}$/.test(drop.batchId) ||
      !["registerExecutable", "openFiles"].includes(drop.kind) || !Array.isArray(drop.fileNames) ||
      drop.fileNames.length < 1 || drop.fileNames.length > 8 ||
      drop.fileNames.some(name => typeof name !== "string" || !name.length || name.length > 256 || /[\\/\p{Cc}\u061c\u200e\u200f\u202a-\u202e\u2066-\u2069]/u.test(name)) ||
      (drop.kind === "registerExecutable" && drop.fileNames.length !== 1) ||
      !Number.isSafeInteger(drop.expiresInMs) || drop.expiresInMs <= 0 || drop.expiresInMs > 60_000)
    throw Error("拖放信息已失效，请重新拖入。");
  return { ...drop, fileNames: [...drop.fileNames] };
}
