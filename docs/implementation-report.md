# Initial implementation and verification report

## Project and architecture

The standalone project is the `4-HP Prime Development Toolkit` checkout,
separate from the sibling HP Prime Emulator repository. It contains a two-crate
Rust workspace: `hpdev-core` for engine adapters and developer services, and
`hpdev-cli` for the `hpdev` executable, supervision and viewers. `npm/` and
`scripts/` prepare native distribution; `docs/`, examples, fixtures, integration
tests and CI are included. The architecture is described in [architecture.md](architecture.md).

All engine dependencies are pinned to emulator commit
`8d7535ec432e3c66f4f210f32a4ae58c5911d96d` with a checked-in lockfile.

| Reused crate/API | Toolkit use |
| --- | --- |
| `ppl-math` (through compiler/VM) | Numeric precision, arithmetic and formatting |
| `ppl-syntax`, engine `Source` | Encodings, program containers, source identity, spans and diagnostics |
| `ppl-compiler::compile_source` | Existing semantic checks, warnings and bytecode |
| `ppl-vm::Machine`, `link_sources` | Real native execution and explicit module catalogs |
| `Host`, `HeadlessHost`, dialog models | Live/scripted input, clock, output and framebuffer presentation |
| `Debugger`, frame/variable inspection | Actual statement stepping and runtime values |
| VM `Limits` and runtime errors | Instruction, allocation, depth/output protection and source stacks |
| `ppl-graphics::Grob` and text renderer | Native framebuffer, PRINT rasterization and PNG output |
| `ppl-engine::arguments` | Lossless arguments and tagged results |
| `ppl-compat::compare_images` | Existing exact graphics comparison |

There is no duplicate parser, compiler, VM or drawing engine. The old `ppl`
binary is not spawned. A supervised instance of `hpdev` directly calls Rust
APIs so the parent can terminate a long native operation or blocked input.
No source changes to the emulator were necessary or made by this task.

## Implemented behavior

All eight commands are implemented: check, run, debug, preview, watch, test,
trace and doctor. Native execution is unconditional by default; firmware is
explicitly unsupported. JSON results, source classes/codes/locations, lossless
returns, output capture, stack traces, seeds and scripted input are integrated.

The Ratatui debugger has source, actual locals/globals, stack/frame navigation,
breakpoints, step/next/out/continue and display integration, plus a line-command
mode. Running debugger sessions route calculator keys to the host. EOF and
interrupts terminate cleanly.

Preview supports real successive framebuffer presentations through terminal
half-blocks, native-resolution PNGs and an authenticated loopback browser
viewer. Both interactive viewers support implemented dialogs and keyboard
input; browser/terminal touch is supported. Frames use bounded queues and
refresh rates, not fake animation. Closing the last browser cancels after a
reconnect grace period.

Watch debounces saves and reaps the preceding worker before starting another.
Program assertions cover output, values, diagnostics, failures and exact or
explicitly tolerated image snapshots, with retained visual differences.
Trace observes native statements, stacks, optional sampled variables and
presentations. Doctor requires no firmware or calculator.

Packaging includes static Linux binaries, checksums, npm command shims,
exact-version optional platform packages, collected license notices, guides,
working examples and an example test manifest. Nothing was published.

## Installation and examples

Build or install from the source checkout:

```sh
cargo build --locked --release --bin hpdev
cargo install --locked --path crates/hpdev-cli
```

The prepared static binary is
`target/x86_64-unknown-linux-musl/release/hpdev`. Native distribution and npm
tarballs are under `dist/`. Locally install the assembled npm packages with:

```sh
npm install -g --offline \
  ./dist/tarballs/hp-prime-devkit-linux-x64-0.1.0.tgz \
  ./dist/tarballs/hp-prime-devkit-0.1.0.tgz
```

Examples, from the checkout or extracted native package directory:

```sh
hpdev check examples/HELLO.ppl
hpdev run examples/SQUARE.ppl --args '[7]' --json
hpdev debug examples/DEBUG.ppl --break 5 --run
hpdev preview examples/DRAW.hpprgm --png screen.png
hpdev preview examples/LIVE.hpprgm --tui
hpdev preview examples/INPUT.ppl --web
hpdev watch examples/DRAW.hpprgm --tui
hpdev test
```

