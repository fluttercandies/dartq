//! Registry completeness pin: every wave-1 rule must exist in the official
//! `package:lints` sets, with its [`Group`] matching the set it came from.
//!
//! The lists below are pinned verbatim from `.third/lints/lib/core.yaml` and
//! `.third/lints/lib/recommended.yaml` (the pinned `package:lints` snapshot;
//! AGENTS.md fact-source discipline). When the snapshot re-pins, update the
//! lists here in the same change as the registry.
//!
//! Also the governance pin for the frozen lint-gate universe
//! (`tests/harness/lint_codes.txt`, the `--codes-file` of
//! `scripts/lint-gate.sh`): the freeze must stay exactly the set of
//! `package:lints` rules this registry can emit — no orphans (rows the
//! implementation cannot emit), no gaps (mounted rules the gate would
//! never judge). A stale freeze fails here with the offending rows named.

use std::collections::BTreeSet;
use std::path::Path;

use dartq_lints::{EnableSet, Group, RULES, rule_by_name};

/// `.third/lints/lib/core.yaml` — the critical set, pinned verbatim.
const OFFICIAL_CORE: &[&str] = &[
    "avoid_empty_else",
    "avoid_relative_lib_imports",
    "avoid_shadowing_type_parameters",
    "avoid_types_as_parameter_names",
    "await_only_futures",
    "camel_case_extensions",
    "camel_case_types",
    "collection_methods_unrelated_type",
    "curly_braces_in_flow_control_structures",
    "dangling_library_doc_comments",
    "depend_on_referenced_packages",
    "empty_catches",
    "file_names",
    "hash_and_equals",
    "implicit_call_tearoffs",
    "library_annotations",
    "no_duplicate_case_values",
    "no_wildcard_variable_uses",
    "non_constant_identifier_names",
    "null_check_on_nullable_type_parameter",
    "prefer_generic_function_type_aliases",
    "prefer_is_empty",
    "prefer_is_not_empty",
    "prefer_iterable_whereType",
    "prefer_typing_uninitialized_variables",
    "provide_deprecation_message",
    "secure_pubspec_urls",
    "type_literal_in_constant_pattern",
    "unintended_html_in_doc_comment",
    "unnecessary_overrides",
    "unrelated_type_equality_checks",
    "use_string_in_part_of_directives",
    "valid_regexps",
    "void_checks",
];

/// `.third/lints/lib/recommended.yaml` — the idiom set (includes core via
/// `include:`), pinned verbatim.
const OFFICIAL_RECOMMENDED: &[&str] = &[
    "annotate_overrides",
    "avoid_function_literals_in_foreach_calls",
    "avoid_init_to_null",
    "avoid_renaming_method_parameters",
    "avoid_return_types_on_setters",
    "avoid_returning_null_for_void",
    "avoid_single_cascade_in_expression_statements",
    "constant_identifier_names",
    "control_flow_in_finally",
    "empty_constructor_bodies",
    "empty_statements",
    "exhaustive_cases",
    "implementation_imports",
    "invalid_runtime_check_with_js_interop_types",
    "library_prefixes",
    "library_private_types_in_public_api",
    "no_leading_underscores_for_library_prefixes",
    "no_leading_underscores_for_local_identifiers",
    "null_closures",
    "overridden_fields",
    "package_names",
    "prefer_adjacent_string_concatenation",
    "prefer_collection_literals",
    "prefer_conditional_assignment",
    "prefer_contains",
    "prefer_final_fields",
    "prefer_for_elements_to_map_fromIterable",
    "prefer_function_declarations_over_variables",
    "prefer_if_null_operators",
    "prefer_initializing_formals",
    "prefer_inlined_adds",
    "prefer_interpolation_to_compose_strings",
    "prefer_is_not_operator",
    "prefer_null_aware_operators",
    "prefer_spread_collections",
    "recursive_getters",
    "slash_for_doc_comments",
    "type_init_formals",
    "unnecessary_brace_in_string_interps",
    "unnecessary_const",
    "unnecessary_constructor_name",
    "unnecessary_getters_setters",
    "unnecessary_late",
    "unnecessary_library_name",
    "unnecessary_new",
    "unnecessary_null_aware_assignments",
    "unnecessary_null_in_if_null_operators",
    "unnecessary_nullable_for_final_variable_declarations",
    "unnecessary_string_escapes",
    "unnecessary_string_interpolations",
    "unnecessary_this",
    "unnecessary_to_list_in_spreads",
    "use_function_type_syntax_for_parameters",
    "use_rethrow_when_possible",
    "use_super_parameters",
];

