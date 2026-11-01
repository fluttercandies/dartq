use dartq_semantic::model::{ArgListId, Ty};

use super::{
    Check, NodeKind, PayloadField, RuleSpec, query_site, same_file, slice, static_type_faces,
    switch_scrutinee_read,
};
use crate::rules::utf16_token_span;

/// The pattern-face problem text (`unrelatedTypeEqualityChecksInPattern`,
/// `diagnostic.g.dart:3727-3734`): the operand against the matched
/// value.
const PATTERN_PROBLEM: &str = "The type of the operand ('{0}') isn't a subtype or a supertype of the value being matched ('{1}').";

/// Official: `pkg/linter/lib/src/rules/unrelated_type_equality_checks.dart`
/// — an `==`/`!=` comparison (a bool-typed one, :47-49) whose operand
/// types are unrelated reports at the operator, with `(right type,
/// left type)` (:62-66; the pattern face :69-83 needs the matched
/// value type — unported).
///
/// The operand types: scalar literals (`int`, `double`, `String`,
/// `bool`), `this` (the enclosing class's name), a bare name whose
/// same-file written type (parameter, local, field, top-level) carries
/// a name — and the bridge static type when the written face has no
/// answer: a bare read row (an import-bound top-level) or an
/// invocation's callee return type (`f() == true`). The comparability
/// relation (`typesAreUnrelated`): the same head name compares its
/// pairwise type arguments (`List<A>` vs `List<B>` with unrelated
/// bounds stays unrelated — the subtype walk's covariance face);
/// `Object`/`dynamic` relate to everything; the `num` family relates
/// `int`/`double`; the four scalar faces are final (nothing else
/// relates to them); any other pair with a same-file class-like name
/// stays silent — the supertype chain, or a cross-file link behind
/// it, can relate the names in ways this face cannot see. A record
/// compares structurally (the `isAssignableTo` arm, :271-273): the
/// positional arities and named field-name sets must match and every
/// field pair must compare, else the record is unrelated to any
/// non-record type. The bool
/// gate (:47-49) reads the left operand's resolved `==` override: a
/// non-`bool` declared return silences (a bare-name operand only; an
/// unresolvable or undecided override keeps the bool assumption).
/// `null` operands skip (:53-55). The pattern face (:69-83,
/// `visitRelationalPattern`): a relational pattern whose operand type
/// and matched value type are unrelated reports at the pattern — the
/// matched value type rides [`Analysis::matched_value_type`] keyed at
/// the switch scrutinee read's own span (an if-case carries no switch
/// ancestor and stays silent). Message: `diagnostic.g.dart`
/// `unrelatedTypeEqualityChecksInExpression`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "unrelated_type_equality_checks",
    &[NodeKind::BinaryExpression, NodeKind::RelationalPattern],
    check,
);

fn check(c: &mut Check<'_>) {
    if c.kind(c.node()) == NodeKind::RelationalPattern {
        pattern_face(c);
        return;
    }
    let node = c.node();
    let Some(op) = c.field(node, PayloadField::Operator) else {
        return;
    };
    if !matches!(c.lexeme(op), "==" | "!=") {
        return;
    }
    // The operand runs of the flattened container (:52-59). The owner
    // slice below the adopted window can carry the enclosing
    // declaration's header strays (an `=>` body's `Type`/`FormalParameters`
    // rows precede the operand — `bool f((int,int) a, …) => a == b`), so
    // the left run trims to the token-adjacent tail at the operator.
    let Some((left, right)) = slice::binary_operands(c) else {
        return;
    };
    let left = slice::contiguous_tail(c, &left, op).to_vec();
    // The official bool gate (:47-49): the comparison's static type is
    // `bool` unless the left operand's class overrides `==` with a
    // non-`bool` return.
    if equality_override_silences(c, &left) {
        return;
    }
    let Some(left_face) = operand_face(c, &left) else {
        return;
    };
    let Some(right_face) = operand_face(c, &right) else {
        return;
    };
    // The `_comparable` extension (:90-103): a `fixnum` `Int32`/`Int64`
    // against `int` is comparable.
    if left_face.is_fixnum_intx() && right_face.is_named("int") {
        return;
    }
    if !unrelated(c, &left_face, &right_face) {
        return; // `_comparable` on the ported face
    }
    let range = utf16_token_span(c, op);
    let (rd, ld) = (right_face.display(), left_face.display());
    c.report(range, &[rd.as_str(), ld.as_str()]);
}

/// The operand's structured type face over the written and bridge
/// channels (`staticType`): enough shape for the upstream
/// `typesAreUnrelated` walk — a named type with its type-argument names,
/// a record's parsed shape, or a type variable with its
/// declared bound.
enum OpFace<'a> {
    /// `int`, `List<A>`, `Int32`: head name plus argument spellings.
    Named {
        name: &'a str,
        args: Vec<String>,
        /// The head names a declared type variable of the enclosing
        /// declaration (`<A extends int>`).
        is_typevar: bool,
        /// The type variable's bound name (`int` of `A extends int`).
        bound: Option<String>,
    },
    /// A record type, structurally parsed — the record arm of
    /// `typesAreUnrelated` decides by record shape (positional arity,
    /// named field names, pairwise field comparability), not by text.
    Record(RecordShape),
    /// `dynamic`/`Null`/undecidable — suppresses (`typesAreUnrelated`
    /// returns false on `DynamicType`/`isBottom`/null).
    Top,
}

/// One parsed record type. `display` is the canonical
/// `getDisplayString` rendering (positional fields bare, the named
/// section sorted by field name — the analyzer keeps record named
/// fields sorted); the field lists carry type spellings whose head
/// names the comparability walk reads.
struct RecordShape {
    /// `(int, {int two})` — the diagnostic's `{0}`/`{1}` spelling.
    display: String,
    /// Positional field type spellings (`num` of `num one`).
    pos: Vec<String>,
    /// `(field name, type spelling)`, sorted by field name.
    named: Vec<(String, String)>,
}

