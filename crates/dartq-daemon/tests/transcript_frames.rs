//! Deterministic pins for the shared transcript decoder (`common::frames`
//! and `common::frames_complete`): every partial-write shape a live
//! snapshot can capture must decode without panicking, and only complete
//! frames may surface.

#![allow(
    clippy::too_many_lines,
    clippy::assert_is_empty,
    clippy::used_underscore_binding,
    clippy::items_after_statements,
    clippy::unused_self,
    clippy::type_complexity,
    clippy::cast_possible_truncation,
    clippy::format_push_string,
    clippy::manual_let_else,
    clippy::default_trait_access,
    clippy::ptr_arg,
    clippy::needless_pass_by_value,
    clippy::match_same_arms,
    clippy::similar_names,
    clippy::single_match_else,
    clippy::doc_lazy_continuation,
    clippy::unnecessary_get_then_check,
    clippy::match_wildcard_for_single_variants,
    clippy::naive_bytecount,
    clippy::sliced_string_as_bytes,
    clippy::nonminimal_bool,
    clippy::useless_conversion,
    clippy::question_mark,
    clippy::option_option,
    clippy::needless_continue,
    clippy::missing_panics_doc,
    clippy::trivially_copy_pass_by_ref,
    clippy::explicit_counter_loop,
    clippy::never_loop,
    clippy::needless_range_loop,
    clippy::unnecessary_unwrap,
    clippy::assigning_clones,
    clippy::double_must_use,
    clippy::single_match,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::ignore_without_reason,
    clippy::unnecessary_debug_formatting,
    clippy::match_wild_err_arm,
    clippy::disallowed_names,
    clippy::many_single_char_names,
    clippy::same_item_push,
    clippy::while_let_loop,
    clippy::if_same_then_else,
    clippy::ref_option,
    clippy::case_sensitive_file_extension_comparisons,
    clippy::unnecessary_filter_map,
    clippy::manual_strip,
    clippy::float_cmp,
    clippy::redundant_guards,
    clippy::empty_line_after_doc_comments,
    clippy::unreadable_literal,
    clippy::enum_variant_names,
    clippy::missing_errors_doc,
    clippy::unnecessary_wraps,
    clippy::too_many_arguments,
    clippy::semicolon_if_nothing_returned
)]
mod common;

use common::{frames, frames_complete};
use serde_json::{Value, json};

/// Runs `body` expecting a panic and returns its message.
fn panic_message<R>(body: impl FnOnce() -> R + std::panic::UnwindSafe) -> String {
    std::panic::catch_unwind(body)
        .err()
        .expect("expected a panic")
        .downcast_ref::<String>()
        .expect("panic payload is a String")
        .clone()
}

/// Every byte prefix of a real frame — including cuts inside the
/// multi-byte body characters — decodes to nothing until the frame is
/// complete.
#[test]
fn every_byte_prefix_of_a_frame_decodes_only_when_complete() {
    let message = json!({"msg": "héllo→"});
    let bytes = common::framed(&message);
    for end in 0..bytes.len() {
        assert_eq!(
            frames(&bytes[..end]),
            Vec::<Value>::new(),
            "prefix of {end} bytes"
        );
    }
    assert_eq!(frames(&bytes), vec![message]);
}

/// A complete frame stays decoded while its successor arrives one byte at
/// a time.
#[test]
fn a_complete_frame_survives_an_incomplete_successor() {
    let first = common::framed(&json!({"n": 1}));
    let second = common::framed(&json!({"n": 2}));
    let mut bytes = first.clone();
    for end in 0..second.len() {
        bytes.truncate(first.len());
        bytes.extend_from_slice(&second[..end]);
        assert_eq!(
            frames(&bytes),
            vec![json!({"n": 1})],
            "second frame at {end} bytes"
        );
    }
    bytes.truncate(first.len());
    bytes.extend_from_slice(&second);
    assert_eq!(frames(&bytes), vec![json!({"n": 1}), json!({"n": 2})]);
}

/// The load-flake shape: the header declares 275 body bytes while the
/// snapshot holds none of them — length-based slicing must not panic.
#[test]
fn a_header_only_snapshot_never_panics() {
    assert_eq!(frames(b"Content-Length: 275\r\n\r\n"), Vec::<Value>::new());
}

/// A length too large to address is malformed, not an arithmetic
/// overflow: it fails with the raw bytes like any other bad length.
#[test]
fn an_unaddressable_content_length_fails_explicitly() {
    let bytes = format!("Content-Length: {}\r\n\r\n", usize::MAX);
    let message = panic_message(|| frames(bytes.as_bytes()));
    assert!(message.contains("bad Content-Length"), "{message}");
}

/// Once the server has finished, a pending tail is a framing bug: it must
/// fail with the raw tail, never silently drop the frame.
#[test]
fn frames_complete_rejects_a_truncated_transcript() {
    for truncated in [
        &b"Content-Length: 275\r\n\r\n"[..],
        b"Content-Length: 275\r\n\r\n{\"partial\"",
        b"Content-Length: ",
    ] {
        let message = panic_message(|| frames_complete(truncated));
        assert!(message.contains("mid-frame"), "{message}");
    }
}

/// Complete-but-broken transcripts fail loudly — the decoder never skips
/// garbage to find the next marker.
#[test]
fn malformed_complete_transcripts_fail_loudly() {
    for (bytes, expected) in [
        (
            &b"XXContent-Length: 2\r\n\r\nok"[..],
            "garbage before a frame",
        ),
        (b"Content-Length: six\r\n\r\n{}", "bad Content-Length"),
        (
            b"Content-Length: 2\r\nX\r\n\r\nok",
            "garbage after the length",
        ),
        (b"Content-Length: 3\r\n\r\n{x ", "bad frame body"),
        (
            b"Content-Length: 2\r\n\r\n42 junk",
            "garbage before a frame",
        ),
    ] {
        let message = panic_message(|| frames(bytes));
        assert!(message.contains(expected), "{message}");
    }
}

/// The positive contract: a finished transcript decodes in full.
#[test]
fn frames_complete_accepts_a_finished_transcript() {
    let bytes = [
        common::framed(&json!({"a": 1})),
        common::framed(&json!({"b": 2})),
    ]
    .concat();
    assert_eq!(
        frames_complete(&bytes),
        vec![json!({"a": 1}), json!({"b": 2})]
    );
}
