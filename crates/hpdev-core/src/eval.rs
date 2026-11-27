//! Expression evaluation, interactive REPL session, and state serialization.
use crate::{
    execution::{load_source_file, resolve_modules},
    model::*,
};
use ppl_compiler::is_system_variable;
use ppl_engine::arguments;
use ppl_graphics::Grob;
use ppl_syntax::SourceFile;
use ppl_vm::{
    AngleMode, ErrorKind, HeadlessHost, Host, Limits, Machine, ResourceKind, RuntimeError,
    SessionContext, Settings, SourceIdentity, Value,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

/// Render a runtime `Value` as a valid PPL source literal for binding or state replay.
pub fn value_to_ppl_literal(value: &Value, settings: &Settings) -> Option<String> {
    let opts = settings.format();
    match value {
        Value::Real(r) => Some(r.to_string()),
        Value::Int(i) => Some(i.to_string()),
        Value::Complex(c) => Some(format!("({},{})", c.re, c.im)),
        Value::Str(s) => Some(ppl_string_literal(s)),
        Value::List(items) => {
            let mut parts = Vec::with_capacity(items.len());
            for item in items.iter() {
                parts.push(value_to_ppl_literal(item, settings)?);
            }
            Some(format!("{{{}}}", parts.join(",")))
        }
        Value::Matrix(m) => {
            if m.vector {
                let mut parts = Vec::with_capacity(m.cols);
                for item in &m.data {
                    parts.push(value_to_ppl_literal(item, settings)?);
                }
                Some(format!("[{}]", parts.join(",")))
            } else {
                let mut rows = Vec::with_capacity(m.rows);
                for r in 0..m.rows {
                    let mut cols = Vec::with_capacity(m.cols);
                    for c in 0..m.cols {
                        cols.push(value_to_ppl_literal(m.get(r, c), settings)?);
                    }
                    rows.push(format!("[{}]", cols.join(",")));
                }
                Some(format!("[{}]", rows.join(",")))
            }
        }
        Value::Grob(_) => {
            let _ = opts;
            None
        }
    }
}

fn ppl_string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// Evaluate a PPL expression against a set of local and global variable bindings.
pub fn eval_with_bindings(
    expr: &str,
    locals: &[(String, Value)],
    globals: &[(String, Value)],
    settings: &Settings,
) -> Result<Value, String> {
    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return Err("expression cannot be empty".into());
    }
    let mut seen = BTreeSet::new();
    let mut local_decls = Vec::new();
    let mut assignments = Vec::new();

    for (name, value) in locals.iter().chain(globals.iter()) {
        let key = name.to_ascii_lowercase();
        if !seen.insert(key) {
            continue;
        }
        if matches!(value, Value::Grob(_)) {
            continue;
        }
        if is_system_variable(name)
            && name.starts_with('G')
            && name.len() == 2
            && name.as_bytes()[1].is_ascii_digit()
        {
            continue;
        }
        let Some(literal) = value_to_ppl_literal(value, settings) else {
            continue;
        };
        if !is_system_variable(name) {
            local_decls.push(name.clone());
        }
        assignments.push(format!("{name}:={literal};"));
    }

    let mut source = String::from("EXPORT __HPDEV_DBG_EVAL()\nBEGIN\n");
    if !local_decls.is_empty() {
        source.push_str("LOCAL ");
        source.push_str(&local_decls.join(","));
        source.push_str(";\n");
    }
    for assign in assignments {
        source.push_str(&assign);
        source.push('\n');
    }
    source.push_str(&format!("RETURN ({trimmed});\nEND;"));

    let file = SourceFile::new("<eval>", &source);
    let (prog, diags) = ppl_compiler::compile_source(&file, &Default::default());
    let Some(prog) = prog else {
        let msg = diags
            .errors()
            .next()
            .map(|d| d.message.clone())
            .unwrap_or_else(|| "invalid expression".into());
        return Err(msg);
    };
    let limits = Limits {
        max_steps: Some(100_000),
        max_time: Some(Duration::from_secs(2)),
        ..Limits::default()
    };
    let mut machine = Machine::new(
        Rc::new(prog),
        Box::new(HeadlessHost::new()),
        settings.clone(),
        limits,
    );
    machine
        .call_function("__HPDEV_DBG_EVAL", Vec::new())
        .map_err(|e| e.to_string())
}

