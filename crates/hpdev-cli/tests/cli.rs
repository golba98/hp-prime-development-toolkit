use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
const BIN: &str = env!("CARGO_BIN_EXE_hpdev");
fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn invoke(args: &[&str], script: &str) -> (i32, Value, String) {
    let mut child = Command::new(BIN)
        .current_dir(root())
        .env("PPL_ENGINE", "firmware")
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
        .write_all(script.as_bytes())
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
    let deadline = Instant::now() + Duration::from_secs(10);
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
        .unwrap_or_else(|e| panic!("invalid JSON: {e}\n{out}\nstderr: {err}"));
    (status.code().unwrap(), value, err)
}
#[test]
fn check_run_and_native_default() {
    let (code, r, _) = invoke(&["check", "examples/HELLO.ppl"], "");
    assert_eq!(code, 0);
    assert_eq!(r["engine"], "native");
    let (code, r, _) = invoke(&["run", "examples/SQUARE.ppl", "--args", "[7]"], "");
    assert_eq!(code, 0);
    assert_eq!(r["return_value"], json!({"type":"real","value":"49"}));
}
#[test]
fn source_and_argument_failures_are_single_json_documents() {
    for (args, code, status) in [
        (
            vec!["check", "tests/fixtures/INVALID.ppl"],
            1,
            "invalid_source",
        ),
        (vec!["run", "missing.ppl"], 5, "io_error"),
        (
            vec!["run", "examples/SQUARE.ppl", "--args", "oops"],
            2,
            "invalid_request",
        ),
        (
            vec![
                "run",
                "examples/SQUARE.ppl",
                "--args",
                "[7]",
                "--engine",
                "firmware",
            ],
            8,
            "unsupported",
        ),
        (
            vec!["run", "examples/HELLO.ppl", "--max-time", "NaN"],
            2,
            "invalid_request",
        ),
        (vec!["run", "--bad-option"], 2, "invalid_request"),
    ] {
        let (c, r, _) = invoke(&args, "");
        assert_eq!(c, code, "{r}");
        assert_eq!(r["status"], status);
    }
}
#[test]
fn debugger_exposes_actual_values_and_frames() {
    let (c, r, _) = invoke(
        &[
            "debug",
            "examples/DEBUG.ppl",
            "--no-tui",
            "--break",
            "5",
            "--run",
        ],
        "p X\nn\np Y\nbt\nup\np A\ndown\nout\np B\nc\n",
    );
    assert_eq!(c, 0, "{r}");
    let history = r["data"]["debug_history"].as_array().unwrap();
    assert_eq!(history[0]["snapshot"]["locals"]["X"]["value"], "3");
    let replies = history
        .iter()
        .filter_map(|e| e["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for value in ["X = 3", "Y = 6", "A = 3", "B = 6", "#0 DOUBLE"] {
        assert!(replies.contains(value), "{replies}");
    }
    assert_eq!(r["return_value"]["value"], "11");
}
#[test]
fn debugger_eof_and_quit_are_cancellation() {
    for script in ["", "q\n"] {
        let (c, r, _) = invoke(&["debug", "examples/DEBUG.ppl", "--no-tui"], script);
        assert_eq!(c, 130);
        assert_eq!(r["status"], "cancelled");
    }
}
#[test]
fn runtime_trace_points_to_real_source() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("ERR.ppl");
    std::fs::write(&file, "EXPORT ERR()\nBEGIN\n RETURN 1/0;\nEND;").unwrap();
    let (c, r, _) = invoke(&["run", file.to_str().unwrap()], "");
    assert_eq!(c, 3);
    assert_eq!(r["stack"][0]["location"]["line"], 3);
    assert!(r["runtime_error"].as_str().unwrap().contains("Division"));
}
#[test]
fn png_is_native_execution_and_write_failures_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("screen.png");
    let (c, r, _) = invoke(
        &[
            "preview",
            "examples/DRAW.hpprgm",
            "--png",
            path.to_str().unwrap(),
        ],
        "",
    );
    assert_eq!(c, 0, "{r}");
    let image = ppl_graphics::Grob::read_png(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!((image.width(), image.height()), (320, 240));
    assert_eq!(image.get(0, 0), Some(0x0f2850));
    let (c, r, _) = invoke(
        &[
            "preview",
            "examples/DRAW.hpprgm",
            "--png",
            dir.path().join("missing/out.png").to_str().unwrap(),
        ],
        "",
    );
    assert_eq!(c, 5);
    assert_eq!(r["status"], "io_error");
}
#[test]
fn scripted_key_and_input_exhaustion_are_distinct() {
    let (c, r, _) = invoke(&["run", "tests/fixtures/WAIT.ppl", "--keys", "4"], "");
    assert_eq!(c, 0, "{r}");
    assert_eq!(r["return_value"]["value"], "4");
    let (c, r, _) = invoke(&["run", "tests/fixtures/WAIT.ppl"], "");
    assert_eq!(c, 7);
    assert_eq!(r["status"], "input_exhausted");
}
#[test]
fn instruction_output_and_deadline_limits_stop_programs() {
    let (c, _, _) = invoke(
        &["run", "tests/fixtures/LOOP.ppl", "--max-steps", "1024"],
        "",
    );
    assert_eq!(c, 6);
    let (c, r, _) = invoke(
        &[
            "run",
            "tests/fixtures/LOOP.ppl",
            "--max-steps",
            "18446744073709551615",
            "--max-time",
            "0.05",
        ],
        "",
    );
    assert_eq!(c, 4);
    assert_eq!(r["status"], "timed_out");
    let (c, _, _) = invoke(
        &["run", "examples/HELLO.ppl", "--max-output-chars", "1"],
        "",
    );
    assert_eq!(c, 6);
}
#[test]
fn trace_is_opt_in_and_preserves_result() {
    let (c, r, _) = invoke(&["trace", "examples/DEBUG.ppl", "--variables"], "");
    assert_eq!(c, 0, "{r}");
    assert!(r["trace"].as_array().unwrap().iter().any(|e| {
        e["kind"] == "statement"
            && e["observed_changes"]
                .as_object()
                .is_some_and(|m| !m.is_empty())
    }));
    assert!(
        r["trace"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "presentation")
    );
    assert_eq!(r["return_value"]["value"], "11");
}
#[test]
fn example_suite_and_doctor_are_headless() {
    let (c, r, _) = invoke(&["test"], "");
    assert_eq!(c, 0, "{r}");
    assert_eq!(r["data"]["passed"], 5);
    let (c, r, _) = invoke(&["doctor"], "");
    assert_eq!(c, 0);
    assert_eq!(r["data"]["native_engine"], true);
    assert_eq!(r["data"]["firmware_required"], false);
}
#[test]
fn report_cannot_overwrite_input() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("A.ppl");
    let source = "EXPORT TST() BEGIN RETURN 1; END;";
    std::fs::write(&file, source).unwrap();
    let (c, _, _) = invoke(
        &[
            "run",
            file.to_str().unwrap(),
            "--report",
            file.to_str().unwrap(),
        ],
        "",
    );
    assert_eq!(c, 2);
    assert_eq!(std::fs::read_to_string(file).unwrap(), source);
}

