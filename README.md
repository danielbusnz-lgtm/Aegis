# Aegis

Voice-controlled AI cursor. Hold INSERT, ask a question, the cursor flies to it.

Built in Rust. Linux/Hyprland natively; Windows/macOS via the `winit-window` feature.

## Run it

```bash
git clone https://github.com/danielbusnz-lgtm/aegis.git
cd aegis
cargo run --release --bin aegis
```

All API calls route through a hosted Cloudflare Worker, so no keys needed locally.

On Hyprland, add the hotkey to `~/.config/hypr/hyprland.conf`:

```conf
bind  = , insert, exec, pkill -SIGUSR1 -f "target/(debug|release)/(aegis|test_)"
bindr = , insert, exec, pkill -SIGUSR2 -f "target/(debug|release)/(aegis|test_)"
```

`hyprctl reload`. Then hold INSERT, ask something, release.

## Windows / macOS / X11

```bash
cargo run --release --bin aegis --no-default-features --features winit-window,crossplatform
```

## Use your own API keys

To bypass the proxy and call the providers directly, drop a `.env`:

```
ANTHROPIC_API_KEY=sk-ant-...
DEEPGRAM_API_KEY=...
CARTESIA_API_KEY=...
AEGIS_ANTHROPIC_DIRECT=1
AEGIS_DEEPGRAM_DIRECT=1
AEGIS_CARTESIA_DIRECT=1
```

Each `_DIRECT=1` opts that provider out of the proxy. Mix and match.

## License

MIT