/// Build `Settings` from CLI angle/format/digits parameters.
pub fn build_settings(angle: &str, format: &str, digits: i64) -> Result<Settings, String> {
    let mut settings = Settings {
        angle: match angle.to_ascii_lowercase().as_str() {
            "rad" | "radians" => AngleMode::Radians,
            "deg" | "degrees" => AngleMode::Degrees,
            "grad" | "gradians" => AngleMode::Gradians,
            other => {
                return Err(format!(
                    "unknown angle mode `{other}`; use rad, deg, or grad"
                ));
            }
        },
        ..Default::default()
    };
    let hformat = match format.to_ascii_lowercase().as_str() {
        "standard" | "std" => 0,
        "fixed" | "fix" => 1,
        "scientific" | "sci" => 2,
        "engineering" | "eng" => 3,
        other => {
            return Err(format!(
                "unknown number format `{other}`; use standard, fixed, sci, or eng"
            ));
        }
    };
    settings.set_format(hformat, digits);
    Ok(settings)
}

#[derive(Clone, Debug)]
pub struct EvalOptions {
    pub expr: String,
    pub file: Option<PathBuf>,
    pub modules: Vec<String>,
    pub angle: String,
    pub format: String,
    pub digits: i64,
    pub seed: u64,
    pub max_steps: Option<u64>,
    pub max_time: Option<f64>,
}

impl Default for EvalOptions {
    fn default() -> Self {
        Self {
            expr: String::new(),
            file: None,
            modules: vec![],
            angle: "rad".into(),
            format: "standard".into(),
            digits: 4,
            seed: 0x5eed,
            max_steps: Some(5_000_000),
            max_time: Some(30.0),
        }
    }
}

struct PrintHost {
    inner: HeadlessHost,
    output: Rc<RefCell<Vec<String>>>,
}

impl Host for PrintHost {
    fn print(&mut self, text: &str) {
        self.output.borrow_mut().push(text.to_owned());
        self.inner.print(text);
    }
    fn clear_terminal(&mut self) {
        self.inner.clear_terminal();
    }
    fn present(&mut self, screen: &Grob, terminal: Option<&[String]>) {
        self.inner.present(screen, terminal);
    }
    fn poll_key(&mut self) -> Result<Option<i64>, RuntimeError> {
        self.inner.poll_key()
    }
    fn is_key_down(&mut self, key: i64) -> Result<bool, RuntimeError> {
        self.inner.is_key_down(key)
    }
    fn wait_key(&mut self, timeout_s: Option<f64>) -> Result<Option<i64>, RuntimeError> {
        self.inner.wait_key(timeout_s)
    }
    fn sleep(&mut self, seconds: f64) -> Result<(), RuntimeError> {
        self.inner.sleep(seconds)
    }
    fn touch(&mut self) -> Result<Option<ppl_vm::host::Touch>, RuntimeError> {
        self.inner.touch()
    }
    fn dialog(
        &mut self,
        dialog: &ppl_vm::Dialog,
        screen: &Grob,
    ) -> Result<ppl_vm::DialogResult, RuntimeError> {
        self.inner.dialog(dialog, screen)
    }
    fn now_ms(&mut self) -> f64 {
        self.inner.now_ms()
    }
    fn local_time(&mut self) -> (i64, i64, i64, i64, i64, f64) {
        self.inner.local_time()
    }
}

