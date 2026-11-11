use crate::{
    debug::{Controller, location},
    host::{Capture, Emit, InputState, ToolkitHost},
    model::*,
};
use ppl_engine::{arguments, source::Source};
use ppl_syntax::{DiagnosticClass, Severity, SourceFile};
use ppl_vm::{
    AngleMode, DisplayMode, ErrorKind, HeadlessHost, Limits, Machine, ResourceKind, Settings,
};
use serde_json::json;
use std::{
    cell::RefCell,
    collections::BTreeMap,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

pub fn protect_output(output: &Path, protected: &[PathBuf]) -> Result<(), String> {
    fn resolved(path: &Path) -> PathBuf {
        if let Ok(path) = path.canonicalize() {
            return path;
        }
        let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
        if let Some(parent) = absolute.parent()
            && let Ok(parent) = parent.canonicalize()
            && let Some(name) = absolute.file_name()
        {
            return parent.join(name);
        }
        let mut normalized = PathBuf::new();
        for component in absolute.components() {
            match component {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    normalized.pop();
                }
                component => normalized.push(component.as_os_str()),
            }
        }
        normalized
    }
    let destination = resolved(output);
    for input in protected {
        if destination == resolved(input) || same_file::is_same_file(output, input).unwrap_or(false)
        {
            return Err(format!(
                "output {} aliases protected input {}",
                output.display(),
                input.display()
            ));
        }
    }
    Ok(())
}

pub fn atomic_bytes(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}

pub fn save_image(path: &Path, image: &ppl_graphics::Grob) -> io::Result<()> {
    let mut bytes = vec![];
    image.write_png(&mut bytes).map_err(io::Error::other)?;
    atomic_bytes(path, &bytes)
}

fn diagnostic(file: &SourceFile, d: &ppl_syntax::Diagnostic) -> Diagnostic {
    Diagnostic {
        severity: match d.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        }
        .into(),
        class: match d.class() {
            DiagnosticClass::Syntax => "syntax",
            DiagnosticClass::Semantic => "semantic",
            DiagnosticClass::Compatibility => "compatibility",
            DiagnosticClass::Runtime => "runtime",
        }
        .into(),
        code: d.code.into(),
        message: d.message.clone(),
        location: Some(location(file, d.span)),
        hint: d.hint.clone(),
        unverified: d.unverified,
    }
}

fn load(path: &Path, bytes: Option<Vec<u8>>) -> Result<Source, (u8, String)> {
    let bytes = match bytes {
        Some(b) => b,
        None => {
            if !std::fs::metadata(path)
                .map_err(|e| (5, e.to_string()))?
                .is_file()
            {
                return Err((5, "source must be a regular file".into()));
            }
            let file =
                std::fs::File::open(path).map_err(|e| (5, format!("{}: {e}", path.display())))?;
            let mut bytes = vec![];
            file.take(8 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| (5, e.to_string()))?;
            bytes
        }
    };
    if bytes.len() > 8 * 1024 * 1024 {
        return Err((6, "source exceeds the 8 MiB loading limit".into()));
    }
    let absolute = std::path::absolute(path).map_err(|e| (5, e.to_string()))?;
    Source::decode(bytes, Some(absolute), path.display().to_string()).map_err(|e| (1, e))
}

pub fn execute(request: Request, input: Rc<RefCell<InputState>>, emit: Emit) -> Report {
    let started = Instant::now();
    let mut report = Report::new(&request.operation, Some(request.file.clone()));
    execute_inner(request, input, emit, &mut report);
    report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
    report
}

