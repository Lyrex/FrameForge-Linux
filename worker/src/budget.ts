// The daily spend brake.
//
// A bug or an abusive caller can only cost a bounded amount: every request
// counts against one shared daily total, and once that total is past the
// threshold the worker stands down until the next reset and clients fetch the
// upstreams direct.
//
// The counter lives in a Durable Object because it has to be one number across
// every edge location that serves a request. KV is eventually consistent, so a
// counter kept there would read minutes-old totals in exactly the burst the
// brake exists to stop.

import { DurableObject } from "cloudflare:workers";

// The client stands down until the next UTC midnight when it sees the
// unavailable signal, so the counter has to reset at the same instant —
// otherwise the two disagree about when service returns.
const DAY_MS = 86_400_000;

// One object, so every edge location counts into the same total.
//
// TODO: that also serializes every request through a single object, whose
// storage write rate caps the whole worker at roughly a few hundred requests a
// second no matter how many edge locations serve them. Shard the counter over N
// objects and sum them at the read, or batch the writes so a request counts
// against an in-memory total that is flushed periodically.
const SINGLETON = "daily";

export class DailyBudget extends DurableObject {
  async spend(limit: number): Promise<boolean> {
    const day = Math.floor(Date.now() / DAY_MS);
    const stored = await this.ctx.storage.get<{ day: number; count: number }>("state");
    const count = (stored?.day === day ? stored.count : 0) + 1;
    await this.ctx.storage.put("state", { day, count });

    // The crossing, not the state: from here on every install worldwide falls
    // back to the upstreams direct, and one line marks the moment rather than
    // repeating it for every request that follows.
    if (count === limit + 1) {
      console.log(JSON.stringify({ event: "budget_exceeded", threshold: limit, count }));
    }

    return count > limit;
  }
}

// Counts this request and answers whether the day's budget is already spent.
export async function overBudget(env: Env): Promise<boolean> {
  try {
    const stub = env.BUDGET.get(env.BUDGET.idFromName(SINGLETON));
    return await stub.spend(Number(env.DAILY_REQUEST_BUDGET));
  } catch {
    // Losing the counter must not lock everybody out: an unreachable object is
    // an outage of the brake, not evidence that the budget was spent. The cost
    // ceiling this gives up is bounded by the platform's own limits.
    return false;
  }
}
