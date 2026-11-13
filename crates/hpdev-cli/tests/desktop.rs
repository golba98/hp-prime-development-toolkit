//! Real X11 window/pixel/input tests. Run explicitly under Xvfb (see docs/testing.md).
#![cfg(all(target_os = "linux", feature = "desktop"))]
use serde_json::Value;
use std::{
    collections::BTreeSet,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use x11rb::{
    CURRENT_TIME,
    connection::Connection,
    protocol::{
        xproto::{
            self, AtomEnum, ClientMessageEvent, ConnectionExt as _, EventMask, ImageFormat,
            InputFocus, MapState,
        },
        xtest::ConnectionExt as _,
    },
    rust_connection::RustConnection,
};

struct App {
    child: Child,
    conn: RustConnection,
    root: u32,
    window: u32,
    dir: tempfile::TempDir,
    stdout: Option<thread::JoinHandle<String>>,
    stderr: Option<thread::JoinHandle<String>>,
}
impl Drop for App {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn wait(mut predicate: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(8);
    while !predicate() {
        assert!(Instant::now() < until, "native window condition timed out");
        thread::sleep(Duration::from_millis(10));
    }
}
impl App {
    fn start(file: &Path, args: &[&str]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_hpdev"))
            .current_dir(root())
            .arg("run")
            .arg(file)
            .args(["--report", dir.path().join("report.json").to_str().unwrap()])
            .args(args)
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("WAYLAND_SOCKET")
            .env("WINIT_X11_SCALE_FACTOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = child.stdout.take().unwrap();
        let stdout = thread::spawn(move || {
            let mut text = String::new();
            output.read_to_string(&mut text).unwrap();
            text
        });
        let mut output = child.stderr.take().unwrap();
        let stderr = thread::spawn(move || {
            let mut text = String::new();
            output.read_to_string(&mut text).unwrap();
            text
        });
        let (conn, screen) =
            x11rb::connect(None).expect("requires Xvfb/X11; do not silently skip GUI integration");
        let root = conn.setup().roots[screen].root;
        let name = file.file_name().unwrap().to_string_lossy();
        let mut window = 0;
        wait(|| {
            assert!(
                child.try_wait().unwrap().is_none(),
                "hpdev exited before mapping a native window"
            );
            for id in conn.query_tree(root).unwrap().reply().unwrap().children {
                let text = window_title(&conn, id);
                if text.contains(name.as_ref())
                    && conn
                        .get_window_attributes(id)
                        .unwrap()
                        .reply()
                        .is_ok_and(|a| a.map_state == MapState::VIEWABLE)
                {
                    window = id;
                    return true;
                }
            }
            false
        });
        conn.set_input_focus(InputFocus::PARENT, window, CURRENT_TIME)
            .unwrap()
            .check()
            .unwrap();
        conn.flush().unwrap();
        Self {
            child,
            conn,
            root,
            window,
            dir,
            stdout: Some(stdout),
            stderr: Some(stderr),
        }
    }
    fn title(&self) -> String {
        window_title(&self.conn, self.window)
    }
    #[track_caller]
    fn title_contains(&self, text: &str) {
        let until = Instant::now() + Duration::from_secs(8);
        while !self.title().contains(text) {
            assert!(
                Instant::now() < until,
                "waiting for {text:?}; current title: {}",
                self.title()
            );
            thread::sleep(Duration::from_millis(10));
        }
        // The program may dismiss a dialog before its next MOUSE poll clears
        // its own touch latch. Allow two of BST's 50ms polling cycles.
        if text == "Running" {
            thread::sleep(Duration::from_millis(150));
        }
    }
    fn keycode(&self, keysym: u32) -> u8 {
        let setup = self.conn.setup();
        let reply = self
            .conn
            .get_keyboard_mapping(setup.min_keycode, setup.max_keycode - setup.min_keycode + 1)
            .unwrap()
            .reply()
            .unwrap();
        reply
            .keysyms
            .chunks(reply.keysyms_per_keycode as usize)
            .position(|row| row.first() == Some(&keysym))
            .map(|i| setup.min_keycode + i as u8)
            .unwrap_or_else(|| panic!("missing keysym {keysym:x}"))
    }
    fn key_state(&self, keysym: u32, down: bool) {
        self.conn
            .xtest_fake_input(
                if down { 2 } else { 3 },
                self.keycode(keysym),
                CURRENT_TIME,
                self.root,
                0,
                0,
                0,
            )
            .unwrap()
            .check()
            .unwrap();
        self.conn.flush().unwrap();
    }
    fn key(&self, keysym: u32) {
        self.key_state(keysym, true);
        thread::sleep(Duration::from_millis(12));
        self.key_state(keysym, false);
    }
    fn text(&self, text: &str) {
        for ch in text.chars() {
            let setup = self.conn.setup();
            let reply = self
                .conn
                .get_keyboard_mapping(setup.min_keycode, setup.max_keycode - setup.min_keycode + 1)
                .unwrap()
                .reply()
                .unwrap();
            let (index, shift) = reply
                .keysyms
                .chunks(reply.keysyms_per_keycode as usize)
                .enumerate()
                .find_map(|(i, row)| {
                    if row[0] == ch as u32 {
                        Some((i, false))
                    } else if row.get(1) == Some(&(ch as u32)) {
                        Some((i, true))
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| panic!("missing text character {ch:?}"));
            let code = setup.min_keycode + index as u8;
            if shift {
                self.key_state(0xffe1, true);
            }
            for kind in [2, 3] {
                self.conn
                    .xtest_fake_input(kind, code, CURRENT_TIME, self.root, 0, 0, 0)
                    .unwrap()
                    .check()
                    .unwrap();
                thread::sleep(Duration::from_millis(12));
            }
            if shift {
                self.key_state(0xffe1, false);
            }
        }
    }
    fn click(&self, x: i16, y: i16) {
        let pos = self
            .conn
            .translate_coordinates(self.window, self.root, x, y)
            .unwrap()
            .reply()
            .unwrap();
        self.conn
            .xtest_fake_input(6, 0, CURRENT_TIME, self.root, pos.dst_x, pos.dst_y, 0)
            .unwrap()
            .check()
            .unwrap();
        self.conn
            .xtest_fake_input(4, 1, CURRENT_TIME, self.root, 0, 0, 0)
            .unwrap()
            .check()
            .unwrap();
        self.conn.flush().unwrap();
        thread::sleep(Duration::from_millis(15));
        self.conn
            .xtest_fake_input(5, 1, CURRENT_TIME, self.root, 0, 0, 0)
            .unwrap()
            .check()
            .unwrap();
        self.conn.flush().unwrap();
    }
    fn pixels(&self, x: i16, y: i16, w: u16, h: u16) -> Vec<u32> {
        let image = self
            .conn
            .get_image(ImageFormat::Z_PIXMAP, self.window, x, y, w, h, !0)
            .unwrap()
            .reply()
            .unwrap();
        let format = self
            .conn
            .setup()
            .pixmap_formats
            .iter()
            .find(|f| f.depth == image.depth)
            .unwrap();
        assert_eq!(format.bits_per_pixel, 32);
        image
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| {
                if self.conn.setup().image_byte_order == xproto::ImageOrder::LSB_FIRST {
                    u32::from_le_bytes(*b) & 0xffffff
                } else {
                    u32::from_be_bytes(*b) & 0xffffff
                }
            })
            .collect()
    }
    fn framebuffer(&self) -> Vec<u32> {
        self.pixels(0, 32, 640, 480)
            .as_chunks::<1280>()
            .0
            .iter()
            .flat_map(|row| row[..640].iter().step_by(2).copied())
            .collect()
    }
    fn debug(&self) {
        self.click(100, 15);
        wait(|| {
            self.conn
                .get_geometry(self.window)
                .unwrap()
                .reply()
                .unwrap()
                .width
                == 1080
        });
    }
    fn pause(&self) {
        self.click(670, 47);
        self.title_contains("Paused:");
    }
    fn command(&self, text: &str) {
        self.click(670, 80);
        self.text(text);
        self.key(0xff0d);
        thread::sleep(Duration::from_millis(100));
    }
    fn close(&self) {
        let protocols = self
            .conn
            .intern_atom(false, b"WM_PROTOCOLS")
            .unwrap()
            .reply()
            .unwrap()
            .atom;
        let delete = self
            .conn
            .intern_atom(false, b"WM_DELETE_WINDOW")
            .unwrap()
            .reply()
            .unwrap()
            .atom;
        let event =
            ClientMessageEvent::new(32, self.window, protocols, [delete, CURRENT_TIME, 0, 0, 0]);
        self.conn
            .send_event(false, self.window, EventMask::NO_EVENT, event)
            .unwrap()
            .check()
            .unwrap();
        self.conn.flush().unwrap();
    }
    fn result(mut self) -> Value {
        wait(|| self.child.try_wait().unwrap().is_some());
        let status = self.child.wait().unwrap();
        let out = self.stdout.take().unwrap().join().unwrap();
        let err = self.stderr.take().unwrap().join().unwrap();
        let report: Value = serde_json::from_slice(
            &std::fs::read(self.dir.path().join("report.json"))
                .unwrap_or_else(|e| panic!("{e}; stdout={out}; stderr={err}")),
        )
        .unwrap();
        assert_eq!(
            status.code().unwrap() as u64,
            report["exit_code"].as_u64().unwrap(),
            "{err}"
        );
        if !out.trim().is_empty() && out.starts_with('{') {
            assert_eq!(serde_json::from_str::<Value>(&out).unwrap(), report);
        }
        report
    }
}
fn window_title(conn: &RustConnection, window: u32) -> String {
    let atom = conn
        .intern_atom(false, b"_NET_WM_NAME")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    conn.get_property(false, window, atom, AtomEnum::ANY, 0, 2048)
        .unwrap()
        .reply()
        .map(|r| String::from_utf8_lossy(&r.value).into_owned())
        .unwrap_or_default()
}
#[test]
#[ignore = "requires Xvfb; exercised by the native GUI CI job"]
fn default_run_creates_a_real_window_and_getkey_is_live() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("KEYLIVE.ppl");
    std::fs::write(&file,"EXPORT KEYLIVE()\nBEGIN\n LOCAL K;\n RECT_P(0,0,319,239,RGB(10,20,30),RGB(10,20,30));\n REPEAT\n K:=GETKEY;\n IF K==2 THEN RECT_P(0,0,319,239,RGB(20,80,120),RGB(20,80,120)); END;\n WAIT(0.02);\n UNTIL K==4;\n RETURN K;\nEND;").unwrap();
    let app = App::start(&file, &["--exit-on-finish"]);
    wait(|| app.pixels(0, 32, 2, 2) == vec![0x0a141e; 4]);
    thread::sleep(Duration::from_millis(200));
    assert!(app.title().contains("Running"));
    app.key(0xff52);
    wait(|| app.pixels(0, 32, 2, 2) == vec![0x145078; 4]);
    app.key(0xff1b);
    let r = app.result();
    assert_eq!(r["status"], "success");
    assert_eq!(r["return_value"]["value"], "4");
    assert_eq!(r["data"]["viewer"]["desktop"], true);
}
#[test]
#[ignore = "requires Xvfb; exercised by the native GUI CI job"]
fn native_window_presents_live_animation_and_holds_the_final_frame() {
    let app = App::start(&root().join("examples/LIVE.hpprgm"), &["--gui", "--json"]);
    let mut colors = BTreeSet::new();
    wait(|| {
        colors.insert(app.pixels(0, 32, 1, 1)[0]);
        app.title().contains("Done:")
    });
    assert!(
        colors.len() >= 4,
        "window did not display live frames: {colors:?}"
    );
    assert_eq!(app.pixels(0, 32, 2, 2), vec![200 << 16; 4]);
    assert!(app.conn.get_geometry(app.window).unwrap().reply().is_ok());
    app.close();
    let r = app.result();
    assert_eq!(r["status"], "success");
    assert_eq!(r["return_value"]["value"], "5");
}
#[test]
#[ignore = "requires Xvfb; exercised by the native GUI CI job"]
fn bst_visualizer_uses_real_touch_dialog_keyboard_and_debugger() {
    let app = App::start(
        &root().join("tests/fixtures/BSTVisualizer.hpprgm"),
        &["--exit-on-finish"],
    );
    app.title_contains("Running");
    wait(|| app.framebuffer()[230 * 320 + 300] != 0xffffff);
    let empty = app.framebuffer();
    assert_eq!(empty.len(), 320 * 240);
    assert!(empty.iter().any(|p| *p != 0xffffff));
    app.click(50, 492);
    app.title_contains("input: Insert node");
    app.text("42");
    app.key(0xff0d);
    app.title_contains("Running");
    wait(|| app.framebuffer() != empty);
    let one = app.framebuffer();
    app.click(50, 492);
    app.title_contains("input: Insert node");
    app.text("21");
    app.key(0xff0d);
    app.title_contains("Running");
    wait(|| app.framebuffer() != one);
    app.debug();
    app.pause();
    app.command("print bst_size");
    app.command("stack");
    let before_step = app.title();
    app.click(840, 47);
    wait(|| app.title().contains("Paused:") && app.title() != before_step); // Real statement step.
    app.click(755, 47);
    app.title_contains("Running");
    let unscrolled = app.framebuffer();
    app.key(0xff54);
    wait(|| app.framebuffer() != unscrolled);
    app.key(0xff52);
    wait(|| app.framebuffer() == unscrolled);
    app.click(160, 492);
    app.title_contains("input: Delete node");
    app.text("42");
    app.key(0xff0d);
    app.title_contains("Running");
    app.click(265, 492);
    app.title_contains("choose: Clear the tree?");
    app.key('1' as u32);
    app.title_contains("Running");
    wait(|| app.framebuffer() == empty);
    app.key(0xff1b);
    app.title_contains("choose: Exit BST Visualizer?");
    app.key('1' as u32);
    let r = app.result();
    assert_eq!(r["status"], "success");
    assert_eq!(r["return_value"]["value"], "0");
    let history = r["data"]["debug_history"].as_array().unwrap();
    assert!(
        history.iter().any(|e| e["text"]
            .as_str()
            .is_some_and(|s| s.contains("bst_size = 2"))),
        "inspection replies: {:?}",
        history
            .iter()
            .filter_map(|e| e["text"].as_str())
            .collect::<Vec<_>>()
    );
    assert!(history.iter().filter(|e| e["kind"] == "stop").count() >= 2);
    assert!(history.iter().any(|e| {
        e["text"]
            .as_str()
            .is_some_and(|s| s.contains("BSTVisualizer"))
    }));
}
#[test]
#[ignore = "requires Xvfb; exercised by the native GUI CI job"]
fn close_cancels_a_worker_blocked_on_input() {
    let app = App::start(&root().join("tests/fixtures/WAIT.ppl"), &[]);
    app.title_contains("Running");
    app.close();
    let r = app.result();
    assert_eq!(r["status"], "cancelled");
    assert_eq!(r["exit_code"], 130);
}

#[test]
#[ignore = "requires Xvfb; exercised by the native GUI CI job"]
fn keypad_and_scale_are_optional_and_use_real_calculator_codes() {
    let app = App::start(
        &root().join("tests/fixtures/WAIT.ppl"),
        &[
            "--show-keys",
            "--scale",
            "1",
            "--exit-on-finish",
            "--gui",
            "--json",
        ],
    );
    app.title_contains("Running");
    let size = app.conn.get_geometry(app.window).unwrap().reply().unwrap();
    assert_eq!((size.width, size.height), (580, 472));
    // Enter is calculator code 30, row 6 in the optional keypad.
    app.click(340, 32 + 6 * 40 + 15);
    let r = app.result();
    assert_eq!(r["status"], "success");
    assert_eq!(r["return_value"]["value"], "30");
}

#[test]
#[ignore = "requires Xvfb; exercised by the native GUI CI job"]
fn native_dialogs_validate_values_and_write_back_real_choices() {
    let app = App::start(
        &root().join("examples/INPUT.ppl"),
        &["--gui", "--json", "--exit-on-finish"],
    );
    app.title_contains("input: Number");
    app.text("bad");
    let before = app.framebuffer();
    app.key(0xff0d);
    wait(|| app.framebuffer() != before);
    app.title_contains("input: Number");
    app.text("7");
    app.key(0xff0d);
    app.title_contains("choose: Add to number");
    app.key(0xff54);
    app.key(0xff0d);
    app.title_contains("message:");
    app.key(0xff0d);
    let r = app.result();
    assert_eq!(r["status"], "success");
    assert_eq!(r["return_value"]["value"], "9");
}

#[test]
#[ignore = "requires Xvfb; exercised by the native GUI CI job"]
fn desktop_waits_beyond_the_old_default_deadline_but_honors_explicit_limits() {
    let app = App::start(
        &root().join("tests/fixtures/WAIT.ppl"),
        &["--gui", "--json", "--exit-on-finish"],
    );
    app.title_contains("Running");
    thread::sleep(Duration::from_secs(31));
    assert!(app.title().contains("Running"));
    app.key(0xff1b);
    assert_eq!(app.result()["status"], "success");

    let app = App::start(
        &root().join("tests/fixtures/WAIT.ppl"),
        &["--gui", "--json", "--exit-on-finish", "--max-time", "0.5"],
    );
    assert_eq!(app.result()["status"], "timed_out");
}

#[test]
#[ignore = "requires Xvfb; exercised by the native GUI CI job"]
fn held_keys_are_released_when_the_window_loses_focus() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("HELD.ppl");
    std::fs::write(&file,"EXPORT HELD()\nBEGIN\n LOCAL K;\n REPEAT\n K:=GETKEY;\n IF ISKEYDOWN(2) THEN RECT_P(0,0,319,239,RGB(1,2,200),RGB(1,2,200)); ELSE RECT_P(0,0,319,239,0,0); END;\n WAIT(0.02);\n UNTIL K==4;\n RETURN 0;\nEND;").unwrap();
    let app = App::start(&file, &["--exit-on-finish"]);
    app.key_state(0xff52, true);
    wait(|| app.pixels(0, 32, 2, 2) == vec![0x0102c8; 4]);
    app.conn
        .set_input_focus(InputFocus::PARENT, app.root, CURRENT_TIME)
        .unwrap()
        .check()
        .unwrap();
    app.conn.flush().unwrap();
    wait(|| app.pixels(0, 32, 2, 2) == vec![0; 4]);
    app.key_state(0xff52, false);
    app.conn
        .set_input_focus(InputFocus::PARENT, app.window, CURRENT_TIME)
        .unwrap()
        .check()
        .unwrap();
    app.conn.flush().unwrap();
    app.key(0xff1b);
    assert_eq!(app.result()["status"], "success");
}
