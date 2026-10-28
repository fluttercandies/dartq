//! Test scaffolding: scan a source snippet and hand-build its arena AST per
//! the collapse-map payload contracts. Rule tests pin the read-side
//! contract directly against hand-built trees; [`Fixture::parse`] builds
//! through the production bridge ([`crate::parse_unit`]) for the tests
//! that exercise the real parser→AST shape.
//!
//! Hand-built arena fixtures ([`Fixture::build`]) remain the tool for
//! payload-contract pins; behavioral case suites (the upstream port) live
//! in the dartq-fixture DSL.

#![cfg(any(test, feature = "test-support"))]

use std::path::{Path, PathBuf};

use dartq_ast::{AstBuilder, NodeKind, PayloadField, TOKEN_NONE};
use dartq_language::LanguageVersion;
use dartq_scanner::{ScanConfig, TokenTable, scan};
use dartq_source::{FileTable, LineIndex};

use crate::engine::{Features, Lint, Unit, narrow_u32, run};
use crate::registry::EnableSet;

/// The scan posture every fixture scans under: the latest feature set
/// with the `augmentations` experiment opted in under the official decode
/// contract — the production bridge's posture. A `const` (the frozen
/// call sites pass it by name), so the two bits are pinned — not derived —
/// to `ScanConfig::from_feature_set` over that exact set by the
/// `config_pins::config_matches_the_feature_fold` test: drift fails the
/// pin, never a fixture silently.
pub const CONFIG: ScanConfig = ScanConfig {
    enable_triple_shift: true,
    enable_augmentations: true,
};

#[cfg(test)]
mod config_pins {
    use super::*;

    /// The const bits must equal the keyed fold over the latest set with
    /// the `augmentations` opt-in: `triple-shift` (shipped 2.14) folds on
    /// by version, `augmentations` (unshipped) only through the explicit
    /// flag. A re-vendor that changes either posture breaks this pin, not
    /// the fixtures.
    #[test]
    fn config_matches_the_feature_fold() {
        let folded = ScanConfig::from_feature_set(
            dartq_language::FeatureSet::latest().with_experiment_for_version(
                dartq_language::DEFAULT_LANGUAGE_VERSION,
                dartq_language::Feature::Augmentations,
            ),
        );
        assert_eq!(CONFIG, folded);
    }
}

/// One parsed unit: everything a [`crate::Unit`] needs, kept alive together.
pub struct Fixture {
    pub source: String,
    pub tokens: TokenTable,
    pub ast: dartq_ast::SyntaxTable,
    pub lines: LineIndex,
    /// The file path the run reports (defaults to a valid lint name; the
    /// `file_names` tests overwrite it).
    pub path: PathBuf,
    pub features: Features,
    /// The settled language version the run gates on (`None` = no version
    /// face; the version-gated rules' tests set it per case).
    pub version: Option<LanguageVersion>,
    _files: FileTable,
}

impl Fixture {
    /// Parses `source` through the production bridge (scanner → parser →
    /// arena, one transient [`crate::ParsedUnit`]). The path defaults to the
    /// fixture name; overwrite `path`/`features`/`version` after
    /// construction for the path/feature/version-dependent rules.
    #[must_use]
    pub fn parse(source: &str) -> Self {
        let parsed = crate::parse_unit(source, Features::default());
        Self {
            source: parsed.source,
            tokens: parsed.tokens,
            ast: parsed.ast,
            lines: parsed.lines,
            path: PathBuf::from("lint_fixture.dart"),
            features: parsed.features,
            version: None,
            _files: FileTable::default(),
        }
    }

    /// Scans `source` and builds `tree` over the resulting token table.
    pub fn build(source: &str, tree: impl FnOnce(&mut AstBuilder, &TokenTable)) -> Self {
        let mut files = FileTable::default();
        let result = scan(source, CONFIG, files.intern(Path::new("lint_fixture.dart")));
        let tokens = result.tokens;
        let mut builder = AstBuilder::new();
        tree(&mut builder, &tokens);
        let ast = builder.finish();
        let lines = LineIndex::new(source);
        Self {
            source: source.to_owned(),
            tokens,
            ast,
            lines,
            path: PathBuf::from("lint_fixture.dart"),
            features: Features::default(),
            version: None,
            _files: files,
        }
    }

