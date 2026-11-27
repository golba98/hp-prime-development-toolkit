//! Standalone developer tools: benchmarking, function plotting, statistics, scaffolding, packaging, and session snapshots.
use crate::{
    eval::{ReplSession, SessionStateFile, angle_name, build_settings},
    execution::{
        atomic_bytes, load_source_file, protect_output, resolve_modules, save_image, to_diagnostic,
    },
    model::*,
};
use ppl_engine::arguments;
use ppl_graphics::{Grob, text::draw_text};
use ppl_syntax::{SourceFile, hpprgm};
use ppl_vm::{AngleMode, HeadlessHost, Limits, Machine, Settings, Value};
use serde_json::{Value as Json, json};
use std::{
    collections::BTreeMap,
    fmt::Write as _,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

// --- Benchmarking ---

pub fn bench_program(request: Request, warmup: usize, iterations: usize) -> Report {
    let started = Instant::now();
    let mut report = Report::new("bench", Some(request.file.clone()));
    if iterations == 0 || iterations > 10_000 || warmup > 1_000 {
        report.fail(
            "invalid_request",
            2,
            "warmup must be <= 1000 and iterations must be 1..=10000",
        );
        return report;
    }
    let source = match load_source_file(&request.file, request.source_bytes.clone()) {
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
    let file = SourceFile::new(&source.identity.display_name, &source.text);
    let (sources, mut catalog) = match resolve_modules(&source, &request.modules) {
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
            Ok(img) => Some(img),
            Err(e) => {
                report.fail("invalid_source", 1, e.to_string());
                report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
                return report;
            }
        }
    };
    let program = if let Some(img) = &linked {
        for (id, diags) in img.diagnostics() {
            let sf = sources.get(&id.name).unwrap_or(&file);
            report
                .diagnostics
                .extend(diags.items.iter().map(|d| to_diagnostic(sf, d)));
        }
        Some(img.program().clone())
    } else {
        let (p, diags) = ppl_compiler::compile_source(&file, &Default::default());
        report.diagnostics = diags
            .items
            .iter()
            .map(|d| to_diagnostic(&file, d))
            .collect();
        p
    };
    let Some(program) = program.filter(|_| !source.text.trim().is_empty()) else {
        report.fail("invalid_source", 1, "source validation failed");
        report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
        return report;
    };
    let entry = match program.pick_entry_with_stem(request.entry.as_deref(), Some(&source.stem)) {
        Ok(e) => e,
        Err(e) => {
            report.fail("invalid_request", 2, e);
            report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
            return report;
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
            report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
            return report;
        }
    };
    let expected =
        program.functions[program.function_index(&entry).expect("entry")].params as usize;
    if args.len() != expected {
        report.fail(
            "invalid_request",
            2,
            format!("{entry} expects {expected} arguments, got {}", args.len()),
        );
        report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
        return report;
    }

    let limits = Limits {
        max_steps: request.max_steps,
        max_time: request
            .max_time
            .and_then(|s| Duration::try_from_secs_f64(s).ok()),
        max_depth: request.max_depth,
        max_collection_len: request.max_collection_len,
        max_string_bytes: request.max_string_bytes,
        max_output_chars: request.max_output_chars,
    };
    let shared_program = Rc::new(program);

    let run_once = || -> Result<(Value, u64, f64), ppl_vm::RuntimeError> {
        let mut host = HeadlessHost::new();
        if let Some(keys) = &request.keys
            && let Ok(k) = ppl_vm::parse_key_script(keys)
        {
            host.keys = k.into();
        }
        host.inputs = request
            .inputs
            .iter()
            .map(|t| ppl_vm::ScriptedInput::parse(t))
            .collect();
        host.choices = request.choices.iter().copied().collect();
        let mut machine = match &linked {
            Some(img) => Machine::new_linked(
                img.clone(),
                Box::new(host),
                settings.clone(),
                limits.clone(),
            ),
            None => Machine::new(
                shared_program.clone(),
                Box::new(host),
                settings.clone(),
                limits.clone(),
            ),
        };
        machine.set_seed(request.seed);
        let t0 = Instant::now();
        let val = machine
            .initialize_dependencies()
            .and_then(|_| machine.call_function(&entry, args.clone()))?;
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        Ok((val, machine.steps(), ms))
    };

    for _ in 0..warmup {
        if let Err(e) = run_once() {
            let (status, code) = crate::eval::classify_runtime_error(&e);
            report.fail(status, code, e.to_string());
            report.runtime_error = Some(e.to_string());
            report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
            return report;
        }
    }

    let mut samples_ms = Vec::with_capacity(iterations);
    let mut last_val = None;
    let mut instructions = 0u64;
    for _ in 0..iterations {
        match run_once() {
            Ok((val, steps, ms)) => {
                samples_ms.push(ms);
                instructions = steps;
                last_val = Some(val);
            }
            Err(e) => {
                let (status, code) = crate::eval::classify_runtime_error(&e);
                report.fail(status, code, e.to_string());
                report.runtime_error = Some(e.to_string());
                report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
                return report;
            }
        }
    }

    samples_ms.sort_by(|a, b| a.total_cmp(b));
    let total_ms: f64 = samples_ms.iter().sum();
    let mean_ms = total_ms / samples_ms.len() as f64;
    let min_ms = samples_ms[0];
    let max_ms = *samples_ms.last().unwrap_or(&0.0);
    let median_ms = if samples_ms.len() % 2 == 1 {
        samples_ms[samples_ms.len() / 2]
    } else {
        (samples_ms[samples_ms.len() / 2 - 1] + samples_ms[samples_ms.len() / 2]) / 2.0
    };
    let p95_idx = ((samples_ms.len() as f64 * 0.95).ceil() as usize)
        .saturating_sub(1)
        .min(samples_ms.len() - 1);
    let p95_ms = samples_ms[p95_idx];
    let runs_per_sec = if mean_ms > 0.0 { 1000.0 / mean_ms } else { 0.0 };

    report.instructions = instructions;
    report.return_value = last_val.as_ref().map(arguments::encode);
    report.data = json!({
        "warmup": warmup,
        "iterations": iterations,
        "instructions": instructions,
        "min_ms": min_ms,
        "median_ms": median_ms,
        "mean_ms": mean_ms,
        "p95_ms": p95_ms,
        "max_ms": max_ms,
        "total_ms": total_ms,
        "runs_per_sec": runs_per_sec,
    });
    report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
    report
}

// --- Function Plotting ---

#[derive(Clone, Debug)]
pub struct PlotOptions {
    pub expressions: Vec<String>,
    pub xmin: f64,
    pub xmax: f64,
    pub ymin: Option<f64>,
    pub ymax: Option<f64>,
    pub samples: usize,
    pub width: usize,
    pub height: usize,
    pub angle: String,
    pub output: Option<PathBuf>,
    pub ascii: bool,
    pub grid: bool,
    pub title: Option<String>,
}

impl Default for PlotOptions {
    fn default() -> Self {
        Self {
            expressions: vec![],
            xmin: -10.0,
            xmax: 10.0,
            ymin: None,
            ymax: None,
            samples: 320,
            width: 320,
            height: 240,
            angle: "rad".into(),
            output: None,
            ascii: false,
            grid: true,
            title: None,
        }
    }
}

const PLOT_COLORS: [u32; 6] = [
    0x0015_65C0,
    0x00C6_2828,
    0x002E_7D32,
    0x006A_1B9A,
    0x00EF_6C00,
    0x0000_838F,
];

pub fn plot_functions(opts: PlotOptions) -> Report {
    let started = Instant::now();
    let mut report = Report::new("plot", None);
    if opts.expressions.is_empty() {
        report.fail("invalid_request", 2, "at least one expression is required");
        return report;
    }
    if !opts.xmin.is_finite() || !opts.xmax.is_finite() || opts.xmin >= opts.xmax {
        report.fail(
            "invalid_request",
            2,
            "xmin and xmax must be finite with xmin < xmax",
        );
        return report;
    }
    if opts.ymin.is_some_and(|y| !y.is_finite()) || opts.ymax.is_some_and(|y| !y.is_finite()) {
        report.fail("invalid_request", 2, "ymin and ymax must be finite");
        return report;
    }
    if let (Some(y0), Some(y1)) = (opts.ymin, opts.ymax)
        && y0 >= y1
    {
        report.fail("invalid_request", 2, "ymin must be less than ymax");
        return report;
    }
    if !(16..=4096).contains(&opts.samples) {
        report.fail("invalid_request", 2, "samples must be between 16 and 4096");
        return report;
    }
    if !(64..=1600).contains(&opts.width) || !(64..=1200).contains(&opts.height) {
        report.fail(
            "invalid_request",
            2,
            "width must be 64..=1600 and height must be 64..=1200",
        );
        return report;
    }
    let settings = match build_settings(&opts.angle, "standard", 6) {
        Ok(s) => s,
        Err(e) => {
            report.fail("invalid_request", 2, e);
            return report;
        }
    };

    let mut all_series: Vec<(String, Vec<Option<f64>>)> =
        Vec::with_capacity(opts.expressions.len());
    let mut total_steps = 0u64;

    for expr in &opts.expressions {
        let src = format!("EXPORT __PLOT(X)\nBEGIN\nRETURN ({expr});\nEND;");
        let file = SourceFile::new("<plot>", &src);
        let (prog, diags) = ppl_compiler::compile_source(&file, &Default::default());
        let Some(prog) = prog else {
            report.diagnostics = diags
                .items
                .iter()
                .map(|d| to_diagnostic(&file, d))
                .collect();
            report.fail(
                "invalid_source",
                1,
                format!("cannot compile plot expression `{expr}`"),
            );
            report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
            return report;
        };
        let prog = Rc::new(prog);
        let limits = Limits {
            max_steps: Some(25_000),
            max_time: Some(Duration::from_secs(2)),
            ..Limits::default()
        };
        let mut ys = Vec::with_capacity(opts.samples);
        for i in 0..opts.samples {
            let x = opts.xmin + (opts.xmax - opts.xmin) * (i as f64) / ((opts.samples - 1) as f64);
            let Ok(mut arg_vec) = arguments::json_args(
                &format!("[{{\"type\":\"real\",\"value\":\"{x:.12}\"}}]"),
                &settings,
            ) else {
                ys.push(None);
                continue;
            };
            let Some(x_val) = arg_vec.pop() else {
                ys.push(None);
                continue;
            };
            let mut machine = Machine::new(
                prog.clone(),
                Box::new(HeadlessHost::new()),
                settings.clone(),
                limits.clone(),
            );
            match machine.call_function("__PLOT", vec![x_val]) {
                Ok(val) => {
                    let y = val.as_real().map(|r| r.to_f64());
                    ys.push(y.filter(|v| v.is_finite()));
                }
                Err(_) => ys.push(None),
            }
            total_steps += machine.steps();
        }
        all_series.push((expr.clone(), ys));
    }

    let mut finite_ys: Vec<f64> = all_series
        .iter()
        .flat_map(|(_, ys)| ys.iter().flatten().copied())
        .collect();
    finite_ys.sort_by(|a, b| a.total_cmp(b));

    let (auto_ymin, auto_ymax) = if finite_ys.is_empty() {
        (-10.0, 10.0)
    } else {
        // Use robust percentile bounds when asymptotic spikes exceed normal spread.
        let p02 = finite_ys[(finite_ys.len() as f64 * 0.02) as usize];
        let p98 = finite_ys[((finite_ys.len() as f64 * 0.98) as usize).min(finite_ys.len() - 1)];
        let raw_min = finite_ys[0];
        let raw_max = *finite_ys.last().unwrap();
        let (lo, hi) = if (raw_max - raw_min) > 10.0 * (p98 - p02).max(1e-9) {
            (p02, p98)
        } else {
            (raw_min, raw_max)
        };
        let span = (hi - lo).abs();
        let pad = if span < 1e-9 { 1.0 } else { span * 0.1 };
        (lo - pad, hi + pad)
    };
    let ymin = opts.ymin.unwrap_or(auto_ymin);
    let ymax = opts.ymax.unwrap_or(auto_ymax);
    if ymin >= ymax {
        report.fail("invalid_request", 2, "resolved ymin must be less than ymax");
        report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
        return report;
    }

    // Render onto Grob
    let mut grob = Grob::new(opts.width, opts.height, 0x00FF_FFFF);
    let pad_l = 32i64;
    let pad_r = 10i64;
    let pad_t = 18i64;
    let pad_b = 18i64;
    let pw = (opts.width as i64 - pad_l - pad_r).max(16);
    let ph = (opts.height as i64 - pad_t - pad_b).max(16);

    let map_x = |x: f64| -> i64 {
        pad_l + (((x - opts.xmin) / (opts.xmax - opts.xmin)) * (pw as f64)).round() as i64
    };
    let map_y = |y: f64| -> i64 {
        pad_t + ph - (((y - ymin) / (ymax - ymin)) * (ph as f64)).round() as i64
    };

    if opts.grid {
        let grid_color = 0x00E6_E6E6;
        for step in 0..=4 {
            let gx = pad_l + (pw * step) / 4;
            grob.line(gx, pad_t, gx, pad_t + ph, grid_color);
            let gy = pad_t + (ph * step) / 4;
            grob.line(pad_l, gy, pad_l + pw, gy, grid_color);
        }
    }
    // Plot frame border
    grob.rect(
        pad_l,
        pad_t,
        pad_l + pw,
        pad_t + ph,
        0x00B0_B0B0,
        0x00FF_FFFF,
    );
    if opts.grid {
        for step in 1..4 {
            let gx = pad_l + (pw * step) / 4;
            grob.line(gx, pad_t + 1, gx, pad_t + ph - 1, 0x00EB_EBEB);
            let gy = pad_t + (ph * step) / 4;
            grob.line(pad_l + 1, gy, pad_l + pw - 1, gy, 0x00EB_EBEB);
        }
    }
    // Axes x=0 and y=0 if in view
    let axis_color = 0x0040_4040;
    if opts.xmin <= 0.0 && opts.xmax >= 0.0 {
        let ax = map_x(0.0);
        grob.line(ax, pad_t, ax, pad_t + ph, axis_color);
    }
    if ymin <= 0.0 && ymax >= 0.0 {
        let ay = map_y(0.0);
        grob.line(pad_l, ay, pad_l + pw, ay, axis_color);
    }

    let y_jump_limit = (ymax - ymin) * 2.5;
    for (idx, (_, ys)) in all_series.iter().enumerate() {
        let color = PLOT_COLORS[idx % PLOT_COLORS.len()];
        let mut prev: Option<(i64, i64, f64)> = None;
        for (i, y_opt) in ys.iter().enumerate() {
            let x = opts.xmin + (opts.xmax - opts.xmin) * (i as f64) / ((opts.samples - 1) as f64);
            let Some(y) = y_opt else {
                prev = None;
                continue;
            };
            let px = map_x(x);
            let py = map_y(*y);
            if let Some((px0, py0, y0)) = prev
                && (y - y0).abs() <= y_jump_limit
                && !(*y < ymin && y0 < ymin)
                && !(*y > ymax && y0 > ymax)
            {
                let cy0 = py0.clamp(pad_t, pad_t + ph);
                let cy1 = py.clamp(pad_t, pad_t + ph);
                grob.line(px0, cy0, px, cy1, color);
            } else if py >= pad_t && py <= pad_t + ph {
                grob.set(px, py, color);
            }
            prev = Some((px, py, *y));
        }
    }

    // Labels
    let label_color = 0x0022_2222;
    draw_text(
        &mut grob,
        &format!("{ymax:.3}"),
        2,
        pad_t - 4,
        9.0,
        label_color,
        Some(pad_l - 2),
        None,
    );
    draw_text(
        &mut grob,
        &format!("{ymin:.3}"),
        2,
        pad_t + ph - 6,
        9.0,
        label_color,
        Some(pad_l - 2),
        None,
    );
    draw_text(
        &mut grob,
        &format!("{:.3}", opts.xmin),
        pad_l,
        pad_t + ph + 3,
        9.0,
        label_color,
        None,
        None,
    );
    draw_text(
        &mut grob,
        &format!("{:.3}", opts.xmax),
        (pad_l + pw - 28).max(pad_l),
        pad_t + ph + 3,
        9.0,
        label_color,
        None,
        None,
    );
    let header = opts
        .title
        .clone()
        .unwrap_or_else(|| opts.expressions.join(", "));
    draw_text(
        &mut grob,
        &header,
        pad_l,
        3,
        10.0,
        0x0011_1111,
        Some(pw),
        None,
    );

    if let Some(out_path) = &opts.output {
        if let Err(e) = save_image(out_path, &grob) {
            report.fail(
                "io_error",
                5,
                format!("cannot save {}: {e}", out_path.display()),
            );
            report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
            return report;
        }
        report.artifacts.push(out_path.clone());
    }

    let ascii_art = if opts.ascii || opts.output.is_none() {
        Some(render_ascii_plot(
            &all_series,
            opts.xmin,
            opts.xmax,
            ymin,
            ymax,
            64,
            18,
        ))
    } else {
        None
    };

    let series_meta: Vec<Json> = all_series
        .iter()
        .map(|(expr, ys)| {
            let valid: Vec<f64> = ys.iter().flatten().copied().collect();
            let min_y = valid.iter().copied().reduce(f64::min);
            let max_y = valid.iter().copied().reduce(f64::max);
            json!({
                "expression": expr,
                "samples": ys.len(),
                "valid_points": valid.len(),
                "min_y": min_y,
                "max_y": max_y,
            })
        })
        .collect();

    report.instructions = total_steps;
    report.data = json!({
        "bounds": {
            "xmin": opts.xmin,
            "xmax": opts.xmax,
            "ymin": ymin,
            "ymax": ymax,
        },
        "width": opts.width,
        "height": opts.height,
        "angle": opts.angle,
        "series": series_meta,
        "ascii": ascii_art,
    });
    report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
    report
}

fn render_ascii_plot(
    series: &[(String, Vec<Option<f64>>)],
    xmin: f64,
    xmax: f64,
    ymin: f64,
    ymax: f64,
    cols: usize,
    rows: usize,
) -> String {
    let mut grid = vec![vec![' '; cols]; rows];
    if ymin <= 0.0 && ymax >= 0.0 {
        let ay = ((ymax - 0.0) / (ymax - ymin) * (rows - 1) as f64)
            .round()
            .clamp(0.0, (rows - 1) as f64) as usize;
        for cell in &mut grid[ay] {
            *cell = '-';
        }
    }
    if xmin <= 0.0 && xmax >= 0.0 {
        let ax = ((0.0 - xmin) / (xmax - xmin) * (cols - 1) as f64)
            .round()
            .clamp(0.0, (cols - 1) as f64) as usize;
        for row in &mut grid {
            row[ax] = if row[ax] == '-' { '+' } else { '|' };
        }
    }
    let markers = ['*', '+', 'o', '#', 'x', '@'];
    for (idx, (_, ys)) in series.iter().enumerate() {
        let ch = markers[idx % markers.len()];
        let n = ys.len();
        for (i, y_opt) in ys.iter().enumerate() {
            let Some(y) = y_opt else { continue };
            if *y < ymin || *y > ymax {
                continue;
            }
            let c = (i * (cols - 1) / (n - 1).max(1)).min(cols - 1);
            let r = ((ymax - *y) / (ymax - ymin) * (rows - 1) as f64)
                .round()
                .clamp(0.0, (rows - 1) as f64) as usize;
            grid[r][c] = ch;
        }
    }
    let mut out = String::new();
    for row in grid {
        let line: String = row.into_iter().collect();
        let _ = writeln!(out, "{line}");
    }
    out
}

// --- Statistics & Regression ---

#[derive(Clone, Debug)]
pub struct StatsOptions {
    pub x: Vec<f64>,
    pub y: Option<Vec<f64>>,
    pub model: String,
    pub predict_y_at: Option<f64>,
    pub predict_x_at: Option<f64>,
}

pub fn parse_number_series(raw: &str) -> Result<Vec<f64>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("number series cannot be empty".into());
    }
    let inner = trimmed
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .or_else(|| trimmed.strip_prefix('{').and_then(|s| s.strip_suffix('}')))
        .unwrap_or(trimmed);
    let mut out = Vec::new();
    for part in inner.split(|c: char| c == ',' || c.is_whitespace()) {
        let tok = part.trim();
        if tok.is_empty() {
            continue;
        }
        let val = tok
            .parse::<f64>()
            .map_err(|_| format!("invalid number `{tok}` in series"))?;
        if !val.is_finite() {
            return Err(format!("non-finite number `{tok}` in series"));
        }
        out.push(val);
    }
    if out.is_empty() {
        return Err("number series contains no values".into());
    }
    Ok(out)
}

