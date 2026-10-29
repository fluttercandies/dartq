use super::{Check, NodeKind, PayloadField, RuleSpec, narrow_u32, slice, static_type_faces};
use crate::rules::{body_members, same_file_declaration};

/// Official: `upstream Dart SDK source,
/// pkg/linter/lib/src/rules/avoid_redundant_argument_values.dart` —
/// an argument whose constant value equals its parameter's default
/// value reports at the argument expression (:64-81 `checkArgument`,
/// `reportAtNode(arg.argumentExpression)` :78). The visitor walks the
/// arguments from the last to the first (:52-61) and breaks after an
/// optional-positional parameter (:56-59 — a later optional positional
/// makes the earlier ones required). The gates (:64-71): the parameter
/// must resolve, be optional (named or optional positional), and not
/// carry `@required`. The registered faces (:35-41): method
/// invocations, function-expression invocations, instance creations,
/// annotations, enum constants, and the dot-shorthand invocation
/// shapes; factory constructors resolve the defaults against the
/// ultimate redirect target's parameters (:134-199).
///
/// dartq faces over the production tiling: the dispatch row is the
/// argument list, the callee resolved same-file per parent face —
/// a following `Send` sibling (bare `g(...)` or member `a.g(...)`
/// through the single-name receiver's written type or class, or the
/// implicit-`this` method of the enclosing class), a parent
/// creation/annotation/enum-constant row (the class's constructor,
/// factory redirects walked to the target with a cycle guard), and a
/// top-level or local function for the bare face. The parameter's
/// default is its written `=`/`:` expression (the bridge init words —
/// upstream compares the computed constant values); the equality runs
/// both faces through the written-constant subset
/// ([`dartq_semantic::const_eval::eval_written`]) so differently
/// spelled but equal values (`2` vs `1 + 1`) report, with the
/// token-for-token comparison kept for shapes outside the subset —
/// identical spellings still compare equal, differently spelled
/// shapes under-report. A parameter without a default clause carries the
/// implicit `null`: only a written `null` argument reports (the
/// `computeConstantValue` null-value face). Constructor rows anchor at
/// the class name (the named selector rides the
/// `MethodDeclarationContinuation` identifier) and factory rows at the
/// `factory` keyword (the name two or four tokens ahead); both faces
/// matched accordingly. The bare selector resolves innermost-first: a
/// block-local function declared before the call (or an ancestor's own
/// name — self-recursion), a function-typed formal of an enclosing
/// executable (its signature rides the `FunctionType` child of the
/// `FormalParameter` row), a local variable bound to a same-file
/// constructor tear-off (`var a = A.new` — the two-`Send` group inside
/// the `VariableInitializer`), then a top-level function, a same-file
/// constructor, or the enclosing class's implicit-`this` method. The
/// enum element's `Arguments` tile under the `EnumBody` as the
/// `EnumElement` marker's left sibling, and the enum's constructor is
/// read inside the `EnumBody` (not `DeclarationBody`). Approximations
/// (documented, under-report): the element-model faces that leave the
/// file — SDK members like `toList(growable: true)`, prefixed
/// annotations and creations, and cross-file declarations — resolve to
/// nothing and stay silent. The exception is the vendored-meta
/// annotation face: `@Immutable(…)`/`@Literal(…)` resolve through the
/// pinned `META_ANNOTATION_DEFAULTS` table (one optional-positional
/// parameter defaulting to `''`), the corpus vendor's transcription of
/// upstream meta.dart.
/// The upstream options face (`ignoredParameters`) does not exist in
/// the current rule source. Message: `diagnostic.g.dart`
/// `avoidRedundantArgumentValues`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "avoid_redundant_argument_values",
    &[NodeKind::Arguments],
    check,
);

/// The callee's resolved formal rows: positional in declaration order
/// (required group first, then the optional-positional group), named
/// in declaration order. `meta_default` rides the vendored-meta
/// annotation face (no formal rows): the pinned optional-positional
/// parameter's default spelling.
struct Formals<'a> {
    params_row: u32,
    positional: Vec<u32>,
    named: Vec<(&'a str, u32)>,
    meta_default: Option<&'static str>,
}

/// The vendored-meta annotation classes the meta face pins: both
/// declare one optional-positional `String` parameter defaulting to
/// `''` (upstream meta.dart `const Immutable([this.reason = ''])` /
/// `const Literal([this.comment = ''])`).
const META_ANNOTATION_DEFAULTS: &[(&str, &str)] = &[("Immutable", "''"), ("Literal", "''")];

