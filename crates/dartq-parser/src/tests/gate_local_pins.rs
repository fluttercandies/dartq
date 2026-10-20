//! Parser-local code pins: one presence pin per hand-written `local_code`
//! in [`crate::message`] that sits outside the frozen syntax-codes TSV
//! (the registry-gap rows emitted through the pseudoSharedCode routing).
//! These rows have no TSV gate and no fixture-coverage manifest cell, so a
//! rename or a dropped emitter would pass every other gate — the pins here
//! own per-code visibility, exactly as `gate_dark_pins.rs` does for the
//! TSV documented-gap rows.
//!
//! Oracle posture (pinned 3.13.1 `dart analyze`, probed 2026-10-20): the
//! front-end's pseudoSharedCode faces are NOT analyzer registry rows —
//! `abstract_extension_field` is absent from the analyzer's diagnostic
//! registry (probes surface `abstract_static_field` or
//! `extension_declares_instance_field` instead) and `augment typedef`
//! surfaces no `typedef_augmentation` row even with the experiment flag.
//! These pins therefore nail dartq's own emission (the tripwire contract:
//! the emitter must not silently change), not oracle parity.

use super::collector::setup;

/// Whole-unit parse, diagnostic-code sequence only.
fn codes_of(source: &'static str) -> Vec<String> {
    let mut p = setup(source);
    p.parse_unit();
    p.listener.error_codes()
}

/// Presence pin: `code` must fire at least once on `source`.
fn assert_code_fires(source: &'static str, code: &str) {
    let got = codes_of(source);
    assert!(
        got.iter().any(|c| c == code),
        "`{code}` must fire on {source:?}; got: {got:?}"
    );
}

/// `abstract` on an extension field (official
/// `parser_impl.dart:4556-4558`, the `DeclarationKind.Extension` arm).
#[test]
fn local_pin_abstract_extension_field() {
    assert_code_fires("extension E on int { abstract int x; }\n", "abstract_extension_field");
}

/// `augment` heading a typedef (official `parseTypedef`'s augmentToken
/// report, `parser_impl.dart:1473`).
///
/// `#[ignore]`d: the emitter is currently unreachable — the typedef
/// dispatch arms drop `mods.augment` and never set `pending_augment`
/// (the enum arm's `self.pending_augment = mods.augment;` mirror is
/// missing), so `augment typedef T = int;` parses clean and the probe
/// got zero rows. The contract pin rides in the tree and flips active
/// with the one-line dispatch fix; until then the silent-clean behavior
/// is itself the recorded drift (oracle probe 3.13.1 surfaces no
/// `typedef_augmentation` row on this shape either — with the experiment
/// flag it recovers through `expected_token` instead).
#[test]
#[ignore = "typedef_augmentation emitter unreachable: typedef dispatch arms drop mods.augment (declarations.rs), pending the dispatch fix"]
fn local_pin_typedef_augmentation() {
    assert_code_fires("augment typedef T = int;\n", "typedef_augmentation");
}

/// A `new`-introducer constructor whose name is `new` again (official
/// `parseMethod`'s new-head name checks; oracle probed: the same code at
/// the name, "Constructors declared with the 'new' keyword can't be named
/// 'new'").
#[test]
fn local_pin_new_constructor_new_name() {
    assert_code_fires("class C { new new() {} }\n", "new_constructor_new_name");
}

/// A `new`-introducer constructor with a qualified name (official
/// `parseMethod`'s new-head qualified-name check; oracle probed: the same
/// code, "Constructors declared with the 'new' keyword can't have
/// qualified names").
#[test]
fn local_pin_new_constructor_qualified_name() {
    assert_code_fires("class C { new a.b() {} }\n", "new_constructor_qualified_name");
}
