//! End-to-end CLI integration tests for expanded commands and improved existing commands.
use serde_json::Value;
use std::{
    io::{Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const BIN: &str = env!("CARGO_BIN_EXE_hpdev");

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn invoke_json(args: &[&str], stdin_data: &str) -> (i32, Value, String) {
    let mut child = Command::new(BIN)
        .current_dir(root())
        .args(args)
        .arg("--json")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin_data.as_bytes())
        .unwrap();
    let mut out = child.stdout.take().unwrap();
    let stdout = std::thread::spawn(move || {
        let mut s = String::new();
        out.read_to_string(&mut s).unwrap();
        s
    });
    let mut err = child.stderr.take().unwrap();
    let stderr = std::thread::spawn(move || {
        let mut s = String::new();
        err.read_to_string(&mut s).unwrap();
        s
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("hpdev timed out: {args:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let out = stdout.join().unwrap();
    let err = stderr.join().unwrap();
    let value = serde_json::from_str(&out)
        .unwrap_or_else(|e| panic!("invalid JSON: {e}\nstdout:\n{out}\nstderr:\n{err}"));
    (status.code().unwrap(), value, err)
}

fn invoke_human(args: &[&str], stdin_data: &str) -> (i32, String, String) {
    let mut child = Command::new(BIN)
        .current_dir(root())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin_data.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn existing_command_enhancements_work_end_to_end() {
    // check --dump-bytecode & --strict
    let (c, r, _) = invoke_json(&["check", "examples/SQUARE.ppl", "--dump-bytecode"], "");
    assert_eq!(c, 0, "{r}");
    assert!(
        r["data"]["disassembly"]
            .as_str()
            .unwrap()
            .contains("fn SQUARE")
    );

    let (c, r, _) = invoke_json(&["check", "tests/fixtures/UNSUPPORTED.ppl", "--strict"], "");
    assert_eq!(c, 1);
    assert_eq!(r["status"], "policy_failed");

    // run --headless --png
    let dir = tempfile::tempdir().unwrap();
    let png = dir.path().join("run_screen.png");
    let (c, r, _) = invoke_json(
        &[
            "run",
            "examples/DRAW.hpprgm",
            "--headless",
            "--png",
            png.to_str().unwrap(),
        ],
        "",
    );
    assert_eq!(c, 0, "{r}");
    assert!(png.exists());

    // trace --filter & --summary
    let (c, r, _) = invoke_json(
        &[
            "trace",
            "examples/DEBUG.ppl",
            "--filter",
            "DOUBLE",
            "--summary",
        ],
        "",
    );
    assert_eq!(c, 0, "{r}");
    let trace_fns: Vec<&str> = r["trace"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["function"].as_str())
        .collect();
    assert!(trace_fns.contains(&"DOUBLE"));
    assert!(!trace_fns.contains(&"MAIN"));
    assert!(r["data"]["trace_summary"]["DOUBLE"].as_u64().unwrap() >= 1);

    // debug --break-if
    let (c, r, _) = invoke_json(
        &[
            "debug",
            "examples/DEBUG.ppl",
            "--no-tui",
            "--run",
            "--break-if",
            "5=X==3",
        ],
        "eval X*10\nc\n",
    );
    assert_eq!(c, 0, "{r}");
    let replies: String = r["data"]["debug_history"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(replies.contains("X*10 = 30"), "{replies}");

    // doctor enriched metadata
    let (c, r, _) = invoke_json(&["doctor"], "");
    assert_eq!(c, 0);
    assert!(r["data"]["builtins"]["total"].as_u64().unwrap() >= 160);
    assert!(r["data"]["commands"].as_array().unwrap().len() >= 25);
}

#[test]
fn eval_repl_compat_ast_symbols_disasm_lint_format_inspect() {
    let (c, r, _) = invoke_json(&["eval", "2+3*4"], "");
    assert_eq!(c, 0);
    assert_eq!(r["return_value"]["value"], "14");

    let (c, out, _) = invoke_human(&["eval", "SIN(90)", "--angle", "deg"], "");
    assert_eq!(c, 0);
    assert_eq!(out.lines().next().unwrap().trim(), "1");

    let (c, r, _) = invoke_json(&["repl"], "X := 7\nX * 6\n:quit\n");
    assert_eq!(c, 0, "{r}");
    assert_eq!(r["return_value"]["value"], "42");

    let (c, r, _) = invoke_json(&["compat", "--commands", "--search", "SIN"], "");
    assert_eq!(c, 0);
    assert!(r["data"]["count"].as_u64().unwrap() >= 1);

    let (c, _r, _) = invoke_json(
        &["compat", "tests/fixtures/UNSUPPORTED.ppl", "--strict"],
        "",
    );
    assert_eq!(c, 1);

    let (c, r, _) = invoke_json(&["ast", "examples/SQUARE.ppl", "--tokens"], "");
    assert_eq!(c, 0);
    assert!(!r["data"]["items"].as_array().unwrap().is_empty());
    assert!(!r["data"]["tokens"].as_array().unwrap().is_empty());

    let (c, r, _) = invoke_json(&["symbols", "examples/DEBUG.ppl", "--exported-only"], "");
    assert_eq!(c, 0);
    assert_eq!(r["data"]["functions"].as_array().unwrap().len(), 1);

    let (c, r, _) = invoke_json(
        &["disasm", "examples/SQUARE.ppl", "--function", "SQUARE"],
        "",
    );
    assert_eq!(c, 0);
    assert!(r["data"]["text"].as_str().unwrap().contains("fn SQUARE"));

    let (c, r, _) = invoke_json(&["inspect", "tests/fixtures/MINIMAL.hpprgm"], "");
    assert_eq!(c, 0);
    assert_eq!(r["data"]["container"]["format"], "hpprgm-binary");

    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("FMT.ppl");
    std::fs::write(&f, "export f(a) begin local u; return a+1; end;\n").unwrap();

    let (c, r, _) = invoke_json(&["lint", f.to_str().unwrap(), "--deny-warnings"], "");
    assert_eq!(c, 1, "{r}");

    let (c, _, _) = invoke_json(&["format", f.to_str().unwrap(), "--check"], "");
    assert_eq!(c, 1);

    let (c, _, _) = invoke_json(
        &[
            "format",
            f.to_str().unwrap(),
            "--write",
            "--uppercase-keywords",
        ],
        "",
    );
    assert_eq!(c, 0);
    let (c, _, _) = invoke_json(&["format", f.to_str().unwrap(), "--check"], "");
    assert_eq!(c, 0);
}

#[test]
fn bench_profile_coverage_plot_stats_init_package_docs_snapshot() {
    let (c, r, _) = invoke_json(
        &[
            "bench",
            "examples/SQUARE.ppl",
            "--args",
            "[5]",
            "--warmup",
            "1",
            "--iterations",
            "4",
        ],
        "",
    );
    assert_eq!(c, 0, "{r}");
    assert_eq!(r["return_value"]["value"], "25");

    let (c, r, _) = invoke_json(&["profile", "examples/DEBUG.ppl"], "");
    assert_eq!(c, 0, "{r}");
    assert!(r["data"]["profile"]["total_instructions"].as_u64().unwrap() > 0);

    let (c, r, _) = invoke_json(
        &["coverage", "--manifest", "hpdev.toml", "--fail-under", "50"],
        "",
    );
    assert_eq!(c, 0, "{r}");
    assert!(r["data"]["line_pct"].as_f64().unwrap() >= 50.0);

    let dir = tempfile::tempdir().unwrap();
    let plot_png = dir.path().join("sin.png");
    let (c, r, _) = invoke_json(
        &[
            "plot",
            "SIN(X)",
            "--xmin",
            "-3.14",
            "--xmax",
            "3.14",
            "--output",
            plot_png.to_str().unwrap(),
        ],
        "",
    );
    assert_eq!(c, 0, "{r}");
    assert!(plot_png.exists());

    let (c, r, _) = invoke_json(
        &[
            "stats",
            "--x",
            "1,2,3,4",
            "--y",
            "3,5,7,9",
            "--predict-y",
            "10",
        ],
        "",
    );
    assert_eq!(c, 0, "{r}");
    assert_eq!(r["data"]["regression"]["pred_y"]["y"], 21.0);

    let proj = dir.path().join("myproj");
    let (c, r, _) = invoke_json(&["init", proj.to_str().unwrap(), "--name", "MYAPP"], "");
    assert_eq!(c, 0, "{r}");
    let manifest = proj.join("hpdev.toml");
    let (c, r, _) = invoke_json(&["test", manifest.to_str().unwrap()], "");
    assert_eq!(c, 0, "{r}");

    let pkg = dir.path().join("SQUARE.hpprgm");
    let (c, r, _) = invoke_json(
        &[
            "package",
            "examples/SQUARE.ppl",
            "-o",
            pkg.to_str().unwrap(),
        ],
        "",
    );
    assert_eq!(c, 0, "{r}");

    // protect_output guard on package
    let (c, _, _) = invoke_json(
        &[
            "package",
            "examples/SQUARE.ppl",
            "-o",
            "examples/SQUARE.ppl",
        ],
        "",
    );
    assert_eq!(c, 2);

    let doc_md = dir.path().join("API.md");
    let (c, r, _) = invoke_json(
        &[
            "docs",
            "--file",
            "examples/SQUARE.ppl",
            "-o",
            doc_md.to_str().unwrap(),
        ],
        "",
    );
    assert_eq!(c, 0, "{r}");
    assert!(std::fs::read_to_string(&doc_md).unwrap().contains("SQUARE"));

    let snap = dir.path().join("snap.json");
    let (c, r, _) = invoke_json(
        &[
            "snapshot",
            "capture",
            "examples/SQUARE.ppl",
            "--args",
            "[6]",
            "--expr",
            "K := Ans",
            "-o",
            snap.to_str().unwrap(),
        ],
        "",
    );
    assert_eq!(c, 0, "{r}");
    let (c, r, _) = invoke_json(
        &[
            "snapshot",
            "run",
            "--state",
            snap.to_str().unwrap(),
            "--expr",
            "K + 4",
        ],
        "",
    );
    assert_eq!(c, 0, "{r}");
    assert_eq!(r["return_value"]["value"], "40");
}
