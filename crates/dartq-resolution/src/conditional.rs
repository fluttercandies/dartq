//! Conditional-URI selection for configurable import/export directives
//! (PRD §9.4), pinned to the official evaluation on both faces.
//!
//! # Selection rule (official, cited)
//!
//! A configurable directive `import 'a.dart' if (n == 'v') 'b.dart' …`
//! selects its URI by scanning the configurations **in source order** and
//! taking the **first** whose condition holds; if none holds, the directive's
//! **primary URI** (the first, unconditioned string literal) is the fallback:
//!
//! - CFE: `fragment_factory_impl.dart:921-929` — `for (config in
//!   configurations) { if (getLibrarySupportValue(config.dottedName) ==
//!   config.condition) { uri = config.importUri; break; } }`, with `uri`
//!   starting as the default URI (`outline_builder.dart:751-752`, "For a
//!   conditional import, this is the default URI").
//! - Analyzer (the differential oracle): `file_state.dart:738-764` —
//!   `selected = selectedConfigurationUri ?? primaryUri` (`??=` keeps the
//!   first match).
//!
//! # Condition evaluation (official, cited)
//!
//! A condition's tested value is the written `=` string, or `"true"` for the
//! bare form (`outline_builder.dart:801`; analyzer `valueOrTrue`,
//! `unlinked_data.dart:327-334` — an empty written value reads back as
//! `"true"`, folded into [`Configuration`] at construction). The dotted name
//! must be a `dart.library.` qualifier (`kernel/lib/target/targets.dart
//! :215-224`); the compared environment value is `"true"` when the named
//! library is available for the target and nothing (never equal) otherwise
//! (`source_loader.dart:451-483`). The [`PlatformSet`] carries exactly that
//! target-derived `"true"` table — derived from the SDK's `libraries.json`
//! rows (key domain = public library names, PRD §7.2), never hardcoded.
//!
//! Known oracle divergence, deliberately following the analyzer: for a
//! non-`dart.library.` name the CFE's `getLibrarySupportValue` returns `""`,
//! so a written `if (foo == '')` would match under `dart compile`; the
//! analyzer's declared-variables comparison never matches an undeclared
//! name, and dart analyze is the oracle dartiq differentiates against.

use dartq_sdk::SdkLibraries;
use rustc_hash::FxHashSet;

/// One `if (name == 'value') 'uri'` clause of a configurable directive.
///
/// `value` is `None` for a bare `if (name)` — and for a written
/// `if (name == '')`: the oracle stores the value as the empty string and
/// `valueOrTrue` reads `''` back as `'true'` (`unlinked_data.dart:327-334`),
/// so [`Configuration::new`] normalizes the written `''` into the bare form
/// and evaluation compares against `"true"`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Configuration {
    /// The condition's dotted name, as written (`dart.library.io`,
    /// `dart.tool.…`, a `-D` name — no normalization; matching is exact).
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
    /// for the bare form (`outline_builder.dart:801`, `valueOrTrue`).
    fn expected(&self) -> &str {
        self.value.as_deref().unwrap_or("true")
    }
}

/// The `dart.library.*` availability table of one target segment (PRD §9.4):
/// the library names for which `dart.library.<name>` evaluates to `"true"`.
///
/// Derived at engine-assembly time from the SDK's `libraries.json` rows —
/// a name is available when its row sits in the target segment's effective
/// library set with `support_conditional_import` set (`source_loader.dart
/// :451-483` reads the same rows; key domain = public library names, PRD
/// §7.2). An unknown segment yields an empty set, under which every
/// condition is unmatched and primary URIs are selected.
///
/// Construction is one pass over the segment's library table; evaluation is
/// a hash lookup per tested name.
#[derive(Clone, Debug, Default)]
pub struct PlatformSet {
    available: FxHashSet<Box<str>>,
}

impl PlatformSet {
    /// Derives the availability table of target segment `segment` (e.g.
    /// `"vm"`) from the `libraries.json` parse result.
    #[must_use]
    pub fn from_segment(libraries: &SdkLibraries, segment: &str) -> Self {
        let Some(target) = libraries.targets.get(segment) else {
            return Self::default();
        };
        let available: FxHashSet<Box<str>> = target
            .libraries
            .iter()
            .filter(|(_, info)| info.support_conditional_import)
            .map(|(name, _)| Box::from(name.as_str()))
            .collect();
        Self { available }
    }

