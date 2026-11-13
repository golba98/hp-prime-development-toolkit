# Architecture and development

## Repository layout

The root contains the Cargo workspace and Node development manifests, the default
`hpdev.toml` example suite, project entry documentation and licence notices.
Keep generated builds, installed dependencies and test output in the existing
ignored `target/`, `dist/`, `node_modules/` and `test-results/` directories.

| Location | Contents |
| --- | --- |
| `crates/hpdev-core/` | Native execution library and Rust integration tests |
| `crates/hpdev-cli/` | CLI, native/terminal/browser frontends and their Rust tests |
| `crates/hpdev-cli/assets/` | Browser viewer and embedded Windows manifest |
| `npm/` | Distributable Node launcher and package metadata |
| `examples/` | Runnable PPL programs |
| `tests/browser/` | Playwright configuration and browser interaction tests |
| `tests/desktop/` | Native OS interaction drivers and acceptance runner |
| `tests/packaging/` | Release, ZIP and offline installation tests; pinned release fixture |
| `tests/fixtures/`, `tests/snapshots/` | Shared program fixtures, provenance and image references |
| `scripts/release/` | Packaging, licence collection, verification and release entry point (`index.mjs`) |
| `scripts/ci/` | CI command-output capture utility |
| `scripts/` | Stable documentation/publication guard entry points, policy and guard tests |
| `docs/` | User/contributor guides, report schema and dependency licences |

Run development commands from the repository root. Rust tests stay alongside
their owning crates. Browser, desktop and packaging suites share `tests/`;
distributed examples retain only the program fixtures and snapshots they need.

## Execution and presentation

`hpdev-cli` owns Clap commands, supervised worker processes, Ratatui/Crossterm
presentation, a default native Winit/Softbuffer desktop viewer, and the optional Axum browser viewer. `hpdev-core` owns request
and report types, compiler/VM orchestration, host adaptation, debugger controls
and test assertions. No parser, compiler, arithmetic implementation, VM or
graphics engine is duplicated.

The dependency revision is
`8d7535ec432e3c66f4f210f32a4ae58c5911d96d` of HP Prime Emulator.
All upstream crates resolve from the same Git source. `ppl-cli`, `ppl-gui`,
`ppl-device` and `prime-emulator` are not toolkit dependencies.

The existing `ppl-engine::native::execute` service requires `HeadlessHost` and
retains only its final framebuffer. The toolkit therefore reuses engine
source decoding/identity and argument conversion while compiling with
`ppl_compiler::compile_source` and driving `ppl_vm::Machine` directly.
`link_sources` handles explicit modules. This is orchestration of upstream
APIs, not a new execution engine. It also preserves diagnostic notes/classes
and provides toolkit-specific exit codes rather than inheriting legacy CLI
classification of exhausted input as timeout.

Each public operation launches a private `hpdev __worker` process. The worker
constructs its own non-Send VM and values; only serialized observations cross
the process boundary. A bounded reader channel and OS pipes apply backpressure
to presentation events. Output on the private pipe is framed, bounded JSON;
public stdout contains a single `Report` document when requested.

The host delegates scripted keys/dialogs/virtual clock to `HeadlessHost`.
Interactive responses enter through one bounded control channel, with an
atomic interruption flag. Blocking waits poll interruption. The parent also
enforces an outer deadline because upstream VM timing excludes blocking host
calls and cannot preempt an individual long graphics/math operation.

`Debugger::on_statement` receives the actual immutable `Machine`. Commands
query `locals`, `globals_for_frame` and `call_stack_frames` while that frame
exists. Step-over/out compare actual depths. Error traces come from
`RuntimeError`; no values or stepping are synthesized.

Desktop, PNG, terminal and browser rendering share native G0 pixels or the upstream
PRINT rasterization. Dialog controls are a separate host presentation layer.
The browser accepts only authenticated loopback input/control, never programs
or filesystem paths. Its viewer protocol is private and may change; the
public JSON result schema is versioned separately.

For local upstream development, use an **untracked developer Cargo patch**
listing each used `ppl-*` package under
`[patch."https://github.com/golba98/hp-prime-emulator"]` with its sibling crate
path. Do not check local patches or machine paths into the distributable
manifest/lockfile. Recheck the pinned build before preparing artifacts.

Contributions must preserve the emulator checkout and its ownership claims.
No commits, pushes, PRs or releases are performed by the toolkit's build
scripts. Updating the pin requires the relevant upstream and toolkit
regression tests, including diagnostics, debugger and framebuffer sequences.

The upstream linker returns text rather than structured diagnostics on failed
module linking. Those failures carry a `module-link` diagnostic with no
invented source span; successful linked modules retain their normal typed
diagnostics and source identities.

Dialog field positions and custom matrix headers are not reproduced by toolkit
controls. This is an explicit native-host limitation, not firmware pixel parity.


The desktop main thread owns the native window and software surface. Once the
surface is initialized, a background supervisor uses the same isolated worker,
bounded event stream, deadlines and cancellation path as other views.
Desktop input crosses the private control protocol; idle GETKEY returns -1.
Persistent pointer contacts coexist with legacy one-shot touches; focus loss
releases held calculator keys.

The desktop adapter reuses the pinned emulator's scaler, key mapping and modal
dialogs under `crates/hpdev-cli/src/desktop/upstream`. `provenance.json` records
their revision and hashes. Rust module paths are adapted; no local Cargo patch
is needed. The whole upstream GUI crate is not linked because it also includes
device and IDE layers. The toolkit retains process supervision and its statement
debugger. Desktop runs attach that debugger in continue mode without tracing.
Pause stops at the next statement callback; blocked host calls must return
before a new snapshot is possible.


Desktop platform policy is isolated in `desktop/platform.rs»: AppKit activation
runs on the main thread, macOS Command accelerators bypass calculator mappings,
and layout converts logical control dimensions to physical display pixels.
The VM, dialogs, debugger, worker protocol and execution limits remain shared.
Occluded/minimized windows continue pumping controls; cancellation and completion
do not depend on receiving another paint callback. The Windows executable keeps
its calling terminal, while its private worker uses CREATE_NO_WINDOW.

Windows MSVC builds embed a per-monitor-v2/asInvoker manifest and statically
link the MSVC runtime. Native binary inspection checks these properties.
macOS builds target a common deployment baseline and inspect Mach-O architecture,
minimum version and system-framework dependencies without claiming signing.