/// Evaluate a standalone PPL expression (optionally with a program file in scope).
pub fn eval_expression(opts: EvalOptions) -> Report {
    let started = std::time::Instant::now();
    let mut report = Report::new("eval", opts.file.clone());
    if opts.expr.trim().is_empty() {
        report.fail("invalid_request", 2, "expression cannot be empty");
        return report;
    }
    if opts.max_time.is_some_and(|s| !s.is_finite() || s <= 0.) || opts.max_steps == Some(0) {
        report.fail(
            "invalid_request",
            2,
            "limits must be positive and time must be finite",
        );
        return report;
    }
    let settings = match build_settings(&opts.angle, &opts.format, opts.digits) {
        Ok(s) => s,
        Err(e) => {
            report.fail("invalid_request", 2, e);
            return report;
        }
    };
    let limits = Limits {
        max_steps: opts.max_steps,
        max_time: opts
            .max_time
            .and_then(|s| Duration::try_from_secs_f64(s).ok()),
        ..Limits::default()
    };

    let output = Rc::new(RefCell::new(Vec::new()));
    let host = PrintHost {
        inner: HeadlessHost::new(),
        output: output.clone(),
    };

    let steps;
    let eval_result: Result<Value, RuntimeError>;

    if let Some(path) = &opts.file {
        let source = match load_source_file(path, None) {
            Ok(s) => s,
            Err((code, msg)) => {
                report.fail(
                    match code {
                        5 => "io_error",
                        6 => "resource_limit",
                        _ => "invalid_source",
                    },
                    code,
                    msg,
                );
                report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
                return report;
            }
        };
        report.source = Some(source.identity.clone());
        let (sources, mut catalog) = match resolve_modules(&source, &opts.modules) {
            Ok(v) => v,
            Err((code, msg)) => {
                report.fail(
                    if code == 5 {
                        "io_error"
                    } else if code == 2 {
                        "invalid_request"
                    } else {
                        "invalid_source"
                    },
                    code,
                    msg,
                );
                report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
                return report;
            }
        };
        let combined = format!(
            "{}\nEXPORT __HPDEV_EVAL()\nBEGIN\nRETURN ({});\nEND;",
            source.text, opts.expr
        );
        let file = SourceFile::new(&source.identity.display_name, &combined);
        if catalog.is_empty() {
            let (program, diags) = ppl_compiler::compile_source(&file, &Default::default());
            report.diagnostics = diags
                .items
                .iter()
                .map(|d| crate::execution::to_diagnostic(&file, d))
                .collect();
            let Some(program) = program else {
                report.fail(
                    "invalid_source",
                    1,
                    "expression or source validation failed",
                );
                report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
                return report;
            };
            let mut machine =
                Machine::new(Rc::new(program), Box::new(host), settings.clone(), limits);
            machine.set_seed(opts.seed);
            eval_result = machine
                .initialize_dependencies()
                .and_then(|_| machine.call_function("__HPDEV_EVAL", Vec::new()));
            steps = machine.steps();
        } else {
            catalog.insert(
                file.name.clone(),
                ppl_vm::ModuleSource {
                    identity: ppl_vm::SourceIdentity::new(
                        &file.name,
                        &source.identity.source_sha256,
                    ),
                    source: combined,
                },
            );
            let image = match ppl_vm::link_sources(&catalog, &file.name) {
                Ok(img) => img,
                Err(e) => {
                    report.fail("invalid_source", 1, e.to_string());
                    report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
                    return report;
                }
            };
            for (identity, diags) in image.diagnostics() {
                let sf = sources.get(&identity.name).unwrap_or(&file);
                report.diagnostics.extend(
                    diags
                        .items
                        .iter()
                        .map(|d| crate::execution::to_diagnostic(sf, d)),
                );
            }
            let mut machine = Machine::new_linked(image, Box::new(host), settings.clone(), limits);
            machine.set_seed(opts.seed);
            eval_result = machine
                .initialize_dependencies()
                .and_then(|_| machine.call_function("__HPDEV_EVAL", Vec::new()));
            steps = machine.steps();
        }
    } else {
        let synthetic = format!(
            "EXPORT __HPDEV_EVAL()\nBEGIN\nRETURN ({});\nEND;",
            opts.expr
        );
        let file = SourceFile::new("<eval>", &synthetic);
        let (program, diags) = ppl_compiler::compile_source(&file, &Default::default());
        report.diagnostics = diags
            .items
            .iter()
            .map(|d| crate::execution::to_diagnostic(&file, d))
            .collect();
        let Some(program) = program else {
            report.fail("invalid_source", 1, "expression validation failed");
            report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
            return report;
        };
        let mut machine = Machine::new(Rc::new(program), Box::new(host), settings.clone(), limits);
        machine.set_seed(opts.seed);
        eval_result = machine.call_function("__HPDEV_EVAL", Vec::new());
        steps = machine.steps();
    }

    report.output = output.borrow().clone();
    report.instructions = steps;
    match eval_result {
        Ok(value) => {
            let formatted = value.display(settings.format());
            let type_name = value.type_name();
            report.return_value = Some(arguments::encode(&value));
            report.data = json!({
                "display": formatted,
                "type": type_name,
                "angle": opts.angle,
                "format": opts.format,
            });
        }
        Err(error) => {
            let (status, code) = classify_runtime_error(&error);
            let msg = error.to_string();
            report.fail(status, code, &msg);
            report.runtime_error = Some(msg);
            if code == 8 {
                report.compatibility_limitations.push(error.to_string());
            }
        }
    }
    report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
    report
}

