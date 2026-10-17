//! The syntactic const-ability face shared by the const-family rules
//! (upstream `prefer_const_constructors`,
//! `prefer_const_literals_to_create_immutables`,
//! `avoid_redundant_argument_values`, `use_named_constants` — their
//! `canBeConst` gate is the analyzer's constant machinery,
//! `upstream Dart SDK source, pkg/analyzer/lib/src/lint/constants.dart`).
//!
//! # The M1 cut: no evaluator
//!
//! The upstream `canBeConst` imagines a `const` keyword at the node and
//! runs the resolved constant verifier. dartq answers the same question
//! from the written shape alone: a recursive predicate over the arena rows
//! (literal faces, const-able subexpressions, same-file const
//! constructors, identifiers bound to same-file const declarations), with
//! zero constant folding and zero semantic-face reads. Two disciplines
//! govern it:
//!
//! - **Under-report on doubt**: every bail is `false`. A name that does
//!   not resolve same-file (imports, SDK types, prefixed names), an
//!   interpolation, a spread or loop element, `??`, closures, cascades —
//!   all answer `false`, so a consuming rule stays silent rather than
//!   reporting a non-const candidate. The accepted residue is documented
//!   at its faces: operators legal in constant expressions are judged
//!   without operand types (`foo + bar` over strings bails only when an
//!   operand is a written string literal), extension-type creations
//!   without a written `const` constructor bail, and a written `const`
//!   wrapper answers `true` without validating its contents (the validity
//!   of an already-written constant is the compiler's question).
//! - **Same-file resolution**: names resolve against the unit's own
//!   declarations. Cross-file const constructors and named constants
//!   under-report — the divergence the wave accepts in exchange for the
//!   evaluator cut.
//!
//! # Face
//!
//! The entry points take [`Unit`], not `Check`: the predicate is a pure
//! query that needs no dispatch state (no path, no reporting, no
//! suppression, no run caches), and a `Unit` is reachable wherever a rule
//! can read `c.unit` — including tests. Resolution builds a same-file
//! index lazily per call and threads it through the recursion; if a
//! differential run ever flags the per-candidate rebuild, the hoist is
//! the consuming ticket's change, not a face change.
//!
//! Production tiling is read the `slice` way: childless send pieces tile
//! flat into the enclosing owner's window, const wrappers keep their
//! content inside their own window, and collection entries may leak into
//! the surrounding sibling run — the row reads here reconstruct runs by
//! span containment over completion windows and never trust a single
//! parent link. The module's pins parse through the production bridge
//! ([`crate::testsupport::Fixture::parse`]) for exactly that reason.

use dartq_ast::{NodeKind, mod_flags};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::engine::Unit;

/// The member modifier-set bit for `static` (the parser lane's
/// `member_bits` encoding, read through [`mod_flags`]).
const STATIC_BIT: u16 = 1 << 3;
/// The lexemes a declaration's modifier run may spell. The scan stops at
/// the first token outside this set, so a run is bounded by its own
/// grammar (at most `static const` / `late final` / …).
const MODIFIER_LEXEMES: [&str; 8] = [
    "augment",
    "external",
    "static",
    "covariant",
    "late",
    "const",
    "final",
    "var",
];

/// Whether the expression row `expr` can be written as a `const`: the
/// syntactic stand-in for the official `canBeConst` (module docs). A row
/// whose shape carries no const face — or whose resolution leaves the
/// file — answers `false` (the under-report).
#[must_use]
pub(crate) fn is_const_able(unit: &Unit<'_>, expr: u32) -> bool {
    let index = ConstIndex::build(unit);
    able(unit, &index, expr)
}

/// The named-constant alias of the const-written creation `creation`: the
/// structural identity proxy of `use_named_constants` (upstream Dart SDK
/// source, `pkg/linter/lib/src/rules/use_named_constants.dart`) — the
/// creation's written shape compared, token for token, against the
/// `static const` initializers of the same-file class it constructs;
/// values are never computed. Returns `(type name, field name)` of the
/// first matching field in declaration order.
///
/// The upstream gates hold in the proxy: the creation must carry a
/// written `const` (`node.isConst`, :50/:59 — pass the const-carrying
/// row, a `ConstExpression`/`ConstLiteral` wrapper; a bare creation
/// answers `None`), the type must be a same-file class or extension type
/// (:64-67), and a creation sitting in a static field initializer of its
/// own class stays silent (:70-77 — `static const a = A(); static const
/// b = A();` never reports). The comparison unwraps the wrapper first, so
/// the leading `const` keyword never breaks the shape equality (`const
/// Color(1)` matches `static const red = Color(1)`). Cross-file classes
/// under-report.
#[must_use]
pub(crate) fn matches_named_constant<'a>(
    unit: &'a Unit<'_>,
    creation: u32,
) -> Option<(&'a str, &'a str)> {
    let inner = match unit.ast.kind(creation) {
        NodeKind::ConstExpression | NodeKind::ConstLiteral => inner_creation(unit, creation)?,
        _ => return None,
    };
    let index = ConstIndex::build(unit);
    let (type_name, extent) = creation_face(unit, inner)?;
    let class_row = index.types.get(type_name).copied()?;
    if !matches!(
        unit.ast.kind(class_row),
        NodeKind::ClassDeclaration | NodeKind::ExtensionTypeDeclaration
    ) {
        return None;
    }
    // The self-field guard: a creation inside a static initializer of
    // its own class is that class's own constant being written, not a
    // use.
    if enclosing_static_field_class(unit, creation) == Some(class_row) {
        return None;
    }
    for member in class_members(unit, class_row) {
        if unit.ast.kind(member) != NodeKind::Fields
            || mod_flags(unit.ast, member) & STATIC_BIT == 0
        {
            continue;
        }
        for decl in child_rows(unit, member) {
            if unit.ast.kind(decl) != NodeKind::VariablesDeclaration || !spells_const(unit, decl) {
                continue;
            }
            for field in child_rows(unit, decl) {
                if unit.ast.kind(field) != NodeKind::InitializedIdentifier {
                    continue;
                }
                if initializer_matches(unit, field, extent.clone()) {
                    let name = head_token(unit, field);
                    return Some((type_name, lexeme(unit, name)));
                }
            }
        }
    }
    None
}