fn check(c: &mut Check<'_>) {
    let node = c.node();
    let pieces = c.collect_direct(node);
    let ranges = slice::arguments(c, &pieces);
    if ranges.is_empty() {
        return;
    }
    let Some(formals) = resolve_callee(c, node) else {
        return;
    };
    // The named-argument face: the group's trailing marker
    // ([`slice::arguments`] tiling).
    let named: Vec<bool> = ranges
        .iter()
        .map(|range| c.kind(pieces[range.end - 1]) == NodeKind::NamedArgument)
        .collect();
    // Each positional argument's 0-based index among the positional
    // arguments before it.
    let mut positional_seen = 0usize;
    let indices: Vec<Option<usize>> = named
        .iter()
        .map(|&is_named| {
            if is_named {
                None
            } else {
                let index = positional_seen;
                positional_seen += 1;
                Some(index)
            }
        })
        .collect();
    // Last to first (:52), breaking after an optional positional
    // (:56-59).
    for i in (0..ranges.len()).rev() {
        let param = if let Some(index) = indices[i] {
            formals.positional.get(index).copied()
        } else {
            let name = c.lexeme(c.head_token(pieces[ranges[i].start]));
            formals
                .named
                .iter()
                .find(|&&(formal, _)| formal == name)
                .map(|&(_, row)| row)
        };
        check_argument(c, &formals, &pieces, &ranges[i], param, indices[i]);
        if param.is_some_and(|p| {
            formal_kind(c, p) == dartq_parser::FormalParameterKind::OptionalPositional as u32
        }) {
            break;
        }
    }
}

/// `checkArgument` (:64-81): the optionality gates, the `@required`
/// metadata gate, then the default-value comparison.
fn check_argument(
    c: &mut Check<'_>,
    formals: &Formals<'_>,
    pieces: &[u32],
    range: &core::ops::Range<usize>,
    param: Option<u32>,
    index: Option<usize>,
) {
    // The vendored-meta annotation face: the resolved class is
    // package:meta's `Immutable`/`Literal` — its single
    // optional-positional parameter defaults to the pinned spelling,
    // so the first positional argument equal to it reports.
    let meta_default = formals.meta_default.filter(|_| param.is_none());
    if let Some(param) = param {
        // `param.isRequired` (:67-69): only the two optional kinds
        // answer.
        let kind = formal_kind(c, param);
        if kind != dartq_parser::FormalParameterKind::OptionalNamed as u32
            && kind != dartq_parser::FormalParameterKind::OptionalPositional as u32
        {
            return;
        }
        // `param.metadata.hasRequired` (:68).
        if param_has_required_annotation(c, formals.params_row, param) {
            return;
        }
    } else if meta_default.is_none() || index != Some(0) {
        return;
    }
    // The value face: the argument expression's own pieces (a named
    // group's name and marker bracket its value pieces).
    let (first, last) = if c.kind(pieces[range.end - 1]) == NodeKind::NamedArgument {
        (range.start + 1, range.end - 2)
    } else {
        (range.start, range.end - 1)
    };
    if first > last {
        return;
    }
    // The value's token range: the widest piece bounds it (a nested
    // call's own pieces tile flat inside the argument list).
    let value_start = c.head_token(pieces[first]);
    let Some(value_end) = (first..=last)
        .filter_map(|k| c.unit.ast.token_range(pieces[k]).end.checked_sub(1))
        .max()
    else {
        return;
    };
    let redundant = if let Some(default) = meta_default {
        // The pinned meta default: one `''` token.
        value_start == value_end && c.lexeme(value_start) == default
    } else {
        match default_clause(c, param.expect("resolved formal")) {
            Some((start, end)) => constants_equal(c, value_start..=value_end, start..=end),
            // No default clause: the implicit `null` — only a written
            // `null` argument equals it (the `computeConstantValue`
            // null-value face).
            None => first == last && c.kind(pieces[first]) == NodeKind::LiteralNull,
        }
    };
    if redundant {
        let start = crate::rules::utf16_token_span(c, value_start).start;
        let end = crate::rules::utf16_token_span(c, value_end).end;
        c.report(start..end, &[]);
    }
}

/// The parameter's default-clause tokens (the `InitializerStart`/
/// `InitializerEnd` words — `InitializerStart` opens the expression,
/// `InitializerEnd` is the peek after it — exclusive).
fn default_clause(c: &Check<'_>, param: u32) -> Option<(u32, u32)> {
    let start = c.field(param, PayloadField::InitializerStart)?;
    let end = c.field(param, PayloadField::InitializerEnd)?;
    if start == crate::rules::NONE_TOKEN || end == crate::rules::NONE_TOKEN || end <= start {
        return None;
    }
    Some((start, end - 1))
}

