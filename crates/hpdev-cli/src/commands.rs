//! Subcommand implementations and human-readable formatters for expanded CLI capabilities.
use clap::{Args, Subcommand, ValueEnum};
use hpdev_core::{
    Report, Request,
    analysis::{
        analyze_compat, disassemble_program, extract_docs, format_source, inspect_ast,
        inspect_program, inspect_symbols, lint_source, list_commands, write_docs_output,
    },
    eval::{EvalOptions, ReplSession, build_settings, eval_expression},
    execution::{atomic_bytes, protect_output},
    testing::Manifest,
    tools::{
        PlotOptions, SnapshotCaptureOptions, SnapshotRunOptions, StatsOptions, bench_program,
        compute_stats, init_project, package_hpprgm, parse_csv_columns, parse_number_series,
        plot_functions, snapshot_capture, snapshot_inspect, snapshot_run,
    },
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, BufRead, IsTerminal, Write},
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::Instant,
};

#[derive(Clone, Copy, ValueEnum)]
pub enum NumFmt {
    Standard,
    Fixed,
    Sci,
    Eng,
}

impl NumFmt {
    pub fn as_str(self) -> &'static str {
        match self {
            NumFmt::Standard => "standard",
            NumFmt::Fixed => "fixed",
            NumFmt::Sci => "sci",
            NumFmt::Eng => "eng",
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub enum StatsModel {
    Linear,
    Log,
    Exp,
    Power,
    Auto,
}

impl StatsModel {
    pub fn as_str(self) -> &'static str {
        match self {
            StatsModel::Linear => "linear",
            StatsModel::Log => "log",
            StatsModel::Exp => "exp",
            StatsModel::Power => "power",
            StatsModel::Auto => "auto",
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub enum ProjectTemplate {
    Basic,
    Graphics,
    Library,
}

impl ProjectTemplate {
    pub fn as_str(self) -> &'static str {
        match self {
            ProjectTemplate::Basic => "basic",
            ProjectTemplate::Graphics => "graphics",
            ProjectTemplate::Library => "library",
        }
    }
}

#[derive(Args)]
pub struct EvalOpts {
    /// PPL expression to evaluate.
    #[arg(allow_hyphen_values = true)]
    pub expr: String,
    /// Optional PPL program file whose exported functions and globals are brought into scope.
    #[arg(short, long)]
    pub file: Option<PathBuf>,
    #[arg(long, value_name = "NAME=PATH")]
    pub module: Vec<String>,
    #[arg(long, value_enum, default_value = "rad")]
    pub angle: crate::cli::Angle,
    #[arg(long, value_enum, default_value = "standard")]
    pub format: NumFmt,
    #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(i64).range(0..=11))]
    pub digits: i64,
    #[arg(long, default_value_t = 0x5eed)]
    pub seed: u64,
    #[arg(long)]
    pub max_steps: Option<u64>,
    #[arg(long, allow_hyphen_values = true)]
    pub max_time: Option<f64>,
}

#[derive(Args)]
pub struct ReplOpts {
    /// Optional PPL program file to load into the session.
    #[arg(long)]
    pub load: Option<PathBuf>,
    /// Restore initial session state from a snapshot JSON file.
    #[arg(long)]
    pub state: Option<PathBuf>,
    /// Save final session state to a snapshot JSON file on exit.
    #[arg(long)]
    pub save_state: Option<PathBuf>,
    #[arg(long, value_enum, default_value = "rad")]
    pub angle: crate::cli::Angle,
    #[arg(long, value_enum, default_value = "standard")]
    pub format: NumFmt,
    #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(i64).range(0..=11))]
    pub digits: i64,
    #[arg(long, default_value_t = 0x5eed)]
    pub seed: u64,
    #[arg(long)]
    pub max_steps: Option<u64>,
    #[arg(long, allow_hyphen_values = true)]
    pub max_time: Option<f64>,
}

#[derive(Args)]
pub struct CompatOpts {
    /// PPL program or .hpprgm container to analyze (omit with --commands).
    pub file: Option<PathBuf>,
    #[arg(long, value_name = "NAME=PATH")]
    pub module: Vec<String>,
    /// Exit non-zero if unsupported builtins, catalog commands, or #cas blocks are used.
    #[arg(long)]
    pub strict: bool,
    /// List known HP Prime commands and their native support status.
    #[arg(long)]
    pub commands: bool,
    #[arg(long)]
    pub category: Option<String>,
    #[arg(long)]
    pub status: Option<String>,
    #[arg(long)]
    pub search: Option<String>,
    /// Include documented but unimplemented catalog commands.
    #[arg(long)]
    pub include_catalog: bool,
}

#[derive(Args)]
pub struct AstOpts {
    pub file: PathBuf,
    /// Include the full lexer token stream (with comments and soft keywords).
    #[arg(long)]
    pub tokens: bool,
}

#[derive(Args)]
pub struct SymbolsOpts {
    pub file: PathBuf,
    #[arg(long, value_name = "NAME=PATH")]
    pub module: Vec<String>,
    /// Show only EXPORT functions and EXPORT globals.
    #[arg(long)]
    pub exported_only: bool,
}

#[derive(Args)]
pub struct DisasmOpts {
    pub file: PathBuf,
    #[arg(long, value_name = "NAME=PATH")]
    pub module: Vec<String>,
    /// Disassemble only the named function.
    #[arg(short, long)]
    pub function: Option<String>,
}

#[derive(Args)]
pub struct LintOpts {
    pub file: PathBuf,
    #[arg(long, value_name = "NAME=PATH")]
    pub module: Vec<String>,
    /// Exit with code 1 if any warning diagnostic is reported.
    #[arg(long)]
    pub deny_warnings: bool,
}

#[derive(Args)]
pub struct FormatOpts {
    pub file: PathBuf,
    /// Check whether the file is already formatted (exit 1 if changes are needed).
    #[arg(long, conflicts_with_all = ["write", "output"])]
    pub check: bool,
    /// Format the file in place atomically.
    #[arg(short, long, conflicts_with_all = ["check", "output"])]
    pub write: bool,
    /// Write formatted source to a separate file atomically.
    #[arg(short, long, conflicts_with_all = ["check", "write"])]
    pub output: Option<PathBuf>,
    /// Spaces per indentation level (1-8).
    #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u8).range(1..=8))]
    pub indent: u8,
    /// Normalize structural PPL keywords to uppercase.
    #[arg(long)]
    pub uppercase_keywords: bool,
}

#[derive(Args)]
pub struct InspectOpts {
    pub file: PathBuf,
    #[arg(long, value_name = "NAME=PATH")]
    pub module: Vec<String>,
}

#[derive(Args)]
pub struct BenchOpts {
    #[command(flatten)]
    pub run: crate::cli::Run,
    /// Unmeasured warmup iterations (0-1000).
    #[arg(long, default_value_t = 3)]
    pub warmup: usize,
    /// Measured benchmark iterations (1-10000).
    #[arg(short = 'n', long, default_value_t = 20)]
    pub iterations: usize,
}

#[derive(Args)]
pub struct ProfileOpts {
    #[command(flatten)]
    pub run: crate::cli::Run,
    /// Maximum number of hot lines to show in human output.
    #[arg(long, default_value_t = 15)]
    pub top: usize,
}

#[derive(Args)]
pub struct CoverageOpts {
    /// PPL program file to measure coverage for (or omit when using --manifest).
    pub file: Option<PathBuf>,
    #[arg(long, value_name = "NAME=PATH")]
    pub module: Vec<String>,
    /// Run all `run` cases from a test manifest and aggregate coverage per file.
    #[arg(long, conflicts_with = "file")]
    pub manifest: Option<PathBuf>,
    #[arg(short, long)]
    pub entry: Option<String>,
    #[arg(short, long, default_value = "[]")]
    pub args: String,
    #[arg(long, value_enum, default_value = "json")]
    pub args_format: crate::cli::ArgsFormat,
    #[arg(long, allow_hyphen_values = true)]
    pub keys: Option<String>,
    #[arg(long)]
    pub input: Vec<String>,
    #[arg(long)]
    pub choose: Vec<usize>,
    #[arg(long, default_value_t = 0x5eed)]
    pub seed: u64,
    #[arg(long, value_enum, default_value = "rad")]
    pub angle: crate::cli::Angle,
    #[arg(long, allow_hyphen_values = true)]
    pub max_time: Option<f64>,
    #[arg(long)]
    pub max_steps: Option<u64>,
    /// Fail with policy_failed (exit 1) if line coverage percentage is below PCT.
    #[arg(long)]
    pub fail_under: Option<f64>,
}

#[derive(Args)]
pub struct PlotOpts {
    /// One or more PPL expressions in `X` (e.g. "SIN(X)" "X^2/10").
    #[arg(required = true)]
    pub expr: Vec<String>,
    #[arg(long, default_value_t = -10.0, allow_hyphen_values = true)]
    pub xmin: f64,
    #[arg(long, default_value_t = 10.0, allow_hyphen_values = true)]
    pub xmax: f64,
    #[arg(long, allow_hyphen_values = true)]
    pub ymin: Option<f64>,
    #[arg(long, allow_hyphen_values = true)]
    pub ymax: Option<f64>,
    #[arg(long, default_value_t = 320)]
    pub samples: usize,
    #[arg(long, default_value_t = 320)]
    pub width: usize,
    #[arg(long, default_value_t = 240)]
    pub height: usize,
    #[arg(long, value_enum, default_value = "rad")]
    pub angle: crate::cli::Angle,
    /// Write a PNG plot image to PATH.
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Render an ASCII plot to stdout even when --output is set.
    #[arg(long)]
    pub ascii: bool,
    /// Hide background gridlines in PNG output.
    #[arg(long)]
    pub no_grid: bool,
    #[arg(long)]
    pub title: Option<String>,
}

#[derive(Args)]
pub struct StatsOpts {
    /// Primary numeric series (comma/space separated, JSON array, or PPL list).
    #[arg(long, allow_hyphen_values = true)]
    pub x: Option<String>,
    /// Optional dependent numeric series for 2-variable stats and regression.
    #[arg(long, allow_hyphen_values = true)]
    pub y: Option<String>,
    /// Load numeric columns from a CSV file instead of --x/--y.
    #[arg(long, conflicts_with_all = ["x", "y"])]
    pub csv: Option<PathBuf>,
    /// 1-based column index or header name for X in --csv.
    #[arg(long, default_value = "1")]
    pub x_col: String,
    /// 1-based column index or header name for Y in --csv.
    #[arg(long)]
    pub y_col: Option<String>,
    #[arg(long, value_enum, default_value = "linear")]
    pub model: StatsModel,
    /// Predict Y at X using the fitted 2-variable model.
    #[arg(long, allow_hyphen_values = true)]
    pub predict_y: Option<f64>,
    /// Predict X at Y using the fitted 2-variable model.
    #[arg(long, allow_hyphen_values = true)]
    pub predict_x: Option<f64>,
}

#[derive(Args)]
pub struct InitOpts {
    /// Target directory to initialize (created if needed).
    #[arg(default_value = ".")]
    pub dir: PathBuf,
    /// Program/entry function name (defaults to directory name or MAIN).
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long, value_enum, default_value = "basic")]
    pub template: ProjectTemplate,
    /// Overwrite existing files in the target directory.
    #[arg(long)]
    pub force: bool,
}

#[derive(Args)]
pub struct PackageOpts {
    /// Input `.ppl` source (when packing) or `.hpprgm` container (with --unpack).
    pub input: PathBuf,
    /// Output `.hpprgm` (or `.ppl` with --unpack) path.
    #[arg(short, long)]
    pub output: PathBuf,
    /// Extract UTF-8 `.ppl` source from a binary `.hpprgm` container.
    #[arg(long)]
    pub unpack: bool,
    /// Skip compiler validation before packing into `.hpprgm`.
    #[arg(long)]
    pub no_check: bool,
}

#[derive(Args)]
pub struct DocsOpts {
    /// Optional command name or substring to look up.
    pub query: Option<String>,
    /// Extract API documentation from a PPL source file.
    #[arg(short, long)]
    pub file: Option<PathBuf>,
    #[arg(long)]
    pub category: Option<String>,
    #[arg(long)]
    pub status: Option<String>,
    /// Include unimplemented catalog commands in lookup results.
    #[arg(long)]
    pub include_catalog: bool,
    /// Write generated Markdown documentation to PATH atomically.
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

#[derive(Args)]
pub struct SnapshotOpts {
    #[command(subcommand)]
    pub action: SnapshotAction,
}

#[derive(Subcommand)]
pub enum SnapshotAction {
    /// Capture calculator session state after executing a program and/or expressions.
    Capture {
        /// Optional PPL program file to run before capturing state.
        file: Option<PathBuf>,
        /// Output snapshot state JSON path.
        #[arg(short, long)]
        output: PathBuf,
        /// Additional PPL expressions/assignments to evaluate before saving.
        #[arg(long = "expr", allow_hyphen_values = true)]
        exprs: Vec<String>,
        #[arg(short, long)]
        entry: Option<String>,
        #[arg(short, long, default_value = "[]")]
        args: String,
        #[arg(long, value_enum, default_value = "json")]
        args_format: crate::cli::ArgsFormat,
        #[arg(long, value_enum, default_value = "rad")]
        angle: crate::cli::Angle,
        #[arg(long, default_value_t = 0x5eed)]
        seed: u64,
        #[arg(long)]
        max_steps: Option<u64>,
        #[arg(long, allow_hyphen_values = true)]
        max_time: Option<f64>,
    },
    /// Inspect the contents of a saved snapshot JSON file.
    Inspect { state: PathBuf },
    /// Restore a saved snapshot and run a program or expression against that state.
    Run {
        /// Optional PPL program file to execute against the restored state.
        file: Option<PathBuf>,
        /// Input snapshot state JSON path.
        #[arg(long)]
        state: PathBuf,
        /// Optionally write updated session state to PATH.
        #[arg(long)]
        save_state: Option<PathBuf>,
        /// Optional PPL expression/statement to evaluate after (or instead of) FILE.
        #[arg(long, allow_hyphen_values = true)]
        expr: Option<String>,
        #[arg(short, long)]
        entry: Option<String>,
        #[arg(short, long, default_value = "[]")]
        args: String,
        #[arg(long, value_enum, default_value = "json")]
        args_format: crate::cli::ArgsFormat,
        #[arg(long, value_enum)]
        angle: Option<crate::cli::Angle>,
        #[arg(long)]
        seed: Option<u64>,
        #[arg(long)]
        max_steps: Option<u64>,
        #[arg(long, allow_hyphen_values = true)]
        max_time: Option<f64>,
    },
}

fn angle_str(a: crate::cli::Angle) -> &'static str {
    match a {
        crate::cli::Angle::Rad => "rad",
        crate::cli::Angle::Deg => "deg",
        crate::cli::Angle::Grad => "grad",
    }
}

fn args_format_str(f: crate::cli::ArgsFormat) -> &'static str {
    match f {
        crate::cli::ArgsFormat::Json => "json",
        crate::cli::ArgsFormat::Ppl => "ppl",
    }
}