    /// The unit the run helpers build — the shell faces' direct consumers
    /// (`run_breaking_version_annotations`,
    /// `unnecessary_ignore_late_rows`) mount their own runs over it.
    pub fn unit(&self) -> Unit<'_> {
        Unit {
            source: &self.source,
            tokens: &self.tokens,
            ast: &self.ast,
            lines: &self.lines,
            path: Some(&self.path),
            features: self.features,
            version: self.version,
            workspace: None,
            reachability: None,
            async_context_uses: None,
            library_type_uses: None,
        }
    }

    /// Runs `rules` over the fixture.
    pub fn run(&self, rules: &EnableSet) -> Vec<Lint> {
        let unit = self.unit();
        run(&unit, rules, &rustc_hash::FxHashSet::default())
    }

    /// [`Fixture::run`] with the containing-package facts mounted — the
    /// pub-aware rules' dispatch face.
    pub fn run_workspace(&self, rules: &EnableSet, facts: &crate::WorkspaceFacts) -> Vec<Lint> {
        let unit = Unit {
            source: &self.source,
            tokens: &self.tokens,
            ast: &self.ast,
            lines: &self.lines,
            path: Some(&self.path),
            features: self.features,
            version: self.version,
            workspace: Some(facts),
            reachability: None,
            async_context_uses: None,
            library_type_uses: None,
        };
        run(&unit, rules, &rustc_hash::FxHashSet::default())
    }

    /// [`Fixture::run`] with the cross-library faces mounted — the #1157 W6
    /// closeout rules' consumption seams (`reachability` for
    /// `unreachable_from_main`, `async_uses` for
    /// `use_build_context_synchronously`); `workspace` rides along because
    /// the async rule's test-directory exemption reads it. A `None` face
    /// keeps its rule silent.
    pub fn run_with(
        &self,
        rules: &EnableSet,
        workspace: Option<&crate::WorkspaceFacts>,
        reachability: Option<&crate::engine::ReachabilityFace<'_>>,
        async_uses: Option<&[crate::engine::AsyncContextUse]>,
    ) -> Vec<Lint> {
        let unit = Unit {
            source: &self.source,
            tokens: &self.tokens,
            ast: &self.ast,
            lines: &self.lines,
            path: Some(&self.path),
            features: self.features,
            version: self.version,
            workspace,
            reachability: reachability.map(|face| crate::engine::ReachabilityFace {
                has_entry: face.has_entry,
                flags: face.flags,
            }),
            async_context_uses: async_uses,
            library_type_uses: None,
        };
        run(&unit, rules, &rustc_hash::FxHashSet::default())
    }

    /// [`Fixture::run`] with the semantic face mounted: the source folded
    /// as the sole analyzed file of `package:t` (the driver's per-file
    /// build shape), so rules answering through `Check::semantics` run
    /// resolved. The context tables leak (`'static`); tests are one-shot
    /// processes.
    #[must_use]
    pub fn run_semantic(&self, rules: &EnableSet) -> Vec<Lint> {
        self.run_semantic_inner(rules, true)
    }

    /// [`Fixture::run_semantic`] for scanner-dirty sources: the identical
    /// run, minus the scanner-clean gate — recovery fixtures (e.g. the
    /// #1956 unterminated block doc) carry a scanner diagnostic by design.
    #[must_use]
    pub fn run_semantic_scanner_dirty(&self, rules: &EnableSet) -> Vec<Lint> {
        self.run_semantic_inner(rules, false)
    }

    fn run_semantic_inner(&self, rules: &EnableSet, require_scanner_clean: bool) -> Vec<Lint> {
        let strings: &'static dartq_source::StringTable =
            Box::leak(Box::new(dartq_source::StringTable::new()));
        let mut files = FileTable::new();
        let file = files.intern(Path::new("lint_semantic.dart"));
        let features = dartq_parser::ParserFeatures::from_feature_set(
            dartq_language::FeatureSet::for_language_version(LanguageVersion::new(3, 15)),
        );
        let scanned = scan(&self.source, CONFIG, file);
        if require_scanner_clean {
            assert!(
                scanned.diagnostics.is_empty(),
                "scanner diagnostics: {:?}",
                self.source
            );
        }
        let elements = dartq_semantic::collect(&self.source, &scanned.tokens, features, strings);
        let mut builder = dartq_resolution::LibraryGraphBuilder::new(strings);
        let lib = builder.intern("package:t/lint_semantic.dart");
        builder.unit(lib, file);
        for (name, declaration) in elements.declarations() {
            if declaration.kind == dartq_semantic::MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib, name, file);
        }
        // The driver's implicit dart:core import (the unindexed synthetic
        // face the oracle's scope chain consults without a directive) — the
        // generated engine table's merged core surface, same source the
        // production context seeds.
        let core_file = files.intern(Path::new("<dart:core>"));
        let (_, core_face) = dartq_engine::sdk_faces_gen::SDK_LIBRARY_FACES
            .iter()
            .find(|(uri, _)| *uri == "core")
            .expect("generated SDK table carries dart:core");
        builder.seed_implicit_core(lib, core_file, core_face);
        let mut index = dartq_semantic::ElementIndex::default();
        index.insert(file, elements);
        let graph: &'static dartq_resolution::LibraryGraph = Box::leak(Box::new(builder.finish()));
        let namespaces: &'static dartq_resolution::Namespaces =
            Box::leak(Box::new(dartq_resolution::Namespaces::build(graph)));
        let index: &'static dartq_semantic::ElementIndex = Box::leak(Box::new(index));
        let analysis = dartq_semantic::analyze_unit(
            &self.source,
            &scanned.tokens,
            features,
            strings,
            file,
            lib,
            graph,
            namespaces,
            index,
            dartq_semantic::StrictModes::default(),
            &dartq_resolution::LibraryClassModifiers::default(),
        );
        let unit = Unit {
            source: &self.source,
            tokens: &self.tokens,
            ast: &self.ast,
            lines: &self.lines,
            path: Some(&self.path),
            features: self.features,
            version: self.version,
            workspace: None,
            reachability: None,
            async_context_uses: None,
            library_type_uses: None,
        };
        unit.run_with_semantics(rules, &rustc_hash::FxHashSet::default(), Some(&analysis))
    }
}

