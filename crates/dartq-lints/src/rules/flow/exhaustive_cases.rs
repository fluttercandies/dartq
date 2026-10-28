//! Official: `pkg/linter/lib/src/rules/exhaustive_cases.dart` +
//! `asEnumLikeType` (`pkg/linter/lib/src/extensions.dart:604-647`) — a
//! switch over an enum-like type (a concrete, unsubclassed class or an
//! extension type, with only private generative constructors and at
//! least two `static const` fields of the type) must cover every
//! constant value or carry a `default`. Each case head whose constant
//! resolves to a value removes it (:49-74); a `default` member ends the
//! whole check silent (:75-77); every remaining value then reports once
//! — at the statement head through the scrutinee's `)` — spelled with
//! the group's preferred (first non-deprecated) name (:80-93). Real
//! enums never enter (the element walk's default arm — the analyzer's
//! own exhaustiveness errors cover them).
//!
//! Mounted face: the element facts ride the same-file declarative
//! channel — the scrutinee's written type resolves through
//! [`same_file::written_type`](crate::rules::expressions::same_file::written_type)
//! (the `type_literal_in_constant_pattern` idiom), the
//! candidate's constructor/field facts walk the same-file declaration
//! rows, and constant-value identity interns canonical initializer
//! texts — `computeConstantValue` same-file edition: two constants
//! share a value when their initializer chains land on equal terminal
//! texts, so `static const b = a` aliases group with `a`
//! (extensions.dart:626-642). A case head covers through the official
//! variable faces only (`Identifier` / `PropertyAccess` /
//! `DotShorthandPropertyAccess`, :59-74) — any other constant pattern
//! covers nothing.
//!
//! Known silences (the same-file under-report posture, each the
//! documented fallback of the channel it rides): a scrutinee whose type
//! leaves the file (an import, a `var`/for-in binding, a call result —
//! `written_type`'s contract), a candidate or a subclass declared in
//! another unit of the library (`asEnumLikeType` sees the whole
//! defining library, extensions.dart:259-270), and constant values
//! spelled differently over the same instance (`E._(0x1)` vs `E._(1)`).

use dartq_ast::{NodeKind, mod_flags};
use dartq_scanner::{KeywordStyle, TokenKind};

use super::{NONE_TOKEN, RuleSpec, first_child};
use crate::engine::Check;
use crate::rules::{SupertypeClause, class_declaration_name, initialized_identifier_name, member_annotations, rows_with_parent, supertypes, utf16_token_span, variables_declaration_keyword};

pub(crate) static SPEC: RuleSpec = crate::spec!(
    "exhaustive_cases",
    &[NodeKind::SwitchStatement],
    check,
);

/// The class-family `abstract` modifier bit (bridge `mod_bits`; the
/// parser lane owns the values).
const ABSTRACT_BIT: u16 = 1 << 2;

/// The alias-hop cap: a legitimate constant alias chain is a few names
/// deep; past this it is pathological and computes to nothing.
pub(super) const MAX_ALIAS_DEPTH: usize = 8;

fn check(c: &mut Check<'_>) {
    let node = c.node();
    let kids = c.collect_direct(node);
    let Some(block_pos) = kids
        .iter()
        .position(|&kid| c.kind(kid) == NodeKind::SwitchBlock)
    else {
        return;
    };
    let block = kids[block_pos];
    // The scrutinee's written type → the same-file enum-like candidate
    // (`statement.expression.staticType` → `InterfaceType.element`,
    // :39-46; a type outside the file stays silent — the channel's
    // documented fallback).
    let Some(scrutinee) = scrutinee_identifier(c, &kids[..block_pos]) else {
        return;
    };
    let name = c.lexeme(c.head_token(scrutinee));
    let Some(written) = crate::rules::expressions::same_file::written_type(c, scrutinee, name)
    else {
        return;
    };
    let Some((candidate_row, candidate_kind)) = class_like_named(
        c,
        written,
        &[
            NodeKind::ClassDeclaration,
            NodeKind::ExtensionTypeDeclaration,
        ],
    ) else {
        return;
    };
    let is_extension_type = candidate_kind == NodeKind::ExtensionTypeDeclaration;
    let constructors = constructor_face(c, candidate_row);
    let field_faces = class_const_fields(c, candidate_row);
    let mut values = ConstValues::default();
    let constants = candidate_constants(c, &mut values, &field_faces, written);
    let candidate = EnumLikeCandidate {
        is_extension_type,
        is_abstract: !is_extension_type
            && mod_flags(c.unit.ast, candidate_row) & ABSTRACT_BIT != 0,
        has_subclass_in_defining_unit: subclassed_in_file(c, candidate_row),
        constants: &constants,
        constructors: &constructors,
    };
    let cases = switch_case_rows(c, block);
    let members: Vec<SwitchMember> = cases
        .into_iter()
        .map(|case| member_face(c, &mut values, &field_faces, case))
        .collect();
    // Owned names: the report mutates the check the verdict borrowed.
    let missing: Vec<String> = check_switch(&candidate, &members)
        .into_iter()
        .map(String::from)
        .collect();
    if missing.is_empty() {
        return;
    }
    // The report span: the `switch` head through the scrutinee's `)`
    // (official `reportAtOffset(statement.offset,
    // rightParenthesis.end - statement.offset)`, :81-90).
    let start = c.utf16_span(node).start;
    let Some(rparen) = c
        .head_token(block)
        .checked_sub(1)
        .filter(|&token| token > c.head_token(node))
    else {
        return;
    };
    let range = start..utf16_token_span(c, rparen).end;
    for name in &missing {
        c.report(range.clone(), &[name]);
    }
}

/// The block's `SwitchCase` rows, source order. The plain tiling walk
/// loses members: each case's completion window starts AT the previous
/// case's arena row (`end_switch_case` snapshots the material start
/// before the previous case seals), so the reverse tiling jump lands on
/// that row and the decrement-first walk skips it as window-start
/// interior — a two-case switch surfaces only the last case. The
/// jump-landed row is this block's member, not the case's child: the
/// walk pushes it and continues below it. Shared with the
/// `control_flow_in_finally` member-label walk.
pub(super) fn switch_case_rows(c: &Check<'_>, block: u32) -> Vec<u32> {
    let window = c.children(block);
    let mut cases: Vec<u32> = Vec::new();
    let mut i = window.end;
    while i > window.start {
        i -= 1;
        let start = c.children(i).start;
        if start < i && c.kind(start) == NodeKind::SwitchCase {
            cases.push(start);
        }
        if c.kind(i) == NodeKind::SwitchCase {
            cases.push(i);
        }
        i = start;
    }
    // The start-checks push in jump order, not id order.
    cases.sort_unstable();
    cases
}

