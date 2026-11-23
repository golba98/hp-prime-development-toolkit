use crate::{
    commands::{
        self, AstOpts, BenchOpts, CompatOpts, CoverageOpts, DisasmOpts, DocsOpts, EvalOpts,
        FormatOpts, InitOpts, InspectOpts, LintOpts, PackageOpts, PlotOpts, ProfileOpts, ReplOpts,
        SnapshotAction, SnapshotOpts, StatsOpts, SymbolsOpts,
    },
    driver::{self, ViewOptions, Views},
};
use clap::{Args, Parser, Subcommand, ValueEnum};
use hpdev_core::{
    Report, Request,
    execution::{atomic_bytes, protect_output},
};
use serde_json::json;
use std::{
    io::{self, IsTerminal},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Parser)]
#[command(
    name = "hpdev",
    version,
    about = "Standalone native HP Prime PPL development toolkit"
)]
pub struct Cli {
    #[arg(long, global = true)]
    pub json: bool,
    #[arg(short, long, global = true)]
    pub quiet: bool,
    /// Show additional diagnostic hints, source metadata, and timing details.
    #[arg(short, long, global = true)]
    pub verbose: bool,
    /// Atomically write the same versioned JSON report to a file.
    #[arg(long, global = true)]
    pub report: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Command,
}
#[derive(Subcommand)]
pub enum Command {
    /// Validate without executing. Compatibility limitations remain distinct.
    Check(Check),
    /// Run in a native interactive window; use --headless for automation.
    Run(DesktopRun),
    /// Debug actual VM statements and inspect real values.
    Debug(Debug),
    /// Preview actual framebuffer updates in a terminal, PNG, or browser.
    Preview(Preview),
    /// Debounce source changes and cancel previous execution before rerunning.
    Watch(Watch),
    /// Run assertions and graphics snapshots from hpdev.toml.
    Test(Test),
    /// Observe executed statements and optional variable changes.
    Trace(Trace),
    /// Inspect native installation and presentation capabilities; never opens USB.
    Doctor,
    /// Evaluate a mathematical or PPL expression directly.
    Eval(EvalOpts),
    /// Interactive stateful PPL session (Home-style environment).
    Repl(ReplOpts),
    /// Analyze HP Prime compatibility or list supported built-in commands.
    Compat(CompatOpts),
    /// Inspect parsed PPL abstract syntax trees and token streams.
    Ast(AstOpts),
    /// Inspect program functions, globals, forward declarations, and pragmas.
    Symbols(SymbolsOpts),
    /// Disassemble compiled PPL bytecode instructions.
    Disasm(DisasmOpts),
    /// Run static code-quality and compatibility lint checks.
    Lint(LintOpts),
    /// Format PPL source code with syntax verification.
    Format(FormatOpts),
    /// Inspect .ppl or binary .hpprgm container structure and compiled metrics.
    Inspect(InspectOpts),
    /// Benchmark deterministic PPL execution timing and instruction counts.
    Bench(BenchOpts),
    /// Profile per-function and per-line VM instruction hot spots.
    Profile(ProfileOpts),
    /// Measure line, statement, and function coverage for a program or manifest.
    Coverage(CoverageOpts),
    /// Plot mathematical expressions in X to a PNG image and/or ASCII terminal view.
    Plot(PlotOpts),
    /// Compute 1-variable and 2-variable statistics and regressions.
    Stats(StatsOpts),
    /// Initialize a structured PPL project with starter code and hpdev.toml.
    Init(InitOpts),
    /// Pack a PPL program into a binary .hpprgm container or unpack one to .ppl.
    Package(PackageOpts),
    /// Look up HP Prime command documentation or extract PPL API docs.
    Docs(DocsOpts),
    /// Capture, inspect, or execute against saved calculator session snapshots.
    Snapshot(SnapshotOpts),
}
#[derive(Clone, Copy, ValueEnum)]
pub enum Engine {
    Native,
    Firmware,
}
#[derive(Clone, Copy, ValueEnum)]
pub enum ArgsFormat {
    Json,
    Ppl,
}
#[derive(Clone, Copy, ValueEnum)]
pub enum Angle {
    Rad,
    Deg,
    Grad,
}
#[derive(Args)]
pub struct Source {
    pub file: PathBuf,
    #[arg(long, value_name = "NAME=PATH")]
    pub module: Vec<String>,
    #[arg(long, value_enum, default_value = "native")]
    pub engine: Engine,
}
#[derive(Args)]
pub struct Check {
    #[command(flatten)]
    pub source: Source,
    #[arg(long)]
    pub deny_warnings: bool,
    #[arg(long)]
    pub deny_compatibility: bool,
    /// Fail on any warning or compatibility limitation.
    #[arg(long)]
    pub strict: bool,
    /// Include compiled bytecode disassembly in check output.
    #[arg(long)]
    pub dump_bytecode: bool,
}
#[derive(Args)]
pub struct Run {
    #[command(flatten)]
    pub source: Source,
    #[arg(short, long)]
    pub entry: Option<String>,
    /// Function arguments. JSON array by default, with lossless tagged values.
    #[arg(short, long, default_value = "[]")]
    pub args: String,
    #[arg(long, value_enum, default_value = "json")]
    pub args_format: ArgsFormat,
    #[arg(long, allow_hyphen_values = true)]
    pub keys: Option<String>,
    #[arg(long)]
    pub input: Vec<String>,
    #[arg(long)]
    pub choose: Vec<usize>,
    #[arg(long, default_value_t = 0x5eed)]
    pub seed: u64,
    #[arg(long, value_enum, default_value = "rad")]
    pub angle: Angle,
    /// Total deadline, including input waits. Headless run defaults to 30s; desktop/debug have none.
    #[arg(long, allow_hyphen_values = true)]
    pub max_time: Option<f64>,
    #[arg(long, conflicts_with = "max_time")]
    pub no_time_limit: bool,
    /// Instruction budget (headless default: 5000000; desktop: unlimited).
    #[arg(long)]
    pub max_steps: Option<u64>,
    #[arg(long, default_value_t = 1000)]
    pub max_depth: usize,
    #[arg(long, default_value_t = 1_000_000)]
    pub max_collection_len: usize,
    #[arg(long, default_value_t = 16 * 1024 * 1024)]
    pub max_string_bytes: usize,
    #[arg(long, default_value_t = 4 * 1024 * 1024)]
    pub max_output_chars: usize,
    /// Allow terminal prompts after scripted input is exhausted.
    #[arg(long)]
    pub interactive: bool,
}
#[derive(Args)]
pub struct DesktopRun {
    #[command(flatten)]
    pub run: Run,
    /// Deterministic scripted execution without opening a window.
    #[arg(long, conflicts_with = "gui")]
    pub headless: bool,
    /// Open a native window even with --json.
    #[arg(long, conflicts_with = "interactive")]
    pub gui: bool,
    /// Initial integer display scale.
    #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u32).range(1..=8))]
    pub scale: u32,
    /// Initially show the optional calculator keypad.
    #[arg(long)]
    pub show_keys: bool,
    /// Close the window as soon as execution completes.
    #[arg(long)]
    pub exit_on_finish: bool,
    /// Save final 320x240 display output as a PNG file.
    #[arg(long)]
    pub png: Option<PathBuf>,
}
#[derive(Args)]
pub struct Debug {
    #[command(flatten)]
    pub run: Run,
    #[arg(short = 'b', long = "break", value_parser = clap::value_parser!(usize))]
    pub breakpoints: Vec<usize>,
    /// Conditional breakpoint formatted as LINE=EXPR (e.g. 5=X>3).
    #[arg(long = "break-if", value_name = "LINE=EXPR")]
    pub break_if: Vec<String>,
    /// Continue to the first breakpoint instead of stopping at the first statement.
    #[arg(long = "run")]
    pub continue_to_break: bool,
    #[arg(long, conflicts_with = "no_tui")]
    pub tui: bool,
    #[arg(long, conflicts_with = "tui")]
    pub no_tui: bool,
    #[arg(long)]
    pub exit_on_finish: bool,
}
#[derive(Args)]
pub struct Preview {
    #[command(flatten)]
    run: Run,
    #[arg(long, conflicts_with = "web")]
    tui: bool,
    #[arg(long)]
    png: Option<PathBuf>,
    #[arg(long, conflicts_with = "tui")]
    web: bool,
    #[arg(long)]
    no_open: bool,
    #[arg(long, default_value_t = 0)]
    port: u16,
    #[arg(long)]
    exit_on_finish: bool,
    /// Terminal cell width / height. Default 0.5 when pixel dimensions are unavailable.
    #[arg(long)]
    cell_aspect: Option<f64>,
    #[arg(long, value_parser = ["auto", "truecolor", "256", "mono"], default_value = "auto")]
    color: String,
}
#[derive(Args)]
pub struct Watch {
    #[command(flatten)]
    run: Run,
    #[arg(long)]
    check_only: bool,
    #[arg(long, conflicts_with = "web")]
    tui: bool,
    #[arg(long, conflicts_with = "tui")]
    web: bool,
    #[arg(long)]
    no_open: bool,
    #[arg(long, default_value_t = 0)]
    port: u16,
}
#[derive(Args)]
pub struct Test {
    #[arg(default_value = "hpdev.toml")]
    manifest: PathBuf,
    #[arg(long, default_value = ".hpdev/test")]
    artifacts_dir: PathBuf,
    #[arg(long)]
    update_snapshots: bool,
}
#[derive(Args)]
pub struct Trace {
    #[command(flatten)]
    pub run: Run,
    #[arg(long)]
    pub variables: bool,
    #[arg(long, default_value_t = 10_000)]
    pub max_trace_events: usize,
    /// Restrict statement trace events to the named function(s).
    #[arg(long = "filter", value_name = "FUNC")]
    pub filter: Vec<String>,
    /// Include per-function statement execution counts in data.trace_summary.
    #[arg(long)]
    pub summary: bool,
}

