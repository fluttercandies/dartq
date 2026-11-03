use dartq_ast::NodeKind;
use dartq_scanner::TokenKind;
use dartq_semantic::model::{ArgListId, Ty};
use rustc_hash::FxHashSet;

use super::{Check, PayloadField, RuleSpec, query_site, slice};

const DART_IS_JS_PROBLEM: &str = "Runtime check between '{0}' and '{1}' checks whether a Dart value is a JS interop type, which might not be platform-consistent.";
const DART_IS_JS_CORRECTION: &str =
    "Try using 'isA' from 'dart:js_interop' to check if the Dart value is a JS interop type.";
const JS_IS_DART_PROBLEM: &str = "Runtime check between '{0}' and '{1}' checks whether a JS interop value is a Dart type, which might not be platform-consistent.";
// The official `invalidRuntimeCheckWithJsInteropTypesJsIsDart` template
// carries no correction message (`diagnostic.g.dart`); an empty text emits
// no correction.
const JS_IS_DART_CORRECTION: &str = "";
const DART_AS_JS_PROBLEM: &str = "Cast from '{0}' to '{1}' casts a Dart value to a JS interop type, which might not be platform-consistent.";
const DART_AS_JS_CORRECTION: &str = "Try using conversion methods from 'dart:js_interop' to convert between Dart types and JS interop types.";
const JS_AS_DART_PROBLEM: &str = "Cast from '{0}' to '{1}' casts a JS interop value to a Dart type, which might not be platform-consistent.";
const JS_AS_DART_CORRECTION: &str = "Try using conversion methods from 'dart:js_interop' to convert between JS interop types and Dart types.";
const CATCH_PROBLEM: &str = "Catch clause with type '{0}' checks whether the caught value is a JS interop type, which might not be platform-consistent.";
const CATCH_CORRECTION: &str = "Remove the type in the catch clause and try using 'isA' from 'dart:js_interop' within the catch block to check if the value is a JS interop type.";

/// The `dart:js_interop` extension types (upstream Dart SDK source,
/// `sdk/lib/js_interop/js_interop.dart`, the pinned 3.13 surface). The
/// official `_isJsInteropType` classifies by declaration library
/// (`element.isFromLibrary('dart:js_interop')`, :54-56); the SDK library
/// lives outside the element index, so its declarations answer
/// `container_has_annotation` with `None` (unknowable) and this closed name
/// set stands in for the library membership. A user declaration of the same
/// name resolves in the index and wins over the set (see
/// [`is_interop_name`]), so the cut only fires on names the unit cannot
/// resolve — the SDK type or a shadowing slip, the documented over-report
/// direction.
const DART_JS_INTEROP_TYPES: &[&str] = &[
    "ExternalDartReference",
    "JSAny",
    "JSArray",
    "JSArrayBuffer",
    "JSBigInt",
    "JSBoolean",
    "JSBoxedDartObject",
    "JSDataView",
    "JSExportedDartFunction",
    "JSFloat32Array",
    "JSFloat64Array",
    "JSFunction",
    "JSInt16Array",
    "JSInt32Array",
    "JSInt8Array",
    "JSIterable",
    "JSIterableProtocol",
    "JSIterator",
    "JSIteratorProtocol",
    "JSIteratorResult",
    "JSNumber",
    "JSObject",
    "JSPromise",
    "JSString",
    "JSSymbol",
    "JSTypedArray",
    "JSUint16Array",
    "JSUint32Array",
    "JSUint8Array",
    "JSUint8ClampedArray",
];

