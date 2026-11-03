//! The `MultiAnalysisRule` umbrella-name resolution pin (#3774): upstream
//! registers `analyzer_public_api` (5 diagnostic codes) and
//! `analyzer_element_model_tracking` (3 codes) once under the umbrella
//! name — `Registry.lintRules` is keyed by rule name only, so the umbrella
//! is the sole `linter.rules` enable switch and a member code name in an
//! options file is `undefined_lint` upstream. dartq splits the codes into
//! one spec each; `EnableSet::resolve` fans an umbrella name out to its
//! member set. These pins hold the fan-out, the disable arm, the
//! mixed-spelling arms, and the exact-code-name regression — against the
//! resolved set, not the source.

use dartq_commands::lints::enable_set;
use dartq_lints::{EnableSet, RuleId, rule_by_name, rules};

const PUBLIC_API_MEMBERS: &[&str] = &[
    "analyzer_public_api_bad_part_directive",
    "analyzer_public_api_bad_type",
    "analyzer_public_api_experimental_inconsistency",
    "analyzer_public_api_exports_non_public_name",
    "analyzer_public_api_impl_in_public_api",
];

const ELEMENT_TRACKING_MEMBERS: &[&str] = &[
    "analyzer_element_model_tracking_bad",
    "analyzer_element_model_tracking_more_than_one",
    "analyzer_element_model_tracking_zero",
];

/// The enabled machine names of `set`, registry order — the comparable
/// enabled-code set the acceptance pins assert on.
#[expect(
    clippy::cast_possible_truncation,
    reason = "rule positions come from the static registry, bounded far \
              below u16::MAX; u16 is the RuleId payload width"
)]
fn enabled_names(set: &EnableSet) -> Vec<&'static str> {
    rules()
        .iter()
        .enumerate()
        .filter(|(index, _)| set.is_enabled(RuleId(*index as u16)))
        .map(|(_, spec)| spec.name)
        .collect()
}

fn enable(rules: &[&str]) -> EnableSet {
    let owned: Vec<String> = rules.iter().map(|name| (*name).to_owned()).collect();
    enable_set(Some(&owned))
}

/// `analyzer_public_api: true` enables every member spec and nothing else —
/// the upstream `MultiAnalysisRule` mount, where the registered rule emits
/// any of its five codes.
#[test]
fn umbrella_true_enables_every_member() {
    let set = enable(&["analyzer_public_api"]);
    assert_eq!(enabled_names(&set), PUBLIC_API_MEMBERS);
    assert!(
        PUBLIC_API_MEMBERS
            .iter()
            .all(|name| set.is_enabled(rule_by_name(name).expect("registered")))
    );

    let set = enable(&["analyzer_element_model_tracking"]);
    assert_eq!(enabled_names(&set), ELEMENT_TRACKING_MEMBERS);
}

/// `analyzer_public_api: false` enables nothing: the options merge drops
/// disabled entries before `lint_rules` is built, so the name never
/// reaches resolution — the upstream `ruleConfigs[name].isEnabled == false`
/// arm (rule not mounted, no member emits).
#[test]
fn umbrella_false_enables_nothing() {
    // The merged `lint_rules` face carries enabled names only; a disabled
    // umbrella is simply absent. An empty effective list resolves empty.
    let set = enable_set(Some(&[]));
    assert_eq!(set, EnableSet::none());
    assert_eq!(enabled_names(&set), Vec::<&str>::new());
}

/// The per-code spelling under an umbrella has no carve-out: upstream the
/// member code name is not a registered lint (`undefined_lint`), so it
/// cannot subtract from the umbrella's mount. dartq mirrors that through
/// `lint_names::is_known` — the member spelling never reaches resolution —
/// so `analyzer_public_api: true` still yields all five, and a
/// member-named entry alone (no umbrella) yields none through the
/// options-file face.
#[test]
fn member_spelling_under_umbrella_matches_upstream() {
    // `analyzer_public_api: true` + `analyzer_public_api_bad_type: false`:
    // the merge drops the disabled member name; the umbrella fans out.
    let set = enable(&["analyzer_public_api"]);
    assert_eq!(enabled_names(&set), PUBLIC_API_MEMBERS);

    // The member spelling alone is filtered by `is_known` (the oracle's
    // `undefined_lint` arm): through the options face it enables nothing.
    let set = enable(&["analyzer_public_api_bad_type"]);
    assert_eq!(enabled_names(&set), Vec::<&str>::new());
}

/// Regression: the conformance path resolves exact member code names
/// through `EnableSet::resolve` directly (no `is_known` gate), and that
/// keeps working — one name, one member, no fan-out.
#[test]
fn exact_member_name_still_resolves() {
    let (set, unknown) = EnableSet::resolve(["analyzer_public_api_bad_type"]);
    assert!(unknown.is_empty());
    assert_eq!(enabled_names(&set), ["analyzer_public_api_bad_type"]);

    // The umbrella name is consumed by the fan-out, not reported unknown.
    let (set, unknown) = EnableSet::resolve(["analyzer_public_api"]);
    assert!(unknown.is_empty());
    assert_eq!(enabled_names(&set), PUBLIC_API_MEMBERS);
}

/// The umbrella enable survives a mixed options list alongside ordinary
/// rule names — one enable-set consult, both arms resolved.
#[test]
fn umbrella_composes_with_plain_names() {
    let set = enable(&["avoid_empty_else", "analyzer_public_api"]);
    assert!(set.is_enabled(rule_by_name("avoid_empty_else").expect("wired")));
    assert!(
        PUBLIC_API_MEMBERS
            .iter()
            .all(|name| set.is_enabled(rule_by_name(name).expect("registered")))
    );
}