impl OpFace<'_> {
    /// `Int32`/`Int64` — the fixnum face (`isFixnumIntX`, :94-105). The
    /// `package:fixnum` URI is not readable through the face; the name
    /// stands in (a same-file `Int32` class is a documented
    /// under-report).
    fn is_fixnum_intx(&self) -> bool {
        matches!(self, Self::Named { name, .. } if matches!(*name, "Int32" | "Int64"))
    }

    fn is_named(&self, want: &str) -> bool {
        matches!(self, Self::Named { name, .. } if *name == want)
    }

    /// `DartType.getDisplayString` — the diagnostic's `{0}`/`{1}` slots.
    fn display(&self) -> String {
        match self {
            Self::Named { name, args, .. } if args.is_empty() => (*name).to_owned(),
            Self::Named { name, args, .. } => format!("{}<{}>", name, args.join(", ")),
            Self::Record(shape) => shape.display.clone(),
            Self::Top => "dynamic".to_owned(),
        }
    }
}

/// The operand's structured face: a scalar literal, `this`, or a bare
/// name's written type (the same-file binding slice — `List<A>` keeps
/// its argument spellings there) — then the bridge static type when
/// the written face has no answer.
fn operand_face<'a>(c: &'a Check<'a>, rows: &[u32]) -> Option<OpFace<'a>> {
    let run = slice::access_run(c, rows);
    let [root] = run.as_slice() else {
        return None; // a multi-row operand is a chain — no face
    };
    match c.kind(*root) {
        NodeKind::LiteralInt => Some(named_face(c, "int")),
        NodeKind::LiteralDouble => Some(named_face(c, "double")),
        NodeKind::StringLiteral => Some(named_face(c, "String")),
        NodeKind::LiteralBool => Some(named_face(c, "bool")),
        NodeKind::LiteralNull => Some(OpFace::Top),
        NodeKind::ThisExpression => {
            let class_row = c.path.iter().rev().find(|&&a| {
                matches!(
                    c.kind(a),
                    NodeKind::ClassDeclaration
                        | NodeKind::MixinDeclaration
                        | NodeKind::EnumDeclaration
                        | NodeKind::ExtensionDeclaration
                )
            })?;
            let name = c.collect_direct(*class_row).into_iter().find(|&kid| {
                c.kind(kid) == NodeKind::Identifier
                    && c.field(kid, PayloadField::Context)
                        == Some(
                            dartq_parser::IdentifierContext::ClassOrMixinOrExtensionDeclaration
                                as u32,
                        )
            })?;
            Some(named_face(c, c.lexeme(c.head_token(name))))
        }
        NodeKind::Identifier => {
            let name = c.lexeme(c.head_token(*root));
            identifier_face(c, *root, name)
        }
        _ => None,
    }
}

/// A bare-name operand's face: the same-file written binding first —
/// `List<A>` reads its argument spellings off the type row, a record
/// reads its source slice, a type variable its declared bound — then
/// the bridge `type_of` for import-bound reads (a `fixnum` `Int32`,
/// an invocation's callee return in the caller's face).
fn identifier_face<'a>(c: &'a Check<'a>, row: u32, name: &'a str) -> Option<OpFace<'a>> {
    if let Some((_, Some(type_row))) = same_file::binding(c, c.node(), name) {
        return Some(written_face(c, type_row));
    }
    // A same-file binding without a written type (`var x`) carries no
    // verdict face — the bridge read still answers its inferred type.
    let analysis = c.semantics()?;
    let ty = analysis.type_of(query_site(c, row))?;
    Some(ty_face(c, analysis, ty))
}

/// The written `Type`/`FunctionType`/`RecordType` row's face: a record
/// parses structurally ([`record_source_text`] + [`record_shape`]), a
/// generic keeps the argument head names, a type variable carries its
/// bound, an enum head marks the enum arm.
fn written_face<'a>(c: &'a Check<'a>, type_row: u32) -> OpFace<'a> {
    if c.kind(type_row) == NodeKind::RecordType {
        let Some(text) = record_source_text(c, type_row) else {
            return OpFace::Top;
        };
        let Some(shape) = record_shape(text) else {
            return OpFace::Top;
        };
        return OpFace::Record(shape);
    }
    let face = crate::rules::type_face(c, type_row);
    let Some(name) = face.name else {
        return OpFace::Top;
    };
    let bound = typevar_bound(c, name);
    OpFace::Named {
        name,
        args: face
            .type_arguments
            .iter()
            .map(|&(arg, _)| arg.to_owned())
            .collect(),
        is_typevar: is_declared_typevar(c, name),
        bound,
    }
}

/// The record type's source text: the `RecordType` row's own span can
/// start past the opening paren or end before the closer (the named
/// fields tile beside the positional run), so the slice anchors at the
/// enclosing row — the parameter's/field's own start — and ends at the
/// written field name's first byte.
fn record_source_text<'a>(c: &'a Check<'a>, type_row: u32) -> Option<&'a str> {
    let parent = c.unit.ast.parent(type_row)?;
    let start = c.byte_span(parent).start;
    let kids = c.collect_direct(parent);
    let idx = kids.iter().position(|&k| k == type_row)?;
    let mut end = c.byte_span(type_row).end;
    if c.kind(parent) == NodeKind::FormalParameter {
        if let Some(name) = c
            .field(parent, PayloadField::Name)
            .filter(|&tok| tok != crate::rules::NONE_TOKEN)
        {
            end = crate::rules::byte_span_of_token(c, name).start;
        }
    }
    for kid in &kids[idx + 1..] {
        end = end.min(c.byte_span(*kid).start);
        break;
    }
    let text = c.unit.source.get(start..end)?.trim();
    (text.starts_with('(') && text.ends_with(')')).then_some(text)
}