/// The unit's same-file const resolution index: class-like and enum names
/// to their declaration rows, and top-level `const` variable names to
/// their `VariablesDeclaration` rows. Built once per public call (the
/// module docs carry the hoist posture).
struct ConstIndex<'a> {
    types: FxHashMap<&'a str, u32>,
    consts: FxHashMap<&'a str, u32>,
}

impl<'a> ConstIndex<'a> {
    fn build(unit: &Unit<'a>) -> Self {
        let mut index = Self {
            types: FxHashMap::default(),
            consts: FxHashMap::default(),
        };
        let Some(root) = unit.ast.root() else {
            return index;
        };
        for top in child_rows(unit, root) {
            for kid in child_rows(unit, top) {
                match unit.ast.kind(kid) {
                    NodeKind::ClassDeclaration
                    | NodeKind::MixinDeclaration
                    | NodeKind::NamedMixinApplication
                    | NodeKind::ExtensionTypeDeclaration
                    | NodeKind::EnumDeclaration => {
                        if let Some(name) = declaration_name(unit, kid) {
                            index.types.entry(name).or_insert(kid);
                        }
                    }
                    NodeKind::TopLevelFields => {
                        for decl in child_rows(unit, kid) {
                            if unit.ast.kind(decl) != NodeKind::VariablesDeclaration
                                || !spells_const(unit, decl)
                            {
                                continue;
                            }
                            for field in child_rows(unit, decl) {
                                if let Some(name) = top_level_const_name(unit, field) {
                                    index.consts.entry(name).or_insert(decl);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        index
    }
}

/// The per-row const-ability verdict. Termination is structural: every
/// recursion step descends into rows whose spans nest strictly inside the
/// current row (the extent readers exclude the row itself), so a cycle
/// would need a row containing itself.
#[expect(
    clippy::match_same_arms,
    reason = "each arm mirrors one class of the official rule's const               taxonomy; merging the two true-verdict groups would erase the               documented structure"
)]
fn able(unit: &Unit<'_>, index: &ConstIndex<'_>, row: u32) -> bool {
    match unit.ast.kind(row) {
        // Const by written shape: the token leaves, the symbol face, and
        // the written `const` wrappers.
        NodeKind::LiteralBool
        | NodeKind::LiteralInt
        | NodeKind::LiteralDouble
        | NodeKind::LiteralNull
        | NodeKind::SymbolLiteral
        | NodeKind::ConstExpression
        | NodeKind::ConstLiteral
        | NodeKind::ConstDotShorthand => true,
        NodeKind::StringLiteral => string_able(unit, row),
        // Collections and records (parenthesized contents land as the
        // `ParenthesizedExpression` seal of the canonical
        // `RecordLiteral` open): every entry row inside the extent is
        // judged — spreads, loop elements and null-aware elements bail in
        // the catch-all below, `if` elements through their condition and
        // arms (the `IfControlFlow` row's extent covers the
        // `ParenthesizedCondition` and both arms).
        NodeKind::ListLiteral
        | NodeKind::SetOrMapLiteral
        | NodeKind::RecordLiteral
        | NodeKind::ParenthesizedExpression
        | NodeKind::IfControlFlow
        | NodeKind::Conditional
        | NodeKind::AsExpression => extent_able(unit, index, row),
        NodeKind::NewExpression | NodeKind::ImplicitCreation => creation_able(unit, index, row),
        NodeKind::Send => send_able(unit, index, row),
        // Operator faces: the const-legal operator set, then the operands
        // through the extent (they tile inside the operator row's own
        // window).
        NodeKind::BinaryExpression => binary_able(unit, index, row),
        NodeKind::UnaryPrefix => unary_able(unit, index, row),
        // Transparent rows: containers and wrappers whose contents are
        // separate extent rows (judged there), and the identifier rows a
        // send group or creation face already owns. A plain identifier is
        // always a group piece in expression position, so it never
        // carries its own verdict.
        NodeKind::Type
        | NodeKind::NoType
        | NodeKind::VoidType
        | NodeKind::FunctionType
        | NodeKind::RecordType
        | NodeKind::TypeArguments
        | NodeKind::NoTypeArguments
        | NodeKind::TypeVariable
        | NodeKind::TypeVariables
        | NodeKind::Arguments
        | NodeKind::NoArguments
        | NodeKind::NamedArgument
        | NodeKind::MapEntry
        | NodeKind::NamedRecordField
        | NodeKind::InitializedIdentifier
        | NodeKind::VariableInitializer
        | NodeKind::ParenthesizedCondition
        | NodeKind::ThenControlFlow
        | NodeKind::ElseControlFlow
        | NodeKind::DotAccess
        | NodeKind::Qualified
        | NodeKind::Identifier
        | NodeKind::NoIdentifier
        | NodeKind::FunctionName => true,
        // Every other shape — assignments, awaits, throws, closures,
        // cascades, indexing, spreads, loop and null-aware elements,
        // constructor references, postfix mutations, recovery sentinels —
        // is not a constant expression by shape.
        _ => false,
    }
}

/// The extent verdict: every row inside `row`'s token extent (its
/// completion window plus the surrounding sibling run, span-filtered) is
/// const-able. The sibling scan covers the production tiling's leaked
/// entries (the `literal_faces` posture in `slice.rs`).
fn extent_able(unit: &Unit<'_>, index: &ConstIndex<'_>, row: u32) -> bool {
    rows_within(unit, row)
        .into_iter()
        .all(|inner| able(unit, index, inner))
}

/// The rows nesting inside `row`'s token extent: the row's and its
/// parent's completion windows, filtered by span containment and
/// deduplicated. The parent scan covers the tiling's leak faces (rows
/// sealing beside the literal row).
fn rows_within(unit: &Unit<'_>, row: u32) -> Vec<u32> {
    let span = unit.ast.token_range(row);
    let mut seen = FxHashSet::default();
    let mut out = Vec::new();
    let mut current = Some(row);
    for _ in 0..2 {
        let Some(node) = current else { break };
        let window = unit.ast.children(node);
        for candidate in window.start..window.end {
            if candidate != row && seen.insert(candidate) && nested(unit, candidate, span.clone()) {
                out.push(candidate);
            }
        }
        current = unit.ast.parent(node);
    }
    out
}

/// Whether the candidate row's span nests inside `span`.
fn nested(unit: &Unit<'_>, candidate: u32, span: core::ops::Range<u32>) -> bool {
    let other = unit.ast.token_range(candidate);
    other.start >= span.start && other.end <= span.end
}

/// The plain-string face: the literal carries no interpolation. The
/// production tiling anchors `StringLiteral` at its head `String` token
/// and splits an interpolated string into segments (`'a$v'` scans as
/// `'a` + `$` + `v` + `'`), so the row's own span covers only the head —
/// interpolation is detected by token kind, not by the row's extent: a
/// `StringInterpolationIdentifier` (`$x`) or `StringInterpolationExpression`
/// (`${…}`) token inside the literal's segment run means interpolated.
/// Adjacent plain strings tile as separate `StringLiteral` rows, so the
/// scan stops at the head's own token count plus any continuation segments
/// the interpolation opened. The escaped `\$` spelling bails too — the
/// under-report.
fn string_able(unit: &Unit<'_>, row: u32) -> bool {
    use dartq_scanner::TokenKind;
    let range = unit.ast.token_range(row);
    // Scan the literal's contiguous string run from the head: `String`
    // segments and interpolation markers only. An interpolation marker
    // anywhere in the run is the bail signal; the run ends when the token
    // kind leaves the string family.
    let mut token = range.start;
    loop {
        let Some(kind) = unit.tokens.get(token as usize).map(|t| t.kind) else {
            return false;
        };
        match kind {
            TokenKind::StringInterpolationIdentifier | TokenKind::StringInterpolationExpression => {
                return false;
            }
            TokenKind::String => {
                // A `String` segment past the head's own span belongs to an
                // interpolated tail (the head row covers only its head); it
                // is plain content, keep scanning until the run breaks.
            }
            _ => break,
        }
        // Stop once the scan has covered the row's own tokens and the next
        // token is not a string continuation — a second `StringLiteral`
        // (adjacent `'b'`) is a separate row with its own verdict.
        token += 1;
        if token >= range.end && !continues_string(unit, token) {
            break;
        }
    }
    // No interpolation marker found — but the head segment of an
    // interpolated string ends without a closing quote, so re-check the
    // head lexeme: `'a` (unterminated) means interpolated even when the
    // marker was somehow outside the scanned run.
    let head = lexeme(unit, range.start);
    string_closes(head)
}

/// Whether the token at `token` continues the current string run — a
/// `String` tail segment or an interpolation piece.
fn continues_string(unit: &Unit<'_>, token: u32) -> bool {
    use dartq_scanner::TokenKind;
    matches!(
        unit.tokens.get(token as usize).map(|t| t.kind),
        Some(
            TokenKind::String
                | TokenKind::StringInterpolationIdentifier
                | TokenKind::StringInterpolationExpression
        )
    )
}

/// Whether a `String` segment's lexeme carries its closing quote — the
/// head of an interpolated string (`'a` of `'a$v'`) ends mid-string.
fn string_closes(lexeme: &str) -> bool {
    // Triple-quoted heads open with `'''`/`"""`; single/double with the
    // one-char quote. `r`-raw strings keep the `r` prefix.
    let body = lexeme.strip_prefix('r').unwrap_or(lexeme);
    let (rest, quote) = if let Some(rest) = body.strip_prefix("'''") {
        (rest, "'''")
    } else if let Some(rest) = body.strip_prefix("\"\"\"") {
        (rest, "\"\"\"")
    } else if let Some(rest) = body.strip_prefix('\'') {
        (rest, "'")
    } else if let Some(rest) = body.strip_prefix('"') {
        (rest, "\"")
    } else {
        return true; // synthetic/empty segment — not an interpolation head
    };
    !rest.is_empty() && rest.ends_with(quote)
}

/// The binary-operator face: the operator must be legal in constant
/// expressions (`??` is not), a `+` over a written string operand is not
/// (the one typed non-const the shape proves), and the operands are
/// judged through the extent.
fn binary_able(unit: &Unit<'_>, index: &ConstIndex<'_>, row: u32) -> bool {
    let op = unit.ast.payload(row).first().copied();
    let legal = op.is_some_and(|token| const_operator(lexeme(unit, token)));
    if !legal {
        return false;
    }
    let strings = op.is_some_and(|token| lexeme(unit, token) == "+")
        && rows_within(unit, row)
            .into_iter()
            .any(|inner| unit.ast.kind(inner) == NodeKind::StringLiteral);
    !strings && extent_able(unit, index, row)
}

/// Whether the binary operator spells a constant-legal operation.
fn const_operator(op: &str) -> bool {
    matches!(
        op,
        "+" | "-"
            | "*"
            | "/"
            | "%"
            | "~/"
            | "=="
            | "<"
            | ">"
            | "<="
            | ">="
            | "&"
            | "|"
            | "^"
            | "<<"
            | ">>"
            | ">>>"
            | "&&"
            | "||"
    )
}

/// The prefix-operator face: `-`, `!` and `~` are the const-legal set
/// (`++`/`--` spell as `UnaryPrefix` too and bail here); the operand is
/// judged through the extent.
fn unary_able(unit: &Unit<'_>, index: &ConstIndex<'_>, row: u32) -> bool {
    matches!(lexeme(unit, head_token(unit, row)), "-" | "!" | "~") && extent_able(unit, index, row)
}

/// The structured creation face (`new C(...)`, the generic implicit
/// creation): the callee resolves to a same-file const constructor and
/// every argument is const-able through the argument list's extent. A
/// prefixed callee (`new p.C()`) under-reports.
fn creation_able(unit: &Unit<'_>, index: &ConstIndex<'_>, row: u32) -> bool {
    let Some((type_name, ctor_name, args)) = creation_parts(unit, row) else {
        return false;
    };
    let Some(class_row) = index.types.get(type_name).copied() else {
        return false;
    };
    const_ctor_face(unit, class_row, ctor_name)
        && args.is_some_and(|args| extent_able(unit, index, args))
}

/// The `(type name, creation token extent)` a const-written creation row
/// spells — the named-constant proxy's comparison face. The extent is the
/// callee-plus-arguments token run:
///
/// - `ImplicitCreation` covers `C<int>.new(1)` whole — its own span;
/// - `NewExpression` spans only `new` — the extent runs through the
///   `ConstructorReference` + `Arguments` siblings that complete right
///   before it in the owner run;
/// - `Send` covers its own segment whole (`C(1)`);
/// - `ConstructorReference` covers the callee only (`const C(1)` /
///   `const C<int>.new(1)` spell `const` + ref + args) — the extent runs
///   through the following `Arguments` sibling.
fn creation_face<'s>(unit: &Unit<'s>, row: u32) -> Option<(&'s str, core::ops::Range<u32>)> {
    match unit.ast.kind(row) {
        NodeKind::ImplicitCreation => {
            creation_parts(unit, row).map(|(name, _, _)| (name, unit.ast.token_range(row)))
        }
        NodeKind::NewExpression => {
            let (callee, args) = new_tail(unit, row)?;
            let name = ctor_ref_name(unit, callee)?;
            Some((name, head_token(unit, row)..unit.ast.token_range(args).end))
        }
        NodeKind::Send => {
            parse_callee(unit, row).map(|callee| (callee.name, unit.ast.token_range(row)))
        }
        NodeKind::ConstructorReference => {
            let name = ctor_ref_name(unit, row)?;
            let (run, pos) = owner_run(unit, row)?;
            let end = match run.get(pos + 1).copied() {
                Some(args) if unit.ast.kind(args) == NodeKind::Arguments => {
                    unit.ast.token_range(args).end
                }
                _ => unit.ast.token_range(row).end,
            };
            Some((name, head_token(unit, row)..end))
        }
        _ => None,
    }
}

/// The `(ConstructorReference, Arguments)` tail a `new` keyword owns.
/// Production tiling: `NewExpression` seals at the `new` keyword alone
/// (span = `new`), and its `ConstructorReference` + `Arguments` complete
/// right after it inside the enclosing statement's completion window —
/// their parent links point into the send run, not the statement, so the
/// read scans the parent's window for the rows whose spans begin exactly
/// where `new` ends. `None` for a `new` not followed by that pair.
fn new_tail(unit: &Unit<'_>, new_row: u32) -> Option<(u32, u32)> {
    let parent = unit.ast.parent(new_row)?;
    let new_end = unit.ast.token_range(new_row).end;
    let mut callee = None;
    let mut args = None;
    for candidate in unit.ast.children(parent) {
        let kind = unit.ast.kind(candidate);
        let range = unit.ast.token_range(candidate);
        match kind {
            NodeKind::ConstructorReference if range.start == new_end && callee.is_none() => {
                callee = Some(candidate);
            }
            NodeKind::Arguments if callee.is_some() && range.start > new_end && args.is_none() => {
                args = Some(candidate);
            }
            _ => {}
        }
        if callee.is_some() && args.is_some() {
            break;
        }
    }
    Some((callee?, args?))
}

/// The type name a `ConstructorReference` row spells: the first
/// `Identifier` inside its span (`C` of `C<int>.new`).
fn ctor_ref_name<'s>(unit: &Unit<'s>, row: u32) -> Option<&'s str> {
    rows_within(unit, row)
        .into_iter()
        .find(|&inner| unit.ast.kind(inner) == NodeKind::Identifier)
        .map(|id| lexeme(unit, head_token(unit, id)))
}

/// The send face: bare names, invocations and member chains over the
/// owner run (production tiling — a send chain tiles as one childless
/// `Send` leaf per selector segment: `C(1)` is one leaf; `E.a` is the
/// `E .` leaf plus the `a` leaf; `C.x()` is `C .` plus `x()` — each
/// segment's `Identifier`/sentinel pieces tile as siblings, and the
/// tail segment carries a `DotAccess` marker). Faces: a bare name is a
/// top-level `const` variable; an invocation of a bare type name is a
/// const constructor call; a member invocation of a class-like name is
/// a named const constructor; a member read is an enum value or a
/// `static const` field. Every segment of a chain answers the whole
/// chain's verdict — the extent walk judges them all.
fn send_able(unit: &Unit<'_>, index: &ConstIndex<'_>, send: u32) -> bool {
    let Some(callee) = parse_callee(unit, send) else {
        return false;
    };
    let creation_face = |selector: &str, args: u32| {
        index
            .types
            .get(callee.name)
            .copied()
            .is_some_and(|class_row| const_ctor_face(unit, class_row, selector))
            && extent_able(unit, index, args)
    };
    match (callee.args, callee.selector) {
        // Bare creation `C(...)`: a same-file const constructor.
        (Some(args), None) => creation_face(callee.name, args),
        // Member invocation `C.named(...)`: a named const constructor.
        (Some(args), Some(selector)) => creation_face(selector, args),
        // Bare name: a top-level `const` variable.
        (None, None) => index.consts.contains_key(callee.name),
        // Member read `Color.red` / `K.z`: an enum value or a `static
        // const` field.
        (None, Some(selector)) => member_const_face(unit, index, callee.name, selector),
    }
}

/// One send chain's callee as the const faces read it: the receiver head
/// name, the selector spelling (`Some` for `C.named` / `Color.red`), and
/// the tail segment's argument row (`None` for a bare read).
struct Callee<'a> {
    name: &'a str,
    selector: Option<&'a str>,
    args: Option<u32>,
}

/// The callee face of the send `send`: resolves the contiguous chain it
/// belongs to — the receiver segment is the leftmost `Send` whose span
/// butts the next one's (`E .` → `a`), the tail is the rightmost. The
/// head name is the receiver segment's span head; the selector is the
/// tail's `DotAccess` marker; the arguments sit immediately before the
/// tail send. `None` when the chain is not one of the const faces — a
/// chain past the first selector (`K.z.w`), or a headless shape.
fn parse_callee<'s>(unit: &Unit<'s>, send: u32) -> Option<Callee<'s>> {
    let (run, pos) = owner_run(unit, send)?;
    // Chain endpoints by span contiguity: each segment's span starts
    // exactly where the previous ended.
    let mut head_pos = pos;
    loop {
        let start = unit.ast.token_range(run[head_pos]).start;
        match run[..head_pos].iter().rposition(|&row| {
            unit.ast.kind(row) == NodeKind::Send && unit.ast.token_range(row).end == start
        }) {
            Some(prev) => head_pos = prev,
            None => break,
        }
    }
    let mut tail_pos = pos;
    loop {
        let end = unit.ast.token_range(run[tail_pos]).end;
        match run[tail_pos + 1..].iter().position(|&row| {
            unit.ast.kind(row) == NodeKind::Send && unit.ast.token_range(row).start == end
        }) {
            Some(off) => tail_pos += off + 1,
            None => break,
        }
    }
    // More than one receiver+selector pair is a chain past the first
    // selector — never a const face.
    let segments = {
        let (mut count, mut at) = (1usize, head_pos);
        while at != tail_pos {
            count += 1;
            at = run[at + 1..]
                .iter()
                .position(|&row| unit.ast.kind(row) == NodeKind::Send)
                .map(|off| at + off + 1)?;
        }
        count
    };
    if segments > 2 {
        return None;
    }
    let name = lexeme(unit, head_token(unit, run[head_pos]));
    let selector = match run.get(tail_pos + 1) {
        Some(&row) if unit.ast.kind(row) == NodeKind::DotAccess => selector_name(unit, row),
        _ => None,
    };
    let args = match run.get(tail_pos.checked_sub(1)?).copied() {
        Some(row) => match unit.ast.kind(row) {
            NodeKind::Arguments => Some(row),
            NodeKind::NoArguments => None,
            _ => return None,
        },
        None => return None,
    };
    Some(Callee {
        name,
        selector,
        args,
    })
}

