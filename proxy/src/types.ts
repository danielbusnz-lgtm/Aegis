// Shared types for the aegis proxy Worker. The runtime bindings (Env) and the
// shapes persisted to KV and R2 live here so handlers and the metering layer
// agree on one definition.

export interface Env {
    /** Anthropic API key. Set via `wrangler secret put ANTHROPIC_API_KEY`. */
    ANTHROPIC_API_KEY: string;
    /** Deepgram API key. Set via `wrangler secret put DEEPGRAM_API_KEY`. */
    DEEPGRAM_API_KEY: string;
    /** Cartesia API key. Set via `wrangler secret put CARTESIA_API_KEY`. */
    CARTESIA_API_KEY: string;
    /**
     * Shared namespace for both usage counters and invite codes. Keys:
     *   usage:trial:{deviceId}                  -> TrialUsage (30d TTL)
     *   usage:demo:{code}:{deviceId}:{date}     -> Usage (48h TTL)
     *   invite:{CODE}                           -> InviteCode (no TTL, managed by hand)
     */
    USAGE_KV: KVNamespace;
    /**
     * Object store for routelet distillation samples. One object per sample:
     *   samples/{date}/{deviceId}/{ts}-{uuid}.json -> RouteletSample
     * Opt-in on the client; the bucket only ever sees redacted text.
     */
    ROUTELET_R2: R2Bucket;
    /** Lifetime turn cap for trial-tier devices. Decimal string. */
    TRIAL_TURNS_CAP: string;
    /** Daily caps for demo-tier devices when the invite code omits a field. */
    DAILY_INPUT_TOKEN_CAP: string;
    DAILY_OUTPUT_TOKEN_CAP: string;
    DAILY_DEEPGRAM_TOKEN_CAP: string;
    DAILY_CARTESIA_TOKEN_CAP: string;
}

export type Usage = {
    /** Anthropic input tokens consumed today. */
    input: number;
    /** Anthropic output tokens consumed today. */
    output: number;
    /** Deepgram tokens minted today (each gates one streaming session). */
    deepgram_tokens: number;
    /** Cartesia tokens minted today (each gates one or more TTS sessions). */
    cartesia_tokens: number;
};

export type TrialUsage = {
    /** Any-endpoint calls this device has made. Compared to TRIAL_TURNS_CAP. */
    turns: number;
};

export type DemoCaps = {
    daily_input_tokens: number;
    daily_output_tokens: number;
    daily_deepgram_tokens: number;
    daily_cartesia_tokens: number;
};

export type InviteCode = DemoCaps & {
    /** ISO 8601. Codes past this date are rejected. */
    expires_at: string;
    /** Hard ceiling on `devices_seen.length`. */
    max_devices: number;
    /** Device IDs that have used this code. Append-only. */
    devices_seen: string[];
};

export type Tier =
    | { kind: "trial"; turnsCap: number }
    | { kind: "demo"; code: string; caps: DemoCaps };

/** Successful read-only resolution of an invite code against KV. */
export type InviteLookup = {
    normalized: string;
    invite: InviteCode;
    /** Whether this device is already bound to the code. */
    deviceKnown: boolean;
    /** Whether the code has an unused device slot (ignoring `deviceKnown`). */
    hasRoom: boolean;
};

/** One distillation sample as stored in R2. */
export type RouteletSample = {
    /** Redacted on-device, scrubbed again here. Maps to the `text` field the routelet trainer reads. */
    text: string;
    /** What routelet predicted on-device, or null if it abstained. */
    routelet_pred: string | null;
    /** Routelet softmax confidence in [0,1], or null when it abstained. */
    routelet_conf: number | null;
    /**
     * Ground-truth label. Reserved for the server-attached Claude label once
     * the fallback path feeds this endpoint; null until then.
     */
    claude_label: string | null;
    /** The `x-aegis-device-id` that produced the sample. */
    device: string;
    /** Unix seconds, server clock (not the client's). */
    ts: number;
};
