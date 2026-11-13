//! The #3153 pins: the non-invocation `type_argument_not_matching_bounds`
//! faces (`_checkForTypeArgumentNotMatchingBounds`,
//! type_arguments_verifier.dart:342-540, run at every `NamedType`,
//! :243-247). Pins cover the four gates this cluster's divergences rode:
//!
//! * the typedef-RHS double check: a `typedef A = C<String>` right-hand
//!   `NamedType` reports exactly once — the written-site capture must not
//!   open while the alias capture owns the same trees
//!   (`GenericTypeAlias` is in `_shouldAllowSuperBoundedTypes`' deny
//!   list, :742, so the row is plain);
//! * the named-extension `on` face: `extension E on C<String>` claims its
//!   `on` type at the header close — `ExtensionOnClause` is NOT denied
//!   (:739-748), so the row rides the super pass like a use site;
//! * the bound-annotation faces' function/record columns: a bound's
//!   function-type columns (`T extends void Function(C<String>)`) and a
//!   signature's own parameter bounds
//!   (`void Function<U extends C<String>>`) are `NamedType`s upstream
//!   visits independently;
//! * the executable-window face: `void f<T extends C<String>>() {}` — a
//!   type-parameter bound no container/alias adopted still reports
//!   through its own window's bound trees.

use super::*;

fn range(offset: u32, length: u32) -> SourceRange {
    SourceRange { offset, length }
}

/// `class C<T extends num> {}` — the bound container every check reads.
fn declare_num_container(bounds: &mut Bounds<'_>, strings: &StringTable) -> StringId {
    let c = strings.intern("C");
    let t = strings.intern("T");
    let num = strings.intern("num");
    bounds.begin_type_variables();
    bounds.leaf(num, false, range(13, 3));
    bounds.end_type_variable(t, true);
    bounds.end_type_variables();
    bounds.note_container(Some(c), 6, 21, false, false);
    c
}

/// A typedef RHS `NamedType` reports exactly once: `typedef A = C<String>`
/// emits a single `String`-at-argument row — before the gate, the site
/// capture opened on the RHS's group and `check_site` emitted the same
/// row a second time (the duplicate-extra family).
#[test]
fn ticket_3153_alias_named_rhs_reports_once() {
    let strings = StringTable::new();
    let mut bounds = Bounds::new();
    let c = declare_num_container(&mut bounds, &strings);
    let alias = strings.intern("A");
    let string = strings.intern("String");
    // `typedef A = C<String>;` — the RHS's group fires inside the open
    // alias capture; the site arm must stay closed (no `Site` pending).
    bounds.begin_alias(alias);
    bounds.begin_group(Some((c, range(13, 1))), true);
    bounds.leaf(string, false, range(15, 6));
    assert!(
        bounds.end_group(range(21, 1)).is_none(),
        "the alias capture owns the RHS — no site tree closes"
    );
    bounds.alias_begin_group(Some((c, range(13, 1))));
    bounds.alias_leaf(string, false, range(15, 6));
    bounds.alias_end_group(range(21, 1));
    bounds.end_alias();
    let issues = bounds.check_alias_rhs_sites(&strings);
    assert_eq!(issues.len(), 1, "expected exactly one issue");
    assert_eq!(issues[0].argument, "String");
    assert_eq!(issues[0].bound, "num");
}

/// The same gate on the function-shaped RHS: `typedef F = C<String>
/// Function()` — the `C<String>` return reports once through the alias
/// face (its parent is the function-type node, so the super pass
/// survives — the row carries the inverted-type context line).
#[test]
fn ticket_3153_alias_function_rhs_return_reports_once() {
    let strings = StringTable::new();
    let mut bounds = Bounds::new();
    let c = declare_num_container(&mut bounds, &strings);
    let f = strings.intern("F");
    let string = strings.intern("String");
    bounds.begin_alias(f);
    bounds.alias_formal_boundary();
    // `C<String>` return type, inside the open alias capture.
    bounds.begin_group(Some((c, range(14, 1))), true);
    bounds.leaf(string, false, range(16, 6));
    assert!(
        bounds.end_group(range(22, 1)).is_none(),
        "the alias capture owns the RHS"
    );
    bounds.alias_begin_group(Some((c, range(14, 1))));
    bounds.alias_leaf(string, false, range(16, 6));
    bounds.alias_end_group(range(22, 1));
    bounds.alias_formals_region_open();
    bounds.alias_formals_closed();
    bounds.alias_rhs_question(false);
    bounds.end_alias();
    let issues = bounds.check_alias_rhs_sites(&strings);
    assert_eq!(issues.len(), 1, "expected exactly one issue");
    assert_eq!(issues[0].argument, "String");
}