/// The `computeConstantValue()` equality (:71-80): both faces evaluated
/// through the written-constant subset
/// ([`dartq_semantic::const_eval::eval_written`]) — values spelled
/// differently but equal (`2` vs `1 + 1`) report; shapes outside the
/// subset keep the token-for-token spelling comparison, where identical
/// spellings compare equal (the documented resolution-blind fallback)
/// and different spellings stay silent; a slice with no known value
/// never matches — not even at the identical spelling (the oracle's
/// `hasKnownValue` gate). A subset value against a shape outside it
/// stays silent too (the under-report).
fn constants_equal(
    c: &Check<'_>,
    left: core::ops::RangeInclusive<u32>,
    right: core::ops::RangeInclusive<u32>,
) -> bool {
    use dartq_semantic::const_eval::WrittenValue;
    let (left_len, right_len) = (
        left.end() - left.start() + 1,
        right.end() - right.start() + 1,
    );
    let written = |range: &core::ops::RangeInclusive<u32>, len: u32| {
        let lexemes: Vec<&str> = (0..len)
            .map(|offset| c.lexeme(range.start() + offset))
            .collect();
        dartq_semantic::const_eval::eval_written(&lexemes)
    };
    match (written(&left, left_len), written(&right, right_len)) {
        (WrittenValue::Value(l), WrittenValue::Value(r)) => l == r,
        // Both outside the subset: the spelling comparison — the
        // `tokens_equal` face this equality started as.
        (WrittenValue::NotEvaluable, WrittenValue::NotEvaluable) => {
            left_len == right_len
                && (0..left_len).all(|offset| {
                    let (left_tok, right_tok) = (left.start() + offset, right.start() + offset);
                    left_tok == right_tok || c.lexeme(left_tok) == c.lexeme(right_tok)
                })
        }
        _ => false,
    }
}

/// The formal's kind word (the `FormalParameterKind` word, bridge
/// `end_formal_parameter`).
fn formal_kind(c: &Check<'_>, param: u32) -> u32 {
    c.field(param, PayloadField::FormalParameterKind)
        .unwrap_or(dartq_parser::FormalParameterKind::RequiredPositional as u32)
}

/// The formal's declared name (the `Name` word).
fn formal_name<'a>(c: &Check<'a>, param: u32) -> Option<&'a str> {
    let name = c.field(param, PayloadField::Name)?;
    (name != crate::rules::NONE_TOKEN).then(|| c.lexeme(name))
}

/// The formal's `@required` metadata (:68): the metadata star is the
/// formal's left sibling inside the parameter group that holds it
/// (the required list, or the `[...]`/`{...}` optional group).
fn param_has_required_annotation(c: &Check<'_>, params_row: u32, param: u32) -> bool {
    let container = c
        .collect_direct(params_row)
        .into_iter()
        .find(|&kid| {
            c.kind(kid) == NodeKind::OptionalFormalParameters
                && c.collect_direct(kid).contains(&param)
        })
        .unwrap_or(params_row);
    let mut pending = false;
    for kid in c.collect_direct(container) {
        match c.kind(kid) {
            NodeKind::MetadataStar => {
                pending = c.collect_direct(kid).into_iter().any(|meta| {
                    c.kind(meta) == NodeKind::Metadata && annotation_named(c, meta, "required")
                });
            }
            NodeKind::FormalParameter | NodeKind::FunctionTypedFormalParameter => {
                if kid == param {
                    return pending;
                }
                pending = false;
            }
            _ => {}
        }
    }
    false
}

fn annotation_named(c: &Check<'_>, meta: u32, name: &str) -> bool {
    c.collect_direct(meta)
        .into_iter()
        .any(|piece| c.kind(piece) == NodeKind::Identifier && c.lexeme(c.head_token(piece)) == name)
}

/// The callee face per parent shape, resolved to its formal rows.
/// `None` = the face cannot resolve same-file (the silent family).
fn resolve_callee<'a>(c: &Check<'a>, args: u32) -> Option<Formals<'a>> {
    let parent = c.unit.ast.parent(args)?;
    match c.kind(parent) {
        NodeKind::NewExpression | NodeKind::ImplicitCreation => creation_formals(c, parent),
        NodeKind::Metadata => annotation_formals(c, parent),
        NodeKind::EnumElement => enum_constant_formals(c, parent),
        _ => {
            // The invocation faces: a `Send` sibling right after the
            // argument list (the send row completes after it). The enum
            // face: `EnumBody` parents the `Arguments` directly, the
            // `EnumElement` constant marker tiles as the next sibling.
            let siblings = c.collect_direct(parent);
            let pos = siblings.iter().position(|&kid| kid == args)?;
            match siblings.get(pos + 1).copied().map(|row| c.kind(row)) {
                Some(NodeKind::Send) => invocation_formals(c, &siblings, pos + 1),
                Some(NodeKind::EnumElement) => enum_constant_formals(c, siblings[pos + 1]),
                _ => None,
            }
        }
    }
}

