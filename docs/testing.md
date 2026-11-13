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

## Reproducing distribution checks

Use the checks in [packaging](packaging.md) against the pinned upstream revision.
A compile-only cross-check is not runtime validation. The five native CI platforms
must pass independently before release. Offline npm checks use an isolated prefix
and cache, run outside the checkout, and reject a corrupted executable. Native
archive smoke checks run without Node or Rust on PATH. Local results do not
establish runtime support on other platforms or validate a modified upstream tree.


## Native desktop integration

On Linux with Xvfb and X11/XKB runtime libraries installed:

```sh
xvfb-run -a -s "-screen 0 1600x1200x24" cargo test --locked -p hpdev-cli --test desktop -- --ignored --test-threads=1
```

These explicitly selected tests fail if the display is unavailable. Ordinary
workspace tests leave them ignored so headless development needs no display;
the required desktop CI job runs them. They discover mapped windows, capture
actual X11 pixels and inject OS keyboard/pointer events using XTest.
The real BST fixture exercises insertion, deletion, clearing, calculator keys,
dialogs, pause, stepping and inspection. No browser, device, mock window or
predefined calculator input script is used.

`hpdev run tests/fixtures/BSTVisualizer.hpprgm` exercises the desktop backend
selected by the current Linux session, including Wayland. Xvfb results establish
X11 behavior; cross-platform builds do not establish interaction fidelity on
Wayland, Windows or macOS.


## Windows, macOS and shared native acceptance

Build the CLI, then run the shared suite in a dedicated logged-in desktop session:

```sh
python3 -B tests/desktop/run.py --binary target/debug/hpdev --artifacts test-results/native
```

On Windows use `python` and `target/debug/hpdev.exe`. The driver uses Win32
window enumeration, client pixels and SendInput. macOS uses Quartz window capture
and events plus Accessibility lifecycle actions. Python and its launching
terminal/runner need Screen Recording and Accessibility permission for these
tests; ordinary hpdev users do not need those automation permissions.
The suite never grants permissions, edits TCC databases or substitutes synthetic
frames. Unavailable access fails the job and is recorded as unavailable,
rather than passing or being labelled runtime-tested.

Tests exercise the real BST fixture (insert, delete, clear, scroll), GETKEY,
dialog validation/writeback, modifiers, focus loss, keypad, debugger inspection
and stepping, Unicode paths, repeated launches, resizing, minimize/restore,
closing a blocked VM, and runtime-error diagnostics. Uploaded evidence includes
only this test output and captures of the tested application's window.
`status.json` records the observed DPI; a standard-DPI result is not evidence
of a real Retina or Windows high-DPI session. The Linux job also runs the full suite with a forced 2× X11 scale and records
that fact; this tests real native-window rendering/input at that scale, without
claiming physical Retina or Windows high-DPI coverage. Pure Rust geometry tests
separately cover 100%, 125%, 150%, 200% and 300% scaling.

The hosted matrix uses Ubuntu, Windows Server 2022/2025 and both macOS 15
architectures. Server results are not Windows 10/11 client-version results.
`Native desktop session validation` is a manual workflow for dedicated
self-hosted Windows 10/11 and Retina Macs with preconfigured GUI permissions.
Select its JSON runner labels and Python executable; it executes the same suite
and uploads the real environment/result. No native session result should be
claimed before that workflow actually runs.

`python3 -B tests/packaging/test_native_zip.py` tests Windows ZIP round trips, Unicode
names, traversal, symlink and case-collision rejection. The packaging job
inspects actual executable linkage and then tests extracted CLI/JSON operation
outside the checkout. Linux retains its original Rust/XTest suite as well.