pub(crate) fn classify_runtime_error(error: &RuntimeError) -> (&'static str, u8) {
    match error.kind {
        ErrorKind::Interrupted => ("cancelled", 130),
        ErrorKind::ResourceLimit(ResourceKind::Time) => ("timed_out", 4),
        ErrorKind::ResourceLimit(_) => ("resource_limit", 6),
        ErrorKind::ScriptExhausted => ("input_exhausted", 7),
        ErrorKind::Unsupported => ("unsupported", 8),
        _ => ("runtime_error", 3),
    }
}

/// Persistent session state file used by `hpdev snapshot` and `hpdev repl`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionStateFile {
    pub schema_version: u32,
    pub seed: u64,
    pub angle: String,
    pub hformat: i64,
    pub hdigits: i64,
    pub complex_results: bool,
    pub globals: BTreeMap<String, serde_json::Value>,
    pub last_value: Option<serde_json::Value>,
    pub notes: BTreeMap<String, String>,
    pub programs: BTreeMap<String, String>,
}

pub fn angle_name(mode: AngleMode) -> &'static str {
    match mode {
        AngleMode::Radians => "rad",
        AngleMode::Degrees => "deg",
        AngleMode::Gradians => "grad",
    }
}

/// Interactive or scripted stateful PPL evaluation session backed by `ppl_vm::SessionContext`.
pub struct ReplSession {
    pub context: SessionContext,
    pub seed: u64,
    pub limits: Limits,
    pub loaded_source: Option<(PathBuf, String)>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReplOutcome {
    pub kind: String,
    pub status: String,
    pub exit_code: u8,
    pub output: Vec<String>,
    pub display: Option<String>,
    pub value: Option<serde_json::Value>,
    pub message: Option<String>,
    pub steps: u64,
}

impl ReplSession {
    pub fn new(settings: Settings, seed: u64, limits: Limits) -> Self {
        Self {
            context: SessionContext::new(settings, seed),
            seed,
            limits,
            loaded_source: None,
        }
    }

    pub fn new_bounded(
        settings: Settings,
        seed: u64,
        max_steps: Option<u64>,
        max_time: Option<f64>,
    ) -> Self {
        let limits = Limits {
            max_steps,
            max_time: max_time.and_then(|s| Duration::try_from_secs_f64(s).ok()),
            ..Limits::default()
        };
        Self::new(settings, seed, limits)
    }

    pub fn load_program(&mut self, path: &Path) -> Result<Vec<String>, String> {
        let source = load_source_file(path, None).map_err(|(_, e)| e)?;
        let file = SourceFile::new(&source.identity.display_name, &source.text);
        let (program, diags) = ppl_compiler::compile_source(&file, &Default::default());
        let Some(program) = program else {
            let msg = diags
                .errors()
                .next()
                .map(|d| d.message.clone())
                .unwrap_or_else(|| "source validation failed".into());
            return Err(msg);
        };
        let exports: Vec<String> = program.exported().map(|(_, c)| c.name.clone()).collect();
        self.loaded_source = Some((path.to_owned(), source.text));
        Ok(exports)
    }