/// The bare / member invocation faces (:117-131): the selector off
/// [`slice::send_parts`], the member shape resolving through the
/// receiver's written type or class.
fn invocation_formals<'a>(c: &Check<'a>, siblings: &[u32], pos: usize) -> Option<Formals<'a>> {
    let (head, arguments) = slice::send_parts(c, siblings, pos)?;
    // A tear-off send carries no invocation.
    arguments?;
    let selector_row = siblings[head];
    let name = c.lexeme(c.head_token(selector_row));
    if slice::access_marker(c, siblings, pos, NodeKind::DotAccess).is_some() {
        let selector = c.head_token(selector_row);
        let receiver = static_type_faces::single_receiver(c, &siblings[..head], selector)?;
        let receiver_name = c.lexeme(c.head_token(receiver));
        // The written-type face first — the shadow-correct variable
        // read (`c.m(1)` through the declared class).
        if let Some(face) = crate::rules::written_type_face(c, receiver, receiver_name)
            && let Some(type_name) = face.name
            && let Some(class_row) = same_file_declaration(c, type_name)
            && let Some(formals) = method_formals(c, class_row, name)
        {
            return Some(formals);
        }
        // A same-file class name in receiver position: the static
        // member face (`C.m(1)`).
        let class_row = same_file_declaration(c, receiver_name)?;
        return method_formals(c, class_row, name);
    }
    // The bare face, innermost binding first (upstream scope order):
    // a local function declared before the call in an enclosing block
    // (or an ancestor's own name — self-recursion), a function-typed
    // formal of an enclosing executable, a local variable initialized
    // to a same-file constructor tear-off (`var a = A.new; a(...)`),
    // then a top-level function, a same-file class constructor
    // (`A(...)` — the upstream InstanceCreationExpression rewrite), or
    // the implicit-`this` method of the enclosing class.
    if let Some(formals) = local_function_formals(c, name) {
        return Some(formals);
    }
    if let Some(formals) = function_typed_param_formals(c, name) {
        return Some(formals);
    }
    if let Some(formals) = tearoff_variable_formals(c, name) {
        return Some(formals);
    }
    if let Some(formals) = top_level_function_formals(c, name) {
        return Some(formals);
    }
    if let Some(class_row) = same_file_declaration(c, name) {
        return constructor_formals(c, class_row, name, None, &mut Vec::new());
    }
    let class_row = c.path.iter().rev().copied().find(|&row| {
        matches!(
            c.kind(row),
            NodeKind::ClassDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::ExtensionDeclaration
                | NodeKind::ExtensionTypeDeclaration
        )
    })?;
    method_formals(c, class_row, name)
}

/// The method `name` of the class-like row, as formals.
fn method_formals<'a>(c: &Check<'a>, class_row: u32, name: &str) -> Option<Formals<'a>> {
    let member = body_members(c, class_row).into_iter().find(|&member| {
        c.kind(member) == NodeKind::Method
            && !crate::rules::member_matches(c, member, crate::rules::MemberQuery::Method, name)
                .is_empty()
    })?;
    formals_of(c, member)
}

/// The top-level function `name`, as formals.
fn top_level_function_formals<'a>(c: &Check<'a>, name: &str) -> Option<Formals<'a>> {
    let root = c.unit.ast.root()?;
    let ctx = dartq_parser::IdentifierContext::TopLevelFunctionDeclaration as u32;
    for top in c.collect_direct(root) {
        if c.kind(top) != NodeKind::TopLevelDeclaration {
            continue;
        }
        for decl in c.collect_direct(top) {
            if c.kind(decl) != NodeKind::TopLevelMethod {
                continue;
            }
            let named = c.collect_direct(decl).into_iter().any(|kid| {
                c.kind(kid) == NodeKind::Identifier
                    && c.field(kid, PayloadField::Context) == Some(ctx)
                    && c.lexeme(c.head_token(kid)) == name
            });
            if named {
                return formals_of(c, decl);
            }
        }
    }
    None
}

