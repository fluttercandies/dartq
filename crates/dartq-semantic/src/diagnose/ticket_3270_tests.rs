//! #3270 — the `Receiver::Created` write face keeps the creation's written
//! type arguments.
//!
//! `Box<B>().slot = B()` over-reported `invalid_assignment`: the folded
//! creation receiver named only the container, so the write target's gate
//! read the declared member face's bare `T` — a type variable admits no
//! interface source, and the written `B` failed it. Upstream instantiates
//! the receiver's resolved type (`Box<B>`) and the write type is the
//! already-instantiated `B`
//! (pkg/analyzer/lib/src/generated/error_detection_helpers.dart:72-75,
//! `correspondingParameter.type`).
//!
//! The fix carries the receiver's own instantiation into the same
//! environment the `Receiver::Typed` arm already composed
//! ([`Engine::member_recv_env`]): the staged member-read record keys the
//! creation head's offset, [`Engine::creation_row_ty`] re-derives the
//! written `<B>` from it, and a raw `Box()` (no written group) falls to
//! the instantiate-to-bounds residual [`Engine::instantiated_head_ty`]
//! answers — the oracle's `staticType` for a raw generic head.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block.

use crate::diagnose::test_rig::{RigImport, code_names, report_ops, report_ops_imports, rows};

/// Asserts `invalid_assignment` fires at least once for `fixture`.
fn assert_invalid_assignment(fixture: &str) {
    let report = report_ops(fixture);
    assert!(
        code_names(&report).contains(&"invalid_assignment"),
        "no invalid_assignment row: {:#?}",
        rows(&report)
    );
}

/// Asserts `invalid_assignment` never fires for `fixture`.
fn assert_assignment_silent(fixture: &str) {
    let report = report_ops(fixture);
    assert!(
        !code_names(&report).contains(&"invalid_assignment"),
        "rows: {:#?}",
        rows(&report)
    );
}

/// The head pin: `Box<B>().slot` writes against `B` — the receiver's
/// written argument, never the container's bare `T`. Was EXTRA.
#[test]
fn ticket_3270_written_args_instantiate_member_write() {
    assert_assignment_silent(
        "class Box<T> { T slot = throw ''; }\nclass B {}\nvoid f() {\n  Box<B>().slot = B();\n}",
    );
}

/// A mismatched source still reports: `Box<B>().slot = C()` checks `C`
/// against the instantiated `B`, and `C` is no subtype of `B` — the
/// negative face the fix must not silence.
#[test]
fn ticket_3270_unrelated_source_still_reports() {
    assert_invalid_assignment(
        "class Box<T> { T slot = throw ''; }\nclass B {}\nclass C {}\nvoid f() {\n  Box<B>().slot = C();\n}",
    );
}

/// A raw `Box()` (no written group) keeps the instantiate-to-bounds
/// residual — `Box<bounds>` is the oracle's resolved `staticType`, so the
/// write face reads the bound, not the bare `T` and not an invented
/// `Box<dynamic>`. `T extends num` bounds the slot to `num`; an `int`
/// source is clean, a `String` source reports.
#[test]
fn ticket_3270_raw_head_keeps_to_bounds_face() {
    assert_assignment_silent(
        "class Box<T extends num> { T slot = throw ''; }\nvoid f() {\n  Box().slot = 1;\n}",
    );
    assert_invalid_assignment(
        "class Box<T extends num> { T slot = throw ''; }\nvoid f() {\n  Box().slot = 's';\n}",
    );
}

/// A prefixed creation's written arguments instantiate the member write
/// exactly like the bare head — `p.Box<B>().slot` writes against `B`.
#[test]
fn ticket_3270_prefixed_written_args() {
    let report = report_ops_imports(
        "import 'package:lib/lib.dart' as p;\nclass B {}\nvoid f() {\n  p.Box<B>().slot = B();\n}",
        &[RigImport {
            uri: "package:lib/lib.dart",
            prefix: Some("p"),
            source: "class Box<T> { T slot = throw ''; }\n",
        }],
    );
    assert!(
        !code_names(&report).contains(&"invalid_assignment"),
        "rows: {:#?}",
        rows(&report)
    );
}

/// A nested written group instantiates the member face: `Box<Box<B>>().slot`
/// writes against `Box<B>`, so a `Box<B>()` source checks clean.
#[test]
fn ticket_3270_nested_written_args() {
    assert_assignment_silent(
        "class Box<T> { T slot = throw ''; }\nclass B {}\nvoid f() {\n  Box<Box<B>>().slot = Box<B>();\n}",
    );
}

/// A multi-argument head instantiates each parameter position:
/// `Pair<A, B>().first` writes against `A`, `.second` against `B`.
#[test]
fn ticket_3270_multi_arg_head() {
    assert_assignment_silent(
        "class Pair<A, B> { A first = throw ''; B second = throw ''; }\nclass X {}\nclass Y {}\nvoid f() {\n  Pair<X, Y>().first = X();\n  Pair<X, Y>().second = Y();\n}",
    );
    assert_invalid_assignment(
        "class Pair<A, B> { A first = throw ''; B second = throw ''; }\nclass X {}\nclass Y {}\nvoid f() {\n  Pair<X, Y>().first = Y();\n}",
    );
}
