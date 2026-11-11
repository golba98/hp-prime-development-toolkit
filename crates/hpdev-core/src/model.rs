use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf};

pub const SCHEMA_VERSION: u32 = 1;
pub const UPSTREAM_REVISION: &str = "8d7535ec432e3c66f4f210f32a4ae58c5911d96d";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Request {
    pub operation: String,
    pub file: PathBuf,
    pub source_bytes: Option<Vec<u8>>,
    pub entry: Option<String>,
    pub args: String,
    pub args_format: String,
    pub modules: Vec<String>,
    pub keys: Option<String>,
    pub inputs: Vec<String>,
    pub choices: Vec<usize>,
    pub seed: u64,
    pub angle: String,
    pub max_time: Option<f64>,
    pub max_steps: u64,
    pub max_depth: usize,
    pub max_collection_len: usize,
    pub max_string_bytes: usize,
    pub max_output_chars: usize,
    pub interactive: bool,
    pub emit_frames: bool,
    pub debug: bool,
    pub breakpoints: Vec<usize>,
    pub run_to_breakpoint: bool,
    pub trace: bool,
    pub trace_variables: bool,
    pub max_trace_events: usize,
    pub png: Option<PathBuf>,
    pub deny_warnings: bool,
    pub deny_compatibility: bool,
}

impl Default for Request {
    fn default() -> Self {
        let limits = ppl_vm::Limits::default();
        Self {
            operation: "run".into(),
            file: PathBuf::new(),
            source_bytes: None,
            entry: None,
            args: "[]".into(),
            args_format: "json".into(),
            modules: vec![],
            keys: None,
            inputs: vec![],
            choices: vec![],
            seed: 0x5eed,
            angle: "rad".into(),
            max_time: Some(30.0),
            max_steps: 5_000_000,
            max_depth: limits.max_depth,
            max_collection_len: limits.max_collection_len,
            max_string_bytes: limits.max_string_bytes,
            max_output_chars: limits.max_output_chars,
            interactive: false,
            emit_frames: false,
            debug: false,
            breakpoints: vec![],
            run_to_breakpoint: false,
            trace: false,
            trace_variables: false,
            max_trace_events: 10_000,
            png: None,
            deny_warnings: false,
            deny_compatibility: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Location {
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub start_byte: u32,
    pub end_byte: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: String,
    pub class: String,
    pub code: String,
    pub message: String,
    pub location: Option<Location>,
    pub hint: Option<String>,
    pub unverified: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Frame {
    pub function: String,
    pub location: Location,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub schema_version: u32,
    pub version: String,
    pub operation: String,
    pub engine: String,
    pub file: Option<PathBuf>,
    pub source: Option<ppl_engine::source::SourceIdentity>,
    pub status: String,
    pub exit_code: u8,
    pub diagnostics: Vec<Diagnostic>,
    pub compatibility_limitations: Vec<String>,
    pub entry: Option<String>,
    pub output: Vec<String>,
    pub return_value: Option<Value>,
    pub runtime_error: Option<String>,
    pub error: Option<String>,
    pub stack: Vec<Frame>,
    pub artifacts: Vec<PathBuf>,
    pub duration_ms: f64,
    pub instructions: u64,
    pub frames_presented: u64,
    pub trace: Vec<Value>,
    pub data: Value,
}

impl Report {
    pub fn new(operation: &str, file: Option<PathBuf>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            version: env!("CARGO_PKG_VERSION").into(),
            operation: operation.into(),
            engine: "native".into(),
            file,
            source: None,
            status: "success".into(),
            exit_code: 0,
            diagnostics: vec![],
            compatibility_limitations: vec![],
            entry: None,
            output: vec![],
            return_value: None,
            runtime_error: None,
            error: None,
            stack: vec![],
            artifacts: vec![],
            duration_ms: 0.,
            instructions: 0,
            frames_presented: 0,
            trace: vec![],
            data: Value::Null,
        }
    }
    pub fn fail(&mut self, status: &str, code: u8, message: impl Into<String>) {
        self.status = status.into();
        self.exit_code = code;
        self.error = Some(message.into());
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DebugSnapshot {
    pub location: Location,
    pub function: String,
    pub depth: usize,
    pub selected_frame: usize,
    pub source_lines: Vec<(u32, String)>,
    pub locals: BTreeMap<String, Value>,
    pub globals: BTreeMap<String, Value>,
    pub stack: Vec<Frame>,
    pub breakpoints: Vec<usize>,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Field {
    pub label: String,
    pub help: String,
    pub current: String,
    pub reset: Option<String>,
    pub page_title: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DialogView {
    pub kind: String,
    pub title: String,
    pub message: String,
    pub items: Vec<String>,
    pub fields: Vec<Field>,
    pub focus: usize,
    pub error: Option<String>,
    pub read_only: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DialogResponse {
    Ok,
    Cancel,
    Choice { index: usize },
    Entries { values: Vec<String> },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Control {
    Cancel,
    Debug {
        command: String,
    },
    Key {
        code: i64,
        down: bool,
        #[serde(default)]
        pulse: bool,
    },
    Touch {
        x: i64,
        y: i64,
    },
    Dialog {
        response: DialogResponse,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    Output {
        text: String,
    },
    Clear,
    Frame {
        sequence: u64,
        pixels: Vec<u32>,
        mode: String,
    },
    Dialog {
        dialog: DialogView,
    },
    Stop {
        snapshot: Box<DebugSnapshot>,
    },
    DebugReply {
        text: String,
    },
    Trace {
        event: Value,
    },
    Final {
        report: Box<Report>,
    },
}