    pub fn run_loaded_entry(
        &mut self,
        entry: Option<&str>,
        args_str: &str,
        args_format: &str,
    ) -> Result<ReplOutcome, (u8, String)> {
        let Some((path, text)) = self.loaded_source.clone() else {
            return Err((2, "no program loaded".into()));
        };
        let file = SourceFile::new(path.display().to_string(), &text);
        let (program, diags) = ppl_compiler::compile_source(&file, &Default::default());
        let Some(program) = program else {
            let msg = diags
                .errors()
                .next()
                .map(|d| d.message.clone())
                .unwrap_or_else(|| "source validation failed".into());
            return Err((1, msg));
        };
        let stem = path.file_stem().and_then(|s| s.to_str());
        let picked = program
            .pick_entry_with_stem(entry, stem)
            .map_err(|e| (2u8, e))?;
        let args = if args_format == "ppl" {
            arguments::ppl_args(Some(args_str), &self.context.settings)
        } else {
            arguments::json_args(args_str, &self.context.settings)
        }
        .map_err(|e| (2u8, e))?;
        let expected =
            program.functions[program.function_index(&picked).expect("entry")].params as usize;
        if args.len() != expected {
            return Err((
                2,
                format!("{picked} expects {expected} arguments, got {}", args.len()),
            ));
        }

        let output = Rc::new(RefCell::new(Vec::new()));
        let host = PrintHost {
            inner: HeadlessHost::new(),
            output: output.clone(),
        };
        let identity = ppl_vm::SourceIdentity::new(path.display().to_string(), &text);
        let run = self.context.run(
            identity,
            Rc::new(program),
            Box::new(host),
            self.limits.clone(),
            true,
            |machine| {
                machine
                    .initialize_dependencies()
                    .and_then(|_| machine.call_function(&picked, args))
            },
        );
        let captured = output.borrow().clone();
        match run.result {
            Ok(value) => {
                let display = value.display(self.context.settings.format());
                Ok(ReplOutcome {
                    kind: "eval".into(),
                    status: "success".into(),
                    exit_code: 0,
                    output: captured,
                    display: Some(display),
                    value: Some(arguments::encode(&value)),
                    message: None,
                    steps: run.steps,
                })
            }
            Err(error) => {
                let (status, code) = classify_runtime_error(&error);
                Ok(ReplOutcome {
                    kind: "eval".into(),
                    status: status.into(),
                    exit_code: code,
                    output: captured,
                    display: None,
                    value: None,
                    message: Some(error.to_string()),
                    steps: run.steps,
                })
            }
        }
    }

    pub fn eval_line(&mut self, line: &str) -> ReplOutcome {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return ReplOutcome {
                kind: "empty".into(),
                status: "success".into(),
                exit_code: 0,
                output: vec![],
                display: None,
                value: None,
                message: None,
                steps: 0,
            };
        }
        if let Some(cmd) = trimmed.strip_prefix(':') {
            return self.meta_command(cmd.trim());
        }

        let output = Rc::new(RefCell::new(Vec::new()));
        let host = PrintHost {
            inner: HeadlessHost::new(),
            output: output.clone(),
        };
        let eval_target = if let Some((_, ref program_src)) = self.loaded_source {
            // Try compiling with loaded program definitions in scope so REPL can invoke loaded EXPORT functions.
            let combined = format!("{program_src}\nEXPORT __HPDEV_REPL()\nBEGIN\n{trimmed};\nEND;");
            let file = SourceFile::new("<repl>", &combined);
            let (prog, diags) = ppl_compiler::compile_source(&file, &Default::default());
            if let Some(prog) = prog {
                let run = self.context.run(
                    SourceIdentity::new("<repl>", &combined),
                    Rc::new(prog),
                    Box::new(host),
                    self.limits.clone(),
                    true,
                    |machine| {
                        machine
                            .initialize_dependencies()
                            .and_then(|_| machine.call_function("__HPDEV_REPL", Vec::new()))
                    },
                );
                let captured = output.borrow().clone();
                return match run.result {
                    Ok(value) => ReplOutcome {
                        kind: "eval".into(),
                        status: "success".into(),
                        exit_code: 0,
                        output: captured,
                        display: Some(value.display(self.context.settings.format())),
                        value: Some(arguments::encode(&value)),
                        message: None,
                        steps: run.steps,
                    },
                    Err(error) => {
                        let (status, code) = classify_runtime_error(&error);
                        ReplOutcome {
                            kind: "eval".into(),
                            status: status.into(),
                            exit_code: code,
                            output: captured,
                            display: None,
                            value: None,
                            message: Some(error.to_string()),
                            steps: run.steps,
                        }
                    }
                };
            } else {
                let _ = diags;
                // Fall back to direct evaluate_home (for error reporting)
                trimmed.to_owned()
            }
        } else {
            trimmed.to_owned()
        };

        let run = self
            .context
            .evaluate_home(&eval_target, Box::new(host), self.limits.clone());
        let captured = output.borrow().clone();
        match run.result {
            Ok(value) => ReplOutcome {
                kind: "eval".into(),
                status: "success".into(),
                exit_code: 0,
                output: captured,
                display: Some(value.display(self.context.settings.format())),
                value: Some(arguments::encode(&value)),
                message: None,
                steps: run.steps,
            },
            Err(error) => {
                let (status, code) = classify_runtime_error(&error);
                ReplOutcome {
                    kind: "eval".into(),
                    status: status.into(),
                    exit_code: code,
                    output: captured,
                    display: None,
                    value: None,
                    message: Some(error.to_string()),
                    steps: run.steps,
                }
            }
        }
    }