/// The `(type name, constructor selector, arguments)` a structured
/// creation spells: the identifiers inside its extent name the type
/// first and the constructor last (`C<int>.named(1)` — the generic
/// implicit creation covers its callee whole; the `new` keyword's
/// `ConstructorReference`+`Arguments` siblings sit right before the
/// `NewExpression` row in the owner run). A prefixed callee
/// under-reports.
fn creation_parts<'s>(unit: &Unit<'s>, row: u32) -> Option<(&'s str, &'s str, Option<u32>)> {
    if unit.ast.kind(row) == NodeKind::NewExpression {
        let (callee, args) = new_tail(unit, row)?;
        let name = ctor_ref_name(unit, callee)?;
        // `new` selects the unnamed constructor — spelled as the class
        // name for `const_ctor_face`.
        return Some((name, name, Some(args)));
    }
    let mut ids = Vec::new();
    let mut args = None;
    for inner in rows_within(unit, row) {
        match unit.ast.kind(inner) {
            NodeKind::Identifier | NodeKind::NewAsIdentifier => ids.push(inner),
            NodeKind::Arguments => args = Some(inner),
            NodeKind::Qualified => return None,
            _ => {}
        }
    }
    let type_name = lexeme(unit, head_token(unit, *ids.first()?));
    let ctor_name = lexeme(unit, head_token(unit, *ids.last()?));
    Some((type_name, ctor_name, args))
}

