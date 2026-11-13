//! Statement-level controls and observations from the real VM callback.
use crate::{
    host::{Capture, Emit, InputState},
    model::*,
};
use ppl_syntax::{SourceFile, Span};
use ppl_vm::{DebugAction, Debugger, Machine};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

#[derive(Clone, Copy)]
enum Mode {
    Step,
    Next(usize),
    Out(usize),
    Continue,
}

pub fn location(file: &SourceFile, span: Span) -> Location {
    let lc = file.line_col(span.start);
    Location {
        file: file.name.clone(),
        line: lc.line,
        column: lc.col,
        start_byte: span.start,
        end_byte: span.end,
    }
}

pub struct Controller {
    root: SourceFile,
    sources: BTreeMap<String, SourceFile>,
    mode: Mode,
    breakpoints: BTreeSet<usize>,
    previous: Option<(String, String, u32, usize)>,
    input: Rc<RefCell<InputState>>,
    capture: Rc<RefCell<Capture>>,
    emit: Emit,
    debug: bool,
    trace: bool,
    variables: bool,
    max_events: usize,
    previous_values: BTreeMap<String, Value>,
}

impl Controller {
    pub fn new(
        request: &Request,
        root: SourceFile,
        sources: BTreeMap<String, SourceFile>,
        input: Rc<RefCell<InputState>>,
        capture: Rc<RefCell<Capture>>,
        emit: Emit,
    ) -> Self {
        Self {
            root,
            sources,
            mode: if request.run_to_breakpoint || !request.debug {
                Mode::Continue
            } else {
                Mode::Step
            },
            breakpoints: request.breakpoints.iter().copied().collect(),
            previous: None,
            input,
            capture,
            emit,
            debug: request.debug,
            trace: request.trace,
            variables: request.trace_variables,
            max_events: request.max_trace_events,
            previous_values: BTreeMap::new(),
        }
    }
    fn source<'a>(&'a self, machine: &Machine, frame: usize) -> &'a SourceFile {
        machine
            .call_stack_frames()
            .get(frame)
            .and_then(|f| f.module.as_ref())
            .and_then(|id| self.sources.get(&id.name))
            .unwrap_or(&self.root)
    }
    fn frames(&self, machine: &Machine) -> Vec<Frame> {
        machine
            .call_stack_frames()
            .iter()
            .enumerate()
            .map(|(i, f)| Frame {
                function: f.function.clone(),
                location: location(self.source(machine, i), f.span),
            })
            .collect()
    }
    fn values(values: Vec<(String, ppl_vm::Value)>) -> BTreeMap<String, Value> {
        values
            .into_iter()
            .map(|(n, v)| (n, ppl_engine::arguments::encode(&v)))
            .collect()
    }
    fn snapshot(
        &self,
        machine: &Machine,
        span: Span,
        depth: usize,
        selected: usize,
        reason: &str,
    ) -> DebugSnapshot {
        let stack = self.frames(machine);
        let source = self.source(machine, selected);
        let here = if selected == 0 {
            location(source, span)
        } else {
            stack[selected].location.clone()
        };
        let source_lines = (here.line.saturating_sub(4).max(1)..=here.line.saturating_add(5))
            .filter_map(|line| {
                let text = source.line_text(line);
                (!text.is_empty() || line == here.line).then(|| (line, text.to_owned()))
            })
            .collect();
        DebugSnapshot {
            location: here,
            function: stack
                .get(selected)
                .map(|f| f.function.clone())
                .unwrap_or_default(),
            depth,
            selected_frame: selected,
            source_lines,
            locals: Self::values(machine.locals(selected)),
            globals: Self::values(machine.globals_for_frame(selected)),
            stack,
            breakpoints: self.breakpoints.iter().copied().collect(),
            reason: reason.into(),
        }
    }
    fn send(&self, event: Event) -> bool {
        let value = serde_json::to_value(&event).expect("debug event is serializable");
        let bytes = value.to_string().len();
        let mut capture = self.capture.borrow_mut();
        if capture.debug_history.len() >= self.max_events
            || capture.trace_bytes + bytes > 4 * 1024 * 1024
        {
            capture.trace_exhausted = true;
            return false;
        }
        capture.trace_bytes += bytes;
        capture.debug_history.push(value);
        drop(capture);
        (self.emit)(event);
        true
    }
    fn reply(&self, text: impl Into<String>) -> bool {
        self.send(Event::DebugReply { text: text.into() })
    }
}