/// A named plain extension's `on` type claims at the header close and
/// keeps the super pass (`ExtensionOnClause` is not in the deny list,
/// type_arguments_verifier.dart:739-748): `extension E on C<String>`
/// reports the `String` argument, while `extension E on C<dynamic>`
/// inverts clean (`C<Never>` satisfies `num`) and stays silent.
#[test]
fn ticket_3153_extension_on_clause_checks_with_super_pass() {
    let strings = StringTable::new();
    let mut bounds = Bounds::new();
    let c = declare_num_container(&mut bounds, &strings);
    let e = strings.intern("E");
    let string = strings.intern("String");
    let dynamic = strings.intern("dynamic");
    bounds.open_header(e, false, false);
    bounds.begin_group(Some((c, range(16, 1))), true);
    bounds.leaf(string, false, range(18, 6));
    bounds.end_group(range(24, 1));
    bounds.finish_header();
    bounds.open_header(e, false, false);
    bounds.begin_group(Some((c, range(40, 1))), true);
    bounds.leaf(dynamic, false, range(42, 7));
    bounds.end_group(range(49, 1));
    bounds.finish_header();
    let issues = bounds.check_clause_sites(&strings);
    assert_eq!(issues.len(), 1, "expected exactly one issue");
    assert_eq!(issues[0].argument, "String");
    assert_eq!(issues[0].bound, "num");
    // The super pass ran: the context face carries the inverted type —
    // `replaceTopAndBottom(C<String>)` flips nothing (`String` is neither
    // top nor bottom), so the instantiation the failing pass checked is
    // `C<String>` itself ("The inverted type 'C<String>' is also not
    // regular-bounded…", type_arguments_verifier.dart:421-427; the second
    // fixture's `C<dynamic>` inverts to `C<Never>`, satisfies `num`, and
    // stays silent).
    assert_eq!(issues[0].inverted_type.as_deref(), Some("C<String>"));
}

/// The bound-annotation face's function columns: `class D<X extends void
/// Function(C<String>)>` — the `C<String>` inside the function bound is a
/// `NamedType` the verifier visits independently (the oracle row anchors
/// at `String`).
#[test]
fn ticket_3153_function_bound_columns_report_nested_named_types() {
    let strings = StringTable::new();
    let mut bounds = Bounds::new();
    let c = declare_num_container(&mut bounds, &strings);
    let d = strings.intern("D");
    let x = strings.intern("X");
    let void_id = strings.intern("void");
    let string = strings.intern("String");
    bounds.begin_type_variables();
    // `void Function(C<String>)` — ret `void`, one `C<String>` formal.
    bounds.begin_function();
    bounds.leaf(void_id, false, range(20, 4));
    bounds.function_formals_begin();
    bounds.begin_group(Some((c, range(34, 1))), false);
    bounds.leaf(string, false, range(36, 6));
    bounds.end_group(range(42, 1));
    bounds.function_formal_end(false, None);
    bounds.end_function(false, range(43, 1));
    bounds.end_type_variable(x, true);
    bounds.end_type_variables();
    bounds.note_container(Some(d), 6, 46, false, false);
    let issues = bounds.check_bound_annotation_sites(&strings);
    assert_eq!(issues.len(), 1, "expected exactly one issue");
    assert_eq!(issues[0].argument, "String");
    assert_eq!(issues[0].bound, "num");
    assert_eq!(issues[0].range.offset, 36);
}

/// The signature's own parameter bounds inside a function-typed bound:
/// `class D<X extends void Function<U extends C<String>>()>` — the
/// `C<String>` bound of the signature's `U` reports at `String` (the
/// `TypeParameter` parent keeps the super pass; the signature's lazy ids
/// read `U`'s bound through the window map).
#[test]
fn ticket_3153_signature_param_bounds_report() {
    let strings = StringTable::new();
    let mut bounds = Bounds::new();
    let c = declare_num_container(&mut bounds, &strings);
    let d = strings.intern("D");
    let x = strings.intern("X");
    let u = strings.intern("U");
    let void_id = strings.intern("void");
    let string = strings.intern("String");
    bounds.begin_type_variables();
    bounds.begin_function();
    bounds.leaf(void_id, false, range(20, 4));
    // The signature's own `<U extends C<String>>` group.
    bounds.begin_type_variables();
    bounds.begin_group(Some((c, range(41, 1))), false);
    bounds.leaf(string, false, range(43, 6));
    bounds.end_group(range(49, 1));
    bounds.end_type_variable(u, true);
    bounds.end_type_variables();
    bounds.function_formals_begin();
    bounds.end_function(false, range(51, 1));
    bounds.end_type_variable(x, true);
    bounds.end_type_variables();
    bounds.note_container(Some(d), 6, 54, false, false);
    let issues = bounds.check_bound_annotation_sites(&strings);
    assert_eq!(issues.len(), 1, "expected exactly one issue");
    assert_eq!(issues[0].argument, "String");
    assert_eq!(issues[0].range.offset, 43);
}

