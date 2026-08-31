export const UNAVAILABLE_HEADER = "X-FrameForge-Worker";
export const UNAVAILABLE_VALUE = "unavailable";

export function jsonError(status: number, error: string, headers: HeadersInit = {}): Response {
  return new Response(JSON.stringify({ error }), {
    status,
    headers: { "Content-Type": "application/json", ...headers },
  });
}

// The one answer that tells a client to stop asking and go straight to the
// upstreams for the rest of the day. Clients detect it by the header, so it
// stays distinguishable from an upstream 503 relayed through a route.
export function workerUnavailable(): Response {
  return jsonError(503, "worker_unavailable", { [UNAVAILABLE_HEADER]: UNAVAILABLE_VALUE });
}