/// The local function `name` in scope at the call: an ancestor
/// declaration first (the self-recursion face — the innermost binder
/// wins), else a same-named `LocalFunctionDeclaration` /
/// `NamedFunctionExpression` statement preceding the call's statement in
/// an enclosing block (the blocks tile their statements flat —
/// `BlockBody`/`Block`/`SwitchBlock` children are the declaration
/// rows). Innermost block first; within a block, declaration order.
fn local_function_formals<'a>(c: &Check<'a>, name: &str) -> Option<Formals<'a>> {
    let is_named_fn = |c: &Check<'_>, row: u32| {
        matches!(
            c.kind(row),
            NodeKind::LocalFunctionDeclaration | NodeKind::NamedFunctionExpression
        ) && crate::rules::declarations::member_declaration_name(c, row)
            .is_some_and(|n| c.lexeme(c.head_token(n)) == name)
    };
    if let Some(row) = c
        .path
        .iter()
        .rev()
        .copied()
        .find(|&row| is_named_fn(c, row))
    {
        return formals_of(c, row);
    }
    for (i, &block) in c.path.iter().enumerate().rev() {
        if !matches!(
            c.kind(block),
            NodeKind::BlockBody | NodeKind::Block | NodeKind::SwitchBlock
        ) {
            continue;
        }
        // Only siblings before the statement on the path bind.
        let stop = c.path.get(i + 1).copied().unwrap_or(u32::MAX);
        for kid in c.collect_direct(block) {
            if kid == stop {
                break;
            }
            if is_named_fn(c, kid) {
                return formals_of(c, kid);
            }
        }
    }
    None
}

/// The direct `FormalParameter`/`FunctionTypedFormalParameter` rows of a
/// parameter-list row, flattened over the `OptionalFormalParameters`
/// group (the `[...]`/`{...}` children tile inside it).
fn params_of(c: &Check<'_>, params_row: u32) -> Vec<u32> {
    let mut params = Vec::new();
    for kid in c.collect_direct(params_row) {
        match c.kind(kid) {
            NodeKind::FormalParameter | NodeKind::FunctionTypedFormalParameter => {
                params.push(kid);
            }
            NodeKind::OptionalFormalParameters => {
                params.extend(c.collect_direct(kid).into_iter().filter(|&row| {
                    matches!(
                        c.kind(row),
                        NodeKind::FormalParameter | NodeKind::FunctionTypedFormalParameter
                    )
                }))
            }
            _ => {}
        }
    }
    params
}

/// The bare-selector face `g(...)` where `g` is a function-typed formal
/// of an enclosing executable (`void f(void Function({int? p}) g)` —
/// upstream `ExecutableElement` parameters are `FunctionTypedElement`s
/// whose own parameter list supplies the defaults). The function-typed
/// formal's signature rides a `FunctionType` child on the
/// `FormalParameter` row (production tiling); the legacy
/// `FunctionTypedFormalParameter` carries its list directly.
fn function_typed_param_formals<'a>(c: &Check<'a>, name: &str) -> Option<Formals<'a>> {
    for &owner in c.path.iter().rev() {
        let Some(params_row) = formal_params_row(c, owner) else {
            continue;
        };
        let Some(param) = params_of(c, params_row)
            .into_iter()
            .find(|&param| formal_name(c, param) == Some(name))
        else {
            continue;
        };
        let signature = c
            .collect_direct(param)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::FunctionType)
            .unwrap_or(param);
        return formals_of(c, signature).or_else(|| formals_of(c, param));
    }
    None
}

