# Architecture and development

`hpdev-cli` owns Clap commands, supervised worker processes, Ratatui/Crossterm
presentation and the optional Axum browser viewer. `hpdev-core` owns request
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

PNG, terminal and browser rendering share native G0 pixels or the upstream
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