/// The analyzer-diagnostic mirrors — not `package:lints` members. The
/// diagnostic registry carries the machine name
/// (`pkg/analyzer/lib/src/diagnostic/diagnostic.g.dart`,
/// `unnecessary_type_check_true` / `unnecessary_type_check_false`); the
/// linter-side rule file left the tree in the Dart 3.15 main re-pin.
const OFFICIAL_DIAGNOSTIC_MIRRORS: &[&str] = &["unnecessary_type_check"];

#[test]
fn rule_names_are_unique() {
    let mut seen: Vec<&str> = Vec::with_capacity(RULES.len());
    for spec in RULES {
        assert!(
            seen.iter().all(|s| *s != spec.name),
            "duplicate rule name {}",
            spec.name
        );
        seen.push(spec.name);
    }
}

/// The completeness pin: a registry entry that leaves the official sets (or
/// carries the wrong `Group`) breaks the crate's contract.
#[test]
fn every_rule_is_in_its_official_set() {
    for spec in RULES {
        let official = match spec.group {
            Group::Core => OFFICIAL_CORE,
            Group::Recommended => OFFICIAL_RECOMMENDED,
            Group::Diagnostic => OFFICIAL_DIAGNOSTIC_MIRRORS,
        };
        assert!(
            official.contains(&spec.name),
            "{} claims group {:?} but is absent from the official list",
            spec.name,
            spec.group
        );
    }
}

/// The recommended set includes core (`include: package:lints/core.yaml`),
/// so a Recommended entry must not also live in core — that would mean it
/// was registered with the wrong group.
#[test]
fn recommended_rules_are_not_core_rules() {
    for spec in RULES {
        if spec.group == Group::Recommended {
            assert!(
                !OFFICIAL_CORE.contains(&spec.name),
                "{} is a core-set rule but registered as Recommended",
                spec.name
            );
        }
    }
}

/// [`EnableSet`] stores two 64-bit words; the registry must stay within it.
/// Grow `WORDS` together with this bound when the set outgrows a word.
#[test]
fn registry_fits_the_enable_set_word_count() {
    assert!(
        RULES.len() <= 128,
        "registry holds {} rules; EnableSet has two 64-bit words",
        RULES.len()
    );
}

/// The slice pin: 89 `package:lints` members through #586 (34 core + 55
/// recommended-only, no member left blocked or documented-out) plus the
/// #570 diagnostic mirror make 90.
#[test]
fn registry_holds_the_pinned_count() {
    assert_eq!(RULES.len(), 90);
}

/// The two camelCase registry spellings are the yaml/LintNames forms
/// byte-for-byte (`prefer_iterable_whereType` /
/// `prefer_for_elements_to_map_fromIterable`), and their machine-form
/// rendering is the oracle's `emitMachineFormat` convention
/// (`pkg/dartdev/lib/src/commands/analyze.dart:605`,
/// `error.code?.toUpperCase()`) — the JSON writer emits `code.name`
/// verbatim, the machine writer the uppercased form.
#[test]
fn camel_case_rule_names_pin_their_code_strings() {
    for (name, machine) in [
        ("prefer_iterable_whereType", "PREFER_ITERABLE_WHERETYPE"),
        (
            "prefer_for_elements_to_map_fromIterable",
            "PREFER_FOR_ELEMENTS_TO_MAP_FROMITERABLE",
        ),
    ] {
        let id = rule_by_name(name).unwrap_or_else(|| panic!("{name} must stay registered"));
        assert_eq!(RULES[id.0 as usize].name, name);
        assert_eq!(name.to_uppercase(), machine);
    }
}

#[test]
fn rule_by_name_round_trips() {
    for (i, spec) in RULES.iter().enumerate() {
        let id = rule_by_name(spec.name).expect("registered name");
        assert_eq!(id.0 as usize, i, "{} sits at a stale index", spec.name);
    }
    assert_eq!(rule_by_name("no_such_lint_name"), None);
}

