//! #3900 pins — the `resolution.rs`/`augmentations.rs` half of the W21
//! wave: (a) the augmentation type-parameter bound pair reads a top-level
//! function's own bound column, (b) the no-context collection literal's
//! leaves type a completed send and join distinct class faces at their
//! nearest shared supertype, (c) a top-level `var` declarator binds its own
//! literal initializer face.
//!
//! Upstream references (Dart SDK 3.13.7, the oracle's version):
//!
//! * `pkg/analyzer/lib/src/generated/error_verifier.dart`
//!   `_checkForAugmentationTypeParameters` (:3192-3250): the pair reports
//!   when `firstTypeParameter.element.bound == null || boundNode.typeOrThrow
//!   != firstBound` — the intro's OWN bound is the comparison's left side, so
//!   an equal bound on both sides of the chain is silent.
//! * `pkg/analyzer/lib/src/generated/least_upper_bound.dart`: the join of
//!   two class faces is the nearest member of the intersection of their
//!   superinterface sets.
//! * `pkg/analyzer/lib/src/generated/typed_literal_resolver.dart`: an
//!   untyped collection literal with no context types itself from its
//!   elements' static types, and an instance creation's static type is the
//!   created class.
//!
//! Routed, not landed here — the owning files are another lane's or landed:
//!
//! * The mixin-application family (#3888's 112 census rows:
//!   `mixin_application_not_implemented_interface` 22M/20E,
//!   `private_collision_in_mixin_application` 23M,
//!   `mixin_application_no_concrete_super_invoked_member` 20M,
//!   `mixin_application_concrete_super_invoked_member_type` 4M,
//!   `const_constructor_with_mixin_with_field` 13M,
//!   `enum_mixin_with_instance_variable` 10M) is emitted upstream by
//!   `error_verifier.dart`'s `_checkForMixinSuperclassConstraints`
//!   (:6112-6150), `_checkForMixinSuperInvokedMembers` (:6155-6205),
//!   `_checkForMixinWithConflictingPrivateMember` (:6206-6250),
//!   `_checkForConstConstructorWithMixinWithField` (:4039-4100), and
//!   `inheritance_override.dart`:890 (the enum instance-variable face). Its
//!   dartq homes are `enum_verifier.rs` (`verify_mixin_application`),
//!   `overrides.rs` (the inheritance walk), and `collector.rs` — none of
//!   them this lane's file. Only the 20 existing
//!   `mixin_application_not_implemented_interface` rows are emitted today
//!   (`enum_verifier.rs`); the other five codes have no emitter at all.
//! * The `invalid_assignment` var-bind seam (8 rows, R's #3879 routing) is
//!   the `engine_core.rs`/`listener.rs` pair: the write's static type read
//!   drops the dynamic-bearing written group, not a `resolution.rs` face.

use super::test_rig::{CORE_PRELUDE, report_ops_experiments_with};

/// The fixture's rows of one code, in source order.
fn rows_with(source: &str, code: &str, experiments: &[dartq_language::Feature]) -> Vec<String> {
    let mut rows: Vec<(u32, String)> = report_ops_experiments_with(source, &[], None, experiments)
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.message.to_string()))
        .collect();
    rows.sort_by_key(|(offset, _)| *offset);
    rows.into_iter().map(|(_, message)| message).collect()
}

/// The same rows over a fixture that names the member-bearing core classes
/// (the prelude shifts the fixture's own line numbers by one, irrelevant to
/// these assertions).
fn core_rows_with(source: &str, code: &str) -> Vec<String> {
    rows_with(&format!("{CORE_PRELUDE}{source}"), code, &[])
}

// —— (a) the augmentation type-parameter bound pair ————————————————

const BOUND_ROW: &str = "The augmentation type parameter must have the same bound as the corresponding type parameter \
     of the declaration.";

/// The census face (co19 `augmenting_functions_A04_t0{1,7,8,9}`,
/// `_A04_t1{8,9}`, `type_annotation_inheritance_A04_t07`, and the two
/// analyzer `AugmentationTypeParameterBoundTest` alias/prefix cases — the 13
/// dartq-extra rows): a top-level function whose augment restates the
/// introductory bound. Both sides answer the same canonical type, so the
/// pair stays silent. Pre-fix the intro face answered an EMPTY bound slice,
/// so every written bound read `dynamic` and every such pair minted a row.
#[test]
fn augment_top_level_function_equal_bound_stays_silent() {
    let source = "\
class Box {}
void foo<T extends Box>(T t) {}
augment void foo<T extends Box>(T t);
";
    let rows = rows_with(
        source,
        "augmentation_type_parameter_bound",
        &[dartq_language::Feature::Augmentations],
    );
    assert!(
        rows.is_empty(),
        "equal bounds must stay silent, got {rows:?}"
    );
}

