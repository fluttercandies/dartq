//! Golden-shape tests: every modeled protocol type is parsed from
//! JSON in the shape the upstream "Dart VM Service Protocol" document
//! publishes, and every type that can be re-encoded round-trips
//! without losing information.

use dartq_debug::{
    BreakpointLocation, Event, EventKind, ExceptionPauseMode, Instance, InstanceKind, Isolate,
    ReloadReport, ScriptList, Sentinel, SentinelKind, Stack, StepOption, ValueOrSentinel, Version,
    Vm, VmObject,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

/// Parses `golden`, re-encodes it, parses again and asserts the two
/// typed values are equal — a drifted serde attribute or a missing
/// field surfaces here before it reaches a live VM.
fn round_trip<T>(golden: &str) -> T
where
    T: DeserializeOwned + Serialize + PartialEq + std::fmt::Debug,
{
    let first: T = serde_json::from_str(golden).expect("golden shape must parse");
    let encoded = serde_json::to_string(&first).expect("typed value must re-encode");
    let second: T = serde_json::from_str(&encoded).expect("re-encoded shape must parse");
    assert_eq!(first, second, "round-trip changed the value");
    first
}

#[test]
fn version_golden_round_trips() {
    let version: Version = round_trip(r#"{"type":"Version","major":4,"minor":23}"#);
    assert_eq!(version.major, 4);
    assert_eq!(version.minor, 23);
}

#[test]
fn sentinel_golden_round_trips() {
    let sentinel: Sentinel =
        round_trip(r#"{"type":"Sentinel","kind":"Collected","valueAsString":"<collected>"}"#);
    assert_eq!(sentinel.kind, SentinelKind::Collected);
    assert_eq!(sentinel.value_as_string.as_deref(), Some("<collected>"));

    let unknown: Sentinel =
        round_trip(r#"{"type":"Sentinel","kind":"NotARealSentinel","valueAsString":"?"}"#);
    assert_eq!(
        unknown.kind,
        SentinelKind::Other("NotARealSentinel".to_owned())
    );
}

#[test]
fn isolate_golden_round_trips() {
    let golden = r#"{
        "type": "Isolate",
        "id": "isolates/2548320041",
        "number": "2548320041",
        "name": "worker-isolate",
        "isSystemIsolate": false,
        "isolateGroupId": "isolateGroups/1",
        "runnable": true,
        "pauseOnExit": false,
        "pauseEvent": {
            "type": "Event",
            "kind": "PauseBreakpoint",
            "isolate": {"type": "@Isolate", "id": "isolates/2548320041", "number": "2548320041", "name": "worker-isolate"},
            "timestamp": 1700000000000,
            "atAsyncSuspension": true
        },
        "rootLib": {"type": "@Library", "id": "libraries/1", "name": "", "uri": "file:///app/main.dart"},
        "breakpoints": [{
            "type": "Breakpoint",
            "id": "breakpoints/1",
            "breakpointNumber": 1,
            "enabled": true,
            "resolved": true,
            "location": {
                "type": "SourceLocation",
                "script": {"type": "@Script", "id": "scripts/1", "uri": "file:///app/main.dart"},
                "tokenPos": 145,
                "endTokenPos": 145,
                "line": 7,
                "column": 3
            }
        }],
        "exceptionPauseMode": "Unhandled"
    }"#;
    let isolate: Isolate = round_trip(golden);
    assert!(isolate.runnable);
    assert_eq!(isolate.name.as_deref(), Some("worker-isolate"));
    assert_eq!(isolate.exception_pause_mode, Some(ExceptionPauseMode::Unhandled));
    assert_eq!(isolate.breakpoints.len(), 1);
    assert!(isolate.breakpoints[0].resolved);
    assert!(matches!(
        isolate.breakpoints[0].location,
        Some(BreakpointLocation::Resolved(ref location)) if location.line == Some(7)
    ));
    let pause = isolate.pause_event.expect("pause event");
    assert!(matches!(pause.kind, EventKind::PauseBreakpoint));
    assert!(pause.at_async_suspension);
}

#[test]
fn isolate_start_event_golden_round_trips() {
    let golden = r#"{
        "type": "Event",
        "kind": "IsolateStart",
        "isolate": {
            "type": "@Isolate",
            "id": "isolates/2548320041",
            "number": "2548320041",
            "name": "worker-isolate"
        },
        "timestamp": 1700000000000
    }"#;
    let event: Event = round_trip(golden);
    assert!(matches!(event.kind, EventKind::IsolateStart));
    assert_eq!(
        event.isolate.as_ref().map(|isolate| isolate.id.as_str()),
        Some("isolates/2548320041")
    );
}

#[test]
fn gc_event_golden_accepts_unmodeled_fields() {
    // The spec's GC example carries fields this crate does not model;
    // they must be ignored, not rejected.
    let golden = r#"{
        "type": "Event",
        "kind": "GC",
        "isolate": {"type": "@Isolate", "id": "isolates/1", "number": "1", "name": "main"},
        "vm": {"type": "@VM", "name": "vm"},
        "timestamp": 1700000000000,
        "gcType": "MarkSweep",
        "reason": "NewSpace",
        "newSpace": {"type": "HeapSpace", "used": 100, "capacity": 200},
        "oldSpace": {"type": "HeapSpace", "used": 300, "capacity": 400}
    }"#;
    let event: Event = round_trip(golden);
    assert!(matches!(event.kind, EventKind::Gc));
}

#[test]
fn pause_breakpoint_event_golden_round_trips() {
    let golden = r#"{
        "type": "Event",
        "kind": "PauseBreakpoint",
        "isolate": {"type": "@Isolate", "id": "isolates/1", "number": "1", "name": "main"},
        "timestamp": 1700000000000,
        "breakpoint": {
            "type": "Breakpoint",
            "id": "breakpoints/1",
            "breakpointNumber": 1,
            "enabled": true,
            "resolved": true,
            "location": {
                "type": "SourceLocation",
                "script": {"type": "@Script", "id": "scripts/1", "uri": "package:app/main.dart"},
                "tokenPos": 145,
                "line": 7,
                "column": 3
            }
        },
        "pauseBreakpoints": [{
            "type": "Breakpoint",
            "id": "breakpoints/1",
            "breakpointNumber": 1,
            "enabled": true,
            "resolved": true
        }],
        "topFrame": {
            "type": "Frame",
            "index": 0,
            "function": {"type": "@Function", "id": "functions/1", "name": "main"},
            "location": {
                "type": "SourceLocation",
                "script": {"type": "@Script", "id": "scripts/1", "uri": "package:app/main.dart"},
                "tokenPos": 145,
                "line": 7,
                "column": 3
            },
            "vars": [{
                "name": "count",
                "value": {"type": "@Instance", "id": "objects/7", "kind": "Int", "valueAsString": "3"},
                "staticType": {"type": "@Instance", "id": "objects/8", "kind": "Int"}
            }],
            "kind": "Regular"
        },
        "atAsyncSuspension": false
    }"#;
    let event: Event = round_trip(golden);
    assert!(matches!(event.kind, EventKind::PauseBreakpoint));
    assert_eq!(event.pause_breakpoints.len(), 1);
    let frame = event.top_frame.expect("top frame");
    assert_eq!(frame.vars.len(), 1);
    assert_eq!(frame.vars[0].name, "count");
    match frame.vars[0].value.as_ref().expect("bound value") {
        ValueOrSentinel::Instance(instance) => {
            assert_eq!(instance.id, "objects/7");
            assert_eq!(instance.kind, InstanceKind::Int);
            assert_eq!(instance.value_as_string.as_deref(), Some("3"));
        }
        other => panic!("expected instance value, got {other:?}"),
    }
}