/// The executable-window face: `void f<T extends C<String>>() {}` — no
/// container or alias adopts the `<T>` window, yet `checkNamedType` runs
/// at the `C<String>` bound upstream, so the unclaimed window still
/// reports through its own bound trees.
#[test]
fn ticket_3153_executable_window_bounds_report() {
    let strings = StringTable::new();
    let mut bounds = Bounds::new();
    let c = declare_num_container(&mut bounds, &strings);
    let t = strings.intern("T");
    let string = strings.intern("String");
    bounds.begin_type_variables();
    bounds.begin_group(Some((c, range(18, 1))), false);
    bounds.leaf(string, false, range(20, 6));
    bounds.end_group(range(26, 1));
    bounds.end_type_variable(t, true);
    bounds.end_type_variables();
    let issues = bounds.check_bound_annotation_sites(&strings);
    assert_eq!(issues.len(), 1, "expected exactly one issue");
    assert_eq!(issues[0].argument, "String");
    assert_eq!(issues[0].bound, "num");
    assert_eq!(issues[0].range.offset, 20);
}

/// The old-style `typedef void F(C<String> x)` whole signature: no
/// `Function(` head event fires, so `fn_seen` latches at the first
/// completed formal and the return type peels off the sink there. Both
/// the return `void` (inert) and the formal `C<String>` report — oracle
/// `2|18`/`3|11` on `s48_old_typedef.dart`.
#[test]
fn ticket_3153_old_typedef_formals_report() {
    let strings = StringTable::new();
    let mut bounds = Bounds::new();
    let c = declare_num_container(&mut bounds, &strings);
    let f = strings.intern("F");
    let void_id = strings.intern("void");
    let x = strings.intern("x");
    let string = strings.intern("String");
    bounds.begin_alias(f);
    // `void` return, then the `C<String>` formal — no `Function(` head.
    bounds.alias_leaf(void_id, false, range(8, 4));
    bounds.alias_begin_group(Some((c, range(14, 1))));
    bounds.alias_leaf(string, false, range(16, 6));
    bounds.alias_end_group(range(22, 1));
    bounds.alias_formal_end(x, 24, true);
    bounds.alias_formals_closed();
    bounds.end_alias();
    let issues = bounds.check_alias_rhs_sites(&strings);
    assert_eq!(issues.len(), 1, "expected exactly one issue");
    assert_eq!(issues[0].argument, "String");
    assert_eq!(issues[0].bound, "num");
    assert_eq!(issues[0].range.offset, 16);
}

/// The self-referential executable bound reads `bound(X) <: B` through
/// the window's written trees: `void f<T extends C<T>>() {}` — `T`'s own
/// bound satisfies `C`'s, so the row stays silent (the lazy scope's
/// `Object?` reading would false-positive here).
#[test]
fn ticket_3153_executable_window_self_referential_bound_stays_silent() {
    let strings = StringTable::new();
    let mut bounds = Bounds::new();
    // `class C<T extends C<T>> {}`
    let c = strings.intern("C");
    let t = strings.intern("T");
    bounds.begin_type_variables();
    bounds.begin_group(Some((c, range(18, 1))), false);
    bounds.leaf(t, false, range(20, 1));
    bounds.end_group(range(21, 1));
    bounds.end_type_variable(t, true);
    bounds.end_type_variables();
    bounds.note_container(Some(c), 6, 24, false, false);
    // `void f<T extends C<T>>() {}`
    bounds.begin_type_variables();
    bounds.begin_group(Some((c, range(40, 1))), false);
    bounds.leaf(t, false, range(42, 1));
    bounds.end_group(range(43, 1));
    bounds.end_type_variable(t, true);
    bounds.end_type_variables();
    let issues = bounds.check_bound_annotation_sites(&strings);
    assert!(issues.is_empty(), "expected no issues");
}
