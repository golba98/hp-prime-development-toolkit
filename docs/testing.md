# Program assertions and graphics snapshots

`hpdev test` loads `hpdev.toml`; an explicit manifest path is also supported.
Each case starts with a fresh native machine and deterministic scripted host.
Paths are relative to the manifest directory. Unknown manifest fields fail.

```toml
schema_version = 1

[[test]]
name = "square"
file = "SQUARE.ppl"
entry = "SQUARE"
args = "[7]"
seed = 24301
max_time = 5
max_steps = 100000
[test.expect]
status = "success"
stdout = []
return_value = { type = "real", value = "49" }
diagnostic_codes = []

[[test]]
name = "drawing"
file = "DRAW.hpprgm"
[test.expect]
png = "snapshots/DRAW.png"
# Optional explicit tolerance; exact RGB comparison is the default.
channel_tolerance = 1
max_changed_pixels = 0
```

`input = ["7", "<ESC>"]`, `choose = [2]` and `keys = "-1*10,4"` supply
scripts. `operation = "check"` checks diagnostics without executing. The
expected status defaults to success; omitted output/value/diagnostic
assertions are not checked. Supplied diagnostic codes match in order.

Image dimensions must match exactly. A pixel differs when any RGB channel
exceeds `channel_tolerance`; at most `max_changed_pixels` may differ. Both
defaults are zero. There are no implicit masks or perceptual similarity rules.
The exact comparison result remains in the report even when a configured
tolerance permits the image.

Mismatches retain actual and magenta-highlighted difference PNGs in
`.hpdev/test` or `--artifacts-dir PATH`. Reports include mismatch counts and
the upstream exact comparison. Missing references fail. Explicit
`--update-snapshots` writes references from actual execution; inspect them
before committing. References, manifests and sources are protected from
ordinary artifact writes.

Rust tests cover compiler integration, real containers/encodings, lossless
values, actual debugger inspection, resource stops, CLI JSON and errors,
image differences and watch restarts. Unix PTY tests exercise raw terminal
input, dialogs and all five native colors in the animation fixture. Browser
transport tests exercise ordered pixels, dialogs, Stop and origin/token
validation. These tests never open USB or execute HP firmware.