    /// Whether `dart.library.<library>` holds for this target
    /// (`source_loader.dart:451-483`): the library has a row in the target
    /// segment's effective set with `support_conditional_import` set.
    /// Unknown names are `false`.
    #[must_use]
    pub fn available(&self, library: &str) -> bool {
        self.available.contains(library)
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
    /// No configuration matched: use the primary URI — the official
    /// fallback (`fragment_factory_impl.dart:921-929` keeps the default
    /// `uri`; `file_state.dart:762` `selectedConfigurationUri ?? primaryUri`).
    Primary,
    /// `configs[index]` was the first matching configuration.
    Configuration(usize),
}

/// Selects the configured URI of a directive (PRD §9.4): source order, first
/// configuration whose dotted name is `dart.library.<name>` with `name`
/// available on `platform` and whose expected value is `"true"`; otherwise
/// the primary URI ([`SelectedUri::Primary`], the official fallback).
///
/// Pure and allocation-free: all comparisons are borrowed string equality.
/// Non-`dart.library.` names never match here — the `-D` declared-variables
/// face of the environment is a separate axis (analysis options), not part
/// of the platform table.
#[must_use]
pub fn resolve_conditional(configs: &[Configuration], platform: &PlatformSet) -> SelectedUri {
    for (index, config) in configs.iter().enumerate() {
        let Some(library) = config.dotted_name.strip_prefix("dart.library.") else {
            continue;
        };
        if platform.available(library) && config.expected() == "true" {
            return SelectedUri::Configuration(index);
        }
    }
    SelectedUri::Primary
}

#[cfg(test)]
mod tests {
    use super::*;
    use dartq_sdk::{SdkLibrary, TargetLibraries};
    use std::collections::BTreeMap;

    /// A `SdkLibraries` with a `vm` segment exposing `io` plus two rows the
    /// loader would carry with `support_conditional_import: false` (an
    /// opt-out row and an `_`-prefixed one, which the real loader forces
    /// `false` at parse time), and a `web` segment exposing `html`; mirrors
    /// the post-parse libraries.json row shape.
    fn sdk_libraries() -> SdkLibraries {
        let library = || SdkLibrary {
            uri: "x/x.dart".into(),
            patches: Vec::new(),
            support_conditional_import: true,
            support_direct_import: dartq_sdk::Importability::Always,
        };
        let unsupported = || SdkLibrary {
            support_conditional_import: false,
            ..library()
        };
        let mut vm: BTreeMap<String, SdkLibrary> = BTreeMap::new();
        vm.insert("io".into(), library());
        vm.insert("_vm_internal".into(), unsupported());
        vm.insert("io_blocked".into(), unsupported());
        let mut web: BTreeMap<String, SdkLibrary> = BTreeMap::new();
        web.insert("html".into(), library());
        SdkLibraries {
            targets: BTreeMap::from([
                ("vm".into(), TargetLibraries { libraries: vm }),
                ("web".into(), TargetLibraries { libraries: web }),
            ]),
        }
    }

    fn vm() -> PlatformSet {
        PlatformSet::from_segment(&sdk_libraries(), "vm")
    }

    /// An available library matches the bare condition (tri-state: match).
    #[test]
    fn available_library_matches() {
        let configs = [Configuration::new("dart.library.io", None, "io.dart")];
        assert_eq!(
            resolve_conditional(&configs, &vm()),
            SelectedUri::Configuration(0)
        );
        assert!(vm().available("io"));
    }

    /// A library present on another segment but not this one does not match;
    /// the primary URI is the fallback (tri-state: no-match fallback).
    #[test]
    fn unavailable_library_falls_back_to_primary() {
        let configs = [Configuration::new("dart.library.html", None, "html.dart")];
        assert_eq!(resolve_conditional(&configs, &vm()), SelectedUri::Primary);
        assert!(!vm().available("html"));
    }