/// The official registry lowercases every name (`registry.dart:42,53,133`),
/// so any casing of a registered name resolves — the SDK machine lowercase
/// form and the `package:lints` mixed-case yaml spelling are the same rule,
/// and only truly unknown names error (the `undefined_lint` face).
#[test]
fn rule_by_name_is_case_insensitive() {
    let canonical = rule_by_name("prefer_iterable_whereType").expect("registered name");
    for spelling in [
        "prefer_iterable_wheretype",
        "PREFER_ITERABLE_WHERETYPE",
        "Prefer_Iterable_WhereType",
    ] {
        assert_eq!(rule_by_name(spelling), Some(canonical), "{spelling}");
    }
    assert_eq!(rule_by_name("prefer_iterable_wheretypeX"), None);
}

#[test]
fn enable_set_all_and_resolve_agree() {
    let all = EnableSet::all();
    for i in 0..RULES.len() {
        assert!(all.is_enabled(dartq_lints::RuleId(i as u16)));
    }
    let (resolved, unknown) = EnableSet::resolve(RULES.iter().map(|s| s.name));
    assert!(unknown.is_empty());
    assert_eq!(resolved, all);
}

/// Every template renders: problem texts are non-empty and `{n}` slots fill
/// (the two-variant rules pass their texts explicitly, so `report_texts`
/// covers them — this pins the registry-level templates only). Literal
/// braces in prose (e.g. `'{}'` in empty_constructor_bodies) are fine; only
/// unfilled `{n}` slots fail.
#[test]
fn templates_are_well_formed() {
    for spec in RULES {
        assert!(
            !spec.problem.is_empty(),
            "{} has no problemMessage",
            spec.name
        );
        let rendered = spec.render_problem(&["X"]);
        assert!(!rendered.contains("{0}"), "{} left a raw slot", spec.name);
        if !spec.correction.is_empty() {
            assert!(spec.render_correction(&["X"]).is_some());
        } else {
            assert_eq!(spec.render_correction(&[]), None);
        }
    }
}

// —— the frozen lint-gate universe (tests/harness/lint_codes.txt) ———

/// The freeze file `scripts/lint-gate.sh` scopes the INFO x LINT run with
/// (`--codes-file`); relative to this crate's manifest.
const LINT_CODES_FREEZE: &str = "../../tests/harness/lint_codes.txt";

/// The `kinds: &[]` set rules that still emit — through the dartq-commands
/// pubspec pass at the #434 mounting face, not through AST dispatch (the
/// crate docs' pubspec seam). The only legitimate empty-`kinds` members.
const PUBSPEC_PASS_SEAM: &[&str] = &["package_names", "secure_pubspec_urls"];

/// Set rules that cannot emit and are therefore consciously out of the
/// freeze (each also needs a `lint_divergences.tsv` `excluded` row — the
/// gate preflight's A3). Must stay exactly equal to the registered set
/// rules that are neither AST-dispatched (`kinds` non-empty) nor
/// pubspec-pass members; empty is the healthy state: a rule landing here
/// is a dead seam the re-freeze must reason about.
const STRUCTURALLY_SILENT: &[&str] = &[];

/// The registered set rules the implementation can emit: non-mirror specs
/// that dispatch on the AST or run through the pubspec pass.
fn emittable_set_rules() -> BTreeSet<&'static str> {
    RULES
        .iter()
        .filter(|spec| spec.group != Group::Diagnostic)
        .filter(|spec| !spec.kinds.is_empty() || PUBSPEC_PASS_SEAM.contains(&spec.name))
        .map(|spec| spec.name)
        .collect()
}