/// The const-constructor face: the class-like body declares a `const`
/// constructor spelling `selector` — the unnamed constructor (spelled as
/// the class name, or selected by `new`) is the constructor written with
/// the class's own name. A constructor without a written `const` bails;
/// extension-type representation constructors without one under-report.
///
/// The written name read: a named constructor's selector is its second
/// `Identifier` child (`const C . x` — the span head stays at `C`), so
/// the match runs over the member's identifier children, not the head.
fn const_ctor_face(unit: &Unit<'_>, class_row: u32, selector: &str) -> bool {
    let class_name = declaration_name(unit, class_row);
    let unnamed = selector == "new" || Some(selector) == class_name;
    let Some(wanted) = (if unnamed { class_name } else { Some(selector) }) else {
        return false;
    };
    class_members(unit, class_row).into_iter().any(|member| {
        unit.ast.kind(member) == NodeKind::Constructor
            && spells_const_back(unit, member, class_name)
            && child_rows(unit, member).into_iter().any(|kid| {
                unit.ast.kind(kid) == NodeKind::Identifier
                    && lexeme(unit, head_token(unit, kid)) == wanted
            })
    })
}

/// The member-const face: `type.selector` is an enum value or a `static
/// const` field of the same-file type.
fn member_const_face(
    unit: &Unit<'_>,
    index: &ConstIndex<'_>,
    type_name: &str,
    selector: &str,
) -> bool {
    let Some(type_row) = index.types.get(type_name).copied() else {
        return false;
    };
    match unit.ast.kind(type_row) {
        NodeKind::EnumDeclaration => enum_value_names(unit, type_row).contains(&selector),
        _ => class_members(unit, type_row).into_iter().any(|member| {
            unit.ast.kind(member) == NodeKind::Fields
                && mod_flags(unit.ast, member) & STATIC_BIT != 0
                && static_const_names(unit, member).contains(&selector)
        }),
    }
}