    fn meta_command(&mut self, cmd: &str) -> ReplOutcome {
        let mut parts = cmd.splitn(2, char::is_whitespace);
        let verb = parts.next().unwrap_or("");
        let rest = parts.next().unwrap_or("").trim();
        let ok = |msg: String| ReplOutcome {
            kind: "meta".into(),
            status: "success".into(),
            exit_code: 0,
            output: vec![],
            display: None,
            value: None,
            message: Some(msg),
            steps: 0,
        };
        let err = |code: u8, status: &str, msg: String| ReplOutcome {
            kind: "meta".into(),
            status: status.into(),
            exit_code: code,
            output: vec![],
            display: None,
            value: None,
            message: Some(msg),
            steps: 0,
        };
        match verb {
            "help" | "h" | "?" => ok(concat!(
                "REPL commands:\n",
                "  :vars                Show session variables and Ans\n",
                "  :angle rad|deg|grad  Set angle mode\n",
                "  :format MODE [N]     Set number format (standard, fixed, sci, eng)\n",
                "  :load FILE           Load PPL program definitions into scope\n",
                "  :save FILE           Save session state snapshot to JSON\n",
                "  :restore FILE        Restore session state snapshot from JSON\n",
                "  :reset               Reset session state\n",
                "  :quit / :exit        Exit REPL"
            )
            .into()),
            "vars" | "v" => {
                let snap = self.context.snapshot();
                let mut lines = Vec::new();
                for (name, val) in &snap.globals {
                    lines.push(format!("{name} = {}", val.display(&snap.settings)));
                }
                if let Some(ans) = &snap.last_value {
                    lines.push(format!("last = {}", ans.display(&snap.settings)));
                }
                ok(lines.join("\n"))
            }
            "reset" => {
                let settings = self.context.settings.clone();
                self.context = SessionContext::new(settings, self.seed);
                self.loaded_source = None;
                ok("Session reset".into())
            }
            "angle" => match rest.to_ascii_lowercase().as_str() {
                "rad" | "radians" => {
                    self.context.settings.angle = AngleMode::Radians;
                    ok("Angle mode: rad".into())
                }
                "deg" | "degrees" => {
                    self.context.settings.angle = AngleMode::Degrees;
                    ok("Angle mode: deg".into())
                }
                "grad" | "gradians" => {
                    self.context.settings.angle = AngleMode::Gradians;
                    ok("Angle mode: grad".into())
                }
                _ => err(2, "invalid_request", "Usage: :angle rad|deg|grad".into()),
            },
            "format" => {
                let mut fparts = rest.split_whitespace();
                let mode = fparts.next().unwrap_or("");
                let digits = fparts
                    .next()
                    .and_then(|d| d.parse::<i64>().ok())
                    .unwrap_or(self.context.settings.hdigits);
                match build_settings(angle_name(self.context.settings.angle), mode, digits) {
                    Ok(s) => {
                        self.context.settings.number_format = s.number_format;
                        self.context.settings.hformat = s.hformat;
                        self.context.settings.hdigits = s.hdigits;
                        ok(format!("Format: {mode} ({digits})"))
                    }
                    Err(e) => err(2, "invalid_request", e),
                }
            }
            "load" => {
                if rest.is_empty() {
                    return err(2, "invalid_request", "Usage: :load FILE".into());
                }
                match self.load_program(Path::new(rest)) {
                    Ok(exports) => ok(format!("Loaded {rest} (exports: {})", exports.join(", "))),
                    Err(e) => err(1, "invalid_source", e),
                }
            }
            "save" => {
                if rest.is_empty() {
                    return err(2, "invalid_request", "Usage: :save PATH".into());
                }
                let state = self.export_state();
                match serde_json::to_vec_pretty(&state)
                    .map_err(|e| e.to_string())
                    .and_then(|b| {
                        crate::execution::atomic_bytes(Path::new(rest), &b)
                            .map_err(|e| e.to_string())
                    }) {
                    Ok(()) => ok(format!("Saved state to {rest}")),
                    Err(e) => err(5, "io_error", e),
                }
            }
            "restore" => {
                if rest.is_empty() {
                    return err(2, "invalid_request", "Usage: :restore PATH".into());
                }
                match std::fs::read_to_string(rest)
                    .map_err(|e| (5u8, "io_error", e.to_string()))
                    .and_then(|s| {
                        serde_json::from_str::<SessionStateFile>(&s)
                            .map_err(|e| (2u8, "invalid_request", e.to_string()))
                    }) {
                    Ok(state) => match self.import_state(&state) {
                        Ok(()) => ok(format!("Restored state from {rest}")),
                        Err(e) => err(2, "invalid_request", e),
                    },
                    Err((c, st, e)) => err(c, st, e),
                }
            }
            "quit" | "exit" => ReplOutcome {
                kind: "quit".into(),
                status: "success".into(),
                exit_code: 0,
                output: vec![],
                display: None,
                value: None,
                message: None,
                steps: 0,
            },
            _ => err(
                2,
                "invalid_request",
                format!("Unknown command `:{verb}`; type :help"),
            ),
        }
    }