/// An `Instance` with every optional field at its default — the base
/// for equality assertions on parsed values.
fn minimal_instance() -> Instance {
    serde_json::from_str(
        r#"{"type":"@Instance","id":"","kind":"PlainInstance"}"#,
    )
    .expect("minimal instance parses")
}

#[test]
fn write_event_golden_round_trips() {
    let golden = r#"{
        "type": "Event",
        "kind": "WriteEvent",
        "isolate": {"type": "@Isolate", "id": "isolates/1", "number": "1", "name": "main"},
        "timestamp": 1700000000000,
        "bytes": "aGVsbG8K"
    }"#;
    let event: Event = round_trip(golden);
    assert_eq!(event.bytes.as_deref(), Some("aGVsbG8K"));
}

#[test]
fn breakpoint_locations_round_trip() {
    let resolved: BreakpointLocation = round_trip(
        r#"{"type":"SourceLocation","script":{"type":"@Script","id":"scripts/1","uri":"package:app/main.dart"},"tokenPos":145,"endTokenPos":145,"line":7,"column":3}"#,
    );
    assert!(matches!(resolved, BreakpointLocation::Resolved(ref location) if location.end_token_pos == Some(145)));

    let unresolved: BreakpointLocation = round_trip(
        r#"{"type":"UnresolvedSourceLocation","scriptUri":"package:app/main.dart","tokenPos":200,"line":11,"column":5}"#,
    );
    match unresolved {
        BreakpointLocation::Unresolved(location) => {
            assert_eq!(location.script_uri.as_deref(), Some("package:app/main.dart"));
            assert_eq!(location.line, Some(11));
            assert_eq!(location.column, Some(5));
        }
        other => panic!("expected unresolved location, got {other:?}"),
    }

    let unknown: BreakpointLocation =
        round_trip(r#"{"type":"SomethingElse","line":1}"#);
    assert_eq!(unknown, BreakpointLocation::Unknown);
}

#[test]
fn stack_golden_round_trips() {
    let golden = r#"{
        "type": "Stack",
        "frames": [{
            "type": "Frame",
            "index": 0,
            "function": {"type": "@Function", "id": "functions/1", "name": "main"},
            "location": {
                "type": "SourceLocation",
                "script": {"type": "@Script", "id": "scripts/1", "uri": "package:app/main.dart"},
                "tokenPos": 145,
                "line": 7,
                "column": 3
            },
            "vars": [],
            "kind": "Regular"
        }],
        "asyncCausalFrames": [{
            "type": "Frame",
            "index": 1,
            "function": {"type": "@Function", "id": "functions/2", "name": "loadData"},
            "kind": "AsyncCausal"
        }],
        "truncated": true
    }"#;
    let stack: Stack = round_trip(golden);
    assert_eq!(stack.frames.len(), 1);
    assert_eq!(stack.async_causal_frames.len(), 1);
    assert!(stack.truncated);
    assert_eq!(
        stack.async_causal_frames[0].kind,
        Some(dartq_debug::FrameKind::AsyncCausal)
    );
}

