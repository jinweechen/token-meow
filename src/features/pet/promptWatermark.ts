import type { PetPrompt } from "../../shared/contracts";
import type { Prompt } from "./presenter";

/** Session-local rendering watermark. Persistent eligibility is owned by the ledger. */
export class PromptWatermark {
  private initialized = false;
  private seen = new Set<string>();
  consume(events: readonly (Omit<PetPrompt, "animationEligible"> & { animationEligible?: boolean })[], now: number, suppressed: boolean): Prompt[] {
    const prompts: Prompt[] = [];
    for (const event of events) {
      const alreadySeen = this.seen.has(event.id);
      this.seen.add(event.id);
      const createdAt = new Date(event.createdAt).getTime();
      if (
        this.initialized &&
        !alreadySeen &&
        !suppressed &&
        event.animationEligible === true &&
        Number.isFinite(createdAt) &&
        now - createdAt >= 0 &&
        now - createdAt <= 10_000
      )
        prompts.push({
          id: event.id,
          status: event.status,
          createdAt,
          kind: event.kind,
        });
    }
    this.initialized = true;
    // Eligibility TTL is ten seconds; retaining a bounded LRU is enough to reject revisions.
    if (this.seen.size > 1024) this.seen = new Set([...this.seen].slice(-1024));
    return prompts;
  }
  reset(): void {
    this.initialized = false;
    this.seen.clear();
  }
}
