//! Terminal rendering is sampled directly from the native 320×240 framebuffer.
use crossterm::{
    event::{
        self, Event as TerminalEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind,
    },
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use hpdev_core::{Control, DebugSnapshot, DialogResponse, DialogView, Event};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use std::{collections::VecDeque, io, time::Duration};

pub struct TerminalUi {
    terminal: Terminal<CrosstermBackend<io::Stderr>>,
    debug: bool,
    aspect: f64,
    color: String,
    pixels: Vec<u32>,
    sequence: u64,
    log: VecDeque<String>,
    snapshot: Option<DebugSnapshot>,
    status: String,
    dialog: Option<Editor>,
    command: Option<String>,
    finished: bool,
    paused: bool,
    page: usize,
    screen_rect: Rect,
    scroll: u16,
}
struct Editor {
    dialog: DialogView,
    values: Vec<String>,
    focus: usize,
    replace: bool,
}
impl Editor {
    fn new(dialog: DialogView) -> Self {
        let values = dialog
            .fields
            .iter()
            .map(|f| f.current.clone())
            .collect::<Vec<_>>();
        let focus = dialog.focus.min(if dialog.kind == "choose" {
            dialog.items.len().saturating_sub(1)
        } else {
            values.len().saturating_sub(1)
        });
        Self {
            dialog,
            values,
            focus,
            replace: true,
        }
    }
    fn key(&mut self, key: KeyEvent) -> Option<DialogResponse> {
        if key.code == KeyCode::Esc {
            return Some(DialogResponse::Cancel);
        }
        let count = if self.dialog.kind == "choose" {
            self.dialog.items.len()
        } else {
            self.values.len()
        };
        match key.code {
            KeyCode::Up | KeyCode::BackTab => {
                self.focus = self.focus.saturating_sub(1);
                self.replace = true;
            }
            KeyCode::Down | KeyCode::Tab => {
                self.focus = (self.focus + 1).min(count.saturating_sub(1));
                self.replace = true;
            }
            KeyCode::Enter => {
                if self.dialog.kind == "message" || self.dialog.read_only {
                    return Some(DialogResponse::Ok);
                }
                if self.dialog.kind == "choose" {
                    return Some(DialogResponse::Choice {
                        index: self.focus + 1,
                    });
                }
                if self.focus + 1 < count {
                    self.focus += 1;
                    self.replace = true;
                } else {
                    return Some(DialogResponse::Entries {
                        values: self.values.clone(),
                    });
                }
            }
            KeyCode::F(2) => {
                if let Some(reset) = self
                    .dialog
                    .fields
                    .get(self.focus)
                    .and_then(|f| f.reset.clone())
                {
                    self.values[self.focus] = reset;
                    self.replace = true;
                }
            }
            KeyCode::Backspace if !self.dialog.read_only => {
                if let Some(value) = self.values.get_mut(self.focus) {
                    if self.replace {
                        value.clear();
                    } else {
                        value.pop();
                    }
                    self.replace = false;
                }
            }
            KeyCode::Char(c)
                if !self.dialog.read_only
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if let Some(value) = self.values.get_mut(self.focus) {
                    if self.replace {
                        value.clear();
                        self.replace = false;
                    }
                    value.push(c);
                }
            }
            _ => {}
        }
        None
    }
    fn lines(&self) -> Vec<Line<'static>> {
        let mut lines = vec![Line::from(self.dialog.message.clone())];
        if let Some(error) = &self.dialog.error {
            lines.push(Line::styled(error.clone(), Style::default().fg(Color::Red)));
        }
        if self.dialog.kind == "choose" {
            for (i, item) in self.dialog.items.iter().enumerate() {
                lines.push(Line::from(format!(
                    "{} {}. {item}",
                    if i == self.focus { "▶" } else { " " },
                    i + 1
                )));
            }
        } else {
            // Keep the focused field visible for large INPUT/EDITMAT requests.
            let start = self.focus.saturating_sub(3);
            for (i, field) in self.dialog.fields.iter().enumerate().skip(start).take(7) {
                lines.push(Line::from(format!(
                    "{} {}: {}",
                    if i == self.focus { "▶" } else { " " },
                    field.label,
                    self.values[i]
                )));
                if i == self.focus && !field.help.is_empty() {
                    lines.push(Line::from(field.help.clone()));
                }
            }
        }
        lines.push(Line::from(
            "Tab/↑/↓ field · Enter accept · F2 reset · Esc cancel",
        ));
        lines
    }
}

