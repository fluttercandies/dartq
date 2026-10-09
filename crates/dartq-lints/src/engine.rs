//! The run entry: one traversal over the arena dispatching enabled rules,
//! suppression applied at the reporting face.
//!
//! Traversal is an explicit-stack DFS (no structural recursion — the engine's
//! termination invariant holds on any input), parents before children, in
//! source order. Rules see the node through [`Check`], which carries the
//! ancestor path so parent questions (`!(x is T)`, else-branch shape) cost
//! one slice read, never the arena's forward-walk `parent`.

use std::path::Path;
use std::sync::LazyLock;

use dartq_ast::{NodeKind, SyntaxTable, byte_span, utf16_span};
use dartq_scanner::TokenTable;
use dartq_source::LineIndex;

use crate::comments::Comments;
use crate::ignore::IgnoreInfo;
use crate::registry::{EnableSet, RULES, RuleId};

/// The language-feature bits a rule may query. Features the caller does not
/// know are treated as enabled (the Dart 3.7+ shipped defaults); the caller
/// pins the bits it wants to control (e.g. from `// @dart=` / experiment
/// config).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Features {
    /// `wildcard_variables` (shipped in Dart 3.7): a lone `_` parameter name
    /// is exempt from `avoid_shadowing_type_parameters`
    /// (avoid_shadowing_type_parameters.dart:34-37).
    pub wildcard_variables: bool,
}

impl Default for Features {
    fn default() -> Self {
        Self {
            wildcard_variables: true,
        }
    }
}

/// The substrate facts one run reads. All references must stay alive for the
/// whole run; the AST/token table must be the unified post-parse pair (the
/// `dartq_ast::byte_span` sequencing contract).
pub struct Unit<'a> {
    pub source: &'a str,
    pub tokens: &'a TokenTable,
    pub ast: &'a SyntaxTable,
    /// Byte↔line mapping used by suppression line attribution.
    pub lines: &'a LineIndex,
    /// The analyzed file path — borrowed, never copied. `file_names` reads
    /// the basename (official `source.shortName`,
    /// file_names.dart:31-41). `None` (a run without a path) skips the
    /// path-dependent rules, mirroring the official `element != null` guard.
    pub path: Option<&'a Path>,
    pub features: Features,
}

/// One reported lint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lint {
    pub rule: RuleId,
    /// UTF-16 half-open range of the reported node/token.
    pub range: core::ops::Range<u32>,
    /// Rendered `problemMessage`.
    pub message: String,
    /// Rendered `correctionMessage`, when the official code provides one.
    pub correction: Option<String>,
}

/// Per-file output fuse (the parser's diagnostic cap, applied to lints):
/// past this many hits a run stops reporting. Structural bound on the sink.
const MAX_LINTS_PER_FILE: usize = 10_000;

/// Kind -> rule dispatch. Built once from the static table; a node whose
/// kind has no registered rules costs one empty-Vec test.
static DISPATCH: LazyLock<[Vec<u16>; NodeKind::KIND_COUNT as usize]> = LazyLock::new(|| {
    let mut table: [Vec<u16>; NodeKind::KIND_COUNT as usize] = std::array::from_fn(|_| Vec::new());
    for (i, spec) in RULES.iter().enumerate() {
        for kind in spec.kinds {
            table[*kind as usize].push(i as u16);
        }
    }
    table
});

