use hpdev_core::{
    Control, DialogResponse, DialogView, Event, Report, Request,
    protocol::{read_packet, write_packet},
};
use serde_json::{Value, json};
use std::{
    io::{self, BufRead, IsTerminal, Write},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

pub struct ViewOptions {
    pub desktop: bool,
    #[cfg_attr(not(feature = "desktop"), allow(dead_code))]
    pub scale: u32,
    #[cfg_attr(not(feature = "desktop"), allow(dead_code))]
    pub show_keys: bool,
    pub tui: bool,
    pub debug: bool,
    pub web: bool,
    #[cfg_attr(
        not(feature = "web"),
        expect(
            dead_code,
            reason = "web options remain parseable in builds omitting the optional viewer"
        )
    )]
    pub no_open: bool,
    #[cfg_attr(
        not(feature = "web"),
        expect(
            dead_code,
            reason = "web options remain parseable in builds omitting the optional viewer"
        )
    )]
    pub port: u16,
    pub exit_on_finish: bool,
    pub cell_aspect: Option<f64>,
    pub color: String,
}
pub struct ViewError {
    pub status: &'static str,
    pub code: u8,
    pub message: String,
}
impl ViewError {
    fn io(message: String) -> Self {
        Self {
            status: "io_error",
            code: 5,
            message,
        }
    }
    fn unsupported(message: impl Into<String>) -> Self {
        Self {
            status: "unsupported",
            code: 8,
            message: message.into(),
        }
    }
}
impl Default for ViewOptions {
    fn default() -> Self {
        Self {
            desktop: false,
            scale: 2,
            show_keys: false,
            tui: false,
            debug: false,
            web: false,
            no_open: false,
            port: 0,
            exit_on_finish: false,
            cell_aspect: None,
            color: "auto".into(),
        }
    }
}
pub struct Views {
    #[cfg(feature = "desktop")]
    desktop: Option<crate::desktop::Bridge>,
    terminal: Option<crate::tui::TerminalUi>,
    #[cfg(feature = "web")]
    browser: Option<crate::web::BrowserViewer>,
    options: ViewOptions,
    last_frame: Option<Instant>,
}
impl Views {
    pub fn empty() -> Self {
        Self {
            #[cfg(feature = "desktop")]
            desktop: None,
            terminal: None,
            #[cfg(feature = "web")]
            browser: None,
            options: ViewOptions::default(),
            last_frame: None,
        }
    }
    pub fn new(options: ViewOptions) -> Result<Self, ViewError> {
        if options
            .cell_aspect
            .is_some_and(|aspect| !aspect.is_finite() || aspect <= 0.)
        {
            return Err(ViewError {
                status: "invalid_request",
                code: 2,
                message: "cell aspect must be positive and finite".into(),
            });
        }
        let mut views = Self::empty();
        if options.tui {
            if !io::stdin().is_terminal()
                || !io::stderr().is_terminal()
                || std::env::var("TERM").as_deref() == Ok("dumb")
            {
                return Err(ViewError::unsupported(
                    "TUI requires terminal stdin/stderr with cursor control; use --png, --web, or debug --no-tui",
                ));
            }
            views.terminal = Some(
                crate::tui::TerminalUi::new(options.debug, options.cell_aspect, &options.color)
                    .map_err(ViewError::io)?,
            );
        }
        if options.web {
            #[cfg(feature = "web")]
            {
                views.browser = Some(
                    crate::web::BrowserViewer::new(options.port, options.no_open)
                        .map_err(ViewError::io)?,
                );
            }
            #[cfg(not(feature = "web"))]
            return Err(ViewError::unsupported(
                "this build excludes the web viewer; rebuild with the web feature",
            ));
        }
        views.options = options;
        Ok(views)
    }
    #[cfg(feature = "desktop")]
    pub fn desktop(bridge: crate::desktop::Bridge) -> Self {
        let mut views = Self::empty();
        views.options.desktop = true;
        views.desktop = Some(bridge);
        views
    }
    pub fn graphical(&self) -> bool {
        self.options.tui || self.options.web || self.options.desktop
    }
    fn ready(&self) -> bool {
        #[cfg(feature = "web")]
        if let Some(browser) = &self.browser {
            return browser.ready();
        }
        true
    }
    pub fn apply(&mut self, event: &Event) -> Result<(), String> {
        #[cfg(feature = "desktop")]
        if let Some(desktop) = &self.desktop {
            desktop.apply(event)?;
        }
        if matches!(event, Event::Frame { .. }) && self.graphical() && !self.options.desktop {
            if let Some(last) = self.last_frame {
                let wait = Duration::from_millis(33).saturating_sub(last.elapsed());
                if !wait.is_zero() {
                    std::thread::sleep(wait);
                }
            }
            self.last_frame = Some(Instant::now());
        }
        if let Some(ui) = &mut self.terminal {
            ui.apply(event)?;
        }
        #[cfg(feature = "web")]
        if let Some(browser) = &self.browser {
            browser.apply(event);
        }
        Ok(())
    }
    pub fn poll(&mut self) -> Result<Vec<Control>, String> {
        let mut commands = vec![];
        #[cfg(feature = "desktop")]
        if let Some(desktop) = &self.desktop {
            commands.extend(desktop.poll());
        }
        if let Some(ui) = &mut self.terminal {
            commands.extend(ui.poll()?);
        }
        #[cfg(feature = "web")]
        if let Some(browser) = &self.browser {
            commands.extend(browser.poll());
        }
        Ok(commands)
    }
    pub fn reset(&mut self) {
        self.last_frame = None;
        if let Some(ui) = &mut self.terminal {
            ui.reset();
        }
        #[cfg(feature = "web")]
        if let Some(browser) = &self.browser {
            browser.reset();
        }
    }
    pub fn metadata(&self) -> Value {
        let data = json!({
            "terminal": self.options.tui,
            "browser": self.options.web,
            "desktop": self.options.desktop,
        });
        #[cfg(feature = "web")]
        {
            let mut data = data;
            if let Some(browser) = &self.browser {
                data["url"] = json!(browser.url());
            }
            data
        }
        #[cfg(not(feature = "web"))]
        {
            data
        }
    }
    pub fn hold(&mut self, report: &Report, cancel: &Arc<AtomicBool>) {
        if !self.graphical() || self.options.exit_on_finish || report.exit_code == 130 {
            return;
        }
        let _ = self.apply(&Event::Final {
            report: Box::new(report.clone()),
        });
        while !cancel.load(Ordering::Acquire) {
            match self.poll() {
                Ok(commands) => {
                    if commands.iter().any(|c| matches!(c, Control::Cancel)) {
                        break;
                    }
                }
                Err(_) => break,
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

struct Guard(Child);
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
pub struct Outcome {
    pub report: Report,
    pub replaced: bool,
}

enum Line {
    Text(String),
    Eof,
}
struct Prompt {
    dialog: DialogView,
    values: Vec<String>,
    index: usize,
}
impl Prompt {
    fn new(dialog: DialogView) -> Self {
        Self {
            values: dialog.fields.iter().map(|f| f.current.clone()).collect(),
            index: 0,
            dialog,
        }
    }
    fn show(&self) {
        eprintln!("{} {}", self.dialog.title, self.dialog.message);
        if let Some(error) = &self.dialog.error {
            eprintln!("{error}");
        }
        match self.dialog.kind.as_str() {
            "message" => eprint!("Enter = OK, <ESC> = cancel: "),
            "choose" => {
                for (i, item) in self.dialog.items.iter().enumerate() {
                    eprintln!("{}. {item}", i + 1);
                }
                eprint!("Choice (0 cancels): ");
            }
            _ => {
                if self.dialog.read_only {
                    for (field, value) in self.dialog.fields.iter().zip(&self.values) {
                        eprintln!("{}: {value}", field.label);
                    }
                    eprint!("Enter to close: ");
                } else if let Some(field) = self.dialog.fields.get(self.index) {
                    eprint!(
                        "{} [{}] ({}): ",
                        field.label, self.values[self.index], field.help
                    );
                }
            }
        }
        let _ = io::stderr().flush();
    }
    fn line(&mut self, line: &str) -> Option<DialogResponse> {
        if line == "<ESC>" {
            return Some(DialogResponse::Cancel);
        }
        match self.dialog.kind.as_str() {
            "message" => Some(DialogResponse::Ok),
            "choose" => match line.parse::<usize>() {
                Ok(0) => Some(DialogResponse::Cancel),
                Ok(index) if index <= self.dialog.items.len() => {
                    Some(DialogResponse::Choice { index })
                }
                _ => {
                    self.show();
                    None
                }
            },
            _ if self.dialog.read_only => Some(DialogResponse::Ok),
            _ => {
                if let Some(value) = self.values.get_mut(self.index)
                    && !line.is_empty()
                {
                    *value = line.into();
                }
                self.index += 1;
                if self.index >= self.values.len() {
                    Some(DialogResponse::Entries {
                        values: self.values.clone(),
                    })
                } else {
                    self.show();
                    None
                }
            }
        }
    }
}

fn send(input: &mut mpsc::SyncSender<Control>, command: &Control) -> io::Result<()> {
    match input.try_send(command.clone()) {
        Ok(()) | Err(mpsc::TrySendError::Disconnected(_)) => Ok(()),
        Err(mpsc::TrySendError::Full(_)) => Err(io::Error::other("worker input queue is full")),
    }
}

pub fn execute(
    request: Request,
    views: &mut Views,
    json_output: bool,
    quiet: bool,
    cancel: &Arc<AtomicBool>,
    mut changed: impl FnMut() -> bool,
) -> Outcome {
    let started = Instant::now();
    let mut fallback = Report::new(&request.operation, Some(request.file.clone()));
    let result = (|| -> Result<Outcome, String> {
        // Wait for the first browser handshake so short programs still have a
        // live viewer for their initial frames. Existing watch viewers are ready.
        while !views.ready() {
            if cancel.load(Ordering::Acquire) {
                let mut report = fallback.clone();
                report.fail("cancelled", 130, "cancelled while waiting for browser");
                return Ok(Outcome {
                    report,
                    replaced: false,
                });
            }
            if request
                .max_time
                .is_some_and(|s| started.elapsed().as_secs_f64() >= s)
            {
                let mut report = fallback.clone();
                report.fail(
                    "timed_out",
                    4,
                    "deadline exceeded while waiting for browser",
                );
                return Ok(Outcome {
                    report,
                    replaced: false,
                });
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut command = Command::new(exe);
        command
            .arg("__worker")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // Keep the CLI's inherited terminal handles; never create a second
            // console for the private VM process.
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = Guard(command.spawn().map_err(|e| e.to_string())?);
        let mut input = child.0.stdin.take().ok_or("worker stdin unavailable")?;
        let mut output = child.0.stdout.take().ok_or("worker stdout unavailable")?;
        write_packet(&mut input, &request).map_err(|e| e.to_string())?;
        // A busy worker must never block supervisor deadlines on its pipe.
        let (control_tx, control_rx) = mpsc::sync_channel(64);
        std::thread::spawn(move || {
            for command in control_rx {
                if write_packet(&mut input, &command).is_err() {
                    break;
                }
            }
        });
        let mut input = control_tx;
        let (tx, rx) = mpsc::sync_channel(8);
        std::thread::spawn(move || {
            loop {
                let event = read_packet::<Event>(&mut output);
                let done = event.is_err() || matches!(&event, Ok(Event::Final { .. }));
                if tx.send(event).is_err() || done {
                    break;
                }
            }
        });
        let lines = if !views.graphical() && (request.debug || request.interactive) {
            let (tx, rx) = mpsc::sync_channel(8);
            std::thread::spawn(move || {
                for line in io::stdin().lock().lines() {
                    match line {
                        Ok(line) => {
                            if tx.send(Line::Text(line)).is_err() {
                                return;
                            }
                        }
                        Err(_) => break,
                    }
                }
                let _ = tx.send(Line::Eof);
            });
            if !quiet {
                eprintln!(
                    "{}",
                    if request.debug {
                        "hpdev debugger: help for commands; EOF quits"
                    } else {
                        "Interactive input: key CODE, touch X Y, Enter, or quit. Dialogs prompt for fields."
                    }
                );
            }
            Some(rx)
        } else {
            None
        };
        let mut prompt: Option<Prompt> = None;
        let mut stopping: Option<(Instant, &str, u8, &str)> = None;
        let mut replaced = false;
        let mut stopped = false;
        let mut report = None;
        loop {
            if stopping.is_none() {
                if cancel.load(Ordering::Acquire) {
                    stopping = Some((Instant::now(), "cancelled", 130, "execution cancelled"));
                } else if changed() {
                    replaced = true;
                    stopping = Some((
                        Instant::now(),
                        "cancelled",
                        130,
                        "replaced by a newer source revision",
                    ));
                } else if request
                    .max_time
                    .is_some_and(|s| started.elapsed().as_secs_f64() >= s)
                {
                    stopping = Some((
                        Instant::now(),
                        "timed_out",
                        4,
                        "total execution deadline exceeded",
                    ));
                }
                if stopping.is_some() {
                    let _ = send(&mut input, &Control::Cancel);
                }
            }
            if let Some((since, status, code, message)) = stopping
                && since.elapsed() > Duration::from_millis(500)
            {
                fallback.fail(status, code, message);
                report = Some(fallback.clone());
                break;
            }
            for command in views.poll()? {
                if stopping.is_some() {
                    continue;
                }
                if matches!(command, Control::Cancel) {
                    stopping = Some((Instant::now(), "cancelled", 130, "execution cancelled"));
                    let _ = send(&mut input, &command);
                    continue;
                }
                send(&mut input, &command).map_err(|e| e.to_string())?;
            }
            if let Some(lines) = &lines {
                // Consume one line only when its recipient is ready. This keeps a
                // piped debugger script from pre-empting a later dialog.
                if (stopped || prompt.is_some() || request.interactive)
                    && let Ok(line) = lines.try_recv()
                {
                    match line {
                        Line::Eof => {
                            let _ = send(&mut input, &Control::Cancel);
                            if stopping.is_none() {
                                stopping = Some((Instant::now(), "cancelled", 130, "end of input"));
                            }
                        }
                        Line::Text(line) => {
                            if let Some(active) = &mut prompt {
                                if let Some(response) = active.line(&line) {
                                    send(&mut input, &Control::Dialog { response })
                                        .map_err(|e| e.to_string())?;
                                    prompt = None;
                                }
                            } else if request.debug && stopped {
                                let verb = line.split_whitespace().next().unwrap_or("");
                                send(
                                    &mut input,
                                    &Control::Debug {
                                        command: line.clone(),
                                    },
                                )
                                .map_err(|e| e.to_string())?;
                                if matches!(
                                    verb,
                                    "s" | "step"
                                        | "n"
                                        | "next"
                                        | "out"
                                        | "fin"
                                        | "finish"
                                        | "c"
                                        | "continue"
                                        | "cont"
                                        | "q"
                                        | "quit"
                                        | "terminate"
                                ) {
                                    stopped = false;
                                }
                            } else if line == "quit" {
                                send(&mut input, &Control::Cancel).map_err(|e| e.to_string())?;
                            } else {
                                let parts = line.split_whitespace().collect::<Vec<_>>();
                                let control = match parts.as_slice() {
                                    ["touch", x, y] => x
                                        .parse()
                                        .ok()
                                        .zip(y.parse().ok())
                                        .map(|(x, y)| Control::Touch { x, y }),
                                    ["key", code] | [code] => {
                                        code.parse().ok().map(|code| Control::Key {
                                            code,
                                            down: true,
                                            pulse: true,
                                        })
                                    }
                                    [] => Some(Control::Key {
                                        code: 30,
                                        down: true,
                                        pulse: true,
                                    }),
                                    _ => None,
                                };
                                if let Some(control) = control {
                                    send(&mut input, &control).map_err(|e| e.to_string())?;
                                } else {
                                    eprintln!("Use key CODE, touch X Y, or quit");
                                }
                            }
                        }
                    }
                }
            }
            match rx.recv_timeout(Duration::from_millis(10)) {
                Ok(Ok(event)) => {
                    views.apply(&event)?;
                    match event {
                        Event::Output { text } => {
                            if !json_output && (!views.graphical() || views.options.desktop) {
                                println!("{text}");
                            }
                        }
                        Event::Dialog { dialog } => {
                            if !views.graphical() {
                                let p = Prompt::new(dialog);
                                p.show();
                                prompt = Some(p);
                            }
                        }
                        Event::Stop { snapshot } => {
                            stopped = true;
                            if !views.graphical() && !json_output {
                                eprintln!(
                                    "[STOP] {} at {}:{}:{} ({})",
                                    snapshot.function,
                                    snapshot.location.file,
                                    snapshot.location.line,
                                    snapshot.location.column,
                                    snapshot.reason
                                );
                                for (line, text) in snapshot.source_lines {
                                    eprintln!(
                                        "{} {line:4} | {text}",
                                        if line == snapshot.location.line {
                                            "=>"
                                        } else {
                                            "  "
                                        }
                                    );
                                }
                                eprint!("(hpdevdb) ");
                                let _ = io::stderr().flush();
                            }
                        }
                        Event::DebugReply { text } => {
                            if !json_output {
                                eprintln!("{text}");
                            }
                        }
                        Event::Trace { event } => {
                            if !json_output && !quiet {
                                eprintln!("{event}");
                            }
                        }
                        Event::Final {
                            report: final_report,
                        } => {
                            report = Some(*final_report);
                            break;
                        }
                        Event::Frame { .. } | Event::Clear => {}
                    }
                }
                Ok(Err(e)) => {
                    if stopping.is_none() {
                        return Err(format!("worker ended without a result: {e}"));
                    }
                    break;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
        let mut report = report.unwrap_or_else(|| {
            let mut r = fallback.clone();
            r.fail(
                "infrastructure_error",
                70,
                "worker ended without a final report",
            );
            r
        });
        if let Some((_, status, code, message)) = stopping {
            report.fail(status, code, message);
        }
        report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
        Ok(Outcome { report, replaced })
    })();
    match result {
        Ok(result) => result,
        Err(e) => {
            fallback.fail(
                if cancel.load(Ordering::Acquire) {
                    "cancelled"
                } else {
                    "infrastructure_error"
                },
                if cancel.load(Ordering::Acquire) {
                    130
                } else {
                    70
                },
                e,
            );
            fallback.duration_ms = started.elapsed().as_secs_f64() * 1000.;
            Outcome {
                report: fallback,
                replaced: false,
            }
        }
    }
}