impl TerminalUi {
    pub fn new(debug: bool, cell_aspect: Option<f64>, color: &str) -> Result<Self, String> {
        let mut aspect = cell_aspect.unwrap_or(0.5);
        if cell_aspect.is_none()
            && let Ok(size) = terminal::window_size()
            && size.width > 0
            && size.height > 0
            && size.columns > 0
            && size.rows > 0
        {
            aspect =
                (size.width as f64 / size.columns as f64) / (size.height as f64 / size.rows as f64);
        }
        let color = if color != "auto" {
            color.to_owned()
        } else if std::env::var_os("NO_COLOR").is_some() {
            "mono".into()
        } else if std::env::var("COLORTERM")
            .is_ok_and(|s| s.contains("truecolor") || s.contains("24bit"))
        {
            "truecolor".into()
        } else if std::env::var("TERM").is_ok_and(|s| s.contains("256color")) {
            "256".into()
        } else {
            "mono".into()
        };
        crossterm::style::force_color_output(color != "mono");
        terminal::enable_raw_mode().map_err(|e| e.to_string())?;
        if let Err(e) = execute!(
            io::stderr(),
            EnterAlternateScreen,
            event::EnableMouseCapture
        ) {
            let _ = terminal::disable_raw_mode();
            return Err(e.to_string());
        }
        let terminal = match Terminal::new(CrosstermBackend::new(io::stderr())) {
            Ok(t) => t,
            Err(e) => {
                let _ = execute!(
                    io::stderr(),
                    LeaveAlternateScreen,
                    event::DisableMouseCapture
                );
                let _ = terminal::disable_raw_mode();
                return Err(e.to_string());
            }
        };
        let mut ui = Self {
            terminal,
            debug,
            aspect,
            color,
            pixels: vec![0xffffff; 320 * 240],
            sequence: 0,
            log: VecDeque::new(),
            snapshot: None,
            status: "starting".into(),
            dialog: None,
            command: None,
            finished: false,
            paused: false,
            page: 0,
            screen_rect: Rect::default(),
            scroll: 0,
        };
        ui.draw()?;
        Ok(ui)
    }
    pub fn reset(&mut self) {
        self.status = "restarting".into();
        self.dialog = None;
        self.snapshot = None;
        self.finished = false;
        self.paused = false;
        self.sequence = 0;
        self.log.clear();
    }
    fn log(&mut self, text: String) {
        for line in text.lines() {
            self.log.push_back(line.to_owned());
        }
        while self.log.len() > 200 {
            self.log.pop_front();
        }
    }
    pub fn apply(&mut self, event: &Event) -> Result<(), String> {
        match event {
            Event::Frame {
                sequence, pixels, ..
            } if pixels.len() == 320 * 240 => {
                self.pixels.clone_from(pixels);
                self.sequence = *sequence;
            }
            Event::Output { text } => self.log(text.clone()),
            Event::Clear => self.log.clear(),
            Event::Stop { snapshot } => {
                self.paused = true;
                self.snapshot = Some(*snapshot.clone());
                self.status = "paused".into();
            }
            Event::DebugReply { text } => self.log(text.clone()),
            Event::Dialog { dialog } => {
                self.dialog = Some(Editor::new(dialog.clone()));
                self.status = "waiting for input".into();
            }
            Event::Final { report } => {
                self.finished = true;
                self.paused = false;
                self.dialog = None;
                self.status = format!("{} · Enter/q closes", report.status);
                if let Some(error) = &report.error {
                    self.log(error.clone());
                }
                for diagnostic in &report.diagnostics {
                    let position = diagnostic
                        .location
                        .as_ref()
                        .map(|l| format!("{}:{}:{}", l.file, l.line, l.column))
                        .unwrap_or_default();
                    self.log(format!(
                        "{position} {}[{}]: {}",
                        diagnostic.severity, diagnostic.code, diagnostic.message
                    ));
                }
                for frame in &report.stack {
                    self.log(format!(
                        "{} at {}:{}",
                        frame.function, frame.location.file, frame.location.line
                    ));
                }
            }
            _ => {}
        }
        self.draw()
    }
    pub fn poll(&mut self) -> Result<Vec<Control>, String> {
        let mut controls = vec![];
        while event::poll(Duration::ZERO).map_err(|e| e.to_string())? {
            match event::read().map_err(|e| e.to_string())? {
                TerminalEvent::Key(key) if key.kind != KeyEventKind::Release => {
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && key.code == KeyCode::Char('c')
                    {
                        controls.push(Control::Cancel);
                        continue;
                    }
                    if self.finished
                        && matches!(key.code, KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q'))
                    {
                        controls.push(Control::Cancel);
                        continue;
                    }
                    if let Some(editor) = &mut self.dialog {
                        if let Some(response) = editor.key(key) {
                            controls.push(Control::Dialog { response });
                            self.dialog = None;
                            self.paused = false;
                            self.status = "running".into();
                        }
                    } else if let Some(command) = &mut self.command {
                        match key.code {
                            KeyCode::Enter => {
                                if matches!(
                                    command.split_whitespace().next(),
                                    Some(
                                        "s" | "step"
                                            | "n"
                                            | "next"
                                            | "out"
                                            | "fin"
                                            | "finish"
                                            | "c"
                                            | "continue"
                                            | "cont"
                                    )
                                ) {
                                    self.paused = false;
                                    self.status = "running".into();
                                }
                                controls.push(Control::Debug {
                                    command: command.clone(),
                                });
                                self.command = None;
                            }
                            KeyCode::Esc => self.command = None,
                            KeyCode::Backspace => {
                                command.pop();
                            }
                            KeyCode::Char(c) => command.push(c),
                            _ => {}
                        }
                    } else if key.code == KeyCode::Tab {
                        self.page = (self.page + 1) % 3;
                    } else if key.code == KeyCode::PageDown {
                        self.scroll = self.scroll.saturating_add(5);
                    } else if key.code == KeyCode::PageUp {
                        self.scroll = self.scroll.saturating_sub(5);
                    } else if self.debug && key.code == KeyCode::Char('q') {
                        controls.push(Control::Cancel);
                    } else if self.debug && self.paused {
                        let command = match key.code {
                            KeyCode::Char(':') => {
                                self.command = Some(String::new());
                                None
                            }
                            KeyCode::Char('s') | KeyCode::F(11) => Some("step".to_owned()),
                            KeyCode::Char('n') | KeyCode::F(10) => Some("next".to_owned()),
                            KeyCode::Char('o') => Some("out".to_owned()),
                            KeyCode::Char('c') | KeyCode::F(5) => Some("continue".to_owned()),
                            KeyCode::Char('q') => {
                                controls.push(Control::Cancel);
                                None
                            }
                            KeyCode::Char('u') => Some("up".into()),
                            KeyCode::Char('d') => Some("down".into()),
                            KeyCode::Char('b') => self
                                .snapshot
                                .as_ref()
                                .map(|s| format!("break {}", s.location.line)),
                            KeyCode::Char('x') => self
                                .snapshot
                                .as_ref()
                                .map(|s| format!("delete {}", s.location.line)),
                            _ => None,
                        };
                        if let Some(command) = command {
                            if matches!(command.as_str(), "step" | "next" | "out" | "continue") {
                                self.paused = false;
                                self.status = "running".into();
                            }
                            controls.push(Control::Debug { command });
                        }
                    } else if let Some(code) = calculator_key(key.code) {
                        controls.push(Control::Key {
                            code,
                            down: true,
                            pulse: true,
                        });
                    }
                }
                TerminalEvent::Mouse(mouse) if matches!(mouse.kind, MouseEventKind::Down(_)) => {
                    let r = self.screen_rect;
                    if r.width > 0
                        && r.height > 0
                        && mouse.column >= r.x
                        && mouse.column < r.right()
                        && mouse.row >= r.y
                        && mouse.row < r.bottom()
                    {
                        controls.push(Control::Touch {
                            x: ((mouse.column - r.x) as i64 * 320 / r.width as i64),
                            y: ((mouse.row - r.y) as i64 * 240 / r.height as i64),
                        });
                    }
                }
                TerminalEvent::Resize(..) => {}
                _ => {}
            }
        }
        self.draw()?;
        Ok(controls)
    }
    fn draw(&mut self) -> Result<(), String> {
        let debug = self.debug;
        let snapshot = &self.snapshot;
        let pixels = &self.pixels;
        let log = &self.log;
        let aspect = self.aspect;
        let color = &self.color;
        let sequence = self.sequence;
        let page = self.page;
        let scroll = self.scroll;
        let status = &self.status;
        let dialog = &self.dialog;
        let command = &self.command;
        let mut screen_rect = Rect::default();
        self.terminal.draw(|frame|{
            let area=frame.area();
            let rows=Layout::default().direction(Direction::Vertical).constraints([Constraint::Length(1),Constraint::Min(1),Constraint::Length(2)]).split(area);
            frame.render_widget(Paragraph::new(format!("hpdev · {status} · frame {sequence}")),rows[0]);
            let source=||{
                snapshot.as_ref().map(|s|s.source_lines.iter().map(|(n,t)|Line::from(format!("{} {n:4} | {t}",if *n==s.location.line{"▶"}else{" "}))).collect::<Vec<_>>()).unwrap_or_else(||vec![Line::from("Waiting for a VM statement…")])
            };
            let variables=||{
                let mut lines=vec![];
                if let Some(s)=snapshot{
                    lines.push(Line::from(format!("Frame #{} {}",s.selected_frame,s.function)));
                    lines.push(Line::from("Locals"));for(n,v)in&s.locals{lines.push(Line::from(format!("{n} = {}",crate::cli::display_value(v))));}
                    lines.push(Line::from("Globals"));for(n,v)in&s.globals{lines.push(Line::from(format!("{n} = {}",crate::cli::display_value(v))));}
                    lines.push(Line::from("Stack"));for(i,f)in s.stack.iter().enumerate(){lines.push(Line::from(format!("#{i} {} :{}",f.function,f.location.line)));}
                    lines.push(Line::from(format!("Breakpoints: {:?}",s.breakpoints)));
                }lines
            };
            let draw_screen=|frame:&mut ratatui::Frame<'_>,area:Rect|->Rect{
                let block=Block::default().title("Native display · 320×240").borders(Borders::ALL);let inner=block.inner(area);frame.render_widget(block,area);
                let (rect,lines)=raster(pixels,inner,aspect,color);frame.render_widget(Paragraph::new(lines),rect);rect
            };
            if !debug{screen_rect=draw_screen(frame,rows[1]);}
            else if area.width>=100 {
                let cols=Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage(55),Constraint::Percentage(45)]).split(rows[1]);
                let left=Layout::default().direction(Direction::Vertical).constraints([Constraint::Percentage(65),Constraint::Percentage(35)]).split(cols[0]);
                let right=Layout::default().direction(Direction::Vertical).constraints([Constraint::Percentage(45),Constraint::Percentage(55)]).split(cols[1]);
                frame.render_widget(Paragraph::new(source()).block(Block::default().title("Source").borders(Borders::ALL)),left[0]);
                let logs=log.iter().rev().take(left[1].height.saturating_sub(2)as usize).cloned().collect::<Vec<_>>().into_iter().rev().map(Line::from).collect::<Vec<_>>();
                frame.render_widget(Paragraph::new(logs).block(Block::default().title("Output / inspector").borders(Borders::ALL)),left[1]);
                frame.render_widget(Paragraph::new(variables()).scroll((scroll,0)).block(Block::default().title("Variables / stack · PgUp/PgDn").borders(Borders::ALL)),right[0]);
                screen_rect=draw_screen(frame,right[1]);
            }else{match page{
                0=>{let split=Layout::default().direction(Direction::Vertical).constraints([Constraint::Percentage(65),Constraint::Percentage(35)]).split(rows[1]);frame.render_widget(Paragraph::new(source()).block(Block::default().title("Source · Tab changes panel").borders(Borders::ALL)),split[0]);frame.render_widget(Paragraph::new(log.iter().rev().take(split[1].height.saturating_sub(2)as usize).cloned().collect::<Vec<_>>().into_iter().rev().map(Line::from).collect::<Vec<_>>()).block(Block::default().title("Output / inspector").borders(Borders::ALL)),split[1]);}
                1=>frame.render_widget(Paragraph::new(variables()).scroll((scroll,0)).block(Block::default().title("Variables / stack · Tab changes panel").borders(Borders::ALL)),rows[1]),
                _=>screen_rect=draw_screen(frame,rows[1]),
            }}
            let footer=if let Some(command)=command{format!(":{command}")}else if debug{"s step · n next · o out · c continue · b break · x delete\nu/d frame · : command · Tab panels · Ctrl-C stop".into()}else{format!("Arrows / Enter / Esc / digits → calculator · Ctrl-C stop\n{}",log.back().map(String::as_str).unwrap_or(""))};
            frame.render_widget(Paragraph::new(footer),rows[2]);
            if let Some(editor)=dialog {
                let width=area.width.saturating_sub(4).min(90);let height=area.height.saturating_sub(2).min(16);
                let rect=Rect::new(area.x+(area.width-width)/2,area.y+(area.height-height)/2,width,height);
                frame.render_widget(Clear,rect);frame.render_widget(Paragraph::new(editor.lines()).wrap(Wrap{trim:false}).block(Block::default().title(format!("{} · {}",editor.dialog.kind,editor.dialog.title)).borders(Borders::ALL)),rect);
            }
        }).map_err(|e|e.to_string())?;
        self.screen_rect = screen_rect;
        Ok(())
    }
}
impl Drop for TerminalUi {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(
            io::stderr(),
            event::DisableMouseCapture,
            LeaveAlternateScreen
        );
        let _ = self.terminal.show_cursor();
    }
}