Public `npm install -g hp-prime-devkit` becomes available only after an
authorized publication. No public npm package or GitHub release was created.

## Stabilization findings

Review fixes cover source/report aliases including missing input paths, correct
cancellation during blocked native operations and late control messages,
terminal color settings, debugger key routing, immediate watch/viewer compiler
diagnostics, removal of temporary screenshot paths from public reports, and
readable package assets after a system-wide installation. Both optional-feature
configurations pass strict linting. No source implementation placeholders remain.

## Executed verification

| Verification | Passed | Failed |
| --- | ---: | ---: |
| Toolkit Rust unit/integration/CLI/PTY/browser-transport tests | 38 | 0 |
| Included PPL assertion/snapshot cases | 5 | 0 |
| Headless Chromium tests against the static release binary | 4 | 0 |
| Isolated offline npm installation/launch/integrity checks | 4 | 0 |
| Extracted native archive version and example-suite smoke checks | 2 | 0 |
| Clean pinned emulator core/compatibility tests | 469 | 0 |
| Clean pinned emulator debugger/engine/module CLI tests | 28 | 0 |

`cargo fmt --all -- --check` and Clippy with warnings denied passed, including
the build without browser support. The musl release build passed. Windows GNU
cross-checking passed; this is a compile check, not Windows runtime testing.
The clean pinned emulator's complete workspace, including its GUI and firmware
implementation crates, builds successfully. No HP firmware or physical
calculator execution was used for verification.

The debugger exposed X=3 at the real breakpoint and Y=6 after stepping. SQUARE
returned 49. INPUT/CHOOSE returned 9. PTY and Chromium tests observed all five
distinct native animation colors before completion. Native PNG dimensions and
pixels, partial screenshots on failures, source diagnostics, structured errors,
watch replacement, instruction/output limits and cancellation were exercised.
A long graphics operation was stopped by the outer deadline despite not
reaching another VM limit check.

The npm test used a disposable global prefix and empty npm cache with offline
installation, then ran outside both repositories with Cargo absent from PATH.
It checked the installed command shim and rejected a corrupted native binary
with a structured JSON failure. Native archive extraction was separately
tested outside the checkout. Package fixture/notice permissions are normalized
so system-wide installations remain readable.

During implementation the canonical emulator checkout acquired substantial
concurrent, uncommitted CAS/GUI changes. A targeted regression attempt against
that evolving checkout failed to compile `Rc<CasValue>` serialization in
`ppl-engine/src/arguments.rs`; no tests ran in that failed attempt. Those
changes were left untouched. The successful upstream counts above refer to
the independently tested clean pinned commit, not the concurrent working tree.
That working tree's ongoing state needs its owner's validation.

## Platforms, limits and readiness

Linux x86_64 native execution, terminal interaction, browser operation, static
musl distribution and npm installation are verified. Windows GNU compilation
is verified. Windows x86_64 MSVC, macOS ARM64/x86_64 and Linux ARM64 runtime
support remain unverified locally; the five-platform CI matrix is prepared
and awaits confirmation from its remote runs; the results above are local checks.

The native engine does not provide full CAS, calculator-app or firmware parity.
Fonts and some graphics/dialog behavior are approximate. Dialogs use toolkit
controls rather than firmware pixel layouts; field positions and custom matrix
headers are not reproduced. Terminal key holding uses pulses when release
events are unavailable. Breakpoint numbers target the root source; failed
module links retain the upstream error without inventing a structured span.
Failed VM frames unwind, so post-error locals at the failing instruction are
unavailable. Variable traces sample statements, and graphics-command tracing
is not implemented. Memory limits are per collection/string, not a total heap
quota. These limits are documented rather than represented as complete parity.

This is a working Linux release candidate with installable local artifacts.
Broader publication awaits the remaining platform CI and maintainer release
review. The concurrent emulator work requires separate stabilization; it is
not consumed by the toolkit's pinned build. This report records local verification before pull-request submission. No
package publications or public releases were made.
