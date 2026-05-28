// Short-lived token minting for Deepgram (STT) and Cartesia (TTS). Both follow
// the same shape: enforce the tier cap, ask the provider for a scoped token,
// hand it back. The client then streams directly to the provider over a
// WebSocket, so the Worker never sits on the audio data path.

import {
    CARTESIA_API_VERSION,
    CARTESIA_TOKEN_TTL_SECONDS,
    CARTESIA_TOKEN_URL,
    DEEPGRAM_TOKEN_TTL_SECONDS,
    DEEPGRAM_TOKEN_URL,
} from "../constants";
import { cors, jsonResponse, requireDeviceId } from "../http";
import { resolveTier } from "../tiers";
import type { Env } from "../types";
import { bumpCounter, consumeTrialTurn, demoUsageKey, readUsage } from "../usage";

/**
 * Mints a short-lived Deepgram JWT scoped to one streaming session. Client
 * uses the token to open a WS directly with Deepgram, bypassing the Worker.
 */
export async function handleDeepgramToken(
    request: Request,
    env: Env,
    ctx: ExecutionContext,
): Promise<Response> {
    const deviceId = requireDeviceId(request);
    if (deviceId instanceof Response) return deviceId;

    const tier = await resolveTier(request, env, deviceId);
    if (tier instanceof Response) return tier;

    if (tier.kind === "trial") {
        const consumed = await consumeTrialTurn(env.USAGE_KV, deviceId, tier.turnsCap);
        if (!consumed) {
            return cors(
                jsonResponse(429, {
                    error: "trial_exhausted",
                    message:
                        "Free trial spent. Use your own API keys (BYOK) or contact us for an invite code.",
                    provider: "deepgram",
                    tier: "trial",
                }),
            );
        }
    } else {
        const kvKey = demoUsageKey(tier.code, deviceId);
        const usage = await readUsage(env.USAGE_KV, kvKey);
        if (usage.deepgram_tokens >= tier.caps.daily_deepgram_tokens) {
            return cors(
                jsonResponse(429, {
                    error: "daily_cap_exceeded",
                    message: "Daily STT session cap reached. Try again tomorrow.",
                    provider: "deepgram",
                    tier: "demo",
                    usage: { used: usage.deepgram_tokens, cap: tier.caps.daily_deepgram_tokens },
                }),
            );
        }
    }

    const upstream = await fetch(DEEPGRAM_TOKEN_URL, {
        method: "POST",
        headers: {
            authorization: `Token ${env.DEEPGRAM_API_KEY}`,
            "content-type": "application/json",
        },
        body: JSON.stringify({ ttl_seconds: DEEPGRAM_TOKEN_TTL_SECONDS }),
    });

    if (!upstream.ok) {
        const body = await upstream.text();
        console.error(`[deepgram/token] upstream ${upstream.status}: ${body}`);
        return cors(
            new Response(body, {
                status: upstream.status,
                headers: { "content-type": "application/json" },
            }),
        );
    }

    const grant = (await upstream.json()) as { access_token: string; expires_in: number };

    if (tier.kind === "demo") {
        ctx.waitUntil(
            bumpCounter(env.USAGE_KV, demoUsageKey(tier.code, deviceId), "deepgram_tokens"),
        );
    }

    return cors(
        jsonResponse(200, {
            token: grant.access_token,
            expires_in: grant.expires_in,
        }),
    );
}

/**
 * Mints a short-lived Cartesia access token scoped to TTS use. Same pattern
 * as Deepgram: client uses the returned token directly against Cartesia's
 * WebSocket, Worker isn't on the data path.
 */
export async function handleCartesiaToken(
    request: Request,
    env: Env,
    ctx: ExecutionContext,
): Promise<Response> {
    const deviceId = requireDeviceId(request);
    if (deviceId instanceof Response) return deviceId;

    const tier = await resolveTier(request, env, deviceId);
    if (tier instanceof Response) return tier;

    if (tier.kind === "trial") {
        const consumed = await consumeTrialTurn(env.USAGE_KV, deviceId, tier.turnsCap);
        if (!consumed) {
            return cors(
                jsonResponse(429, {
                    error: "trial_exhausted",
                    message:
                        "Free trial spent. Use your own API keys (BYOK) or contact us for an invite code.",
                    provider: "cartesia",
                    tier: "trial",
                }),
            );
        }
    } else {
        const kvKey = demoUsageKey(tier.code, deviceId);
        const usage = await readUsage(env.USAGE_KV, kvKey);
        if (usage.cartesia_tokens >= tier.caps.daily_cartesia_tokens) {
            return cors(
                jsonResponse(429, {
                    error: "daily_cap_exceeded",
                    message: "Daily TTS session cap reached. Try again tomorrow.",
                    provider: "cartesia",
                    tier: "demo",
                    usage: { used: usage.cartesia_tokens, cap: tier.caps.daily_cartesia_tokens },
                }),
            );
        }
    }

    const upstream = await fetch(CARTESIA_TOKEN_URL, {
        method: "POST",
        headers: {
            authorization: `Bearer ${env.CARTESIA_API_KEY}`,
            "cartesia-version": CARTESIA_API_VERSION,
            "content-type": "application/json",
        },
        body: JSON.stringify({
            grants: { tts: true },
            expires_in: CARTESIA_TOKEN_TTL_SECONDS,
        }),
    });

    if (!upstream.ok) {
        const body = await upstream.text();
        console.error(`[cartesia/token] upstream ${upstream.status}: ${body}`);
        return cors(
            new Response(body, {
                status: upstream.status,
                headers: { "content-type": "application/json" },
            }),
        );
    }

    const grant = (await upstream.json()) as { token: string };

    if (tier.kind === "demo") {
        ctx.waitUntil(
            bumpCounter(env.USAGE_KV, demoUsageKey(tier.code, deviceId), "cartesia_tokens"),
        );
    }

    return cors(
        jsonResponse(200, {
            token: grant.token,
            expires_in: CARTESIA_TOKEN_TTL_SECONDS,
        }),
    );
}
