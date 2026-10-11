//! Conditional-URI selection for configurable import/export directives
//! (PRD §9.4), pinned to the differential oracle — `dart analyze` — not the
//! CFE.
//!
//! # Oracle pin (frozen)
//!
//! 1. Each `if (name)` / `if (name == 'value')` clause tests the written
//!    `=` string; the bare form tests `"true"` (`valueOrTrue`,
//!    `unlinked_data.dart:327-334`; CFE `outline_builder.dart:801`).
//! 2. No forking — one analysis per file. The oracle builds its context
//!    with EMPTY declared variables when no `-D` is passed
//!    (`driver.dart:583-593`, `DeclaredVariables.fromMap({})`);
//!    `dart.library.*` is never auto-populated.
//! 3. Selection scans the configurations in source order and the FIRST
//!    clause with `declaredVariables[name] == tested value` wins
//!    (`file_state.dart:738-764`; `selectedConfigurationUri ??=` keeps the
//!    first match). Exact string equality; an undeclared name never
//!    matches.
//! 4. No match → the primary URI (the first, unconditioned string
//!    literal; `file_state.dart:759` `selected: selectedConfigurationUri
//!    ?? primaryUri`).
//! 5. Only the selected URI is analyzed and diagnosed; unmatched branch
//!    URIs are kept for navigation but never read; a missing selected
//!    target reports `uri_does_not_exist` at the directive's URI literal.
//! 6. Documented divergence, deliberately not followed: the CFE evaluates
//!    `dart.library.X` against SDK target availability
//!    (`source_loader.dart:451-483`), so `dart compile` selects `b.dart`
//!    for `import 'a.dart' if (dart.library.io) 'b.dart';` while the
//!    oracle — dartq's differential truth — selects `a.dart`.
//!
//! Under the engine's empty default context (today's oracle truth, no
//! `-D` surface) every condition misses and the primary URI always wins.

use rustc_hash::FxHashMap;

/// One `if (name == 'value') 'uri'` clause of a configurable directive.
///
/// `value` is `None` for a bare `if (name)` — and for a written
/// `if (name == '')`: the oracle stores the value as the empty string and
/// `valueOrTrue` reads `''` back as `'true'` (`unlinked_data.dart:327-334`),
/// so [`Configuration::new`] normalizes the written `''` into the bare form
/// and evaluation compares against `"true"`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Configuration {
    /// The condition's dotted name, as written (`dart.library.io`, a `-D`
    /// name — no normalization; matching compares it verbatim).
    pub dotted_name: Box<str>,
    /// The `=` value of the condition; `None` for the bare form (a written
    /// `''` normalizes to `None` too — see the type's docs).
    pub value: Option<Box<str>>,
    /// The URI string selected when this configuration matches.
    pub uri: Box<str>,
}

impl Configuration {
    /// Builds one configuration, folding a written empty `=` value
    /// (`if (x == '')`) into the bare form: the oracle's `valueOrTrue`
    /// treats `''` and an absent value identically (`unlinked_data.dart
    /// :327-334`).
    #[must_use]
    pub fn new(dotted_name: &str, value: Option<&str>, uri: &str) -> Self {
        Self {
            dotted_name: Box::from(dotted_name),
            value: value.filter(|value| !value.is_empty()).map(Box::from),
            uri: Box::from(uri),
        }
    }

    /// The condition's tested value: the written `=` string, or `"true"`
    /// for the bare form (`valueOrTrue`).
    fn expected(&self) -> &str {
        self.value.as_deref().unwrap_or("true")
    }
}

/// Which URI string a configurable directive selects.
///
/// Index-based so the caller keeps the primary/configuration split without
/// re-allocating URI strings: [`SelectedUri::Primary`] selects the directive's
/// primary URI, [`SelectedUri::Configuration`] selects
/// `configs[index].uri`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectedUri {
    /// No configuration matched: use the primary URI — the oracle fallback
    /// (`file_state.dart:759` `selected: selectedConfigurationUri ??
    /// primaryUri`).
    Primary,
    /// `configs[index]` was the first matching configuration.
    Configuration(usize),
}