/// Parses the record spelling into the canonical face: positional
/// fields render bare (the written positional name `num one` is
/// dropped — `getDisplayString` keeps positional fields unnamed), the
/// named section renders `type name` sorted by field name, nullability
/// is washed out (upstream `promoteToNonNull`).
fn record_shape(text: &str) -> Option<RecordShape> {
    let inner = text.trim().trim_end_matches('?');
    let inner = inner.strip_prefix('(')?.strip_suffix(')')?.trim();
    let (pos_text, named_text) = match top_level_index(inner, '{') {
        Some(i) => (&inner[..i], inner[i + 1..].trim_end_matches('}')),
        None => (inner, ""),
    };
    let pos: Vec<String> = split_top_level(pos_text)
        .into_iter()
        .filter_map(positional_field)
        .collect();
    let mut named: Vec<(String, String)> = split_top_level(named_text)
        .into_iter()
        .filter_map(named_field)
        .collect();
    named.sort_by(|a, b| a.0.cmp(&b.0));
    let mut display = String::from("(");
    if !pos.is_empty() {
        display.push_str(&pos.join(", "));
    }
    if !named.is_empty() {
        if !pos.is_empty() {
            display.push_str(", ");
        }
        display.push('{');
        display.push_str(
            &named
                .iter()
                .map(|(name, ty)| format!("{ty} {name}"))
                .collect::<Vec<_>>()
                .join(", "),
        );
        display.push('}');
    }
    display.push(')');
    Some(RecordShape {
        display,
        pos,
        named,
    })
}

/// The first top-level occurrence of `want` — depth counted over the
/// bracket/type-argument delimiters. The `want` arm leads so a record
/// opening at its named-brace (`({int one})`'s inner text) matches at
/// depth zero.
fn top_level_index(inner: &str, want: char) -> Option<usize> {
    let mut depth = 0usize;
    for (i, ch) in inner.char_indices() {
        if ch == want && depth == 0 {
            return Some(i);
        }
        match ch {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' | '>' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    None
}

/// Splits on top-level commas (nested records, type arguments stay).
fn split_top_level(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (i, ch) in text.char_indices() {
        match ch {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' | '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                out.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&text[start..]);
    out
}

fn is_bare_identifier(word: &str) -> bool {
    let mut chars = word.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// A positional field's type spelling: a trailing bare identifier after
/// whitespace is the field's name (`num one`) — dropped from the face.
fn positional_field(item: &str) -> Option<String> {
    let spelling = item.trim();
    if spelling.is_empty() {
        return None;
    }
    let spelling = match spelling.rsplit_once(' ') {
        Some((ty, tail)) if is_bare_identifier(tail) => ty.trim(),
        _ => spelling,
    };
    let spelling = spelling.trim_end_matches('?').trim();
    (!spelling.is_empty()).then(|| spelling.to_owned())
}

/// A named field `(name, type spelling)` off the `type name` item.
fn named_field(item: &str) -> Option<(String, String)> {
    let spelling = item.trim();
    if spelling.is_empty() {
        return None;
    }
    let (ty, name) = spelling.rsplit_once(' ')?;
    let ty = ty.trim().trim_end_matches('?').trim();
    let name = name.trim().trim_end_matches('?');
    (is_bare_identifier(name) && !ty.is_empty()).then(|| (name.to_owned(), ty.to_owned()))
}

/// The type spelling's head name (`List` of `List<int>`) — the name the
/// comparability walk reads.
fn type_head(spelling: &str) -> &str {
    spelling.split('<').next().unwrap_or(spelling).trim()
}

/// The record arm's comparability (`isAssignableTo` either direction,
/// `dart_type_utilities.dart:271-273`): the positional arities and the
/// named field-name sets must match, and every pairwise field face must
/// be comparable. Both directions collapse into the pairwise form — a
/// pair is comparable exactly when some direction subtypes.
fn records_related(c: &Check<'_>, l: &RecordShape, r: &RecordShape) -> bool {
    if l.pos.len() != r.pos.len() || l.named.len() != r.named.len() {
        return false;
    }
    for (l, r) in l.pos.iter().zip(r.pos.iter()) {
        if named_unrelated(c, type_head(l), type_head(r)) {
            return false;
        }
    }
    for ((lname, lty), (rname, rty)) in l.named.iter().zip(r.named.iter()) {
        if lname != rname || named_unrelated(c, type_head(lty), type_head(rty)) {
            return false;
        }
    }
    true
}

/// The bridge `Ty` as a face: `Named` keeps its head name (the arg
/// list does not decompose through `Analysis` — `List<int>` under-
/// reports the arg walk); `Record`/`Function` carry no readable shape
/// and stay `Top` (the written face answers those shapes first).
fn ty_face<'a>(
    c: &'a Check<'a>,
    analysis: &dartq_semantic::Analysis<'a>,
    ty: Ty,
) -> OpFace<'a> {
    match ty {
        Ty::Named { name, .. } => {
            let spelled = analysis.strings().resolve(name);
            OpFace::Named {
                name: spelled,
                args: Vec::new(),
                is_typevar: is_declared_typevar(c, spelled),
                bound: typevar_bound(c, spelled),
            }
        }
        _ => OpFace::Top,
    }
}

fn named_face<'a>(c: &'a Check<'a>, name: &'a str) -> OpFace<'a> {
    OpFace::Named {
        name,
        args: Vec::new(),
        is_typevar: is_declared_typevar(c, name),
        bound: typevar_bound(c, name),
    }
}

/// Whether `name` is a declared type variable of the enclosing
/// declaration (an `<A>`/`<A extends …>` `TypeVariables` row at any
/// ancestor — function, method, or class-like header).
fn is_declared_typevar(c: &Check<'_>, name: &str) -> bool {
    typevar_row(c, name).is_some()
}

/// The `TypeVariable` row declaring `name` in the enclosing
/// declaration's `TypeVariables`, if any.
fn typevar_row(c: &Check<'_>, name: &str) -> Option<u32> {
    for &ancestor in c.path.iter().rev() {
        // The header rows tile beside their declaration row under the
        // wrapper (the production shape) — scan the ancestor's own
        // children and its parent's sibling run alike.
        let scopes = c
            .unit
            .ast
            .parent(ancestor)
            .into_iter()
            .chain(core::iter::once(ancestor));
        for scope in scopes {
            for vars in c
                .collect_direct(scope)
                .into_iter()
                .filter(|&kid| c.kind(kid) == NodeKind::TypeVariables)
            {
                for tv in c.collect_direct(vars) {
                    if c.kind(tv) != NodeKind::TypeVariable {
                        continue;
                    }
                    if let Some(found) = typevar_in(c, tv, name) {
                        return Some(found);
                    }
                }
            }
        }
    }
    None
}