pub type CsvColumns = (Vec<f64>, Option<Vec<f64>>);

pub fn parse_csv_columns(
    path: &Path,
    x_col: &str,
    y_col: Option<&str>,
) -> Result<CsvColumns, (u8, String)> {
    let text =
        std::fs::read_to_string(path).map_err(|e| (5u8, format!("{}: {e}", path.display())))?;
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let Some(first) = lines.next() else {
        return Err((2, "CSV file is empty".into()));
    };
    let first_cells: Vec<&str> = first.split(',').map(str::trim).collect();
    let has_header = first_cells.iter().any(|c| c.parse::<f64>().is_err());

    let resolve_col = |spec: &str| -> Result<usize, (u8, String)> {
        if let Ok(idx) = spec.parse::<usize>() {
            if idx == 0 || idx > first_cells.len() {
                return Err((
                    2,
                    format!("column index {idx} out of range 1..={}", first_cells.len()),
                ));
            }
            return Ok(idx - 1);
        }
        if has_header
            && let Some(pos) = first_cells
                .iter()
                .position(|h| h.eq_ignore_ascii_case(spec))
        {
            return Ok(pos);
        }
        Err((2, format!("column `{spec}` not found in CSV")))
    };

    let xi = resolve_col(x_col)?;
    let yi = y_col.map(resolve_col).transpose()?;

    let mut xs = Vec::new();
    let mut ys = yi.map(|_| Vec::new());

    let data_lines: Box<dyn Iterator<Item = &str>> = if has_header {
        Box::new(lines)
    } else {
        Box::new(std::iter::once(first).chain(lines))
    };

    for (row_no, line) in data_lines.enumerate() {
        let cells: Vec<&str> = line.split(',').map(str::trim).collect();
        let x_str = cells
            .get(xi)
            .ok_or_else(|| (2u8, format!("CSV row {} missing x column", row_no + 1)))?;
        let x_val = x_str.parse::<f64>().map_err(|_| {
            (
                2u8,
                format!("invalid numeric value `{x_str}` in row {}", row_no + 1),
            )
        })?;
        if !x_val.is_finite() {
            return Err((2, format!("non-finite number `{x_str}`")));
        }
        xs.push(x_val);
        if let (Some(y_idx), Some(y_vec)) = (yi, ys.as_mut()) {
            let y_str = cells
                .get(y_idx)
                .ok_or_else(|| (2u8, format!("CSV row {} missing y column", row_no + 1)))?;
            let y_val = y_str.parse::<f64>().map_err(|_| {
                (
                    2u8,
                    format!("invalid numeric value `{y_str}` in row {}", row_no + 1),
                )
            })?;
            if !y_val.is_finite() {
                return Err((2, format!("non-finite number `{y_str}`")));
            }
            y_vec.push(y_val);
        }
    }
    Ok((xs, ys))
}

