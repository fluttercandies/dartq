//! The run entry: one traversal over the arena dispatching enabled rules,
//! suppression applied at the reporting face.
//!
//! Traversal is an explicit-stack DFS (no structural recursion — the engine's
//! termination invariant holds on any input), parents before children, in
//! source order. Rules see the node through [`Check`], which carries the
//! ancestor path so parent questions (`!(x is T)`, else-branch shape) cost
//! one slice read, never the arena's forward-walk `parent`.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use dartq_ast::{NodeKind, SyntaxTable, byte_span, utf16_span};
use dartq_scanner::TokenTable;
use dartq_source::LineIndex;
use rustc_hash::FxHashMap;

use crate::comments::Comments;
use crate::ignore::IgnoreInfo;
use crate::registry::{EnableSet, RULES, RuleId, rule_by_name};
use crate::rules::SupertypeClause;
use crate::rules::expressions::same_file::Decls;

/// The language-feature bits a rule may query. Features the caller does not
/// know are treated per their shipped defaults; the caller pins the bits it
/// wants to control (e.g. from `// @dart=` / experiment config).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Features {
    /// `wildcard_variables` (shipped in Dart 3.7): a lone `_` parameter name
    /// is exempt from `avoid_shadowing_type_parameters`
    /// (avoid_shadowing_type_parameters.dart:34-37) and from the library
    /// prefix rules (library_prefixes.dart:36-39,
    /// no_leading_underscores_for_library_prefixes.dart:46-50).
    pub wildcard_variables: bool,
    /// `enhanced_parts` (experimental, unshipped — `enhanced-parts` has no
    /// `enabledIn`, `tools/experimental_features.yaml:150-152`):
    /// `use_string_in_part_of_directives` registers only without it
    /// (use_string_in_part_of_directives.dart:32-34).
    pub enhanced_parts: bool,
    /// `unnamed_libraries` (shipped in 2.19, `experimental_features.yaml:397-400`):
    /// `unnecessary_library_name` registers only with it
    /// (unnecessary_library_name.dart:32-34).
    pub unnamed_libraries: bool,
    /// `patterns` (shipped in 3.0, `experimental_features.yaml:387-389`): the
    /// bit the official AstBuilder keys switch-statement member
    /// materialization on (ast_builder.dart:1295) — with it on, every `case`
    /// of a switch statement is a `SwitchPatternCase`, so
    /// `no_duplicate_case_values` (whose visitor processes only legacy
    /// `SwitchCase` members, no_duplicate_case_values.dart:41) is silent.
    pub patterns: bool,
}