pub fn calculator_key(key: KeyCode) -> Option<i64> {
    Some(match key {
        KeyCode::Up => 2,
        KeyCode::Esc => 4,
        KeyCode::Left => 7,
        KeyCode::Right => 8,
        KeyCode::Down => 12,
        KeyCode::Backspace | KeyCode::Delete => 19,
        KeyCode::Enter => 30,
        KeyCode::Char('7') => 32,
        KeyCode::Char('8') => 33,
        KeyCode::Char('9') => 34,
        KeyCode::Char('/') => 35,
        KeyCode::Char('4') => 37,
        KeyCode::Char('5') => 38,
        KeyCode::Char('6') => 39,
        KeyCode::Char('*') => 40,
        KeyCode::Char('1') => 42,
        KeyCode::Char('2') => 43,
        KeyCode::Char('3') => 44,
        KeyCode::Char('-') => 45,
        KeyCode::Char('0') => 47,
        KeyCode::Char('.') => 48,
        KeyCode::Char('+') => 50,
        _ => return None,
    })
}

fn raster(pixels: &[u32], area: Rect, aspect: f64, color: &str) -> (Rect, Vec<Line<'static>>) {
    if area.width == 0 || area.height == 0 || pixels.len() != 320 * 240 {
        return (area, vec![]);
    }
    let height = (area.height as f64)
        .min(area.width as f64 * aspect * 3. / 4.)
        .floor()
        .max(1.) as u16;
    let width = ((height as f64 * 4. / 3. / aspect).floor() as u16)
        .min(area.width)
        .max(1);
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let mut lines = vec![];
    for y in 0..height {
        let mut spans = vec![];
        for x in 0..width {
            let sx = x as usize * 320 / width as usize;
            let sy = y as usize * 240 / height as usize;
            let top = pixels[sy * 320 + sx];
            let bottom = pixels[(sy + 120 / height as usize).min(239) * 320 + sx];
            if color == "mono" {
                let dark = |p: u32| {
                    let (r, g, b) = ppl_graphics::channels(p);
                    (r as u32 * 299 + g as u32 * 587 + b as u32 * 114) < 128000
                };
                spans.push(Span::raw(match (dark(top), dark(bottom)) {
                    (true, true) => "█",
                    (true, false) => "▀",
                    (false, true) => "▄",
                    _ => " ",
                }));
            } else {
                let c = |p: u32| {
                    let (r, g, b) = ppl_graphics::channels(p);
                    if color == "256" {
                        Color::Indexed(
                            16 + 36 * (r as u16 * 5 / 255) as u8
                                + 6 * (g as u16 * 5 / 255) as u8
                                + (b as u16 * 5 / 255) as u8,
                        )
                    } else {
                        Color::Rgb(r, g, b)
                    }
                };
                spans.push(Span::styled("▀", Style::default().fg(c(top)).bg(c(bottom))));
            }
        }
        lines.push(Line::from(spans));
    }
    (rect, lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn raster_preserves_aspect_and_real_pixels() {
        let image = vec![0; 320 * 240];
        let (rect, lines) = raster(&image, Rect::new(0, 0, 80, 30), 0.5, "mono");
        assert_eq!((rect.width, rect.height), (80, 30));
        assert_eq!(lines.len(), 30);
        assert!(lines[0].spans.iter().all(|s| s.content == "█"));
        let (_, white) = raster(&vec![0xffffff; 320 * 240], rect, 0.5, "mono");
        assert!(white[0].spans.iter().all(|s| s.content == " "));
    }
}