fn median_sorted(sorted: &[f64]) -> f64 {
    let n = sorted.len();
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    }
}

fn one_var_stats(values: &[f64]) -> Json {
    let n = values.len();
    let sum: f64 = values.iter().sum();
    let sum_sq: f64 = values.iter().map(|v| v * v).sum();
    let mean = sum / n as f64;
    let ss: f64 = values.iter().map(|v| (v - mean).powi(2)).sum();
    let pop_var = ss / n as f64;
    let pop_sd = pop_var.sqrt();
    let (sample_var, sample_sd) = if n > 1 {
        let sv = ss / (n - 1) as f64;
        (Some(sv), Some(sv.sqrt()))
    } else {
        (None, None)
    };
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let min = sorted[0];
    let max = sorted[n - 1];
    let median = median_sorted(&sorted);
    let (q1, q3) = if n == 1 {
        (sorted[0], sorted[0])
    } else {
        let mid = n / 2;
        let lower = &sorted[..mid];
        let upper = if n.is_multiple_of(2) {
            &sorted[mid..]
        } else {
            &sorted[mid + 1..]
        };
        (median_sorted(lower), median_sorted(upper))
    };
    json!({
        "n": n,
        "sum": sum,
        "sum_sq": sum_sq,
        "mean": mean,
        "sample_sd": sample_sd,
        "pop_sd": pop_sd,
        "sample_var": sample_var,
        "pop_var": pop_var,
        "min": min,
        "q1": q1,
        "median": median,
        "q3": q3,
        "max": max,
        "iqr": q3 - q1,
        "range": max - min,
    })
}