impl Default for Features {
    fn default() -> Self {
        Self {
            wildcard_variables: true,
            enhanced_parts: false,
            unnamed_libraries: true,
            patterns: true,
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
    /// The containing-package facts the pub-aware rules read
    /// ([`WorkspaceFacts`]). `None` — outside any known package, or where
    /// the containing pubspec is unparseable — is the official no-pubspec
    /// posture (`package is! PubPackage` / `pubspec == null`,
    /// depend_on_referenced_packages.dart:33-40): those rules stay silent.
    pub workspace: Option<&'a WorkspaceFacts>,
}

/// The pubspec-derived facts of the analyzed file's containing package,
/// built once per package by the mounting face (the package table's
/// attribution plus the containing pubspec's parsed dependency names).
pub struct WorkspaceFacts {
    /// The containing package's own name (official `pubspec.name`).
    pub own_package: String,
    /// The own name plus every `dependencies` entry — available to every
    /// import of the package (depend_on_referenced_packages.dart:44-48).
    pub dependencies: rustc_hash::FxHashSet<String>,
    /// Every `dev_dependencies` entry — available only outside the public
    /// dirs; the rule's check applies the `isInPublicDir` gate per unit
    /// (ast.dart:112-129: `lib/`, `bin/`, `hook/build.dart`,
    /// `hook/link.dart`).
    pub dev_dependencies: rustc_hash::FxHashSet<String>,
    /// The containing package's root (canonical) — the public-dir gates'
    /// base.
    pub root: PathBuf,
    /// The package's package-URI directory (`packageUri`, normally `lib/`):
    /// a file under it resolves to a `package:` URI —
    /// implementation_imports' registration gate
    /// (implementation_imports.dart:30-33).
    pub lib_dir: PathBuf,
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

/// A wildcard local/parameter declaration: the byte range of the `_`-name and
/// the byte range of the scope it binds over. `no_wildcard_variable_uses`
/// reads the table (with the feature off, a `_` local/parameter binds, and a
/// bare use of it reports — no_wildcard_variable_uses.dart:37-42 flags
/// Local/Formal elements only, so fields/getters never enter the table).
#[derive(Clone, Debug)]
pub struct WildcardScope {
    /// The declared name's byte range (all underscores).
    pub name: core::ops::Range<usize>,
    /// The byte range of the scope the declaration binds over.
    pub scope: core::ops::Range<usize>,
}

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

/// The run-scoped, lazily-built caches for the whole-unit declaration faces
/// (the #703 F4 hoist): before this, every candidate node re-walked all
/// top-level declarations (`same_file::scan` + `declares_name`) and every
/// field query re-walked the class body (`type_face_of_field`) —
/// O(candidates x declarations) arena walks and allocations per run. Each
/// slot is built once on first query (units a rule family never touches pay
/// nothing) and dropped with the run; no global state. Contents are token
/// tables only — name queries compare lexemes over the cached rows (the
/// O(children) read the rules are documented against), never re-walk the
/// arena.
///
/// Slot contract (the [`Check::direct_children`] materialize-first rule,
/// applied to caches): a returned borrow is a read-only view shared freely,
/// but it must not stay alive across a build of the *same* slot — the build
/// takes the slot exclusively. The accessors keep guards function-local to
/// their read, so no call site can trip this; nested-borrow panics name the
/// violation.
pub(crate) struct RunCaches {
    /// The unit's same-file declaration index
    /// ([`Decls`]), read by the annotation/shadow/creation faces.
    pub(crate) decls: core::cell::RefCell<Option<Decls>>,
    /// Class row -> its instance fields as `(name token, written type row)`
    /// in body order (`None` type row = the field is untyped). Read by
    /// [`crate::rules::type_face_of_field`].
    pub(crate) field_types: core::cell::RefCell<FxHashMap<u32, Vec<(u32, Option<u32>)>>>,
    /// Class-like row -> its written supertypes as `(name head token,
    /// clause)` pairs in source order. Read by [`crate::rules::supertypes`]
    /// (#758: the header scan ran per ancestor per ancestor-chain build —
    /// O(class subtree) each — and its `TypeArguments` skip never advanced
    /// the cursor, hanging on every generic declaration).
    pub(crate) supertype_lists: core::cell::RefCell<FxHashMap<u32, Vec<(u32, SupertypeClause)>>>,
    /// Supertype/class-like name -> the same-file class-like row, built once
    /// per run on first query (`None`-guarded like [`Self::decls`]). Read by
    /// [`crate::rules::same_file_declaration`] (#758: every lookup re-walked
    /// all top-level wrappers).
    pub(crate) class_rows: core::cell::RefCell<Option<FxHashMap<String, u32>>>,
    /// `(class-like row, concrete)` -> the transitive same-file ancestor
    /// chain in written order. Read by
    /// [`crate::rules::same_file_ancestors`] (#758: `annotate_overrides`
    /// rebuilt the identical chain per member).
    pub(crate) ancestor_chains: core::cell::RefCell<FxHashMap<(u32, bool), Vec<u32>>>,
    /// Class-like row -> its member rows ([`crate::rules::body_members`]).
    /// Read by the override-family rules, which re-derive the identical list
    /// per member (#758: each derivation filtered the whole arena by
    /// `parent`).
    pub(crate) body_member_rows: core::cell::RefCell<FxHashMap<u32, Vec<u32>>>,
}

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
    // The wildcard table is collected inside the same DFS (one `Identifier`
    // kind test per node) and only when the one rule that reads it runs —
    // the rule registers only with the wildcard feature OFF
    // (no_wildcard_variable_uses.dart:36-38).
    let no_wildcard = rule_by_name("no_wildcard_variable_uses")
        .is_some_and(|id| enabled.is_enabled(id))
        && !unit.features.wildcard_variables;
    let mut wildcards: Vec<WildcardScope> = Vec::new();

    let mut out = Vec::new();

    // Explicit-stack DFS (no structural recursion — the engine's termination
    // invariant holds on any input). The stack holds `(node, depth)` where
    // `depth` is the ancestor-path length the node belongs at; `path` is the
    // shared chain the rules read. A single scratch buffer collects direct
    // children per node (one allocation for the whole run).
    let mut stack: Vec<(u32, usize)> = Vec::new();
    let mut path: Vec<u32> = vec![root];
    let direct = core::cell::RefCell::new(Vec::new());
    let caches = RunCaches {
        decls: core::cell::RefCell::new(None),
        field_types: core::cell::RefCell::new(FxHashMap::default()),
        supertype_lists: core::cell::RefCell::new(FxHashMap::default()),
        class_rows: core::cell::RefCell::new(None),
        ancestor_chains: core::cell::RefCell::new(FxHashMap::default()),
        body_member_rows: core::cell::RefCell::new(FxHashMap::default()),
    };
    dispatch(
        unit, &comments, &ignore, &path, enabled, &direct, &caches, &wildcards, &mut out,
    );
    push_direct(unit.ast, root, &mut stack, &direct, path.len());

    while let Some((node, depth)) = stack.pop() {
        path.truncate(depth);
        path.push(node);
        if no_wildcard {
            collect_wildcard(unit, &path, &mut wildcards);
        }
        dispatch(
            unit, &comments, &ignore, &path, enabled, &direct, &caches, &wildcards, &mut out,
        );
        push_direct(unit.ast, node, &mut stack, &direct, path.len());
    }

    out
}

/// Records one wildcard local/parameter declaration. Only the three element
/// faces the official rule flags enter the table (`LocalVariableElement` /
/// `FormalParameterElement` / catch parameter — the official test's
/// `test_field`/`test_getter` faces stay out); the scope is the declaration's
/// binding extent: the innermost enclosing block/for-in/catch clause for
/// locals and catch parameters, the enclosing function for parameters.
fn collect_wildcard(unit: &Unit<'_>, path: &[u32], out: &mut Vec<WildcardScope>) {
    const LOCAL_SCOPES: [NodeKind; 3] = [NodeKind::Block, NodeKind::ForIn, NodeKind::CatchClause];
    const FUNCTION_SCOPES: [NodeKind; 7] = [
        NodeKind::Method,
        NodeKind::TopLevelMethod,
        NodeKind::Constructor,
        NodeKind::Factory,
        NodeKind::FunctionExpression,
        NodeKind::LocalFunctionDeclaration,
        NodeKind::PrimaryConstructorBody,
    ];

    let node = *path.last().expect("collect on a node");
    if unit.ast.kind(node) != NodeKind::Identifier {
        return;
    }
    // Identifier payload word 0 is the context discriminant (collapse map
    // §3.5 Identifier row).
    let Some(&ctx) = unit.ast.payload(node).first() else {
        return;
    };
    let scopes: &[NodeKind] = match ctx {
        c if c == dartq_parser::IdentifierContext::LocalVariableDeclaration as u32 => &LOCAL_SCOPES,
        c if c == dartq_parser::IdentifierContext::CatchParameter as u32 => &LOCAL_SCOPES[2..],
        c if c == dartq_parser::IdentifierContext::FormalParameterDeclaration as u32 => {
            &FUNCTION_SCOPES
        }
        _ => return,
    };
    let name_bytes = byte_span(unit.tokens, unit.ast, node);
    let name = &unit.source[name_bytes.clone()];
    if !name.bytes().all(|b| b == b'_') {
        return;
    }
    let scope = path
        .iter()
        .rev()
        .find(|&&ancestor| scopes.contains(&unit.ast.kind(ancestor)))
        .map(|&ancestor| byte_span(unit.tokens, unit.ast, ancestor));
    let Some(scope) = scope else {
        return;
    };
    out.push(WildcardScope {
        name: name_bytes,
        scope,
    });
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
#[allow(clippy::too_many_arguments)]
fn dispatch(
    unit: &Unit<'_>,
    comments: &Comments,
    ignore: &IgnoreInfo,
    path: &[u32],
    enabled: &EnableSet,
    direct: &core::cell::RefCell<Vec<u32>>,
    caches: &RunCaches,
    wildcards: &[WildcardScope],
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
            wildcards,
            rule: RuleId(rule),
            direct,
            caches,
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
    /// The wildcard local/parameter declaration table (`no_wildcard_variable_
    /// uses`; empty unless that rule runs with the wildcard feature off).
    pub wildcards: &'a [WildcardScope],
    /// The rule being dispatched.
    pub rule: RuleId,
    /// Shared direct-children scratch (one buffer per run, refilled per
    /// call — no per-node allocation).
    direct: &'a core::cell::RefCell<Vec<u32>>,
    /// The run's lazily-built declaration caches ([`RunCaches`]); the
    /// whole-unit scans the declaration rules read, built once per run.
    pub(crate) caches: &'a RunCaches,
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

    /// The direct children of `id`, ascending, in the shared scratch.
    ///
    /// # The materialize-first contract
    ///
    /// The scratch is ONE buffer per run, so the returned borrow is the only
    /// legal view of it: it must not stay alive across any nested children
    /// read — the next refill would clobber the buffer under the guard. Code
    /// whose children must survive a nested read materializes them first
    /// through [`Check::collect_direct`] (an owned copy) — the rule the
    /// rule-side comments call "collect first". Debug builds assert the
    /// contract here and name the violation; release keeps the `RefCell`
    /// guard as the backstop (the refill panics on the live borrow).
    pub fn direct_children(&self, id: u32) -> core::cell::Ref<'_, Vec<u32>> {
        debug_assert!(
            self.direct.try_borrow().is_ok(),
            "the shared children scratch is already checked out: \
             materialize with `collect_direct` before a nested children read"
        );
        fill_direct(self.unit.ast, id, &mut self.direct.borrow_mut());
        self.direct.borrow()
    }

    /// [`Check::direct_children`] as an owned vec — the materialize-first
    /// contract's escape hatch: it never touches the shared scratch, so the
    /// result survives any nested children read (and any nested
    /// `direct_children` borrow). Use it whenever children are held across a
    /// call that may itself read children; prefer [`Check::direct_children`]
    /// otherwise (no allocation).
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
            workspace: None,
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