/// Wraps a `tree` closure so the compilation-unit root is implicit.
pub fn unit_of(
    tree: impl FnOnce(&mut AstBuilder, &TokenTable),
) -> impl FnOnce(&mut AstBuilder, &TokenTable) {
    |b: &mut AstBuilder, t: &TokenTable| {
        b.begin(NodeKind::CompilationUnit, 0);
        tree(b, t);
        b.end(NodeKind::CompilationUnit, narrow_u32(t.len()));
    }
}

/// Convenience: enable one rule by name.
pub fn one_rule(name: &str) -> EnableSet {
    let (set, unknown) = EnableSet::resolve([name]);
    assert!(unknown.is_empty(), "unknown rule {name}");
    set
}

/// A convenience absent-token constant for tree builders.
pub const NONE: u32 = TOKEN_NONE;

/// The identifier-context payload word the test trees stamp (the rules read
/// the token, never the context).
pub const EXPR_CTX: u32 = dartq_parser::IdentifierContext::Expression as u32;

/// Builds an `Identifier` node spanning `[first, end)` (Fielded: the context
/// word must be patched, builder contract — named through the payload-field
/// table, so a wrong field fails the builder's debug assert at build time).
pub fn identifier(b: &mut AstBuilder, first: u32, end: u32) {
    b.begin(NodeKind::Identifier, first);
    b.patch_field(PayloadField::Context, EXPR_CTX);
    b.end(NodeKind::Identifier, end);
}

/// Index of the `nth` (0-based) token whose lexeme is `needle` — tree
/// builders address tokens by index; lexeme addressing keeps the tree
/// closures readable against the source snippet above them.
pub fn tok(source: &str, tokens: &TokenTable, needle: &str, nth: usize) -> u32 {
    let mut seen = 0;
    for i in 0..tokens.as_slice().len() {
        if tokens.lexeme(source, i) == needle {
            if seen == nth {
                return narrow_u32(i);
            }
            seen += 1;
        }
    }
    panic!("no token `{needle}`#{nth} in {source:?}");
}

/// UTF-16 offset of token `index` in the fixture.
pub fn utf16_at(fixture: &Fixture, index: u32) -> u32 {
    fixture
        .tokens
        .get(index as usize)
        .expect("token index")
        .utf16_offset
}

#[cfg(test)]
mod parse_tests {
    use super::*;
    use crate::registry::rule_by_name;

    /// The real-bridge fixture: rows come out of the production parser, not
    /// a hand-built tree.
    #[test]
    fn parse_fixture_runs_rules_over_the_real_tree() {
        let fixture = Fixture::parse("void f() {\n  if (b) ; else ;\n}\n");
        let lints = fixture.run(&one_rule("avoid_empty_else"));
        assert_eq!(lints.len(), 1, "the else `;` reports through the bridge");
        assert_eq!(
            lints[0].rule,
            rule_by_name("avoid_empty_else").expect("wired")
        );
    }

    /// The double-case switch regression: `end_switch_case` snapshots the
    /// next case's material start AFTER its own seal. A pre-seal snapshot
    /// pointed at the just-sealed `SwitchCase` row itself, so the second
    /// case's `begin_switch_case` anchored strictly inside the first case's
    /// subtree — tripping the builder's well-nestedness assert in debug
    /// builds and nesting the cases in release builds. Single-case switches
    /// never consume the stale snapshot, which is why parser-only suites
    /// stayed green.
    #[test]
    fn parse_fixture_double_case_switch_builds_sibling_cases() {
        let fixture = Fixture::parse(
            "void f(int x) {\n  switch (x) {\n    case 1: break;\n    case 2: break;\n  }\n}\n",
        );
        let ast = &fixture.ast;
        let cases: Vec<u32> = (0..ast.len())
            .filter(|&row| ast.kind(row as u32) == dartq_ast::NodeKind::SwitchCase)
            .map(|row| row as u32)
            .collect();
        assert_eq!(cases.len(), 2, "two switch cases: {cases:?}");
        let first = ast.children(cases[0]);
        let second = ast.children(cases[1]);
        assert!(
            first.end <= second.start,
            "case 2 must not adopt case 1: {first:?} vs {second:?}"
        );
    }
}
