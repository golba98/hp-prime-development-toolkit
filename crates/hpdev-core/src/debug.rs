//! Statement-level controls and observations from the real VM callback.
use crate::{
    eval::eval_with_bindings,
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
    time::Instant,
};

#[derive(Clone, Copy)]
enum Mode {
    Step,
    Next(usize),
    Out(usize),
    Continue,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Breakpoint {
    file: Option<String>,
    line: usize,
    condition: Option<String>,
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

/// Attribute remaining VM instructions after `call_function` completes.
pub fn finalize_profile(capture: &mut Capture, final_steps: u64) {
    if let Some((file, function, line)) = capture.last_profile_site.take() {
        let delta_steps = final_steps.saturating_sub(capture.last_profile_steps);
        let delta_us = capture
            .last_profile_time
            .take()
            .map(|t| t.elapsed().as_secs_f64() * 1_000_000.0)
            .unwrap_or(0.0);
        let f_entry = capture
            .profile_functions
            .entry(function.clone())
            .or_insert((0, 0, 0.0));
        f_entry.1 += delta_steps;
        f_entry.2 += delta_us;
        let l_entry = capture
            .profile_lines
            .entry((file, function, line))
            .or_insert((0, 0, 0.0));
        l_entry.1 += delta_steps;
        l_entry.2 += delta_us;
    }
}

pub struct Controller {
    root: SourceFile,
    sources: BTreeMap<String, SourceFile>,
    mode: Mode,
    breakpoints: Vec<Breakpoint>,
    watches: Vec<String>,
    previous: Option<(String, String, u32, usize, u32)>,
    input: Rc<RefCell<InputState>>,
    capture: Rc<RefCell<Capture>>,
    emit: Emit,
    debug: bool,
    trace: bool,
    trace_filter: Vec<String>,
    trace_summary: bool,
    profile: bool,
    coverage: bool,
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
        let mut breakpoints: Vec<Breakpoint> = request
            .breakpoints
            .iter()
            .map(|&line| Breakpoint {
                file: None,
                line,
                condition: None,
            })
            .collect();
        for (line, cond) in &request.conditional_breakpoints {
            breakpoints.push(Breakpoint {
                file: None,
                line: *line,
                condition: Some(cond.clone()),
            });
        }
        for (file, line, cond) in &request.file_breakpoints {
            breakpoints.push(Breakpoint {
                file: Some(file.clone()),
                line: *line,
                condition: cond.clone(),
            });
        }
        Self {
            root,
            sources,
            mode: if request.run_to_breakpoint || !request.debug {
                Mode::Continue
            } else {
                Mode::Step
            },
            breakpoints,
            watches: vec![],
            previous: None,
            input,
            capture,
            emit,
            debug: request.debug,
            trace: request.trace,
            trace_filter: request.trace_filter.clone(),
            trace_summary: request.trace_summary,
            profile: request.profile,
            coverage: request.coverage,
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

    fn root_breakpoint_lines(&self) -> Vec<usize> {
        let mut lines = BTreeSet::new();
        for bp in &self.breakpoints {
            if bp.file.is_none() || bp.file.as_deref() == Some(self.root.name.as_str()) {
                lines.insert(bp.line);
            }
        }
        lines.into_iter().collect()
    }

    fn evaluate_watches(&self, machine: &Machine, selected: usize) -> BTreeMap<String, Value> {
        let mut out = BTreeMap::new();
        if self.watches.is_empty() {
            return out;
        }
        let locals = machine.locals(selected);
        let globals = machine.globals_for_frame(selected);
        for expr in &self.watches {
            let val = match eval_with_bindings(expr, &locals, &globals, &machine.settings) {
                Ok(v) => ppl_engine::arguments::encode(&v),
                Err(e) => json!({ "error": e }),
            };
            out.insert(expr.clone(), val);
        }
        out
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
            breakpoints: self.root_breakpoint_lines(),
            reason: reason.into(),
            watches: self.evaluate_watches(machine, selected),
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

    fn parse_break_args(&self, rest: &str) -> Result<Breakpoint, String> {
        let rest = rest.trim();
        let (target, condition) = if let Some(idx) = rest.find(" if ") {
            (&rest[..idx], Some(rest[idx + 4..].trim().to_owned()))
        } else {
            (rest, None)
        };
        if let Some(ref c) = condition
            && c.is_empty()
        {
            return Err("Condition after `if` cannot be empty".into());
        }
        let target = target.trim();
        let (file, line_str) = if let Some((f, l)) = target.rsplit_once(':') {
            (Some(f.trim().to_owned()), l.trim())
        } else {
            (None, target)
        };
        let line = line_str
            .parse::<usize>()
            .map_err(|_| "Usage: break [FILE:]LINE [if EXPR]".to_owned())?;
        if line == 0 {
            return Err("Breakpoint line must be positive".into());
        }
        Ok(Breakpoint {
            file,
            line,
            condition,
        })
    }

    fn format_breakpoints(&self) -> String {
        if self.breakpoints.is_empty() {
            return "Breakpoints: []".into();
        }
        let items: Vec<String> = self
            .breakpoints
            .iter()
            .map(|bp| {
                let mut s = match &bp.file {
                    Some(f) => format!("{f}:{}", bp.line),
                    None => bp.line.to_string(),
                };
                if let Some(cond) = &bp.condition {
                    s.push_str(&format!(" if {cond}"));
                }
                s
            })
            .collect();
        format!("Breakpoints: [{}]", items.join(", "))
    }

    fn matching_breakpoint(
        &self,
        machine: &Machine,
        here: &Location,
    ) -> Option<(bool, Option<String>)> {
        let mut condition_error = None;
        for bp in &self.breakpoints {
            let file_matches = match &bp.file {
                None => here.file == self.root.name,
                Some(f) => {
                    here.file == *f
                        || here.file.ends_with(&format!("/{f}"))
                        || here.file.ends_with(&format!("\\{f}"))
                }
            };
            if !file_matches || bp.line != here.line as usize {
                continue;
            }
            match &bp.condition {
                None => return Some((true, None)),
                Some(expr) => {
                    let locals = machine.locals(0);
                    let globals = machine.globals_for_frame(0);
                    match eval_with_bindings(expr, &locals, &globals, &machine.settings) {
                        Ok(val) => {
                            if val.truthy().unwrap_or(false) {
                                return Some((true, None));
                            }
                        }
                        Err(e) => {
                            condition_error = Some(format!(
                                "Breakpoint condition `{expr}` error at line {}: {e}",
                                bp.line
                            ));
                            return Some((true, condition_error));
                        }
                    }
                }
            }
        }
        let _ = condition_error;
        None
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
        if self.coverage {
            self.capture.borrow_mut().covered_spans.insert((
                here.file.clone(),
                span.start,
                span.end,
            ));
        }
        if self.profile {
            let now = Instant::now();
            let steps_now = machine.steps();
            let mut capture = self.capture.borrow_mut();
            finalize_profile(&mut capture, steps_now);
            let f_entry = capture
                .profile_functions
                .entry(function.to_owned())
                .or_insert((0, 0, 0.0));
            f_entry.0 += 1;
            let l_entry = capture
                .profile_lines
                .entry((here.file.clone(), function.to_owned(), here.line))
                .or_insert((0, 0, 0.0));
            l_entry.0 += 1;
            capture.last_profile_site = Some((here.file.clone(), function.to_owned(), here.line));
            capture.last_profile_steps = steps_now;
            capture.last_profile_time = Some(now);
        }
        if self.trace {
            let matches_filter = self.trace_filter.is_empty()
                || self
                    .trace_filter
                    .iter()
                    .any(|f| f.eq_ignore_ascii_case(function));
            if self.trace_summary && matches_filter {
                *self
                    .capture
                    .borrow_mut()
                    .trace_counts
                    .entry(function.to_owned())
                    .or_insert(0) += 1;
            }
            if matches_filter {
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
        }
        if !self.debug {
            return DebugAction::Continue;
        }
        let position = (
            here.file.clone(),
            function.to_owned(),
            here.line,
            depth,
            span.start,
        );
        let previous = self.previous.replace(position);
        let same_line_other_stmt =
            previous
                .as_ref()
                .is_some_and(|(f, fn_name, l, d, prev_start)| {
                    f == &here.file
                        && fn_name == function
                        && *l == here.line
                        && *d == depth
                        && *prev_start != span.start
                });
        let bp_match = if !same_line_other_stmt {
            self.matching_breakpoint(machine, &here)
        } else {
            None
        };
        let breakpoint = bp_match.as_ref().is_some_and(|(hit, _)| *hit);
        let bp_warning = bp_match.and_then(|(_, err)| err);
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
        if let Some(warn) = bp_warning {
            self.reply(warn);
        }
        loop {
            let raw_command = match self.input.borrow_mut().command() {
                Ok(c) => c,
                Err(_) => return DebugAction::Abort,
            };
            let trimmed = raw_command.trim();
            if trimmed.is_empty() {
                continue;
            }
            let mut split = trimmed.splitn(2, char::is_whitespace);
            let command = split.next().unwrap_or("");
            let rest = split.next().unwrap_or("").trim();
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
                    if rest.is_empty() {
                        self.reply(self.format_breakpoints());
                    } else {
                        match self.parse_break_args(rest) {
                            Ok(bp) => {
                                let label = match &bp.file {
                                    Some(f) => format!("{f}:{}", bp.line),
                                    None => format!("line {}", bp.line),
                                };
                                let cond_note = bp
                                    .condition
                                    .as_ref()
                                    .map(|c| format!(" if {c}"))
                                    .unwrap_or_default();
                                self.breakpoints
                                    .retain(|b| !(b.file == bp.file && b.line == bp.line));
                                self.breakpoints.push(bp);
                                self.reply(format!("Breakpoint set at {label}{cond_note}"));
                            }
                            Err(msg) => {
                                self.reply(msg);
                            }
                        }
                    }
                }
                "d" | "delete" => {
                    if rest.is_empty() {
                        self.breakpoints.clear();
                    } else if let Ok(line) = rest.parse::<usize>() {
                        self.breakpoints.retain(|bp| bp.line != line);
                    } else {
                        self.reply("Usage: delete [LINE]");
                    }
                    self.reply(self.format_breakpoints());
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
                    let name = rest;
                    let value = machine
                        .locals(selected)
                        .into_iter()
                        .chain(machine.globals_for_frame(selected))
                        .find(|(n, _)| n.eq_ignore_ascii_case(name));
                    if let Some((n, v)) = value {
                        self.reply(format!("{n} = {}", v.display(machine.settings.format())));
                    } else if name
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '_' || c == 'θ')
                        && !name.is_empty()
                    {
                        self.reply(format!("Variable {name:?} not found"));
                    } else if !name.is_empty() {
                        let locals = machine.locals(selected);
                        let globals = machine.globals_for_frame(selected);
                        match eval_with_bindings(name, &locals, &globals, &machine.settings) {
                            Ok(v) => {
                                self.reply(format!(
                                    "{name} = {}",
                                    v.display(machine.settings.format())
                                ));
                            }
                            Err(e) => {
                                self.reply(format!("Eval error: {e}"));
                            }
                        }
                    } else {
                        self.reply("Usage: print NAME|EXPR");
                    }
                }
                "e" | "eval" => {
                    if rest.is_empty() {
                        self.reply("Usage: eval EXPR");
                    } else {
                        let locals = machine.locals(selected);
                        let globals = machine.globals_for_frame(selected);
                        match eval_with_bindings(rest, &locals, &globals, &machine.settings) {
                            Ok(v) => {
                                self.reply(format!(
                                    "{rest} = {}",
                                    v.display(machine.settings.format())
                                ));
                            }
                            Err(e) => {
                                self.reply(format!("Eval error: {e}"));
                            }
                        }
                    }
                }
                "w" | "watch" => {
                    if rest.is_empty() {
                        self.reply("Usage: watch EXPR");
                    } else {
                        if !self.watches.iter().any(|w| w == rest) {
                            self.watches.push(rest.to_owned());
                        }
                        self.reply(format!("Watching `{rest}`"));
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
                }
                "unwatch" => {
                    if rest.is_empty() || rest == "all" {
                        self.watches.clear();
                        self.reply("Cleared all watches");
                    } else {
                        self.watches.retain(|w| w != rest);
                        self.reply(format!("Removed watch `{rest}`"));
                    }
                }
                "watches" => {
                    let evaluated = self.evaluate_watches(machine, selected);
                    if evaluated.is_empty() {
                        self.reply("No watches set");
                    } else {
                        let lines: Vec<String> = evaluated
                            .into_iter()
                            .map(|(k, v)| format!("{k} = {v}"))
                            .collect();
                        self.reply(lines.join("\n"));
                    }
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
                        "f" | "frame" => rest.parse().unwrap_or(selected),
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
                    self.reply("step (s), next (n), out, continue (c), break [FILE:]LINE [if EXPR], delete [LINE], locals (l), globals (g), print NAME|EXPR, eval EXPR, watch EXPR, unwatch [EXPR|all], watches, stack (bt), up, down, frame N, list, status, quit (q)");
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