#[test]
fn instance_golden_round_trips() {
    let object: Instance = round_trip(
        r#"{
            "type": "Instance",
            "id": "objects/42",
            "kind": "PlainInstance",
            "class": {"type": "@Class", "id": "classes/1", "name": "Cart"},
            "fields": [
                {
                    "decl": {"type": "@Field", "id": "fields/1", "name": "items"},
                    "name": "items",
                    "value": {
                        "type": "@Instance",
                        "id": "objects/43",
                        "kind": "List",
                        "class": {"type": "@Class", "id": "classes/2", "name": "_GrowableList"},
                        "length": 2
                    }
                }
            ]
        }"#,
    );
    assert_eq!(object.kind, InstanceKind::PlainInstance);
    assert_eq!(
        object.class.as_ref().and_then(|class| class.name.as_deref()),
        Some("Cart")
    );
    assert_eq!(object.fields.len(), 1);
    assert_eq!(object.fields[0].name, Some(json!("items")));
}

#[test]
fn list_and_map_instances_round_trip() {
    let list: Instance = round_trip(
        r#"{
            "type": "Instance",
            "id": "objects/10",
            "kind": "List",
            "length": 2,
            "elements": [
                {"type": "@Instance", "id": "objects/11", "kind": "Int", "valueAsString": "1"},
                {"type": "@Instance", "id": "objects/12", "kind": "String", "valueAsString": "two"}
            ]
        }"#,
    );
    assert_eq!(list.length, Some(2));
    assert_eq!(list.elements.len(), 2);
    assert_eq!(list.elements[0], ValueOrSentinel::Instance(Box::new({
        let mut instance = minimal_instance();
        instance.id = "objects/11".to_owned();
        instance.kind = InstanceKind::Int;
        instance.value_as_string = Some("1".to_owned());
        instance
    })));

    let map: Instance = round_trip(
        r#"{
            "type": "Instance",
            "id": "objects/20",
            "kind": "Map",
            "length": 1,
            "associations": [{
                "key": {"type": "@Instance", "id": "objects/21", "kind": "String", "valueAsString": "k"},
                "value": {"type": "@Instance", "id": "objects/22", "kind": "Int", "valueAsString": "9"}
            }]
        }"#,
    );
    assert_eq!(map.kind, InstanceKind::Map);
    assert_eq!(map.associations.len(), 1);
    assert!(map.associations[0].key.is_some());
}