/// The member rows of a class-like declaration (its body's children).
fn class_members(unit: &Unit<'_>, class_row: u32) -> Vec<u32> {
    child_rows(unit, class_row)
        .into_iter()
        .filter(|&row| {
            matches!(
                unit.ast.kind(row),
                NodeKind::DeclarationBody | NodeKind::EnumBody
            )
        })
        .flat_map(|body| child_rows(unit, body))
        .collect()
}

/// The selector name a `DotAccess` marker spells (the dot is payload
/// word 0; the name follows it).
fn selector_name<'s>(unit: &Unit<'s>, marker: u32) -> Option<&'s str> {
    let dot = unit.ast.payload(marker).first().copied()?;
    Some(lexeme(unit, dot + 1))
}

/// The enum's value names, in declaration order.
fn enum_value_names<'s>(unit: &Unit<'s>, enum_row: u32) -> Vec<&'s str> {
    child_rows(unit, enum_row)
        .into_iter()
        .filter(|&row| unit.ast.kind(row) == NodeKind::EnumBody)
        .flat_map(|body| child_rows(unit, body))
        .filter(|&row| unit.ast.kind(row) == NodeKind::EnumElement)
        .map(|row| lexeme(unit, head_token(unit, row)))
        .collect()
}

/// The declared names of a fields row's `const` declarators (the caller
/// has proven the `static` bit).
fn static_const_names<'s>(unit: &Unit<'s>, fields: u32) -> Vec<&'s str> {
    child_rows(unit, fields)
        .into_iter()
        .filter(|&decl| unit.ast.kind(decl) == NodeKind::VariablesDeclaration)
        .filter(|&decl| spells_const(unit, decl))
        .flat_map(|decl| child_rows(unit, decl))
        .filter(|&field| unit.ast.kind(field) == NodeKind::InitializedIdentifier)
        .map(|field| lexeme(unit, head_token(unit, field)))
        .collect()
}

