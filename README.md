# HP Prime Development Toolkit

[![CI](https://github.com/golba98/hp-prime-development-toolkit/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/golba98/hp-prime-development-toolkit/actions/workflows/ci.yml) [![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE) [![Status: unreleased](https://img.shields.io/badge/status-unreleased-orange.svg)](docs/packaging.md)

**Write PPL on your computer. Check it, run it, and see what it draws.**

`hpdev` brings HP Prime PPL development to your terminal: compiler checks,
execution, a stepping debugger, repeatable tests, and live display previews.
It is a standalone Rust application built on the native engine from
[HP Prime Emulator](https://github.com/golba98/hp-prime-emulator), with no GUI,
HP firmware, or connected calculator required.

[Install](#install) · [Quick start](#quick-start) · [Commands](#commands) · [Documentation](#documentation) · [Compatibility](#compatibility-and-troubleshooting)

| Develop | Inspect | Automate |
| --- | --- | --- |
| Check and run `.ppl` / `.hpprgm` files | Step through code and inspect variables | Assert results and compare image snapshots |
| Preview the 320×240 display in a terminal or browser | Export PNGs and execution traces | Watch files and consume JSON reports |

<p align="center">
  <img src="tests/snapshots/DRAW.png" width="320" height="240" alt="Native 320 by 240 framebuffer output from the DRAW example">
  <br>
  <sub>Actual framebuffer output from the included DRAW example.</sub>
</p>

The toolkit is an independent, unofficial project, not affiliated with or
endorsed by HP. HP and HP Prime are trademarks of their respective owners.

## Install

### Build from source

Public npm packages and native releases have **not been published**. You can
install from this checkout now (tested with Rust 1.96):

```sh
git clone https://github.com/golba98/hp-prime-development-toolkit.git
cd hp-prime-development-toolkit
cargo install --locked --path crates/hpdev-cli
hpdev --help
```

The core needs no GUI, HID or Unicorn build dependencies. CI checks Linux
x86_64/ARM64, Windows x86_64, and macOS ARM64/x86_64. All five platform checks
must pass before a release; the CI badge above shows the current status.
Linux release packaging targets musl for portability.

### npm and native downloads — forthcoming

After the first public release, the installation command will be:

```sh
npm install -g hp-prime-devkit
hpdev --version
```

The npm launcher installs an exact-version prebuilt executable for your platform;
Rust is not required. Keep npm optional dependencies enabled. Standalone archives
will also be available from
[GitHub Releases](https://github.com/golba98/hp-prime-development-toolkit/releases)
for installation without Node or Rust. See [packaging](docs/packaging.md) for
local packages and the release process.

## Quick start

From the repository root:

```sh
hpdev check examples/HELLO.ppl
hpdev run examples/HELLO.ppl
hpdev run examples/SQUARE.ppl --args '[7]' --json
hpdev debug examples/DEBUG.ppl
hpdev preview examples/DRAW.hpprgm --png screen.png
hpdev preview examples/LIVE.hpprgm --tui
hpdev preview examples/INPUT.ppl --web
hpdev watch examples/DRAW.hpprgm --tui
hpdev test
hpdev doctor --json
```

Packaged npm installations include the `examples/` directory inside
`hp-prime-devkit`; copy those files into your project before using relative
example paths. Plain `.ppl`, textual `.hpprgm`, UTF-8/UTF-16 sources and the
upstream loader's supported binary program containers use the same engine.

## Commands

| Command | Behavior |
| --- | --- |
| `check FILE` | Parse and compile without running initializers or functions |
| `run FILE` | Execute a selected native PPL entry point |
| `debug FILE` | Actual statement stepping, breakpoints and variable/frame inspection |
| `preview FILE` | Real framebuffer in terminal, native PNG, or local browser |
| `watch FILE` | Debounced check/run, cancelling the previous execution before rerunning |
| `test [MANIFEST]` | Assert program results and image references; defaults to `hpdev.toml` |
| `trace FILE` | Executed statement/stack observations and presentation events |
| `doctor` | Installation, native renderer, platform and terminal capabilities |

`--json`, `--quiet` and `--report PATH` work across commands. JSON writes one
complete result to stdout; unrelated messages and terminal UI use stderr.
Quiet suppresses summaries, not program PRINT output or error diagnostics.
Reports and screenshots cannot overwrite source files through ordinary paths,
symlinks or hard links. Files are written atomically.

Use `hpdev COMMAND --help` for all options. Common execution options include:

```sh
hpdev run program.hpprgm --entry MAIN --args '[7,"text",[1,2]]'
hpdev run program.ppl --args-format ppl --args '7,{1,2}'
hpdev run program.ppl --keys '-1*30,4' --input 7 --choose 2 --seed 24301
hpdev run program.ppl --interactive
hpdev run program.ppl --max-time 5 --max-steps 1000000 --max-depth 100
hpdev check program.ppl --deny-warnings --deny-compatibility
hpdev run root.ppl --module LIB=library.ppl
```

JSON arguments default to an array of arguments. Arrays within it are PPL
lists; tagged arguments represent matrices, complex numbers, calculator
integers and decimal values without binary floating-point conversion. For
example, `--args '[{"type":"real","value":"1e400"}]'` preserves the exponent.
The upstream engine's PPL expression argument syntax is available explicitly.

Native execution is always the default, even when `PPL_ENGINE=firmware` or
firmware configuration exists. `--engine firmware` reports unsupported;
there is no automatic backend fallback or hardware control command.

## Input and execution limits

Ordinary runs are deterministic and scripted. `--input TEXT` and `--choose N`
are repeatable; `<ESC>` cancels INPUT and choice 0 cancels CHOOSE. The upstream
`--keys` syntax accepts key codes 0–50, `-1` polls and `touch:X:Y` events.
An exhausted script is a failure, not a successful execution or timeout.

`run --interactive` enables terminal prompts; TUI and browser previews enable
live input automatically. Line-mode prompts accept one field at a time;
outside dialogs use `key 30`, `touch 120 40`, Enter, or `quit`. Interactive
mode uses wall time; scripts use the upstream virtual clock and fixed date.
Scripted messages acknowledge automatically. Interactive messages require OK.

Default protections are 30 seconds total, five million VM instructions, depth
1000, collection size one million, string size 16 MiB, and PRINT/dialog output
4 MiB. The outer deadline includes compilation, blocking input and waiting for
the first browser connection. `--no-time-limit` explicitly disables it.
Debugger sessions have no wall deadline by default; a requested debugger
deadline includes time at the prompt. Instruction checks follow the upstream
VM's batching and can overshoot the requested budget before the next check.

Source loading is bounded to 8 MiB per file and key scripts to one million
events. Debug/trace observations are bounded to 10,000 events and 4 MiB.
VM collection/string limits are not an aggregate process heap limit.

Every run executes in an isolated instance of the same binary, directly using
the Rust VM. A supervisor cancels blocked input and long native operations;
after a 500 ms grace period it terminates and reaps the worker. This is process
isolation for development, not a general-purpose OS security sandbox.

## Debugger

`debug` selects the TUI on capable terminals. Use `--no-tui` for line commands
and scripts, or `--tui` to request it explicitly. JSON defaults to command mode.

```sh
printf 'p X\nn\np Y\nbt\nout\np B\nc\n' |
  hpdev debug examples/DEBUG.ppl --no-tui --break 5 --run --json
```

| TUI key | Action |
| --- | --- |
| `s` / F11, `n` / F10, `o` | Step into, step over, step out |
| `c` / F5 | Continue |
| `b`, `x` | Add/remove a breakpoint at the displayed line |
| `u`, `d` | Select caller/callee frame |
| `:` | Enter a debugger command |
| Tab, PgUp/PgDn | Compact-layout panels; scroll variable inspection |
| `q`, Ctrl-C | Terminate |

Line commands: `step`, `next`, `out`, `continue`, `break [LINE]`,
`delete [LINE]`, `locals`, `globals`, `print NAME`, `stack`, `up`, `down`,
`frame N`, `list`, `status`, `help`, `quit`. EOF cancels. Breakpoint numbers
refer to the root source. Variables are read from real paused VM frames.
While running, calculator keys reach the program; debugger commands apply while paused.
Runtime failures retain the engine's source stack; the VM unwinds failed
frames, so post-error locals at the failing instruction are not available.

## Display previews

`preview --tui` renders real 320×240 framebuffer pixels using Unicode
half-blocks. It fits a 4:3 display using available terminal cell dimensions,
with a 0.5 width/height fallback. Override with `--cell-aspect`. Color modes
are `--color auto|truecolor|256|mono`; auto respects `NO_COLOR`.
Cursor-incapable terminals receive a clear error and can use PNG or web mode.

Frames come from real `Host::present` calls, including successive redraws,
WAIT and FREEZE. An ordered queue is bounded to eight events; visual refresh
is limited to 30 FPS with backpressure. Fast drawing between native
presentation events is not invented into additional frames. PRINT mode is
rasterized by the upstream engine's terminal renderer.

Arrows, Enter, Escape, digits and arithmetic keys target the local calculator.
Escape is key 4, not a toolkit stop command. Ctrl-C stops execution. In dialogs,
Tab/Up/Down selects a field, Enter accepts, F2 restores an explicit reset value
and Escape cancels. Click inside the display for touch coordinates. Terminals
without release events approximate a held key with a short pulse.

`--png PATH` exports native resolution and can accompany a live viewer. A
runtime failure remains a failure even if a partial screenshot was saved.
Completed TUI/web previews stay visible until Enter/q or Stop/Ctrl-C; use
`--exit-on-finish` for automated sessions.

The browser viewer binds only to `127.0.0.1`, uses an ephemeral port by default,
and checks its per-session token and Origin/Host. It has no source-loading or
execution endpoint. `--no-open` prints a URL without launching a browser;
`--port N` selects a port. Execution waits for the first viewer connection so
short programs have a viewer for their initial frames. Use the display's
keyboard focus, on-screen keys, touch, dialog controls and Stop button.
A slow browser that falls behind the bounded stream reconnects to resynchronize.
Closing the last viewer cancels execution after a two-second reconnect grace period.

Dialogs use toolkit controls beside/over the framebuffer. They preserve the
native dialog model, validation and writeback; their terminal/browser chrome
does not claim pixel parity with HP calculator dialogs.

## Watch, assertions and tracing

`watch FILE` checks and executes each stable revision. `--check-only` validates
without execution; `--tui` or `--web` keeps a live preview. Parent-directory
polling handles atomic saves, with 200 ms debounce and stable reads. A newer
revision cancels and reaps its predecessor; runs do not overlap. Temporary
missing or incomplete files remain recoverable. Ctrl-C stops watching.
JSON emits one final summary; `--report PATH` atomically updates the latest
run while watching.

See [test manifests](docs/testing.md) for assertions and explicit image
tolerances. `hpdev test --update-snapshots` intentionally creates/replaces
references; ordinary test runs never accept missing references automatically.
The included root manifest exercises five real workflows.

`trace --variables` adds sampled variable differences to statement and stack
observations. These reflect values at statement boundaries, not every
bytecode write. Empty calls without statement callbacks may not be observed.
Graphics presentation events are recorded; graphics-command tracing is not
implemented. Normal execution does not attach a tracing debugger.

## Compatibility and troubleshooting

This reuses the native engine; it does not provide complete firmware parity.
CAS, built-in calculator apps and unsupported command variants report
compatibility limitations. Native text uses DejaVu Sans with approximate
metrics. Some Cartesian graphics, menu layout, transparency and dialog
controls differ from firmware. No physical-device parity is claimed.

- Missing npm binary: enable optional dependencies and reinstall the exact
  version. Unsupported platforms can build from source.
- No terminal preview: use a UTF-8 terminal with cursor control, `--png`, or
  `--web`. Resize or adjust `--cell-aspect` if the display looks stretched.
- Input exhausted: supply scripts or opt into interaction.
- Browser waiting: open the printed token URL before its deadline; it must
  reach the same machine's loopback interface.
- Timeout: inspect loops and input, or explicitly raise the deadline/budget.

## Documentation

| Guide | What you will find |
| --- | --- |
| [Architecture](docs/architecture.md) | Native engine, worker isolation, and viewers |
| [JSON API](docs/json-api.md) | Structured reports and automation interfaces |
| [Testing](docs/testing.md) | Test manifests, assertions, and image snapshots |
| [Packaging and releases](docs/packaging.md) | Local packages, release dry runs, and publishing setup |

## Development and license

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --release --bin hpdev
```

Upstream crates use an exact Git commit and a checked-in lockfile. The
original emulator remains independent and is not modified. This repository's
source is MIT licensed; [third-party notices](THIRD_PARTY_NOTICES.md) retain
the emulator and font licenses. No proprietary HP firmware or calculator
assets are distributed. Do not use physical calculators in automated tests.
