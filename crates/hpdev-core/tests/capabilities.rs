//! Tests for expanded hpdev-core analysis, evaluation, debugging, profiling, coverage, and tool services.
use hpdev_core::{
    Event, Report, Request,
    analysis::{
        analyze_compat, disassemble_program, extract_docs, format_source, inspect_ast,
        inspect_program, inspect_symbols, lint_source, list_commands,
    },
    eval::{EvalOptions, ReplSession, eval_expression},
    execution::execute,
    host::{Emit, InputState},
    testing::run_suite,
    tools::{
        PlotOptions, SnapshotCaptureOptions, SnapshotRunOptions, StatsOptions, bench_program,
        compute_stats, init_project, package_hpprgm, parse_csv_columns, parse_number_series,
        plot_functions, snapshot_capture, snapshot_inspect, snapshot_run,
    },
};
use serde_json::json;
use std::{
    cell::RefCell,
    path::PathBuf,
    rc::Rc,
    sync::{Arc, atomic::AtomicBool, mpsc},
};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn run_with_commands(request: Request, commands: Vec<&str>) -> (Report, Vec<Event>) {
    let (tx, rx) = mpsc::sync_channel(64);
    for cmd in commands {
        tx.send(hpdev_core::Control::Debug {
            command: cmd.to_owned(),
        })
        .unwrap();
    }
    let input = Rc::new(RefCell::new(InputState::new(
        rx,
        Arc::new(AtomicBool::new(false)),
    )));
    let events = Rc::new(RefCell::new(vec![]));
    let recorded = events.clone();
    let emit: Emit = Rc::new(move |e| recorded.borrow_mut().push(e));
    let report = execute(request, input, emit);
    let recorded = events.borrow().clone();
    (report, recorded)
}

#[test]
fn eval_preserves_decimal_precision_angle_modes_and_program_scope() {
    let r = eval_expression(EvalOptions {
        expr: "1/3".into(),
        ..Default::default()
    });
    assert_eq!(r.exit_code, 0, "{:?}", r.error);
    assert_eq!(
        r.return_value,
        Some(json!({"type": "real", "value": "0.333333333333"}))
    );

    let deg = eval_expression(EvalOptions {
        expr: "SIN(90)".into(),
        angle: "deg".into(),
        ..Default::default()
    });
    assert_eq!(deg.exit_code, 0);
    assert_eq!(deg.return_value.unwrap()["value"], "1");

    let rad = eval_expression(EvalOptions {
        expr: "COS(0)".into(),
        angle: "rad".into(),
        ..Default::default()
    });
    assert_eq!(rad.exit_code, 0);
    assert_eq!(rad.return_value.unwrap()["value"], "1");

    let scoped = eval_expression(EvalOptions {
        expr: "SQUARE(8)".into(),
        file: Some(root().join("examples/SQUARE.ppl")),
        ..Default::default()
    });
    assert_eq!(scoped.exit_code, 0, "{:?}", scoped.error);
    assert_eq!(scoped.return_value.unwrap()["value"], "64");

    let div0 = eval_expression(EvalOptions {
        expr: "1/0".into(),
        ..Default::default()
    });
    assert_eq!(div0.exit_code, 3);
    assert_eq!(div0.status, "runtime_error");

    let syntax = eval_expression(EvalOptions {
        expr: "1 + *".into(),
        ..Default::default()
    });
    assert_eq!(syntax.exit_code, 1);
    assert_eq!(syntax.status, "invalid_source");
}

#[test]
fn repl_session_persists_variables_ans_and_snapshots() {
    let mut session =
        ReplSession::new_bounded(Default::default(), 0x5eed, Some(100_000), Some(5.0));
    let o1 = session.eval_line("A := 6");
    assert_eq!(o1.exit_code, 0);
    let o2 = session.eval_line("A * 7");
    assert_eq!(o2.exit_code, 0);
    assert_eq!(o2.display.as_deref(), Some("42"));
    let o3 = session.eval_line("Ans + 1");
    assert_eq!(o3.exit_code, 0);
    assert_eq!(o3.display.as_deref(), Some("43"));

    let state = session.export_state();
    assert_eq!(state.globals["A"]["value"], "6");
    assert_eq!(state.globals["Ans"]["value"], "43");

    let mut restored = ReplSession::new_bounded(Default::default(), 1, Some(100_000), Some(5.0));
    restored.import_state(&state).unwrap();
    let o4 = restored.eval_line("A + Ans");
    assert_eq!(o4.exit_code, 0);
    assert_eq!(o4.display.as_deref(), Some("49"));

    let meta = restored.eval_line(":angle deg");
    assert_eq!(meta.exit_code, 0);
    let o5 = restored.eval_line("SIN(30)");
    assert_eq!(o5.display.as_deref(), Some("0.5"));
}