pub fn run_eval(o: &EvalOpts, json_out: bool, quiet: bool) -> Report {
    let report = eval_expression(EvalOptions {
        expr: o.expr.clone(),
        file: o.file.clone(),
        modules: o.module.clone(),
        angle: angle_str(o.angle).into(),
        format: o.format.as_str().into(),
        digits: o.digits,
        seed: o.seed,
        max_steps: o.max_steps.or(Some(5_000_000)),
        max_time: o.max_time.or(Some(30.0)),
    });
    if !json_out && report.exit_code == 0 {
        for line in &report.output {
            println!("{line}");
        }
        if let Some(disp) = report.data["display"].as_str() {
            println!("{disp}");
        }
        if !quiet {
            eprintln!(
                "eval: {} ({:.1} ms, {} instructions)",
                report.status, report.duration_ms, report.instructions
            );
        }
    }
    report
}

pub fn run_repl(o: &ReplOpts, json_out: bool, quiet: bool, cancel: &Arc<AtomicBool>) -> Report {
    let started = Instant::now();
    let mut report = Report::new("repl", o.load.clone());
    if let Some(save) = &o.save_state {
        let mut protected = vec![];
        if let Some(l) = &o.load {
            protected.push(l.clone());
        }
        if let Some(st) = &o.state {
            protected.push(st.clone());
        }
        if let Err(e) = protect_output(save, &protected) {
            report.fail("invalid_request", 2, e);
            return report;
        }
    }
    let settings = match build_settings(angle_str(o.angle), o.format.as_str(), o.digits) {
        Ok(s) => s,
        Err(e) => {
            report.fail("invalid_request", 2, e);
            return report;
        }
    };
    let mut session = ReplSession::new_bounded(
        settings,
        o.seed,
        o.max_steps.or(Some(5_000_000)),
        o.max_time,
    );
    if let Some(state_path) = &o.state {
        let text = match std::fs::read_to_string(state_path) {
            Ok(t) => t,
            Err(e) => {
                report.fail("io_error", 5, format!("{}: {e}", state_path.display()));
                return report;
            }
        };
        let state = match serde_json::from_str(&text) {
            Ok(s) => s,
            Err(e) => {
                report.fail("invalid_request", 2, format!("invalid state JSON: {e}"));
                return report;
            }
        };
        if let Err(e) = session.import_state(&state) {
            report.fail("invalid_request", 2, e);
            return report;
        }
    }
    if let Some(load_path) = &o.load
        && let Err(e) = session.load_program(load_path)
    {
        report.fail("invalid_source", 1, e);
        return report;
    }

    let interactive = !json_out && io::stdin().is_terminal() && io::stderr().is_terminal();
    if interactive && !quiet {
        eprintln!("hpdev repl — type PPL expressions or :help (:quit to exit)");
    }

    let mut history = Vec::new();
    let mut total_steps = 0u64;
    let mut last_val = None;
    let mut all_output = Vec::new();
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();
    loop {
        if cancel.load(std::sync::atomic::Ordering::Acquire) {
            report.fail("cancelled", 130, "repl cancelled");
            break;
        }
        if interactive {
            eprint!("ppl> ");
            let _ = io::stderr().flush();
        }
        let Some(next_line) = lines.next() else {
            break;
        };
        let Ok(line) = next_line else {
            break;
        };
        let outcome = session.eval_line(&line);
        if outcome.kind == "quit" {
            break;
        }
        if outcome.kind == "empty" {
            continue;
        }
        total_steps += outcome.steps;
        all_output.extend(outcome.output.clone());
        if outcome.value.is_some() {
            last_val = outcome.value.clone();
        }
        if !json_out {
            for out_line in &outcome.output {
                println!("{out_line}");
            }
            if let Some(disp) = &outcome.display {
                println!("{disp}");
            } else if let Some(msg) = &outcome.message {
                if outcome.exit_code == 0 {
                    println!("{msg}");
                } else {
                    eprintln!("error: {msg}");
                }
            }
        }
        history.push(json!({
            "input": line,
            "outcome": outcome,
        }));
    }

    let final_state = session.export_state();
    if let Some(save) = &o.save_state {
        match serde_json::to_vec_pretty(&final_state)
            .map_err(|e| e.to_string())
            .and_then(|b| atomic_bytes(save, &b).map_err(|e| e.to_string()))
        {
            Ok(()) => report.artifacts.push(save.clone()),
            Err(e) => {
                report.fail(
                    "io_error",
                    5,
                    format!("cannot write {}: {e}", save.display()),
                );
                return report;
            }
        }
    }

    report.output = all_output;
    report.return_value = last_val;
    report.instructions = total_steps;
    report.data = json!({
        "history": history,
        "state": final_state,
    });
    report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
    report
}