/// Official:
/// `pkg/linter/lib/src/rules/invalid_runtime_check_with_js_interop_types.dart`
/// — a runtime type test involving JS interop types that is not statically
/// guaranteed reports (:360-398, eight diagnostic variants; the visit
/// faces :429-470).
///
/// The interop classification (`_isJsInteropType`, :43-73 — extension
/// types, `@staticInterop`, SDK web libraries; recursive over type
/// arguments and type-parameter bounds, `InteropTypeChecker` :204-229)
/// reduces on this substrate to a name cut with three answer faces: a
/// `dart:js_interop` name ([`DART_JS_INTEROP_TYPES`]), a container carrying
/// the `@JS` annotation (same-file through the declaration's own
/// annotation star, cross-file through [`Analysis::container_has_annotation`]),
/// and a same-file extension type whose representation type is interop
/// (the `representation.type` arm, :57-67). A resolvable Dart container
/// (`Some(false)`) and any name outside the set are not interop; an
/// unresolvable name that is no `dart:js_interop` type stays silent.
///
/// The reported faces: the `is`/`as` test (:429-441, :457-470) when exactly
/// one side of the written pair contains an interop type, and the catch
/// clause (:443-455) whose written exception type contains one at any depth.
/// Subtype reasoning (`canBeSubtypeOf`, :406-425) is not modelled: a pair
/// where both sides contain interop types stays silent (the official
/// `JsIsInconsistent`/`JsIsUnrelated`/`JsAsIncompatible` variants are the
/// under-reported direction, none of them reachable from this substrate),
/// and a cast or check whose supertype side is `Object`/`dynamic` stays
/// silent (every JS interop type subtypes them, :348-360/:373-374).
/// Messages: `diagnostic.g.dart`
/// `invalidRuntimeCheckWithJsInteropTypesDart{Is,As}Js`,
/// `...JsIsDart`, `...CatchClauseJsInteropType`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "invalid_runtime_check_with_js_interop_types",
    &[
        NodeKind::CatchClause,
        NodeKind::IsExpression,
        NodeKind::AsExpression
    ],
    check,
);

fn check(c: &mut Check<'_>) {
    if c.kind(c.node()) == NodeKind::CatchClause {
        catch_clause(c);
    } else {
        is_or_as(c);
    }
}

/// `visitCatchClause` (:443-455): the written exception type reports when it
/// contains an interop type at any depth, at the type's own extent.
fn catch_clause(c: &mut Check<'_>) {
    let clause = c.node();
    let Some(type_row) = c
        .collect_direct(clause)
        .into_iter()
        .find(|&kid| is_type_row(c.kind(kid)))
    else {
        return;
    };
    let mut visiting = FxHashSet::default();
    if !type_contains_interop(c, type_row, &mut visiting) {
        return;
    }
    let display = type_text(c, type_row);
    if display.is_empty() {
        return;
    }
    c.report_texts(
        type_utf16(c, type_row),
        CATCH_PROBLEM,
        CATCH_CORRECTION,
        &[display.as_str()],
    );
}

