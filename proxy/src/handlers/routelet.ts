import { SAMPLE_MAX_TEXT_CHARS } from "../constants";
import { cors, jsonResponse, requireDeviceId, scrubText, validLabelOrNull } from "../http";
import type { Env, RouteletSample } from "../types";
import { utcDateKey } from "../usage";

/**
 * Store one redacted classification sample in R2 for offline routelet
 * distillation. No usage metering: this is opt-in telemetry, not a billed
 * provider call. The client redacts before sending; we scrub again and reject
 * anything that fails validation rather than storing it dirty.
 */
export async function handleRouteletSample(request: Request, env: Env): Promise<Response> {
    const deviceId = requireDeviceId(request);
    if (deviceId instanceof Response) return deviceId;

    let body: unknown;
    try {
        body = await request.json();
    } catch {
        return cors(jsonResponse(400, { error: "request body must be JSON" }));
    }
    if (typeof body !== "object" || body === null) {
        return cors(jsonResponse(400, { error: "body must be an object" }));
    }
    const b = body as Record<string, unknown>;

    const rawText = typeof b.text === "string" ? b.text : "";
    const text = scrubText(rawText).slice(0, SAMPLE_MAX_TEXT_CHARS).trim();
    if (text.length === 0) {
        return cors(jsonResponse(400, { error: "text required" }));
    }

    const routeletPred = validLabelOrNull(b.routelet_pred);
    if (routeletPred instanceof Response) return routeletPred;
    const claudeLabel = validLabelOrNull(b.claude_label);
    if (claudeLabel instanceof Response) return claudeLabel;

    let routeletConf: number | null = null;
    if (typeof b.routelet_conf === "number") {
        if (b.routelet_conf < 0 || b.routelet_conf > 1) {
            return cors(jsonResponse(400, { error: "routelet_conf must be in [0,1]" }));
        }
        routeletConf = b.routelet_conf;
    }

    const ts = Math.floor(Date.now() / 1000);
    const sample: RouteletSample = {
        text,
        routelet_pred: routeletPred,
        routelet_conf: routeletConf,
        claude_label: claudeLabel,
        device: deviceId,
        ts,
    };

    // One immutable object per sample. R2 has no append, and a key per sample
    // sidesteps read-modify-write races across concurrent devices. Compaction
    // into batched JSONL happens offline at pull time.
    const key = `samples/${utcDateKey(new Date())}/${deviceId}/${ts}-${crypto.randomUUID()}.json`;
    try {
        await env.ROUTELET_R2.put(key, JSON.stringify(sample), {
            httpMetadata: { contentType: "application/json" },
        });
    } catch (e) {
        console.error(`[routelet/sample] R2 put failed: ${e}`);
        return cors(jsonResponse(502, { error: "storage write failed" }));
    }

    return cors(jsonResponse(200, { ok: true }));
}