#[test]
fn typed_data_and_string_instances_round_trip() {
    let bytes: Instance = round_trip(
        r#"{"type":"Instance","id":"objects/30","kind":"Uint8List","length":3,"bytes":"AQID"}"#,
    );
    assert_eq!(bytes.kind, InstanceKind::Uint8List);
    assert_eq!(bytes.bytes.as_deref(), Some("AQID"));

    let text: Instance = round_trip(
        r#"{"type":"Instance","id":"objects/31","kind":"String","valueAsString":"hello wor","valueAsStringIsTruncated":true,"length":500}"#,
    );
    assert_eq!(text.kind, InstanceKind::String);
    assert!(text.value_as_string_is_truncated);
    assert_eq!(text.length, Some(500));
}

#[test]
fn unknown_instance_kind_is_preserved() {
    let instance: Instance =
        round_trip(r#"{"type":"@Instance","id":"objects/1","kind":"FutureInstanceKind"}"#);
    assert_eq!(
        instance.kind,
        InstanceKind::Other("FutureInstanceKind".to_owned())
    );
}

#[test]
fn script_list_and_reload_report_round_trip() {
    let scripts: ScriptList = round_trip(
        r#"{"type":"ScriptList","scripts":[
            {"type":"@Script","id":"scripts/1","uri":"package:app/main.dart"},
            {"type":"@Script","id":"scripts/2","uri":"package:app/cart.dart"}
        ]}"#,
    );
    assert_eq!(scripts.scripts.len(), 2);
    assert_eq!(
        scripts.scripts[1].uri.as_deref(),
        Some("package:app/cart.dart")
    );

    let report: ReloadReport = round_trip(r#"{"type":"ReloadReport","success":true}"#);
    assert!(report.success);
}

#[test]
fn vm_golden_round_trips() {
    let golden = r#"{
        "type": "VM",
        "name": "vm",
        "architectureBits": 64,
        "hostCPU": "Apple M3",
        "operatingSystem": "macos",
        "targetCPU": "arm64",
        "version": "3.15.0 (stable)",
        "pid": 4242,
        "startTime": 1700000000000,
        "isolates": [{"type": "@Isolate", "id": "isolates/1", "number": "1", "name": "main"}],
        "isolateGroups": [{"type": "@IsolateGroup", "id": "isolateGroups/1"}],
        "systemIsolates": [],
        "systemIsolateGroups": []
    }"#;
    let vm: Vm = round_trip(golden);
    assert_eq!(vm.architecture_bits, 64);
    assert_eq!(vm.pid, 4242);
    assert_eq!(vm.isolates.len(), 1);
    assert_eq!(vm.version.as_deref(), Some("3.15.0 (stable)"));
}

#[test]
fn step_option_spellings_match_the_wire() {
    for (step, spelling) in [
        (StepOption::Into, "Into"),
        (StepOption::Over, "Over"),
        (StepOption::OverAsyncSuspension, "OverAsyncSuspension"),
        (StepOption::Out, "Out"),
        (StepOption::Rewind, "Rewind"),
    ] {
        assert_eq!(step.as_str(), spelling);
        assert_eq!(
            serde_json::to_value(step).expect("serialize step"),
            json!(spelling)
        );
    }
}

#[test]
fn exception_pause_mode_round_trips_and_defaults_to_none() {
    for (mode, spelling) in [
        (ExceptionPauseMode::None, "None"),
        (ExceptionPauseMode::Unhandled, "Unhandled"),
        (ExceptionPauseMode::All, "All"),
    ] {
        let encoded = serde_json::to_value(mode).expect("serialize mode");
        assert_eq!(encoded, json!(spelling));
        let decoded: ExceptionPauseMode =
            serde_json::from_value(encoded).expect("deserialize mode");
        assert_eq!(decoded, mode);
    }
    let unknown: ExceptionPauseMode =
        serde_json::from_value(json!("SomethingWider")).expect("unknown mode parses");
    assert_eq!(unknown, ExceptionPauseMode::None);
}

