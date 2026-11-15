//! `--trace-events` JSONL face pins: a real apply/analyze
//! sequence records the taxonomy events with stable bytes, and the capture
//! aggregates to the Cache Hit Rate.

#![allow(
    clippy::useless_conversion,
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    clippy::type_complexity,
    clippy::format_push_string,
    clippy::manual_let_else,
    clippy::default_trait_access,
    clippy::ptr_arg,
    clippy::needless_pass_by_value,
    clippy::match_same_arms,
    clippy::assert_is_empty,
    clippy::used_underscore_binding,
    clippy::items_after_statements,
    clippy::similar_names,
    clippy::single_match_else,
    clippy::doc_lazy_continuation,
    clippy::unnecessary_get_then_check,
    clippy::match_wildcard_for_single_variants,
    clippy::naive_bytecount,
    clippy::sliced_string_as_bytes,
    clippy::nonminimal_bool,
    clippy::semicolon_if_nothing_returned
)]
mod common;

use common::{Fixture, GREETER_0, MAIN_0};
use dartq_engine::CancellationToken;
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
        .analyze(&[], &mut |_: EngineEvent| {}, &CancellationToken::default())
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
    // A cold run serves nothing from cache: hit rate 0.
    assert_eq!(cache_hit_rate(&capture), Some(0.0));

    // The clean re-request of one library reports from_cache — the
    // AnalyzeFile face — seeding the rate: 1 hit of 3 analyses.
    let main = fixture.path("lib/main.dart");
    let main = engine.file_table().try_file_id(&main).expect("interned");
    engine
        .analyze(
            &[main],
            &mut |_: EngineEvent| {},
            &CancellationToken::default(),
        )
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
        .analyze(&[], &mut |_: EngineEvent| {}, &CancellationToken::default())
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
        .analyze(&[], &mut |_: EngineEvent| {}, &CancellationToken::default())
        .expect("second analyze");
    let disarmed_len = std::fs::metadata(&path).expect("capture persists").len();
    assert_eq!(armed_len, disarmed_len, "a disarmed engine writes nothing");

    drop(engine);
    let _ = std::fs::remove_file(&path);
}
