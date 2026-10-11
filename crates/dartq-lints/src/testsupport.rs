//! Test scaffolding: scan a source snippet and hand-build its arena AST per
//! the collapse-map payload contracts. Rule tests pin the read-side
//! contract directly against hand-built trees; [`Fixture::parse`] builds
//! through the production bridge ([`crate::parse_unit`]) for the tests
//! that exercise the real parser→AST shape.

#![cfg(test)]

use std::path::{Path, PathBuf};

use dartq_ast::{AstBuilder, NodeKind, TOKEN_NONE};
use dartq_scanner::{ScanConfig, TokenTable, scan};
use dartq_source::{FileTable, LineIndex};

use crate::engine::{Features, Lint, Unit, run};
use crate::registry::EnableSet;

pub const CONFIG: ScanConfig = ScanConfig {
    enable_triple_shift: true,
    enable_augmentations: true,
};

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
    _files: FileTable,
}

impl Fixture {
    /// Parses `source` through the production bridge (scanner → parser →
    /// arena, one transient [`crate::ParsedUnit`]). The path defaults to the
    /// fixture name; overwrite `path`/`features` after construction for the
    /// path/feature-dependent rules.
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
            _files: files,
        }
    }

    /// Runs `rules` over the fixture.
    pub fn run(&self, rules: &EnableSet) -> Vec<Lint> {
        let unit = Unit {
            source: &self.source,
            tokens: &self.tokens,
            ast: &self.ast,
            lines: &self.lines,
            path: Some(&self.path),
            features: self.features,
            workspace: None,
        };
        run(&unit, rules)
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
            workspace: Some(facts),
        };
        run(&unit, rules)
    }
}

/// Wraps a `tree` closure so the compilation-unit root is implicit.
pub fn unit_of(
    tree: impl FnOnce(&mut AstBuilder, &TokenTable),
) -> impl FnOnce(&mut AstBuilder, &TokenTable) {
    |b: &mut AstBuilder, t: &TokenTable| {
        b.begin(NodeKind::CompilationUnit, 0);
        tree(b, t);
        b.end(NodeKind::CompilationUnit, t.len() as u32);
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
/// word must be patched, builder contract).
pub fn identifier(b: &mut AstBuilder, first: u32, end: u32) {
    b.begin(NodeKind::Identifier, first);
    b.patch(0, EXPR_CTX);
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
                return i as u32;
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
}