impl Source {
    fn request(&self, operation: &str) -> Result<Request, Box<Report>> {
        let mut request = Request {
            operation: operation.into(),
            file: self.file.clone(),
            modules: self.module.clone(),
            ..Default::default()
        };
        if matches!(self.engine, Engine::Firmware) {
            let mut report = Report::new(operation, Some(self.file.clone()));
            report.fail(
                "unsupported",
                8,
                "hpdev supports the native engine; firmware execution is not included",
            );
            return Err(Box::new(report));
        }
        if operation == "debug" {
            request.max_time = None;
            request.debug = true;
        }
        Ok(request)
    }
}
impl Run {
    pub fn request(&self, operation: &str) -> Result<Request, Box<Report>> {
        let mut r = self.source.request(operation)?;
        r.entry = self.entry.clone();
        r.args = self.args.clone();
        r.args_format = match self.args_format {
            ArgsFormat::Json => "json",
            ArgsFormat::Ppl => "ppl",
        }
        .into();
        r.keys = self.keys.clone();
        r.inputs = self.input.clone();
        r.choices = self.choose.clone();
        r.seed = self.seed;
        r.angle = match self.angle {
            Angle::Rad => "rad",
            Angle::Deg => "deg",
            Angle::Grad => "grad",
        }
        .into();
        if self.no_time_limit {
            r.max_time = None;
        } else if let Some(s) = self.max_time {
            r.max_time = Some(s);
        }
        if let Some(steps) = self.max_steps {
            r.max_steps = Some(steps);
        }
        r.max_depth = self.max_depth;
        r.max_collection_len = self.max_collection_len;
        r.max_string_bytes = self.max_string_bytes;
        r.max_output_chars = self.max_output_chars;
        r.interactive = self.interactive;
        if r.max_time.is_some_and(|s| !s.is_finite() || s <= 0.)
            || r.max_steps == Some(0)
            || r.max_depth == 0
        {
            let mut report = Report::new(operation, Some(r.file));
            report.fail(
                "invalid_request",
                2,
                "limits must be positive and time must be finite",
            );
            return Err(Box::new(report));
        }
        Ok(r)
    }
}