/// Selects the configured URI of a directive (PRD §9.4, the oracle pin):
/// source order, first configuration whose full dotted name is declared in
/// `declared` with exactly the tested value ([`Configuration::expected`];
/// `file_state.dart:738-764`); otherwise the primary URI
/// ([`SelectedUri::Primary`], `file_state.dart:759`).
///
/// Pure and allocation-free: all comparisons are borrowed string equality.
/// An undeclared name never matches — under the engine's empty default
/// map (the no-`-D` oracle context) nothing matches at all.
#[expect(
    clippy::implicit_hasher,
    reason = "the declared-variables map is engine-internal and pinned to FxBuildHasher; std HashMap is disallowed workspace-wide"
)]
#[must_use]
pub fn resolve_conditional(
    configs: &[Configuration],
    declared: &FxHashMap<String, String>,
) -> SelectedUri {
    for (index, config) in configs.iter().enumerate() {
        if declared
            .get(&*config.dotted_name)
            .is_some_and(|value| value == config.expected())
        {
            return SelectedUri::Configuration(index);
        }
    }
    SelectedUri::Primary
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declared(pairs: &[(&str, &str)]) -> FxHashMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    /// The pin's anchor shape under the oracle-default EMPTY context:
    /// `import 'a.dart' if (dart.library.io) 'b.dart';` selects the primary
    /// `a.dart` — the declared map carries no `dart.library.*` rows, so no
    /// condition matches (steps 2 + 4). The CFE would pick the branch on
    /// the VM — the documented divergence (step 6).
    #[test]
    fn empty_context_selects_primary() {
        let configs = [
            Configuration::new("dart.library.io", None, "b.dart"),
            Configuration::new("dart.library.html", None, "c.dart"),
        ];
        assert_eq!(
            resolve_conditional(&configs, &FxHashMap::default()),
            SelectedUri::Primary
        );
    }

    /// A bare `if (name)` tests `"true"` exactly: it matches a declared
    /// `name=true` and nothing else (`valueOrTrue`, step 1); an undeclared
    /// name never matches (step 3).
    #[test]
    fn bare_condition_tests_true() {
        let configs = [Configuration::new("dart.library.io", None, "b.dart")];
        let context = declared(&[("dart.library.io", "true")]);
        assert_eq!(
            resolve_conditional(&configs, &context),
            SelectedUri::Configuration(0)
        );
        let context = declared(&[("dart.library.io", "TRUE")]);
        assert_eq!(
            resolve_conditional(&configs, &context),
            SelectedUri::Primary,
            "no case folding"
        );
        let context = declared(&[("other", "true")]);
        assert_eq!(
            resolve_conditional(&configs, &context),
            SelectedUri::Primary,
            "an undeclared name never matches"
        );
    }

    /// `if (name == 'value')` matches exact string equality against the
    /// declared value (step 3).
    #[test]
    fn written_value_is_exact_equality() {
        let context = declared(&[("flutter.mode", "release")]);
        let configs = [Configuration::new(
            "flutter.mode",
            Some("release"),
            "b.dart",
        )];
        assert_eq!(
            resolve_conditional(&configs, &context),
            SelectedUri::Configuration(0)
        );
        for value in ["Release", "releases", "true"] {
            let configs = [Configuration::new("flutter.mode", Some(value), "b.dart")];
            assert_eq!(
                resolve_conditional(&configs, &context),
                SelectedUri::Primary,
                "`{value}` is not the declared `release`"
            );
        }
    }

    /// Source order, first match wins; an earlier clause whose value misses
    /// never shadows a later match (`selectedConfigurationUri ??=`, step 3).
    #[test]
    fn first_match_wins_in_source_order() {
        let context = declared(&[("a", "x"), ("b", "y")]);
        let configs = [
            Configuration::new("a", Some("nope"), "a.dart"),
            Configuration::new("b", Some("y"), "b.dart"),
            Configuration::new("b", Some("y"), "c.dart"),
        ];
        assert_eq!(
            resolve_conditional(&configs, &context),
            SelectedUri::Configuration(1)
        );
    }

    /// No match falls back to the primary URI (step 4); an empty
    /// configuration list is trivially primary.
    #[test]
    fn no_match_falls_back_to_primary() {
        let context = declared(&[("a", "x")]);
        let configs = [Configuration::new("a", Some("y"), "b.dart")];
        assert_eq!(
            resolve_conditional(&configs, &context),
            SelectedUri::Primary
        );
        assert_eq!(resolve_conditional(&[], &context), SelectedUri::Primary);
    }

    /// A written `= ''` is the absent test: `valueOrTrue` reads `''` back as
    /// `'true'` (`unlinked_data.dart:327-334`) — normalized at construction.
    #[test]
    fn written_empty_value_is_the_absent_test() {
        assert_eq!(
            Configuration::new("a", Some(""), "cfg.dart").value,
            None,
            "`= ''` folds into the bare form"
        );
        let context = declared(&[("a", "true")]);
        let configs = [Configuration::new("a", Some(""), "cfg.dart")];
        assert_eq!(
            resolve_conditional(&configs, &context),
            SelectedUri::Configuration(0)
        );
    }
}