/// The freeze file's code rows: one lowercase machine name per line, `#`
/// comments and blanks ignored (the gate script's own reading).
fn freeze_rows(text: &str) -> Vec<&str> {
    text.lines()
        .map(|line| line.trim_end_matches('\r'))
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

/// The governance pin: the freeze and the registry-derived universe must
/// be the same set. Catches the two drift directions that matter — a
/// retired/mis-spelled/silent row the gate would never see fire, and a
/// mounted rule the oracle can report while the freeze keeps dartq's
/// silence out of the denominator.
#[test]
fn lint_codes_freeze_matches_the_registry() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(LINT_CODES_FREEZE);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let rows = freeze_rows(&text);

    let mut seen = BTreeSet::new();
    let dupes: Vec<&str> = rows
        .iter()
        .copied()
        .filter(|row| !seen.insert(*row))
        .collect();
    assert!(
        dupes.is_empty(),
        "{LINT_CODES_FREEZE}: duplicate entries: {dupes:?}"
    );

    let emittable = emittable_set_rules();
    let mut orphans = Vec::new();
    for row in &rows {
        let Some(id) = rule_by_name(row) else {
            orphans.push(format!("{row}: not a registered rule"));
            continue;
        };
        let spec = RULES[id.0 as usize];
        if spec.group == Group::Diagnostic {
            orphans.push(format!("{row}: a diagnostic mirror, opt-in by name only"));
        } else if !emittable.contains(spec.name) {
            orphans.push(format!("{row}: structurally silent (kinds: &[])"));
        }
    }
    assert!(
        orphans.is_empty(),
        "{LINT_CODES_FREEZE}: rows the implementation cannot emit: {orphans:?}"
    );

    let frozen: BTreeSet<&str> = rows.iter().copied().collect();
    let gaps: Vec<String> = emittable
        .iter()
        .filter(|name| !frozen.contains(name.to_ascii_lowercase().as_str()))
        .map(|name| name.to_ascii_lowercase())
        .collect();
    assert!(
        gaps.is_empty(),
        "{LINT_CODES_FREEZE}: mounted package:lints rules missing from the freeze (the \
         gate would never judge them): {gaps:?} — re-freeze via regen_lint_codes_freeze \
         and reconcile the header prose"
    );

    let actually_silent: BTreeSet<&'static str> = RULES
        .iter()
        .filter(|spec| spec.group != Group::Diagnostic)
        .filter(|spec| spec.kinds.is_empty() && !PUBSPEC_PASS_SEAM.contains(&spec.name))
        .map(|spec| spec.name)
        .collect();
    let silent: BTreeSet<&str> = STRUCTURALLY_SILENT.iter().copied().collect();
    assert_eq!(
        silent, actually_silent,
        "STRUCTURALLY_SILENT must name exactly the set rules that cannot emit \
         (registry holds {:?}; the pin lists {:?})",
        actually_silent, silent
    );

    for name in PUBSPEC_PASS_SEAM {
        let Some(id) = rule_by_name(name) else {
            panic!("PUBSPEC_PASS_SEAM member {name} is not registered");
        };
        let spec = RULES[id.0 as usize];
        assert!(
            spec.group != Group::Diagnostic && spec.kinds.is_empty(),
            "PUBSPEC_PASS_SEAM member {name} no longer matches the pubspec seam shape"
        );
    }
}

/// Regenerates the freeze file's entry block from the registry-derived
/// universe, preserving the curated comment header verbatim. Oracle-free:
/// the inclusion rule is mechanical (the registered set rules that can
/// emit). Run explicitly, then reconcile the header prose with the new
/// rows — the re-freeze is deliberate, the regen is only its mechanical
/// half:
///
/// ```text
/// cargo test -p dartq-lints --test registry_pin regen_lint_codes_freeze -- --ignored
/// ```
#[test]
#[ignore = "rewrites tests/harness/lint_codes.txt; explicit re-freeze maintenance"]
fn regen_lint_codes_freeze() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(LINT_CODES_FREEZE);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let header_end = text
        .lines()
        .position(|line| {
            let line = line.trim_end_matches('\r');
            !line.is_empty() && !line.starts_with('#')
        })
        .expect("the freeze file carries an entry block");

    let mut entries: Vec<String> = emittable_set_rules()
        .into_iter()
        .map(|name| name.to_ascii_lowercase())
        .collect();
    entries.sort();

    let old: BTreeSet<String> = freeze_rows(&text)
        .into_iter()
        .map(str::to_ascii_lowercase)
        .collect();
    let new: BTreeSet<String> = entries.iter().cloned().collect();
    let added: Vec<&String> = new.difference(&old).collect();
    let removed: Vec<&String> = old.difference(&new).collect();

    let mut out = String::with_capacity(text.len() + entries.len() * 24);
    for line in text.lines().take(header_end) {
        out.push_str(line.trim_end_matches('\r'));
        out.push('\n');
    }
    for entry in &entries {
        out.push_str(entry);
        out.push('\n');
    }
    std::fs::write(&path, out).unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));

    println!(
        "regen_lint_codes_freeze: {} entries (was {}); reconcile the comment header",
        entries.len(),
        old.len()
    );
    println!("  added: {added:?}");
    println!("  removed: {removed:?}");
}