    /// An unknown library key is `false` and never matches
    /// (`source_loader.dart:470-482`: absent → `null`, never equal).
    #[test]
    fn unknown_library_key_is_false() {
        let configs = [Configuration::new(
            "dart.library.nonexistent",
            None,
            "x.dart",
        )];
        assert_eq!(resolve_conditional(&configs, &vm()), SelectedUri::Primary);
        assert!(!vm().available("nonexistent"));
    }

    /// First match in source order wins; later matches never override.
    #[test]
    fn first_match_wins_in_chain() {
        let configs = [
            Configuration::new("dart.library.io", None, "io.dart"),
            Configuration::new("dart.library.html", None, "html.dart"),
        ];
        assert_eq!(
            resolve_conditional(&configs, &vm()),
            SelectedUri::Configuration(0)
        );
        let web = PlatformSet::from_segment(&sdk_libraries(), "web");
        assert_eq!(
            resolve_conditional(&configs, &web),
            SelectedUri::Configuration(1),
            "on web the io condition misses and html is the first match"
        );
    }

    /// Non-`dart.library.` names never match the platform table; the
    /// analyzer oracle never matches an undeclared name either.
    #[test]
    fn non_dart_library_names_never_match() {
        let configs = [
            Configuration::new("foo", None, "a.dart"),
            Configuration::new("foo", Some(""), "b.dart"),
            Configuration::new("dart.tool.dart2js", None, "c.dart"),
        ];
        assert_eq!(resolve_conditional(&configs, &vm()), SelectedUri::Primary);
    }

    /// `support_conditional_import: false` removes the name from the table
    /// (the `_`-prefixed rows are forced `false` at parse time upstream).
    #[test]
    fn conditional_import_support_gates_availability() {
        let platform = vm();
        assert!(!platform.available("io_blocked"));
        assert!(
            !platform.available("_vm_internal"),
            "`_`-prefixed internal names are parse-time forced false"
        );
        let configs = [
            Configuration::new("dart.library.io_blocked", None, "a.dart"),
            Configuration::new("dart.library._vm_internal", None, "b.dart"),
        ];
        assert_eq!(
            resolve_conditional(&configs, &platform),
            SelectedUri::Primary
        );
    }

    /// An unknown segment derives an empty table: every condition misses.
    #[test]
    fn unknown_segment_matches_nothing() {
        let platform = PlatformSet::from_segment(&sdk_libraries(), "wasm");
        assert!(
            !platform.available("io"),
            "unknown segment derives an empty table"
        );
        let configs = [Configuration::new("dart.library.io", None, "io.dart")];
        assert_eq!(
            resolve_conditional(&configs, &platform),
            SelectedUri::Primary
        );
    }

    /// The comparison is exact string equality against `"true"`
    /// (`source_loader.dart:453-455` `"true"`; no case folding, no other
    /// spellings).
    #[test]
    fn comparison_is_exact_string_equality() {
        let platform = vm();
        let configs = [Configuration::new("dart.library.io", Some("1"), "a.dart")];
        assert_eq!(
            resolve_conditional(&configs, &platform),
            SelectedUri::Primary
        );
        let configs = [Configuration::new(
            "dart.library.io",
            Some("True"),
            "a.dart",
        )];
        assert_eq!(
            resolve_conditional(&configs, &platform),
            SelectedUri::Primary
        );
        let configs = [Configuration::new(
            "dart.library.io",
            Some("true"),
            "a.dart",
        )];
        assert_eq!(
            resolve_conditional(&configs, &platform),
            SelectedUri::Configuration(0)
        );
    }

    /// A written `= ''` is the absent test: `valueOrTrue` reads `''` back as
    /// `'true'` (`unlinked_data.dart:327-334`) — normalized at construction.
    #[test]
    fn written_empty_value_is_the_absent_test() {
        assert_eq!(
            Configuration::new("dart.library.io", Some(""), "cfg.dart").value,
            None,
            "`= ''` folds into the bare form"
        );
        let configs = [Configuration::new("dart.library.io", Some(""), "cfg.dart")];
        assert_eq!(
            resolve_conditional(&configs, &vm()),
            SelectedUri::Configuration(0)
        );
    }

    /// An empty configuration list selects the primary URI.
    #[test]
    fn empty_configs_select_primary() {
        assert_eq!(resolve_conditional(&[], &vm()), SelectedUri::Primary);
    }
}