#[derive(Clone, Debug)]
struct RegressionFit {
    model: &'static str,
    equation: String,
    a: f64,
    b: f64,
    r: f64,
    r_squared: f64,
}

impl RegressionFit {
    fn predict_y(&self, x: f64) -> Option<f64> {
        let y = match self.model {
            "linear" => self.a * x + self.b,
            "log" => {
                if x <= 0.0 {
                    return None;
                }
                self.a + self.b * x.ln()
            }
            "exp" => self.a * (self.b * x).exp(),
            "power" => {
                if x <= 0.0 {
                    return None;
                }
                self.a * x.powf(self.b)
            }
            _ => return None,
        };
        y.is_finite().then_some(y)
    }

    fn predict_x(&self, y: f64) -> Option<f64> {
        let x = match self.model {
            "linear" => {
                if self.a.abs() < 1e-15 {
                    return None;
                }
                (y - self.b) / self.a
            }
            "log" => {
                if self.b.abs() < 1e-15 {
                    return None;
                }
                ((y - self.a) / self.b).exp()
            }
            "exp" => {
                if y <= 0.0 || self.a <= 0.0 || self.b.abs() < 1e-15 {
                    return None;
                }
                (y / self.a).ln() / self.b
            }
            "power" => {
                if y <= 0.0 || self.a <= 0.0 || self.b.abs() < 1e-15 {
                    return None;
                }
                (y / self.a).ln() / self.b
            }
            _ => return None,
        };
        let x = if self.model == "power" { x.exp() } else { x };
        x.is_finite().then_some(x)
    }
}

