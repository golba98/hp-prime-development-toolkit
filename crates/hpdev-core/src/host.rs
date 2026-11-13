use crate::model::*;
use chrono::{Datelike, Timelike};
use ppl_graphics::Grob;
use ppl_vm::{Dialog, DialogResult, ErrorKind, HeadlessHost, Host, RuntimeError, host::Touch};
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Receiver,
    },
    time::{Duration, Instant},
};

pub type Emit = Rc<dyn Fn(Event)>;

#[derive(Default)]
pub struct Capture {
    pub output: Vec<String>,
    pub frames: u64,
    pub dialogs: Vec<String>,
    pub trace: Vec<serde_json::Value>,
    pub trace_bytes: usize,
    pub trace_exhausted: bool,
    pub debug_history: Vec<serde_json::Value>,
    pub dialog_bytes: usize,
}

impl Capture {
    pub(crate) fn record_trace(&mut self, event: serde_json::Value, max_events: usize) -> bool {
        let bytes = event.to_string().len();
        if self.trace.len() >= max_events || self.trace_bytes + bytes > 4 * 1024 * 1024 {
            self.trace_exhausted = true;
            return false;
        }
        self.trace_bytes += bytes;
        self.trace.push(event);
        true
    }

    fn record_dialog(&mut self, log: String, max_output: usize) -> Result<(), RuntimeError> {
        self.dialog_bytes = self.dialog_bytes.saturating_add(log.len());
        if self.dialog_bytes > max_output {
            return Err(RuntimeError::kind(ErrorKind::ResourceLimit(
                ppl_vm::ResourceKind::Output,
            )));
        }
        self.dialogs.push(log);
        Ok(())
    }
}

pub struct InputState {
    rx: Receiver<Control>,
    pub cancel: Arc<AtomicBool>,
    pub commands: VecDeque<String>,
    pub pause_requested: bool,
    pointer: Option<Touch>,
    keys: VecDeque<i64>,
    down: BTreeMap<i64, Option<Instant>>,
    touch: Option<Touch>,
    dialog: Option<DialogResponse>,
    overflow: bool,
}