/// The pair compares canonical types, not spellings: a typedef alias on the
/// augmenting side names the same type as the declaration's plain head (the
/// analyzer `AugmentationTypeParameterBoundTest` alias case).
#[test]
fn augment_top_level_function_alias_bound_stays_silent() {
    let source = "\
class Box {}
typedef Alias = Box;
void foo<T extends Box>(T t) {}
augment void foo<T extends Alias>(T t);
";
    let rows = rows_with(
        source,
        "augmentation_type_parameter_bound",
        &[dartq_language::Feature::Augmentations],
    );
    assert!(
        rows.is_empty(),
        "an alias-spelled bound is the same type, got {rows:?}"
    );
}

/// The reporting direction over the same face — proof the fix reads the
/// intro's bound instead of suppressing the check: two distinct class bounds
/// report exactly once.
#[test]
fn augment_top_level_function_different_bound_reports() {
    let source = "\
class Box {}
class Crate {}
void foo<T extends Box>(T t) {}
augment void foo<T extends Crate>(T t);
";
    let rows = rows_with(
        source,
        "augmentation_type_parameter_bound",
        &[dartq_language::Feature::Augmentations],
    );
    assert_eq!(rows, vec![BOUND_ROW.to_string()]);
}

/// The unbounded-intro direction (upstream `firstBound == null` → report):
/// an introduction without a bound never passes a written one, so the row
/// stays. This guards against turning the pair into a comparison that
/// silently accepts a newly written bound.
#[test]
fn augment_top_level_function_unbounded_intro_written_bound_reports() {
    let source = "\
class Box {}
void foo<T>(T t) {}
augment void foo<T extends Box>(T t);
";
    let rows = rows_with(
        source,
        "augmentation_type_parameter_bound",
        &[dartq_language::Feature::Augmentations],
    );
    assert_eq!(rows, vec![BOUND_ROW.to_string()]);
}

// —— (b) the no-context collection literal's leaves ————————————————

/// Two instance creations as map values: each leaf's row carries `Unknown`
/// at push time (a completed send), so the leaf must read the callee's type;
/// the two distinct class faces then join at their nearest shared supertype
/// rather than collapsing the whole argument to `dynamic` (the census C2
/// shape: `{1: 'x', 3.0: new RegExp('.')}` binds `Map<num, Pattern>`). The
/// verdict's spelled type pins both halves at once.
#[test]
#[ignore = "held: the collection literal's creation-typed leaf faces never reach the completed send — `{1: A(), 2: B()}` should type each leaf via the created class and join at the shared `Shared` superinterface (upstream least_upper_bound.dart + typed_literal_resolver.dart); re-verified 2026-11-05 vs oracle 3.13.7: still fails — zero `invalid_assignment` rows where the oracle reports `'Map<int, Shared>'->'Map<int, String>'`; owner the collection-literal leaf faces' send-typed read"]
fn map_literal_creation_leaves_join_at_shared_supertype() {
    let source = "\
class Shared {}
class A implements Shared {}
class B implements Shared {}
void f() {
  var m = {1: A(), 2: B()};
  Map<int, String> target = m;
}
";
    let rows = core_rows_with(source, "invalid_assignment");
    assert_eq!(rows.len(), 1, "one verdict, got {rows:?}");
    assert!(
        rows[0].contains("Map<int, Shared>"),
        "the leaves must join at their shared supertype, got {rows:?}"
    );
}

/// The same shapes with no shared supertype keep the `dynamic` rung (the
/// provability-equal under-report this fold has always held): the verdict
/// still fires, spelling `dynamic`.
#[test]
fn map_literal_unrelated_leaves_keep_dynamic() {
    let source = "\
class A {}
class B {}
void f() {
  var m = {1: A(), 2: B()};
  Map<int, String> target = m;
}
";
    let rows = core_rows_with(source, "invalid_assignment");
    assert_eq!(rows.len(), 1, "one verdict, got {rows:?}");
    assert!(
        rows[0].contains("Map<int, dynamic>"),
        "no shared class keeps dynamic, got {rows:?}"
    );
}

// —— (c) a top-level variable's literal initializer ————————————————

/// A top-level `var` declarator binds its own literal initializer face: the
/// window opens at the declaration site (the local walk names no frame
/// binding for a top-level variable), so the same verdict the function-scope
/// variant reports fires here instead of staying silent (the census C2
/// missing row).
#[test]
#[ignore = "held: a top-level `var` declarator never binds its own literal initializer face — `var m = {1: A(), 2: B()}` should type `m` as `Map<int, Shared>` and report on the `Map<int, String>` read; re-verified 2026-11-05 vs oracle 3.13.7: still fails — zero `invalid_assignment` rows where the oracle reports `'Map<int, Shared>'->'Map<int, String>'`; owner the top-level var declarator's literal initializer binding"]
fn top_level_var_collection_literal_binds_its_face() {
    let source = "\
class Shared {}
class A implements Shared {}
class B implements Shared {}
var m = {1: A(), 2: B()};
void f() {
  Map<int, String> target = m;
}
";
    let rows = core_rows_with(source, "invalid_assignment");
    assert_eq!(rows.len(), 1, "one verdict, got {rows:?}");
    assert!(
        rows[0].contains("Map<int, Shared>"),
        "the top-level var binds its literal face, got {rows:?}"
    );
}