/// The bare-selector face `a(...)` where `a` is a local variable
/// initialized to a same-file constructor tear-off (`var a = A.new` —
/// upstream resolves the variable's `FunctionType` from the tear-off's
/// constructor). The initializer tiles as a two-`Send` group (`A.` +
/// `new`) inside the `VariableInitializer`; a name chain without a `.`
/// selector (`var a = b`) is not a tear-off. Only siblings before the
/// call's statement bind.
fn tearoff_variable_formals<'a>(c: &Check<'a>, name: &str) -> Option<Formals<'a>> {
    for (i, &block) in c.path.iter().enumerate().rev() {
        if !matches!(
            c.kind(block),
            NodeKind::BlockBody | NodeKind::Block | NodeKind::SwitchBlock
        ) {
            continue;
        }
        let stop = c.path.get(i + 1).copied().unwrap_or(u32::MAX);
        for kid in c.collect_direct(block) {
            if kid == stop {
                break;
            }
            if c.kind(kid) != NodeKind::VariablesDeclaration {
                continue;
            }
            let Some(var) = c.collect_direct(kid).into_iter().find(|&var| {
                c.kind(var) == NodeKind::InitializedIdentifier
                    && crate::rules::initialized_identifier_name(c, var)
                        .is_some_and(|token| c.lexeme(token) == name)
            }) else {
                continue;
            };
            let Some(init) = c
                .collect_direct(var)
                .into_iter()
                .find(|&piece| c.kind(piece) == NodeKind::VariableInitializer)
            else {
                continue;
            };
            // The pure tear-off shape: two `Send` pieces joined by a
            // `.` with no `Arguments` (`A.new` / `A<int>.named`). A
            // `var a = A.new()` creation or a plain `var a = b` binding
            // is not the face.
            let pieces = c.collect_direct(init);
            let sends: Vec<u32> = pieces
                .iter()
                .copied()
                .filter(|&piece| c.kind(piece) == NodeKind::Send)
                .collect();
            if sends.len() < 2
                || pieces
                    .iter()
                    .any(|&piece| c.kind(piece) == NodeKind::Arguments)
            {
                continue;
            }
            let class_name = c.lexeme(c.head_token(sends[0]));
            let selector = c.lexeme(c.head_token(*sends.last().expect("nonempty")));
            let Some(class_row) = same_file_declaration(c, class_name) else {
                continue;
            };
            return constructor_formals(c, class_row, class_name, Some(selector), &mut Vec::new());
        }
    }
    None
}

/// The creation face (:122-127): the class constructor (the unnamed
/// face or the spelled selector), factory redirects walked to the
/// ultimate target (:143-158).
fn creation_formals<'a>(c: &Check<'a>, creation: u32) -> Option<Formals<'a>> {
    let args = c
        .collect_direct(creation)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::Arguments)?;
    // `new` opens the keyword shape's span; the implicit creation
    // begins at the class name itself.
    let from = if c.kind(creation) == NodeKind::NewExpression {
        c.head_token(creation) + 1
    } else {
        c.head_token(creation)
    };
    let (class_name, selector) = creation_names(c, from, c.head_token(args))?;
    let class_row = same_file_declaration(c, class_name)?;
    constructor_formals(c, class_row, class_name, selector, &mut Vec::new())
}

/// The annotation face (:84-92): the annotation names a constructor
/// of its class (one identifier is the class, two name a constructor
/// selector — prefixed names under-report).
fn annotation_formals<'a>(c: &Check<'a>, metadata: u32) -> Option<Formals<'a>> {
    let mut ids: Vec<&str> = c
        .collect_direct(metadata)
        .into_iter()
        .filter(|&piece| c.kind(piece) == NodeKind::Identifier)
        .map(|piece| c.lexeme(c.head_token(piece)))
        .collect();
    let selector = (ids.len() >= 2).then(|| ids.swap_remove(1));
    let class_name = ids.pop()?;
    let Some(class_row) = same_file_declaration(c, class_name) else {
        // The vendored-meta face: `@Immutable(…)` / `@Literal(…)` — the
        // annotation classes live in package:meta (the corpus vendor),
        // whose single optional-positional parameter defaults to `''`
        // (upstream meta.dart `const Immutable([this.reason = ''])`).
        // The element model resolves the annotation; the text face pins
        // the same spelling and default.
        return META_ANNOTATION_DEFAULTS
            .iter()
            .find(|&&(name, _)| name == class_name)
            .map(|&(_, default)| Formals {
                params_row: metadata,
                positional: Vec::new(),
                named: Vec::new(),
                meta_default: Some(default),
            });
    };
    constructor_formals(c, class_row, class_name, selector, &mut Vec::new())
}

/// The enum-constant face (:107-114): the enum's own constructor —
/// the constant's name is the element's span head; an optional
/// `.name` between it and the argument list selects a named
/// constructor (`a.n(…)`).
fn enum_constant_formals<'a>(c: &Check<'a>, element: u32) -> Option<Formals<'a>> {
    let body = c.unit.ast.parent(element)?;
    let enum_row = c.unit.ast.parent(body)?;
    if c.kind(enum_row) != NodeKind::EnumDeclaration {
        return None;
    }
    // Production tiles the `Arguments` as the element's left sibling
    // inside the `EnumBody` (the `EnumElement` constant marker closes
    // after the list); hand-built trees nest the list inside the
    // element row itself.
    let args = c
        .collect_direct(element)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::Arguments)
        .or_else(|| {
            let siblings = c.collect_direct(body);
            siblings
                .iter()
                .position(|&kid| kid == element)
                .and_then(|pos| siblings.get(pos.wrapping_sub(1)).copied())
                .filter(|&kid| c.kind(kid) == NodeKind::Arguments)
        })?;
    // The selector rides a `.` directly after the constant's name.
    let name_end = c.unit.ast.token_range(element);
    let args_head = c.head_token(args);
    let selector = (name_end.start + 1 < args_head && c.lexeme(name_end.start + 1) == ".")
        .then(|| c.lexeme(name_end.start + 2));
    let class_name = crate::rules::class_declaration_name(c, enum_row)?;
    // The enum's members tile inside the `EnumBody` (not a
    // `DeclarationBody`) — `body_members` reads through the container
    // row directly.
    constructor_formals(c, body, class_name, selector, &mut Vec::new())
}