impl Debugger for Controller {
    fn on_statement(
        &mut self,
        machine: &Machine,
        function: &str,
        span: Span,
        depth: usize,
    ) -> DebugAction {
        if self.input.borrow_mut().pump().is_err() {
            return DebugAction::Abort;
        }
        let here = location(self.source(machine, 0), span);
        if self.trace {
            let mut changes = BTreeMap::new();
            if self.variables {
                let mut current = Self::values(machine.globals());
                current.extend(
                    Self::values(machine.locals(0))
                        .into_iter()
                        .map(|(n, v)| (format!("{depth}:{function}:{n}"), v)),
                );
                for (name, value) in &current {
                    if self.previous_values.get(name) != Some(value) {
                        changes.insert(name.clone(), value.clone());
                    }
                }
                self.previous_values = current;
            }
            let event = json!({
                "kind": "statement",
                "location": here,
                "function": function,
                "depth": depth,
                "stack": self.frames(machine),
                "observed_changes": changes,
            });
            let mut capture = self.capture.borrow_mut();
            if !capture.record_trace(event.clone(), self.max_events) {
                return DebugAction::Abort;
            }
            drop(capture);
            (self.emit)(Event::Trace { event });
        }
        if !self.debug {
            return DebugAction::Continue;
        }
        let position = (here.file.clone(), function.to_owned(), here.line, depth);
        let previous = self.previous.replace(position.clone());
        let breakpoint = here.file == self.root.name
            && self.breakpoints.contains(&(here.line as usize))
            && previous != Some(position);
        let pause = std::mem::take(&mut self.input.borrow_mut().pause_requested);
        let stop = pause
            || breakpoint
            || match self.mode {
                Mode::Step => true,
                Mode::Next(d) => depth <= d,
                Mode::Out(d) => depth < d,
                Mode::Continue => false,
            };
        if !stop {
            return DebugAction::Continue;
        }
        let mut selected = 0;
        let reason = if pause {
            "pause"
        } else if breakpoint {
            "breakpoint"
        } else {
            "step"
        };
        if !self.send(Event::Stop {
            snapshot: Box::new(self.snapshot(machine, span, depth, selected, reason)),
        }) {
            return DebugAction::Abort;
        }
        loop {
            let command = match self.input.borrow_mut().command() {
                Ok(c) => c,
                Err(_) => return DebugAction::Abort,
            };
            let mut parts = command.split_whitespace();
            let Some(command) = parts.next() else {
                continue;
            };
            match command {
                "s" | "step" => {
                    self.mode = Mode::Step;
                    return DebugAction::Continue;
                }
                "n" | "next" => {
                    self.mode = Mode::Next(depth);
                    return DebugAction::Continue;
                }
                "out" | "fin" | "finish" => {
                    self.mode = Mode::Out(depth);
                    return DebugAction::Continue;
                }
                "c" | "continue" | "cont" => {
                    self.mode = Mode::Continue;
                    return DebugAction::Continue;
                }
                "q" | "quit" | "terminate" => return DebugAction::Abort,
                "b" | "break" => {
                    if let Some(line) = parts.next() {
                        if let Ok(line) = line.parse::<usize>() {
                            if line > 0 {
                                self.breakpoints.insert(line);
                                self.reply(format!("Breakpoint set at line {line}"));
                            } else {
                                self.reply("Breakpoint line must be positive");
                            }
                        } else {
                            self.reply("Usage: break [LINE]");
                        }
                    } else {
                        self.reply(format!("Breakpoints: {:?}", self.breakpoints));
                    }
                }
                "d" | "delete" => {
                    if let Some(line) = parts.next() {
                        if let Ok(line) = line.parse() {
                            self.breakpoints.remove(&line);
                        } else {
                            self.reply("Usage: delete [LINE]");
                        }
                    } else {
                        self.breakpoints.clear();
                    }
                    self.reply(format!("Breakpoints: {:?}", self.breakpoints));
                }
                "l" | "locals" | "g" | "globals" => {
                    let values = if command == "l" || command == "locals" {
                        machine.locals(selected)
                    } else {
                        machine.globals_for_frame(selected)
                    };
                    self.reply(
                        values
                            .into_iter()
                            .map(|(n, v)| format!("{n} = {}", v.display(machine.settings.format())))
                            .collect::<Vec<_>>()
                            .join("\n"),
                    );
                }
                "p" | "print" => {
                    let name = parts.next().unwrap_or("");
                    let value = machine
                        .locals(selected)
                        .into_iter()
                        .chain(machine.globals_for_frame(selected))
                        .find(|(n, _)| n.eq_ignore_ascii_case(name));
                    self.reply(
                        value
                            .map(|(n, v)| format!("{n} = {}", v.display(machine.settings.format())))
                            .unwrap_or_else(|| format!("Variable {name:?} not found")),
                    );
                }
                "bt" | "stack" => {
                    self.reply(
                        self.frames(machine)
                            .iter()
                            .enumerate()
                            .map(|(i, f)| {
                                format!(
                                    "#{i} {} at {}:{}:{}",
                                    f.function, f.location.file, f.location.line, f.location.column
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    );
                }
                "up" | "down" | "f" | "frame" | "list" | "status" => {
                    let frames = machine.call_stack_frames().len();
                    let next = match command {
                        "up" => selected.saturating_add(1),
                        "down" => selected.saturating_sub(1),
                        "f" | "frame" => parts
                            .next()
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(selected),
                        _ => selected,
                    };
                    if next < frames {
                        selected = next;
                    } else {
                        self.reply("Frame is outside the current stack");
                    }
                    self.send(Event::Stop {
                        snapshot: Box::new(self.snapshot(
                            machine,
                            span,
                            depth,
                            selected,
                            "inspection",
                        )),
                    });
                }
                "h" | "help" => {
                    self.reply("step (s), next (n), out, continue (c), break [LINE], delete [LINE], locals (l), globals (g), print NAME, stack (bt), up, down, frame N, list, status, quit (q)");
                }
                _ => {
                    self.reply(format!("Unknown command {command:?}; type help"));
                }
            }
            if self.capture.borrow().trace_exhausted {
                return DebugAction::Abort;
            }
        }
    }
}
