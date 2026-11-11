use crate::driver::{self, ViewOptions, Views};
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
struct Cli {
    #[arg(long, global = true)]
    json: bool,
    #[arg(short, long, global = true)]
    quiet: bool,
    /// Atomically write the same versioned JSON report to a file.
    #[arg(long, global = true)]
    report: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Validate without executing. Compatibility limitations remain distinct.
    Check(Check),
    /// Execute with the native Rust engine (scripted input by default).
    Run(Run),
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
}
#[derive(Clone, Copy, ValueEnum)]
enum Engine {
    Native,
    Firmware,
}
#[derive(Clone, Copy, ValueEnum)]
enum ArgsFormat {
    Json,
    Ppl,
}
#[derive(Clone, Copy, ValueEnum)]
enum Angle {
    Rad,
    Deg,
    Grad,
}
#[derive(Args)]
struct Source {
    file: PathBuf,
    #[arg(long, value_name = "NAME=PATH")]
    module: Vec<String>,
    #[arg(long, value_enum, default_value = "native")]
    engine: Engine,
}
#[derive(Args)]
struct Check {
    #[command(flatten)]
    source: Source,
    #[arg(long)]
    deny_warnings: bool,
    #[arg(long)]
    deny_compatibility: bool,
}
#[derive(Args)]
struct Run {
    #[command(flatten)]
    source: Source,
    #[arg(short, long)]
    entry: Option<String>,
    /// Function arguments. JSON array by default, with lossless tagged values.
    #[arg(short, long, default_value = "[]")]
    args: String,
    #[arg(long, value_enum, default_value = "json")]
    args_format: ArgsFormat,
    #[arg(long, allow_hyphen_values = true)]
    keys: Option<String>,
    #[arg(long)]
    input: Vec<String>,
    #[arg(long)]
    choose: Vec<usize>,
    #[arg(long, default_value_t = 0x5eed)]
    seed: u64,
    #[arg(long, value_enum, default_value = "rad")]
    angle: Angle,
    /// Total deadline, including input waits. Run defaults to 30s; debug has none.
    #[arg(long, allow_hyphen_values = true)]
    max_time: Option<f64>,
    #[arg(long, conflicts_with = "max_time")]
    no_time_limit: bool,
    #[arg(long, default_value_t = 5_000_000)]
    max_steps: u64,
    #[arg(long, default_value_t = 1000)]
    max_depth: usize,
    #[arg(long, default_value_t = 1_000_000)]
    max_collection_len: usize,
    #[arg(long,default_value_t=16*1024*1024)]
    max_string_bytes: usize,
    #[arg(long,default_value_t=4*1024*1024)]
    max_output_chars: usize,
    /// Allow terminal prompts after scripted input is exhausted.
    #[arg(long)]
    interactive: bool,
}
#[derive(Args)]
struct Debug {
    #[command(flatten)]
    run: Run,
    #[arg(short='b',long="break",value_parser=clap::value_parser!(usize))]
    breakpoints: Vec<usize>,
    /// Continue to the first breakpoint instead of stopping at the first statement.
    #[arg(long = "run")]
    continue_to_break: bool,
    #[arg(long, conflicts_with = "no_tui")]
    tui: bool,
    #[arg(long, conflicts_with = "tui")]
    no_tui: bool,
    #[arg(long)]
    exit_on_finish: bool,
}
#[derive(Args)]
struct Preview {
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
    #[arg(long,value_parser=["auto","truecolor","256","mono"],default_value="auto")]
    color: String,
}
#[derive(Args)]
struct Watch {
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
struct Test {
    #[arg(default_value = "hpdev.toml")]
    manifest: PathBuf,
    #[arg(long, default_value = ".hpdev/test")]
    artifacts_dir: PathBuf,
    #[arg(long)]
    update_snapshots: bool,
}
#[derive(Args)]
struct Trace {
    #[command(flatten)]
    run: Run,
    #[arg(long)]
    variables: bool,
    #[arg(long, default_value_t = 10_000)]
    max_trace_events: usize,
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
    fn request(&self, operation: &str) -> Result<Request, Box<Report>> {
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
        r.max_steps = self.max_steps;
        r.max_depth = self.max_depth;
        r.max_collection_len = self.max_collection_len;
        r.max_string_bytes = self.max_string_bytes;
        r.max_output_chars = self.max_output_chars;
        r.interactive = self.interactive;
        if r.max_time.is_some_and(|s| !s.is_finite() || s <= 0.)
            || r.max_steps == 0
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
        Command::Run(o) => Some(&o.source),
        Command::Debug(o) => Some(&o.run.source),
        Command::Preview(o) => Some(&o.run.source),
        Command::Watch(o) => Some(&o.run.source),
        Command::Trace(o) => Some(&o.run.source),
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
    if let Command::Test(o) = command {
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
    if let Command::Preview(o) = command
        && let Some(p) = &o.png
    {
        paths.push(p.clone());
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
                    r.data = json!({"text":error.to_string()});
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
            let mut r = Report::new("doctor", None);
            r.data = json!({"platform":format!("{}-{}",std::env::consts::OS,std::env::consts::ARCH),
                "native_engine":true,"graphics_renderer":true,"browser_viewer":cfg!(feature="web"),
                "terminal":{"stdin_tty":io::stdin().is_terminal(),"stderr_tty":io::stderr().is_terminal(),"term":std::env::var("TERM").ok(),"colorterm":std::env::var("COLORTERM").ok()},
                "upstream_revision":hpdev_core::UPSTREAM_REVISION,"firmware_required":false,"calculator_required":false});
            return r;
        }
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
            r.deny_warnings = o.deny_warnings;
            r.deny_compatibility = o.deny_compatibility;
            (r, ViewOptions::default())
        }
        Command::Run(o) => (
            match o.request("run") {
                Ok(r) => r,
                Err(e) => return *e,
            },
            ViewOptions::default(),
        ),
        Command::Trace(o) => {
            let mut r = match o.run.request("trace") {
                Ok(r) => r,
                Err(e) => return *e,
            };
            r.trace = true;
            r.trace_variables = o.variables;
            r.max_trace_events = o.max_trace_events;
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
    request.interactive |= options.tui || options.web;
    request.emit_frames = options.tui || options.web;
    if request.interactive && !options.web && !options.tui && !io::stdin().is_terminal() {
        let mut report = Report::new(&request.operation, Some(request.file));
        report.fail("invalid_request",2,"--interactive requires terminal stdin; use scripted --input/--choose/--keys for automation");
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
        print_diagnostics(&report);
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
            } else {
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
            print_diagnostics(&outcome.report);
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
    report.data = json!({"runs":runs,"last_run":last,"viewer":views.metadata()});
    report
}

fn print_diagnostics(report: &Report) {
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
