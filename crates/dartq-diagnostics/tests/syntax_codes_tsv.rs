//! Content pin for the authoritative syntax-codes table (`data/syntax-codes.tsv`).

#![allow(
    clippy::items_after_statements,
    clippy::too_many_lines,
    clippy::type_complexity,
    clippy::cast_possible_truncation,
    clippy::assert_is_empty,
    clippy::used_underscore_binding,
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
    clippy::unused_self,
    clippy::fn_params_excessive_bools,
    clippy::semicolon_if_nothing_returned
)]
use std::path::Path;

/// blake3 digest of the pinned TSV snapshot (dart 3.13.7 oracle — the pinned
/// SDK registry `diagnostic.g.dart`; refreshed 2026-11-01 from the 2026-10-08
/// dart 3.13.1 capture, 7 measured-dead rows pruned 2026-10-09 — #222). The
/// table is 259 data rows: 255 registry-gated codes + 4 supplemental rows
/// (the 3-code language-version family plus
/// `invalid_covariant_modifier_in_primary_constructor`).
///
/// Any content edit changes the generated registry: re-verify the full table
/// against the pinned registry sources first, then re-pin this digest
/// deliberately. This test exists so a wrong text cannot ship green.
const PINNED_BLAKE3: &str = "0ca513794ac5b1523688affcb62e93b523288880fa377a23d59b0b16099dbb8b";

#[test]
fn syntax_codes_tsv_matches_pinned_blake3() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("data/syntax-codes.tsv");
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let actual = blake3::hash(&bytes).to_hex().to_string();
    assert_eq!(
        actual, PINNED_BLAKE3,
        "syntax-codes.tsv drifted from the pinned snapshot (actual blake3: {actual}); \
         re-verify against the pinned registry sources, then re-pin deliberately"
    );
}