/// Runs the enabled rules over one unit. Output is in traversal order:
/// pre-order (a node before its children), registry order within a node —
/// the official visitor dispatch order.
///
/// Traversal visits every node exactly once. The arena's `children` range is
/// a completion window (descendants complete inside it, before their
/// ancestor), so the walk extracts *direct* children with the subtree-tiling
/// scan in [`push_direct`]: a child's whole subtree occupies
/// `[children(child).start, child]`, contiguous, ending at the child
/// itself — the next direct sibling starts right after.
#[must_use]
pub fn run(unit: &Unit<'_>, enabled: &EnableSet) -> Vec<Lint> {
    let root = match unit.ast.root() {
        Some(root) if enabled != &EnableSet::none() => root,
        _ => return Vec::new(),
    };
    let comments = Comments::extract(unit.source, unit.tokens);
    let ignore = IgnoreInfo::parse(unit.source, &comments);

    let mut out = Vec::new();

    // Explicit-stack DFS (no structural recursion — the engine's termination
    // invariant holds on any input). The stack holds `(node, depth)` where
    // `depth` is the ancestor-path length the node belongs at; `path` is the
    // shared chain the rules read. A single scratch buffer collects direct
    // children per node (one allocation for the whole run).
    let mut stack: Vec<(u32, usize)> = Vec::new();
    let mut path: Vec<u32> = vec![root];
    let direct = core::cell::RefCell::new(Vec::new());
    dispatch(unit, &comments, &ignore, &path, enabled, &direct, &mut out);
    push_direct(unit.ast, root, &mut stack, &direct, path.len());

    while let Some((node, depth)) = stack.pop() {
        path.truncate(depth);
        path.push(node);
        dispatch(unit, &comments, &ignore, &path, enabled, &direct, &mut out);
        push_direct(unit.ast, node, &mut stack, &direct, path.len());
    }

    out
}

/// Pushes the direct children of `id` onto the work stack, last child first
/// so pops yield source order. Direct children are read off the completion
/// window by the subtree tiling: entries descend by whole subtrees, each
/// subtree ending at the ancestor child itself. `scratch` is the run's
/// shared buffer (the rules borrow it through `Check` between calls).
fn push_direct(
    ast: &SyntaxTable,
    id: u32,
    stack: &mut Vec<(u32, usize)>,
    scratch: &core::cell::RefCell<Vec<u32>>,
    depth: usize,
) {
    let mut scratch = scratch.borrow_mut();
    scratch.clear();
    let window = ast.children(id);
    let mut i = window.end;
    while i > window.start {
        i -= 1;
        scratch.push(i);
        i = ast.children(i).start;
    }
    // Descending (last child first); pushed as-is, the last push is the
    // first child, so pops yield source order.
    stack.extend(scratch.drain(..).map(|child| (child, depth)));
}

/// Dispatches the rules registered for the path's last node.
fn dispatch(
    unit: &Unit<'_>,
    comments: &Comments,
    ignore: &IgnoreInfo,
    path: &[u32],
    enabled: &EnableSet,
    direct: &core::cell::RefCell<Vec<u32>>,
    out: &mut Vec<Lint>,
) {
    let current = *path.last().expect("dispatch on a node");
    for &rule in DISPATCH[unit.ast.kind(current) as usize]
        .iter()
        .filter(|rule| enabled.is_enabled(RuleId(**rule)))
    {
        if out.len() >= MAX_LINTS_PER_FILE {
            return;
        }
        let mut check = Check {
            unit,
            comments,
            ignore,
            path,
            rule: RuleId(rule),
            direct,
            out: &mut *out,
        };
        (RULES[rule as usize].check)(&mut check);
    }
}

/// What a rule sees: the unit, its comments/suppression table, the ancestor
/// path (current node last), and the reporting face. One lifetime: every
/// field is re-borrowed to the dispatch-call scope, so each rule gets a
/// fresh check per node.
pub struct Check<'a> {
    pub unit: &'a Unit<'a>,
    pub comments: &'a Comments,
    pub ignore: &'a IgnoreInfo,
    /// Ancestor path, root first, current node last.
    pub path: &'a [u32],
    /// The rule being dispatched.
    pub rule: RuleId,
    /// Shared direct-children scratch (one buffer per run, refilled per
    /// call — no per-node allocation).
    direct: &'a core::cell::RefCell<Vec<u32>>,
    out: &'a mut Vec<Lint>,
}

/// Fills `buf` with the direct children of `id`, ascending, via the subtree
/// tiling of the completion window (see [`run`]).
fn fill_direct(ast: &SyntaxTable, id: u32, buf: &mut Vec<u32>) {
    buf.clear();
    let window = ast.children(id);
    let mut i = window.end;
    while i > window.start {
        i -= 1;
        buf.push(i);
        i = ast.children(i).start;
    }
    buf.reverse();
}