/// The bare-identifier scrutinee row: the leading expression piece
/// (`matched_value_is_type`'s proven `scrutinee.first()` face,
/// flow.rs:1011). The bridge leaks the send-group leaves past the
/// condition wrapper as owner siblings — the `Identifier`, the
/// `NoTypeArguments` leaf anchored at the follower `)`, and the `Send`
/// leaf that spans through it — so the piece run says nothing about the
/// expression shape beyond its head. Only a selector continuation
/// (a `DotAccess` piece: `a.b`, `e.multi()`) disqualifies the bare
/// name; any other scrutinee has no same-file written type and stays
/// silent through `written_type`'s `None`.
fn scrutinee_identifier(c: &Check<'_>, pieces: &[u32]) -> Option<u32> {
    let &first = pieces.first()?;
    if c.kind(first) != NodeKind::Identifier {
        return None;
    }
    let selector = pieces[1..]
        .iter()
        .any(|&piece| c.kind(piece) == NodeKind::DotAccess);
    (!selector).then_some(first)
}

/// The same-file class-like declaration named `name`: `(row, kind)` —
/// the top-level wrapper walk of `same_file_declaration` under the
/// caller's kind filter. The candidate face accepts only the
/// enum-like-eligible kinds (a class or an extension type; a same-file
/// enum is the analyzer's own exhaustiveness error, a mixin cannot
/// carry const instances); the superclass chain additionally walks
/// mixin applications.
fn class_like_named(c: &Check<'_>, name: &str, kinds: &[NodeKind]) -> Option<(u32, NodeKind)> {
    let root = c.unit.ast.root()?;
    for top in c.collect_direct(root) {
        if c.kind(top) != NodeKind::TopLevelDeclaration {
            continue;
        }
        for kid in c.collect_direct(top) {
            if kinds.contains(&c.kind(kid)) && declaration_name(c, kid) == Some(name) {
                return Some((kid, c.kind(kid)));
            }
        }
    }
    None
}

/// The written name of a class-like declaration row: the sibling
/// identifier for the class family (`class_declaration_name`), the
/// name-token payload word for an extension type (bridge
/// `begin_extension_type_declaration`).
fn declaration_name<'a>(c: &'a Check<'_>, row: u32) -> Option<&'a str> {
    if c.kind(row) == NodeKind::ExtensionTypeDeclaration {
        let name = c.payload(row).get(2).copied()?;
        return (name != NONE_TOKEN).then(|| c.lexeme(name));
    }
    class_declaration_name(c, row)
}

/// The `(is_private, is_generative)` constructor face of the candidate:
/// the declared constructors and factories plus the header primary
/// constructor (the official `constructors.every((c) => c.isPrivate &&
/// c.isGenerative)` walk, extensions.dart:620). An empty face is the
/// element model's synthesized default constructor — public, so the
/// gate fails on it.
fn constructor_face(c: &Check<'_>, candidate_row: u32) -> Vec<(bool, bool)> {
    let mut ctors = Vec::new();
    let mut push = |row: u32| match c.kind(row) {
        NodeKind::Constructor => ctors.push((constructor_name_private(c, row), true)),
        // A factory is never generative — the gate fails on it however
        // its name is spelled (:620).
        NodeKind::Factory => ctors.push((false, false)),
        // A primary constructor names through `._` (private) or not at
        // all (the public type-named form).
        NodeKind::PrimaryConstructor => {
            ctors.push((c.payload(row).get(2) == Some(&1), true));
        }
        _ => {}
    };
    // The header face (an extension type's representation declaration
    // hangs off the declaration row) and the declared members.
    for row in rows_with_parent(c, candidate_row) {
        push(row);
    }
    for row in crate::rules::body_members(c, candidate_row) {
        push(row);
    }
    if ctors.is_empty() {
        ctors.push((false, true));
    }
    ctors
}

/// Whether the constructor named at the row head is private: the head
/// is the class-name token, and the element name is that token — bare
/// for an unnamed constructor, `.`-suffixed for a named one — so the
/// official name test (`name.isPrivateName`) reduces to the head
/// lexeme leading with `_`: a library-private class is private in
/// every spelling (`_Priv(`, `_Priv._(`, `_Priv.named(`), a public
/// class only through the `._` tail (`E(` and `E.named(` are public).
fn constructor_name_private(c: &Check<'_>, row: u32) -> bool {
    let head = c.head_token(row);
    if lexeme_at(c, head).starts_with('_') {
        return true;
    }
    lexeme_at(c, head + 1) == "." && lexeme_at(c, head + 2).starts_with('_')
}

/// The lexeme of `token`, empty past the table (the tail guard of the
/// constructor-name walk).
fn lexeme_at<'a>(c: &'a Check<'_>, token: u32) -> &'a str {
    c.unit
        .tokens
        .get(token as usize)
        .map_or("", |t| &c.unit.source[t.byte_range()])
}

/// Whether any same-file class's superclass chain reaches the candidate
/// — `_hasSubclassInDefiningCompilationUnit` (extensions.dart:259-270)
/// over the current unit (the official walk sees the whole library;
/// another unit's subclass stays unseen).
fn subclassed_in_file(c: &Check<'_>, candidate: u32) -> bool {
    let Some(root) = c.unit.ast.root() else {
        return false;
    };
    for top in c.collect_direct(root) {
        if c.kind(top) != NodeKind::TopLevelDeclaration {
            continue;
        }
        for kid in c.collect_direct(top) {
            if !matches!(
                c.kind(kid),
                NodeKind::ClassDeclaration | NodeKind::NamedMixinApplication
            ) {
                continue;
            }
            if chain_reaches(c, &superclass_faces(c, kid), candidate) {
                return true;
            }
        }
    }
    false
}