#[test]
fn ast_symbols_disasm_and_inspect_work_on_ppl_and_hpprgm() {
    let square = root().join("examples/SQUARE.ppl");
    let ast = inspect_ast(&square, true);
    assert_eq!(ast.exit_code, 0);
    assert!(!ast.data["items"].as_array().unwrap().is_empty());
    assert!(!ast.data["tokens"].as_array().unwrap().is_empty());
    assert!(
        ast.data["tree"]
            .as_str()
            .unwrap()
            .contains("Function(EXPORT SQUARE(N))")
    );

    let syms = inspect_symbols(&root().join("examples/DEBUG.ppl"), &[], false);
    assert_eq!(syms.exit_code, 0);
    let fns = syms.data["functions"].as_array().unwrap();
    assert_eq!(fns.len(), 2);

    let syms_exp = inspect_symbols(&root().join("examples/DEBUG.ppl"), &[], true);
    assert_eq!(syms_exp.data["functions"].as_array().unwrap().len(), 1);

    let disasm = disassemble_program(&square, &[], Some("SQUARE"));
    assert_eq!(disasm.exit_code, 0);
    assert!(disasm.data["text"].as_str().unwrap().contains("fn SQUARE"));

    let bad_fn = disassemble_program(&square, &[], Some("NO_SUCH_FN"));
    assert_eq!(bad_fn.exit_code, 2);

    let hpprgm = inspect_program(&root().join("tests/fixtures/MINIMAL.hpprgm"), &[]);
    assert_eq!(hpprgm.exit_code, 0);
    assert_eq!(hpprgm.data["container"]["format"], "hpprgm-binary");
}

#[test]
fn lint_detects_code_quality_rules_and_honors_deny_warnings() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("LINTME.ppl");
    std::fs::write(
        &file,
        r#"LOCAL shared_val;
helper(X)
begin
  LOCAL unused, shared_val;
  RETURN X;
  unused := 1;
end;
EXPORT MAIN()
BEGIN
  IF 1 THEN
  END;
  RETURN 42;
END;
"#,
    )
    .unwrap();

    let report = lint_source(&file, &[], false);
    assert_eq!(report.exit_code, 0);
    let codes: Vec<&str> = report.diagnostics.iter().map(|d| d.code.as_str()).collect();
    for expected in [
        "soft-keyword-casing",
        "unused-private-function",
        "shadowed-global",
        "unused-local",
        "unreachable-code",
        "empty-block",
    ] {
        assert!(codes.contains(&expected), "missing {expected} in {codes:?}");
    }

    let strict = lint_source(&file, &[], true);
    assert_eq!(strict.exit_code, 1);
    assert_eq!(strict.status, "policy_failed");
}

#[test]
fn format_normalizes_preserves_comments_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("MESSY.ppl");
    std::fs::write(
        &file,
        "// header comment\nexport FOO(X,Y) begin local Z; Z:=X+Y*2; if Z>0 then return Z; else return 0; end; end;\n",
    )
    .unwrap();

    let first = format_source(&file, 2, true).unwrap();
    assert!(first.changed);
    assert!(first.formatted.contains("// header comment"));
    assert!(first.formatted.contains("EXPORT FOO(X, Y)"));
    assert!(first.formatted.contains("  LOCAL Z;"));

    std::fs::write(&file, &first.formatted).unwrap();
    let second = format_source(&file, 2, true).unwrap();
    assert!(!second.changed);
    assert_eq!(second.formatted, first.formatted);
}

#[test]
fn compat_classifies_supported_and_unsupported_commands() {
    let ok = analyze_compat(&root().join("examples/HELLO.ppl"), &[], true);
    assert_eq!(ok.exit_code, 0);
    assert_eq!(ok.data["compatible"], true);

    let unsup = analyze_compat(&root().join("tests/fixtures/UNSUPPORTED.ppl"), &[], true);
    assert_eq!(unsup.exit_code, 1);
    assert_eq!(unsup.status, "policy_failed");
    assert_eq!(unsup.data["compatible"], false);

    let table = list_commands(Some("math"), None, Some("SIN"), false);
    assert_eq!(table.exit_code, 0);
    assert!(table.data["count"].as_u64().unwrap() >= 1);
}

