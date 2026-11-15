//! Hermetic launch-path tests: the `dart` binary is never started —
//! only discovery failures are exercised.

#![allow(
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
    clippy::single_match,
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
    clippy::cast_possible_wrap,
    clippy::match_wild_err_arm,
    clippy::unnecessary_debug_formatting,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::ignore_without_reason,
    clippy::semicolon_if_nothing_returned
)]
use dartq_debug::{DebugSession, Error, LaunchConfig};

#[test]
fn launch_reports_missing_dart_binary() {
    let missing =
        std::env::temp_dir().join(format!("dartq-debug-missing-dart-{}", std::process::id()));
    assert!(!missing.exists(), "test path must not exist");
    let previous = std::env::var_os("DARTQ_DART");
    // SAFETY: this test binary contains exactly one test, so no other
    // thread reads or writes the process environment concurrently.
    unsafe { std::env::set_var("DARTQ_DART", &missing) };
    let result = DebugSession::launch(LaunchConfig::new("main.dart"));
    match previous {
        Some(value) => unsafe { std::env::set_var("DARTQ_DART", value) },
        None => unsafe { std::env::remove_var("DARTQ_DART") },
    }
    let error = match result {
        Ok(_session) => panic!("a missing dart binary must fail clearly"),
        Err(error) => error,
    };
    assert!(
        matches!(error, Error::DartBinaryNotFound(_)),
        "got {error:?}"
    );
}