/// The superclass-chain faces of a class row: the written `extends`
/// names (`with` mixins ride on the extends face without joining the
/// chain — `classType.superclass`, extensions.dart:263), or a mixin
/// application's `=` name (its superclass face).
fn superclass_faces<'a>(c: &'a Check<'_>, row: u32) -> Vec<&'a str> {
    match c.kind(row) {
        NodeKind::ClassDeclaration => supertypes(c, row)
            .into_iter()
            .filter(|&(_, clause)| clause == SupertypeClause::Extends)
            .map(|(name, _)| name)
            .collect(),
        NodeKind::NamedMixinApplication => supertypes(c, row)
            .into_iter()
            .next()
            .map(|(name, _)| vec![name])
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Whether the written superclass names reach `target` over the
/// same-file class rows (a cycle-guarded walk; a name resolving
/// outside the file dead-ends).
fn chain_reaches(c: &Check<'_>, names: &[&str], target: u32) -> bool {
    let chain_class = |name: &str| {
        class_like_named(
            c,
            name,
            &[NodeKind::ClassDeclaration, NodeKind::NamedMixinApplication],
        )
        .map(|(row, _)| row)
    };
    let mut seen: Vec<u32> = Vec::new();
    let mut queue: Vec<u32> = Vec::new();
    for &name in names {
        if let Some(row) = chain_class(name) {
            if row == target {
                return true;
            }
            if !seen.contains(&row) {
                seen.push(row);
                queue.push(row);
            }
        }
    }
    while let Some(row) = queue.pop() {
        for name in superclass_faces(c, row) {
            if let Some(next) = chain_class(name) {
                if next == target {
                    return true;
                }
                if !seen.contains(&next) {
                    seen.push(next);
                    queue.push(next);
                }
            }
        }
    }
    false
}

/// One `static const` field of a same-file class-like scope, in
/// declaration order (the official field walk,
/// extensions.dart:627-642).
struct ConstFieldFace<'a> {
    name: &'a str,
    /// The initializer's token range; `None` when absent (the
    /// `computeConstantValue() == null` skip).
    init: Option<core::ops::Range<u32>>,
    /// The written type's head name (`E` of `E?`/`E<int>`); empty for a
    /// function/void type (never the candidate type), `None` unwritten.
    written: Option<&'a str>,
    /// `metadata.hasDeprecated` — `@deprecated` / `@Deprecated`.
    deprecated: bool,
}

/// The `static const` fields of the class-like `class_row`.
fn class_const_fields<'a>(c: &'a Check<'_>, class_row: u32) -> Vec<ConstFieldFace<'a>> {
    let body = c
        .collect_direct(class_row)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::DeclarationBody)
        .unwrap_or(class_row);
    let mut fields = Vec::new();
    for member in rows_with_parent(c, body) {
        if c.kind(member) != NodeKind::Fields
            || c.payload(member).first()
                != Some(&(dartq_parser::MemberKind::StaticField as u32))
        {
            continue;
        }
        let deprecated = member_annotations(c, body, member).iter().any(|&meta| {
            c.collect_direct(meta).into_iter().any(|piece| {
                c.kind(piece) == NodeKind::Identifier
                    && matches!(c.lexeme(c.head_token(piece)), "deprecated" | "Deprecated")
            })
        });
        let written = c
            .collect_direct(member)
            .into_iter()
            .find_map(|kid| match c.kind(kid) {
                NodeKind::Type => Some(c.lexeme(c.head_token(kid))),
                NodeKind::FunctionType | NodeKind::VoidType => Some(""),
                _ => None,
            });
        for decl in c
            .collect_direct(member)
            .into_iter()
            .filter(|&kid| c.kind(kid) == NodeKind::VariablesDeclaration)
        {
            let Some(kw) = variables_declaration_keyword(c, decl) else {
                continue;
            };
            if c.lexeme(kw) != "const" {
                continue;
            }
            for var in c
                .collect_direct(decl)
                .into_iter()
                .filter(|&var| c.kind(var) == NodeKind::InitializedIdentifier)
            {
                let Some(name_row) = initialized_identifier_name(c, var) else {
                    continue;
                };
                let init = initializer_range(c, var);
                fields.push(ConstFieldFace {
                    name: c.lexeme(name_row),
                    init,
                    written,
                    deprecated,
                });
            }
        }
    }
    fields
}

/// The initializer's token range of an `InitializedIdentifier` row —
/// the tokens after the `=` up to the declarator's end: the first
/// depth-0 `,` (a sibling declarator's separator) or the terminating
/// `;`, whichever comes first. The row's own tail can leak past the
/// declarator (the bridge seals the row at the last completed node,
/// which a name-shaped initializer — `newFoo`, `E.e` — never
/// completes, while a creation `E._(1)` does), so a leaked sibling
/// (`b = E._(2)`) must not join the value text; the `,` cut only
/// fires outside brackets, depth tracked over `(`, `[`, `{` — inside
/// them a `,` belongs to the expression (argument lists, list/map/set
/// literals). `<`/`>` are deliberately not tracked: a depth-0 `,`
/// inside `<...>` only occurs in a parameterized tear-off or type
/// literal (`map<int, String>`), whose value can never head the
/// candidate type, so cutting there is invisible to coverage — while
/// tracking `<` would swallow the relational shape's real separator
/// (`a = b < c, d = E._(2)`). A `;` cannot occur inside a constant
/// initializer (function literals are not const; a string literal's
/// semicolons ride inside the string token), so it terminates at any
/// depth.
pub(crate) fn initializer_range(c: &Check<'_>, var: u32) -> Option<core::ops::Range<u32>> {
    let range = c.unit.ast.token_range(var);
    // The row's own extent can seal before adopted value leaves (the
    // prefix negation's literal completes before the container begins,
    // so `= -1` seals at `-`); the declaring declaration's extent
    // covers the whole declarator list and bounds the scan instead.
    let end_limit = match c.unit.ast.parent(var) {
        Some(parent) if c.kind(parent) == NodeKind::VariablesDeclaration => {
            c.unit.ast.token_range(parent).end
        }
        _ => range.end,
    };
    let mut token = range.start + 1;
    while token < end_limit {
        if c.lexeme(token) == "=" {
            let mut end = token + 1;
            let mut depth = 0usize;
            while end < end_limit {
                match c.lexeme(end) {
                    "(" | "[" | "{" => depth += 1,
                    ")" | "]" | "}" => depth = depth.saturating_sub(1),
                    ";" => break,
                    "," if depth == 0 => break,
                    _ => {}
                }
                end += 1;
            }
            return Some(token + 1..end);
        }
        token += 1;
    }
    None
}

/// The candidate's constant fields, valued (extensions.dart:626-642):
/// each field's value id resolves through the alias channel, and
/// `of_this_type` reads the written type or — unwritten — the canonical
/// terminal's head (`field.type != thisType`).
fn candidate_constants<'a>(
    c: &Check<'a>,
    values: &mut ConstValues,
    field_faces: &[ConstFieldFace<'a>],
    candidate_name: &str,
) -> Vec<ConstantField<'a>> {
    field_faces
        .iter()
        .map(|face| {
            let value = face
                .init
                .clone()
                .and_then(|init| init_value(c, values, field_faces, init, &mut Vec::new(), 0));
            let of_this_type = match face.written {
                Some(name) => name == candidate_name,
                None => value.is_some_and(|id| {
                    values.text(id).split(' ').next() == Some(candidate_name)
                }),
            };
            ConstantField {
                name: face.name,
                is_static_const: true,
                of_this_type,
                deprecated: face.deprecated,
                value,
            }
        })
        .collect()
}

/// One switch member's verdict face (the official member walk,
/// exhaustive_cases.dart:49-77): `default` silence, and coverage
/// through the head's constant-value identity.
fn member_face(
    c: &Check<'_>,
    values: &mut ConstValues,
    field_faces: &[ConstFieldFace<'_>],
    case: u32,
) -> SwitchMember {
    let is_default = c.payload(case).first().copied() != Some(NONE_TOKEN);
    let mut covers = None;
    if !is_default {
        // The first case head only — the official reads
        // `member.expression` / the single `guardedPattern` (:51-57).
        let head = c
            .collect_direct(case)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::CaseExpression);
        if let Some(head) = head
            && let Some(range) = head_range(c, head)
            && let Some(face) = head_face(c, range)
        {
            covers = alias_value(c, values, field_faces, face, &mut Vec::new(), 0);
        }
    }
    SwitchMember { is_default, covers }
}