/// `visitIsExpression`/`visitAsExpression` (:429-441, :457-470): the written
/// pair reports with `(dart type, js type)` when exactly one side contains
/// an interop type.
fn is_or_as(c: &mut Check<'_>) {
    let node = c.node();
    let is_check = c.kind(node) == NodeKind::IsExpression;
    // The left operand rows precede the operator container; the tested
    // type tiles inside it.
    let Some(parent) = c.ancestor(1) else {
        return;
    };
    // Left faces: a lone `this` (the enclosing Dart class) immediately
    // before the operator container — an arrow body's signature rows
    // tile before it in the member window, so only the trailing `this`
    // is the tested expression and everything earlier must be
    // signature — or a bare operand whose written type names a type.
    // A condition container's own window holds only the operator
    // container — the condition's operand rows tile flat in the
    // statement owner's window (the same probe posture
    // `avoid_double_and_int_checks` reads) — so a condition parent
    // takes the left run from the grandparent statement: the rows
    // inside the condition's token range that close before the
    // operator. The arrow-body return-type window (`preceding`)
    // exists only in the member-window shape.
    let (left, preceding): (Vec<u32>, Vec<u32>) = if c.kind(parent)
        == NodeKind::ParenthesizedCondition
    {
        let Some(stmt) = c.ancestor(2) else {
            return;
        };
        let cond = c.unit.ast.token_range(parent);
        let node_start = c.unit.ast.token_range(node).start;
        let run: Vec<u32> = c
            .collect_direct(stmt)
            .into_iter()
            .filter(|&row| {
                let span = c.unit.ast.token_range(row);
                span.start >= cond.start
                    && span.end <= node_start
                    && !matches!(c.kind(row), NodeKind::DotAccess | NodeKind::Qualified)
            })
            .collect();
        (run, Vec::new())
    } else {
        let kids = c.collect_direct(parent);
        let Some(pos) = kids.iter().position(|&kid| kid == node) else {
            return;
        };
        (
            kids[..pos]
                .iter()
                .copied()
                .filter(|&row| !matches!(c.kind(row), NodeKind::DotAccess | NodeKind::Qualified))
                .collect(),
            kids[..pos].to_vec(),
        )
    };
    // The operand row: the trailing row of the run, with a bare identifier's
    // tear-off group (`Identifier, NoTypeArguments, NoArguments, Send`)
    // folded back to its name row.
    let Some((root, prefix)) = trailing_operand(c, &left) else {
        return;
    };
    // The operand row decides both the display name and the report
    // start — `IsExpression`/`AsExpression` containers anchor at the
    // `is`/`as` operator, so `utf16_span(node)` would open at the
    // operator; the official `reportAtNode` opens at the operand.
    //
    // An arrow body's member window tiles the whole signature (return
    // type, name, parameters) before the operand, so the trailing rows are
    // the operand only when every earlier row is signature; a lone `this`
    // there is the enclosing Dart class, and a lone identifier takes its
    // written declaration's type. The condition shape keeps its own run
    // (the condition container holds nothing but the operator, so the
    // operand rows sit in the statement owner's window).
    let bare = prefix.iter().all(|&row| is_signature_row(c, row));
    let (dart_type, operand_row, left_type_row): (Option<String>, u32, Option<u32>) = if bare {
        match c.kind(root) {
            NodeKind::ThisExpression => (enclosing_class_name(c).map(str::to_owned), root, None),
            NodeKind::Identifier => {
                let name = c.lexeme(c.head_token(root));
                match written_type_row(c, root, name) {
                    Some(row) => (Some(type_text(c, row)), root, Some(row)),
                    None => (typed_identifier_display(c, root), root, None),
                }
            }
            _ => (None, root, None),
        }
    } else {
        let stripped = slice::access_run(c, &left);
        match stripped.as_slice() {
            [only] if c.kind(*only) == NodeKind::Identifier => {
                let name = c.lexeme(c.head_token(*only));
                match written_type_row(c, *only, name) {
                    Some(row) => (Some(type_text(c, row)), *only, Some(row)),
                    None => (typed_identifier_display(c, *only), *only, None),
                }
            }
            _ => (None, root, None),
        }
    };
    let Some(dart_type) = dart_type else {
        return;
    };
    // The tested type: the `Type` row naming a same-file `@JS` class,
    // in a file importing `dart:js_interop`. The type tiles inside the
    // operator container (the bridge begins it at the operator, before
    // the type parses); the sibling face — the rows completing before
    // the container seals — is accepted too. The head token after the
    // operator keyword tells the tested type from the member's return
    // type (token ids are source-ordered).
    let op_head = c.head_token(node);
    let mut candidates = c.collect_direct(node);
    candidates.extend_from_slice(&preceding);
    let Some(type_row) = candidates
        .into_iter()
        .rev()
        .find(|&row| is_type_row(c.kind(row)) && c.head_token(row) > op_head)
    else {
        return;
    };
    if !imports_js_interop(c) {
        return;
    }
    let js_type = type_text(c, type_row);
    if js_type.is_empty() {
        return;
    }
    let mut visiting = FxHashSet::default();
    let right_interop = type_contains_interop(c, type_row, &mut visiting);
    let mut visiting = FxHashSet::default();
    let left_interop = match left_type_row {
        Some(row) => type_contains_interop(c, row, &mut visiting),
        None => is_interop_name(c, dart_type.trim_end_matches('?'), &mut visiting),
    };
    // `canBeSubtypeOf` (:406-425) is out of reach: a pair where both sides
    // are (or contain) interop types could be a legal test or cast, so it
    // stays silent, and the supertype side of an otherwise-unrelated pair
    // (`Object`/`dynamic`, the two types every interop type subtypes) stays
    // silent too — the official `!erasedLeftIsSubtype`/`!erasedRightIsDynamic`
    // (:359) and `!erasedLeftIsDynamic`/`!erasedRightIsDynamic` (:390-393)
    // survivor tests.
    if left_interop == right_interop {
        return;
    }
    if is_check && is_top_type(&js_type) {
        return;
    }
    if !is_check && (is_top_type(&dart_type) || is_top_type(&js_type)) {
        return;
    }
    // The container anchors at the `is`/`as` operator (a delta-tile
    // row — `token_range().start` is the operator token); the official
    // `reportAtNode` opens at the operand. `head_token` on the operand
    // row answers that start.
    let range = crate::rules::utf16_token_span(c, c.head_token(operand_row)).start
        ..type_utf16(c, type_row).end.max(c.utf16_span(node).end);
    let args = [dart_type.as_str(), js_type.as_str()];
    match (is_check, left_interop) {
        (true, false) => c.report_texts(range, DART_IS_JS_PROBLEM, DART_IS_JS_CORRECTION, &args),
        (true, true) => c.report_texts(range, JS_IS_DART_PROBLEM, JS_IS_DART_CORRECTION, &args),
        (false, false) => c.report_texts(range, DART_AS_JS_PROBLEM, DART_AS_JS_CORRECTION, &args),
        (false, true) => c.report_texts(range, JS_AS_DART_PROBLEM, JS_AS_DART_CORRECTION, &args),
    }
}