/// The class-like declaration's own name — the naming `Identifier` whose
/// context payload is the declaration kind. Production tiling keeps it as
/// the row's LEFT SIBLING under the `TopLevelDeclaration` wrapper (the
/// `class_declaration_name` face in rules/mod.rs); hand-built trees may
/// nest it inside the row.
fn declaration_name<'s>(unit: &Unit<'s>, row: u32) -> Option<&'s str> {
    let is_name = |kid: u32| {
        unit.ast.kind(kid) == NodeKind::Identifier
            && matches!(
                unit.ast.payload(kid).first().copied(),
                Some(ctx)
                    if ctx
                        == dartq_parser::IdentifierContext::ClassOrMixinOrExtensionDeclaration
                            as u32
                        || ctx == dartq_parser::IdentifierContext::EnumDeclaration as u32
            )
    };
    if let Some(name) = child_rows(unit, row).into_iter().find(|&kid| is_name(kid)) {
        return Some(lexeme(unit, head_token(unit, name)));
    }
    let parent = unit.ast.parent(row)?;
    let siblings = child_rows(unit, parent);
    let pos = siblings.iter().position(|&kid| kid == row)?;
    let name = siblings[..pos]
        .iter()
        .rev()
        .copied()
        .find(|&kid| is_name(kid))?;
    Some(lexeme(unit, head_token(unit, name)))
}

/// The name of a top-level `const` declarator (the index-build caller
/// has proven the `const` spelling).
fn top_level_const_name<'s>(unit: &Unit<'s>, field: u32) -> Option<&'s str> {
    (unit.ast.kind(field) == NodeKind::InitializedIdentifier)
        .then(|| lexeme(unit, head_token(unit, field)))
}

/// Whether the declaration's modifier run spells `const` (a forward scan
/// from the span head — the run is the head-when-present modifier
/// sequence, bounded by the grammar to a couple of tokens).
fn spells_const(unit: &Unit<'_>, row: u32) -> bool {
    let mut token = head_token(unit, row);
    for _ in 0..3 {
        match lexeme(unit, token) {
            "const" => return true,
            lexeme if MODIFIER_LEXEMES.contains(&lexeme) => token += 1,
            _ => return false,
        }
    }
    false
}

/// Whether a constructor's written header spells `const`: the modifiers
/// precede the anchored name, so the scan walks backward from the span
/// head, stepping over the selector dots and the class-name spellings of
/// a named constructor (`const C.named()`).
fn spells_const_back(unit: &Unit<'_>, row: u32, class_name: Option<&str>) -> bool {
    let mut token = head_token(unit, row);
    for _ in 0..5 {
        let Some(prev) = token.checked_sub(1) else {
            return false;
        };
        token = prev;
        match lexeme(unit, token) {
            "const" => return true,
            lexeme if MODIFIER_LEXEMES.contains(&lexeme) => {}
            "." => {}
            lexeme if Some(lexeme) == class_name => {}
            _ => return false,
        }
    }
    false
}

/// Whether the field's initializer tokens spell the creation's `extent`
/// — the structural identity of the named-constant proxy (whitespace and
/// comments dropped; synthetic recovery tokens participate as written,
/// identical recovery on both sides compares equal).
fn initializer_matches(unit: &Unit<'_>, field: u32, extent: core::ops::Range<u32>) -> bool {
    let range = unit.ast.token_range(field);
    let Some(eq) = range.clone().find(|&token| lexeme(unit, token) == "=") else {
        return false;
    };
    let written = eq + 1..range.end;
    written.len() == extent.len()
        && written
            .zip(extent)
            .all(|(a, b)| a == b || lexeme(unit, a) == lexeme(unit, b))
}

