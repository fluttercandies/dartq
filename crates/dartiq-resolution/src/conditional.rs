//! Conditional-URI selection, pinned to the analyzer oracle
//! (`docs/facts/resolution-design-spec.md` §2.2).
//!
//! `dart analyze` selects a configured directive URI by **exact string
//! match** against externally injected declared variables, which default to
//! an empty set (`file_state.dart:743-758`,
//! `declared_variables.dart:9-30`). The analyzer never seeds
//! `dart.library.*` keys — under a default environment every condition is
//! unmatched and the primary URI is selected. The CFE's target-based
//! `dart.library.*` evaluation (`source_loader.dart:461-483`) is the
//! `dart compile` path and is deliberately not implemented here (§2.4/§6-1):
//! implementing it would diverge dartiq's URI selection from the oracle.
//!
//! Match rule, verbatim from §2.2: for each configuration in source order,
//! compare `env.get(name) == value_or_true` where an absent `=` value means
//! `"true"` (`unlinked_data.dart:327-334`); the **first** match wins
//! (`??=`), and any later match does not override. No bool parsing, no
//! trimming — the comparison is plain string equality on any dotted name.

use std::collections::BTreeMap;

/// One `if (name == 'value') 'uri'` clause of a configurable directive.
///
/// `value` is `None` when the source wrote `if (name)` with no `=` value;
/// evaluation then compares against `"true"`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Configuration {
    /// The condition's dotted name, as written (no `dart.library` derivation,
    /// no normalization — matching is exact).
    pub dotted_name: Box<str>,
    /// The `=` value of the condition, or `None` for a bare `if (name)`.
    pub value: Option<Box<str>>,
    /// The URI string selected when this configuration matches.
    pub uri: Box<str>,
}

/// The declared-variables environment of URI selection (§2.2): an exact
/// string-to-string table injected from the outside, empty by default.
///
/// `BTreeMap` per the spec draft; lookups are per-configured-directive, not
/// per-token, so ordering determinism was preferred over hashed lookup.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConditionalEnv {
    vars: BTreeMap<Box<str>, Box<str>>,
}

impl ConditionalEnv {
    /// An empty environment — the analyzer's default, under which every
    /// condition is unmatched and primary URIs are selected.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Declares one variable, replacing any previous value (the analyzer
    /// rebuilds its table per context; last write wins at injection time).
    pub fn declare(&mut self, name: &str, value: &str) {
        self.vars.insert(Box::from(name), Box::from(value));
    }

    /// The value declared for `name`, or `None` when undeclared. An
    /// undeclared name matches no condition — including `if (x)`'s implicit
    /// `"true"`.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.vars.get(name).map(AsRef::as_ref)
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
    /// No configuration matched: use the primary URI (§2.2 last line).
    Primary,
    /// `configs[index]` was the first matching configuration.
    Configuration(usize),
}

/// Selects the configured URI of a directive (§2.2): source order, exact
/// `name`/`value` string equality against `env` with an absent value meaning
/// `"true"`, first match wins, otherwise the primary URI.
///
/// Pure and allocation-free: all comparisons are borrowed string equality.
#[must_use]
pub fn select_configured_uri(configs: &[Configuration], env: &ConditionalEnv) -> SelectedUri {
    for (index, config) in configs.iter().enumerate() {
        let expected = config.value.as_deref().unwrap_or("true");
        if env.get(&config.dotted_name) == Some(expected) {
            return SelectedUri::Configuration(index);
        }
    }
    SelectedUri::Primary
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(name: &str, value: Option<&str>, uri: &str) -> Configuration {
        Configuration {
            dotted_name: Box::from(name),
            value: value.map(Box::from),
            uri: Box::from(uri),
        }
    }

    /// §2.2: an empty declared-variables table (the analyzer default) selects
    /// the primary URI — the analyzer never seeds `dart.library.*`.
    #[test]
    fn empty_env_selects_primary() {
        let configs = [config("dart.library.io", None, "io.dart")];
        assert_eq!(
            select_configured_uri(&configs, &ConditionalEnv::new()),
            SelectedUri::Primary
        );
        assert_eq!(
            select_configured_uri(&[], &ConditionalEnv::new()),
            SelectedUri::Primary
        );
    }

    /// First match in source order wins; later matches never override
    /// (file_state.dart:743-758 `??=`).
    #[test]
    fn first_match_wins() {
        let configs = [
            config("a", Some("1"), "first.dart"),
            config("a", Some("1"), "second.dart"),
            config("b", None, "third.dart"),
        ];
        let mut env = ConditionalEnv::new();
        env.declare("a", "1");
        env.declare("b", "true");
        assert_eq!(
            select_configured_uri(&configs, &env),
            SelectedUri::Configuration(0)
        );
    }

    /// `if (x)` is `x == "true"` exactly (valueOrTrue, no bool parsing).
    #[test]
    fn bare_condition_compares_against_true() {
        let configs = [config("flutter", None, "fl.dart")];
        let mut env = ConditionalEnv::new();
        env.declare("flutter", "true");
        assert_eq!(
            select_configured_uri(&configs, &env),
            SelectedUri::Configuration(0)
        );

        env.declare("flutter", "True");
        assert_eq!(
            select_configured_uri(&configs, &env),
            SelectedUri::Primary,
            "matching is case-sensitive exact string equality"
        );
    }

    /// No trimming, no numeric coercion: the comparison is plain equality.
    #[test]
    fn comparison_is_exact_string_equality() {
        let configs = [config("a", Some(" 1"), "x.dart")];
        let mut env = ConditionalEnv::new();
        env.declare("a", "1");
        assert_eq!(select_configured_uri(&configs, &env), SelectedUri::Primary);

        env.declare("a", " 1");
        assert_eq!(
            select_configured_uri(&configs, &env),
            SelectedUri::Configuration(0)
        );
    }

    /// A later configuration with a different name matches when the earlier
    /// one does not.
    #[test]
    fn later_configuration_matches_when_earlier_misses() {
        let configs = [
            config("a", Some("1"), "a1.dart"),
            config("b", Some("2"), "b2.dart"),
        ];
        let mut env = ConditionalEnv::new();
        env.declare("b", "2");
        assert_eq!(
            select_configured_uri(&configs, &env),
            SelectedUri::Configuration(1)
        );
    }
}