/// The head expression's token face from the `CaseExpression` row: the
/// constant pattern's span, with the bridge's two tiling wrinkles
/// normalized —
///
/// - a parenthesized head (`case (E.e):`) rides a `ParenthesizedPattern`
///   leaf spanning the parens; the inner constant pattern (the window's
///   first tile) carries the unparenthesized face (the official
///   `pattern.unParenthesized`, :52);
/// - a direct head's adopted window holds only the trailing operand
///   subtree, so a dot-shorthand member name (`case .e:`) leaks as
///   preceding `CaseExpression` children (the `Identifier` /
///   `DotShorthandHead` leaves — the `type_literal_in_constant_pattern`
///   idiom); the range widens over them. Literal/creation heads grow no
///   leak, and a `when` guard completes after the pattern, outside the
///   widened prefix.
pub(super) fn head_range(c: &Check<'_>, head: u32) -> Option<core::ops::Range<u32>> {
    let kids = c.collect_direct(head);
    let pos = kids.iter().position(|&kid| {
        matches!(
            c.kind(kid),
            NodeKind::ConstantPattern | NodeKind::ParenthesizedPattern
        )
    })?;
    let row = kids[pos];
    if c.kind(row) == NodeKind::ParenthesizedPattern {
        let mut inner = first_child(c, row)?;
        while c.kind(inner) == NodeKind::ParenthesizedPattern {
            inner = first_child(c, inner)?;
        }
        return (c.kind(inner) == NodeKind::ConstantPattern)
            .then(|| c.unit.ast.token_range(inner));
    }
    let mut range = c.unit.ast.token_range(row);
    for &kid in &kids[..pos] {
        let kid_range = c.unit.ast.token_range(kid);
        range.start = range.start.min(kid_range.start);
        range.end = range.end.max(kid_range.end);
    }
    Some(range)
}

/// The head's alias face — the official variable faces only
/// (`Identifier` / `PropertyAccess` / `DotShorthandPropertyAccess`,
/// :59-74): identifier segments joined by `.`, an optional `const`
/// introducer stripped (`case const E.e:` covers through the same
/// expression). `None` — list/map/object patterns, literals,
/// creations — covers nothing.
fn head_face<'a>(c: &'a Check<'a>, range: core::ops::Range<u32>) -> Option<AliasFace<'a>> {
    let (leading_dot, names) = segments(c, strip_const(c, range))?;
    match names.as_slice() {
        [name] if leading_dot => Some(AliasFace::Shorthand(name)),
        [name] => Some(AliasFace::Bare(name)),
        // A leading-dot chain (`.a.b`) is a selector on a shorthand,
        // not one of the official variable faces.
        [.., class, field] if !leading_dot => Some(AliasFace::Qualified(class, field)),
        _ => None,
    }
}

/// The token range past a leading `const` introducer — `const` heads
/// and initializers carry their constant through the same expression
/// as the bare spelling.
pub(super) fn strip_const(
    c: &Check<'_>,
    range: core::ops::Range<u32>,
) -> core::ops::Range<u32> {
    if range.start < range.end && c.lexeme(range.start) == "const" {
        return range.start + 1..range.end;
    }
    range
}

/// The dot-joined identifier segments of a token range: `(leading_dot,
/// names)` — `E.e` → `(false, ["E", "e"])`, `prefix.E.e` →
/// `(false, ["prefix", "E", "e"])`, `.e` → `(true, ["e"])`. `None` for
/// any other expression (a non-name token inside a segment — the
/// literal/creation/interpolation faces).
pub(super) fn segments<'a>(
    c: &'a Check<'_>,
    range: core::ops::Range<u32>,
) -> Option<(bool, Vec<&'a str>)> {
    let mut names: Vec<&'a str> = Vec::new();
    let mut shorthand = false;
    let mut current: Option<&'a str> = None;
    for token in range {
        match c.lexeme(token) {
            "." if current.is_none() && names.is_empty() => shorthand = true,
            "." => names.push(current.take()?),
            lex => {
                if current.is_some() || !is_name_token(c, token) {
                    return None;
                }
                current = Some(lex);
            }
        }
    }
    if let Some(last) = current.take() {
        names.push(last);
    }
    (!names.is_empty()).then_some((shorthand, names))
}

/// Whether the token lexes as a plain name (an identifier, or a
/// built-in keyword — a constant named `set` binds;
/// `initialized_identifier_name`'s tolerance).
fn is_name_token(c: &Check<'_>, token: u32) -> bool {
    match c.unit.tokens.get(token as usize).map(|t| t.kind) {
        Some(TokenKind::Identifier) => true,
        Some(TokenKind::Keyword(kw)) if kw.style() == KeywordStyle::BuiltIn => true,
        _ => false,
    }
}