fn protected(command: &Command) -> Vec<PathBuf> {
    let source = match command {
        Command::Check(o) => Some(&o.source),
        Command::Run(o) => Some(&o.run.source),
        Command::Debug(o) => Some(&o.run.source),
        Command::Preview(o) => Some(&o.run.source),
        Command::Watch(o) => Some(&o.run.source),
        Command::Trace(o) => Some(&o.run.source),
        Command::Bench(o) => Some(&o.run.source),
        Command::Profile(o) => Some(&o.run.source),
        _ => None,
    };
    let mut paths = vec![];
    if let Some(source) = source {
        paths.push(source.file.clone());
        paths.extend(
            source
                .module
                .iter()
                .filter_map(|s| s.split_once('=').map(|(_, p)| PathBuf::from(p))),
        );
    }
    match command {
        Command::Test(o) => {
            paths.push(o.manifest.clone());
            if let Ok(text) = std::fs::read_to_string(&o.manifest)
                && let Ok(m) = toml::from_str::<hpdev_core::testing::Manifest>(&text)
            {
                let root = o.manifest.parent().unwrap_or(Path::new("."));
                for c in m.tests {
                    paths.push(root.join(c.file));
                    if let Some(p) = c.expect.png {
                        paths.push(root.join(p));
                    }
                }
            }
        }
        Command::Preview(o) => {
            if let Some(p) = &o.png {
                paths.push(p.clone());
            }
        }
        Command::Run(o) => {
            if let Some(p) = &o.png {
                paths.push(p.clone());
            }
        }
        Command::Eval(o) => {
            if let Some(f) = &o.file {
                paths.push(f.clone());
            }
        }
        Command::Repl(o) => {
            if let Some(f) = &o.load {
                paths.push(f.clone());
            }
            if let Some(s) = &o.state {
                paths.push(s.clone());
            }
            if let Some(s) = &o.save_state {
                paths.push(s.clone());
            }
        }
        Command::Compat(o) => {
            if let Some(f) = &o.file {
                paths.push(f.clone());
            }
        }
        Command::Ast(o) => paths.push(o.file.clone()),
        Command::Symbols(o) => paths.push(o.file.clone()),
        Command::Disasm(o) => paths.push(o.file.clone()),
        Command::Lint(o) => paths.push(o.file.clone()),
        Command::Format(o) => {
            paths.push(o.file.clone());
            if let Some(out) = &o.output {
                paths.push(out.clone());
            }
        }
        Command::Inspect(o) => paths.push(o.file.clone()),
        Command::Coverage(o) => {
            if let Some(f) = &o.file {
                paths.push(f.clone());
            }
            if let Some(m) = &o.manifest {
                paths.push(m.clone());
            }
        }
        Command::Plot(o) => {
            if let Some(out) = &o.output {
                paths.push(out.clone());
            }
        }
        Command::Stats(o) => {
            if let Some(csv) = &o.csv {
                paths.push(csv.clone());
            }
        }
        Command::Package(o) => {
            paths.push(o.input.clone());
            paths.push(o.output.clone());
        }
        Command::Docs(o) => {
            if let Some(f) = &o.file {
                paths.push(f.clone());
            }
            if let Some(out) = &o.output {
                paths.push(out.clone());
            }
        }
        Command::Snapshot(o) => match &o.action {
            SnapshotAction::Capture { file, output, .. } => {
                if let Some(f) = file {
                    paths.push(f.clone());
                }
                paths.push(output.clone());
            }
            SnapshotAction::Inspect { state } => paths.push(state.clone()),
            SnapshotAction::Run {
                file,
                state,
                save_state,
                ..
            } => {
                if let Some(f) = file {
                    paths.push(f.clone());
                }
                paths.push(state.clone());
                if let Some(s) = save_state {
                    paths.push(s.clone());
                }
            }
        },
        _ => {}
    }
    paths
}