pub fn run_compat(o: &CompatOpts, json_out: bool, quiet: bool) -> Report {
    let report = if o.commands || o.file.is_none() {
        if !o.commands && o.file.is_none() {
            let mut r = Report::new("compat", None);
            r.fail(
                "invalid_request",
                2,
                "provide a program FILE or pass --commands to list supported commands",
            );
            return r;
        }
        list_commands(
            o.category.as_deref(),
            o.status.as_deref(),
            o.search.as_deref(),
            o.include_catalog,
        )
    } else {
        analyze_compat(o.file.as_deref().unwrap(), &o.module, o.strict)
    };

    if !json_out && report.exit_code <= 1 {
        if let Some(cmds) = report.data["commands"].as_array()
            && report.data.get("builtins_total").is_some()
        {
            println!("{:<16} {:<11} {:<10} status", "command", "category", "args");
            for c in cmds {
                println!(
                    "{:<16} {:<11} {:<10} {}",
                    c["name"].as_str().unwrap_or(""),
                    c["category"].as_str().unwrap_or(""),
                    c["arity"].as_str().unwrap_or(""),
                    c["status"].as_str().unwrap_or("")
                );
            }
            if !quiet {
                eprintln!(
                    "{} command(s) shown ({} builtins: {} implemented, {} approximate, {} unsupported; {} catalog)",
                    report.data["count"],
                    report.data["builtins_total"],
                    report.data["implemented"],
                    report.data["approximate"],
                    report.data["unsupported"],
                    report.data["catalog_total"]
                );
            }
        } else if let Some(cmds) = report.data["commands"].as_array() {
            println!(
                "compatible: {} (implemented: {}, approximate: {}, unsupported: {}, catalog: {}, #cas blocks: {})",
                report.data["compatible"],
                report.data["implemented_count"],
                report.data["approximate_count"],
                report.data["unsupported_count"],
                report.data["catalog_count"],
                report.data["cas_block_count"]
            );
            for c in cmds {
                let lines: Vec<String> = c["locations"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|l| l["line"].as_u64().map(|n| n.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                println!(
                    "  {:<16} {:<14} {:<12} line(s): {}",
                    c["name"].as_str().unwrap_or(""),
                    c["category"].as_str().unwrap_or(""),
                    c["status"].as_str().unwrap_or(""),
                    lines.join(",")
                );
            }
        }
    }
    report
}

pub fn run_ast(o: &AstOpts, json_out: bool) -> Report {
    let report = inspect_ast(&o.file, o.tokens);
    if !json_out {
        if let Some(tree) = report.data["tree"].as_str() {
            print!("{tree}");
        }
        if o.tokens
            && let Some(tokens) = report.data["tokens"].as_array()
        {
            println!("\nTokens ({}):", tokens.len());
            for tok in tokens {
                println!(
                    "  {:>4}:{:<3} {:<24} {:?}",
                    tok["location"]["line"],
                    tok["location"]["column"],
                    tok["kind"].as_str().unwrap_or(""),
                    tok["text"].as_str().unwrap_or("")
                );
            }
        }
    }
    report
}

pub fn run_symbols(o: &SymbolsOpts, json_out: bool, quiet: bool) -> Report {
    let report = inspect_symbols(&o.file, &o.module, o.exported_only);
    if !json_out && report.exit_code == 0 {
        if let Some(fns) = report.data["functions"].as_array() {
            println!("Functions ({}):", fns.len());
            for f in fns {
                let vis = if f["is_key"] == true {
                    "KEY"
                } else if f["exported"] == true {
                    "EXPORT"
                } else if f["is_local"] == true {
                    "LOCAL"
                } else {
                    "PRIVATE"
                };
                let params: Vec<&str> = f["params"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();
                println!(
                    "  {:<8} {}({})  line {} ({} ops)",
                    vis,
                    f["name"].as_str().unwrap_or(""),
                    params.join(", "),
                    f["location"]["line"],
                    f["bytecode_len"]
                );
            }
        }
        if let Some(globs) = report.data["globals"].as_array()
            && !globs.is_empty()
        {
            println!("Globals ({}):", globs.len());
            for g in globs {
                let vis = if g["exported"] == true {
                    "EXPORT"
                } else {
                    "LOCAL"
                };
                println!(
                    "  {:<8} {}  line {}",
                    vis,
                    g["name"].as_str().unwrap_or(""),
                    g["location"]["line"]
                );
            }
        }
        if !quiet {
            eprintln!("symbols: {} ({:.1} ms)", report.status, report.duration_ms);
        }
    }
    report
}

pub fn run_disasm(o: &DisasmOpts, json_out: bool) -> Report {
    let report = disassemble_program(&o.file, &o.module, o.function.as_deref());
    if !json_out
        && report.exit_code == 0
        && let Some(text) = report.data["text"].as_str()
    {
        print!("{text}");
    }
    report
}

pub fn run_lint(o: &LintOpts, json_out: bool, quiet: bool) -> Report {
    let report = lint_source(&o.file, &o.module, o.deny_warnings);
    if !json_out && !quiet {
        let s = &report.data["summary"];
        if s.is_object() {
            eprintln!(
                "lint: {} ({} error(s), {} warning(s), {} note(s))",
                report.status, s["errors"], s["warnings"], s["notes"]
            );
        }
    }
    report
}

pub fn run_format(o: &FormatOpts, json_out: bool, quiet: bool) -> Report {
    let started = Instant::now();
    if let Some(out) = &o.output
        && let Err(e) = protect_output(out, std::slice::from_ref(&o.file))
    {
        let mut r = Report::new("format", Some(o.file.clone()));
        r.fail("invalid_request", 2, e);
        return r;
    }
    match format_source(&o.file, o.indent as usize, o.uppercase_keywords) {
        Err(err_report) => *err_report,
        Ok(res) => {
            let mut report = Report::new("format", Some(o.file.clone()));
            if o.check {
                if res.changed {
                    report.fail(
                        "policy_failed",
                        1,
                        format!("{} is not formatted", o.file.display()),
                    );
                }
                if !json_out && res.changed {
                    for line in &res.diff_lines {
                        eprintln!("{line}");
                    }
                }
            } else if o.write {
                if res.changed {
                    if let Err(e) = atomic_bytes(&o.file, res.formatted.as_bytes()) {
                        report.fail(
                            "io_error",
                            5,
                            format!("cannot write {}: {e}", o.file.display()),
                        );
                        return report;
                    }
                    report.artifacts.push(o.file.clone());
                }
                if !json_out && !quiet {
                    eprintln!(
                        "format: {} ({})",
                        o.file.display(),
                        if res.changed { "updated" } else { "unchanged" }
                    );
                }
            } else if let Some(out) = &o.output {
                if let Err(e) = atomic_bytes(out, res.formatted.as_bytes()) {
                    report.fail(
                        "io_error",
                        5,
                        format!("cannot write {}: {e}", out.display()),
                    );
                    return report;
                }
                report.artifacts.push(out.clone());
            } else if !json_out {
                print!("{}", res.formatted);
            }
            report.data = json!({
                "changed": res.changed,
                "diff": res.diff_lines,
                "formatted": if json_out { Value::String(res.formatted) } else { Value::Null },
            });
            report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
            report
        }
    }
}

pub fn run_inspect(o: &InspectOpts, json_out: bool) -> Report {
    let report = inspect_program(&o.file, &o.module);
    if !json_out && report.exit_code == 0 {
        println!(
            "{}",
            serde_json::to_string_pretty(&report.data).expect("JSON")
        );
    }
    report
}

pub fn run_bench(o: &BenchOpts, json_out: bool) -> Report {
    let request = match o.run.request("bench") {
        Ok(r) => r,
        Err(e) => return *e,
    };
    let report = bench_program(request, o.warmup, o.iterations);
    if !json_out && report.exit_code == 0 {
        let d = &report.data;
        println!(
            "bench {} ({}): {} instructions | min {:.3} ms | median {:.3} ms | mean {:.3} ms | p95 {:.3} ms | max {:.3} ms | {:.1} runs/s",
            report.entry.as_deref().unwrap_or("?"),
            o.iterations,
            d["instructions"],
            d["min_ms"].as_f64().unwrap_or(0.0),
            d["median_ms"].as_f64().unwrap_or(0.0),
            d["mean_ms"].as_f64().unwrap_or(0.0),
            d["p95_ms"].as_f64().unwrap_or(0.0),
            d["max_ms"].as_f64().unwrap_or(0.0),
            d["runs_per_sec"].as_f64().unwrap_or(0.0),
        );
    }
    report
}

pub fn run_profile(
    o: &ProfileOpts,
    json_out: bool,
    quiet: bool,
    cancel: &Arc<AtomicBool>,
) -> Report {
    let mut request = match o.run.request("profile") {
        Ok(r) => r,
        Err(e) => return *e,
    };
    request.profile = true;
    let mut views = crate::driver::Views::empty();
    let outcome = crate::driver::execute(request, &mut views, json_out, quiet, cancel, || false);
    let report = outcome.report;
    if !json_out && report.exit_code == 0 {
        let p = &report.data["profile"];
        println!("Total instructions: {}", p["total_instructions"]);
        if let Some(fns) = p["functions"].as_array() {
            println!("\nBy function:");
            println!(
                "  {:<20} {:>10} {:>12} {:>7} {:>10}",
                "function", "stmts", "instrs", "pct", "time(ms)"
            );
            for f in fns {
                println!(
                    "  {:<20} {:>10} {:>12} {:>6.2}% {:>10.3}",
                    f["function"].as_str().unwrap_or(""),
                    f["statements"].as_u64().unwrap_or(0),
                    f["instructions"].as_u64().unwrap_or(0),
                    f["pct_instructions"].as_f64().unwrap_or(0.0),
                    f["duration_ms"].as_f64().unwrap_or(0.0),
                );
            }
        }
        if let Some(lines) = p["hot_lines"].as_array()
            && !lines.is_empty()
        {
            println!("\nTop {} hot lines:", o.top);
            for l in lines.iter().take(o.top) {
                println!(
                    "  {}:{} ({})  hits={}  instrs={}  {:.3} ms",
                    l["file"].as_str().unwrap_or(""),
                    l["line"].as_u64().unwrap_or(0),
                    l["function"].as_str().unwrap_or(""),
                    l["hits"].as_u64().unwrap_or(0),
                    l["instructions"].as_u64().unwrap_or(0),
                    l["duration_ms"].as_f64().unwrap_or(0.0),
                );
            }
        }
    }
    report
}

pub fn run_coverage(
    o: &CoverageOpts,
    json_out: bool,
    quiet: bool,
    cancel: &Arc<AtomicBool>,
) -> Report {
    let started = Instant::now();
    if let Some(pct) = o.fail_under
        && (!pct.is_finite() || !(0.0..=100.0).contains(&pct))
    {
        let mut r = Report::new("coverage", o.file.clone());
        r.fail(
            "invalid_request",
            2,
            "--fail-under must be between 0 and 100",
        );
        return r;
    }

    if let Some(manifest_path) = &o.manifest {
        let text = match std::fs::read_to_string(manifest_path) {
            Ok(t) => t,
            Err(e) => {
                let mut r = Report::new("coverage", Some(manifest_path.clone()));
                r.fail("io_error", 5, e.to_string());
                return r;
            }
        };
        let manifest: Manifest = match toml::from_str(&text) {
            Ok(m) => m,
            Err(e) => {
                let mut r = Report::new("coverage", Some(manifest_path.clone()));
                r.fail("invalid_request", 2, e.to_string());
                return r;
            }
        };
        let root = manifest_path.parent().unwrap_or(Path::new("."));
        let mut file_cov: BTreeMap<String, (BTreeSet<u64>, BTreeSet<u64>)> = BTreeMap::new();
        let mut case_reports = Vec::new();
        for case in manifest.tests {
            if case.operation != "run" {
                continue;
            }
            let req = Request {
                operation: "coverage".into(),
                file: root.join(&case.file),
                entry: case.entry,
                args: case.args,
                keys: case.keys,
                inputs: case.inputs,
                choices: case.choices,
                seed: case.seed.unwrap_or(0x5eed),
                max_time: case.max_time.or(Some(30.0)),
                max_steps: case.max_steps.or(Some(5_000_000)),
                coverage: true,
                ..Default::default()
            };
            let mut views = crate::driver::Views::empty();
            let out = crate::driver::execute(req, &mut views, true, true, cancel, || false);
            let cov = &out.report.data["coverage"];
            if cov.is_object() {
                let fname = cov["file"].as_str().unwrap_or("unknown").to_owned();
                let entry = file_cov.entry(fname).or_default();
                if let Some(cl) = cov["covered_lines"].as_array() {
                    for l in cl.iter().filter_map(|v| v.as_u64()) {
                        entry.0.insert(l);
                    }
                }
                if let Some(ul) = cov["uncovered_lines"].as_array() {
                    for l in ul.iter().filter_map(|v| v.as_u64()) {
                        entry.1.insert(l);
                    }
                }
            }
            case_reports.push(json!({
                "name": case.name,
                "status": out.report.status,
                "coverage": cov,
            }));
        }
        let mut total_lines = 0usize;
        let mut covered_lines = 0usize;
        let mut files_summary = Vec::new();
        for (fname, (cov_set, uncov_set)) in file_cov {
            let all: BTreeSet<u64> = cov_set.union(&uncov_set).copied().collect();
            let uncov: Vec<u64> = all.difference(&cov_set).copied().collect();
            total_lines += all.len();
            covered_lines += cov_set.len();
            let pct = if all.is_empty() {
                100.0
            } else {
                (cov_set.len() as f64 / all.len() as f64) * 100.0
            };
            files_summary.push(json!({
                "file": fname,
                "lines_total": all.len(),
                "lines_covered": cov_set.len(),
                "line_pct": (pct * 100.0).round() / 100.0,
                "uncovered_lines": uncov,
            }));
        }
        let overall_pct = if total_lines == 0 {
            100.0
        } else {
            (covered_lines as f64 / total_lines as f64) * 100.0
        };
        let mut report = Report::new("coverage", Some(manifest_path.clone()));
        report.data = json!({
            "lines_total": total_lines,
            "lines_covered": covered_lines,
            "line_pct": (overall_pct * 100.0).round() / 100.0,
            "files": files_summary,
            "cases": case_reports,
        });
        if let Some(min_pct) = o.fail_under
            && overall_pct < min_pct
        {
            report.fail(
                "policy_failed",
                1,
                format!("line coverage {overall_pct:.2}% is below --fail-under {min_pct:.2}%"),
            );
        }
        if !json_out {
            println!("Coverage: {covered_lines}/{total_lines} lines ({overall_pct:.2}%)");
            if let Some(files) = report.data["files"].as_array() {
                for f in files {
                    println!(
                        "  {}  {}/{} lines ({:.2}%)  uncovered: {:?}",
                        f["file"].as_str().unwrap_or(""),
                        f["lines_covered"],
                        f["lines_total"],
                        f["line_pct"].as_f64().unwrap_or(0.0),
                        f["uncovered_lines"]
                    );
                }
            }
        }
        report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
        return report;
    }

    let Some(file) = &o.file else {
        let mut r = Report::new("coverage", None);
        r.fail("invalid_request", 2, "provide a FILE or --manifest PATH");
        return r;
    };
    let request = Request {
        operation: "coverage".into(),
        file: file.clone(),
        modules: o.module.clone(),
        entry: o.entry.clone(),
        args: o.args.clone(),
        args_format: args_format_str(o.args_format).into(),
        keys: o.keys.clone(),
        inputs: o.input.clone(),
        choices: o.choose.clone(),
        seed: o.seed,
        angle: angle_str(o.angle).into(),
        max_time: o.max_time.or(Some(30.0)),
        max_steps: o.max_steps.or(Some(5_000_000)),
        coverage: true,
        ..Default::default()
    };
    let mut views = crate::driver::Views::empty();
    let mut report =
        crate::driver::execute(request, &mut views, json_out, quiet, cancel, || false).report;
    if report.exit_code == 0 {
        let cov = report.data["coverage"].clone();
        let line_pct = cov["line_pct"].as_f64().unwrap_or(0.0);
        if let Some(min_pct) = o.fail_under
            && line_pct < min_pct
        {
            report.fail(
                "policy_failed",
                1,
                format!("line coverage {line_pct:.2}% is below --fail-under {min_pct:.2}%"),
            );
        }
        if !json_out {
            println!(
                "Coverage {}: {}/{} lines ({:.2}%), {}/{} statements ({:.2}%), {}/{} functions ({:.2}%)",
                cov["file"].as_str().unwrap_or(""),
                cov["lines_covered"],
                cov["lines_total"],
                cov["line_pct"].as_f64().unwrap_or(0.0),
                cov["statements_covered"],
                cov["statements_total"],
                cov["statement_pct"].as_f64().unwrap_or(0.0),
                cov["functions_covered"],
                cov["functions_total"],
                cov["function_pct"].as_f64().unwrap_or(0.0),
            );
            if let Some(uncov) = cov["uncovered_lines"].as_array()
                && !uncov.is_empty()
            {
                let lines: Vec<String> = uncov
                    .iter()
                    .filter_map(|v| v.as_u64().map(|n| n.to_string()))
                    .collect();
                println!("Uncovered lines: {}", lines.join(", "));
            }
        }
    }
    report
}

pub fn run_plot(o: &PlotOpts, json_out: bool, quiet: bool) -> Report {
    let report = plot_functions(PlotOptions {
        expressions: o.expr.clone(),
        xmin: o.xmin,
        xmax: o.xmax,
        ymin: o.ymin,
        ymax: o.ymax,
        samples: o.samples,
        width: o.width,
        height: o.height,
        angle: angle_str(o.angle).into(),
        output: o.output.clone(),
        ascii: o.ascii,
        grid: !o.no_grid,
        title: o.title.clone(),
    });
    if !json_out && report.exit_code == 0 {
        if let Some(ascii) = report.data["ascii"].as_str() {
            print!("{ascii}");
        }
        if !quiet {
            let b = &report.data["bounds"];
            eprintln!(
                "plot: x=[{:.3}, {:.3}], y=[{:.3}, {:.3}] ({} instructions)",
                b["xmin"].as_f64().unwrap_or(0.0),
                b["xmax"].as_f64().unwrap_or(0.0),
                b["ymin"].as_f64().unwrap_or(0.0),
                b["ymax"].as_f64().unwrap_or(0.0),
                report.instructions
            );
        }
    }
    report
}

pub fn run_stats(o: &StatsOpts, json_out: bool) -> Report {
    let (xs, ys) = if let Some(csv) = &o.csv {
        match parse_csv_columns(csv, &o.x_col, o.y_col.as_deref()) {
            Ok(v) => v,
            Err((code, msg)) => {
                let mut r = Report::new("stats", Some(csv.clone()));
                r.fail(
                    if code == 5 {
                        "io_error"
                    } else {
                        "invalid_request"
                    },
                    code,
                    msg,
                );
                return r;
            }
        }
    } else {
        let Some(x_raw) = &o.x else {
            let mut r = Report::new("stats", None);
            r.fail("invalid_request", 2, "provide --x VALUES or --csv PATH");
            return r;
        };
        let xs = match parse_number_series(x_raw) {
            Ok(v) => v,
            Err(e) => {
                let mut r = Report::new("stats", None);
                r.fail("invalid_request", 2, e);
                return r;
            }
        };
        let ys = match &o.y {
            Some(y_raw) => match parse_number_series(y_raw) {
                Ok(v) => Some(v),
                Err(e) => {
                    let mut r = Report::new("stats", None);
                    r.fail("invalid_request", 2, e);
                    return r;
                }
            },
            None => None,
        };
        (xs, ys)
    };
    let report = compute_stats(StatsOptions {
        x: xs,
        y: ys,
        model: o.model.as_str().into(),
        predict_y_at: o.predict_y,
        predict_x_at: o.predict_x,
    });
    if !json_out && report.exit_code == 0 {
        let d = &report.data;
        let x = &d["x"];
        println!(
            "X (n={}): mean={:.6}  sd(s)={}  sd(p)={:.6}  min={}  Q1={}  med={}  Q3={}  max={}",
            x["n"],
            x["mean"].as_f64().unwrap_or(0.0),
            x["sample_sd"]
                .as_f64()
                .map(|v| format!("{v:.6}"))
                .unwrap_or_else(|| "n/a".into()),
            x["pop_sd"].as_f64().unwrap_or(0.0),
            x["min"],
            x["q1"],
            x["median"],
            x["q3"],
            x["max"]
        );
        if d["mode"] == "2-var" {
            let y = &d["y"];
            println!(
                "Y (n={}): mean={:.6}  sd(s)={}  sd(p)={:.6}  min={}  Q1={}  med={}  Q3={}  max={}",
                y["n"],
                y["mean"].as_f64().unwrap_or(0.0),
                y["sample_sd"]
                    .as_f64()
                    .map(|v| format!("{v:.6}"))
                    .unwrap_or_else(|| "n/a".into()),
                y["pop_sd"].as_f64().unwrap_or(0.0),
                y["min"],
                y["q1"],
                y["median"],
                y["q3"],
                y["max"]
            );
            let reg = &d["regression"];
            println!(
                "Regression ({}): {}  r={:.6}  r²={:.6}",
                reg["model"].as_str().unwrap_or(""),
                reg["equation"].as_str().unwrap_or(""),
                reg["r"].as_f64().unwrap_or(0.0),
                reg["r_squared"].as_f64().unwrap_or(0.0),
            );
            if let Some(py) = reg["pred_y"].as_object() {
                println!("PredY({}) = {:?}", py["x"], py["y"]);
            }
            if let Some(px) = reg["pred_x"].as_object() {
                println!("PredX({}) = {:?}", px["y"], px["x"]);
            }
        }
    }
    report
}

pub fn run_init(o: &InitOpts, json_out: bool) -> Report {
    let default_name = o
        .dir
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| {
            let clean: String = s
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect();
            if clean.starts_with(|c: char| c.is_ascii_alphabetic()) {
                clean
            } else {
                "MAIN".into()
            }
        })
        .unwrap_or_else(|| "MAIN".into());
    let name = o.name.as_deref().unwrap_or(&default_name);
    let report = init_project(&o.dir, name, o.template.as_str(), o.force);
    if !json_out && report.exit_code == 0 {
        println!(
            "Initialized {} PPL project `{}` in {}",
            o.template.as_str(),
            report.data["name"].as_str().unwrap_or(""),
            o.dir.display()
        );
        for path in &report.artifacts {
            println!("  created {}", path.display());
        }
    }
    report
}

pub fn run_package(o: &PackageOpts, json_out: bool) -> Report {
    let report = package_hpprgm(&o.input, &o.output, o.unpack, o.no_check);
    if !json_out && report.exit_code == 0 {
        if o.unpack {
            println!(
                "Unpacked {} ({} chars) -> {}",
                o.input.display(),
                report.data["source_chars"],
                o.output.display()
            );
        } else {
            println!(
                "Packaged {} -> {} ({} bytes)",
                o.input.display(),
                o.output.display(),
                report.data["size_bytes"]
            );
        }
    }
    report
}

pub fn run_docs(o: &DocsOpts, json_out: bool) -> Report {
    if let Some(file) = &o.file {
        let mut r = extract_docs(file);
        if r.exit_code == 0
            && let Some(out) = &o.output
            && let Some(md) = r.data["markdown"].as_str()
        {
            if let Err(e) = write_docs_output(out, Some(file), md) {
                r.fail("io_error", 5, e);
                return r;
            }
            r.artifacts.push(out.clone());
        }
        if !json_out
            && r.exit_code == 0
            && o.output.is_none()
            && let Some(md) = r.data["markdown"].as_str()
        {
            print!("{md}");
        }
        r
    } else {
        let mut r = list_commands(
            o.category.as_deref(),
            o.status.as_deref(),
            o.query.as_deref(),
            o.include_catalog,
        );
        r.operation = "docs".into();
        let mut md = String::from("# HP Prime Native Command Reference\n\n");
        md.push_str(
            "| Command | Category | Arity | Status | Source |\n| --- | --- | --- | --- | --- |\n",
        );
        if let Some(cmds) = r.data["commands"].as_array() {
            for c in cmds {
                use std::fmt::Write;
                let _ = writeln!(
                    md,
                    "| `{}` | {} | {} | {} | {} |",
                    c["name"].as_str().unwrap_or(""),
                    c["category"].as_str().unwrap_or(""),
                    c["arity"].as_str().unwrap_or(""),
                    c["status"].as_str().unwrap_or(""),
                    c["source"].as_str().unwrap_or(""),
                );
            }
        }
        r.data["markdown"] = Value::String(md.clone());
        if let Some(out) = &o.output {
            if let Err(e) = write_docs_output(out, None, &md) {
                r.fail("io_error", 5, e);
                return r;
            }
            r.artifacts.push(out.clone());
        } else if !json_out {
            print!("{md}");
        }
        r
    }
}

pub fn run_snapshot(o: &SnapshotOpts, json_out: bool) -> Report {
    match &o.action {
        SnapshotAction::Capture {
            file,
            output,
            exprs,
            entry,
            args,
            args_format,
            angle,
            seed,
            max_steps,
            max_time,
        } => {
            let report = snapshot_capture(SnapshotCaptureOptions {
                file: file.clone(),
                exprs: exprs.clone(),
                output: output.clone(),
                entry: entry.clone(),
                args: args.clone(),
                args_format: args_format_str(*args_format).into(),
                angle: angle_str(*angle).into(),
                seed: *seed,
                max_steps: *max_steps,
                max_time: *max_time,
            });
            if !json_out && report.exit_code == 0 {
                println!(
                    "Captured snapshot -> {} ({} global(s))",
                    output.display(),
                    report.data["state"]["globals"]
                        .as_object()
                        .map(|m| m.len())
                        .unwrap_or(0)
                );
            }
            report
        }
        SnapshotAction::Inspect { state } => {
            let report = snapshot_inspect(state);
            if !json_out && report.exit_code == 0 {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report.data["state"]).expect("JSON")
                );
            }
            report
        }
        SnapshotAction::Run {
            file,
            state,
            save_state,
            expr,
            entry,
            args,
            args_format,
            angle,
            seed,
            max_steps,
            max_time,
        } => {
            let report = snapshot_run(SnapshotRunOptions {
                state_path: state.clone(),
                save_state: save_state.clone(),
                file: file.clone(),
                entry: entry.clone(),
                args: args.clone(),
                args_format: args_format_str(*args_format).into(),
                expr: expr.clone(),
                angle: angle.map(|a| angle_str(a).to_owned()),
                seed: *seed,
                max_steps: *max_steps,
                max_time: *max_time,
            });
            if !json_out && report.exit_code == 0 {
                for line in &report.output {
                    println!("{line}");
                }
                if let Some(val) = &report.return_value {
                    println!("returned: {}", crate::cli::display_value(val));
                }
            }
            report
        }
    }
}