    pub fn export_state(&self) -> SessionStateFile {
        let snap = self.context.snapshot();
        let mut globals = BTreeMap::new();
        for (k, v) in &snap.globals {
            globals.insert(k.clone(), arguments::encode(&v.to_value()));
        }
        SessionStateFile {
            schema_version: 1,
            seed: self.seed,
            angle: angle_name(snap.settings.angle).into(),
            hformat: snap.settings.hformat,
            hdigits: snap.settings.hdigits,
            complex_results: snap.settings.complex_results,
            globals,
            last_value: snap
                .last_value
                .as_ref()
                .map(|v| arguments::encode(&v.to_value())),
            notes: snap.notes,
            programs: snap.programs,
        }
    }

    pub fn import_state(&mut self, state: &SessionStateFile) -> Result<(), String> {
        if state.schema_version != 1 {
            return Err(format!(
                "unsupported state schema_version {}",
                state.schema_version
            ));
        }
        let mut settings = Settings {
            angle: match state.angle.as_str() {
                "deg" => AngleMode::Degrees,
                "grad" => AngleMode::Gradians,
                _ => AngleMode::Radians,
            },
            complex_results: state.complex_results,
            ..Default::default()
        };
        settings.set_format(state.hformat, state.hdigits);

        self.seed = state.seed;
        self.context = SessionContext::new(settings.clone(), state.seed);

        let mut statements = Vec::new();
        for (name, json_val) in &state.globals {
            if !is_system_variable(name) {
                continue;
            }
            if name.starts_with('G') && name.len() == 2 && name.as_bytes()[1].is_ascii_digit() {
                continue;
            }
            let arr_str = serde_json::to_string(&vec![json_val]).map_err(|e| e.to_string())?;
            let decoded = arguments::json_args(&arr_str, &settings)?;
            if let Some(val) = decoded.into_iter().next()
                && let Some(lit) = value_to_ppl_literal(&val, &settings)
            {
                if name.eq_ignore_ascii_case("Ans") {
                    continue;
                }
                statements.push(format!("{name}:={lit};"));
            }
        }
        for (k, v) in &state.notes {
            statements.push(format!(
                "Notes({}):={};",
                ppl_string_literal(k),
                ppl_string_literal(v)
            ));
        }
        for (k, v) in &state.programs {
            statements.push(format!(
                "Programs({}):={};",
                ppl_string_literal(k),
                ppl_string_literal(v)
            ));
        }
        if !statements.is_empty() {
            let script = statements.join("\n");
            let res = self.context.evaluate_home(
                &script,
                Box::new(HeadlessHost::new()),
                self.limits.clone(),
            );
            if let Err(e) = res.result {
                return Err(format!("failed to restore state variables: {e}"));
            }
        }
        // Restore Ans last so previous assignments don't clobber Ans.
        if let Some(ans_json) = state.globals.get("Ans").or(state.last_value.as_ref()) {
            let arr_str = serde_json::to_string(&vec![ans_json]).map_err(|e| e.to_string())?;
            let decoded = arguments::json_args(&arr_str, &settings)?;
            if let Some(val) = decoded.into_iter().next()
                && let Some(lit) = value_to_ppl_literal(&val, &settings)
            {
                let res = self.context.evaluate_home(
                    &lit,
                    Box::new(HeadlessHost::new()),
                    self.limits.clone(),
                );
                if let Err(e) = res.result {
                    return Err(format!("failed to restore Ans: {e}"));
                }
            }
        }
        Ok(())
    }
}
