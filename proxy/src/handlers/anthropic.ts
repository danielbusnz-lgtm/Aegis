import { ANTHROPIC_URL } from "../constants";
import { cors, jsonResponse, passthroughHeaders, requireDeviceId } from "../http";
import { resolveTier } from "../tiers";
import type { Env } from "../types";
import { consumeTrialTurn, demoUsageKey, readUsage, tallyAnthropicUsage } from "../usage";

/**
 * Full HTTP proxy for Anthropic's Messages API. Trial tier bumps the turn
 * counter pre-flight; demo tier tallies token usage from a teed SSE copy.
 */
export async function handleAnthropic(
    request: Request,
    env: Env,
    ctx: ExecutionContext,
): Promise<Response> {
    const deviceId = requireDeviceId(request);
    if (deviceId instanceof Response) return deviceId;

    const tier = await resolveTier(request, env, deviceId);
    if (tier instanceof Response) return tier;

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

    // Pre-flight cap check. Trial pays a turn up front; demo pays tokens after
    // the SSE stream reports usage.
    if (tier.kind === "trial") {
        const consumed = await consumeTrialTurn(env.USAGE_KV, deviceId, tier.turnsCap);
        if (!consumed) {
            return cors(
                jsonResponse(429, {
                    error: "trial_exhausted",
                    message:
                        "Free trial spent. Use your own API keys (BYOK) or contact us for an invite code.",
                    provider: "anthropic",
                    tier: "trial",
                }),
            );
        }
    } else {
        const kvKey = demoUsageKey(tier.code, deviceId);
        const usage = await readUsage(env.USAGE_KV, kvKey);
        if (
            usage.input >= tier.caps.daily_input_tokens ||
            usage.output >= tier.caps.daily_output_tokens
        ) {
            return cors(
                jsonResponse(429, {
                    error: "daily_cap_exceeded",
                    message: "Daily cap for this invite code reached. Try again tomorrow.",
                    provider: "anthropic",
                    tier: "demo",
                    usage,
                    caps: {
                        input: tier.caps.daily_input_tokens,
                        output: tier.caps.daily_output_tokens,
                    },
                }),
            );
        }
    }

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
        return cors(
            new Response(upstream.body, {
                status: upstream.status,
                headers: passthroughHeaders(upstream.headers),
            }),
        );
    }

    // Only demo tier needs token accounting. Trial already paid its turn.
    if (tier.kind === "demo") {
        const [toClient, toTally] = upstream.body.tee();
        ctx.waitUntil(
            tallyAnthropicUsage(toTally, env.USAGE_KV, demoUsageKey(tier.code, deviceId)),
        );
        return cors(
            new Response(toClient, {
                status: 200,
                headers: passthroughHeaders(upstream.headers),
            }),
        );
    }

    return cors(
        new Response(upstream.body, {
            status: 200,
            headers: passthroughHeaders(upstream.headers),
        }),
    );
}