impl InputState {
    pub fn new(rx: Receiver<Control>, cancel: Arc<AtomicBool>) -> Self {
        Self {
            rx,
            cancel,
            commands: VecDeque::new(),
            pause_requested: false,
            pointer: None,
            keys: VecDeque::new(),
            down: BTreeMap::new(),
            touch: None,
            dialog: None,
            overflow: false,
        }
    }
    fn dispatch(&mut self, command: Control) {
        if self.keys.len() >= 4096 || self.commands.len() >= 4096 {
            self.overflow = true;
            return;
        }
        match command {
            Control::Pause => self.pause_requested = true,
            Control::Pointer { x, y, down } => {
                self.pointer = down.then_some(Touch {
                    x: x.clamp(0, 319),
                    y: y.clamp(0, 239),
                });
                if down {
                    self.touch = self.pointer;
                }
            }
            Control::Cancel => {
                self.cancel.store(true, Ordering::Release);
            }
            Control::Debug { command } => self.commands.push_back(command),
            Control::Key { code, down, pulse } if (0..=50).contains(&code) => {
                if down {
                    self.keys.push_back(code);
                    self.down.insert(
                        code,
                        pulse.then(|| Instant::now() + Duration::from_millis(180)),
                    );
                } else {
                    self.down.remove(&code);
                }
            }
            Control::Key { .. } => {}
            Control::Touch { x, y } => {
                self.touch = Some(Touch {
                    x: x.clamp(0, 319),
                    y: y.clamp(0, 239),
                })
            }
            Control::Dialog { response } => self.dialog = Some(response),
        }
    }
    pub fn pump(&mut self) -> Result<(), RuntimeError> {
        while let Ok(command) = self.rx.try_recv() {
            self.dispatch(command);
        }
        if self.overflow {
            return Err(RuntimeError::kind(ErrorKind::ResourceLimit(
                ppl_vm::ResourceKind::Memory,
            )));
        }
        self.down
            .retain(|_, until| until.is_none_or(|t| t > Instant::now()));
        if self.cancel.load(Ordering::Acquire) {
            return Err(RuntimeError::kind(ErrorKind::Interrupted));
        }
        Ok(())
    }
    pub fn wait(&mut self) -> Result<(), RuntimeError> {
        self.pump()?;
        match self.rx.recv_timeout(Duration::from_millis(25)) {
            Ok(command) => self.dispatch(command),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(RuntimeError::kind(ErrorKind::Interrupted));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
        self.pump()
    }
    pub fn command(&mut self) -> Result<String, RuntimeError> {
        loop {
            self.pump()?;
            if let Some(command) = self.commands.pop_front() {
                return Ok(command);
            }
            self.wait()?;
        }
    }
}

pub struct ToolkitHost {
    pub scripted: HeadlessHost,
    pub interactive: bool,
    pub input: Rc<RefCell<InputState>>,
    pub capture: Rc<RefCell<Capture>>,
    pub emit: Emit,
    pub started: Instant,
    pub emit_frames: bool,
    pub trace: bool,
    pub max_events: usize,
    pub max_output: usize,
}

impl Host for ToolkitHost {
    fn print(&mut self, text: &str) {
        self.capture.borrow_mut().output.push(text.into());
        (self.emit)(Event::Output { text: text.into() });
    }
    fn clear_terminal(&mut self) {
        (self.emit)(Event::Clear);
    }
    fn present(&mut self, screen: &Grob, terminal: Option<&[String]>) {
        let mut capture = self.capture.borrow_mut();
        capture.frames += 1;
        let sequence = capture.frames;
        if self.trace {
            let event = serde_json::json!({
                "kind": "presentation",
                "sequence": sequence,
                "mode": if terminal.is_some() { "terminal" } else { "graphics" },
            });
            if !capture.record_trace(event, self.max_events) {
                self.input.borrow().cancel.store(true, Ordering::Release);
            }
        }
        drop(capture);
        if !self.emit_frames {
            return;
        }
        let image = terminal.map(ppl_graphics::text::render_terminal);
        (self.emit)(Event::Frame {
            sequence,
            pixels: image.as_ref().unwrap_or(screen).pixels().to_vec(),
            mode: if terminal.is_some() {
                "terminal"
            } else {
                "graphics"
            }
            .into(),
        });
    }
    fn poll_key(&mut self) -> Result<Option<i64>, RuntimeError> {
        if !self.interactive || !self.scripted.keys.is_empty() {
            return self.scripted.poll_key();
        }
        let mut input = self.input.borrow_mut();
        input.pump()?;
        Ok(input.keys.pop_front())
    }
    fn is_key_down(&mut self, key: i64) -> Result<bool, RuntimeError> {
        if !self.interactive || !self.scripted.keys.is_empty() {
            return self.scripted.is_key_down(key);
        }
        let mut input = self.input.borrow_mut();
        input.pump()?;
        Ok(input.down.contains_key(&key))
    }
    fn wait_key(&mut self, timeout_s: Option<f64>) -> Result<Option<i64>, RuntimeError> {
        if !self.interactive || !self.scripted.keys.is_empty() {
            return self.scripted.wait_key(timeout_s);
        }
        let deadline = timeout_s.map(|s| Instant::now() + Duration::from_secs_f64(s.max(0.)));
        loop {
            let mut input = self.input.borrow_mut();
            input.pump()?;
            if let Some(key) = input.keys.pop_front() {
                return Ok(Some(key));
            }
            if input.touch.take().is_some() {
                return Ok(None);
            }
            if deadline.is_some_and(|d| Instant::now() >= d) {
                return Ok(None);
            }
            input.wait()?;
        }
    }
    fn sleep(&mut self, seconds: f64) -> Result<(), RuntimeError> {
        if !self.interactive {
            return self.scripted.sleep(seconds);
        }
        let duration = Duration::try_from_secs_f64(seconds.max(0.))
            .map_err(|_| RuntimeError::kind(ErrorKind::BadArgValue))?;
        let deadline = Instant::now()
            .checked_add(duration)
            .ok_or_else(|| RuntimeError::kind(ErrorKind::BadArgValue))?;
        while Instant::now() < deadline {
            self.input.borrow_mut().pump()?;
            std::thread::sleep(
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(10)),
            );
        }
        Ok(())
    }
    fn touch(&mut self) -> Result<Option<Touch>, RuntimeError> {
        if !self.interactive || !self.scripted.keys.is_empty() {
            return self.scripted.touch();
        }
        let mut input = self.input.borrow_mut();
        input.pump()?;
        Ok(input.touch.take().or(input.pointer))
    }
    fn dialog(&mut self, dialog: &Dialog, screen: &Grob) -> Result<DialogResult, RuntimeError> {
        let has_script = match dialog {
            Dialog::Choose { .. } => !self.scripted.choices.is_empty(),
            Dialog::Input { .. } | Dialog::Matrix(_) => !self.scripted.inputs.is_empty(),
            Dialog::Message { .. } => false,
        };
        if !self.interactive || has_script {
            let result = self.scripted.dialog(dialog, screen);
            let dialogs = std::mem::take(&mut self.scripted.dialogs);
            let mut capture = self.capture.borrow_mut();
            for log in dialogs {
                capture.record_dialog(log, self.max_output)?;
            }
            return result;
        }
        let view = dialog_view(dialog);
        self.capture
            .borrow_mut()
            .record_dialog(format!("{}: {}", view.kind, view.title), self.max_output)?;
        (self.emit)(Event::Dialog { dialog: view });
        loop {
            let mut input = self.input.borrow_mut();
            input.pump()?;
            if let Some(response) = input.dialog.take() {
                return Ok(match response {
                    DialogResponse::Ok => DialogResult::Ok,
                    DialogResponse::Cancel => DialogResult::Cancelled,
                    DialogResponse::Choice { index } => DialogResult::Chosen(index),
                    DialogResponse::Entries { values } => DialogResult::Entered(values),
                });
            }
            input.wait()?;
        }
    }
    fn now_ms(&mut self) -> f64 {
        if self.interactive {
            self.started.elapsed().as_secs_f64() * 1000.
        } else {
            self.scripted.now_ms()
        }
    }
    fn local_time(&mut self) -> (i64, i64, i64, i64, i64, f64) {
        if !self.interactive {
            return self.scripted.local_time();
        }
        let now = chrono::Local::now();
        (
            now.year().into(),
            now.month().into(),
            now.day().into(),
            now.hour().into(),
            now.minute().into(),
            now.second() as f64,
        )
    }
    fn interrupt_flag(&self) -> Option<Arc<AtomicBool>> {
        Some(self.input.borrow().cancel.clone())
    }
}