/// The alias face of a constant — a bare name (`static const b = a`),
/// a shorthand (`= .e`; never resolvable standalone), or a
/// `Class.member` face.
#[derive(Clone, Copy, Debug)]
enum AliasFace<'a> {
    Bare(&'a str),
    Shorthand(&'a str),
    Qualified(&'a str, &'a str),
}

/// The constant value id of one initializer: a dot-joined name segment
/// aliases into the named constant's value, and any other expression —
/// the terminal face (`E._(1)`, a literal) — is its own value text (the
/// `computeConstantValue` identity: equal texts denote the same
/// instance). A `.e` shorthand has no standalone value. `seen` breaks
/// alias cycles, `depth` bounds them.
fn init_value(
    c: &Check<'_>,
    values: &mut ConstValues,
    field_faces: &[ConstFieldFace<'_>],
    init: core::ops::Range<u32>,
    seen: &mut Vec<u32>,
    depth: usize,
) -> Option<u32> {
    if depth > MAX_ALIAS_DEPTH || seen.contains(&init.start) {
        return None;
    }
    let init = strip_const(c, init);
    match segments(c, init.clone()) {
        None => Some(values.intern(c, init.clone())),
        Some((true, _)) => None,
        Some((false, names)) => {
            seen.push(init.start);
            let target = match names.as_slice() {
                [name] => field_faces
                    .iter()
                    .find(|field| field.name == *name)
                    .and_then(|field| field.init.clone())
                    .or_else(|| top_level_const_init(c, name)),
                [.., class, field] => class_const_init(c, class, field),
                _ => return None,
            }?;
            init_value(c, values, field_faces, target, seen, depth + 1)
        }
    }
}

/// The value of a head's alias face: resolve the named constant, then
/// its initializer's value (unresolvable — nothing covered).
fn alias_value(
    c: &Check<'_>,
    values: &mut ConstValues,
    field_faces: &[ConstFieldFace<'_>],
    face: AliasFace<'_>,
    seen: &mut Vec<u32>,
    depth: usize,
) -> Option<u32> {
    if depth > MAX_ALIAS_DEPTH {
        return None;
    }
    let target = match face {
        // A bare head names the candidate's own constants first (class
        // scope shadows the top level), then the top-level scope.
        AliasFace::Bare(name) => field_faces
            .iter()
            .find(|field| field.name == name)
            .and_then(|field| field.init.clone())
            .or_else(|| top_level_const_init(c, name)),
        // A shorthand head resolves against the scrutinee's type — the
        // candidate's constants.
        AliasFace::Shorthand(name) => field_faces
            .iter()
            .find(|field| field.name == name)
            .and_then(|field| field.init.clone()),
        AliasFace::Qualified(class, field) => class_const_init(c, class, field),
    }?;
    init_value(c, values, field_faces, target, seen, depth + 1)
}

/// The initializer of the `static const` field `field` of the same-file
/// class-like `class` — the `E.e` head face and the `Class.member`
/// alias target.
pub(super) fn class_const_init(
    c: &Check<'_>,
    class: &str,
    field: &str,
) -> Option<core::ops::Range<u32>> {
    let (row, _) = class_like_named(
        c,
        class,
        &[
            NodeKind::ClassDeclaration,
            NodeKind::ExtensionTypeDeclaration,
        ],
    )?;
    class_const_fields(c, row)
        .into_iter()
        .find(|known| known.name == field)?
        .init
}

/// The initializer of the same-file top-level `const` named `name` —
/// the bare-name alias target outside class scope.
pub(super) fn top_level_const_init(c: &Check<'_>, name: &str) -> Option<core::ops::Range<u32>> {
    let root = c.unit.ast.root()?;
    for top in c.collect_direct(root) {
        if c.kind(top) != NodeKind::TopLevelDeclaration {
            continue;
        }
        for kid in c.collect_direct(top) {
            if c.kind(kid) != NodeKind::TopLevelFields {
                continue;
            }
            for decl in c
                .collect_direct(kid)
                .into_iter()
                .filter(|&decl| c.kind(decl) == NodeKind::VariablesDeclaration)
            {
                let Some(kw) = variables_declaration_keyword(c, decl) else {
                    continue;
                };
                if c.lexeme(kw) != "const" {
                    continue;
                }
                for var in c
                    .collect_direct(decl)
                    .into_iter()
                    .filter(|&var| c.kind(var) == NodeKind::InitializedIdentifier)
                {
                    if let Some(name_row) = initialized_identifier_name(c, var)
                        && c.lexeme(name_row) == name
                    {
                        return initializer_range(c, var);
                    }
                }
            }
        }
    }
    None
}

/// The canonical constant-value table: initializer texts interned to
/// dense ids.
#[derive(Default)]
struct ConstValues {
    texts: Vec<String>,
}

impl ConstValues {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "interned ids are bounded by one unit's constant count; \
                      u32 is the payload width"
    )]
    fn intern(&mut self, c: &Check<'_>, range: core::ops::Range<u32>) -> u32 {
        let text = token_text(c, range);
        if let Some(id) = self.texts.iter().position(|known| *known == text) {
            return id as u32;
        }
        self.texts.push(text);
        (self.texts.len() - 1) as u32
    }

    fn text(&self, id: u32) -> &str {
        &self.texts[id as usize]
    }
}

/// The canonical text of a token range — lexemes joined with a space
/// (whitespace-free, comment-free value identity).
fn token_text(c: &Check<'_>, range: core::ops::Range<u32>) -> String {
    let mut text = String::new();
    for token in range {
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(c.lexeme(token));
    }
    text
}

/// One `static const` field of the candidate type (the official field
/// walk, extensions.dart:627-642).
pub struct ConstantField<'a> {
    pub name: &'a str,
    /// `field.isConst && field.isStatic`.
    pub is_static_const: bool,
    /// The field's type is the enclosing type (the
    /// `field.type != thisType` skip).
    pub of_this_type: bool,
    /// `metadata.hasDeprecated` — aliases are reported last.
    pub deprecated: bool,
    /// The computed constant-value identity; aliasing fields
    /// (`static const b = a`) share the id, `None` when the value is
    /// not computable (the `fieldValue == null` skip).
    pub value: Option<u32>,
}

/// The candidate type face — `asEnumLikeType`'s inputs
/// (extensions.dart:604-647).
pub struct EnumLikeCandidate<'a> {
    /// An extension type (`ExtensionTypeElement`) is always
    /// enum-like-eligible; a class must additionally pass the two
    /// class-only gates below.
    pub is_extension_type: bool,
    /// Class only: `element.isAbstract`.
    pub is_abstract: bool,
    /// Class only: a subclass exists in the defining compilation unit
    /// (`_hasSubclassInDefiningCompilationUnit`).
    pub has_subclass_in_defining_unit: bool,
    /// Every constructor as `(is_private, is_generative)` — all must
    /// hold (:620).
    pub constructors: &'a [(bool, bool)],
    /// The type's fields, declaration order (:627); the enumeration
    /// applies the official `isOriginGetterSetter` skip (getter/setter
    /// pseudo-fields are not fields and never enter the face).
    pub constants: &'a [ConstantField<'a>],
}

/// One switch member's face (the official member walk,
/// exhaustive_cases.dart:49-77).
pub struct SwitchMember {
    /// `default:` — ends the whole check silent (:75-77).
    pub is_default: bool,
    /// The constant value the head covers: a constant-pattern /
    /// legacy-case head whose expression resolves to one of the type's
    /// constant values (the `Identifier` / `PropertyAccess` /
    /// `DotShorthandPropertyAccess` faces all resolve through
    /// `computeConstantValue`). `None` — other patterns, foreign
    /// constants, unresolved heads — covers nothing.
    pub covers: Option<u32>,
}

/// The official verdict: the missing values' preferred names, in field
/// declaration order — the mounter renders each into the template at
/// the statement span (statement head through the scrutinee's `)`,
/// :80-93). Empty when the type is not enum-like or a `default` is
/// present.
#[must_use]
pub fn check_switch<'a>(
    candidate: &'a EnumLikeCandidate<'a>,
    members: &[SwitchMember],
) -> Vec<&'a str> {
    if !candidate.is_extension_type
        && (candidate.is_abstract || candidate.has_subclass_in_defining_unit)
    {
        return Vec::new();
    }
    if !candidate
        .constructors
        .iter()
        .all(|&(is_private, is_generative)| is_private && is_generative)
    {
        return Vec::new();
    }
    // The constant groups in declaration order, keyed by value id
    // (extensions.dart:626-642). Constant counts are tiny — linear
    // grouping beats a map.
    let mut groups: Vec<(u32, Vec<&'a ConstantField<'a>>)> = Vec::new();
    let mut value_count = 0;
    for field in candidate.constants {
        if !field.is_static_const || !field.of_this_type {
            continue;
        }
        let Some(value) = field.value else {
            continue;
        };
        value_count += 1;
        match groups.iter_mut().find(|(id, _)| *id == value) {
            Some((_, group)) => group.push(field),
            None => groups.push((value, vec![field])),
        }
    }
    // The two-constant minimum counts fields, not values (:640) — a
    // `static const b = a` pair is enum-like with one group.
    if value_count < 2 {
        return Vec::new();
    }
    let mut covered: Vec<u32> = Vec::new();
    for member in members {
        if member.is_default {
            return Vec::new();
        }
        if let Some(value) = member.covers.filter(|value| !covered.contains(value)) {
            covered.push(value);
        }
    }
    groups
        .iter()
        .filter(|(value, _)| !covered.contains(value))
        .map(|(_, group)| preferred_name(group))
        .collect()
}