/// The operand's own row and the rows preceding it in the window: the
/// trailing row, with a bare identifier's read group folded back to the
/// name row (the bridge tiles a bare read as `Identifier, NoTypeArguments,
/// NoArguments, Send`).
fn trailing_operand<'r>(c: &Check<'_>, rows: &'r [u32]) -> Option<(u32, &'r [u32])> {
    let len = rows.len();
    if len >= 4 && slice::is_tear_off_group(c, rows, len - 4) {
        return Some((rows[len - 4], &rows[..len - 4]));
    }
    rows.split_last().map(|(&last, rest)| (last, rest))
}

/// Whether a type row's kind spells a type.
fn is_type_row(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Type | NodeKind::VoidType | NodeKind::FunctionType | NodeKind::RecordType
    )
}

/// The token extent of a written type: the head row plus its argument
/// group, which tiles as an adjacent sibling (the bridge's completion order
/// emits `List` and `<JSString>` as separate rows, and a type-argument row
/// may complete before or after its head). The extent is what the official
/// `reportAtNode(expression)`/display covers.
fn type_extent(c: &Check<'_>, row: u32) -> (u32, u32) {
    let range = c.unit.ast.token_range(row);
    let (mut start, mut end) = (range.start, range.end);
    let Some(parent) = c.unit.ast.parent(row) else {
        return (start, end);
    };
    let siblings = c.collect_direct(parent);
    let Some(pos) = siblings.iter().position(|&sibling| sibling == row) else {
        return (start, end);
    };
    let is_arguments = |kind: NodeKind| {
        matches!(
            kind,
            NodeKind::TypeArguments | NodeKind::InvalidTypeArguments
        )
    };
    for &sibling in siblings[..pos].iter().rev() {
        if !is_arguments(c.kind(sibling)) {
            break;
        }
        let span = c.unit.ast.token_range(sibling);
        start = start.min(span.start);
        end = end.max(span.end);
    }
    for &sibling in &siblings[pos + 1..] {
        if !is_arguments(c.kind(sibling)) {
            break;
        }
        let span = c.unit.ast.token_range(sibling);
        start = start.min(span.start);
        end = end.max(span.end);
    }
    (start, end)
}

/// The written text of a type row — the analyzer's display spelling for the
/// corpus' unaliased, unprefixed types (an alias or a `p.` prefix would
/// display differently; the documented divergence of this name cut).
fn type_text(c: &Check<'_>, row: u32) -> String {
    let (start, end) = type_extent(c, row);
    let (Some(first), Some(last)) = (
        c.unit.tokens.get(start as usize),
        c.unit.tokens.get(end.saturating_sub(1) as usize),
    ) else {
        return String::new();
    };
    let span = first.byte_range().start..last.byte_range().end;
    c.unit.source[span].to_owned()
}

/// The reported UTF-16 extent of a written type.
fn type_utf16(c: &Check<'_>, row: u32) -> core::ops::Range<u32> {
    let (start, end) = type_extent(c, row);
    let (Some(from), Some(to)) = (
        c.utf16_of_token(start),
        c.utf16_end_of_token(end.saturating_sub(1)),
    ) else {
        return c.utf16_span(row);
    };
    from..to
}