/// The class-like row owning the nearest enclosing static field of
/// `row`'s position, or `None` (a local initializer, a body — the
/// upstream `nodeField?.enclosingElement` guard's miss face).
fn enclosing_static_field_class(unit: &Unit<'_>, row: u32) -> Option<u32> {
    let mut current = unit.ast.parent(row);
    while let Some(node) = current {
        if unit.ast.kind(node) == NodeKind::VariablesDeclaration {
            let mut owner = unit.ast.parent(node);
            while let Some(outer) = owner {
                if unit.ast.kind(outer) == NodeKind::Fields {
                    return if mod_flags(unit.ast, outer) & STATIC_BIT != 0 {
                        owning_class(unit, outer)
                    } else {
                        None
                    };
                }
                owner = unit.ast.parent(outer);
            }
            return None;
        }
        current = unit.ast.parent(node);
    }
    None
}

/// The class-like row a member row belongs to (the nearest ancestor of
/// the class-like kinds).
fn owning_class(unit: &Unit<'_>, member: u32) -> Option<u32> {
    let mut current = unit.ast.parent(member);
    while let Some(node) = current {
        if matches!(
            unit.ast.kind(node),
            NodeKind::ClassDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::ExtensionTypeDeclaration
                | NodeKind::EnumDeclaration
        ) {
            return Some(node);
        }
        current = unit.ast.parent(node);
    }
    None
}

/// The const-written wrapper's inner creation row: the last
/// creation-shaped row inside the wrapper's extent (the content tiles
/// flat beside the wrapper's `const` leaf — `const Color(1)` lands as a
/// `ConstructorReference` + `Arguments` run, `const C<int>.new(1)` as an
/// `ImplicitCreation`, a bare `const C(1)`-shaped send as `Send`).
fn inner_creation(unit: &Unit<'_>, wrapper: u32) -> Option<u32> {
    rows_within(unit, wrapper).into_iter().rfind(|&row| {
        matches!(
            unit.ast.kind(row),
            NodeKind::NewExpression
                | NodeKind::ImplicitCreation
                | NodeKind::Send
                | NodeKind::ConstructorReference
        )
    })
}

/// The row's owner run: the parent window's direct rows (parent-linked,
/// the clean-window posture — expression containers never carry the
/// switch-member borrow) with the row's position.
fn owner_run(unit: &Unit<'_>, row: u32) -> Option<(Vec<u32>, usize)> {
    let parent = unit.ast.parent(row)?;
    let kids = child_rows(unit, parent);
    let pos = kids.iter().position(|&kid| kid == row)?;
    Some((kids, pos))
}

/// The parent-linked direct rows of `id`, source-ascending (the completion
/// window's own order — a send's callee run reads `head … selector … args,
/// send` and `parse_callee` depends on it).
fn child_rows(unit: &Unit<'_>, id: u32) -> Vec<u32> {
    let window = unit.ast.children(id);
    (window.start..window.end)
        .filter(|&kid| unit.ast.parent(kid) == Some(id))
        .collect()
}

fn head_token(unit: &Unit<'_>, row: u32) -> u32 {
    unit.ast.token_range(row).start
}

fn lexeme<'s>(unit: &Unit<'s>, token: u32) -> &'s str {
    unit.tokens.lexeme(unit.source, token as usize)
}

#[cfg(test)]
mod w3_const_pins {
    use dartq_ast::NodeKind;

    use super::{head_token, is_const_able, lexeme, matches_named_constant};
    use crate::engine::Unit;
    use crate::engine::narrow_u32;
    use crate::testsupport::Fixture;

