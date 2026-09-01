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

// One object, so every edge location counts into the same total. It sits in one
// location worldwide, so a request that waited on it would pay a cross-globe
// round trip before it could be answered from the edge cache beside it — hence
// the batching below.
const SINGLETON = "daily";

// How long an isolate serves on the verdict it already has before reporting the
// requests it has counted since. Short enough that the brake trips within
// seconds of the threshold, long enough that a busy isolate writes to the
// object a handful of times a minute instead of once per request.
const FLUSH_INTERVAL_MS = 5_000;

// What this isolate knows, which is the whole point: the request path answers
// from here and never waits on the object.
//
// That makes the brake eventually consistent by up to FLUSH_INTERVAL_MS. It can
// overshoot at the crossing by one flush interval of traffic, and an isolate
// that has never flushed serves while its first report is in flight. Both are
// deliberate: this is a spend ceiling, not a correctness gate, and both
// overshoots are bounded by the interval rather than by the day.
let verdict = false;
let pending = 0;
let lastFlush = 0;

export class DailyBudget extends DurableObject {
  async spend(limit: number, requests: number): Promise<boolean> {
    const day = Math.floor(Date.now() / DAY_MS);
    const stored = await this.ctx.storage.get<{ day: number; count: number }>("state");
    const before = stored?.day === day ? stored.count : 0;
    const count = before + requests;
    await this.ctx.storage.put("state", { day, count });

    // The crossing, not the state: from here on every install worldwide falls
    // back to the upstreams direct, and one line marks the moment rather than
    // repeating it for every request that follows. A batch can carry the total
    // well past the threshold in one step, so the line marks the step that
    // passed it rather than one particular total.
    if (before <= limit && count > limit) {
      console.log(JSON.stringify({ event: "budget_exceeded", threshold: limit, count }));
    }

    return count > limit;
  }
}

// Counts this request and answers from what this isolate last heard. Nothing
// here awaits: the report of the requests counted so far goes out behind the
// response.
export function overBudget(env: Env, ctx: ExecutionContext): boolean {
  pending++;

  if (Date.now() - lastFlush >= FLUSH_INTERVAL_MS) {
    // Stamped before the flush is launched, so the requests arriving while it is
    // in flight wait for the next interval instead of each starting one of
    // their own.
    lastFlush = Date.now();
    ctx.waitUntil(flush(env));
  }

  return verdict;
}

// The accurate answer, straight from the object, for a caller with nobody
// waiting on it. The cron tick is the worker's heaviest upstream consumer and
// runs every five minutes, so it is worth the round trip to know rather than
// guess — and it reports whatever the isolate had accumulated on the way.
export async function overBudgetNow(env: Env): Promise<boolean> {
  // A tick spends against the same daily total as a request does.
  pending++;
  lastFlush = Date.now();
  await flush(env);
  return verdict;
}

// Reports the requests counted since the last flush and stores the answer for
// the ones that follow.
async function flush(env: Env): Promise<void> {
  const requests = pending;
  pending = 0;
  try {
    const stub = env.BUDGET.get(env.BUDGET.idFromName(SINGLETON));
    verdict = await stub.spend(Number(env.DAILY_REQUEST_BUDGET), requests);
  } catch (error) {
    // Losing the counter must not lock everybody out: an unreachable object is
    // an outage of the brake, not evidence that the budget was spent, so the
    // last verdict stands. The batch it was carrying is lost with it, which
    // undercounts by one interval. The cost ceiling this gives up is bounded by
    // the platform's own limits.
    //
    // It still gets a line: the platform's own error for a failed object call
    // says nothing about which worker made it or what it was carrying.
    console.log(
      JSON.stringify({
        event: "budget_flush_failed",
        requests,
        error: error instanceof Error ? error.message : String(error),
      }),
    );
  }
}

// Isolate state outlives a single test, and a verdict or a half-counted batch
// left behind would decide the next one. Not used outside tests.
export function resetBudgetState(): void {
  verdict = false;
  pending = 0;
  lastFlush = 0;
}
