// Per-device usage metering backed by KV. Owns the key schema, the read/write
// of usage counters, and the SSE token tally for the Anthropic stream.

import { DEMO_KV_TTL_SECONDS, TRIAL_KV_TTL_SECONDS } from "./constants";
import type { TrialUsage, Usage } from "./types";

export function demoUsageKey(code: string, deviceId: string): string {
    return `usage:demo:${code}:${deviceId}:${utcDateKey(new Date())}`;
}

export function trialUsageKey(deviceId: string): string {
    return `usage:trial:${deviceId}`;
}

export function utcDateKey(d: Date): string {
    return d.toISOString().slice(0, 10);
}

export function parseCap(value: string | undefined, fallback: number): number {
    const n = parseInt(value ?? "", 10);
    return Number.isFinite(n) && n > 0 ? n : fallback;
}

export async function readUsage(kv: KVNamespace, key: string): Promise<Usage> {
    const raw = await kv.get(key);
    if (!raw) return emptyUsage();
    try {
        const parsed = JSON.parse(raw) as Partial<Usage>;
        return {
            input: typeof parsed.input === "number" ? parsed.input : 0,
            output: typeof parsed.output === "number" ? parsed.output : 0,
            deepgram_tokens:
                typeof parsed.deepgram_tokens === "number" ? parsed.deepgram_tokens : 0,
            cartesia_tokens:
                typeof parsed.cartesia_tokens === "number" ? parsed.cartesia_tokens : 0,
        };
    } catch {
        return emptyUsage();
    }
}

export async function readTrialUsage(kv: KVNamespace, key: string): Promise<TrialUsage> {
    const raw = await kv.get(key);
    if (!raw) return { turns: 0 };
    try {
        const parsed = JSON.parse(raw) as Partial<TrialUsage>;
        return { turns: typeof parsed.turns === "number" ? parsed.turns : 0 };
    } catch {
        return { turns: 0 };
    }
}

export function emptyUsage(): Usage {
    return { input: 0, output: 0, deepgram_tokens: 0, cartesia_tokens: 0 };
}

export async function bumpCounter(
    kv: KVNamespace,
    key: string,
    field: "deepgram_tokens" | "cartesia_tokens",
): Promise<void> {
    const existing = await readUsage(kv, key);
    existing[field] += 1;
    await kv.put(key, JSON.stringify(existing), { expirationTtl: DEMO_KV_TTL_SECONDS });
}

/**
 * Trial-tier read-modify-write: returns true and bumps the counter if the
 * device has turns left, false otherwise. Small race window where two
 * concurrent requests both succeed at the same cap boundary is acceptable.
 */
export async function consumeTrialTurn(
    kv: KVNamespace,
    deviceId: string,
    cap: number,
): Promise<boolean> {
    const key = trialUsageKey(deviceId);
    const existing = await readTrialUsage(kv, key);
    if (existing.turns >= cap) return false;
    existing.turns += 1;
    await kv.put(key, JSON.stringify(existing), { expirationTtl: TRIAL_KV_TTL_SECONDS });
    return true;
}

/**
 * Walk the Anthropic SSE response stream, sum input/output tokens, then add
 * them into the day's KV entry. Input tokens arrive on the `message_start`
 * event, the running output total on each `message_delta`.
 */
export async function tallyAnthropicUsage(
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
                    } else if (
                        obj.type === "message_delta" &&
                        obj.usage?.output_tokens != null
                    ) {
                        output = obj.usage.output_tokens;
                    }
                } catch {
                    // skip non-JSON / partial
                }
            }
        }
    } catch (err) {
        console.error("tally read error:", err);
    }

    if (input === 0 && output === 0) return;

    const existing = await readUsage(kv, kvKey);
    const total = {
        ...existing,
        input: existing.input + input,
        output: existing.output + output,
    };
    await kv.put(kvKey, JSON.stringify(total), { expirationTtl: DEMO_KV_TTL_SECONDS });
}