#[test]
fn bench_profile_and_coverage_measure_real_execution() {
    let bench = bench_program(
        Request {
            operation: "bench".into(),
            file: root().join("examples/SQUARE.ppl"),
            args: "[9]".into(),
            ..Default::default()
        },
        1,
        5,
    );
    assert_eq!(bench.exit_code, 0);
    assert_eq!(bench.return_value.unwrap()["value"], "81");
    assert!(bench.instructions > 0);
    assert!(bench.data["median_ms"].as_f64().unwrap() >= 0.0);

    let (prof, _) = run_with_commands(
        Request {
            operation: "profile".into(),
            file: root().join("examples/DEBUG.ppl"),
            profile: true,
            ..Default::default()
        },
        vec![],
    );
    assert_eq!(prof.exit_code, 0);
    let fn_names: Vec<&str> = prof.data["profile"]["functions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["function"].as_str())
        .collect();
    assert!(fn_names.contains(&"DEBUG"));
    assert!(fn_names.contains(&"DOUBLE"));

    let dir = tempfile::tempdir().unwrap();
    let branch_file = dir.path().join("BRANCH.ppl");
    std::fs::write(
        &branch_file,
        "EXPORT BRANCH(X)\nBEGIN\n  IF X > 0 THEN\n    RETURN 1;\n  ELSE\n    RETURN 0;\n  END;\nEND;\n",
    )
    .unwrap();
    let (cov, _) = run_with_commands(
        Request {
            operation: "coverage".into(),
            file: branch_file,
            args: "[5]".into(),
            coverage: true,
            ..Default::default()
        },
        vec![],
    );
    assert_eq!(cov.exit_code, 0);
    assert_eq!(cov.data["coverage"]["uncovered_lines"], json!([6]));
    assert!(cov.data["coverage"]["line_pct"].as_f64().unwrap() < 100.0);
}

#[test]
fn debugger_supports_conditional_breakpoints_watches_and_eval() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("COND.ppl");
    std::fs::write(
        &file,
        "EXPORT COND()\nBEGIN\n  LOCAL I, S;\n  S := 0;\n  FOR I FROM 1 TO 5 DO\n    S := S + I;\n  END;\n  RETURN S;\nEND;\n",
    )
    .unwrap();

    let (r, _) = run_with_commands(
        Request {
            operation: "debug".into(),
            file,
            debug: true,
            run_to_breakpoint: true,
            conditional_breakpoints: vec![(6, "I == 4".into())],
            max_time: None,
            ..Default::default()
        },
        vec!["p I", "eval S + I", "w S * 2", "c"],
    );
    assert_eq!(r.exit_code, 0, "{:?}", r.error);
    assert_eq!(r.return_value.unwrap()["value"], "15");
    let history = r.data["debug_history"].as_array().unwrap();
    let stop = history
        .iter()
        .find(|e| e["kind"] == "stop" && e["snapshot"]["reason"] == "breakpoint")
        .expect("conditional breakpoint stop");
    assert_eq!(stop["snapshot"]["locals"]["I"]["value"], "4");
    assert_eq!(stop["snapshot"]["locals"]["S"]["value"], "6");
    let replies: Vec<&str> = history.iter().filter_map(|e| e["text"].as_str()).collect();
    assert!(replies.contains(&"I = 4"));
    assert!(replies.contains(&"S + I = 10"));
    let watch_stop = history.iter().rfind(|e| e["kind"] == "stop").unwrap();
    assert_eq!(watch_stop["snapshot"]["watches"]["S * 2"]["value"], "12");
}

#[test]
fn plot_renders_png_and_handles_singularities() {
    let dir = tempfile::tempdir().unwrap();
    let png = dir.path().join("plot.png");
    let r = plot_functions(PlotOptions {
        expressions: vec!["SIN(X)".into(), "1/X".into()],
        xmin: -5.0,
        xmax: 5.0,
        output: Some(png.clone()),
        ascii: true,
        ..Default::default()
    });
    assert_eq!(r.exit_code, 0, "{:?}", r.error);
    let bytes = std::fs::read(&png).unwrap();
    let img = hpdev_core::ppl_graphics::Grob::read_png(&bytes).unwrap();
    assert_eq!((img.width(), img.height()), (320, 240));
    assert!(img.pixels().iter().any(|&p| p != 0x00FF_FFFF));
    assert!(r.data["ascii"].as_str().unwrap().contains('*'));
}