/// The declared-name match over one `TypeVariable` row and the later
/// variables of the same header nested inside it: the name anchors each
/// row's own head token (the parser mints the row at the name's span),
/// and the reverse-completion tiling places every subsequent variable
/// of the header inside the previous row's subtree.
fn typevar_in(c: &Check<'_>, row: u32, name: &str) -> Option<u32> {
    if c.lexeme(c.head_token(row)) == name {
        return Some(row);
    }
    for kid in c.collect_direct(row) {
        if c.kind(kid) != NodeKind::TypeVariable {
            continue;
        }
        if let Some(found) = typevar_in(c, kid, name) {
            return Some(found);
        }
    }
    None
}

/// The declared bound of a type variable visible at the operand —
/// the enclosing declaration's `TypeVariables` row (`<A extends int>`
/// → `A` maps `int`). `None` for an unbounded or non-typevar name.
fn typevar_bound(c: &Check<'_>, name: &str) -> Option<String> {
    let tv = typevar_row(c, name)?;
    c.collect_direct(tv)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::Type)
        .map(|t| c.lexeme(c.head_token(t)).to_owned())
}

/// The [`NodeKind`] of a same-file declaration named `name` (enum vs
/// class vs mixin — the `EnumElement` arm of
/// `interfaceTypesAreUnrelated`).
fn same_file_kind(c: &Check<'_>, name: &str) -> Option<NodeKind> {
    crate::rules::same_file_declaration(c, name).map(|row| c.kind(row))
}

/// The relational-pattern face (:69-83): the operand's static type
/// against the matched value type, reported at the pattern
/// (`reportAtNode`). The operand rows precede the childless pattern
/// row (the operator token first, then the operand parsing — the
/// pattern row opens at the operand's first token and seals past it,
/// so the report runs from the operator through the row end).
fn pattern_face(c: &mut Check<'_>) {
    let node = c.node();
    let Some(op) = c.field(node, PayloadField::Operator) else {
        return;
    };
    if !matches!(c.lexeme(op), "==" | "!=") {
        return;
    }
    let Some(parent) = c.ancestor(1) else {
        return;
    };
    let kids = c.collect_direct(parent);
    let Some(pos) = kids.iter().position(|&kid| kid == node) else {
        return;
    };
    // The operand's type: a scalar literal or a bare read row — the
    // binary face's bridge shapes. A `null` operand or an undecidable
    // run stays silent.
    let run: Vec<u32> = kids[..pos].to_vec();
    let Some(operand_ty) = operand_static_ty(c, &run) else {
        return;
    };
    // The matched value type, keyed at the switch scrutinee read's own
    // span; a non-lone-identifier scrutinee carries no row.
    let Some(analysis) = c.semantics() else {
        return;
    };
    let Some(scrutinee) = switch_scrutinee_read(c) else {
        return;
    };
    let Some(matched_ty) = analysis.matched_value_type(query_site(c, scrutinee)) else {
        return;
    };
    // The faces ride the same `OpFace` verdict as the binary face — a
    // bare `Ty` names the head (arg lists and record shapes do not
    // decompose through `Analysis`, the documented under-report).
    let operand = ty_face(c, analysis, operand_ty);
    let matched = ty_face(c, analysis, matched_ty);
    if !unrelated(c, &matched, &operand) {
        return; // `_comparable(valueType, operandType)` (:82)
    }
    let operand = operand.display();
    let matched = matched.display();
    let range = utf16_token_span(c, op).start..c.utf16_span(node).end;
    c.report_texts(
        range,
        PATTERN_PROBLEM,
        "Try changing one or both of the operands.",
        &[operand.as_str(), matched.as_str()],
    );
}

/// The operand run's static type: a scalar literal, or the bridge read
/// face over a lone identifier (the `operandType` shapes this face's
/// display can render exactly).
fn operand_static_ty(c: &Check<'_>, run: &[u32]) -> Option<Ty> {
    let named = |name: &'static str| -> Option<Ty> {
        let analysis = c.semantics()?;
        Some(Ty::Named {
            name: analysis.strings().intern(name),
            nullable: false,
            args: ArgListId::EMPTY,
        })
    };
    let root = static_type_faces::run_root(c, run)?;
    match c.kind(root) {
        NodeKind::LiteralInt => named("int"),
        NodeKind::LiteralDouble => named("double"),
        NodeKind::StringLiteral => named("String"),
        NodeKind::LiteralBool => named("bool"),
        _ => {
            let identifier_run = slice::access_run(c, run);
            let [only] = identifier_run.as_slice() else {
                return None;
            };
            if c.kind(*only) != NodeKind::Identifier {
                return None;
            }
            c.semantics()?.type_of(query_site(c, *only))
        }
    }
}

/// Whether the left operand's resolved `==` override declares a
/// non-`bool` return (the official gate's silhouette face): a
/// bare-name operand only — the override lookup needs the receiver's
/// own read row.
fn equality_override_silences(c: &Check<'_>, left: &[u32]) -> bool {
    let Some(analysis) = c.semantics() else {
        return false;
    };
    let only = match slice::access_run(c, left).as_slice() {
        [only] if c.kind(*only) == NodeKind::Identifier => *only,
        _ => return false,
    };
    let site = query_site(c, only);
    if !matches!(analysis.type_of(site), Some(Ty::Named { .. })) {
        return false;
    }
    match analysis.member_of(site, analysis.strings().intern("==")) {
        Some(binding) => !matches!(
            binding.entry.return_ty,
            Ty::Named {
                name,
                nullable: false,
                ..
            } if analysis.strings().resolve(name) == "bool"
        ),
        None => false,
    }
}

