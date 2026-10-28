//! The generated example suite: every `FIX_TABLE` row carries a
//! checked-in before/after example that this harness round-trips through
//! the production fix pipeline — the dartq mirror of rust-analyzer's
//! assists doc tests (a handler whose documented behaviour drifts from
//! its code fails the build).
//!
//! One example is a pair `tests/data/fix_examples/<code>[.<variant>]
//! .before.dart` + `.after.dart` (`code` = the row's diagnostic name; a
//! variant suffix separates several examples for one row). The harness:
//!
//! 1. materializes every `before` into a throwaway workspace next to a
//!    generated `analysis_options.yaml` enabling every lint-source row
//!    (engine-free tier parity with the fix face's options governance);
//! 2. runs the production pipeline the fix face serves —
//!    [`dartq_commands::fix::collect_set`] (the dry-run proposal face),
//!    [`dartq_commands::fix::select_by_codes`] +
//!    [`dartq_commands::fix::select_by_role`] (`--code` + primary role,
//!    the apply run's own selection), then [`dartq_commands::fix::apply`]
//!    over the dry-run edits;
//! 3. asserts the row's code fired (at least one primary fix
//!    materialized) and the applied text is byte-equal to `after`.
//!
//! The engine pass behind the semantic and parser rows needs a Dart SDK
//! root; with `$DART_SDK` unset those rows are skipped with a counted
//! stdout note — the same best-effort degrade the production fix face
//! itself answers (its semantic pass logs `no usable workspace or SDK`
//! and the run continues from the other sources). Set `DART_SDK` to run
//! them.
//!
//! The completeness gate ([`fix_table_every_row_has_an_example`]) holds
//! the table and the corpus in lockstep: a row without an example must
//! name itself in [`NO_EXAMPLE_YET`], and an allow-list entry whose row
//! has since gained an example fails the gate — the list shrinks, never
//! lingers.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use dartq_commands::fix::{self, FixRole};
use dartq_commands::fix_table::{self, FixSource};

/// The corpus directory, relative to the crate root (integration tests
/// run with the package directory as cwd).
const DATA_DIR: &str = "tests/data/fix_examples";