pub fn main() -> u8 {
    let argv: Vec<_> = std::env::args_os().collect();
    let json_requested = argv.iter().any(|s| s == "--json");
    let cli = match Cli::try_parse_from(&argv) {
        Ok(c) => c,
        Err(error) => {
            if json_requested {
                let mut r = Report::new("arguments", None);
                if error.use_stderr() {
                    r.fail("invalid_request", 2, error.to_string());
                } else {
                    r.data = json!({"text": error.to_string()});
                }
                println!("{}", serde_json::to_string(&r).expect("report JSON"));
                return r.exit_code;
            }
            let code = error.exit_code() as u8;
            let _ = error.print();
            return code;
        }
    };
    if let Some(path) = &cli.report
        && let Err(e) = protect_output(path, &protected(&cli.command))
    {
        let mut r = Report::new("arguments", None);
        r.fail("invalid_request", 2, e);
        return finish(r, &cli);
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    if let Err(e) = ctrlc::set_handler(move || flag.store(true, Ordering::Release)) {
        let mut r = Report::new("startup", None);
        r.fail("infrastructure_error", 70, e.to_string());
        return finish(r, &cli);
    }
    let result = dispatch(&cli, &cancel);
    finish(result, &cli)
}

fn dispatch(cli: &Cli, cancel: &Arc<AtomicBool>) -> Report {
    let (mut request, options) = match &cli.command {
        Command::Doctor => {
            let (total, implemented, approximate, unsupported, catalog) =
                hpdev_core::analysis::builtin_summary_counts();
            let mut r = Report::new("doctor", None);
            r.data = json!({
                "platform": format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
                "native_engine": true,
                "graphics_renderer": true,
                "browser_viewer": cfg!(feature = "web"),
                "desktop_viewer": cfg!(feature = "desktop"),
                "terminal": {
                    "stdin_tty": io::stdin().is_terminal(),
                    "stderr_tty": io::stderr().is_terminal(),
                    "term": std::env::var("TERM").ok(),
                    "colorterm": std::env::var("COLORTERM").ok(),
                },
                "upstream_revision": hpdev_core::UPSTREAM_REVISION,
                "firmware_required": false,
                "calculator_required": false,
                "builtins": {
                    "total": total,
                    "implemented": implemented,
                    "approximate": approximate,
                    "unsupported": unsupported,
                },
                "catalog_commands": catalog,
                "commands": [
                    "check", "run", "debug", "preview", "watch", "test", "trace", "doctor",
                    "eval", "repl", "compat", "ast", "symbols", "disasm", "lint", "format",
                    "inspect", "bench", "profile", "coverage", "plot", "stats", "init",
                    "package", "docs", "snapshot"
                ],
            });
            return r;
        }
        Command::Eval(o) => return commands::run_eval(o, cli.json, cli.quiet),
        Command::Repl(o) => return commands::run_repl(o, cli.json, cli.quiet, cancel),
        Command::Compat(o) => return commands::run_compat(o, cli.json, cli.quiet),
        Command::Ast(o) => return commands::run_ast(o, cli.json),
        Command::Symbols(o) => return commands::run_symbols(o, cli.json, cli.quiet),
        Command::Disasm(o) => return commands::run_disasm(o, cli.json),
        Command::Lint(o) => return commands::run_lint(o, cli.json, cli.quiet),
        Command::Format(o) => return commands::run_format(o, cli.json, cli.quiet),
        Command::Inspect(o) => return commands::run_inspect(o, cli.json),
        Command::Bench(o) => return commands::run_bench(o, cli.json),
        Command::Profile(o) => return commands::run_profile(o, cli.json, cli.quiet, cancel),
        Command::Coverage(o) => return commands::run_coverage(o, cli.json, cli.quiet, cancel),
        Command::Plot(o) => return commands::run_plot(o, cli.json, cli.quiet),
        Command::Stats(o) => return commands::run_stats(o, cli.json),
        Command::Init(o) => return commands::run_init(o, cli.json),
        Command::Package(o) => return commands::run_package(o, cli.json),
        Command::Docs(o) => return commands::run_docs(o, cli.json),
        Command::Snapshot(o) => return commands::run_snapshot(o, cli.json),
        Command::Test(o) => {
            return hpdev_core::testing::run_suite(
                &o.manifest,
                &o.artifacts_dir,
                o.update_snapshots,
                |request| {
                    let mut views = Views::empty();
                    driver::execute(request, &mut views, true, true, cancel, || false).report
                },
            );
        }
        Command::Check(o) => {
            let mut r = match o.source.request("check") {
                Ok(r) => r,
                Err(e) => return *e,
            };
            r.deny_warnings = o.deny_warnings || o.strict;
            r.deny_compatibility = o.deny_compatibility || o.strict;
            r.dump_bytecode = o.dump_bytecode;
            (r, ViewOptions::default())
        }
        Command::Run(o) => {
            let mut r = match o.run.request("run") {
                Ok(r) => r,
                Err(e) => return *e,
            };
            r.png = o.png.clone();
            let desktop = o.gui || !(o.headless || cli.json || o.run.interactive);
            if desktop {
                r.interactive = true;
                r.emit_frames = true;
                r.debug = true;
                r.run_to_breakpoint = true;
                if o.run.max_time.is_none() {
                    r.max_time = None;
                }
                r.max_steps = o.run.max_steps;
            }
            (
                r,
                ViewOptions {
                    desktop,
                    scale: o.scale,
                    show_keys: o.show_keys,
                    exit_on_finish: o.exit_on_finish,
                    ..Default::default()
                },
            )
        }
        Command::Trace(o) => {
            let mut r = match o.run.request("trace") {
                Ok(r) => r,
                Err(e) => return *e,
            };
            r.trace = true;
            r.trace_variables = o.variables;
            r.max_trace_events = o.max_trace_events;
            r.trace_filter = o.filter.clone();
            r.trace_summary = o.summary;
            (r, ViewOptions::default())
        }
        Command::Debug(o) => {
            let mut r = match o.run.request("debug") {
                Ok(r) => r,
                Err(e) => return *e,
            };
            r.breakpoints = o.breakpoints.clone();
            r.run_to_breakpoint = o.continue_to_break;
            if r.breakpoints.contains(&0) {
                let mut e = Report::new("debug", Some(r.file));
                e.fail("invalid_request", 2, "breakpoint lines must be positive");
                return e;
            }
            for spec in &o.break_if {
                let Some((line_str, expr)) = spec.split_once('=') else {
                    let mut e = Report::new("debug", Some(r.file));
                    e.fail(
                        "invalid_request",
                        2,
                        "conditional breakpoint must be LINE=EXPR",
                    );
                    return e;
                };
                let Ok(line) = line_str.trim().parse::<usize>() else {
                    let mut e = Report::new("debug", Some(r.file));
                    e.fail(
                        "invalid_request",
                        2,
                        "breakpoint line must be a positive integer",
                    );
                    return e;
                };
                let cond = expr.trim();
                if line == 0 || cond.is_empty() {
                    let mut e = Report::new("debug", Some(r.file));
                    e.fail(
                        "invalid_request",
                        2,
                        "conditional breakpoint requires positive LINE and non-empty EXPR",
                    );
                    return e;
                }
                r.conditional_breakpoints.push((line, cond.to_owned()));
            }
            let tui = o.tui
                || (!o.no_tui
                    && !cli.json
                    && io::stdin().is_terminal()
                    && io::stderr().is_terminal()
                    && std::env::var("TERM").as_deref() != Ok("dumb"));
            (
                r,
                ViewOptions {
                    tui,
                    debug: true,
                    exit_on_finish: o.exit_on_finish,
                    ..Default::default()
                },
            )
        }
        Command::Preview(o) => {
            let mut r = match o.run.request("preview") {
                Ok(r) => r,
                Err(e) => return *e,
            };
            r.png = o.png.clone();
            (
                r,
                ViewOptions {
                    tui: o.tui || (!o.web && o.png.is_none()),
                    web: o.web,
                    no_open: o.no_open,
                    port: o.port,
                    exit_on_finish: o.exit_on_finish,
                    cell_aspect: o.cell_aspect,
                    color: o.color.clone(),
                    ..Default::default()
                },
            )
        }
        Command::Watch(o) => {
            let r = match o.run.request(if o.check_only { "check" } else { "watch" }) {
                Ok(r) => r,
                Err(e) => return *e,
            };
            (
                r,
                ViewOptions {
                    tui: o.tui,
                    web: o.web,
                    no_open: o.no_open,
                    port: o.port,
                    ..Default::default()
                },
            )
        }
    };
    if options
        .cell_aspect
        .is_some_and(|aspect| !aspect.is_finite() || aspect <= 0.)
    {
        let mut report = Report::new(&request.operation, Some(request.file));
        report.fail(
            "invalid_request",
            2,
            "cell aspect must be positive and finite",
        );
        return report;
    }
    if options.desktop {
        #[cfg(feature = "desktop")]
        return crate::desktop::run(request, options, cli.json, cli.quiet, cancel.clone());
        #[cfg(not(feature = "desktop"))]
        {
            let mut report = Report::new(&request.operation, Some(request.file));
            report.fail("unsupported", 8, "this build excludes the desktop viewer; use --headless or rebuild with the desktop feature");
            return report;
        }
    }
    request.interactive |= options.tui || options.web;
    request.emit_frames = options.tui || options.web;
    if request.interactive && !options.web && !options.tui && !io::stdin().is_terminal() {
        let mut report = Report::new(&request.operation, Some(request.file));
        report.fail(
            "invalid_request",
            2,
            "--interactive requires terminal stdin; use scripted --input/--choose/--keys for automation",
        );
        return report;
    }
    let mut views = match Views::new(options) {
        Ok(v) => v,
        Err(e) => {
            let mut r = Report::new(&request.operation, Some(request.file));
            r.fail(e.status, e.code, e.message);
            return r;
        }
    };
    if let Command::Watch(_) = &cli.command {
        return watch(cli, request, &mut views, cancel);
    }
    let mut outcome = driver::execute(request, &mut views, cli.json, cli.quiet, cancel, || false);
    views.hold(&outcome.report, cancel);
    outcome.report.data["viewer"] = views.metadata();
    outcome.report
}

fn finish(mut report: Report, cli: &Cli) -> u8 {
    if let Some(path) = &cli.report {
        // Do not write a rejected alias, including on an early argument failure.
        if protect_output(path, &protected(&cli.command)).is_ok() {
            let result = serde_json::to_vec_pretty(&report)
                .map_err(io::Error::other)
                .and_then(|bytes| atomic_bytes(path, &bytes));
            if let Err(e) = result {
                report.fail(
                    "io_error",
                    5,
                    format!("cannot write report {}: {e}", path.display()),
                );
            }
        }
    }
    if cli.json {
        println!("{}", serde_json::to_string(&report).expect("report JSON"));
    } else {
        if report.data["viewer"]["desktop"] != true {
            print_diagnostics(&report, cli.verbose);
        }
        if report.operation == "check"
            && let Some(disasm) = report.data["disassembly"].as_str()
        {
            print!("{disasm}");
        }
        if !cli.quiet {
            if report.operation == "test" {
                if let Some(cases) = report.data["cases"].as_array() {
                    for case in cases {
                        println!(
                            "{} {}",
                            if case["passed"] == true {
                                "PASS"
                            } else {
                                "FAIL"
                            },
                            case["name"].as_str().unwrap_or("unnamed")
                        );
                        if let Some(errors) = case["errors"].as_array() {
                            for error in errors {
                                eprintln!("  {}", error.as_str().unwrap_or("assertion failed"));
                            }
                        }
                    }
                }
                println!(
                    "{} passed; {} failed",
                    report.data["passed"], report.data["failed"]
                );
            } else if report.operation == "doctor" {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report.data).expect("data JSON")
                );
            } else if matches!(
                report.operation.as_str(),
                "check" | "run" | "debug" | "preview" | "watch" | "trace"
            ) {
                if report.operation == "trace"
                    && let Some(summary) = report.data["trace_summary"].as_object()
                    && !summary.is_empty()
                {
                    eprintln!(
                        "trace_summary: {}",
                        serde_json::Value::Object(summary.clone())
                    );
                }
                eprintln!(
                    "{}: {}{} ({:.1} ms, {} instructions, {} presentations)",
                    report.operation,
                    report.status,
                    report
                        .return_value
                        .as_ref()
                        .map(|v| format!(", returned {}", display_value(v)))
                        .unwrap_or_default(),
                    report.duration_ms,
                    report.instructions,
                    report.frames_presented
                );
            }
            if cli.verbose
                && let Some(src) = &report.source
            {
                eprintln!(
                    "source: {} ({}, sha256={})",
                    src.display_name, src.representation, src.source_sha256
                );
            }
            for path in &report.artifacts {
                eprintln!("artifact: {}", path.display());
            }
        }
    }
    report.exit_code
}