/// The creation header's names: the first identifier token names the
/// class, the identifier after the last `.` is the named-constructor
/// selector (`C<int>.named(` — the whole header is a handful of
/// tokens).
fn creation_names<'a>(
    c: &'a Check<'_>,
    from: u32,
    args_start: u32,
) -> Option<(&'a str, Option<&'a str>)> {
    let mut class_name = None;
    let mut selector = None;
    let mut token = from;
    while token < args_start {
        if is_identifier_lexeme(c, token) {
            if class_name.is_none() {
                class_name = Some(c.lexeme(token));
            } else if token > 0 && c.lexeme(token - 1) == "." {
                selector = Some(c.lexeme(token));
            }
        }
        token += 1;
    }
    Some((class_name?, selector))
}

/// The constructor row spelling `selector` on the class-like row, its
/// formals after the factory-redirect walk (:143-199). `visited`
/// carries the redirect cycle guard (:150-153 — a cycle is a
/// compile-time error; silent). When no `Constructor`/`Factory` member
/// matches, the class-like's own `PrimaryConstructor` header row
/// carries the formals (`class C({bool p = true})` /
/// `class C.named(…)` — the unnamed face matches a nameless primary,
/// the `.named` face its `PrimaryConstructorDeclaration` identifier).
fn constructor_formals<'a>(
    c: &Check<'a>,
    class_row: u32,
    class_name: &str,
    selector: Option<&str>,
    visited: &mut Vec<u32>,
) -> Option<Formals<'a>> {
    let unnamed = selector.is_none() || selector == Some("new");
    let wanted: Option<&str> = if unnamed { None } else { selector };
    let mut row = body_members(c, class_row)
        .into_iter()
        .find(|&member| match c.kind(member) {
            NodeKind::Constructor => constructor_matches(c, member, class_name, wanted),
            NodeKind::Factory => factory_matches(c, member, class_name, wanted),
            _ => false,
        })
        .or_else(|| {
            c.collect_direct(class_row).into_iter().find(|&kid| {
                c.kind(kid) == NodeKind::PrimaryConstructor && primary_ctor_matches(c, kid, wanted)
            })
        })?;
    // The factory-redirect walk: `= Target` resolves to the target
    // constructor whose defaults decide.
    while c.kind(row) == NodeKind::Factory {
        if visited.contains(&row) {
            return None;
        }
        visited.push(row);
        let (target_class, target_selector) = redirect_target(c, row)?;
        let target_row = same_file_declaration(c, &target_class)?;
        let wanted = target_selector.as_deref();
        row = body_members(c, target_row)
            .into_iter()
            .find(|&member| match c.kind(member) {
                NodeKind::Constructor => constructor_matches(c, member, &target_class, wanted),
                NodeKind::Factory => factory_matches(c, member, &target_class, wanted),
                _ => false,
            })?;
    }
    formals_of(c, row)
}

/// Whether the class-like's `PrimaryConstructor` row spells `selector`
/// (`None` = the unnamed face — a nameless primary or one written
/// `.new`; `Some(name)` = the `PrimaryConstructorDeclaration`
/// identifier match).
fn primary_ctor_matches(c: &Check<'_>, primary: u32, selector: Option<&str>) -> bool {
    let named = c.collect_direct(primary).into_iter().find(|&kid| {
        c.kind(kid) == NodeKind::Identifier
            && c.field(kid, PayloadField::Context)
                == Some(dartq_parser::IdentifierContext::PrimaryConstructorDeclaration as u32)
    });
    match (selector, named.map(|kid| c.lexeme(c.head_token(kid)))) {
        (None, None | Some("new")) => true,
        (Some(wanted), Some(name)) => name == wanted,
        _ => false,
    }
}