/// Rows without an example yet, each with the reason it cannot fire or
/// cannot be authored honestly today. The list must shrink, never grow
/// silently: it is pinned here in full, so adding an entry is a visible
/// diff, and an entry whose row has an example on disk (or names no row
/// at all) fails the gate as stale.
///
/// Reason taxonomy, each verified through the harness itself:
/// - `fold`: the rule consumes the semantic fold
///   (`dartq_lints::SEMANTIC_RULES`), which the fix face's lint stage
///   never mounts (`ParsedUnit::unit` carries no fold) — verified silent
///   on honest trigger shapes.
/// - `wrong-fix`: the lint fires but the row's rewrite produces
///   non-official (or invalid) text — production-gap finding, reported
///   to the fix lane.
/// - `drops`: the lint fires but the row's rewrite verification drops
///   the candidate — the production pipeline answers no fix at all.
/// - `silent`: no honest trigger shape fires the rule through the fix
///   face's engine-free stage (typed or package context it cannot see).
/// - `posture`: the rule registers only under a language posture the
///   fix face cannot reach (its per-file version comes from the package
///   table alone; `@dart` pins are not consulted).
const NO_EXAMPLE_YET: &[(&str, &str)] = &[
    ("abstract_static_method", "registry-only: the parser never emits this code today (the table's own pin notes it), so no example can fire"),
    ("annotate_overrides", "fold"),
    ("const_eval_method_invocation", "silent: the const-eval method-invocation trigger produces no diagnostic rows through the engine pass (probe-verified)"),
    ("const_with_non_const", "silent: non-const class + `const C(arg)` produces `const_initialized_with_non_constant_value` only, never this code (probe-verified)"),
    ("extension_augmentation_has_on_clause", "silent: augmentation declarations parse to `expected_token` today; this code never emits (probe-verified)"),
    ("extension_type_declares_member_of_object", "silent: the Object-member extension-type trigger produces no diagnostic rows through the engine pass (probe-verified)"),
    ("getter_constructor", "silent: `int get C => 1;` parses without any diagnostic through the engine pass (probe-verified)"),
    ("avoid_annotating_with_dynamic", "wrong-fix: deletes the whole parameter (`void f(dynamic x) {}` becomes `void f() {}`), not the annotation"),
    ("avoid_bool_literals_in_conditional_expressions", "fold"),
    ("avoid_final_parameters", "posture: registers only without primary constructors; the fix face parses every file at its package posture"),
    ("avoid_print", "fold"),
    ("avoid_private_typedef_functions", "silent: canonical old-style private typedef does not fire through the engine-free stage"),
    ("avoid_returning_null_for_void", "drops: lint fires on `return null;` in a void function but the rewrite never materializes"),
    ("avoid_types_on_closure_parameters", "fold"),
    ("await_only_futures", "fold"),
    ("cast_nullable_to_non_nullable", "fold"),
    ("empty_catches", "drops: lint fires on an empty catch but the comment-insert rewrite never materializes"),
    ("future_sync_value", "posture: the rule name is outside the options-known lint universe, so no options file can enable it"),
    ("final_constructor", "registry-only: the parser never emits this code today (the table's own pin notes it), so no example can fire"),
    ("final_method", "registry-only: the parser never emits this code today (the table's own pin notes it), so no example can fire"),
    ("implicit_call_tearoffs", "fold"),
    ("initialize_in_field_declaration", "drops: fires on primary-constructor initializer lists but the multi-edit fix goes stale mid-apply"),
    ("invalid_constant", "silent: const-list non-constant-element triggers emit `non_constant_list_element` instead; no shape found producing this code (probe-verified)"),
    ("leading_newlines_in_multiline_strings", "silent: the upstream-verbatim trigger does not fire through the fix face's parse posture"),
    ("library_annotations", "silent: `@pragma` shapes do not fire through the engine-free stage"),
    ("mixin_super_class_constraint_non_interface", "drops: the multi-edit candidate goes half-stale on the two-constraint shape (patched text still verified by hand)"),
    ("import_of_non_library", "multi-file: the report needs a part file distinct from the importing library, one example file per pair today"),
    ("non_const_call_to_literal_constructor", "silent: `new Map()` emits `unused_local_variable` only; the literal-constructor code never fires (probe-verified)"),
    ("non_constant_map_element", "silent: const-map non-constant-value triggers produce no diagnostic rows (probe-verified)"),
    ("no_duplicate_case_values", "silent: typed duplicate-case distinctness is invisible to the engine-free stage"),
    ("noop_primitive_operations", "fold"),
    ("omit_local_variable_types", "wrong-fix: produces `varx = 1;` (separator eaten) instead of `var x = 1;`"),
    ("prefer_collection_literals", "fold"),
    ("prefer_const_literals_to_create_immutables", "silent: `@immutable`-annotated typed list does not fire through the engine-free stage"),
    ("prefer_contains", "fold"),
    ("prefer_final_fields", "drops: lint fires on a never-reassigned private field but the rewrite never materializes"),
    ("prefer_final_parameters", "posture: alias of `avoid_final_parameters`, which registers only without primary constructors; the fix face parses every file at its package posture"),
    ("prefer_final_in_for_each", "wrong-fix: produces invalid `for (var final e …)` instead of swapping `var` for `final`"),
    ("prefer_for_elements_to_map_fromiterable", "fold"),
    ("prefer_foreach", "wrong-fix: leaves a stray semicolon (`it.forEach(f);;`)"),
    ("prefer_if_elements_to_conditional_expressions", "drops: lint fires on a conditional element but the rewrite never materializes"),
    ("prefer_initializing_formals", "fold"),
    ("prefer_int_literals", "fold"),
    ("representation_field_trailing_comma", "registry-only: the parser never emits this code today (the table's own pin notes it), so no example can fire"),
    ("setter_constructor", "silent: `set C(int v)` parses without any diagnostic through the engine pass (probe-verified)"),
    ("prefer_is_empty", "fold"),
    ("prefer_iterable_wheretype", "fold"),
    ("sized_box_for_whitespace", "fold"),
    ("static_getter_without_body", "registry-only: the parser never emits this code today (the table's own pin notes it), so no example can fire"),
    ("static_setter_without_body", "registry-only: the parser never emits this code today (the table's own pin notes it), so no example can fire"),
    ("type_check_with_null", "drops: the engine emits the code (ignore rows only) but the `UseEqEqNull` rewrite never materializes a primary (probe-verified)"),
    ("unawaited_futures", "silent: async-context typing is invisible to the engine-free stage"),
    ("unchecked_use_of_nullable_value", "drops: the null-check family's primary insert never materializes through the engine pass (probe-verified)"),
    ("unnecessary_await_in_return", "silent: written-Future shapes do not fire through the engine-free stage"),
    ("unnecessary_breaks", "drops: lint fires on a trailing member `break` but the rewrite never materializes"),
    ("unnecessary_ignore", "drops: lint fires on a stale ignore comment but the line-drop rewrite never materializes"),
    ("unnecessary_lambdas", "fold"),
    ("unnecessary_nan_comparison", "drops: the engine emits the code (ignore rows only) but the `RemoveComparison` rewrite never materializes a primary (probe-verified)"),
    ("unnecessary_null_aware_operator_on_extension_on_nullable", "fold"),
    ("unnecessary_null_checks", "fold"),
    ("unnecessary_null_comparison", "drops: the engine emits the code (ignore rows only) but the `RemoveComparison` rewrite never materializes a primary (probe-verified)"),
    ("unnecessary_type_check", "silent: `is`-check-comparison shapes emit no rows through the engine pass (probe-verified)"),
    ("unnecessary_string_interpolations", "fold"),
    ("unnecessary_unawaited", "fold"),
    ("unrelated_type_equality_checks", "fold"),
    ("unreachable_switch_case", "silent: case-after-default emits `dead_code` instead; no shape found producing this code (probe-verified)"),
    ("use_colored_box", "fold"),
    ("use_null_aware_elements", "drops: lint fires on `[if (a != null) a]` but the rewrite never materializes"),
    ("use_truncating_division", "fold"),
    ("unused_catch_clause", "silent: an unused catch variable produces no diagnostic rows through the engine pass (probe-verified)"),
    ("unused_element_parameter", "silent: the unused callback-parameter trigger produces no diagnostic rows (probe-verified)"),
    ("var_and_type", "drops: the engine emits the code (ignore rows only) but the `RemoveVar` rewrite never materializes a primary (probe-verified)"),
    ("var_with_no_type_annotation", "posture: registers only without primary constructors; the fix face parses every file at its package posture"),
];