/// The `extensionTypeErasure` of a name (:166-167): an
/// `extension type A(String value)` erases to `String`; every other
/// name stays. The declaration's name may ride the row's `Name` word
/// (no `Identifier` child), and the representation type nests inside
/// the primary constructor's parameter list — never a direct `Type`
/// child — so the match reads the payload name and the representation
/// walks the row's tokens to the `(`.
fn ext_type_erasure<'a>(c: &'a Check<'a>, name: &'a str) -> &'a str {
    let Some(root) = c.unit.ast.root() else {
        return name;
    };
    for top in c.collect_direct(root) {
        for kid in c.collect_direct(top) {
            if c.kind(kid) != NodeKind::ExtensionTypeDeclaration {
                continue;
            }
            let named = c
                .field(kid, PayloadField::Name)
                .filter(|&tok| tok != crate::rules::NONE_TOKEN && c.lexeme(tok) == name)
                .is_some()
                || c.collect_direct(kid).iter().any(|&k| {
                    c.kind(k) == NodeKind::Identifier && c.lexeme(c.head_token(k)) == name
                });
            if !named {
                continue;
            }
            let end = c.unit.ast.token_range(kid).end;
            let mut token = c.head_token(kid);
            for _ in 0..64 {
                if token >= end {
                    break;
                }
                match c.lexeme(token) {
                    "(" => {
                        let rep = token + 1;
                        return if rep < end { c.lexeme(rep) } else { name };
                    }
                    "{" | ";" => break,
                    _ => {}
                }
                token += 1;
            }
        }
    }
    name
}

/// The comparability gate (`typesAreUnrelated`,
/// `util/dart_type_utilities.dart:163-283`): `dynamic`/`Null` relate to
/// everything; `isSubtypeOf` either direction relates; two interface
/// types run the direct-supertype matrix (:244-280), two type
/// variables recurse on their bounds (:198-204), a record is unrelated
/// to anything but the same-shaped record.
fn unrelated(c: &Check<'_>, left: &OpFace<'_>, right: &OpFace<'_>) -> bool {
    // `DynamicType`/`isBottom`/`Null` relate to everything (:170-176);
    // `Null` against a non-nullable target is unrelated (:178-183 — the
    // face has no nullability granularity, the documented miss).
    if matches!(left, OpFace::Top) || matches!(right, OpFace::Top) {
        return false;
    }
    let (
        OpFace::Named {
            name: lname0,
            args: largs,
            is_typevar: ltvar,
            bound: lbound,
        },
        OpFace::Named {
            name: rname0,
            args: rargs,
            is_typevar: rtvar,
            bound: rbound,
        },
    ) = (left, right)
    else {
        if let (OpFace::Record(l), OpFace::Record(r)) = (left, right) {
            return !records_related(c, l, r);
        }
        // A record is unrelated to any non-record type (:271-273).
        return true;
    };
    // `extensionTypeErasure` (:166-167): an extension-type operand reads
    // its representation type before every comparison.
    let lname = ext_type_erasure(c, lname0);
    let rname = ext_type_erasure(c, rname0);
    // The same element (mod generics): the pairwise type-argument walk
    // decides (:244-263) — same-head generics only relate when every
    // argument pair does (`List<A>` vs `List<B>` with unrelated bounds
    // stays unrelated). Equal empty-arg faces (`E` vs `E`) walk zero
    // pairs and stay comparable, exactly the `isSubtypeOf` answer.
    if lname == rname {
        return largs
            .iter()
            .zip(rargs.iter())
            .any(|(l, r)| named_unrelated(c, l, r));
    }
    // `isSubtypeOf` either direction relates (:184-189): a type
    // variable reaching the other through its bound, or a same-file
    // supertype chain containing the other name.
    if names_related(c, lname, rname, lbound.as_deref(), rbound.as_deref()) {
        return false;
    }
    if *ltvar && *rtvar {
        // Two type variables: a `null` bound relates (:200-203); else
        // the bounds' own face decides.
        let (Some(lb), Some(rb)) = (lbound.as_deref(), rbound.as_deref()) else {
            return false;
        };
        return named_unrelated(c, lb, rb);
    }
    // A type variable against an interface type falls through the
    // upstream chain to `return false` (:208-215) — comparable.
    if *ltvar || *rtvar {
        return false;
    }
    named_unrelated(c, lname, rname)
}

/// The subtype-style relatedness of two names: equal, `Object`/`dynamic`
/// either side, the `num` chain (`int`/`double` subtype `num`), a type
/// variable's bound reaching the other (`B extends A` — `B` subtypes
/// `A`), or the same-file transitive supertype chain containing the
/// other name (`isSubtypeOf` :184-189).
fn names_related(
    c: &Check<'_>,
    left: &str,
    right: &str,
    lbound: Option<&str>,
    rbound: Option<&str>,
) -> bool {
    if left == right {
        return true;
    }
    if matches!(left, "Object" | "dynamic") || matches!(right, "Object" | "dynamic") {
        return true;
    }
    let core_super = |name: &str| match name {
        "int" | "double" => Some("num"),
        "num" | "bool" | "String" | "Record" | "Function" | "Null" => Some("Object"),
        _ => None,
    };
    // A bound reaching the other name is the `B extends A` subtype face.
    if lbound == Some(right) || rbound == Some(left) {
        return true;
    }
    // The core chains: `int`/`double` subtype `num`/`Object`.
    let mut chain = core_super(left);
    let mut hops = 0;
    while let Some(s) = chain {
        if s == right {
            return true;
        }
        hops += 1;
        chain = if hops > 2 { None } else { core_super(s) };
    }
    let mut chain = core_super(right);
    let mut hops = 0;
    while let Some(s) = chain {
        if s == left {
            return true;
        }
        hops += 1;
        chain = if hops > 2 { None } else { core_super(s) };
    }
    // The same-file transitive closures (a cross-file link could relate
    // the names behind an unindexed edge — then the chain is incomplete
    // and the face stays silent).
    let left_chain = same_file::supertype_closure(c, left);
    let right_chain = same_file::supertype_closure(c, right);
    left_chain
        .as_ref()
        .is_some_and(|chain| chain.iter().any(|n| n == right))
        || right_chain
            .as_ref()
            .is_some_and(|chain| chain.iter().any(|n| n == left))
}

