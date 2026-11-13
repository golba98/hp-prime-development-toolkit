//! Native calculator presentation over the supervised worker protocol.
mod platform;
#[allow(dead_code)] // Preserve the attributed upstream API and its regression tests.
pub mod upstream;
use crate::driver::{self, ViewOptions, Views};
use hpdev_core::{Control, DebugSnapshot, DialogResponse, DialogView, Event, Report, Request};
use ppl_graphics::{Grob, text::draw_text};
use ppl_vm::host::{Dialog, DialogResult, InputField, InputPosition, MatrixDialog};
use std::{
    collections::BTreeMap,
    num::NonZeroU32,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use upstream::{
    dialog::{KeyPress, ModalDialog, make_dialog},
    keymap,
    scaler::{Viewport, blit_scaled, window_to_framebuffer},
};
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow},
    keyboard::{Key, ModifiersState, NamedKey, PhysicalKey},
    window::{Window, WindowId},
};

const BAR: u32 = 32;
const KEYS_WIDTH: u32 = 260;
const DEBUG_WIDTH: u32 = 440;
const BG: u32 = 0x181e28;
const FG: u32 = 0xe4eaf4;
pub struct Bridge {
    last_pixels: std::cell::RefCell<Vec<u32>>,
    events: mpsc::SyncSender<Event>,
    controls: mpsc::Receiver<Control>,
}
impl Bridge {
    pub fn apply(&self, event: &Event) -> Result<(), String> {
        // The supervisor sends its final result after cancellation/deadline policy.
        if matches!(event, Event::Final { .. }) {
            return Ok(());
        }
        if let Event::Frame { pixels, .. } = event {
            let mut last = self.last_pixels.borrow_mut();
            if *last == *pixels {
                return Ok(());
            }
            last.clone_from(pixels);
        }
        self.events
            .send(event.clone())
            .map_err(|_| "desktop window disconnected".into())
    }
    pub fn poll(&self) -> Vec<Control> {
        self.controls.try_iter().collect()
    }
}
pub fn run(
    request: Request,
    options: ViewOptions,
    json: bool,
    quiet: bool,
    cancel: Arc<AtomicBool>,
) -> Report {
    let fallback = Report::new(&request.operation, Some(request.file.clone()));
    let mut app = Desktop::new(request, options, json, quiet, cancel);
    let result = platform::event_loop()
        .build()
        .map_err(|e| e.to_string())
        .and_then(|event_loop| event_loop.run_app(&mut app).map_err(|e| e.to_string()));
    app.cancel.store(true, Ordering::Release);
    app.events.take(); // Release a supervisor blocked on presentation.
    if let Some(worker) = app.worker.take() {
        let _ = worker.join();
    }
    let mut report = app.report.unwrap_or(fallback);
    if let Some(error) = app.error.or(result.err()) {
        report.fail(
            "infrastructure_error",
            70,
            format!("native desktop: {error}; use --headless for execution without a display"),
        );
    }
    report
}
struct Desktop {
    request: Option<Request>,
    options: ViewOptions,
    json: bool,
    quiet: bool,
    cancel: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    events: Option<mpsc::Receiver<Event>>,
    controls: Option<mpsc::SyncSender<Control>>,
    window: Option<Arc<Window>>,
    context: Option<softbuffer::Context<Arc<Window>>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    screen: Grob,
    viewport: Viewport,
    cursor: (f64, f64),
    modifiers: ModifiersState,
    held: BTreeMap<PhysicalKey, i64>,
    mouse_key: Option<i64>,
    pointer_down: bool,
    dialog: Option<Box<dyn ModalDialog>>,
    dialog_view: Option<DialogView>,
    snapshot: Option<Box<DebugSnapshot>>,
    show_debug: bool,
    command_focus: bool,
    command: String,
    reply: String,
    scroll: usize,
    pause_pending: bool,
    dirty: bool,
    closing: bool,
    occluded: bool,
    report: Option<Report>,
    error: Option<String>,
    name: String,
}
impl Desktop {
    fn new(
        request: Request,
        options: ViewOptions,
        json: bool,
        quiet: bool,
        cancel: Arc<AtomicBool>,
    ) -> Self {
        let name = request
            .file
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        Self {
            request: Some(request),
            options,
            json,
            quiet,
            cancel,
            worker: None,
            events: None,
            controls: None,
            window: None,
            context: None,
            surface: None,
            screen: Grob::screen(),
            viewport: Viewport::calculate_integer(640, 480),
            cursor: (0., 0.),
            modifiers: ModifiersState::empty(),
            held: BTreeMap::new(),
            mouse_key: None,
            pointer_down: false,
            dialog: None,
            dialog_view: None,
            snapshot: None,
            show_debug: false,
            command_focus: false,
            command: String::new(),
            reply: String::new(),
            scroll: 0,
            pause_pending: false,
            dirty: true,
            closing: false,
            occluded: false,
            report: None,
            error: None,
            name,
        }
    }
    fn send(&mut self, control: Control) {
        if self.report.is_some() {
            return;
        }
        if let Some(tx) = &self.controls {
            match tx.try_send(control) {
                Ok(()) | Err(mpsc::TrySendError::Disconnected(_)) => {}
                Err(mpsc::TrySendError::Full(_)) => {
                    self.error = Some("desktop input queue is full".into());
                    self.cancel.store(true, Ordering::Release);
                    self.closing = true;
                }
            }
        }
    }
    fn debug_command(&mut self, command: String) {
        if matches!(
            command.split_whitespace().next(),
            Some("c" | "continue" | "s" | "step" | "n" | "next" | "out")
        ) {
            self.snapshot = None;
            self.command_focus = false;
        }
        self.send(Control::Debug { command });
        self.dirty = true;
    }
    fn status(&self) -> String {
        if let Some(report) = &self.report {
            return format!("Done: {}", report.status);
        }
        if let Some(snapshot) = &self.snapshot {
            return format!("Paused: {}:{}", snapshot.function, snapshot.location.line);
        }
        if self.pause_pending {
            return "Pause pending (next statement)".into();
        }
        if let Some(dialog) = &self.dialog_view {
            return format!("{}: {}", dialog.kind, dialog.title);
        }
        "Running".into()
    }
    fn key_press(&mut self, key: KeyPress) {
        if key.code == Some(keymap::KEY_ON) && self.dialog.is_none() {
            self.cancel.store(true, Ordering::Release);
            return;
        }
        if let Some(dialog) = &mut self.dialog {
            if let Some(result) = dialog.handle(key) {
                let response = match result {
                    DialogResult::Ok => DialogResponse::Ok,
                    DialogResult::Cancelled => DialogResponse::Cancel,
                    DialogResult::Chosen(index) => DialogResponse::Choice { index },
                    DialogResult::Entered(values) => DialogResponse::Entries { values },
                };
                self.dialog = None;
                self.dialog_view = None;
                self.send(Control::Dialog { response });
            }
            self.dirty = true;
        } else if self.snapshot.is_none()
            && let Some(code) = key.code
        {
            self.send(Control::Key {
                code,
                down: true,
                pulse: false,
            });
        }
    }
    fn release_all(&mut self) {
        for (_, code) in std::mem::take(&mut self.held) {
            self.send(Control::Key {
                code,
                down: false,
                pulse: false,
            });
        }
        if let Some(code) = self.mouse_key.take() {
            self.send(Control::Key {
                code,
                down: false,
                pulse: false,
            });
        }
        self.pointer_down = false;
        self.send(Control::Pointer {
            x: 0,
            y: 0,
            down: false,
        });
    }
    fn resize_for_panels(&self) {
        if let Some(window) = &self.window {
            let width = 320 * self.options.scale
                + if self.options.show_keys {
                    KEYS_WIDTH
                } else {
                    0
                }
                + if self.show_debug { DEBUG_WIDTH } else { 0 };
            let _ = window.request_inner_size(LogicalSize::new(
                width,
                (240 * self.options.scale).max(if self.options.show_keys { 440 } else { 0 }) + BAR,
            ));
            window.set_min_inner_size(Some(LogicalSize::new(
                320 + if self.options.show_keys {
                    KEYS_WIDTH
                } else {
                    0
                } + if self.show_debug { DEBUG_WIDTH } else { 0 },
                240 + BAR,
            )));
        }
    }
    fn close(&mut self, event_loop: &ActiveEventLoop) {
        self.closing = true;
        self.cancel.store(true, Ordering::Release);
        if self.report.is_some() || self.worker.is_none() {
            event_loop.exit();
        }
    }
    fn fail(&mut self, event_loop: &ActiveEventLoop, message: impl ToString) {
        self.error = Some(message.to_string());
        self.close(event_loop);
    }
    fn apply(&mut self, event: Event) {
        match event {
            Event::Frame { pixels, .. } if pixels.len() == 320 * 240 => {
                self.screen.pixels_mut().copy_from_slice(&pixels)
            }
            Event::Dialog { dialog } => {
                self.release_all();
                self.command_focus = false;
                self.dialog = Some(make_dialog(&native_dialog(&dialog)));
                self.dialog_view = Some(dialog);
            }
            Event::Stop { snapshot } => {
                self.release_all();
                self.snapshot = Some(snapshot);
                self.pause_pending = false;
                self.scroll = 0;
            }
            Event::DebugReply { text } => self.reply = text,
            Event::Final { report } => {
                self.dialog = None;
                self.dialog_view = None;
                self.snapshot = None;
                self.report = Some(*report);
            }
            _ => {}
        }
        self.dirty = true;
    }
    fn panel_x(&self, width: u32) -> u32 {
        width.saturating_sub(if self.show_debug { DEBUG_WIDTH } else { 0 })
    }
    fn click(&mut self) {
        let dpi = self.window.as_ref().map_or(1., |w| w.scale_factor());
        let (x, y) = (self.cursor.0 / dpi, self.cursor.1 / dpi);
        if y < f64::from(BAR) {
            match (x as u32) / 72 {
                0 => {
                    self.options.show_keys = !self.options.show_keys;
                    self.resize_for_panels();
                }
                1 => {
                    self.show_debug = !self.show_debug;
                    self.command_focus = false;
                    self.resize_for_panels();
                }
                2 => {
                    self.cancel.store(true, Ordering::Release);
                }
                3 if self.dialog.is_some() => self.key_press(KeyPress::code(30)),
                4 if self.dialog.is_some() => self.key_press(KeyPress::code(4)),
                _ => {}
            }
            self.dirty = true;
            return;
        }
        let width = self.window.as_ref().map_or(640, |w| {
            w.inner_size().to_logical::<u32>(w.scale_factor()).width
        });
        let debug_x = self.panel_x(width);
        if self.show_debug && x >= f64::from(debug_x) {
            if y < 64. {
                self.command_focus = false;
                match ((x as u32 - debug_x) / 85).min(4) {
                    0 => {
                        if self.snapshot.is_none() {
                            self.pause_pending = true;
                            self.send(Control::Pause);
                        }
                    }
                    1 if self.snapshot.is_some() => self.debug_command("continue".into()),
                    2 if self.snapshot.is_some() => self.debug_command("step".into()),
                    3 if self.snapshot.is_some() => self.debug_command("next".into()),
                    4 if self.snapshot.is_some() => self.debug_command("out".into()),
                    _ => {}
                }
            } else if y < 96. {
                self.command_focus = true;
            }
            self.dirty = true;
            return;
        }
        self.command_focus = false;
        if self.options.show_keys && x >= f64::from(debug_x.saturating_sub(KEYS_WIDTH)) {
            let col = (x as u32 - debug_x.saturating_sub(KEYS_WIDTH)) / 52;
            let row = (y as u32 - BAR) / 40;
            let code = (row * 5 + col) as i64;
            if code <= 50 {
                self.mouse_key = Some(code);
                self.key_press(KeyPress::code(code));
            }
            return;
        }
        if let Some((cx, cy)) = window_to_framebuffer(self.cursor.0, self.cursor.1, &self.viewport)
        {
            if self.dialog.is_some() {
                if cy >= 190 {
                    self.key_press(KeyPress::code(if cx < 160 { 30 } else { 4 }));
                } else if let Some(view) = &self.dialog_view
                    && view.kind == "choose"
                    && (56..184).contains(&cy)
                {
                    let index = ((cy - 56) / 16 + 1) as usize;
                    if index <= view.items.len()
                        && let Some(code) = digit_code(index)
                    {
                        self.key_press(KeyPress::code(code));
                    }
                }
            } else if self.snapshot.is_none() {
                self.pointer_down = true;
                self.send(Control::Pointer {
                    x: cx,
                    y: cy,
                    down: true,
                });
            }
        }
    }
    fn draw(&mut self) -> Result<(), String> {
        let Some(window) = &self.window else {
            return Ok(());
        };
        let size = window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return Ok(());
        };
        if u64::from(w.get()) * u64::from(h.get()) > 64 * 1024 * 1024 {
            return Err("desktop surface exceeds 64 million pixels".into());
        }
        let sidebar = if self.options.show_keys {
            KEYS_WIDTH
        } else {
            0
        } + if self.show_debug { DEBUG_WIDTH } else { 0 };
        let dpi = window.scale_factor();
        self.viewport = platform::viewport(size.width, size.height, dpi, sidebar);
        let screen = self.dialog.as_ref().map(|d| d.render(&self.screen));
        let mut canvas = Grob::new(size.width as usize, size.height as usize, BG);
        blit_scaled(
            screen.as_ref().unwrap_or(&self.screen).pixels(),
            320,
            240,
            canvas.pixels_mut(),
            size.width as usize,
            size.height as usize,
            &self.viewport,
            BG,
        );
        let logical = size.to_logical::<u32>(dpi);
        let mut chrome = Grob::new(logical.width as usize, logical.height as usize, BG);
        chrome.fill_rect(0, 0, i64::from(logical.width), i64::from(BAR) - 1, 0x273344);
        for (i, text) in ["Keys", "Debug", "Stop"].iter().enumerate() {
            button(&mut chrome, i as i64 * 72, 0, 70, BAR as i64, text);
        }
        if self.dialog.is_some() {
            button(&mut chrome, 216, 0, 70, BAR as i64, "OK");
            button(&mut chrome, 288, 0, 70, BAR as i64, "Cancel");
        }
        let status = self.status();
        label(
            &mut chrome,
            &status,
            if self.dialog.is_some() { 364 } else { 220 },
            10,
            FG,
        );
        let debug_x = self.panel_x(logical.width);
        if self.options.show_keys {
            let left = debug_x.saturating_sub(KEYS_WIDTH);
            chrome.fill_rect(
                i64::from(left),
                i64::from(BAR),
                i64::from(debug_x) - 1,
                i64::from(logical.height),
                BG,
            );
            for code in 0..=50 {
                let x = i64::from(left) + (code % 5) * 52;
                let y = i64::from(BAR) + (code / 5) * 40;
                button(
                    &mut chrome,
                    x,
                    y,
                    50,
                    38,
                    keymap::hp_key_name(code).unwrap_or("?"),
                );
            }
        }
        if self.show_debug {
            let left = i64::from(debug_x);
            chrome.fill_rect(
                left,
                i64::from(BAR),
                i64::from(logical.width),
                i64::from(logical.height),
                BG,
            );
            for (i, text) in ["Pause", "Resume", "Step", "Next", "Out"]
                .iter()
                .enumerate()
            {
                button(
                    &mut chrome,
                    left + i as i64 * 85,
                    i64::from(BAR),
                    83,
                    32,
                    text,
                );
            }
            chrome.rect(
                left + 4,
                66,
                i64::from(logical.width) - 4,
                94,
                if self.command_focus {
                    0x64b5ff
                } else {
                    0x526078
                },
                0x10151e,
            );
            label(
                &mut chrome,
                &format!(
                    "> {}{}",
                    self.command,
                    if self.command_focus { "_" } else { "" }
                ),
                left + 8,
                74,
                FG,
            );
            let mut lines = vec![
                status,
                "Commands: break LINE, delete LINE, frame N,".into(),
                "print NAME, locals, globals, stack, up, down".into(),
            ];
            if let Some(snapshot) = &self.snapshot {
                lines.push(format!(
                    "{}:{}  frame {}",
                    snapshot.location.file, snapshot.location.line, snapshot.selected_frame
                ));
                for (line, text) in &snapshot.source_lines {
                    lines.push(format!(
                        "{} {:4} {}",
                        if *line == snapshot.location.line {
                            ">"
                        } else {
                            " "
                        },
                        line,
                        text
                    ));
                }
                lines.push("Locals".into());
                lines.extend(
                    snapshot
                        .locals
                        .iter()
                        .map(|(n, v)| format!("{n} = {}", crate::cli::display_value(v))),
                );
                lines.push("Globals".into());
                lines.extend(
                    snapshot
                        .globals
                        .iter()
                        .map(|(n, v)| format!("{n} = {}", crate::cli::display_value(v))),
                );
                lines.push("Stack".into());
                lines.extend(
                    snapshot
                        .stack
                        .iter()
                        .enumerate()
                        .map(|(i, f)| format!("#{i} {}:{}", f.function, f.location.line)),
                );
            }
            lines.extend(self.reply.lines().map(str::to_owned));
            self.scroll = self.scroll.min(lines.len().saturating_sub(1));
            for (i, line) in lines.iter().skip(self.scroll).enumerate() {
                let y = 104 + i as i64 * 18;
                if y + 16 >= i64::from(logical.height) {
                    break;
                }
                label(&mut chrome, line, left + 8, y, FG);
            }
        }
        let sidebar_x = size.width.saturating_sub(platform::pixels(sidebar, dpi));
        for y in 0..size.height {
            for x in 0..size.width {
                if y < platform::pixels(BAR, dpi) || x >= sidebar_x {
                    let lx = (f64::from(x) / dpi) as i64;
                    let ly = (f64::from(y) / dpi) as i64;
                    if let Some(color) = chrome.get(lx, ly) {
                        canvas.pixels_mut()[(y * size.width + x) as usize] = color;
                    }
                }
            }
        }
        window.set_ime_allowed(self.dialog.is_some() || self.command_focus);
        window.set_title(&format!("hpdev — {} — {}", self.name, self.status()));
        let surface = self.surface.as_mut().ok_or("desktop surface unavailable")?;
        surface.resize(w, h).map_err(|e| e.to_string())?;
        let mut buffer = surface.buffer_mut().map_err(|e| e.to_string())?;
        buffer.copy_from_slice(canvas.pixels());
        window.pre_present_notify();
        buffer.present().map_err(|e| e.to_string())?;
        self.dirty = false;
        Ok(())
    }
}
impl ApplicationHandler for Desktop {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let result = (|| -> Result<(), String> {
            let width = 320 * self.options.scale
                + if self.options.show_keys {
                    KEYS_WIDTH
                } else {
                    0
                };
            let window = Arc::new(
                event_loop
                    .create_window(
                        Window::default_attributes()
                            .with_title(format!("hpdev — {}", self.name))
                            .with_inner_size(LogicalSize::new(
                                width,
                                (240 * self.options.scale).max(if self.options.show_keys {
                                    440
                                } else {
                                    0
                                }) + BAR,
                            ))
                            .with_min_inner_size(LogicalSize::new(320, 240 + BAR)),
                    )
                    .map_err(|e| e.to_string())?,
            );
            let context = softbuffer::Context::new(window.clone()).map_err(|e| e.to_string())?;
            let surface =
                softbuffer::Surface::new(&context, window.clone()).map_err(|e| e.to_string())?;
            self.context = Some(context);
            self.surface = Some(surface);
            window.focus_window();
            self.window = Some(window);
            self.draw()?;
            let (event_tx, event_rx) = mpsc::sync_channel(8);
            let (control_tx, control_rx) = mpsc::sync_channel(64);
            self.events = Some(event_rx);
            self.controls = Some(control_tx);
            let request = self
                .request
                .take()
                .ok_or("desktop request already consumed")?;
            let (json, quiet, cancel) = (self.json, self.quiet, self.cancel.clone());
            self.worker = Some(std::thread::spawn(move || {
                let bridge = Bridge {
                    last_pixels: Default::default(),
                    events: event_tx.clone(),
                    controls: control_rx,
                };
                let mut views = Views::desktop(bridge);
                let mut outcome =
                    driver::execute(request, &mut views, json, quiet, &cancel, || false);
                outcome.report.data["viewer"] =
                    serde_json::json!({"desktop": true, "terminal": false, "browser": false});
                if !json {
                    crate::cli::print_diagnostics(&outcome.report);
                }
                let _ = event_tx.send(Event::Final {
                    report: Box::new(outcome.report),
                });
            }));
            Ok(())
        })();
        if let Err(e) = result {
            self.fail(event_loop, e);
        }
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        for _ in 0..16 {
            match self.events.as_ref().map(|rx| rx.try_recv()) {
                Some(Ok(event)) => self.apply(event),
                Some(Err(mpsc::TryRecvError::Disconnected)) if self.report.is_none() => {
                    self.fail(event_loop, "desktop supervisor ended without a report");
                    break;
                }
                _ => break,
            }
        }
        if self.cancel.load(Ordering::Acquire) && self.report.is_some() {
            self.closing = true;
        }
        let hidden = self.occluded
            || self
                .window
                .as_ref()
                .is_none_or(|w| w.inner_size().width == 0 || w.inner_size().height == 0);
        if self.report.is_some()
            && (self.closing || (self.options.exit_on_finish && (!self.dirty || hidden)))
        {
            event_loop.exit();
        } else if self.dirty
            && !hidden
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + Duration::from_millis(10),
        ));
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.close(event_loop),
            WindowEvent::RedrawRequested => {
                if let Err(e) = self.draw() {
                    self.fail(event_loop, e);
                }
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => self.dirty = true,
            WindowEvent::Occluded(occluded) => {
                self.occluded = occluded;
                self.dirty = true;
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                if self.command_focus {
                    if self.command.len() + text.len() <= 4096 {
                        self.command.push_str(&text);
                    }
                } else if self.dialog.is_some() {
                    for ch in text.chars().filter(|c| !c.is_control()) {
                        self.key_press(KeyPress {
                            code: None,
                            ch: Some(ch),
                        });
                    }
                }
                self.dirty = true;
            }
            WindowEvent::Focused(false) => {
                self.release_all();
                self.modifiers = ModifiersState::empty();
            }
            WindowEvent::ModifiersChanged(state) => self.modifiers = state.state(),
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
                if self.pointer_down
                    && let Some((x, y)) =
                        window_to_framebuffer(position.x, position.y, &self.viewport)
                {
                    self.send(Control::Pointer { x, y, down: true });
                }
            }
            WindowEvent::MouseWheel { delta, .. } if self.show_debug => {
                let amount = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as i32,
                    MouseScrollDelta::PixelDelta(p) => (p.y / 18.) as i32,
                };
                self.scroll = self.scroll.saturating_add_signed(-(amount as isize));
                self.dirty = true;
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                if state == ElementState::Pressed {
                    self.click();
                } else {
                    if let Some(code) = self.mouse_key.take() {
                        self.send(Control::Key {
                            code,
                            down: false,
                            pulse: false,
                        });
                    }
                    if self.pointer_down {
                        self.pointer_down = false;
                        self.send(Control::Pointer {
                            x: 0,
                            y: 0,
                            down: false,
                        });
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Released {
                    if let Some(code) = self.held.remove(&event.physical_key) {
                        self.send(Control::Key {
                            code,
                            down: false,
                            pulse: false,
                        });
                    }
                    return;
                }
                if platform::close_shortcut(&event.logical_key, self.modifiers) {
                    self.close(event_loop);
                    return;
                }
                if platform::command_modifier(self.modifiers) {
                    // Native Command accelerators must not become calculator keys.
                    return;
                }
                if keymap::is_break_key(
                    &event.logical_key,
                    event.physical_key,
                    self.modifiers.control_key(),
                ) {
                    self.cancel.store(true, Ordering::Release);
                    return;
                }
                if self.command_focus && self.show_debug {
                    match &event.logical_key {
                        Key::Named(NamedKey::Enter) => {
                            let command = std::mem::take(&mut self.command);
                            if self.snapshot.is_some() {
                                self.debug_command(command);
                            } else {
                                self.reply =
                                    "Pause execution before issuing debugger commands".into();
                            }
                        }
                        Key::Named(NamedKey::Space) if self.command.len() < 4096 => {
                            self.command.push(' ');
                        }
                        Key::Named(NamedKey::Backspace) => {
                            self.command.pop();
                        }
                        Key::Named(NamedKey::Escape) => self.command_focus = false,
                        Key::Character(text) if self.command.len() < 4096 => {
                            self.command.push_str(text);
                        }
                        _ => {}
                    }
                    self.dirty = true;
                    return;
                }
                let code = keymap::map_winit_key(
                    &event.logical_key,
                    event.physical_key,
                    event.text.as_deref(),
                );
                let ch = if platform::text_allowed(self.modifiers) {
                    keymap::typed_char(&event.logical_key, event.text.as_deref())
                } else {
                    None
                };
                if self.dialog.is_some() {
                    self.key_press(KeyPress { code, ch });
                } else if !event.repeat
                    && let Some(code) = code
                {
                    self.held.insert(event.physical_key, code);
                    self.key_press(KeyPress::code(code));
                }
            }
            _ => {}
        }
    }
}
fn label(canvas: &mut Grob, text: &str, x: i64, y: i64, color: u32) {
    draw_text(
        canvas,
        text,
        x,
        y,
        13.,
        color,
        Some((canvas.width() as i64 - x - 4).max(0)),
        None,
    );
}
fn button(canvas: &mut Grob, x: i64, y: i64, width: i64, height: i64, text: &str) {
    canvas.rect(
        x + 1,
        y + 1,
        x + width - 1,
        y + height - 2,
        0x4b5c73,
        0x2d3b50,
    );
    draw_text(
        canvas,
        text,
        x + 6,
        y + height / 2 - 5,
        12.,
        FG,
        Some(width - 10),
        None,
    );
}
fn digit_code(index: usize) -> Option<i64> {
    [47, 42, 43, 44, 37, 38, 39, 32, 33, 34].get(index).copied()
}
fn native_dialog(view: &DialogView) -> Dialog {
    match view.kind.as_str() {
        "message" => Dialog::Message {
            text: view.message.clone(),
        },
        "choose" => Dialog::Choose {
            title: view.title.clone(),
            items: view.items.clone(),
            current: view.focus + 1,
        },
        "matrix" => Dialog::Matrix(MatrixDialog {
            title: view.title.clone(),
            row_headers: view.row_headers.clone(),
            column_headers: view.column_headers.clone(),
            rows: view.rows,
            cols: view.cols,
            cells: view.fields.iter().map(|f| f.current.clone()).collect(),
            read_only: view.read_only,
            error: view.error.clone(),
            focus: view.focus,
        }),
        _ => Dialog::Input {
            title: view.title.clone(),
            fields: view
                .fields
                .iter()
                .map(|f| InputField {
                    label: f.label.clone(),
                    help: f.help.clone(),
                    current: f.current.clone(),
                    string_only: f.string_only,
                    reset_text: f.reset.clone(),
                    position: f.position.map(|(start_percent, width_percent, line)| {
                        InputPosition {
                            start_percent,
                            width_percent,
                            line,
                        }
                    }),
                    page_title: f.page_title.clone(),
                })
                .collect(),
            error: view.error.clone(),
            focus: view.focus,
        },
    }
}