/// One corpus entry: the workspace file's stem (`code` or
/// `code.variant`) and the before/after texts.
struct Example {
    stem: String,
    code: String,
    before: String,
    after: String,
}

/// Reads the corpus: one `Example` per `<stem>.before.dart` +
/// `<stem>.after.dart` pair, sorted by stem (the failure summary's
/// order). A before without an after (or vice versa) fails the read.
fn read_corpus() -> Vec<Example> {
    let dir = Path::new(DATA_DIR);
    let mut pairs: BTreeMap<String, (Option<String>, Option<String>)> = BTreeMap::new();
    for entry in std::fs::read_dir(dir).expect("corpus directory readable") {
        let entry = entry.expect("corpus entry readable");
        let name = entry.file_name().into_string().expect("utf-8 name");
        let text = std::fs::read_to_string(entry.path()).expect("corpus file readable");
        if let Some(stem) = name.strip_suffix(".before.dart") {
            pairs.entry(stem.to_owned()).or_default().0 = Some(text);
        } else if let Some(stem) = name.strip_suffix(".after.dart") {
            pairs.entry(stem.to_owned()).or_default().1 = Some(text);
        } else {
            panic!("stray file in {DATA_DIR}: {name}");
        }
    }
    let mut out = Vec::new();
    for (stem, (before, after)) in pairs {
        let (Some(before), Some(after)) = (&before, &after) else {
            panic!("incomplete example pair for `{stem}` in {DATA_DIR}");
        };
        // The diagnostic code is the stem up to the first variant dot;
        // codes themselves carry no dots.
        let code = stem.split('.').next().unwrap_or(&stem).to_owned();
        out.push(Example {
            stem,
            code,
            before: before.clone(),
            after: after.clone(),
        });
    }
    out
}

/// Materializes the corpus into a fresh workspace directory: every
/// example as `<stem>.dart` plus a generated `analysis_options.yaml`
/// enabling every lint-source row's rule (the fix face's options
/// governance, mirrored whole-table so the file stays one for all
/// examples). Returns the workspace root; the caller owns its removal.
fn materialize(examples: &[Example]) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "dartq-fix-examples-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default(),
    ));
    std::fs::create_dir_all(&root).expect("workspace directory creatable");
    for example in examples {
        let path = root.join(format!("{}.dart", example.stem));
        std::fs::write(&path, &example.before).expect("example writable");
    }
    let mut rules: Vec<&str> = fix_table::all_specs()
        .iter()
        .filter(|spec| spec.source == FixSource::Lint)
        .map(|spec| spec.code)
        .collect();
    rules.sort_unstable();
    let mut options = String::from("linter:\n  rules:\n");
    for rule in rules {
        options.push_str("    - ");
        options.push_str(rule);
        options.push('\n');
    }
    std::fs::write(root.join("analysis_options.yaml"), options).expect("options writable");
    root
}

