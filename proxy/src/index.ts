// aegis-proxy: Cloudflare Worker that fronts Anthropic for the aegis client.
//
// Why it exists:
//   The desktop app ships without API keys so non-technical users can install
//   it and have it just work. The Worker holds the secret Anthropic key, caps
//   per-device daily usage from a KV store, and streams responses back.
//
// v0.1 scope: Anthropic /v1/messages with streaming only. Deepgram + Cartesia
// stay BYOK in the client until the WebSocket-proxy plumbing is built.

export interface Env {
    /** Anthropic API key. Set via `wrangler secret put ANTHROPIC_API_KEY`. */
    ANTHROPIC_API_KEY: string;
    /** KV namespace storing `usage:{deviceId}:{yyyy-mm-dd}` -> `{input,output}`. */
    USAGE_KV: KVNamespace;
    /** Daily caps as decimal strings (tunable without redeploy via wrangler.toml). */
    DAILY_INPUT_TOKEN_CAP: string;
    DAILY_OUTPUT_TOKEN_CAP: string;
}

type Usage = { input: number; output: number };

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const ANTHROPIC_URL = "https://api.anthropic.com/v1/messages";
const KV_TTL_SECONDS = 48 * 60 * 60; // two days, lets yesterday roll off

export default {
    async fetch(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
        const url = new URL(request.url);

        if (request.method === "OPTIONS") return cors(new Response(null, { status: 204 }));
        if (request.method !== "POST" || url.pathname !== "/v1/anthropic/messages") {
            return cors(new Response("Not found", { status: 404 }));
        }

        // 1. Verify device id header. UUID format only; v0.1 has no real auth,
        //    but the format check stops trivial typo-spam and obvious bots.
        const deviceId = request.headers.get("x-aegis-device-id");
        if (!deviceId || !UUID_RE.test(deviceId)) {
            return cors(jsonResponse(401, { error: "missing or invalid X-Aegis-Device-Id" }));
        }

        // 2. Read body and require streaming. Non-streaming would bypass our
        //    token tally (response shape differs), so we just refuse those.
        const rawBody = await request.text();
        let parsed: { stream?: boolean };
        try {
            parsed = JSON.parse(rawBody);
        } catch {
            return cors(jsonResponse(400, { error: "request body must be JSON" }));
        }
        if (parsed.stream !== true) {
            return cors(jsonResponse(400, { error: "stream: true required" }));
        }

        // 3. Look up today's usage and reject if either cap is hit.
        const today = utcDateKey(new Date());
        const kvKey = `usage:${deviceId}:${today}`;
        const usage = await readUsage(env.USAGE_KV, kvKey);
        const inputCap = parseCap(env.DAILY_INPUT_TOKEN_CAP, 30_000);
        const outputCap = parseCap(env.DAILY_OUTPUT_TOKEN_CAP, 10_000);

        if (usage.input >= inputCap || usage.output >= outputCap) {
            return cors(
                jsonResponse(429, {
                    error: "daily_cap_exceeded",
                    message: "Daily free tier cap reached. Try again tomorrow.",
                    usage,
                    caps: { input: inputCap, output: outputCap },
                }),
            );
        }

        // 4. Forward to Anthropic with our secret key. Pass through the client's
        //    body as-is (model, tools, etc.). anthropic-beta is forwarded only
        //    if the client sent one — Anthropic rejects an empty value.
        const upstreamHeaders: Record<string, string> = {
            "x-api-key": env.ANTHROPIC_API_KEY,
            "anthropic-version": request.headers.get("anthropic-version") ?? "2023-06-01",
            "content-type": "application/json",
        };
        const beta = request.headers.get("anthropic-beta");
        if (beta) upstreamHeaders["anthropic-beta"] = beta;
        const upstream = await fetch(ANTHROPIC_URL, {
            method: "POST",
            headers: upstreamHeaders,
            body: rawBody,
        });

        if (!upstream.ok || !upstream.body) {
            // Pass error responses straight through. The client knows how to
            // surface Anthropic errors already.
            return cors(
                new Response(upstream.body, {
                    status: upstream.status,
                    headers: passthroughHeaders(upstream.headers),
                }),
            );
        }

        // 5. Tee the SSE stream: one copy to the client (latency-critical),
        //    one copy parsed in the background to tally tokens. waitUntil keeps
        //    the Worker alive past the response so the tally can finish.
        const [toClient, toTally] = upstream.body.tee();
        ctx.waitUntil(tallyAndPersist(toTally, env.USAGE_KV, kvKey));

        return cors(
            new Response(toClient, {
                status: 200,
                headers: passthroughHeaders(upstream.headers),
            }),
        );
    },
} satisfies ExportedHandler<Env>;