/// The two `canBeSubtypeOf` supertype faces every JS interop type subtypes.
fn is_top_type(text: &str) -> bool {
    matches!(text, "Object" | "dynamic" | "void" | "Object?" | "dynamic?")
}

/// Whether a written type row contains an interop type at any depth: the
/// `InteropTypeChecker` walk (:204-229) over the row's identifier tokens,
/// each resolved through [`is_interop_name`] (which follows a type
/// parameter to its bound and an extension type to its representation).
fn type_contains_interop(c: &Check<'_>, row: u32, visiting: &mut FxHashSet<String>) -> bool {
    let (start, end) = type_extent(c, row);
    for index in start..end {
        let Some(token) = c.unit.tokens.get(index as usize) else {
            continue;
        };
        if token.kind != TokenKind::Identifier {
            continue;
        }
        if is_interop_name(c, c.lexeme(index), visiting) {
            return true;
        }
    }
    false
}

/// The interop class of a type reference name: a type parameter follows its
/// bound (:45), a same-file extension type follows its representation type
/// (:57-67), a `@JS`-annotated container is a user interop type (:68), an
/// indexed container without the annotation is a plain Dart type, and an
/// unresolvable name is a `dart:js_interop` type only when it is one of the
/// SDK's own ([`DART_JS_INTEROP_TYPES`]).
fn is_interop_name(c: &Check<'_>, name: &str, visiting: &mut FxHashSet<String>) -> bool {
    if !visiting.insert(name.to_owned()) {
        return false;
    }
    if let Some(bound) = type_parameter_bound(c, name) {
        return type_contains_interop(c, bound, visiting);
    }
    if let Some(representation) = extension_type_representation(c, name) {
        return type_contains_interop(c, representation, visiting);
    }
    if same_file_js_class(c, name) {
        return true;
    }
    // `None` — an unresolvable container (an SDK type, an unanalyzed
    // dependency) — falls back to the SDK's own name set; `Some(_)` is a
    // resolvable workspace container whose own annotation rows answer.
    if let Some(analysis) = c.semantics() {
        let strings = analysis.strings();
        if let Some(annotated) =
            analysis.container_has_annotation(strings.intern(name), strings.intern("JS"))
        {
            return annotated;
        }
    }
    DART_JS_INTEROP_TYPES.contains(&name)
}

/// The bound type row of the type parameter `name` in scope at the current
/// node (a `TypeVariables` child of an enclosing declaration, its
/// `TypeVariable` rows naming the parameter and carrying the written bound
/// — the `visitTypeParameter` walk's substrate).
fn type_parameter_bound(c: &Check<'_>, name: &str) -> Option<u32> {
    for &ancestor in c.path.iter().rev().skip(1) {
        let Some(variables) = c
            .collect_direct(ancestor)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::TypeVariables)
        else {
            continue;
        };
        for variable in c.collect_direct(variables) {
            if c.kind(variable) != NodeKind::TypeVariable {
                continue;
            }
            let range = c.unit.ast.token_range(variable);
            let Some(declared) = (range.start..range.end).find(|&index| {
                c.unit
                    .tokens
                    .get(index as usize)
                    .is_some_and(|token| token.kind == TokenKind::Identifier)
            }) else {
                continue;
            };
            if c.lexeme(declared) != name {
                continue;
            }
            return c
                .children(variable)
                .filter(|&inner| {
                    is_type_row(c.kind(inner)) && c.unit.ast.parent(inner) == Some(variable)
                })
                .next();
        }
    }
    None
}

/// The representation type row of the same-file `extension type name(...)`:
/// the first type row inside its representation parameter group (the
/// `element.representation.type` face, :58).
fn extension_type_representation(c: &Check<'_>, name: &str) -> Option<u32> {
    let root = c.unit.ast.root()?;
    for top in c.collect_direct(root) {
        if c.kind(top) != NodeKind::TopLevelDeclaration {
            continue;
        }
        for declaration in c.collect_direct(top) {
            if c.kind(declaration) != NodeKind::ExtensionTypeDeclaration {
                continue;
            }
            let Some(name_token) = c.field(declaration, PayloadField::Name) else {
                continue;
            };
            if name_token == crate::rules::NONE_TOKEN || c.lexeme(name_token) != name {
                continue;
            }
            let (open, close) = representation_parens(c, name_token)?;
            return c.children(declaration).find(|&row| {
                let span = c.unit.ast.token_range(row);
                is_type_row(c.kind(row)) && span.start >= open && span.end <= close
            });
        }
    }
    None
}