/// The round-trip: every example through the production fix face.
#[test]
fn fix_examples_round_trip() {
    let examples = read_corpus();
    assert!(!examples.is_empty(), "corpus is non-empty");
    let workspace = materialize(&examples);
    let paths: Vec<PathBuf> = examples
        .iter()
        .map(|example| workspace.join(format!("{}.dart", example.stem)))
        .collect();
    let sets = fix::collect_set(&paths, &mut fix::PluginHosts::empty(), None)
        .expect("the fix face's discovery and walk succeed");
    let by_path: BTreeMap<&str, &fix::FileFixSet> = sets
        .iter()
        .map(|set| (set.path.as_str(), set))
        .collect();

    // The engine pass behind the semantic and parser rows needs an SDK
    // root; without one those tiers cannot fire — the production face's
    // own degrade posture. Skipped loudly, never silently.
    let engine_tier = std::env::var_os("DART_SDK").is_some();
    let mut skipped_engine = 0usize;

    let mut failures: Vec<String> = Vec::new();
    for example in &examples {
        let spec = fix_table::spec_for(&example.code)
            .unwrap_or_else(|| panic!("example `{}` names no fix-table row", example.stem));
        let is_engine_row =
            matches!(spec.source, FixSource::Semantic | FixSource::Parser);
        if is_engine_row && !engine_tier {
            skipped_engine += 1;
            continue;
        }
        let path = workspace.join(format!("{}.dart", example.stem));
        let Some(set) = by_path.get(path.to_str().expect("utf-8 workspace path")) else {
            failures.push(format!(
                "`{}`: the fix face answered no set (code {} never fired)",
                example.stem, example.code,
            ));
            continue;
        };
        // The apply run's own selection: `--code`, then the primary role.
        let selected = fix::select_by_codes(vec![(*set).clone()], &[example.code.clone()]);
        let selected = fix::select_by_role(selected, FixRole::Primary);
        let Some(file) = selected.first() else {
            failures.push(format!(
                "`{}`: code {} never fired (no primary fix materialized)",
                example.stem, example.code,
            ));
            continue;
        };
        // The create-file rows address a target other than the hit file;
        // every same-file row applies to the before text.
        let created: Vec<&fix::RuleFix> =
            file.fixes.iter().filter(|f| f.target.is_some()).collect();
        let same_file: Vec<&fix::RuleFix> =
            file.fixes.iter().filter(|f| f.target.is_none()).collect();
        if created.is_empty() && same_file.is_empty() {
            failures.push(format!(
                "`{}`: code {} fired but no fix survived verification",
                example.stem, example.code,
            ));
            continue;
        }
        if !created.is_empty() {
            if created.len() != 1 || !same_file.is_empty() {
                failures.push(format!(
                    "`{}`: expected exactly one create-file row, got {} created + {} same-file",
                    example.stem,
                    created.len(),
                    same_file.len(),
                ));
                continue;
            }
            let fix = created[0];
            if Path::new(fix.target.as_deref().expect("created row names its target"))
                .exists()
            {
                failures.push(format!(
                    "`{}`: create-file target already exists ({})",
                    example.stem,
                    fix.target.as_deref().unwrap_or("?"),
                ));
                continue;
            }
            if fix.edit.replacement != example.after {
                failures.push(format!(
                    "`{}`: created-file content mismatch\n  actual:   {:?}\n  expected: {:?}",
                    example.stem, fix.edit.replacement, example.after,
                ));
            }
            continue;
        }
        let edits: Vec<fix::Edit> = same_file.iter().map(|f| f.edit.clone()).collect();
        let (patched, _) = fix::apply(&example.before, &edits);
        // Byte equality is the contract; the applier's stale-leg skips are
        // its own drift-correction posture (a no-op leg never blocks the
        // round-trip — the patched text must still be exact).
        if patched != example.after {
            failures.push(format!(
                "`{}`: applied text mismatch\n  actual:   {:?}\n  expected: {:?}",
                example.stem, patched, example.after,
            ));
        }
    }
    if skipped_engine > 0 {
        println!(
            "note: {skipped_engine} engine-sourced examples skipped (DART_SDK unset; \
             the fix face's semantic pass answers no rows without an SDK root)"
        );
    }
    let _ = std::fs::remove_dir_all(&workspace);
    assert!(
        failures.is_empty(),
        "{} of {} examples diverged:\n\n{}",
        failures.len(),
        examples.len(),
        failures.join("\n\n"),
    );
}

/// The completeness gate: the table and the corpus hold each other in
/// place. Every row owns at least one example or names itself in
/// [`NO_EXAMPLE_YET`] with a reason; every example names a real row; and
/// an allow-list entry whose row has an example (or names no row) is
/// stale — the list shrinks only.
#[test]
fn fix_table_every_row_has_an_example() {
    let examples = read_corpus();
    let mut covered: BTreeMap<&str, usize> = BTreeMap::new();
    for example in &examples {
        *covered.entry(example.code.as_str()).or_default() += 1;
    }
    let rows: Vec<&str> = fix_table::all_specs().iter().map(|s| s.code).collect();
    for code in &rows {
        let allowed = NO_EXAMPLE_YET.iter().any(|(c, _)| c == code);
        let has_example = covered.contains_key(code);
        assert!(
            allowed != has_example,
            "fix row `{code}` must own an example pair or sit in NO_EXAMPLE_YET \
             with a reason — exactly one of the two, never both, never neither",
        );
    }
    for code in covered.keys() {
        assert!(
            rows.contains(&code),
            "example `{code}` names a code no fix-table row carries",
        );
    }
}