#[test]
fn a_missing_input_is_protected_from_absolute_report_aliases() {
    let root = root().canonicalize().unwrap();
    let dir = tempfile::tempdir_in(root.join("target")).unwrap();
    let source = dir.path().join("MISSING.ppl");
    let relative = source.strip_prefix(&root).unwrap();
    let (code, report, _) = invoke(
        &[
            "check",
            relative.to_str().unwrap(),
            "--report",
            source.to_str().unwrap(),
        ],
        "",
    );
    assert_eq!(code, 2, "{report}");
    assert!(!source.exists());
}

#[test]
fn outer_deadline_preempts_a_long_native_graphics_operation() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("LONG.ppl");
    std::fs::write(
        &file,
        "EXPORT LONG() BEGIN LINE_P(0,0,1000000000,1000000000); END;",
    )
    .unwrap();
    let start = Instant::now();
    let (code, report, _) = invoke(&["run", file.to_str().unwrap(), "--max-time", "0.05"], "");
    assert_eq!(code, 4, "{report}");
    assert_eq!(report["status"], "timed_out");
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[cfg(unix)]
#[test]
fn watch_debounces_atomic_replacement_and_cancels_previous_run() {
    use std::os::unix::process::ExitStatusExt;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("A.ppl");
    let report = dir.path().join("latest.json");
    std::fs::write(&file, "EXPORT TST() BEGIN WHILE 1 DO END; END;").unwrap();
    let mut child = Command::new(BIN)
        .args([
            "watch",
            file.to_str().unwrap(),
            "--no-time-limit",
            "--max-steps",
            "18446744073709551615",
            "--report",
            report.to_str().unwrap(),
            "--json",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let errors = std::thread::spawn(move || {
        let mut s = String::new();
        stderr.read_to_string(&mut s).unwrap();
        s
    });
    std::thread::sleep(Duration::from_millis(200));
    let replacement = dir.path().join("replacement");
    std::fs::write(&replacement, "EXPORT TST() BEGIN RETURN 42; END;").unwrap();
    std::fs::rename(replacement, &file).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if std::fs::read(&report)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .is_some_and(|r| r["return_value"]["value"] == "42")
        {
            break;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("watch did not rerun");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    // Send SIGINT to the toolkit process, never to a calculator or firmware process.
    let signal = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(signal.success());
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code().or(output.status.signal()), Some(130));
    let r: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(r["data"]["runs"], 2);
    assert_eq!(r["data"]["last_run"]["return_value"]["value"], "42");
    let _ = errors.join().unwrap();
}

#[cfg(unix)]
#[test]
fn watch_shows_precise_diagnostics_without_a_json_report() {
    let mut child = Command::new(BIN)
        .current_dir(root())
        .args([
            "watch",
            "tests/fixtures/INVALID.ppl",
            "--check-only",
            "--quiet",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = child.stderr.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(stderr).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut found = false;
    while Instant::now() < deadline {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(50))
            && line.contains("undeclared-identifier")
        {
            assert!(line.contains(":3:10"), "{line}");
            found = true;
            break;
        }
    }
    let status = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let deadline = Instant::now() + Duration::from_secs(3);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("watch did not cancel");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(rx);
    reader.join().unwrap();
    assert!(found, "watch did not show compiler diagnostics");
}

#[test]
fn tolerance_is_explicit_and_mismatches_produce_artifacts() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("A.ppl");
    std::fs::write(
        &source,
        "EXPORT TST() BEGIN RECT_P(0,0,319,239,RGB(10,20,30)); END;",
    )
    .unwrap();
    let reference = dir.path().join("reference.png");
    ppl_graphics::Grob::new(320, 240, 0x0b141e)
        .save_png(&reference)
        .unwrap();
    let manifest = dir.path().join("hpdev.toml");
    let body = "schema_version=1\n[[test]]\nname='image'\nfile='A.ppl'\n[test.expect]\npng='reference.png'\n";
    std::fs::write(&manifest, body).unwrap();
    let artifacts = dir.path().join("artifacts");
    let (c, r, _) = invoke(
        &[
            "test",
            manifest.to_str().unwrap(),
            "--artifacts-dir",
            artifacts.to_str().unwrap(),
        ],
        "",
    );
    assert_eq!(c, 1, "{r}");
    assert_eq!(r["artifacts"].as_array().unwrap().len(), 2);
    for path in r["artifacts"].as_array().unwrap() {
        assert!(Path::new(path.as_str().unwrap()).exists());
    }
    std::fs::write(&manifest, format!("{body}channel_tolerance=1\n")).unwrap();
    let (c, r, _) = invoke(&["test", manifest.to_str().unwrap()], "");
    assert_eq!(c, 0, "{r}");
}

#[test]
fn explicit_headless_runs_need_no_display_and_preserve_args_and_scripts() {
    for (file, extra, expected) in [
        ("examples/SQUARE.ppl", vec!["--args", "[7]"], "49"),
        ("tests/fixtures/WAIT.ppl", vec!["--keys", "4"], "4"),
    ] {
        let output = Command::new(BIN)
            .current_dir(root())
            .args(["run", file, "--headless", "--json"])
            .args(extra)
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("WAYLAND_SOCKET")
            .output()
            .unwrap();
        assert!(output.status.success(), "{:?}", output);
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["return_value"]["value"], expected);
        assert_eq!(report["data"]["viewer"]["desktop"], false);
    }
}
#[test]
#[cfg(all(target_os = "linux", feature = "desktop"))]
fn requested_gui_reports_display_initialization_failure_without_fallback() {
    let output = Command::new(BIN)
        .current_dir(root())
        .args(["run", "tests/fixtures/WAIT.ppl", "--gui", "--json"])
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("WAYLAND_SOCKET")
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["status"], "infrastructure_error");
    assert!(report["error"].as_str().unwrap().contains("--headless"));
    assert_eq!(report["frames_presented"], 0);
}

#[test]
fn unicode_and_space_paths_preserve_native_arguments_and_reports() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("計算 é file.ppl");
    let report_path = directory.path().join("résultat 計算.json");
    std::fs::write(&file, "EXPORT MAIN(X) BEGIN RETURN X+1; END;").unwrap();
    let (code, report, _) = invoke(
        &[
            "run",
            file.to_str().unwrap(),
            "--headless",
            "--entry",
            "MAIN",
            "--args",
            "[41]",
            "--report",
            report_path.to_str().unwrap(),
        ],
        "",
    );
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["return_value"]["value"], "42");
    let saved: Value = serde_json::from_slice(&std::fs::read(report_path).unwrap()).unwrap();
    assert_eq!(saved, report);
}
