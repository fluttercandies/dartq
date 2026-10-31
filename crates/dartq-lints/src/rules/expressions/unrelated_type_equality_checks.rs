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
/// relation (`typesAreUnrelated`): same name related; `Object`/
/// `dynamic` relate to everything; the `num` family relates
/// `int`/`double`; the four scalar faces are final (nothing else
/// relates to them); any other pair with a same-file class-like name
/// stays silent — the supertype chain, or a cross-file link behind
/// it, can relate the names in ways this face cannot see. The bool
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
/// a record's source-verbatim display, or a type variable with its
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
    /// A record type: the source-verbatim display (`(int, int)`).
    Record(String),
    /// `dynamic`/`Null`/undecidable — suppresses (`typesAreUnrelated`
    /// returns false on `DynamicType`/`isBottom`/null).
    Top,
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
            Self::Record(text) => text.clone(),
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
/// renders its source slice verbatim (`(int, int)`, `({int one, …})`),
/// a generic keeps the argument head names, a type variable carries
/// its bound, an enum head marks the enum arm.
fn written_face<'a>(c: &'a Check<'a>, type_row: u32) -> OpFace<'a> {
    // A record type row opens at `(` — its source slice is the exact
    // `getDisplayString` rendering for the named-field forms too.
    let span = c.byte_span(type_row);
    let text = &c.unit.source[span.start..span.end];
    if text.starts_with('(') {
        return OpFace::Record(text.trim_end_matches('?').to_owned());
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
        for vars in c
            .collect_direct(ancestor)
            .into_iter()
            .filter(|&kid| c.kind(kid) == NodeKind::TypeVariables)
        {
            for tv in c.collect_direct(vars) {
                if c.kind(tv) != NodeKind::TypeVariable {
                    continue;
                }
                // The declared name carries the declaration context; the
                // bound's `TypeReference` identifier shares the window and
                // must not count (`A extends int` — `int` is not a type
                // variable of this declaration).
                let named = c.collect_direct(tv).iter().any(|&kid| {
                    c.kind(kid) == NodeKind::Identifier
                        && c.field(kid, PayloadField::Context)
                            == Some(
                                dartq_parser::IdentifierContext::TypeVariableDeclaration as u32
                            )
                        && c.lexeme(c.head_token(kid)) == name
                });
                if named {
                    return Some(tv);
                }
            }
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
/// name stays. The extension-type declaration indexes outside the
/// class-like table, so the root scan reads it directly.
fn ext_type_erasure<'a>(c: &'a Check<'a>, name: &'a str) -> &'a str {
    let Some(root) = c.unit.ast.root() else {
        return name;
    };
    for top in c.collect_direct(root) {
        for kid in c.collect_direct(top) {
            if c.kind(kid) != NodeKind::ExtensionTypeDeclaration {
                continue;
            }
            let kids = c.collect_direct(kid);
            let named = kids.iter().any(|&k| {
                c.kind(k) == NodeKind::Identifier && c.lexeme(c.head_token(k)) == name
            });
            if !named {
                continue;
            }
            if let Some(rep) = kids.iter().find(|&&k| c.kind(k) == NodeKind::Type) {
                return c.lexeme(c.head_token(*rep));
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
    if let (OpFace::Record(l), OpFace::Record(r)) = (left, right) {
        return l != r;
    }
    if matches!(left, OpFace::Record(_)) != matches!(right, OpFace::Record(_)) {
        return true; // a record is unrelated to any non-record type
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
        return false;
    };
    // `extensionTypeErasure` (:166-167): an extension-type operand reads
    // its representation type before every comparison.
    let lname = ext_type_erasure(c, lname0);
    let rname = ext_type_erasure(c, rname0);
    // `isSubtypeOf` either direction relates (:184-189): same name, a
    // type variable reaching the other through its bound, or a
    // same-file supertype chain containing the other name.
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
    // Two interface types (:244-280): same element compares the
    // pairwise type arguments; otherwise the direct-supertype matrix.
    if lname == rname {
        return largs
            .iter()
            .zip(rargs.iter())
            .any(|(l, r)| named_unrelated(c, l, r));
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
