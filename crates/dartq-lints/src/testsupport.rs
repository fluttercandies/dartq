//! Test scaffolding: scan a source snippet and hand-build its arena AST per
//! the collapse-map payload contracts (the parser→AST bridge lives in the
//! engine seam; rule tests pin the read-side contract directly).

#![cfg(test)]

use std::path::Path;

use dartq_ast::{AstBuilder, NodeKind, TOKEN_NONE};
use dartq_scanner::{scan, ScanConfig, TokenTable};
use dartq_source::{FileTable, LineIndex};

use crate::engine::{run, Lint, Unit};
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
    _files: FileTable,
}

impl Fixture {
    /// Scans `source` and builds `tree` over the resulting token table.
    pub fn build(source: &str, tree: impl FnOnce(&mut AstBuilder, &TokenTable)) -> Self {
        let mut files = FileTable::default();
        let result = scan(
            source,
            CONFIG,
            files.intern(Path::new("lint_fixture.dart")),
        );
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
