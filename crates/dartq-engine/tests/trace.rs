//! `--trace-events` JSONL face pins (PRD §17.4): a real apply/analyze
//! sequence records the taxonomy events with stable bytes, and the capture
//! aggregates to the §19.3 Cache Hit Rate.

mod common;

use common::{Fixture, GREETER_0, MAIN_0};
use dartq_engine::EngineEvent;
use dartq_engine::event::{TraceSink, cache_hit_rate};

fn temp_path(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("dartq-trace-{tag}-{}.jsonl", std::process::id()))
}

#[test]
fn trace_capture_records_taxonomy_and_rates() {
    let fixture = Fixture::new(&[("lib/main.dart", MAIN_0), ("lib/greeter.dart", GREETER_0)]);
    let path = temp_path("capture");
    let _ = std::fs::remove_file(&path);

    let mut engine = fixture.engine();
    engine.set_trace_sink(Some(TraceSink::create(&path).expect("sink opens")));
    engine.set_content(&fixture.path("lib/main.dart"), MAIN_0.as_bytes());
    engine.set_content(&fixture.path("lib/greeter.dart"), GREETER_0.as_bytes());
    engine
        .analyze(&[], &mut |_: EngineEvent| {}, &Default::default())
        .expect("cold analyze");

    let capture = std::fs::read_to_string(&path).expect("capture readable");
    // The apply boundary opens the capture, byte-exact (counts, stable
    // field order — the record structs are the contract).
    assert!(
        capture.starts_with("{\"event\":\"OverlayApplied\",\"changed\":2,\"removed\":0}\n"),
        "unexpected capture head: {capture}"
    );
    for kind in ["FileRevalidated", "CascadeInvalidated", "LibraryAnalyzed"] {
        assert!(
            capture.contains(&format!("\"event\":\"{kind}\"")),
            "missing {kind} in: {capture}"
        );
    }
    // A cold run serves nothing from cache: §19.3 rate 0.
    assert_eq!(cache_hit_rate(&capture), Some(0.0));

    // The clean re-request of one library reports from_cache — the
    // §1.3/§1.4 AnalyzeFile face — seeding the rate: 1 hit of 3 analyses.
    let main = fixture.path("lib/main.dart");
    let main = engine.file_table().try_file_id(&main).expect("interned");
    engine
        .analyze(&[main], &mut |_: EngineEvent| {}, &Default::default())
        .expect("warm analyze");
    let capture = std::fs::read_to_string(&path).expect("capture grows in place");
    assert_eq!(cache_hit_rate(&capture), Some(1.0 / 3.0));

    drop(engine);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn disarmed_sink_stops_the_capture() {
    let fixture = Fixture::new(&[("lib/main.dart", MAIN_0)]);
    let path = temp_path("disarm");
    let _ = std::fs::remove_file(&path);

    let mut engine = fixture.engine();
    engine.set_trace_sink(Some(TraceSink::create(&path).expect("sink opens")));
    engine.set_content(&fixture.path("lib/main.dart"), MAIN_0.as_bytes());
    engine
        .analyze(&[], &mut |_: EngineEvent| {}, &Default::default())
        .expect("cold analyze");
    let armed_len = std::fs::metadata(&path)
        .expect("armed capture exists")
        .len();
    assert!(armed_len > 0, "an armed run records");

    // Disarming (the `None` short-circuit) stops all writes: the capture
    // stays byte-count-identical through further work.
    engine.set_trace_sink(None);
    engine.set_content(
        &fixture.path("lib/main.dart"),
        "String greet() => 'hi';\n".as_bytes(),
    );
    engine
        .analyze(&[], &mut |_: EngineEvent| {}, &Default::default())
        .expect("second analyze");
    let disarmed_len = std::fs::metadata(&path).expect("capture persists").len();
    assert_eq!(armed_len, disarmed_len, "a disarmed engine writes nothing");

    drop(engine);
    let _ = std::fs::remove_file(&path);
}