// ────────────────────────────────────────────────────────────────────────────
// Helpers
// ────────────────────────────────────────────────────────────────────────────

function utcDateKey(d: Date): string {
    // YYYY-MM-DD in UTC. Same key everywhere regardless of user timezone.
    return d.toISOString().slice(0, 10);
}

function parseCap(value: string | undefined, fallback: number): number {
    const n = parseInt(value ?? "", 10);
    return Number.isFinite(n) && n > 0 ? n : fallback;
}

async function readUsage(kv: KVNamespace, key: string): Promise<Usage> {
    const raw = await kv.get(key);
    if (!raw) return { input: 0, output: 0 };
    try {
        const parsed = JSON.parse(raw) as Partial<Usage>;
        return {
            input: typeof parsed.input === "number" ? parsed.input : 0,
            output: typeof parsed.output === "number" ? parsed.output : 0,
        };
    } catch {
        return { input: 0, output: 0 };
    }
}

function jsonResponse(status: number, body: unknown): Response {
    return new Response(JSON.stringify(body), {
        status,
        headers: { "content-type": "application/json" },
    });
}

function cors(res: Response): Response {
    const headers = new Headers(res.headers);
    headers.set("access-control-allow-origin", "*");
    headers.set("access-control-allow-methods", "POST, OPTIONS");
    headers.set(
        "access-control-allow-headers",
        "content-type, anthropic-version, anthropic-beta, x-aegis-device-id",
    );
    return new Response(res.body, { status: res.status, headers });
}

function passthroughHeaders(upstream: Headers): Headers {
    // Keep content-type so SSE works; drop hop-by-hop and CF-specific stuff.
    const out = new Headers();
    const ct = upstream.get("content-type");
    if (ct) out.set("content-type", ct);
    const cc = upstream.get("cache-control");
    if (cc) out.set("cache-control", cc);
    return out;
}

/**
 * Walk the SSE response stream, sum input/output tokens reported by Anthropic,
 * then add them into the day's KV entry. Anthropic reports usage in two places:
 *
 *   - event `message_start` carries `message.usage.input_tokens` (final input count)
 *   - event `message_delta` carries `usage.output_tokens` (cumulative output)
 *
 * Each later `message_delta` supersedes earlier ones for the same response, so
 * we overwrite `output` with whatever the latest delta says. `input_tokens`
 * appears exactly once at the start.
 */
async function tallyAndPersist(
    stream: ReadableStream<Uint8Array>,
    kv: KVNamespace,
    kvKey: string,
): Promise<void> {
    let input = 0;
    let output = 0;
    const reader = stream.getReader();
    const decoder = new TextDecoder();
    let buf = "";

    try {
        while (true) {
            const { done, value } = await reader.read();
            if (done) break;
            buf += decoder.decode(value, { stream: true });

            // SSE events are delimited by a blank line. Split, keep the last
            // (possibly partial) event in the buffer for the next iteration.
            const events = buf.split("\n\n");
            buf = events.pop() ?? "";

            for (const evt of events) {
                const dataLine = evt.split("\n").find((l) => l.startsWith("data: "));
                if (!dataLine) continue;
                const payload = dataLine.slice(6);
                if (payload === "[DONE]") continue;
                try {
                    const obj = JSON.parse(payload);
                    if (obj.type === "message_start" && obj.message?.usage?.input_tokens != null) {
                        input = obj.message.usage.input_tokens;
                    } else if (obj.type === "message_delta" && obj.usage?.output_tokens != null) {
                        output = obj.usage.output_tokens;
                    }
                } catch {
                    // Non-JSON or partial payload — skip.
                }
            }
        }
    } catch (err) {
        console.error("tally read error:", err);
    }

    if (input === 0 && output === 0) return;

    // Read-modify-write is not atomic in KV. Two concurrent requests from the
    // same device can race here. For per-device daily caps the worst case is
    // mild under-counting on bursts, which is acceptable for the MVP. Real
    // protection is rate-limit-by-IP, which Cloudflare already gives us.
    const existing = await readUsage(kv, kvKey);
    const total = { input: existing.input + input, output: existing.output + output };
    await kv.put(kvKey, JSON.stringify(total), { expirationTtl: KV_TTL_SECONDS });
}