/// The token ids of the representation parameter group's parens, searched
/// after the extension type's name.
fn representation_parens(c: &Check<'_>, name_token: u32) -> Option<(u32, u32)> {
    let mut depth = 0usize;
    let mut open = None;
    let mut index = name_token.checked_add(1)?;
    for _ in 0..1024 {
        let token = c.unit.tokens.get(index as usize)?;
        match token.kind {
            TokenKind::OpenParen => {
                if open.is_none() {
                    open = Some(index);
                }
                depth += 1;
            }
            TokenKind::CloseParen => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return open.map(|open| (open, index));
                }
            }
            // The header ends before any group opens: a shape we do not
            // read leaves the representation unknowable.
            TokenKind::Semicolon | TokenKind::OpenCurly if open.is_none() => return None,
            _ => {}
        }
        index += 1;
    }
    None
}

/// The written type row of the declaration `name` binds at `use_node` (a
/// parameter, local, field, or top-level variable — the rule context's own
/// lookup; a `dynamic`/untyped declaration answers no row).
fn written_type_row(c: &Check<'_>, use_node: u32, name: &str) -> Option<u32> {
    let (_, row) = super::same_file::binding(c, use_node, name)?;
    row.filter(|&row| is_type_row(c.kind(row)))
}

/// The static type display of a bare operand read: the bridge read
/// row's named type (the display argument must be exact — an
/// argument-carrying type has no renderable display here and falls back
/// to the written declaration, [`written_type_row`]; `dynamic` and untyped
/// reads carry no Dart-type face).
fn typed_identifier_display(c: &Check<'_>, row: u32) -> Option<String> {
    let analysis = c.semantics()?;
    let Ty::Named {
        name,
        nullable,
        args,
    } = analysis.type_of(query_site(c, row))?
    else {
        return None;
    };
    if args != ArgListId::EMPTY {
        return None;
    }
    let mut display = analysis.strings().resolve(name).to_owned();
    if nullable {
        display.push('?');
    }
    Some(display)
}

/// The enclosing class-like declaration's name (the wrapper-aware
/// read — the production name is the row's left sibling).
fn enclosing_class_name<'a>(c: &'a Check<'_>) -> Option<&'a str> {
    let class_row = c.path.iter().rev().find(|&&a| {
        matches!(
            c.kind(a),
            NodeKind::ClassDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::EnumDeclaration
                | NodeKind::ExtensionDeclaration
        )
    })?;
    crate::rules::class_declaration_name(c, *class_row)
}

/// The member-signature rows that tile before an arrow body's
/// expression (the written return type, the name, the parameter list,
/// their wrappers — and the childless `ExpressionBody` handle the
/// expression rows trail).
fn is_signature_row(c: &Check<'_>, row: u32) -> bool {
    matches!(
        c.kind(row),
        NodeKind::Type
            | NodeKind::VoidType
            | NodeKind::FunctionType
            | NodeKind::Identifier
            | NodeKind::FormalParameters
            | NodeKind::OptionalFormalParameters
            | NodeKind::TypeVariables
            | NodeKind::OperatorName
            | NodeKind::NoTypeArguments
            | NodeKind::TypeArguments
            | NodeKind::ExpressionBody
    )
}

/// Whether the unit imports `dart:js_interop`.
fn imports_js_interop(c: &Check<'_>) -> bool {
    let Some(root) = c.unit.ast.root() else {
        return false;
    };
    // The import directive tiles inside its `TopLevelDeclaration`
    // wrapper; a root-direct Import is accepted too should a shape ever
    // leave it bare.
    let has_uri = |c: &Check<'_>, row: u32| {
        c.kind(row) == NodeKind::Import
            && c.collect_direct(row).into_iter().any(|kid| {
                c.kind(kid) == NodeKind::StringLiteral && {
                    let span = c.byte_span(kid);
                    let text = &c.unit.source[span.start..span.end];
                    text.trim_matches(|q| q == '\'' || q == '"') == "dart:js_interop"
                }
            })
    };
    c.collect_direct(root).into_iter().any(|row| {
        has_uri(c, row)
            || (c.kind(row) == NodeKind::TopLevelDeclaration
                && c.collect_direct(row).into_iter().any(|kid| has_uri(c, kid)))
    })
}