#[test]
fn stats_computes_exact_1var_and_2var_regressions() {
    let xs = parse_number_series("[2, 4, 4, 4, 5, 5, 7, 9]").unwrap();
    let r1 = compute_stats(StatsOptions {
        x: xs,
        y: None,
        model: "linear".into(),
        predict_y_at: None,
        predict_x_at: None,
    });
    assert_eq!(r1.exit_code, 0);
    assert_eq!(r1.data["x"]["n"], 8);
    assert_eq!(r1.data["x"]["mean"], 5.0);
    assert_eq!(r1.data["x"]["pop_sd"], 2.0);
    assert_eq!(r1.data["x"]["median"], 4.5);
    assert_eq!(r1.data["x"]["q1"], 4.0);
    assert_eq!(r1.data["x"]["q3"], 6.0);

    let r2 = compute_stats(StatsOptions {
        x: vec![1.0, 2.0, 3.0, 4.0],
        y: Some(vec![5.0, 7.0, 9.0, 11.0]),
        model: "linear".into(),
        predict_y_at: Some(10.0),
        predict_x_at: Some(23.0),
    });
    assert_eq!(r2.exit_code, 0);
    let reg = &r2.data["regression"];
    assert!((reg["a"].as_f64().unwrap() - 2.0).abs() < 1e-12);
    assert!((reg["b"].as_f64().unwrap() - 3.0).abs() < 1e-12);
    assert!((reg["r_squared"].as_f64().unwrap() - 1.0).abs() < 1e-12);
    assert!((reg["pred_y"]["y"].as_f64().unwrap() - 23.0).abs() < 1e-12);
    assert!((reg["pred_x"]["x"].as_f64().unwrap() - 10.0).abs() < 1e-12);

    let dir = tempfile::tempdir().unwrap();
    let csv = dir.path().join("data.csv");
    std::fs::write(&csv, "x,y\n1,2\n2,4\n3,6\n").unwrap();
    let (cx, cy) = parse_csv_columns(&csv, "x", Some("y")).unwrap();
    assert_eq!(cx, vec![1.0, 2.0, 3.0]);
    assert_eq!(cy.unwrap(), vec![2.0, 4.0, 6.0]);
}

#[test]
fn init_package_docs_and_snapshot_workflows_end_to_end() {
    // init + test suite
    let dir = tempfile::tempdir().unwrap();
    for tmpl in ["basic", "graphics", "library"] {
        let proj = dir.path().join(tmpl);
        let r = init_project(&proj, "DEMO", tmpl, false);
        assert_eq!(r.exit_code, 0, "{:?}", r.error);
        let test_rep = run_suite(
            &proj.join("hpdev.toml"),
            &proj.join(".hpdev/test"),
            false,
            |req| {
                let (_tx, rx) = mpsc::sync_channel(8);
                let input = Rc::new(RefCell::new(InputState::new(
                    rx,
                    Arc::new(AtomicBool::new(false)),
                )));
                execute(req, input, Rc::new(|_| {}))
            },
        );
        assert_eq!(
            test_rep.exit_code, 0,
            "template {tmpl} failed: {test_rep:?}"
        );
    }

    // package pack & unpack round-trip
    let pkg_path = dir.path().join("SQUARE.hpprgm");
    let packed = package_hpprgm(&root().join("examples/SQUARE.ppl"), &pkg_path, false, false);
    assert_eq!(packed.exit_code, 0, "{:?}", packed.error);
    let (run_pkg, _) = run_with_commands(
        Request {
            file: pkg_path.clone(),
            args: "[7]".into(),
            ..Default::default()
        },
        vec![],
    );
    assert_eq!(run_pkg.exit_code, 0);
    assert_eq!(run_pkg.return_value.unwrap()["value"], "49");
    let unpacked_path = dir.path().join("UNPACKED.ppl");
    let unpacked = package_hpprgm(&pkg_path, &unpacked_path, true, false);
    assert_eq!(unpacked.exit_code, 0);
    assert!(
        std::fs::read_to_string(&unpacked_path)
            .unwrap()
            .contains("EXPORT SQUARE")
    );

    // docs extraction
    let doc_src = dir.path().join("DOC.ppl");
    std::fs::write(
        &doc_src,
        "// Compute the cube of X.\nEXPORT CUBE(X)\nBEGIN\n  RETURN X^3;\nEND;\n",
    )
    .unwrap();
    let docs = extract_docs(&doc_src);
    assert_eq!(docs.exit_code, 0);
    assert!(
        docs.data["markdown"]
            .as_str()
            .unwrap()
            .contains("Compute the cube of X.")
    );

    // snapshot capture -> inspect -> run
    let snap = dir.path().join("state.json");
    let cap = snapshot_capture(SnapshotCaptureOptions {
        file: None,
        exprs: vec!["A := 42".into()],
        output: snap.clone(),
        entry: None,
        args: "[]".into(),
        args_format: "json".into(),
        angle: "rad".into(),
        seed: 123,
        max_steps: Some(100_000),
        max_time: Some(5.0),
    });
    assert_eq!(cap.exit_code, 0, "{:?}", cap.error);
    let insp = snapshot_inspect(&snap);
    assert_eq!(insp.exit_code, 0);
    let sr = snapshot_run(SnapshotRunOptions {
        state_path: snap,
        save_state: None,
        file: None,
        entry: None,
        args: "[]".into(),
        args_format: "json".into(),
        expr: Some("A + 8".into()),
        angle: None,
        seed: None,
        max_steps: Some(100_000),
        max_time: Some(5.0),
    });
    assert_eq!(sr.exit_code, 0, "{:?}", sr.error);
    assert_eq!(sr.return_value.unwrap()["value"], "50");
}
