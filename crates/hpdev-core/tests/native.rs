use hpdev_core::{
    execution::execute,
    host::{Emit, InputState},
    *,
};
use serde_json::json;
use std::{
    cell::RefCell,
    collections::BTreeSet,
    path::PathBuf,
    rc::Rc,
    sync::{Arc, atomic::AtomicBool, mpsc},
};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn run(mut request: Request) -> (Report, Vec<Event>) {
    request.file = root().join(&request.file);
    let (_tx, rx) = mpsc::sync_channel(8);
    let input = Rc::new(RefCell::new(InputState::new(
        rx,
        Arc::new(AtomicBool::new(false)),
    )));
    let events = Rc::new(RefCell::new(vec![]));
    let recorded = events.clone();
    let emit: Emit = Rc::new(move |e| recorded.borrow_mut().push(e));
    let report = execute(request, input, emit);
    let recorded = events.borrow().clone();
    (report, recorded)
}

#[test]
fn preserves_semantic_diagnostic_locations() {
    let (r, _) = run(Request {
        operation: "check".into(),
        file: "tests/fixtures/INVALID.ppl".into(),
        ..Default::default()
    });
    assert_eq!(r.exit_code, 1);
    assert_eq!(r.diagnostics[0].code, "undeclared-identifier");
    let loc = r.diagnostics[0].location.as_ref().unwrap();
    assert_eq!((loc.line, loc.column), (3, 10));
    assert_eq!(r.diagnostics[0].class, "semantic");
}
#[test]
fn compatibility_is_not_a_language_error() {
    let (r, _) = run(Request {
        operation: "check".into(),
        file: "tests/fixtures/UNSUPPORTED.ppl".into(),
        ..Default::default()
    });
    assert_eq!(r.exit_code, 0);
    assert!(!r.compatibility_limitations.is_empty());
    assert!(r.diagnostics.iter().all(|d| d.class == "compatibility"));
    let (r, _) = run(Request {
        file: "tests/fixtures/UNSUPPORTED.ppl".into(),
        ..Default::default()
    });
    assert_eq!((r.status.as_str(), r.exit_code), ("unsupported", 8));
    assert!(!r.stack.is_empty());
}
#[test]
fn receives_every_real_presentation_in_order() {
    let (r, events) = run(Request {
        file: "examples/LIVE.hpprgm".into(),
        emit_frames: true,
        trace: true,
        ..Default::default()
    });
    assert_eq!(r.return_value, Some(json!({"type":"real","value":"5"})));
    let frames = events
        .iter()
        .filter_map(|e| {
            if let Event::Frame {
                sequence, pixels, ..
            } = e
            {
                Some((*sequence, pixels[0]))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(frames.len() as u64, r.frames_presented);
    assert!(frames.windows(2).all(|w| w[1].0 == w[0].0 + 1));
    let colors = frames.iter().map(|(_, p)| *p).collect::<BTreeSet<_>>();
    for red in [40, 80, 120, 160, 200] {
        assert!(colors.contains(&(red << 16)));
    }
    assert!(r.trace.iter().any(|e| e["kind"] == "presentation"));
}
#[test]
fn default_runs_do_not_serialize_frames() {
    let (r, events) = run(Request {
        file: "examples/LIVE.hpprgm".into(),
        ..Default::default()
    });
    assert_eq!(r.exit_code, 0);
    assert!(r.frames_presented >= 5);
    assert!(!events.iter().any(|e| matches!(e, Event::Frame { .. })));
}
#[test]
fn scripted_input_and_cancellation_preserve_writeback() {
    let (r, _) = run(Request {
        file: "examples/INPUT.ppl".into(),
        inputs: vec!["7".into()],
        choices: vec![2],
        ..Default::default()
    });
    assert_eq!(r.output, ["9"]);
    assert_eq!(r.return_value.unwrap()["value"], "9");
    let (r, _) = run(Request {
        file: "examples/INPUT.ppl".into(),
        inputs: vec!["<ESC>".into()],
        ..Default::default()
    });
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.return_value.unwrap()["value"], "-1");
    let (r, _) = run(Request {
        file: "examples/INPUT.ppl".into(),
        ..Default::default()
    });
    assert_eq!((r.status.as_str(), r.exit_code), ("input_exhausted", 7));
}
#[test]
fn loads_real_containers_and_both_utf16_byte_orders() {
    let (fixture, _) = run(Request {
        file: "tests/fixtures/MINIMAL.hpprgm".into(),
        ..Default::default()
    });
    assert_eq!(fixture.exit_code, 0);
    assert_eq!(fixture.return_value.unwrap()["value"], "1");
    assert_eq!(fixture.source.unwrap().representation, "hpprgm-container");
    let source = "EXPORT TST() BEGIN RETURN 49; END;";
    let container = ppl_syntax::hpprgm::build_program(source)
        .unwrap()
        .to_bytes();
    let mut variants = vec![container];
    for be in [false, true] {
        let mut bytes = if be { vec![254, 255] } else { vec![255, 254] };
        for word in source.encode_utf16() {
            bytes.extend(if be {
                word.to_be_bytes()
            } else {
                word.to_le_bytes()
            });
        }
        variants.push(bytes);
    }
    for source_bytes in variants {
        let (r, _) = run(Request {
            file: "virtual.hpprgm".into(),
            source_bytes: Some(source_bytes),
            ..Default::default()
        });
        assert_eq!(r.exit_code, 0, "{:?}", r.error);
        assert_eq!(r.return_value.unwrap()["value"], "49");
    }
}
#[test]
fn precise_math_and_collection_semantics_come_from_upstream() {
    let source = "EXPORT TST() BEGIN LOCAL L:={1,2},M; M:=L; M(1):=9; RETURN {1/3,L,M,[[1,2],[3,4]]*[[1],[1]]}; END;";
    let (r, _) = run(Request {
        file: "virtual.ppl".into(),
        source_bytes: Some(source.as_bytes().to_vec()),
        ..Default::default()
    });
    assert_eq!(r.exit_code, 0, "{:?}", r.error);
    let values = &r.return_value.unwrap()["value"];
    assert_eq!(values[0]["value"], "0.333333333333");
    assert_eq!(values[1]["value"][0]["value"], "1");
    assert_eq!(values[2]["value"][0]["value"], "9");
    assert_eq!(values[3]["value"][0]["value"], "3");
}
#[test]
fn instruction_and_trace_limits_are_failures() {
    let (r, _) = run(Request {
        file: "tests/fixtures/LOOP.ppl".into(),
        max_steps: Some(1024),
        ..Default::default()
    });
    assert_eq!(r.exit_code, 6);
    let (r, _) = run(Request {
        file: "examples/DEBUG.ppl".into(),
        trace: true,
        max_trace_events: 1,
        ..Default::default()
    });
    assert_eq!((r.status.as_str(), r.exit_code), ("resource_limit", 6));
}
#[test]
fn output_aliases_are_rejected_before_truncation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("A.ppl");
    let text = "EXPORT TST() BEGIN RETURN 1; END;";
    std::fs::write(&path, text).unwrap();
    let (r, _) = run(Request {
        file: path.clone(),
        png: Some(path.clone()),
        ..Default::default()
    });
    assert_eq!(r.exit_code, 2);
    assert_eq!(std::fs::read_to_string(path).unwrap(), text);
}