impl<'a> Check<'a> {
    /// The current node.
    #[must_use]
    pub fn node(&self) -> u32 {
        *self.path.last().expect("check always has a current node")
    }

    /// The `i`-th ancestor from the current node (`0` = current, `1` =
    /// parent, ...), `None` past the root.
    #[must_use]
    pub fn ancestor(&self, depth: usize) -> Option<u32> {
        self.path.len().checked_sub(depth + 1).map(|i| self.path[i])
    }

    #[must_use]
    pub fn kind(&self, id: u32) -> NodeKind {
        self.unit.ast.kind(id)
    }

    /// The completion window of `id` — node ids whose construction happened
    /// inside `id`'s begin..end. Contains descendants; use
    /// [`Check::direct_children`] when direct children are meant (or the
    /// window-O(1) forms: an empty window has no descendants, and the last
    /// window entry is always the last direct child).
    #[must_use]
    pub fn children(&self, id: u32) -> core::ops::Range<u32> {
        self.unit.ast.children(id)
    }

    /// The direct children of `id`, ascending, in the shared scratch. The
    /// borrow lives until the returned guard drops; a nested
    /// `direct_children` call while a guard is alive panics — nest through
    /// [`Check::collect_direct`] instead.
    pub fn direct_children(&self, id: u32) -> core::cell::Ref<'_, Vec<u32>> {
        fill_direct(self.unit.ast, id, &mut self.direct.borrow_mut());
        self.direct.borrow()
    }

    /// [`Check::direct_children`] as an owned vec, for the rare nested scans.
    pub fn collect_direct(&self, id: u32) -> Vec<u32> {
        let mut buf = Vec::new();
        fill_direct(self.unit.ast, id, &mut buf);
        buf
    }

    #[must_use]
    pub fn payload(&self, id: u32) -> &[u32] {
        self.unit.ast.payload(id)
    }

    /// The token id at the head of `id`'s span.
    #[must_use]
    pub fn head_token(&self, id: u32) -> u32 {
        self.unit.ast.token_range(id).start
    }

    #[must_use]
    pub fn lexeme(&self, token: u32) -> &'a str {
        self.unit.tokens.lexeme(self.unit.source, token as usize)
    }

    #[must_use]
    pub fn token_is_synthetic(&self, token: u32) -> bool {
        self.unit
            .tokens
            .get(token as usize)
            .is_some_and(dartq_scanner::Token::is_synthetic)
    }

    /// Byte span of `id` (raw source coordinates, for slicing lexemes).
    #[must_use]
    pub fn byte_span(&self, id: u32) -> core::ops::Range<usize> {
        byte_span(self.unit.tokens, self.unit.ast, id)
    }

    /// UTF-16 span of `id` — the engine's external coordinate convention.
    #[must_use]
    pub fn utf16_span(&self, id: u32) -> core::ops::Range<u32> {
        utf16_span(self.unit.source, self.unit.tokens, self.unit.ast, id)
    }

    /// Reports the rule's official code at `range` (UTF-16), filling the
    /// `{n}` slots with `args`. Suppressed hits are dropped here
    /// (`IgnoreInfo.ignored`, line of the hit's start).
    pub fn report(&mut self, range: core::ops::Range<u32>, args: &[&str]) {
        let spec = RULES[self.rule.0 as usize];
        self.emit(range, spec.problem, spec.correction, args);
    }

    /// Reports with explicit texts — for the rules whose official code has
    /// two message variants under one name (field/local splits). Texts are
    /// passed verbatim from the official templates.
    pub fn report_texts(
        &mut self,
        range: core::ops::Range<u32>,
        problem: &str,
        correction: &str,
        args: &[&str],
    ) {
        self.emit(range, problem, correction, args);
    }

    fn emit(
        &mut self,
        range: core::ops::Range<u32>,
        problem: &str,
        correction: &str,
        args: &[&str],
    ) {
        let name = RULES[self.rule.0 as usize].name;
        let (line, _) = self
            .unit
            .lines
            .line_col_utf16(self.unit.source, range.start);
        if self.ignore.ignored(line, name) {
            return;
        }
        self.out.push(Lint {
            rule: self.rule,
            range,
            message: crate::registry::render(problem, args),
            correction: (!correction.is_empty()).then(|| crate::registry::render(correction, args)),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::rule_by_name;
    use crate::testsupport::{CONFIG, Fixture, unit_of};
    use dartq_ast::{AstBuilder, NodeKind};
    use dartq_scanner::scan;
    use dartq_source::FileTable;
    use std::path::Path;

    #[test]
    fn traversal_is_pre_order_and_reports_suppress() {
        // Two hits in one unit; `// ignore:` above the second line drops
        // only that hit. Comments are not tokenized, so the token table is
        // if(0)((1)b(2))(3);(4)else(5);(6) | if(7)((8)b(9))(10);(11)else(12);(13).
        let source = "if (b) ; else ;\n// ignore: avoid_empty_else\nif (b) ; else ;\n";
        let mut files = FileTable::default();
        let scanned = scan(source, CONFIG, files.intern(Path::new("t.dart")));
        let tokens = scanned.tokens;
        // Trees: two if-statements, each with an empty else.
        let mut b = AstBuilder::new();
        b.begin(NodeKind::CompilationUnit, 0);
        // if #1: if(0) ((1) b(2) )(3) ;(4) else(5) ;(6)
        b.begin(NodeKind::IfStatement, 0);
        b.patch(0, 5);
        b.begin(NodeKind::ParenthesizedCondition, 1);
        b.patch(0, 1);
        b.patch(1, 3);
        b.end(NodeKind::ParenthesizedCondition, 4);
        b.leaf(NodeKind::EmptyStatement, 4);
        b.leaf(NodeKind::EmptyStatement, 6);
        b.end(NodeKind::IfStatement, 7);
        // if #2: if(7) ((8) b(9) )(10) ;(11) else(12) ;(13)
        b.begin(NodeKind::IfStatement, 7);
        b.patch(0, 12);
        b.begin(NodeKind::ParenthesizedCondition, 8);
        b.patch(0, 8);
        b.patch(1, 10);
        b.end(NodeKind::ParenthesizedCondition, 11);
        b.leaf(NodeKind::EmptyStatement, 11);
        b.leaf(NodeKind::EmptyStatement, 13);
        b.end(NodeKind::IfStatement, 14);
        b.end(NodeKind::CompilationUnit, 14);
        let ast = b.finish();
        let lines = LineIndex::new(source);

        let unit = Unit {
            source,
            tokens: &tokens,
            ast: &ast,
            lines: &lines,
            path: None,
            features: Features::default(),
        };
        let id = rule_by_name("avoid_empty_else").unwrap();
        let mut enabled = EnableSet::none();
        enabled.add(id);
        let lints = run(&unit, &enabled);
        assert_eq!(lints.len(), 1, "the suppressed else must not report");
        assert_eq!(lints[0].rule, id);
        // The surviving hit is the FIRST if's else (`;` at token 6):
        // pre-order output.
        assert_eq!(lints[0].range.start, utf16_offset(&tokens, 6));
    }

    fn utf16_offset(tokens: &TokenTable, index: usize) -> u32 {
        tokens.get(index).expect("token").utf16_offset
    }

    #[test]
    fn empty_enable_set_is_a_no_op() {
        let f = Fixture::build("if (b) ;", unit_of(|_, _| {}));
        assert!(f.run(&EnableSet::none()).is_empty());
    }

    #[test]
    fn unknown_names_are_reported_back() {
        let (set, unknown) = EnableSet::resolve(["avoid_empty_else", "not_a_lint"]);
        assert_eq!(unknown, vec!["not_a_lint"]);
        assert!(set.is_enabled(rule_by_name("avoid_empty_else").unwrap()));
    }
}