/// Whether `name` is a same-file class carrying a `@JS` annotation
/// (the annotation star precedes the top-level wrapper at the root).
fn same_file_js_class(c: &Check<'_>, name: &str) -> bool {
    let Some(root) = c.unit.ast.root() else {
        return false;
    };
    let tops = c.collect_direct(root);
    for (i, &top) in tops.iter().enumerate() {
        if c.kind(top) != NodeKind::TopLevelDeclaration {
            continue;
        }
        let named = c.collect_direct(top).into_iter().any(|kid| {
            c.kind(kid) == NodeKind::Identifier
                && c.field(kid, PayloadField::Context)
                    == Some(
                        dartq_parser::IdentifierContext::ClassOrMixinOrExtensionDeclaration as u32,
                    )
                && c.lexeme(c.head_token(kid)) == name
        });
        if !named {
            continue;
        }
        // The class's annotation star tiles as the wrapper's left
        // sibling at the root (the annotation seals before the wrapper
        // opens), or inside the wrapper when the wrapper opened first —
        // read both faces.
        let mut stars: Vec<u32> = Vec::new();
        if i > 0 && c.kind(tops[i - 1]) == NodeKind::MetadataStar {
            stars.push(tops[i - 1]);
        }
        for kid in c.collect_direct(top) {
            if c.kind(kid) == NodeKind::MetadataStar {
                stars.push(kid);
            }
        }
        let annotated = stars.into_iter().any(|star| {
            c.collect_direct(star).into_iter().any(|meta| {
                c.kind(meta) == NodeKind::Metadata
                    && c.collect_direct(meta).into_iter().any(|piece| {
                        c.kind(piece) == NodeKind::Identifier
                            && c.lexeme(c.head_token(piece)) == "JS"
                    })
            })
        });
        if annotated {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use crate::testsupport::{Fixture, one_rule};

    fn run(source: &str) -> Vec<crate::engine::Lint> {
        Fixture::parse(source).run(&one_rule("invalid_runtime_check_with_js_interop_types"))
    }

    /// The reported text under a lint's range (the test sources are ASCII,
    /// so the UTF-16 range slices the source directly).
    fn reported<'a>(source: &'a str, lint: &crate::engine::Lint) -> &'a str {
        &source[lint.range.start as usize..lint.range.end as usize]
    }

    /// `doc_bad_0` / `doc_bad_1`: the dart-is-js face reports with both
    /// written types, and a nested interop type argument is enough
    /// (`List<JSString>` — the recursive `InteropTypeChecker` walk).
    #[test]
    fn dart_is_js_face_reports_with_both_written_types() {
        let source = "import 'dart:js_interop';\n\nbool f(Object b) => b is JSBoolean;\n";
        let lints = run(source);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Runtime check between 'Object' and 'JSBoolean' checks whether a Dart value is a JS \
             interop type, which might not be platform-consistent."
        );
        assert_eq!(
            lints[0].correction.as_deref(),
            Some(
                "Try using 'isA' from 'dart:js_interop' to check if the Dart value is a JS interop \
                 type."
            )
        );
        assert_eq!(reported(source, &lints[0]), "b is JSBoolean");

        let source =
            "import 'dart:js_interop';\n\nbool f(List<Object> l) => l is List<JSString>;\n";
        let lints = run(source);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Runtime check between 'List<Object>' and 'List<JSString>' checks whether a Dart value \
             is a JS interop type, which might not be platform-consistent."
        );
        assert_eq!(reported(source, &lints[0]), "l is List<JSString>");
    }

    /// `doc_bad_2`: the js-is-dart direction, which carries no correction
    /// message (the official template has none).
    #[test]
    fn js_is_dart_face_reports_without_a_correction() {
        let source = "import 'dart:js_interop';\n\nbool f(JSAny a) => a is String;\n";
        let lints = run(source);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Runtime check between 'JSAny' and 'String' checks whether a JS interop value is a \
             Dart type, which might not be platform-consistent."
        );
        assert_eq!(lints[0].correction, None);
        assert_eq!(reported(source, &lints[0]), "a is String");
    }

    /// `doc_bad_3`: the cast face reports with `(js type, dart type)` when
    /// the JS side is the source.
    #[test]
    fn js_as_dart_cast_reports() {
        let source = "import 'dart:js_interop';\n\nString f(JSAny a) => a as String;\n";
        let lints = run(source);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Cast from 'JSAny' to 'String' casts a JS interop value to a Dart type, which might \
             not be platform-consistent."
        );
        assert_eq!(reported(source, &lints[0]), "a as String");
    }

    /// `test_nullabilityIs_*` / `doc_good_*`: a supertype side
    /// (`Object`/`dynamic`, which every JS interop type subtypes) and a
    /// `null` operand stay silent, and so does a pair with no interop type
    /// at all.
    #[test]
    fn supertype_and_null_faces_stay_silent() {
        for source in [
            "import 'dart:js_interop';\n\nbool f(JSAny a) => a is Object;\n",
            "import 'dart:js_interop';\n\nObject f(Object o) => o as JSObject;\n",
            "import 'dart:js_interop';\n\nvoid f() {\n  null is JSAny;\n}\n",
            "import 'dart:js_interop';\n\nbool f(Object o) => o is Object;\n",
        ] {
            assert!(run(source).is_empty(), "{source}");
        }
    }

    /// `test_catchClause_js_type` / `_nested`: the catch clause reports at
    /// the written exception type, at any depth, and a plain Dart type stays
    /// silent.
    #[test]
    fn catch_clause_reports_a_type_containing_an_interop_type() {
        let source = concat!(
            "import 'dart:js_interop';\n",
            "\n",
            "void f() {\n",
            "  try {\n",
            "  } on JSString catch (_) {\n",
            "  } on List<JSNumber> catch (_) {\n",
            "  } on bool catch (_) {\n",
            "  }\n",
            "}\n",
        );
        let lints = run(source);
        assert_eq!(lints.len(), 2, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Catch clause with type 'JSString' checks whether the caught value is a JS interop \
             type, which might not be platform-consistent."
        );
        assert_eq!(reported(source, &lints[0]), "JSString");
        assert_eq!(
            lints[1].message,
            "Catch clause with type 'List<JSNumber>' checks whether the caught value is a JS \
             interop type, which might not be platform-consistent."
        );
        assert_eq!(reported(source, &lints[1]), "List<JSNumber>");
    }

    /// `test_catchClause_js_type_nested` / `_static_interop_type_nested` /
    /// `_user_interop_type_nested`: a type parameter follows its bound, so
    /// the bare `T` of `void f<T extends JSNumber>()` reports.
    #[test]
    fn catch_clause_follows_type_parameter_bounds() {
        let source = concat!(
            "import 'dart:js_interop';\n",
            "\n",
            "void f<T extends JSNumber>() {\n",
            "  try {\n",
            "  } on T catch (_) {\n",
            "  }\n",
            "}\n",
        );
        let lints = run(source);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Catch clause with type 'T' checks whether the caught value is a JS interop type, \
             which might not be platform-consistent."
        );
        assert_eq!(reported(source, &lints[0]), "T");
    }

    /// `test_catchClause_user_interop_type`: a same-file extension type whose
    /// representation type is a `dart:js_interop` type is a user interop type
    /// (`element.representation.type`, :58), and so is an `@JS` class.
    #[test]
    fn catch_clause_classifies_user_interop_types() {
        let source = concat!(
            "import 'dart:js_interop';\n",
            "\n",
            "extension type JSError(JSObject _) implements JSObject {}\n",
            "\n",
            "@JS()\n",
            "@staticInterop\n",
            "class Foo {}\n",
            "\n",
            "void f() {\n",
            "  try {\n",
            "  } on JSError catch (_) {\n",
            "  } on Foo catch (_) {\n",
            "  }\n",
            "}\n",
        );
        let lints = run(source);
        assert_eq!(lints.len(), 2, "{lints:?}");
        assert_eq!(reported(source, &lints[0]), "JSError");
        assert_eq!(reported(source, &lints[1]), "Foo");
    }
}