pub fn display_value(value: &serde_json::Value) -> String {
    value
        .get("value")
        .and_then(|s| s.as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}

#[derive(PartialEq)]
struct Revision {
    bytes: Option<Vec<u8>>,
    metadata: Option<(u64, Option<std::time::SystemTime>)>,
}
fn read_revision(path: &Path) -> Revision {
    use std::io::Read;
    let metadata = std::fs::metadata(path)
        .ok()
        .filter(|m| m.is_file())
        .map(|m| (m.len(), m.modified().ok()));
    let bytes = if metadata
        .as_ref()
        .is_some_and(|(len, _)| *len <= 8 * 1024 * 1024)
    {
        std::fs::File::open(path).ok().and_then(|f| {
            let mut bytes = vec![];
            f.take(8 * 1024 * 1024 + 1).read_to_end(&mut bytes).ok()?;
            (bytes.len() <= 8 * 1024 * 1024).then_some(bytes)
        })
    } else {
        None
    };
    Revision { bytes, metadata }
}

struct Changes {
    paths: Vec<PathBuf>,
    current: Vec<Revision>,
    pending: Option<Instant>,
    checked: Instant,
}
impl Changes {
    fn new(request: &Request) -> Self {
        let paths = std::iter::once(request.file.clone())
            .chain(
                request
                    .modules
                    .iter()
                    .filter_map(|s| s.split_once('=').map(|(_, p)| PathBuf::from(p))),
            )
            .collect::<Vec<_>>();
        let current = paths.iter().map(|p| read_revision(p)).collect();
        Self {
            paths,
            current,
            pending: None,
            checked: Instant::now(),
        }
    }
    fn changed(&mut self) -> bool {
        if self.checked.elapsed() < Duration::from_millis(50) {
            return false;
        }
        self.checked = Instant::now();
        let next = self
            .paths
            .iter()
            .map(|p| read_revision(p))
            .collect::<Vec<_>>();
        if next != self.current {
            self.current = next;
            self.pending = Some(Instant::now());
        }
        if self
            .pending
            .is_some_and(|t| t.elapsed() >= Duration::from_millis(200))
        {
            self.pending = None;
            return true;
        }
        false
    }
}

fn watch(cli: &Cli, mut request: Request, views: &mut Views, cancel: &Arc<AtomicBool>) -> Report {
    let started = Instant::now();
    let mut changes = Changes::new(&request);
    let mut runs = 0;
    let mut last = None;
    loop {
        if cancel.load(Ordering::Acquire) {
            break;
        }
        request.source_bytes = changes.current.first().and_then(|r| r.bytes.clone());
        views.reset();
        let outcome = driver::execute(request.clone(), views, cli.json, cli.quiet, cancel, || {
            changes.changed()
        });
        runs += 1;
        if !cli.json && !views.graphical() {
            print_diagnostics(&outcome.report, cli.verbose);
        }
        if !cli.quiet {
            eprintln!("watch run {runs}: {}", outcome.report.status);
        }
        if let Some(path) = &cli.report
            && protect_output(path, &protected(&cli.command)).is_ok()
            && let Ok(bytes) = serde_json::to_vec_pretty(&outcome.report)
            && let Err(e) = atomic_bytes(path, &bytes)
        {
            eprintln!("cannot update watch report: {e}");
        }
        last = Some(outcome.report);
        if outcome.replaced {
            continue;
        }
        while !cancel.load(Ordering::Acquire) && !changes.changed() {
            match views.poll() {
                Ok(commands) => {
                    if commands
                        .iter()
                        .any(|c| matches!(c, hpdev_core::Control::Cancel))
                    {
                        cancel.store(true, Ordering::Release);
                    }
                }
                Err(e) => {
                    eprintln!("viewer: {e}");
                    cancel.store(true, Ordering::Release);
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    let mut report = Report::new("watch", Some(request.file));
    report.fail("cancelled", 130, "watch cancelled");
    report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
    report.data = json!({"runs": runs, "last_run": last, "viewer": views.metadata()});
    report
}

pub(crate) fn print_diagnostics(report: &Report, verbose: bool) {
    for d in &report.diagnostics {
        let position = d
            .location
            .as_ref()
            .map(|l| format!("{}:{}:{}", l.file, l.line, l.column))
            .unwrap_or_else(|| {
                report
                    .file
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            });
        eprintln!(
            "{position} {}[{}] ({}): {}",
            d.severity, d.code, d.class, d.message
        );
        if verbose && let Some(hint) = &d.hint {
            eprintln!("  hint: {hint}");
        }
    }
    if let Some(error) = &report.error {
        eprintln!("{}: {error}", report.status);
    }
    for frame in &report.stack {
        eprintln!(
            "  at {} ({}:{}:{})",
            frame.function, frame.location.file, frame.location.line, frame.location.column
        );
    }
}