/// The direct written supertype of a name — the first entry of its
/// same-file supertype closure (`class D2 extends D1` → `D1`), `Object`
/// for a clause-less class, `Enum` for an enum (the implicit
/// `extends Enum`), the core chains for the scalars, `"?"` for a
/// cross-file/unreadable declaration (never equal — the unrelated
/// direction stays decidable only through readable faces).
fn direct_super(c: &Check<'_>, name: &str) -> String {
    match same_file_kind(c, name) {
        Some(NodeKind::EnumDeclaration) => "Enum".to_owned(),
        Some(_) => same_file::supertype_closure(c, name)
            .and_then(|chain| chain.first().cloned())
            .unwrap_or_else(|| "Object".to_owned()),
        None => match name {
            "int" | "double" => "num".to_owned(),
            "num" | "bool" | "String" | "Record" | "Function" | "Null" => {
                "Object".to_owned()
            }
            _ => "?".to_owned(),
        },
    }
}

/// The plain two-name interface verdict used by the arg/bound
/// recursions: distinct names are unrelated when the direct supers
/// differ, the left is an enum, the shared super is `ProtobufEnum`, or
/// the left's direct super is `Object` (:264-280). `int`/`double` map
/// to `num`, the other scalars to `Object`, an enum to `Enum`, a
/// cross-file name to `"?"` (never equal — unrelated stays the
/// decidable direction only when the left face is readable).
fn named_unrelated(c: &Check<'_>, left: &str, right: &str) -> bool {
    if left == right {
        return false;
    }
    // The type-variable arm (:198-204): two type variables decide by
    // their declared bounds — a `null` bound relates. A type variable
    // against a named type reads the variable's bound as its face
    // (`A extends int` vs `String` decides `int`/`String`).
    let ltvar = is_declared_typevar(c, left);
    let rtvar = is_declared_typevar(c, right);
    if ltvar && rtvar {
        let (Some(lb), Some(rb)) = (typevar_bound(c, left), typevar_bound(c, right)) else {
            return false;
        };
        return named_unrelated(c, &lb, &rb);
    }
    // A type variable against an interface type falls through the
    // upstream `else if` chain to `return false` (:208-215) — always
    // comparable.
    if ltvar || rtvar {
        return false;
    }
    // `isSubtypeOf` either direction first (:184-189): the core chains
    // (`int <: num`) and the same-file supertype closures relate a pair
    // before the direct-supertype matrix applies — the matrix's
    // `leftSuper != rightSuper` arm presumes the subtype answer has run
    // (`int`/`num` must not read as unrelated through `num` vs `Object`).
    if names_related(c, left, right, None, None) {
        return false;
    }
    let ls = direct_super(c, left);
    let rs = direct_super(c, right);
    if ls != rs {
        return true;
    }
    if same_file_kind(c, left) == Some(NodeKind::EnumDeclaration) {
        return true;
    }
    if ls == "ProtobufEnum" {
        return true;
    }
    ls == "Object"
}

#[cfg(test)]
mod tests {
    use crate::testsupport::{Fixture, one_rule};

