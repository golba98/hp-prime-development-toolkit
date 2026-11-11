//! Exercise raw mode, framebuffer colors and input through a real PTY.
#![cfg(unix)]
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde_json::Value;
use std::{
    io::{Read, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn session(args: &[&str], keys: Option<&[&[u8]]>) -> (u32, Value, String) {
    session_with_prompts(args, keys, &[])
}
fn session_with_prompts(
    args: &[&str],
    keys: Option<&[&[u8]]>,
    prompts: &[&str],
) -> (u32, Value, String) {
    let dir = tempfile::tempdir().unwrap();
    let report = dir.path().join("report.json");
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 36,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_hpdev"));
    command.cwd(root());
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    command.args(args);
    command.args(["--report", report.to_str().unwrap()]);
    let mut child = pair.slave.spawn_command(command).unwrap();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let captured = Arc::new(Mutex::new(Vec::new()));
    let reader_output = Arc::clone(&captured);
    let output = std::thread::spawn(move || {
        let mut chunk = [0; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(count) => reader_output
                    .lock()
                    .unwrap()
                    .extend_from_slice(&chunk[..count]),
            }
        }
    });
    let mut writer = pair.master.take_writer().unwrap();
    if let Some(keys) = keys {
        assert!(prompts.is_empty() || prompts.len() == keys.len());
        if prompts.is_empty() {
            std::thread::sleep(Duration::from_millis(250));
        }
        for (index, chunk) in keys.iter().enumerate() {
            if let Some(prompt) = prompts.get(index) {
                let deadline = Instant::now() + Duration::from_secs(8);
                loop {
                    let terminal = String::from_utf8_lossy(&captured.lock().unwrap()).into_owned();
                    if terminal.contains(prompt) {
                        break;
                    }
                    if Instant::now() > deadline || child.try_wait().unwrap().is_some() {
                        let _ = child.kill();
                        let _ = child.wait();
                        panic!(
                            "PTY never displayed {prompt:?}: {args:?}; terminal output: {terminal}"
                        );
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            writer.write_all(chunk).unwrap();
            writer.flush().unwrap();
            if prompts.is_empty() {
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
    let deadline = Instant::now() + Duration::from_secs(8);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "PTY command did not terminate: {args:?}; terminal output: {}",
                String::from_utf8_lossy(&captured.lock().unwrap())
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    drop(writer);
    drop(pair.master);
    output.join().unwrap();
    let output = String::from_utf8_lossy(&captured.lock().unwrap()).into_owned();
    let json = std::fs::read(&report)
        .unwrap_or_else(|e| panic!("no report: {e}; terminal output: {output}"));
    (
        status.exit_code(),
        serde_json::from_slice(&json).unwrap(),
        output,
    )
}
#[test]
fn renders_all_five_native_colors_and_restores_terminal() {
    let (code, r, output) = session(
        &[
            "preview",
            "examples/LIVE.hpprgm",
            "--tui",
            "--color",
            "truecolor",
            "--exit-on-finish",
        ],
        None,
    );
    assert_eq!(code, 0, "{r}");
    assert!(r["frames_presented"].as_u64().unwrap() >= 5);
    for red in [40, 80, 120, 160, 200] {
        assert!(
            output.contains(&format!("2;{red};0;0")),
            "missing real framebuffer color {red}"
        );
    }
    assert!(
        output.contains("\u{1b}[?1049l"),
        "alternate screen was not restored"
    );
}
#[test]
fn wait_receives_calculator_escape() {
    let (code, r, _) = session(
        &[
            "preview",
            "tests/fixtures/WAIT.ppl",
            "--tui",
            "--exit-on-finish",
        ],
        Some(&[b"\x1b"]),
    );
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["return_value"]["value"], "4");
}
#[test]
fn control_c_terminates_a_program_waiting_for_input() {
    let (code, r, output) = session(
        &[
            "preview",
            "tests/fixtures/WAIT.ppl",
            "--tui",
            "--exit-on-finish",
            "--no-time-limit",
        ],
        Some(&[b"\x03"]),
    );
    assert_eq!(code, 130, "{r}");
    assert_eq!(r["status"], "cancelled");
    assert!(output.contains("\u{1b}[?1049l"));
}
#[test]
fn terminal_debugger_runs_real_steps() {
    let (code, r, _) = session(
        &["debug", "examples/DEBUG.ppl", "--tui", "--exit-on-finish"],
        Some(&[b"n", b"n", b"n", b"c"]),
    );
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["return_value"]["value"], "11");
    assert!(r["data"]["debug_history"].as_array().unwrap().len() >= 4);
}
#[test]
fn terminal_dialog_response_is_written_back() {
    let (code, r, _) = session_with_prompts(
        &["preview", "examples/INPUT.ppl", "--tui", "--exit-on-finish"],
        Some(&[b"7\r", b"\x1b[B\r", b"\r"]),
        // Single words remain contiguous even when ratatui emits cursor moves
        // between words. Each belongs to a different rendered dialog.
        &["Number", "Three", "message"],
    );
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["return_value"]["value"], "9");
}

#[test]
fn running_debugger_routes_calculator_keys_to_the_real_host() {
    let (code, r, _) = session(
        &[
            "debug",
            "tests/fixtures/WAIT.ppl",
            "--tui",
            "--run",
            "--exit-on-finish",
        ],
        Some(&[b"\x1b"]),
    );
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["return_value"]["value"], "4");
}