    /// The unit handle over a fixture — the predicate's raw face.
    fn unit(f: &Fixture) -> Unit<'_> {
        Unit {
            source: &f.source,
            tokens: &f.tokens,
            ast: &f.ast,
            lines: &f.lines,
            path: Some(&f.path),
            features: f.features,
            version: f.version,
            workspace: None,
            reachability: None,
            async_context_uses: None,
            library_type_uses: None,
        }
    }

    /// The rows of `kind`, in arena (completion = source) order.
    fn rows_of(f: &Fixture, kind: NodeKind) -> Vec<u32> {
        (0..narrow_u32(f.ast.len()))
            .filter(|&id| f.ast.kind(id) == kind)
            .collect()
    }

    /// The sends whose span head spells `name` — a bare callee's send
    /// anchors at the callee, a member invocation at the selector.
    fn sends_named(f: &Fixture, unit: &Unit<'_>, name: &str) -> Vec<u32> {
        rows_of(f, NodeKind::Send)
            .into_iter()
            .filter(|&row| lexeme(unit, head_token(unit, row)) == name)
            .collect()
    }

    /// The accept face: literals, collections, records, operators,
    /// same-file const constructors (bare, named, generic-implicit, new),
    /// const-bound identifiers, enum values and static-const members all
    /// answer true through the production bridge. Every `C(...)` send in
    /// the fixture is an accept; the structured shapes are asserted by
    /// kind.
    #[test]
    fn accept_faces_are_const_able() {
        let source = "class C { const C(); const C.x(); }\n\
                      const top = 3;\n\
                      enum E { a, b }\n\
                      class K { static const z = 7; }\n\
                      void f(bool b) {\n\
                      \x20 C(1); C(true); C(1.5); C(null); C('a'); C('a' 'b'); C(#k);\n\
                      \x20 C([1, 2]); C({'a': 1}); C({1}); C((1, 2)); C((1)); C([]);\n\
                      \x20 C([if (true) 1]); C(const [1]);\n\
                      \x20 C(-1); C(1 + 2); C(!true); C(true && false);\n\
                      \x20 C(true ? 1 : 2); C(1 == 2); C(1 | 2); C(~1); C(1 as int);\n\
                      \x20 C(top); C(E.a); C(K.z);\n\
                      \x20 C(); C.x();\n\
                      \x20 new C(); C<int>(1); C<int>.new(1);\n\
                      }\n";
        let f = Fixture::parse(source);
        let unit = unit(&f);
        for row in sends_named(&f, &unit, "C") {
            assert!(is_const_able(&unit, row), "C-send at {row} not const-able");
        }
        for row in sends_named(&f, &unit, "x") {
            assert!(is_const_able(&unit, row), "C.x() at {row} not const-able");
        }
        for row in rows_of(&f, NodeKind::NewExpression) {
            assert!(is_const_able(&unit, row), "new C() not const-able");
        }
        for row in rows_of(&f, NodeKind::ImplicitCreation) {
            assert!(is_const_able(&unit, row), "C<int>.new(1) not const-able");
        }
    }

    /// The reject face: non-const constructors, runtime calls, non-const
    /// identifiers, interpolation, spreads and loop elements, `??`/`is`/
    /// `!`/`++`, string concatenation, closures, member chains, prefixed
    /// and unresolved names — every `C(...)` send in the fixture answers
    /// false (the under-report), and the standalone shapes answer false.
    #[test]
    fn runtime_shapes_under_report() {
        let source = "class D { D(); }\n\
                      class C { const C(); }\n\
                      int v = 0;\n\
                      int g() => 0;\n\
                      class K { static const z = 7; }\n\
                      void f() {\n\
                      \x20 D();\n\
                      \x20 C(g()); C(v); C('a$v'); C([...v]); C([for (var i in v) i]);\n\
                      \x20 C(v ?? 1); C(v is int); C(v!); C(v++); C('a' + 'b');\n\
                      \x20 C(() {}); C(K.z.w); C(p.C(1)); C(Unknown(1)); C(Unknown);\n\
                      }\n";
        let f = Fixture::parse(source);
        let unit = unit(&f);
        for row in sends_named(&f, &unit, "C") {
            assert!(!is_const_able(&unit, row), "C-send at {row} must bail");
        }
        for name in ["D", "g", "p", "Unknown"] {
            for row in sends_named(&f, &unit, name) {
                assert!(!is_const_able(&unit, row), "{name}-send must bail");
            }
        }
    }

    /// The named-constant proxy: `const Color(1)` matches the class's
    /// `static const red = Color(1)` in declaration order.
    #[test]
    fn named_constant_matches_the_class_field() {
        let source = "class Color {\n\
                      \x20 const Color(this.value);\n\
                      \x20 final int value;\n\
                      \x20 static const red = Color(1);\n\
                      \x20 static const other = Color(2);\n\
                      }\n\
                      void f() {\n\
                      \x20 var a = const Color(1);\n\
                      }\n";
        let f = Fixture::parse(source);
        let unit = unit(&f);
        let wrapper = rows_of(&f, NodeKind::ConstExpression)[0];
        assert_eq!(
            matches_named_constant(&unit, wrapper),
            Some(("Color", "red"))
        );
    }

    /// The written-const and shape gates: a bare (contextually const)
    /// creation is silent, a const-written creation matches the field its
    /// spelling equals (`other`), and a different shape stays silent.
    #[test]
    fn named_constant_requires_the_const_spelling_and_shape() {
        let source = "class Color {\n\
                      \x20 const Color(this.value);\n\
                      \x20 final int value;\n\
                      \x20 static const red = Color(1);\n\
                      \x20 static const other = Color(2);\n\
                      }\n\
                      void f() {\n\
                      \x20 var a = Color(2);\n\
                      \x20 var b = const Color(2);\n\
                      \x20 var c = const Color(9);\n\
                      }\n";
        let f = Fixture::parse(source);
        let unit = unit(&f);
        let wrappers = rows_of(&f, NodeKind::ConstExpression);
        assert_eq!(wrappers.len(), 2);
        // `b`: the shape matches the second field.
        assert_eq!(
            matches_named_constant(&unit, wrappers[0]),
            Some(("Color", "other"))
        );
        // `c`: no field spells `Color(9)`.
        assert_eq!(matches_named_constant(&unit, wrappers[1]), None);
        // `a`: a bare creation never carries the written-const gate.
        let bare = sends_named(&f, &unit, "Color")[0];
        assert_eq!(matches_named_constant(&unit, bare), None);
    }

    /// The self-field guard: a const-written creation inside a static
    /// initializer of its own class stays silent, while the same spelling
    /// inside another class's static initializer matches.
    #[test]
    fn named_constant_skips_the_own_static_initializer() {
        let source = "class C {\n\
                      \x20 const C();\n\
                      \x20 static const made = C();\n\
                      }\n\
                      class User {\n\
                      \x20 static const u = const C();\n\
                      }\n\
                      void f() {\n\
                      \x20 var x = const C();\n\
                      }\n";
        let f = Fixture::parse(source);
        let unit = unit(&f);
        let wrappers = rows_of(&f, NodeKind::ConstExpression);
        assert_eq!(wrappers.len(), 2);
        // User's `u`: the same-file class field matches (different owner).
        assert_eq!(
            matches_named_constant(&unit, wrappers[0]),
            Some(("C", "made"))
        );
        // f's local: no self-field guard applies; `made` is contextually
        // const and bare, so the proxy's const-written gate holds and the
        // match comes from the wrapper.
        assert_eq!(
            matches_named_constant(&unit, wrappers[1]),
            Some(("C", "made"))
        );
        // The bare `made` initializer itself: silent (own-class face is
        // moot — it carries no written `const`).
        let made = sends_named(&f, &unit, "C")[0];
        assert_eq!(matches_named_constant(&unit, made), None);
    }

    /// The scope gate: top-level constants and non-static fields are not
    /// named-constant candidates (the upstream walk reads the class's
    /// static fields only), and a non-class type is silent.
    #[test]
    fn named_constant_walks_class_statics_only() {
        let source = "class Color {\n\
                      \x20 const Color(this.value);\n\
                      \x20 final int value;\n\
                      \x20 const also = Color(1);\n\
                      }\n\
                      const t = Color(1);\n\
                      enum EE { a }\n\
                      void f() {\n\
                      \x20 var x = const Color(1);\n\
                      }\n";
        let f = Fixture::parse(source);
        let unit = unit(&f);
        let wrapper = rows_of(&f, NodeKind::ConstExpression)[0];
        assert_eq!(matches_named_constant(&unit, wrapper), None);
    }
}