fn dialog_view(dialog: &Dialog) -> DialogView {
    let mut view = DialogView {
        rows: 0,
        cols: 0,
        row_headers: vec![],
        column_headers: vec![],
        kind: String::new(),
        title: String::new(),
        message: String::new(),
        items: vec![],
        fields: vec![],
        focus: 0,
        error: None,
        read_only: false,
    };
    match dialog {
        Dialog::Message { text } => {
            view.kind = "message".into();
            view.message = text.clone();
        }
        Dialog::Choose {
            title,
            items,
            current,
        } => {
            view.kind = "choose".into();
            view.title = title.clone();
            view.items = items.clone();
            view.focus = current.saturating_sub(1);
        }
        Dialog::Input {
            title,
            fields,
            error,
            focus,
        } => {
            view.kind = "input".into();
            view.title = title.clone();
            view.error = error.clone();
            view.focus = *focus;
            view.fields = fields
                .iter()
                .map(|f| Field {
                    position: f
                        .position
                        .map(|p| (p.start_percent, p.width_percent, p.line)),
                    string_only: f.string_only,
                    label: f.label.clone(),
                    help: f.help.clone(),
                    current: f.current.clone(),
                    reset: f.reset_text.clone(),
                    page_title: f.page_title.clone(),
                })
                .collect();
        }
        Dialog::Matrix(matrix) => {
            view.kind = "matrix".into();
            view.rows = matrix.rows;
            view.cols = matrix.cols;
            view.row_headers = matrix.row_headers.clone();
            view.column_headers = matrix.column_headers.clone();
            view.title = matrix.title.clone();
            view.error = matrix.error.clone();
            view.focus = matrix.focus;
            view.read_only = matrix.read_only;
            view.fields = matrix
                .cells
                .iter()
                .enumerate()
                .map(|(i, s)| Field {
                    position: None,
                    string_only: false,
                    label: format!("[{},{}]", i / matrix.cols + 1, i % matrix.cols + 1),
                    help: String::new(),
                    current: s.clone(),
                    reset: None,
                    page_title: None,
                })
                .collect();
        }
    }
    view
}