    /// The whole upstream oracle corpus — `UnrelatedTypeEqualityChecksTest`
    /// (46 cases, `pkg/linter/test/rules/unrelated_type_equality_checks_test.dart`)
    /// plus the 12 `messages.yaml` doc cases: 58 total,
    /// `conformance/status.tsv` `lint unrelated_type_equality_checks 58 …`.
    /// Each entry is the exact materialized case source (the test-file
    /// cases carry a leading blank line, the doc files do not) and the
    /// oracle's `(line, column, length)` for every
    /// `unrelated_type_equality_checks` row (`expected.tsv`). Only the
    /// rule's own rows are pinned — the co-reported
    /// `unused_local_variable`/`dead_code`/`class_used_as_mixin`/…
    /// rows are other faces.
    ///
    /// The `package:fixnum`/`package:protobuf` cases stand in for the
    /// import-bound names with the name-level face (the mock packages
    /// are not mounted in a single-source fixture): `Int32`/`Int64`
    /// keep their head names (`isFixnumIntX` reads them off the face)
    /// and the `ProtobufEnum` subclasses resolve through the shared
    /// `"Object"` super face — the verdict and the oracle rows are the
    /// same as in the census.
    ///
    /// Every mismatch is collected before the single panic so one run
    /// shows the whole deviation set.
    #[test]
    fn unrelated_type_equality_checks_oracle_corpus_pins() {
        // (case, source, oracle rows as (line, column, length)).
        let cases: &[(&str, &str, &[(u32, u32, u32)])] = &[
            (
                "assignment_ok",
                "\nvoid m(int? a1, num a2) {\n  var b1 = a1 == a2;\n  var b2 = a2 == a1;\n}\n",
                &[],
            ),
            ("dynamic_andInt", "\nvoid f(dynamic x) {\n  if (x == 0) {}\n}\n", &[]),
            (
                "extensionTypes_same",
                "\nvoid m(A a, B b) {\n  if (a == b) {}\n}\nextension type A(String value) {}\nextension type B(String value) {}\n",
                &[],
            ),
            (
                "extensionTypes_sameRepresentationType",
                "\nvoid m(A a, B b) {\n  if (a == b) {}\n}\nextension type A(String value) {}\nextension type B(String value) {}\n",
                &[],
            ),
            (
                "extensionTypes_unrelatedRepresentationType",
                "\nvoid m(A a, B b) {\n  if (a == b) {}\n}\nextension type A(String value) {}\nextension type B(int value) {}\n",
                &[(3, 9, 2)],
            ),
            (
                "fixnum_int32_leftSide",
                "\nimport 'package:fixnum/fixnum.dart';\n\nvoid f(Int32 p) {\n  if (p == 0) {}\n}\n",
                &[],
            ),
            (
                "fixnum_int32_rightSide",
                "\nimport 'package:fixnum/fixnum.dart';\n\nvoid f(Int32 p) {\n  if (0 == p) {}\n}\n",
                &[(5, 9, 2)],
            ),
            (
                "fixnum_int64_leftSide",
                "\nimport 'package:fixnum/fixnum.dart';\n\nvoid f(Int64 p) {\n  if (p == 0) {}\n}\n",
                &[],
            ),
            (
                "fixnum_int64_rightSide",
                "\nimport 'package:fixnum/fixnum.dart';\n\nvoid f(Int64 p) {\n  if (0 == p) {}\n}\n",
                &[(5, 9, 2)],
            ),
            (
                "futureOfDynamic_andFutureOfVoid",
                "\nvoid f(Future<dynamic> x, Future<void> y) {\n  if (x == y) {}\n}\n",
                &[],
            ),
            ("int_andInt", "\nvoid f() {\n  if (1 == 0) {}\n}\n", &[]),
            (
                "mixin_andTypeImplementingMixin",
                "\nvoid f(M x, C y) {\n  if (x == y) {}\n}\nmixin M {}\nclass C implements M {}\n",
                &[],
            ),
            (
                "mixin_andTypeWithMixin",
                "\nvoid f(M x, C y) {\n  if (x == y) {}\n}\nmixin M {}\nclass C with M {}\n",
                &[],
            ),
            (
                "mixins",
                "\nvoid f(M1 m1, M2 m2) {\n  if (m1 == m2) {}\n}\nmixin M1 {}\nmixin M2 {}\n",
                &[(3, 10, 2)],
            ),
            (
                "mixins_ok",
                "\nvoid f(M1 m1, M2 m2) {\n  if (m1 == m2) {}\n}\nmixin M1 on M2 {}\nmixin M2 {}\n",
                &[],
            ),
            (
                "object_andArbitraryType",
                "\nvoid f(Object x, C y) {\n  if (x == y) {}\n}\nclass C {}\n",
                &[],
            ),
            (
                "object_andDynamic",
                "\nvoid f(Object x, dynamic y) {\n  if (x == y) {}\n}\n",
                &[],
            ),
            (
                "oneEnum_andSameEnum",
                "\nvoid f(E x, E y) {\n  if (x == y) {}\n}\nenum E { one, two; }\n",
                &[],
            ),
            (
                "oneEnum_andUnrelatedEnum",
                "\nvoid f(E x, F y) {\n  if (x == y) {}\n}\nenum E { one, two; }\nenum F { three, four; }\n",
                &[(3, 9, 2)],
            ),
            (
                "oneTypeVariable_andOneSubtypeTypeVariable",
                "\nvoid f<A, B extends A>(A a, B b) {\n  if (a == b) {}\n}\n",
                &[],
            ),
            (
                "oneType_andSubtype",
                "\nvoid f(C x, D y) {\n  if (x == y) {}\n}\nclass C {}\nclass D extends C {}\n",
                &[],
            ),
            (
                "protobufEnum_differentEnums",
                "\nimport 'package:protobuf/src/protobuf/protobuf_enum.dart';\n\nvoid f(E1 one, E2 two) {\n  print(one == two);\n}\nclass E1 extends ProtobufEnum {}\nclass E2 extends ProtobufEnum {}\n",
                &[(5, 13, 2)],
            ),
            (
                "protobufEnum_sameEnum",
                "\nimport 'package:protobuf/src/protobuf/protobuf_enum.dart';\n\nvoid f(E one, E two) {\n  print(one == two);\n}\nclass E extends ProtobufEnum {}\n",
                &[],
            ),
            (
                "recordAndInterfaceType_unrelated",
                "\nbool f((int, int) a, String b) => a == b;\n",
                &[(2, 37, 2)],
            ),
            (
                "recordsWithNamedAndPositional_related",
                "\nbool f((int, {int two}) a, (num one, {num two}) b) => a == b;\n",
                &[],
            ),
            (
                "recordsWithNamedAndPositional_unrelated",
                "\nbool f((int, {int two}) a, (String one, {String two}) b) => a == b;\n",
                &[(2, 63, 2)],
            ),
            (
                "recordsWithNamed_related",
                "\nbool f(({int one, int two}) a, ({num two, num one}) b) => a == b;\n",
                &[],
            ),
            (
                "recordsWithNamed_unrelated",
                "\nbool f(({int one, int two}) a, ({String one, String two}) b) => a == b;\n",
                &[(2, 67, 2)],
            ),
            (
                "records_related",
                "\nbool f((int, int) a, (num, num) b) => a == b;\n",
                &[],
            ),
            (
                "records_unrelated",
                "\nbool f((int, int) a, (String, String) b) => a == b;\n",
                &[(2, 47, 2)],
            ),
            (
                "string_andInt",
                "\nvoid f() {\n  if ('foo' == 1) {}\n}\n",
                &[(3, 13, 2)],
            ),
            (
                "string_andNull",
                "\nvoid f() {\n  if ('foo' == null) {}\n}\n",
                &[],
            ),
            (
                "string_andString",
                "\nvoid f() {\n  if ('foo' == 'bar') {}\n}\n",
                &[],
            ),
            (
                "switchExpression",
                "\nconst space = 32;\n\nString f(int char) {\n  return switch (char) {\n    == 'space' => 'space',\n  };\n}\n",
                &[(6, 5, 10)],
            ),
            (
                "switchExpression_lessEq_ok",
                "\nString f(int i) {\n  return switch (i) {\n    <= 1 => 'one',\n  };\n}\n",
                &[],
            ),
            (
                "switchExpression_notEq",
                "\nconst space = 32;\n\nString f(int char) {\n  return switch (char) {\n    != 'space' => 'space',\n  };\n}\n",
                &[(6, 5, 10)],
            ),
            (
                "switchExpression_ok",
                "\nString f(String char) {\n  return switch (char) {\n    == 'space' => 'space',\n  };\n}\n",
                &[],
            ),
            (
                "twoListsOfTypeVariables_unrelatedBounds",
                "\nvoid f<A extends int, B extends bool>(List<A> a, List<B> b) {\n  if (a == b) {}\n}\n",
                &[(3, 9, 2)],
            ),
            (
                "twoListsOfUnrelatedTypeVariables",
                "\nvoid f<A, B>(List<A> a, List<B> b) {\n  if (a == b) {}\n}\n",
                &[],
            ),
            (
                "twoMapsOfUnrelatedTypeVariableKeys",
                "\nvoid f<A, B>(Map<A, int> a, Map<B, int> b) {\n  if (a == b) {}\n}\n",
                &[],
            ),
            (
                "twoMapsOfUnrelatedTypeVariableValues",
                "\nvoid f<A, B>(Map<int, A> a, Map<int, B> b) {\n  if (a == b) {}\n}\n",
                &[],
            ),
            (
                "twoSubtypesOfSharedDistantSupertype",
                "\nvoid f(D2 x, E2 y) {\n  if (x == y) {}\n}\nclass C {}\nclass D1 extends C {}\nclass E1 extends C {}\nclass D2 extends D1 {}\nclass E2 extends E1 {}\n",
                &[(3, 9, 2)],
            ),
            (
                "twoSubtypesOfSharedSupertype",
                "\nvoid f(D x, E y) {\n  if (x == y) {}\n}\nclass C {}\nclass D extends C {}\nclass E extends C {}\n",
                &[],
            ),
            (
                "twoTypeVariables_sameBounds",
                "\nvoid f<A extends int, B extends int>(A a, B b) {\n  if (a == b) {}\n}\n",
                &[],
            ),
            (
                "twoTypeVariables_unrelatedBounds",
                "\nvoid f<A extends int, B extends bool>(A a, B b) {\n  if (a == b) {}\n}\n",
                &[(3, 9, 2)],
            ),
            (
                "twoUnrelatedTypeVariables",
                "\nvoid f<A, B>(A a, B b) {\n  if (a == b) {}\n}\n",
                &[],
            ),
            (
                "doc_bad_0",
                "bool f(String s) {\n  return s == 1;\n}\n",
                &[(2, 12, 2)],
            ),
            (
                "doc_bad_1",
                "void someFunction() {\n  var x = '1';\n  if (x == 1) print('someFunction'); // LINT\n}\n",
                &[(3, 9, 2)],
            ),
            (
                "doc_bad_2",
                "void someFunction1() {\n  String x = '1';\n  if (x == 1) print('someFunction1'); // LINT\n}\n",
                &[(3, 9, 2)],
            ),
            (
                "doc_bad_3",
                "void someFunction13(DerivedClass2 instance) {\n  var other = DerivedClass3();\n  if (other == instance) print('someFunction13'); // LINT\n}\nclass ClassBase {}\nclass DerivedClass1 extends ClassBase {}\nabstract class Mixin {}\nclass DerivedClass2 extends ClassBase with Mixin {}\nclass DerivedClass3 extends ClassBase implements Mixin {}\n",
                &[],
            ),
            (
                "doc_good_0",
                "bool f(String s) {\n  return s.length == 1;\n}\n",
                &[],
            ),
            (
                "doc_good_1",
                "void someFunction2() {\n  var x = '1';\n  var y = '2';\n  if (x == y) print(someFunction2); // OK\n}\n",
                &[],
            ),
            (
                "doc_good_2",
                "void someFunction3() {\n  for (var i = 0; i < 10; i++) {\n    if (i == 0) print(someFunction3); // OK\n  }\n}\n",
                &[],
            ),
            (
                "doc_good_3",
                "void someFunction4() {\n  var x = '1';\n  if (x == null) print(someFunction4); // OK\n}\n",
                &[],
            ),
            (
                "doc_good_4",
                "void someFunction7() {\n  List someList;\n  if (someList.length == 0) print('someFunction7'); // OK\n}\n",
                &[],
            ),
            (
                "doc_good_5",
                "void someFunction8(ClassBase instance) {\n  DerivedClass1 other;\n  if (other == instance) print('someFunction8'); // OK\n}\n",
                &[],
            ),
            (
                "doc_good_6",
                "void someFunction10(unknown) {\n  var what = unknown - 1;\n  for (var index = 0; index < unknown; index++) {\n    if (what == index) print('someFunction10'); // OK\n  }\n}\n",
                &[],
            ),
            (
                "doc_good_7",
                "void someFunction11(Mixin instance) {\n  var other = DerivedClass2();\n  if (other == instance) print('someFunction11'); // OK\n  if (other != instance) print('!someFunction11'); // OK\n}\nclass ClassBase {}\nabstract class Mixin {}\nclass DerivedClass2 extends ClassBase with Mixin {}\n",
                &[],
            ),
        ];
        // The UTF-16 offset of a 1-based `(line, column)` in an ASCII source.
        fn offset(source: &str, line: u32, column: u32) -> u32 {
            let line_start: usize = source
                .split_inclusive('\n')
                .take(line as usize - 1)
                .map(str::len)
                .sum();
            (line_start + column as usize - 1) as u32
        }
        let mut mismatches = Vec::new();
        for (case, source, oracle) in cases {
            let f = Fixture::parse(source);
            let mut lints = f.run_semantic(&one_rule("unrelated_type_equality_checks"));
            lints.sort_by_key(|lint| lint.range.start);
            let got: Vec<(u32, u32)> = lints
                .iter()
                .map(|lint| (lint.range.start, lint.range.end))
                .collect();
            let want: Vec<(u32, u32)> = oracle
                .iter()
                .map(|&(line, column, length)| {
                    let start = offset(source, line, column);
                    (start, start + length)
                })
                .collect();
            if got != want {
                mismatches.push(format!("{case}: want {want:?} got {got:?}"));
            }
        }
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    }
}
