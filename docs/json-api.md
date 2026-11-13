# JSON API version 1

`--json` writes one complete JSON document to stdout. Errors, invalid CLI
options and help/version requests also produce a document when JSON is
requested. Human logs, browser URLs and optional TUI output use stderr.

`run --json` keeps deterministic headless execution. Add `--gui` explicitly
to open a desktop window; JSON is emitted after the session closes (or use
`--exit-on-finish`). `data.viewer.desktop` identifies native desktop sessions.
Window diagnostics use stderr. The public report schema remains version 1.

Core fields:

| Field | Meaning |
| --- | --- |
| `schema_version`, `version` | API schema and toolkit version |
| `operation`, `engine`, `file` | Command, always native engine, input path |
| `source` | Upstream encoding, representation, normalized source hash and original input hash |
| `status`, `exit_code`, `error` | Overall outcome and general failure explanation |
| `diagnostics` | Original severity/class/code/message, optional source span, hint and unverified flag |
| `compatibility_limitations` | Native limitations; never reclassified as PPL syntax errors |
| `entry`, `output`, `return_value` | Selected function, PRINT lines and lossless tagged return value |
| `runtime_error`, `stack` | Runtime failure and innermost-first source frames |
| `artifacts` | Retained produced files, relative to invocation cwd when relative paths were used |
| `duration_ms`, `instructions`, `frames_presented` | Observed duration and native counters |
| `trace` | Opt-in statement and presentation observations |
| `data` | Command-specific doctor, debugger, test, watcher and viewer details |

Locations use one-based lines and Unicode character columns with byte offsets
into the upstream normalized source. Container byte offsets are not source
byte offsets. Notes, warnings and errors retain their original severity.

Reals encode as `{"type":"real","value":"49"}`; decimals are strings.
Lists, matrices, complex numbers and calculator integers use the upstream
lossless tagged representation. Do not convert arbitrary real strings to
IEEE doubles if exact calculator precision matters.

| Exit | Status meaning |
| --- | --- |
| 0 | `success` |
| 1 | `invalid_source`, `policy_failed`, `test_failed` |
| 2 | `invalid_request` |
| 3 | `runtime_error` |
| 4 | `timed_out` |
| 5 | `io_error` |
| 6 | `resource_limit` |
| 7 | `input_exhausted` |
| 8 | `unsupported` |
| 70 | `infrastructure_error` |
| 130 | `cancelled` |

A screenshot on failure does not make the operation successful. Worker death
without a terminal result is an infrastructure failure. Suite success means
the assertions passed; each underlying case still carries its true runtime
status, including deliberately expected errors.

Debugger `data.debug_history` contains stop snapshots and inspection replies.
Snapshots represent values before their labelled statement. Watch JSON is
emitted only when watching ends; `data.runs` and `data.last_run` summarize it.
Use `--report PATH` to read atomically replaced reports while watching.

Additional fields may be added within version 1. Consumers should ignore
unknown properties. The structural contract is in [report.schema.json](report.schema.json).