fn fit_linear(xs: &[f64], ys: &[f64]) -> Result<(f64, f64, f64), String> {
    let n = xs.len() as f64;
    let mx = xs.iter().sum::<f64>() / n;
    let my = ys.iter().sum::<f64>() / n;
    let mut sxx = 0.0;
    let mut syy = 0.0;
    let mut sxy = 0.0;
    for (x, y) in xs.iter().zip(ys.iter()) {
        let dx = x - mx;
        let dy = y - my;
        sxx += dx * dx;
        syy += dy * dy;
        sxy += dx * dy;
    }
    if sxx <= 0.0 {
        return Err("regression requires non-constant x values".into());
    }
    let slope = sxy / sxx;
    let intercept = my - slope * mx;
    let r = if syy > 0.0 {
        (sxy / (sxx * syy).sqrt()).clamp(-1.0, 1.0)
    } else {
        1.0
    };
    Ok((slope, intercept, r))
}

fn fit_model(kind: &str, xs: &[f64], ys: &[f64]) -> Result<RegressionFit, String> {
    match kind {
        "linear" => {
            let (m, b, r) = fit_linear(xs, ys)?;
            Ok(RegressionFit {
                model: "linear",
                equation: format!("y = {m:.6}*x + {b:.6}"),
                a: m,
                b,
                r,
                r_squared: r * r,
            })
        }
        "log" => {
            if xs.iter().any(|&x| x <= 0.0) {
                return Err("logarithmic regression requires all x > 0".into());
            }
            let lx: Vec<f64> = xs.iter().map(|x| x.ln()).collect();
            let (b, a, r) = fit_linear(&lx, ys)?;
            Ok(RegressionFit {
                model: "log",
                equation: format!("y = {a:.6} + {b:.6}*ln(x)"),
                a,
                b,
                r,
                r_squared: r * r,
            })
        }
        "exp" => {
            if ys.iter().any(|&y| y <= 0.0) {
                return Err("exponential regression requires all y > 0".into());
            }
            let ly: Vec<f64> = ys.iter().map(|y| y.ln()).collect();
            let (b, lna, r) = fit_linear(xs, &ly)?;
            let a = lna.exp();
            Ok(RegressionFit {
                model: "exp",
                equation: format!("y = {a:.6}*exp({b:.6}*x)"),
                a,
                b,
                r,
                r_squared: r * r,
            })
        }
        "power" => {
            if xs.iter().any(|&x| x <= 0.0) || ys.iter().any(|&y| y <= 0.0) {
                return Err("power regression requires all x > 0 and y > 0".into());
            }
            let lx: Vec<f64> = xs.iter().map(|x| x.ln()).collect();
            let ly: Vec<f64> = ys.iter().map(|y| y.ln()).collect();
            let (b, lna, r) = fit_linear(&lx, &ly)?;
            let a = lna.exp();
            Ok(RegressionFit {
                model: "power",
                equation: format!("y = {a:.6}*x^{b:.6}"),
                a,
                b,
                r,
                r_squared: r * r,
            })
        }
        other => Err(format!(
            "unknown regression model `{other}`; use linear, log, exp, power, or auto"
        )),
    }
}