#[test]
fn value_or_sentinel_round_trips() {
    let golden = json!({
        "type": "@Instance",
        "id": "objects/7",
        "kind": "Int",
        "valueAsString": "3"
    });
    let parsed: ValueOrSentinel = serde_json::from_value(golden).expect("instance slot");
    assert!(matches!(parsed, ValueOrSentinel::Instance(ref instance) if instance.id == "objects/7"));
    let encoded = serde_json::to_value(&parsed).expect("re-encode instance slot");
    assert_eq!(encoded["type"], "Instance");
    assert_eq!(encoded["kind"], "Int");
    let reparsed: ValueOrSentinel = serde_json::from_value(encoded).expect("reparse");
    assert_eq!(parsed, reparsed);

    let sentinel: ValueOrSentinel = serde_json::from_value(json!({
        "type": "Sentinel",
        "kind": "OptimizedOut",
        "valueAsString": "<optimized out>"
    }))
    .expect("sentinel slot");
    let encoded = serde_json::to_value(&sentinel).expect("re-encode sentinel");
    assert_eq!(encoded["type"], "Sentinel");
    let reparsed: ValueOrSentinel = serde_json::from_value(encoded).expect("reparse");
    assert_eq!(sentinel, reparsed);

    let other: ValueOrSentinel =
        serde_json::from_value(json!({"type": "@Library", "id": "libraries/1"}))
            .expect("unknown slot");
    assert!(matches!(other, ValueOrSentinel::Other(_)));
}

#[test]
fn vm_object_dispatches_on_the_type_tag() {
    match VmObject::from_value(json!({
        "type": "Sentinel",
        "kind": "Collected",
        "valueAsString": "<collected>"
    })) {
        VmObject::Sentinel(sentinel) => assert_eq!(sentinel.kind, SentinelKind::Collected),
        other => panic!("expected Sentinel, got {other:?}"),
    }

    match VmObject::from_value(json!({
        "type": "Error",
        "kind": "UnhandledException",
        "message": "boom"
    })) {
        VmObject::Error { kind, message } => {
            assert_eq!(kind, "UnhandledException");
            assert_eq!(message, "boom");
        }
        other => panic!("expected Error, got {other:?}"),
    }

    match VmObject::from_value(json!({
        "type": "@Instance",
        "id": "objects/1",
        "kind": "Int",
        "valueAsString": "1"
    })) {
        VmObject::Instance(instance) => assert_eq!(instance.kind, InstanceKind::Int),
        other => panic!("expected Instance, got {other:?}"),
    }

    match VmObject::from_value(json!({"type": "@Script", "id": "scripts/1"})) {
        VmObject::Other(value) => assert_eq!(value["type"], "@Script"),
        other => panic!("expected Other, got {other:?}"),
    }

    // An Instance-tagged value with a missing required field degrades
    // to `Other` carrying the raw payload, so the caller still sees
    // what the VM sent.
    match VmObject::from_value(json!({"type": "@Instance", "valueAsString": "x"})) {
        VmObject::Other(Value::Object(raw)) => assert_eq!(raw["valueAsString"], "x"),
        other => panic!("expected raw Other, got {other:?}"),
    }
}

#[test]
fn event_without_timestamp_is_a_protocol_error() {
    // The spec requires `int timestamp` — a missing field must not
    // silently default to zero.
    let missing = serde_json::from_str::<Event>(
        r#"{"type":"Event","kind":"Resume","isolate":{"type":"@Isolate","id":"isolates/1"}}"#,
    );
    assert!(missing.is_err(), "an event without timestamp must fail");
    let wrong_type = serde_json::from_str::<Event>(
        r#"{"type":"Event","kind":"Resume","timestamp":"soon"}"#,
    );
    assert!(wrong_type.is_err(), "a non-integer timestamp must fail");
}

#[test]
fn unknown_event_kind_round_trips() {
    let event: Event = round_trip(
        r#"{"type":"Event","kind":"FuturePauseKind","timestamp":1700000000000}"#,
    );
    assert_eq!(
        event.kind,
        EventKind::Other("FuturePauseKind".to_owned())
    );
}