fn execute_inner(
    request: Request,
    input: Rc<RefCell<InputState>>,
    emit: Emit,
    report: &mut Report,
) {
    if let Err(error) = input.borrow_mut().pump() {
        let (status, code) = if error.kind == ErrorKind::Interrupted {
            ("cancelled", 130)
        } else {
            ("resource_limit", 6)
        };
        report.fail(status, code, error.to_string());
        return;
    }
    let source = match load(&request.file, request.source_bytes.clone()) {
        Ok(s) => s,
        Err((code, error)) => {
            report.fail(
                match code {
                    5 => "io_error",
                    6 => "resource_limit",
                    _ => "invalid_source",
                },
                code,
                &error,
            );
            if code == 1 {
                report.diagnostics.push(Diagnostic {
                    severity: "error".into(),
                    class: "syntax".into(),
                    code: "source-load".into(),
                    message: error,
                    location: None,
                    hint: None,
                    unverified: false,
                });
            }
            return;
        }
    };
    report.source = Some(source.identity.clone());
    let file = SourceFile::new(&source.identity.display_name, &source.text);
    let mut sources = BTreeMap::new();
    let mut catalog = BTreeMap::new();
    let mut protected = vec![request.file.clone()];
    for module in &request.modules {
        let Some((name, path)) = module.split_once('=') else {
            report.fail("invalid_request", 2, "module must be NAME=PATH");
            return;
        };
        if name.is_empty() || name == file.name || catalog.contains_key(name) {
            report.fail("invalid_request", 2, "duplicate or empty module name");
            return;
        }
        let module_source = match load(Path::new(path), None) {
            Ok(s) => s,
            Err((code, e)) => {
                report.fail(
                    if code == 5 {
                        "io_error"
                    } else {
                        "invalid_source"
                    },
                    code,
                    e,
                );
                return;
            }
        };
        sources.insert(name.to_owned(), SourceFile::new(path, &module_source.text));
        catalog.insert(
            name.to_owned(),
            ppl_vm::ModuleSource {
                identity: ppl_vm::SourceIdentity::new(name, &module_source.identity.source_sha256),
                source: module_source.text,
            },
        );
        protected.push(PathBuf::from(path));
    }
    if let Some(path) = &request.png
        && let Err(e) = protect_output(path, &protected)
    {
        report.fail("invalid_request", 2, e);
        return;
    }
    let linked = if catalog.is_empty() {
        None
    } else {
        catalog.insert(
            file.name.clone(),
            ppl_vm::ModuleSource {
                identity: ppl_vm::SourceIdentity::new(&file.name, &source.identity.source_sha256),
                source: source.text.clone(),
            },
        );
        match ppl_vm::link_sources(&catalog, &file.name) {
            Ok(image) => Some(image),
            Err(e) => {
                report.diagnostics.push(Diagnostic {
                    severity: "error".into(),
                    class: "semantic".into(),
                    code: "module-link".into(),
                    message: e.detail.clone(),
                    location: None,
                    hint: Some("Check the referenced module's exports and source".into()),
                    unverified: false,
                });
                report.fail("invalid_source", 1, e.to_string());
                return;
            }
        }
    };
    let program = if let Some(image) = &linked {
        for (identity, diagnostics) in image.diagnostics() {
            let source_file = sources.get(&identity.name).unwrap_or(&file);
            report
                .diagnostics
                .extend(diagnostics.items.iter().map(|d| diagnostic(source_file, d)));
        }
        Some(image.program().clone())
    } else {
        let (program, diagnostics) = ppl_compiler::compile_source(&file, &Default::default());
        report.diagnostics = diagnostics
            .items
            .iter()
            .map(|d| diagnostic(&file, d))
            .collect();
        program
    };
    report.compatibility_limitations = report
        .diagnostics
        .iter()
        .filter(|d| d.class == "compatibility")
        .map(|d| d.message.clone())
        .collect();
    let Some(program) = program.filter(|_| !source.text.trim().is_empty()) else {
        report.fail("invalid_source", 1, "source validation failed");
        return;
    };
    if (request.deny_warnings && report.diagnostics.iter().any(|d| d.severity == "warning"))
        || (request.deny_compatibility && !report.compatibility_limitations.is_empty())
    {
        report.fail(
            "policy_failed",
            1,
            "diagnostics rejected by the requested CI policy",
        );
        return;
    }
    if request.operation == "check" {
        return;
    }
    let entry = match program.pick_entry_with_stem(request.entry.as_deref(), Some(&source.stem)) {
        Ok(e) => e,
        Err(e) => {
            report.fail("invalid_request", 2, e);
            return;
        }
    };
    report.entry = Some(entry.clone());
    let settings = Settings {
        angle: match request.angle.as_str() {
            "deg" => AngleMode::Degrees,
            "grad" => AngleMode::Gradians,
            _ => AngleMode::Radians,
        },
        ..Default::default()
    };
    let args = if request.args_format == "ppl" {
        arguments::ppl_args(Some(&request.args), &settings)
    } else {
        arguments::json_args(&request.args, &settings)
    };
    let args = match args {
        Ok(a) => a,
        Err(e) => {
            report.fail("invalid_request", 2, e);
            return;
        }
    };
    let expected =
        program.functions[program.function_index(&entry).expect("selected entry")].params as usize;
    if args.len() != expected {
        report.fail(
            "invalid_request",
            2,
            format!("{entry} expects {expected} arguments, got {}", args.len()),
        );
        return;
    }
    let cancel = input.borrow().cancel.clone();
    let mut scripted = HeadlessHost {
        interrupt: Some(cancel),
        ..Default::default()
    };
    if let Some(keys) = &request.keys {
        // Bound expansion before calling the upstream script parser.
        let mut count = 0usize;
        for token in keys.split(',') {
            let repeats = match token.split_once('*') {
                Some((_, n)) => n.parse::<usize>().unwrap_or(usize::MAX),
                None => 1,
            };
            count = count.saturating_add(repeats);
        }
        if count > 1_000_000 {
            report.fail(
                "invalid_request",
                2,
                "key script exceeds one million events",
            );
            return;
        }
        scripted.keys = match ppl_vm::parse_key_script(keys) {
            Ok(k) => k.into(),
            Err(e) => {
                report.fail("invalid_request", 2, e);
                return;
            }
        };
    }
    scripted.inputs = request
        .inputs
        .iter()
        .map(|t| ppl_vm::ScriptedInput::parse(t))
        .collect();
    scripted.choices = request.choices.iter().copied().collect();
    let capture = Rc::new(RefCell::new(Capture::default()));
    let host = ToolkitHost {
        scripted,
        interactive: request.interactive,
        input: input.clone(),
        capture: capture.clone(),
        emit: emit.clone(),
        started: Instant::now(),
        emit_frames: request.emit_frames,
        trace: request.trace,
        max_events: request.max_trace_events,
        max_output: request.max_output_chars,
    };
    let limits = Limits {
        max_steps: Some(request.max_steps),
        max_time: request
            .max_time
            .and_then(|s| Duration::try_from_secs_f64(s).ok()),
        max_depth: request.max_depth,
        max_collection_len: request.max_collection_len,
        max_string_bytes: request.max_string_bytes,
        max_output_chars: request.max_output_chars,
    };
    let mut machine = match linked {
        Some(image) => Machine::new_linked(image, Box::new(host), settings, limits),
        None => Machine::new(Rc::new(program), Box::new(host), settings, limits),
    };
    machine.set_seed(request.seed);
    if request.debug || request.trace {
        machine.set_debugger(Some(Box::new(Controller::new(
            &request,
            file.clone(),
            sources.clone(),
            input,
            capture.clone(),
            emit,
        ))));
    }
    let result = machine
        .initialize_dependencies()
        .and_then(|_| machine.call_function(&entry, args));
    let captured = capture.borrow();
    report.output = captured.output.clone();
    report.frames_presented = captured.frames;
    report.trace = captured.trace.clone();
    report.data = json!({"dialogs":captured.dialogs,"debug_history":captured.debug_history});
    report.instructions = machine.steps();
    match result {
        Ok(value) => {
            report.return_value = Some(arguments::encode(&value));
        }
        Err(error) => {
            let (status, code) = match error.kind {
                ErrorKind::Interrupted if captured.trace_exhausted => ("resource_limit", 6),
                ErrorKind::Interrupted => ("cancelled", 130),
                ErrorKind::ResourceLimit(ResourceKind::Time) => ("timed_out", 4),
                ErrorKind::ResourceLimit(_) => ("resource_limit", 6),
                ErrorKind::ScriptExhausted => ("input_exhausted", 7),
                ErrorKind::Unsupported => ("unsupported", 8),
                _ => ("runtime_error", 3),
            };
            let message = if captured.trace_exhausted {
                "debug/trace observation limit exceeded".into()
            } else {
                error.to_string()
            };
            report.fail(status, code, &message);
            report.runtime_error = Some(message);
            report.stack = error
                .trace
                .iter()
                .map(|frame| Frame {
                    function: frame.function.clone(),
                    location: location(
                        frame
                            .module
                            .as_ref()
                            .and_then(|id| sources.get(&id.name))
                            .unwrap_or(&file),
                        frame.span,
                    ),
                })
                .collect();
            if code == 8 {
                report.compatibility_limitations.push(error.to_string());
            }
        }
    }
    if captured.trace_exhausted {
        report.fail(
            "resource_limit",
            6,
            "debug/trace observation limit exceeded",
        );
        report.runtime_error = report.error.clone();
    }
    if request.trace
        && report.runtime_error.is_some()
        && report.trace.len() < request.max_trace_events
    {
        report.trace.push(
            json!({"kind":"runtime_failure","message":report.runtime_error,"stack":report.stack}),
        );
    }
    drop(captured);
    if let Some(path) = &request.png {
        let image = if machine.display_mode() == DisplayMode::Terminal {
            ppl_graphics::text::render_terminal(&machine.terminal)
        } else {
            machine.screen().clone()
        };
        match save_image(path, &image) {
            Ok(()) => report.artifacts.push(path.clone()),
            Err(e) => {
                let prior = report.error.take();
                report.fail(
                    "io_error",
                    5,
                    format!(
                        "cannot save {}: {e}{}",
                        path.display(),
                        prior
                            .map(|e| format!("; execution: {e}"))
                            .unwrap_or_default()
                    ),
                );
            }
        }
    }
}