pub fn compute_stats(opts: StatsOptions) -> Report {
    let started = Instant::now();
    let mut report = Report::new("stats", None);
    if opts.x.is_empty() {
        report.fail("invalid_request", 2, "x dataset cannot be empty");
        return report;
    }
    let x_stats = one_var_stats(&opts.x);
    let Some(ys) = opts.y else {
        report.data = json!({
            "mode": "1-var",
            "x": x_stats,
        });
        report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
        return report;
    };
    if ys.len() != opts.x.len() {
        report.fail(
            "invalid_request",
            2,
            format!(
                "x and y must have equal length (got {} vs {})",
                opts.x.len(),
                ys.len()
            ),
        );
        return report;
    }
    if opts.x.len() < 2 {
        report.fail(
            "invalid_request",
            2,
            "2-variable statistics requires at least 2 pairs",
        );
        return report;
    }

    let y_stats = one_var_stats(&ys);
    let n = opts.x.len() as f64;
    let mx = x_stats["mean"].as_f64().unwrap_or(0.0);
    let my = y_stats["mean"].as_f64().unwrap_or(0.0);
    let sum_xy: f64 = opts.x.iter().zip(ys.iter()).map(|(x, y)| x * y).sum();
    let sxy: f64 = opts
        .x
        .iter()
        .zip(ys.iter())
        .map(|(x, y)| (x - mx) * (y - my))
        .sum();
    let cov_pop = sxy / n;
    let cov_sample = sxy / (n - 1.0);

    let fit = if opts.model.eq_ignore_ascii_case("auto") {
        let candidates: Vec<RegressionFit> = ["linear", "log", "exp", "power"]
            .into_iter()
            .filter_map(|m| fit_model(m, &opts.x, &ys).ok())
            .collect();
        candidates
            .into_iter()
            .max_by(|a, b| a.r_squared.total_cmp(&b.r_squared))
            .ok_or_else(|| "no valid regression model fit this dataset".to_owned())
    } else {
        fit_model(&opts.model.to_ascii_lowercase(), &opts.x, &ys)
    };

    let fit = match fit {
        Ok(f) => f,
        Err(e) => {
            report.fail("invalid_request", 2, e);
            report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
            return report;
        }
    };

    let pred_y = opts.predict_y_at.map(|x| {
        json!({
            "x": x,
            "y": fit.predict_y(x),
        })
    });
    let pred_x = opts.predict_x_at.map(|y| {
        json!({
            "y": y,
            "x": fit.predict_x(y),
        })
    });

    report.data = json!({
        "mode": "2-var",
        "x": x_stats,
        "y": y_stats,
        "sum_xy": sum_xy,
        "cov_pop": cov_pop,
        "cov_sample": cov_sample,
        "regression": {
            "model": fit.model,
            "equation": fit.equation,
            "a": fit.a,
            "b": fit.b,
            "r": fit.r,
            "r_squared": fit.r_squared,
            "pred_y": pred_y,
            "pred_x": pred_x,
        }
    });
    report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
    report
}

// --- Project Initialization ---

pub fn init_project(dir: &Path, name: &str, template: &str, force: bool) -> Report {
    let started = Instant::now();
    let mut report = Report::new("init", Some(dir.to_owned()));
    if name.is_empty()
        || name.len() > 32
        || !name.starts_with(|c: char| c.is_ascii_alphabetic())
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        report.fail(
            "invalid_request",
            2,
            "project name must start with an ASCII letter and contain only letters, digits, or _ (max 32 chars)",
        );
        return report;
    }
    let fn_name = name.to_ascii_uppercase();
    let src_rel = format!("src/{fn_name}.ppl");
    let (program_text, manifest_text) = match template {
        "basic" => (
            format!(
                "// {fn_name} - Basic HP Prime PPL project\nEXPORT {fn_name}(N)\nBEGIN\n  LOCAL result;\n  result := N * N;\n  PRINT(result);\n  RETURN result;\nEND;\n"
            ),
            format!(
                "schema_version = 1\n\n[[test]]\nname = \"basic square\"\nfile = \"{src_rel}\"\nargs = \"[7]\"\n[test.expect]\nstdout = [\"49\"]\nreturn_value = {{ type = \"real\", value = \"49\" }}\n"
            ),
        ),
        "graphics" => (
            format!(
                "// {fn_name} - Graphics HP Prime PPL project\nEXPORT {fn_name}()\nBEGIN\n  RECT_P(0, 0, 319, 239, RGB(15, 40, 80));\n  RECT_P(40, 40, 279, 199, RGB(255, 215, 0), RGB(30, 90, 160));\n  TEXTOUT_P(\"HP Prime PPL\", 100, 112, 2, RGB(255, 255, 255));\n  FREEZE;\n  RETURN 1;\nEND;\n"
            ),
            format!(
                "schema_version = 1\n\n[[test]]\nname = \"graphics check\"\nfile = \"{src_rel}\"\noperation = \"check\"\n[test.expect]\nstatus = \"success\"\n\n[[test]]\nname = \"graphics run\"\nfile = \"{src_rel}\"\nkeys = \"30\"\n[test.expect]\nreturn_value = {{ type = \"real\", value = \"1\" }}\n"
            ),
        ),
        "library" => (
            format!(
                "// {fn_name} - Reusable PPL library module\nEXPORT ADD_ONE(X)\nBEGIN\n  RETURN X + 1;\nEND;\n\nEXPORT {fn_name}(X)\nBEGIN\n  RETURN ADD_ONE(X) * 2;\nEND;\n"
            ),
            format!(
                "schema_version = 1\n\n[[test]]\nname = \"library entry\"\nfile = \"{src_rel}\"\nentry = \"{fn_name}\"\nargs = \"[4]\"\n[test.expect]\nreturn_value = {{ type = \"real\", value = \"10\" }}\n"
            ),
        ),
        other => {
            report.fail(
                "invalid_request",
                2,
                format!("unknown template `{other}`; use basic, graphics, or library"),
            );
            return report;
        }
    };

    let src_path = dir.join(&src_rel);
    let manifest_path = dir.join("hpdev.toml");
    let gitignore_path = dir.join(".gitignore");

    if !force && (src_path.exists() || manifest_path.exists()) {
        report.fail(
            "invalid_request",
            2,
            "target directory already contains project files; pass --force to overwrite",
        );
        return report;
    }

    if let Err(e) = std::fs::create_dir_all(dir.join("src")) {
        report.fail("io_error", 5, e.to_string());
        return report;
    }
    for (p, content) in [
        (&src_path, program_text.as_bytes()),
        (&manifest_path, manifest_text.as_bytes()),
        (&gitignore_path, b".hpdev/\n*.png\n".as_slice()),
    ] {
        if let Err(e) = atomic_bytes(p, content) {
            report.fail("io_error", 5, format!("cannot write {}: {e}", p.display()));
            return report;
        }
        report.artifacts.push(p.clone());
    }
    report.data = json!({
        "directory": dir,
        "name": fn_name,
        "template": template,
        "files": report.artifacts,
    });
    report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
    report
}