/// Whether the constructor row names `class_name` (its anchor) with
/// `selector` (the named face rides the `MethodDeclarationContinuation`
/// identifier — declarations.rs `constructor_name_part` idiom).
fn constructor_matches(c: &Check<'_>, ctor: u32, class_name: &str, selector: Option<&str>) -> bool {
    if c.lexeme(c.head_token(ctor)) != class_name {
        return false;
    }
    let part = c.collect_direct(ctor).into_iter().find(|&kid| {
        c.kind(kid) == NodeKind::Identifier
            && c.field(kid, PayloadField::Context)
                == Some(dartq_parser::IdentifierContext::MethodDeclarationContinuation as u32)
    });
    match (selector, part.map(|p| c.lexeme(c.head_token(p)))) {
        (None, None | Some("new")) => true,
        (Some(wanted), Some(name)) => name == wanted,
        _ => false,
    }
}

/// Whether the factory row names `class_name` with `selector`: the row
/// anchors at the `factory` keyword, the name rides one token later
/// (three past a named selector — docs.rs `factory_name_token` idiom).
fn factory_matches(c: &Check<'_>, factory: u32, class_name: &str, selector: Option<&str>) -> bool {
    let head = c.head_token(factory);
    let count = narrow_u32(c.unit.tokens.len());
    let name = head + 1;
    if name >= count || c.lexeme(name) != class_name {
        return false;
    }
    let written = if head + 2 < count && c.lexeme(head + 2) == "." && head + 3 < count {
        Some(c.lexeme(head + 3))
    } else {
        None
    };
    written == selector
}

/// The `= Target.named` of a redirecting factory body (the segments'
/// identifier tokens; one segment is the class, two the named
/// constructor — prefixed targets under-report).
fn redirect_target(c: &Check<'_>, factory: u32) -> Option<(String, Option<String>)> {
    let body = c
        .collect_direct(factory)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::RedirectingFactoryBody)?;
    let tokens = c.unit.ast.token_range(body);
    let mut cursor = tokens.start;
    while cursor < tokens.end && c.lexeme(cursor) != "=" {
        cursor += 1;
    }
    cursor += 1;
    let mut segments: Vec<&str> = Vec::new();
    while cursor < tokens.end && c.lexeme(cursor) != ";" {
        if is_identifier_lexeme(c, cursor) {
            segments.push(c.lexeme(cursor));
        }
        cursor += 1;
    }
    let selector = (segments.len() >= 2).then(|| segments[1].to_owned());
    let &class = segments.first()?;
    Some((class.to_owned(), selector))
}

/// The executable's formal rows (see [`Formals`]).
fn formals_of<'a>(c: &Check<'a>, owner: u32) -> Option<Formals<'a>> {
    let params_row = formal_params_row(c, owner)?;
    let mut positional = Vec::new();
    let mut named = Vec::new();
    for kid in c.collect_direct(params_row) {
        match c.kind(kid) {
            NodeKind::FormalParameter | NodeKind::FunctionTypedFormalParameter => {
                // The required-positional group tiles directly inside
                // the list.
                positional.push(kid);
            }
            NodeKind::OptionalFormalParameters => {
                for row in c.collect_direct(kid) {
                    if !matches!(
                        c.kind(row),
                        NodeKind::FormalParameter | NodeKind::FunctionTypedFormalParameter
                    ) {
                        continue;
                    }
                    match formal_kind(c, row) {
                        k if k == dartq_parser::FormalParameterKind::OptionalPositional as u32 => {
                            positional.push(row);
                        }
                        k if k == dartq_parser::FormalParameterKind::OptionalNamed as u32
                            || k == dartq_parser::FormalParameterKind::RequiredNamed as u32 =>
                        {
                            if let Some(name) = formal_name(c, row) {
                                named.push((name, row));
                            }
                        }
                        _ => positional.push(row),
                    }
                }
            }
            _ => {}
        }
    }
    Some(Formals {
        params_row,
        positional,
        named,
        meta_default: None,
    })
}

/// The owner's parameter list: a direct child, or one nested in the
/// function-expression child of a named function expression.
fn formal_params_row(c: &Check<'_>, owner: u32) -> Option<u32> {
    let direct = c
        .collect_direct(owner)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters);
    direct.or_else(|| {
        c.collect_direct(owner)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::FunctionExpression)
            .and_then(|expression| {
                c.collect_direct(expression)
                    .into_iter()
                    .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
            })
    })
}

/// Whether the token is a plain identifier (keywords excluded — `new`
/// and `const` never name a class or a target segment).
fn is_identifier_lexeme(c: &Check<'_>, token: u32) -> bool {
    c.unit
        .tokens
        .get(token as usize)
        .is_some_and(|t| matches!(t.kind, dartq_scanner::TokenKind::Identifier))
}