/// The group's reporting name (:84-88): the first non-deprecated
/// member, else the first.
fn preferred_name<'a>(group: &[&'a ConstantField<'a>]) -> &'a str {
    group
        .iter()
        .find(|field| !field.deprecated)
        .unwrap_or(&group[0])
        .name
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRIVATE_GENERATIVE: (bool, bool) = (true, true);

    fn candidate<'a>(
        constructors: &'a [(bool, bool)],
        constants: &'a [ConstantField<'a>],
    ) -> EnumLikeCandidate<'a> {
        EnumLikeCandidate {
            is_extension_type: false,
            is_abstract: false,
            has_subclass_in_defining_unit: false,
            constructors,
            constants,
        }
    }

    fn const_field(name: &str, value: Option<u32>) -> ConstantField<'_> {
        ConstantField {
            name,
            is_static_const: true,
            of_this_type: true,
            deprecated: false,
            value,
        }
    }

    fn member(covers: Option<u32>) -> SwitchMember {
        SwitchMember {
            is_default: false,
            covers,
        }
    }

    /// The official `test_class_enumLike_allCases` face — every value
    /// covered, silent.
    #[test]
    fn fully_covered_switch_is_silent() {
        let ctors = [PRIVATE_GENERATIVE];
        let fields = [
            const_field("e", Some(0)),
            const_field("f", Some(1)),
            const_field("g", Some(2)),
        ];
        let members = [member(Some(0)), member(Some(1)), member(Some(2))];
        assert!(check_switch(&candidate(&ctors, &fields), &members).is_empty());
    }

    /// The official `test_class_enumLike_missingCase` — the uncovered
    /// value reports with its name.
    #[test]
    fn missing_value_reports_the_name() {
        let ctors = [PRIVATE_GENERATIVE];
        let fields = [
            const_field("e", Some(0)),
            const_field("f", Some(1)),
            const_field("g", Some(2)),
        ];
        let members = [member(Some(0)), member(Some(1))];
        assert_eq!(check_switch(&candidate(&ctors, &fields), &members), ["g"]);
    }

    /// The official `test_class_enumLike_withDefault` — a `default`
    /// silences the whole check even with values missing (:75-77).
    #[test]
    fn default_silences() {
        let ctors = [PRIVATE_GENERATIVE];
        let fields = [
            const_field("e", Some(0)),
            const_field("f", Some(1)),
            const_field("g", Some(2)),
        ];
        let members = [
            member(Some(0)),
            SwitchMember {
                is_default: true,
                covers: None,
            },
        ];
        assert!(check_switch(&candidate(&ctors, &fields), &members).is_empty());
    }

    /// A non-constant / foreign / unresolved head covers nothing (the
    /// official `enumConstants.remove` misses) — missing values report
    /// in declaration order.
    #[test]
    fn foreign_head_covers_nothing() {
        let ctors = [PRIVATE_GENERATIVE];
        let fields = [
            const_field("e", Some(0)),
            const_field("f", Some(1)),
            const_field("g", Some(2)),
        ];
        let members = [member(Some(0)), member(None)];
        assert_eq!(
            check_switch(&candidate(&ctors, &fields), &members),
            ["f", "g"]
        );
    }

    /// The official `test_class_enumLike_deprecatedAliases` — aliases
    /// share a value id; covering either alias covers the value, and a
    /// missing alias group reports the first non-deprecated name.
    #[test]
    fn alias_group_prefers_the_live_name() {
        let ctors = [PRIVATE_GENERATIVE];
        let fields = [
            const_field("newFoo", Some(0)),
            ConstantField {
                deprecated: true,
                ..const_field("oldFoo", Some(0))
            },
            const_field("bar", Some(1)),
            const_field("baz", Some(2)),
        ];
        // newFoo + bar + baz covers everything (the deprecated alias
        // never reports).
        let all = [member(Some(0)), member(Some(1)), member(Some(2))];
        assert!(check_switch(&candidate(&ctors, &fields), &all).is_empty());
        // bar + baz leaves the alias group; the live name reports.
        let partial = [member(Some(1)), member(Some(2))];
        assert_eq!(
            check_switch(&candidate(&ctors, &fields), &partial),
            ["newFoo"]
        );
    }

    /// The enum-like gate (extensions.dart:604-647): public or
    /// non-generative constructors, an abstract class, a subclass in
    /// the defining unit, or fewer than two value-carrying
    /// `static const` fields of the type all stay silent.
    #[test]
    fn gate_rejections() {
        let fields = [const_field("e", Some(0)), const_field("f", Some(1))];
        let members = [member(Some(0))];
        // A public constructor.
        let public = [(false, true)];
        assert!(check_switch(&candidate(&public, &fields), &members).is_empty());
        // A non-generative constructor (a factory).
        let factory = [(true, false)];
        assert!(check_switch(&candidate(&factory, &fields), &members).is_empty());
        // Abstract, or subclassed in the defining unit.
        let mut abstract_candidate = candidate(&[PRIVATE_GENERATIVE], &fields);
        abstract_candidate.is_abstract = true;
        assert!(check_switch(&abstract_candidate, &members).is_empty());
        let mut subclassed = candidate(&[PRIVATE_GENERATIVE], &fields);
        subclassed.has_subclass_in_defining_unit = true;
        assert!(check_switch(&subclassed, &members).is_empty());
        // One constant only (the official
        // `test_class_notEnumLike_singleConstant`).
        let single = [const_field("e", Some(0))];
        assert!(check_switch(&candidate(&[PRIVATE_GENERATIVE], &single), &members).is_empty());
        // Non-const / foreign-typed / non-computable fields do not
        // count toward the minimum.
        let uncounted = [
            ConstantField {
                is_static_const: false,
                ..const_field("e", Some(0))
            },
            ConstantField {
                of_this_type: false,
                ..const_field("f", Some(1))
            },
            const_field("g", None),
        ];
        let two = [member(Some(0)), member(Some(1))];
        assert!(check_switch(&candidate(&[PRIVATE_GENERATIVE], &uncounted), &two).is_empty());
    }

    /// Extension types skip the class-only gates (the official
    /// `test_extensionType_*` family); duplicate values count fields,
    /// not groups, toward the minimum (:640).
    #[test]
    fn extension_types_and_aliased_pairs() {
        let fields = [const_field("a", Some(0)), const_field("b", Some(0))];
        let mut ext = candidate(&[PRIVATE_GENERATIVE], &fields);
        ext.is_extension_type = true;
        assert_eq!(check_switch(&ext, &[]), ["a"]);
        // As a class, the same shape passes the gates too (two fields).
        assert_eq!(
            check_switch(&candidate(&[PRIVATE_GENERATIVE], &fields), &[]),
            ["a"]
        );
    }

    // —— the mounted face: the official test sources through the
    // production bridge (`Fixture::parse`) ——

    use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

    fn run(source: &str) -> Vec<crate::engine::Lint> {
        Fixture::parse(source).run(&one_rule("exhaustive_cases"))
    }

    /// `test_class_enumLike_allCases` — every value covered, silent.
    #[test]
    fn mounted_all_cases_silent() {
        assert!(
            run(r"
class E {
  final int i;
  const E._(this.i);

  static const e = E._(1);
  static const f = E._(2);
  static const g = E._(3);
}

void ok(E e) {
  switch (e) {
    case E.e:
      break;
    case E.f:
      break;
    case E.g:
      break;
  }
}
")
            .is_empty()
        );
    }

    /// `test_class_enumLike_missingCase` — the uncovered value reports
    /// at the statement head through the scrutinee's `)`, with the
    /// official message and correction.
    #[test]
    fn mounted_missing_case_reports() {
        let source = r"
class E {
  final int i;
  const E._(this.i);

  static const e = E._(1);
  static const f = E._(2);
  static const g = E._(3);
}

void e(E e) {
  switch (e) {
    case E.e:
      break;
    case E.f:
  }
}
";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("exhaustive_cases"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Missing case clauses for some constants in 'g'."
        );
        assert_eq!(
            lints[0].correction.as_deref(),
            Some("Try adding case clauses for the missing constants.")
        );
        // The span: the `switch` head through the scrutinee's `)`
        // (the sixth close paren — after the four creation spellings
        // and the parameter list).
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(source, &f.tokens, "switch", 0))
        );
        assert_eq!(
            lints[0].range.end,
            utf16_at(&f, tok(source, &f.tokens, ")", 5)) + 1
        );
    }

    /// `test_class_enumLike_withDefault` — a `default` member silences
    /// the whole check.
    #[test]
    fn mounted_default_silences() {
        assert!(
            run(r"
class E {
  final int i;
  const E._(this.i);

  static const e = E._(1);
  static const f = E._(2);
  static const g = E._(3);
}

void okDefault(E e) {
  switch (e) {
    case E.e:
      break;
    default:
      break;
  }
}
")
            .is_empty()
        );
    }

    /// `test_class_enumLike_parenthesizedCases` — parenthesized case
    /// heads cover (the `unParenthesized` walk).
    #[test]
    fn mounted_parenthesized_cases_cover() {
        assert!(
            run(r"
class E {
  final int i;
  const E._(this.i);

  static const e = E._(1);
  static const f = E._(2);
  static const g = E._(3);
}

void okParenthesized(E e) {
  switch (e) {
    case (E.e):
      break;
    case ((E.f)):
      break;
    case (E.g):
      break;
  }
}
")
            .is_empty()
        );
    }

    /// `test_class_enumLike_deprecatedAliases` — the deprecated alias
    /// groups with its live twin; exactly one report (`bar`, the group
    /// the second switch leaves open), the other two switches silent.
    #[test]
    fn mounted_deprecated_aliases() {
        let source = r"
class DeprecatedFields {
  final int i;
  const DeprecatedFields._(this.i);

  @deprecated
  static const oldFoo = newFoo;
  static const newFoo = DeprecatedFields._(1);
  static const bar = DeprecatedFields._(2);
  static const baz = DeprecatedFields._(3);
}

void dep(DeprecatedFields e) {
  switch (e) {
    case DeprecatedFields.newFoo:
      break;
    case DeprecatedFields.bar:
      break;
    case DeprecatedFields.baz:
      break;
  }

  switch (e) {
    case DeprecatedFields.newFoo:
      break;
    case DeprecatedFields.baz:
      break;
  }

  switch (e) {
    case DeprecatedFields.oldFoo:
      break;
    case DeprecatedFields.bar:
      break;
    case DeprecatedFields.baz:
      break;
  }
}
";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("exhaustive_cases"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Missing case clauses for some constants in 'bar'."
        );
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(source, &f.tokens, "switch", 1))
        );
    }

    /// `test_class_notEnumLike_publicConstructor` / `_singleConstant` /
    /// `_subclassed` — the enum-like gate rejections, silent.
    #[test]
    fn mounted_not_enum_like_silent() {
        assert!(
            run(r"
class PublicCons {
  const PublicCons();
  static const e = PublicCons();
  static const f = PublicCons();
}

void p(PublicCons e) {
  switch (e) {
    case PublicCons.e:
  }
}
")
            .is_empty()
        );
        assert!(
            run(r"
class TooFew {
  const TooFew._();

  static const e = TooFew._();
}

void t(TooFew e) {
  switch (e) {
    case TooFew.e:
  }
}
")
            .is_empty()
        );
        assert!(
            run(r"
class Subclassed {
  const Subclassed._();

  static const e = Subclassed._();
  static const f = Subclassed._();
  static const g = Subclassed._();
}

class Subclass extends Subclassed {
  Subclass() : super._();
}

void s(Subclassed e) {
  switch (e) {
    case Subclassed.e:
  }
}
")
            .is_empty()
        );
    }

    /// `test_class_enumLike_dotShorthand_missingCase` /
    /// `_withDefault` — the `.e` head face covers and misses like the
    /// spelled form.
    #[test]
    fn mounted_dot_shorthand_heads() {
        let source = r"
class E {
  final int i;
  const E._(this.i);

  static const e = E._(1);
  static const f = E._(2);
  static const g = E._(3);
}

void fn(E e) {
  switch (e) {
    case .e:
      break;
    case .f:
  }
}
";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("exhaustive_cases"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Missing case clauses for some constants in 'g'."
        );
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(source, &f.tokens, "switch", 0))
        );
        assert!(
            run(r"
class E {
  final int i;
  const E._(this.i);

  static const e = E._(1);
  static const f = E._(2);
  static const g = E._(3);
}

void fn(E e) {
  switch (e) {
    case .e:
      break;
    default:
      break;
  }
}
")
            .is_empty()
        );
    }

    /// The `test_extensionType_*` family — enum-like extension types
    /// (private representation, two-plus same-typed constants) report
    /// the missing constant; the public-representation and
    /// single-constant shapes stay silent.
    #[test]
    fn mounted_extension_type_family() {
        let source = r"
extension type const E._(int i) {
  static const E a = E._(1);
  static const E b = E._(2);
  static const E c = E._(3);
}

void f(E e) {
  switch (e) {
    case E.a:
      break;
    case E.b:
  }
}
";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("exhaustive_cases"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Missing case clauses for some constants in 'c'."
        );
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(source, &f.tokens, "switch", 0))
        );
        assert!(
            run(r"
extension type const E(int i) {
  static const E a = E(1);
  static const E b = E(2);
}

void f(E e) {
  switch (e) {
    case E.a:
  }
}
")
            .is_empty()
        );
        assert!(
            run(r"
extension type const E._(int i) {
  static const E a = E._(1);
}

void f(E e) {
  switch (e) {}
}
")
            .is_empty()
        );
        assert!(
            run(r"
extension type const E._(int i) {
  static const E a = E._(1);
  static const E b = E._(2);
}

void f(E e) {
  switch (e) {
    case E.a:
      break;
    default:
  }
}
")
            .is_empty()
        );
    }

    /// The shorthand extension-type head (`.a`) — the official
    /// `test_extensionType_enumLike_dotShorthand_missingCase`.
    #[test]
    fn mounted_extension_type_dot_shorthand() {
        let source = r"
extension type const E._(int i) {
  static const E a = E._(1);
  static const E b = E._(2);
}

void f(E e) {
  switch (e) {
    case .a:
  }
}
";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("exhaustive_cases"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Missing case clauses for some constants in 'b'."
        );
    }

    /// `test_enum_missingCase` — a real enum never enters (the
    /// element walk's default arm; the analyzer's own exhaustiveness
    /// error covers it, and that diagnostic is out of this face).
    #[test]
    fn mounted_enum_scrutinee_silent() {
        assert!(
            run(r"
enum Color { red, green }

void f(Color c) {
  switch (c) {
    case Color.red:
  }
}
")
            .is_empty()
        );
    }

    /// The value-based coverage faces: a bare top-level alias head
    /// (`case alias:`) and a cross-class `Class.member` head cover by
    /// value identity — the remaining values report in declaration
    /// order.
    #[test]
    fn mounted_value_based_heads() {
        let alias_source = r"
class E {
  final int i;
  const E._(this.i);

  static const e = E._(1);
  static const f = E._(2);
  static const g = E._(3);
}

const alias = E.e;

void fn(E e) {
  switch (e) {
    case alias:
  }
}
";
        let lints = run(alias_source);
        assert_eq!(lints.len(), 2, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Missing case clauses for some constants in 'f'."
        );
        assert_eq!(
            lints[1].message,
            "Missing case clauses for some constants in 'g'."
        );
        assert_eq!(
            run(r"
class E {
  final int i;
  const E._(this.i);

  static const e = E._(1);
  static const f = E._(2);
  static const g = E._(3);
}

class Other {
  static const z = E.e;
}

void fn(E e) {
  switch (e) {
    case Other.z:
  }
}
")
            .iter()
            .map(|lint| lint.message.as_str())
            .collect::<Vec<_>>(),
            [
                "Missing case clauses for some constants in 'f'.",
                "Missing case clauses for some constants in 'g'.",
            ]
        );
    }

    /// The same-file silences: a non-identifier scrutinee (no written
    /// type on the channel) and an out-of-file candidate (an import
    /// prefix) — the documented under-report family.
    #[test]
    fn mounted_channel_fallbacks_silent() {
        assert!(
            run(r"
class E {
  final int i;
  const E._(this.i);

  static const e = E._(1);
  static const f = E._(2);
}

E make() => E._(1);

void fn() {
  switch (make()) {
    case E.e:
  }
}
")
            .is_empty()
        );
        assert!(
            run(r"
import 'e.dart' as prefixed;

void fn(prefixed.E e) {
  switch (e) {
    case prefixed.E.e:
  }
}
")
            .is_empty()
        );
    }

    /// A creation-only multi-declarator row (`static const a =
    /// E._(1), b = E._(2);`) seals cleanly per declarator, and the
    /// depth-0 `,` cut keeps each value's own tokens: full coverage
    /// is silent, a partial switch reports the uncovered declarator.
    #[test]
    fn mounted_multi_declarator_creations() {
        assert!(
            run(r"
class E {
  final int i;
  const E._(this.i);

  static const a = E._(1), b = E._(2);
}

void f(E e) {
  switch (e) {
    case E.a:
      break;
    case E.b:
      break;
  }
}
")
            .is_empty()
        );
        let source = r"
class E {
  final int i;
  const E._(this.i);

  static const a = E._(1), b = E._(2);
}

void f(E e) {
  switch (e) {
    case E.a:
  }
}
";
        let lints = run(source);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Missing case clauses for some constants in 'b'."
        );
    }

    /// The mixed-alias multi-declarator leak pin: a name-shaped
    /// initializer (`newFoo`) never completes a bridge node, so `a`'s
    /// row span can seal past the sibling `,` — without the depth-0
    /// cut the value text interns as `newFoo , b = E._(2)` and `a`'s
    /// alias to `E._(1)` breaks. The official corpus has no
    /// multi-declarator case; the expectation follows
    /// `asEnumLikeType`'s value resolution — `a`'s
    /// `computeConstantValue()` is the `E._(1)` instance, `newFoo`'s
    /// identity, so `case E.newFoo:` covers `a` and the row is
    /// silent, while leaving the shared group open reports it under
    /// its first name.
    #[test]
    fn mounted_multi_declarator_alias_no_false_positive() {
        assert!(
            run(r"
class E {
  final int i;
  const E._(this.i);

  static const E a = newFoo, b = E._(2);
  static const newFoo = E._(1);
}

void f(E e) {
  switch (e) {
    case E.newFoo:
      break;
    case E.b:
      break;
  }
}
")
            .is_empty()
        );
        let source = r"
class E {
  final int i;
  const E._(this.i);

  static const E a = newFoo, b = E._(2);
  static const newFoo = E._(1);
}

void f(E e) {
  switch (e) {
    case E.b:
  }
}
";
        let lints = run(source);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Missing case clauses for some constants in 'a'."
        );
    }

    /// A library-private class's unnamed constructor (`_Priv(`) is
    /// private — the official `isPrivate` is the element name's
    /// leading `_`, which every constructor of the class leads with —
    /// so the enum-like gate holds and the missing case reports; the
    /// public spelling (`Priv(`) stays gate-silent.
    #[test]
    fn mounted_private_class_unnamed_constructor() {
        let source = r"
class _Priv {
  final int i;
  const _Priv(this.i);

  static const a = _Priv(1);
  static const b = _Priv(2);
}

void f(_Priv e) {
  switch (e) {
    case _Priv.a:
  }
}
";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("exhaustive_cases"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Missing case clauses for some constants in 'b'."
        );
    }
}