// --- Packaging (.ppl <-> .hpprgm binary container) ---

pub fn package_hpprgm(input: &Path, output: &Path, unpack: bool, skip_check: bool) -> Report {
    let started = Instant::now();
    let mut report = Report::new("package", Some(input.to_owned()));
    if let Err(e) = protect_output(output, &[input.to_path_buf()]) {
        report.fail("invalid_request", 2, e);
        return report;
    }
    if unpack {
        let raw = match std::fs::read(input) {
            Ok(b) => b,
            Err(e) => {
                report.fail("io_error", 5, format!("{}: {e}", input.display()));
                return report;
            }
        };
        let container = match hpprgm::parse(&raw) {
            Ok(c) => c,
            Err(e) => {
                report.fail(
                    "invalid_source",
                    1,
                    format!("invalid .hpprgm container: {e}"),
                );
                return report;
            }
        };
        let src = match container.source() {
            Ok(s) => s,
            Err(e) => {
                report.fail(
                    "invalid_source",
                    1,
                    format!("missing container source: {e}"),
                );
                return report;
            }
        };
        if let Err(e) = atomic_bytes(output, src.source.as_bytes()) {
            report.fail(
                "io_error",
                5,
                format!("cannot write {}: {e}", output.display()),
            );
            return report;
        }
        let exports: Vec<String> = container.exports().into_iter().map(|(n, _)| n).collect();
        report.artifacts.push(output.to_owned());
        report.data = json!({
            "mode": "unpack",
            "main_file": src.name,
            "exports": exports,
            "source_chars": src.source.chars().count(),
        });
        report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
        return report;
    }

    let source = match load_source_file(input, None) {
        Ok(s) => s,
        Err((code, msg)) => {
            report.fail(
                if code == 5 {
                    "io_error"
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
    report.source = Some(source.identity.clone());
    let file = SourceFile::new(&source.identity.display_name, &source.text);
    if !skip_check {
        let (prog, diags) = ppl_compiler::compile_source(&file, &Default::default());
        report.diagnostics = diags
            .items
            .iter()
            .map(|d| to_diagnostic(&file, d))
            .collect();
        if prog.is_none() || source.text.trim().is_empty() {
            report.fail(
                "invalid_source",
                1,
                "source validation failed (pass --no-check to pack unchecked)",
            );
            report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
            return report;
        }
    }
    let container = match hpprgm::build_program(&source.text) {
        Ok(c) => c,
        Err(e) => {
            report.fail(
                "invalid_source",
                1,
                format!("cannot build .hpprgm container: {e}"),
            );
            report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
            return report;
        }
    };
    let bytes = container.to_bytes();
    // Verify round-trip parse before writing
    if let Err(e) = hpprgm::parse(&bytes).and_then(|c| c.source()) {
        report.fail(
            "infrastructure_error",
            70,
            format!("container verification failed: {e}"),
        );
        report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
        return report;
    }
    if let Err(e) = atomic_bytes(output, &bytes) {
        report.fail(
            "io_error",
            5,
            format!("cannot write {}: {e}", output.display()),
        );
        report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
        return report;
    }
    report.artifacts.push(output.to_owned());
    report.data = json!({
        "mode": "pack",
        "size_bytes": bytes.len(),
        "source_sha256": source.identity.source_sha256,
    });
    report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
    report
}

// --- Snapshots ---

#[derive(Clone, Debug)]
pub struct SnapshotRunOptions {
    pub state_path: PathBuf,
    pub save_state: Option<PathBuf>,
    pub file: Option<PathBuf>,
    pub entry: Option<String>,
    pub args: String,
    pub args_format: String,
    pub expr: Option<String>,
    pub angle: Option<String>,
    pub seed: Option<u64>,
    pub max_steps: Option<u64>,
    pub max_time: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct SnapshotCaptureOptions {
    pub file: Option<PathBuf>,
    pub exprs: Vec<String>,
    pub output: PathBuf,
    pub entry: Option<String>,
    pub args: String,
    pub args_format: String,
    pub angle: String,
    pub seed: u64,
    pub max_steps: Option<u64>,
    pub max_time: Option<f64>,
}

pub fn snapshot_capture(opts: SnapshotCaptureOptions) -> Report {
    let started = Instant::now();
    let mut report = Report::new("snapshot", opts.file.clone());
    if let Some(f) = &opts.file
        && let Err(e) = protect_output(&opts.output, std::slice::from_ref(f))
    {
        report.fail("invalid_request", 2, e);
        return report;
    }
    let settings = match build_settings(&opts.angle, "standard", 4) {
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
    let mut session = ReplSession::new(settings, opts.seed, limits);
    let mut outputs = Vec::new();
    let mut last_val = None;
    let mut total_steps = 0u64;

    if let Some(f) = &opts.file {
        if let Err(e) = session.load_program(f) {
            report.fail("invalid_source", 1, e);
            report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
            return report;
        }
        match session.run_loaded_entry(opts.entry.as_deref(), &opts.args, &opts.args_format) {
            Ok(outcome) => {
                outputs.extend(outcome.output);
                total_steps += outcome.steps;
                last_val = outcome.value;
                if outcome.exit_code != 0 {
                    report.fail(
                        &outcome.status,
                        outcome.exit_code,
                        outcome.message.unwrap_or_else(|| "execution failed".into()),
                    );
                    report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
                    return report;
                }
            }
            Err((code, msg)) => {
                report.fail(
                    if code == 2 {
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
        }
    }
    for expr in &opts.exprs {
        let outcome = session.eval_line(expr);
        outputs.extend(outcome.output);
        total_steps += outcome.steps;
        if outcome.exit_code != 0 {
            report.fail(
                &outcome.status,
                outcome.exit_code,
                outcome
                    .message
                    .unwrap_or_else(|| "evaluation failed".into()),
            );
            report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
            return report;
        }
        last_val = outcome.value;
    }

    let state = session.export_state();
    let bytes = match serde_json::to_vec_pretty(&state) {
        Ok(b) => b,
        Err(e) => {
            report.fail("infrastructure_error", 70, e.to_string());
            return report;
        }
    };
    if let Err(e) = atomic_bytes(&opts.output, &bytes) {
        report.fail(
            "io_error",
            5,
            format!("cannot write {}: {e}", opts.output.display()),
        );
        return report;
    }
    report.output = outputs;
    report.return_value = last_val;
    report.instructions = total_steps;
    report.artifacts.push(opts.output);
    report.data = json!({
        "action": "capture",
        "state": state,
    });
    report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
    report
}

pub fn snapshot_inspect(state_path: &Path) -> Report {
    let started = Instant::now();
    let mut report = Report::new("snapshot", Some(state_path.to_owned()));
    let text = match std::fs::read_to_string(state_path) {
        Ok(t) => t,
        Err(e) => {
            report.fail("io_error", 5, format!("{}: {e}", state_path.display()));
            return report;
        }
    };
    let state: SessionStateFile = match serde_json::from_str(&text) {
        Ok(s) => s,
        Err(e) => {
            report.fail("invalid_request", 2, format!("invalid snapshot JSON: {e}"));
            return report;
        }
    };
    if state.schema_version != 1 {
        report.fail(
            "invalid_request",
            2,
            format!(
                "unsupported snapshot schema_version {}",
                state.schema_version
            ),
        );
        return report;
    }
    report.data = json!({
        "action": "inspect",
        "global_count": state.globals.len(),
        "note_count": state.notes.len(),
        "program_count": state.programs.len(),
        "state": state,
    });
    report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
    report
}

pub fn snapshot_run(opts: SnapshotRunOptions) -> Report {
    let started = Instant::now();
    let mut report = Report::new("snapshot", opts.file.clone());
    if opts.file.is_none() && opts.expr.is_none() {
        report.fail("invalid_request", 2, "provide a program FILE or --expr");
        return report;
    }
    if let Some(save) = &opts.save_state {
        let mut protected = vec![opts.state_path.clone()];
        if let Some(f) = &opts.file {
            protected.push(f.clone());
        }
        if let Err(e) = protect_output(save, &protected) {
            report.fail("invalid_request", 2, e);
            return report;
        }
    }
    let text = match std::fs::read_to_string(&opts.state_path) {
        Ok(t) => t,
        Err(e) => {
            report.fail("io_error", 5, format!("{}: {e}", opts.state_path.display()));
            return report;
        }
    };
    let state: SessionStateFile = match serde_json::from_str(&text) {
        Ok(s) => s,
        Err(e) => {
            report.fail("invalid_request", 2, format!("invalid snapshot JSON: {e}"));
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
    let mut session =
        ReplSession::new(Settings::default(), opts.seed.unwrap_or(state.seed), limits);
    if let Err(e) = session.import_state(&state) {
        report.fail("invalid_request", 2, e);
        return report;
    }
    if let Some(angle) = &opts.angle {
        let s = match build_settings(angle, "standard", session.context.settings.hdigits) {
            Ok(s) => s,
            Err(e) => {
                report.fail("invalid_request", 2, e);
                return report;
            }
        };
        session.context.settings.angle = s.angle;
    }
    if let Some(seed) = opts.seed {
        session.seed = seed;
    }

    let before_globals = state.globals.clone();
    let mut outputs = Vec::new();
    let mut total_steps = 0u64;
    let mut last_val = None;

    if let Some(file) = &opts.file {
        if let Err(e) = session.load_program(file) {
            report.fail("invalid_source", 1, e);
            report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
            return report;
        }
        match session.run_loaded_entry(opts.entry.as_deref(), &opts.args, &opts.args_format) {
            Ok(outcome) => {
                outputs.extend(outcome.output);
                total_steps += outcome.steps;
                last_val = outcome.value;
                if outcome.exit_code != 0 {
                    report.fail(
                        &outcome.status,
                        outcome.exit_code,
                        outcome.message.unwrap_or_else(|| "execution failed".into()),
                    );
                    report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
                    return report;
                }
            }
            Err((code, msg)) => {
                report.fail(
                    if code == 2 {
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
        }
    }
    if let Some(expr) = &opts.expr {
        let outcome = session.eval_line(expr);
        outputs.extend(outcome.output);
        total_steps += outcome.steps;
        if outcome.exit_code != 0 {
            report.fail(
                &outcome.status,
                outcome.exit_code,
                outcome
                    .message
                    .unwrap_or_else(|| "evaluation failed".into()),
            );
            report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
            return report;
        }
        last_val = outcome.value;
    }

    let after_state = session.export_state();
    let mut changed_globals = BTreeMap::new();
    for (k, v) in &after_state.globals {
        if before_globals.get(k) != Some(v) {
            changed_globals.insert(k.clone(), v.clone());
        }
    }
    if let Some(save) = &opts.save_state {
        let bytes = match serde_json::to_vec_pretty(&after_state) {
            Ok(b) => b,
            Err(e) => {
                report.fail("infrastructure_error", 70, e.to_string());
                return report;
            }
        };
        if let Err(e) = atomic_bytes(save, &bytes) {
            report.fail(
                "io_error",
                5,
                format!("cannot write {}: {e}", save.display()),
            );
            return report;
        }
        report.artifacts.push(save.clone());
    }

    report.output = outputs;
    report.return_value = last_val;
    report.instructions = total_steps;
    report.data = json!({
        "action": "run",
        "angle": angle_name(session.context.settings.angle),
        "changed_globals": changed_globals,
        "state": after_state,
    });
    report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
    report
}
