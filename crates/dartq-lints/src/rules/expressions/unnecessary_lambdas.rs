use super::{Check, NodeKind, PayloadField, RuleSpec, query_site, slice};
use crate::rules::{NONE_TOKEN, get_set_token, variables_declaration_keyword};
use dartq_semantic::model::{ArgListId, Formal, Formals, Ty};
use dartq_semantic::{Analysis, FnSig};

/// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/unnecessary_lambdas.dart`
/// — a lambda whose whole body is one invocation forwarding exactly its
/// own parameters reports at the lambda (a tear-off will do), provided
/// the callee's function type is assignable to the lambda's own. The
/// registration is the function expression (:41); a named function
/// expression and an `async`/`sync*` body bail (:155-158). Bodies
/// (:160-186): a single-statement block whose statement is an
/// invocation or a `return` of one, an arrow body whose expression is
/// an invocation, or — constructor tear-offs enabled — an arrow body
/// whose expression is an instance creation (:181-185).
///
/// The invocation face (:228-289): every argument is the lambda's own
/// parameter — positional in order, named by name, both counts equal
/// (`argumentsMatchParameters`, dart_type_utilities.dart:13-64); the
/// callee's static invoke type is assignable to the lambda's function
/// type (:246, `isAssignableTo(invocationType, nodeType)`); a
/// deferred-prefix callee or receiver bails (:255, :264,
/// `mightBeDeferred` :314-324); the bare callee (:260) or the receiver
/// (:283) is final (`isFinalNode` :50-79, `isFinal` :326-335 — a
/// parameter of the lambda, a non-`final` or `late` variable, and a
/// non-variable accessor are not); the receiver chain carries no
/// null-aware link (:282) and the invocation no explicit type
/// arguments (:285). The named-argument and variable-initializer
/// positions add a pure-subtype check of the tear-off against the
/// context type (:269-279): for a forwarding lambda that is the main
/// relation read without the permissive fallback, which is exactly how
/// the gate below reads it, so it adds no face. The constructor face
/// (:189-226): a non-`const`, non-deferred creation (:195-197) whose
/// constructor type is assignable to the lambda's (:208) and whose
/// arguments are bare identifiers naming the lambda's parameters by
/// index with equal counts (:212-223).
///
/// dartq faces over the production tiling. The dispatch row is the
/// [`NodeKind::FunctionExpression`] (a named function expression is a
/// different row kind), the async/`sync*` gate scans the lambda's
/// tokens, and the body root is the widest row of its run (a wider
/// binary keeps the lambda silent, `test_nonFunctionCall`). The
/// arguments-match gate reads each argument's bare-identifier tear-off
/// group (parens unwrap) and requires it to resolve to the lambda's
/// own parameter (the parent-chain walk — the canonical-element
/// identity). The finality face resolves a bare receiver or callee to
/// its same-file binding: a closure or catch parameter, an enclosing
/// method parameter, a getter, and a non-`final` or `late final`
/// variable bail; a `final`/`const` variable, field, or function
/// proceeds; a non-bare receiver bails (`isFinalNode`'s shape gate);
/// the walk's terminal (cross-file and unresolved names) hands the
/// verdict to the signature gate.
///
/// The assignability gate is the semantic face — the wave posture: no
/// face, no report (the engine contract). The callee's signature is
/// the model's function-type face ([`FnSig`]): a bare callee through
/// [`Analysis::function_signature_of`] (a top-level function or an
/// implicit-`this` method, cross-file capable), a `receiver.method`
/// callee through [`Analysis::member_of`] + [`FnSig::of_member`] (a
/// method only), a creation through [`Analysis::resolve_constructor`]
/// + [`FnSig::new`] over the constructor's formals (an undeclared
/// unnamed constructor composes the implicit default's empty table).
/// The lambda's own signature is [`FnSig::new`] over its written
/// formals — names, parser kinds, written types — with a Right-Top
/// return: for every forwarding shape the covariant return position
/// holds by construction, because the lambda's inferred return is the
/// LUB of its one returned expression, the callee's own return
/// (`body_inference_context.dart:183-207`), or the typing context's
/// `void` (:157-181), and an expression-statement block body's `Null`
/// (:197-199) only ever meets the permissive fallback upstream. The
/// verdict is [`FnSig::assignable_from`]'s tri-state read in the
/// upstream decision order (:246-249,
/// `!isAssignableTo(invocationType, nodeType) &&
/// !parametersMatch(invocationType, nodeType)`): `Some(true)` —
/// provably assignable from written declarations — reports;
/// `Some(false)` (provably not) stays silent; `None` (undecidable —
/// the inferred channel: an untyped lambda formal's context type,
/// generics, hierarchy and parameterized leaves the declared facts
/// cannot decide) consults the `parametersMatch` fallback (:107-151).
/// The fallback is ported on both faces. The no-signature face: a
/// callee the model resolves to no function type — a local function,
/// a function-typed variable, an unresolved spelling (the dart:core
/// names the gate rows exercise) — takes the non-`FunctionType` arm
/// (:108-111, "We must assume is valid here") and reports: in a
/// compiling forward every invocable callee carries a function-typed
/// invocation type whose required counts the arguments-match gate has
/// already tied, so the arm cannot fail the assignability relation.
/// The FunctionType path (:112-150): the required-positional counts
/// match, the optional-positional pairs to the minimum length and the
/// invocation's named formals assign — an undecided leaf passing, a
/// decided-false one failing. The constructor gate keeps plain
/// assignability (:208 — no fallback consultation upstream), and the
/// bare creation head keeps its cut below: a class-like name is the
/// one bare callee whose invocation is no function-type composition
/// at all.
///
/// Deliberate silences. The `Some(false)` faces: a nullable written
/// formal at a non-nullable target formal, a required target formal
/// the lambda lacks, a constructor formal the lambda lacks. The
/// undecided `None` faces the fallback's gates fail: a
/// required-positional count mismatch (an optional target formal
/// against the untyped forward's required slot). The faces that never
/// reach the gate: a getter, field, or prefix callee (the member
/// face's accessor cut — no method signature composes); a
/// `receiver.method` whose receiver type the model cannot name; a
/// prefixed or unresolved creation head; a creation head published as
/// a value reference (the bare `C(a)` send, no type-use row) — of
/// which only the zero-argument shape is provable: a bare same-file
/// class-like name invoked with no arguments by a parameterless lambda
/// (`() => C()`, `test_constructorCall_noArgs`) is replaceable by
/// construction, since a compiling zero-argument call means the
/// constructor declares no required formal and optional leftovers on
/// the replacement side are admissible (`subtype.dart:141-146`,
/// :157-165). The undecided-but-plausible faces report through the
/// fallback — an untyped, `dynamic`, `void`, parameterized,
/// prefixed, function-typed, or record-typed written formal, a
/// generic callee, distinct named types — matching upstream, where
/// the context-inferred lambda type makes the tear-off stand (the
/// #2095 lane's live probes on 3.13.1; #2109's resolved-callee
/// channel). Message: `diagnostic.g.dart` `unnecessaryLambdas`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "unnecessary_lambdas",
    &[NodeKind::FunctionExpression],
    check,
);

fn check(c: &mut Check<'_>) {
    // The wave posture: the assignability gate (:208, :246) reads the
    // semantic face; face absent is the plain-run posture and stays
    // silent (the engine contract).
    let Some(analysis) = c.semantics() else {
        return;
    };
    let lambda = c.node();
    // `node.body.keyword != null` bail (:156): an async/`sync*` lambda
    // never reduces to a tear-off.
    if c.unit
        .ast
        .token_range(lambda)
        .any(|token| matches!(c.lexeme(token), "async" | "sync" | "await" | "yield"))
    {
        return;
    }
    // The body face: the single statement's run, or the arrow body's
    // expression pieces (:160-186).
    let Some((run, arrow)) = body_run(c, lambda) else {
        return;
    };
    // The oracle-of-record's initializer face (probed 2026-10-18 on
    // 3.13.1): a *block*-body lambda in variable-initializer position
    // stays silent — only arrow bodies report there, while argument
    // positions report for both body forms. Upstream main reads both
    // bodies alike; the shipped 3.13.1 linter does not.
    if !arrow
        && (1..=2).any(|up| {
            c.ancestor(up)
                .is_some_and(|row| c.kind(row) == NodeKind::VariableInitializer)
        })
    {
        return;
    }
    // The invocation face first (:163, :169, :178); the constructor
    // face (:181-185) takes an arrow body whose root is a creation.
    if let Some(root) = expression_root(c, &run, NodeKind::Send) {
        check_invocation(c, analysis, lambda, &run, root);
        return;
    }
    if arrow {
        for kind in [NodeKind::ImplicitCreation, NodeKind::NewExpression] {
            if let Some(root) = expression_root(c, &run, kind) {
                check_constructor(c, analysis, lambda, root);
                return;
            }
        }
    }
}

/// The lambda's body piece run: the single statement of a block body
/// (`(arrow, false)`), or the arrow expression's pieces
/// (`(arrow, true)`) — the lambda's own rows past the parameter list
/// (the childless `ExpressionBody` handle leaves the expression pieces
/// as the lambda row's children). `None` when no face applies: a block
/// body with zero or several statements, an unhandled statement kind.
fn body_run(c: &Check<'_>, lambda: u32) -> Option<(Vec<u32>, bool)> {
    let kids = c.collect_direct(lambda);
    let params_end = kids
        .iter()
        .copied()
        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
        .map_or(0, |params| c.children(params).end);
    if let Some(body) = kids
        .iter()
        .copied()
        .find(|&kid| c.kind(kid) == NodeKind::BlockBody)
    {
        // The statement rows sit among the statement's own flat pieces
        // (the send groups tile as siblings); count statement rows only.
        // The `ExpressionStatement` is a childless leaf — its operand
        // pieces are the body's flat children inside the statement's
        // token window; a `ReturnStatement` owns its operand children.
        let statements: Vec<u32> = c
            .collect_direct(body)
            .into_iter()
            .filter(|&row| crate::rules::is_statement_row(c, row))
            .collect();
        let [only] = statements[..] else {
            return None;
        };
        let run: Vec<u32> = match c.kind(only) {
            NodeKind::ExpressionStatement => {
                let span = c.byte_span(only);
                c.collect_direct(body)
                    .into_iter()
                    .filter(|&row| {
                        row != only
                            && c.byte_span(row).start >= span.start
                            && c.byte_span(row).end <= span.end
                    })
                    .collect()
            }
            NodeKind::ReturnStatement => c.collect_direct(only),
            _ => return None,
        };
        return Some((run, false));
    }
    let arrow = kids
        .iter()
        .copied()
        .any(|kid| c.kind(kid) == NodeKind::ExpressionBody);
    if !arrow {
        return None;
    }
    // The arrow expression's pieces: the lambda rows past the
    // parameter list (the body expression completes after the
    // parameters).
    Some(((params_end..c.children(lambda).end).collect(), true))
}

/// The run's widest row of `kind` — the expression root — unless any
/// other *expression* row is strictly wider (the marker's same-span tie
/// stands). Container markers (`ExpressionBody`, `FormalParameters`)
/// span the whole body and are not operands, so they never out-vote a
/// real expression root.
fn expression_root(c: &Check<'_>, run: &[u32], kind: NodeKind) -> Option<u32> {
    let root = run
        .iter()
        .copied()
        .filter(|&row| c.kind(row) == kind)
        .max_by_key(|&row| {
            let span = c.byte_span(row);
            span.end - span.start
        })?;
    let root_len = {
        let span = c.byte_span(root);
        span.end - span.start
    };
    run.iter()
        .filter(|&&row| {
            !matches!(
                c.kind(row),
                NodeKind::ExpressionBody
                    | NodeKind::FormalParameters
                    | NodeKind::NoFormalParameters
                    | NodeKind::BlockBody
                    | NodeKind::ExpressionStatement
                    | NodeKind::ReturnStatement
            )
        })
        .all(|&row| {
            let span = c.byte_span(row);
            span.end - span.start <= root_len
        })
        .then_some(root)
}

/// The invocation face (:228-289): the arguments match the lambda's
/// parameters, the chain carries no explicit type arguments or
/// null-aware link, the receiver (or bare callee) is final, and the
/// callee's signature is provably assignable to the lambda's.
fn check_invocation(
    c: &mut Check<'_>,
    analysis: &Analysis<'_>,
    lambda: u32,
    run: &[u32],
    root: u32,
) {
    let Some(pos) = run.iter().position(|&row| row == root) else {
        return;
    };
    let marker = slice::access_marker(c, run, pos, NodeKind::DotAccess);
    let Some((selector, arguments)) = slice::send_parts(c, run, pos) else {
        return;
    };
    // `node.typeArguments == null` bail (:285).
    if run
        .iter()
        .any(|&row| c.kind(row) == NodeKind::TypeArguments)
    {
        return;
    }
    // `containsNullAwareInvocationInChain` bail (:282): any null-aware
    // marker in the run's chain.
    if run.iter().any(|&row| {
        c.kind(row) == NodeKind::DotAccess && c.field(row, PayloadField::IsNullAware) == Some(1)
    }) {
        return;
    }
    // `argumentsMatchParameters` (:232-239).
    let Some(arguments) = arguments else {
        return;
    };
    let formals = lambda_formals(c, lambda);
    if !arguments_match_parameters(c, lambda, &formals, arguments) {
        return;
    }
    // The bare-callee face (`FunctionExpressionInvocation`, :254-262):
    // the selector must be a bare name (only wrapper markers may sit
    // between the name and the argument list).
    if marker.is_none() {
        // The bare callee's name is followed by its type-argument
        // wrapper, then the argument list. The argument *content* send
        // groups tile inside the `Arguments` token window — bound the
        // bare-name check by token position, not row position.
        let args_start = c.byte_span(arguments).start;
        let between = &run[selector + 1..pos];
        if between.iter().any(|&row| {
            c.byte_span(row).start < args_start
                && !matches!(
                    c.kind(row),
                    NodeKind::NoTypeArguments | NodeKind::TypeArguments
                )
        }) {
            return;
        }
    }
    // The finality face: the method receiver (:283) or the bare callee
    // (:260).
    let probe = match marker {
        Some(_) => match slice::bare_receiver(c, run, selector) {
            Some(receiver) if c.kind(receiver) == NodeKind::Identifier => receiver,
            // `isFinalNode`'s shape gate: a compound target is not
            // final.
            _ => return,
        },
        None => run[selector],
    };
    if c.kind(probe) != NodeKind::Identifier {
        return;
    }
    let name = c.lexeme(c.head_token(probe));
    if matches!(binding_finality(c, probe, name), Finality::NotFinal) {
        return;
    }
    // `invocationType` (:244-245, :266-267): the bare callee's own
    // declaration, or the receiver's method (a getter or field never
    // composes — the `methodName.element.isFinal` accessor cut, :284).
    let target = match marker {
        None => analysis.function_signature_of(query_site(c, probe)),
        Some(_) => {
            let method = analysis
                .strings()
                .intern(c.lexeme(c.head_token(run[selector])));
            analysis
                .member_of(query_site(c, probe), method)
                .and_then(|member| FnSig::of_member(member.entry))
        }
    };
    let fires = match target {
        // The :246-249 gate — assignability, else the fallback on the
        // undecided face.
        Some(target) => lambda_accepts(c, analysis, &formals, target, true),
        // No callable face composes. A same-file class-like name is
        // the creation head: only the zero-argument shape is provable
        // (the constructor formals the model cannot name stay
        // unprovable). Every other bare callee takes the
        // `parametersMatch` non-`FunctionType` arm (:111-114, "We
        // must assume is valid here"): an invocation type that
        // composes no model function type cannot fail the
        // assignability gate, and for a compiling forward the
        // arguments-match, finality, type-argument, and null-aware
        // gates above carry the verdict. A prefixed callee rides the
        // member face and stays with the creation cut.
        None => {
            marker.is_none()
                && if same_file_class(c, name) {
                    formals.is_empty() && zero_arguments(c, arguments)
                } else {
                    true
                }
        }
    };
    if fires {
        let range = c.utf16_span(lambda);
        c.report(range, &[]);
    }
}

/// The upstream zero-argument face: no argument list marker (the
/// width-0 `NoArguments`) or an empty `()` pair.
fn zero_arguments(c: &Check<'_>, arguments: u32) -> bool {
    c.kind(arguments) != NodeKind::Arguments || c.unit.ast.token_range(arguments).count() == 2
}

/// Whether `name` declares a same-file class-like type (the bare
/// creation head the model publishes as a value reference).
fn same_file_class(c: &Check<'_>, name: &str) -> bool {
    crate::rules::same_file_declaration(c, name).is_some_and(|row| {
        matches!(
            c.kind(row),
            NodeKind::ClassDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::EnumDeclaration
                | NodeKind::ExtensionDeclaration
                | NodeKind::ExtensionTypeDeclaration
        )
    })
}

/// The constructor face (:189-226): an arrow body over a `new` or
/// type-argumented creation whose constructor composes a signature
/// provably assignable to the lambda's (:208) and whose arguments are
/// bare identifiers naming the lambda's parameters by index with
/// equal counts (:212-223). The const form tiles as a
/// `ConstExpression` root and never reaches here (:195); a prefixed
/// head resolves no container (the deferred bail, :196).
fn check_constructor(c: &mut Check<'_>, analysis: &Analysis<'_>, lambda: u32, root: u32) {
    let kids = c.collect_direct(root);
    // The creation head: the type name (the type-use row the
    // constructor face keys on) and the optional `.name` selector.
    let Some(reference) = kids
        .iter()
        .copied()
        .find(|&kid| c.kind(kid) == NodeKind::ConstructorReference)
    else {
        return;
    };
    let mut names = c
        .collect_direct(reference)
        .into_iter()
        .filter(|&kid| c.kind(kid) == NodeKind::Identifier);
    let Some(type_row) = names.next() else {
        return;
    };
    let selector = names
        .next()
        .map(|row| analysis.strings().intern(c.lexeme(c.head_token(row))));
    let Some(binding) = analysis.resolve_constructor(query_site(c, type_row), selector) else {
        return;
    };
    if !binding.exists {
        return;
    }
    let formals = lambda_formals(c, lambda);
    let arguments = kids
        .iter()
        .copied()
        .find(|&kid| matches!(c.kind(kid), NodeKind::Arguments | NodeKind::NoArguments));
    if !constructor_arguments_match(c, &formals, arguments) {
        return;
    }
    // The constructor's signature: its declared formals, else the
    // implicit default constructor's empty table.
    let implicit = Formals {
        positional: Box::new([]),
        named: Box::new([]),
    };
    let target = FnSig::new(
        0,
        Ty::named(binding.container),
        binding.formals.unwrap_or(&implicit),
    );
    // :208 reads plain assignability — no fallback consultation.
    if lambda_accepts(c, analysis, &formals, target, false) {
        let range = c.utf16_span(lambda);
        c.report(range, &[]);
    }
}

/// The constructor arguments face (:212-223): every argument is a bare
/// identifier (a named argument is not, :213), the counts are equal
/// (:217), and each names the lambda's parameter at the same index
/// (:219-223 — a lexeme comparison).
fn constructor_arguments_match(
    c: &Check<'_>,
    formals: &[LambdaFormal<'_>],
    arguments: Option<u32>,
) -> bool {
    let Some(arguments) = arguments.filter(|&row| c.kind(row) == NodeKind::Arguments) else {
        return formals.is_empty();
    };
    let raw = c.collect_direct(arguments);
    let ranges = slice::arguments(c, &raw);
    ranges.len() == formals.len()
        && ranges.into_iter().zip(formals).all(|(range, formal)| {
            c.kind(raw[range.end - 1]) != NodeKind::NamedArgument
                && bare_argument_name(c, &raw[range]) == Some(formal.name)
        })
}

/// The assignability gate (:208, :246): the callee's signature stands
/// where the lambda's does. `Some(true)` reports, `Some(false)` stays
/// silent, and the undecided `None` consults the `parametersMatch`
/// fallback — the invocation gate only (:246-249; the constructor
/// gate :208 reads plain assignability upstream), per `fallback`.
fn lambda_accepts(
    c: &Check<'_>,
    analysis: &Analysis<'_>,
    formals: &[LambdaFormal<'_>],
    target: FnSig<'_>,
    fallback: bool,
) -> bool {
    let own = lambda_formal_table(c, analysis, formals);
    match FnSig::new(0, Ty::Void, &own).assignable_from(target) {
        Some(accepts) => accepts,
        None => fallback && parameters_match(&target, &own),
    }
}

/// The `parametersMatch` FunctionType path (:112-150), the undecided
/// face's gate: the required-positional counts match (:112-120); the
/// optional-positional pairs to the minimum length (:121-128) and the
/// invocation's named formals (:129-149 — absent required fails,
/// absent optional skips, required flags agree) assign, an undecided
/// leaf passing (the "we must assume is valid here" posture, :109)
/// and a decided-false one failing. The invocation type is a function
/// type here by construction: a callee composing no model signature
/// takes the non-`FunctionType` arm at the call site (:108-111).
fn parameters_match(invocation: &FnSig<'_>, node: &Formals) -> bool {
    let invocation_positional = &invocation.formals.positional;
    if invocation_positional
        .iter()
        .filter(|formal| formal.required)
        .count()
        != node
            .positional
            .iter()
            .filter(|formal| formal.required)
            .count()
    {
        return false;
    }
    let node_optional = node.positional.iter().filter(|formal| !formal.required);
    if invocation_positional
        .iter()
        .filter(|formal| !formal.required)
        .zip(node_optional)
        .any(|(invocation_param, node_param)| {
            node_param.ty.assignable_from(invocation_param.ty) == Some(false)
        })
    {
        return false;
    }
    for invocation_param in invocation.formals.named.iter() {
        match node.named_formal(invocation_param.name) {
            None => {
                if invocation_param.required {
                    return false;
                }
            }
            Some(node_param) => {
                if invocation_param.required != node_param.required
                    || node_param.ty.assignable_from(invocation_param.ty) == Some(false)
                {
                    return false;
                }
            }
        }
    }
    true
}

/// The `argumentsMatchParameters` face (dart_type_utilities.dart:13-64):
/// every argument is the lambda's own parameter — positional in order
/// (:47, :51-55), named by name (:57-61), both counts equal (:47-49),
/// parens unwrap.
fn arguments_match_parameters(
    c: &Check<'_>,
    lambda: u32,
    formals: &[LambdaFormal<'_>],
    args: u32,
) -> bool {
    let positional_params: Vec<&str> = formals
        .iter()
        .filter(|formal| formal.is_positional())
        .map(|formal| formal.name)
        .collect();
    let named_params: Vec<&str> = formals
        .iter()
        .filter(|formal| !formal.is_positional())
        .map(|formal| formal.name)
        .collect();
    let raw = c.collect_direct(args);
    let ranges = slice::arguments(c, &raw);
    let mut positional = 0usize;
    let mut named = 0usize;
    for range in ranges {
        let (arg_name, value): (Option<&str>, &[u32]) =
            if c.kind(raw[range.end - 1]) == NodeKind::NamedArgument {
                (
                    Some(c.lexeme(c.head_token(raw[range.start]))),
                    &raw[range.start + 1..range.end - 1],
                )
            } else {
                (None, &raw[range])
            };
        let Some(value_name) = bare_argument_name(c, value) else {
            return false;
        };
        // The argument must be the lambda's own parameter (the
        // canonical-element identity face): the bare identifier probe
        // resolves past closer bindings to the lambda's parameter.
        let Some(&probe) = value.iter().find(|&&row| {
            c.kind(row) == NodeKind::Identifier && c.lexeme(c.head_token(row)) == value_name
        }) else {
            return false;
        };
        if !resolves_to_lambda_parameter(c, probe, lambda, formals, value_name) {
            return false;
        }
        if let Some(arg_name) = arg_name {
            if !named_params.contains(&arg_name) || value_name != arg_name {
                return false;
            }
            named += 1;
        } else {
            if positional >= positional_params.len() || positional_params[positional] != value_name
            {
                return false;
            }
            positional += 1;
        }
    }
    positional == positional_params.len() && named == named_params.len()
}

/// The bare-identifier argument name: the argument's piece run is a
/// single tear-off group ([`slice::is_tear_off_group`]), parens
/// unwrap.
fn bare_argument_name<'a>(c: &'a Check<'_>, value: &[u32]) -> Option<&'a str> {
    let mut owned;
    let mut pieces = value;
    loop {
        if pieces.len() == 1 && c.kind(pieces[0]) == NodeKind::ParenthesizedExpression {
            owned = c.collect_direct(pieces[0]);
            pieces = &owned;
            continue;
        }
        break;
    }
    (pieces.len() == 4 && slice::is_tear_off_group(c, pieces, 0))
        .then(|| c.lexeme(c.head_token(pieces[0])))
}

/// One formal of the lambda's parameter list: its name, its row, and
/// its parser kind (payload word 4 — required/optional × positional/
/// named).
struct LambdaFormal<'a> {
    name: &'a str,
    row: u32,
    kind: u32,
}

impl LambdaFormal<'_> {
    fn is_positional(&self) -> bool {
        self.kind == dartq_parser::FormalParameterKind::RequiredPositional as u32
            || self.kind == dartq_parser::FormalParameterKind::OptionalPositional as u32
    }

    fn is_required(&self) -> bool {
        self.kind == dartq_parser::FormalParameterKind::RequiredPositional as u32
            || self.kind == dartq_parser::FormalParameterKind::RequiredNamed as u32
    }
}

/// The lambda's formals in declaration order (`node.parameters`): the
/// `(...)` group's own rows, then the nested `[...]`/`{...}` group's.
/// A nameless formal (recovery) is skipped.
fn lambda_formals<'a>(c: &'a Check<'_>, lambda: u32) -> Vec<LambdaFormal<'a>> {
    let mut formals = Vec::new();
    let Some(params) = c
        .collect_direct(lambda)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
    else {
        return formals;
    };
    let mut push = |row: u32| {
        if c.kind(row) == NodeKind::FormalParameter
            && let Some(name) = parameter_name(c, row)
        {
            let kind = c
                .field(row, PayloadField::FormalParameterKind)
                .unwrap_or(dartq_parser::FormalParameterKind::RequiredPositional as u32);
            formals.push(LambdaFormal { name, row, kind });
        }
    };
    for kid in c.collect_direct(params) {
        if c.kind(kid) == NodeKind::OptionalFormalParameters {
            for inner in c.collect_direct(kid) {
                push(inner);
            }
        } else {
            push(kid);
        }
    }
    formals
}

/// The name token of a formal parameter (the row's `Name` word).
fn parameter_name<'a>(c: &'a Check<'_>, parameter: u32) -> Option<&'a str> {
    let token = c.field(parameter, PayloadField::Name)?;
    (token != NONE_TOKEN).then(|| c.lexeme(token))
}

/// The lambda's own formal table (`nodeType.formalParameters`, :242):
/// names and parser kinds as written, each type through
/// [`written_ty`].
fn lambda_formal_table(
    c: &Check<'_>,
    analysis: &Analysis<'_>,
    formals: &[LambdaFormal<'_>],
) -> Formals {
    let mut positional = Vec::new();
    let mut named = Vec::new();
    for formal in formals {
        let entry = Formal {
            name: analysis.strings().intern(formal.name),
            ty: written_ty(c, analysis, formal.row),
            required: formal.is_required(),
            covariant: false,
            default_value: None,
        };
        if formal.is_positional() {
            positional.push(entry);
        } else {
            named.push(entry);
        }
    }
    Formals {
        positional: positional.into_boxed_slice(),
        named: named.into_boxed_slice(),
    }
}

/// The written type of a lambda formal as the model's own `Ty` — only
/// the shapes the model's collector canonicalizes identically: a bare
/// name with an optional `?` (the argument-less `Named`), and `Null`.
/// Everything the comparison leaf cannot decide from written facts
/// stays [`Ty::Unknown`] (`None` downstream): an untyped formal (its
/// type is inferred from the context upstream), a parameterized type
/// (the argument slots are pool-bound), a prefixed name, a `void`,
/// function, or record type, and a written `dynamic` — the leaf's
/// `dynamic`-downcast arm is `isAssignableTo`'s whole-type rule, not
/// the function-subtype parameter leaf's `isSubtypeOf`
/// (`subtype.dart:127`), so a non-top target formal would over-prove.
fn written_ty(c: &Check<'_>, analysis: &Analysis<'_>, parameter: u32) -> Ty {
    let kids = c.collect_direct(parameter);
    let Some(index) = kids.iter().position(|&kid| {
        matches!(
            c.kind(kid),
            NodeKind::Type
                | NodeKind::VoidType
                | NodeKind::FunctionType
                | NodeKind::RecordType
                | NodeKind::FunctionTypedFormalParameter
        )
    }) else {
        return Ty::Unknown;
    };
    // The type-argument group seals before its head (`List<int>`).
    if c.kind(kids[index]) != NodeKind::Type
        || kids[..index]
            .iter()
            .any(|&kid| c.kind(kid) == NodeKind::TypeArguments)
    {
        return Ty::Unknown;
    }
    let mut tokens = c.unit.ast.token_range(kids[index]);
    let Some(head) = tokens.next() else {
        return Ty::Unknown;
    };
    let nullable = match (tokens.next(), tokens.next()) {
        (None, _) => false,
        (Some(tail), None) if c.lexeme(tail) == "?" => true,
        // A prefixed name (`p.T`) or any wider spelling.
        _ => return Ty::Unknown,
    };
    match c.lexeme(head) {
        "dynamic" | "void" => Ty::Unknown,
        "Null" => Ty::Null,
        name => Ty::Named {
            name: analysis.strings().intern(name),
            nullable,
            args: ArgListId::EMPTY,
        },
    }
}

/// The canonical-element identity face for an argument: no closer
/// binding of the name sits between the use site and the lambda's own
/// parameter list.
fn resolves_to_lambda_parameter(
    c: &Check<'_>,
    use_row: u32,
    lambda: u32,
    formals: &[LambdaFormal<'_>],
    name: &str,
) -> bool {
    let mut current = use_row;
    loop {
        let Some(parent) = c.unit.ast.parent(current) else {
            return false;
        };
        current = parent;
        match c.kind(current) {
            NodeKind::Block | NodeKind::BlockBody => {
                if block_binds(c, current, c.byte_span(use_row).start, name).is_some() {
                    return false;
                }
            }
            NodeKind::FunctionExpression
            | NodeKind::NamedFunctionExpression
            | NodeKind::LocalFunctionDeclaration => {
                if current == lambda {
                    return formals.iter().any(|formal| formal.name == name);
                }
                if params_binds(c, current, name) {
                    return false;
                }
            }
            NodeKind::CatchClause => {
                let ctx = dartq_parser::IdentifierContext::CatchParameter as u32;
                if c.children(current).any(|row| {
                    c.kind(row) == NodeKind::Identifier
                        && c.field(row, PayloadField::Context) == Some(ctx)
                        && c.lexeme(c.head_token(row)) == name
                }) {
                    return false;
                }
            }
            _ => {}
        }
    }
}

/// Whether a closure's parameter list binds `name`.
fn params_binds(c: &Check<'_>, owner: u32, name: &str) -> bool {
    c.collect_direct(owner)
        .into_iter()
        .filter(|&kid| c.kind(kid) == NodeKind::FormalParameters)
        .any(|params| {
            c.collect_direct(params).into_iter().any(|kid| {
                c.kind(kid) == NodeKind::FormalParameter && parameter_name(c, kid) == Some(name)
            })
        })
}

/// The keyword token of the block-local of `name` declared before the
/// use site (the `block_binding` scan shape); a keyword-less typed
/// declaration (`K k = …;`) answers its head token, which
/// [`keyword_finality`] reads as not final (a mutable variable).
fn block_binds(c: &Check<'_>, block: u32, use_start: usize, name: &str) -> Option<u32> {
    let ctx = dartq_parser::IdentifierContext::LocalVariableDeclaration as u32;
    c.collect_direct(block).into_iter().find_map(|kid| {
        if c.kind(kid) != NodeKind::VariablesDeclaration {
            return None;
        }
        let binds = c.collect_direct(kid).into_iter().any(|var| {
            let name_row = if c.kind(var) == NodeKind::Identifier {
                Some(var)
            } else {
                c.collect_direct(var)
                    .into_iter()
                    .find(|&piece| c.kind(piece) == NodeKind::Identifier)
            };
            name_row.is_some_and(|row| {
                c.field(row, PayloadField::Context) == Some(ctx)
                    && c.lexeme(c.head_token(row)) == name
                    && c.byte_span(row).start < use_start
            })
        });
        binds.then(|| variables_declaration_keyword(c, kid).unwrap_or_else(|| c.head_token(kid)))
    })
}

/// The finality verdict of the binding `name` at the use site (the
/// upstream `isFinal` extension, :326-335, under `isFinalNode`'s
/// SimpleIdentifier arm, :70-76): a `final`/`const` variable, field,
/// or any function is final; a parameter, getter, `late` variable, or
/// mutable variable is not; the walk's terminal — the enclosing class's
/// own name (a constructor), cross-file and unresolved names — leaves
/// the verdict to the signature gate (`Unknown`).
fn binding_finality(c: &Check<'_>, use_row: u32, name: &str) -> Finality {
    let use_start = c.byte_span(use_row).start;
    let mut current = use_row;
    loop {
        let Some(parent) = c.unit.ast.parent(current) else {
            return Finality::Unknown;
        };
        current = parent;
        match c.kind(current) {
            NodeKind::Block | NodeKind::BlockBody => {
                if let Some(keyword) = block_binds(c, current, use_start, name) {
                    return keyword_finality(c, keyword);
                }
            }
            NodeKind::FunctionExpression
            | NodeKind::NamedFunctionExpression
            | NodeKind::LocalFunctionDeclaration => {
                if params_binds(c, current, name) {
                    return Finality::NotFinal;
                }
            }
            NodeKind::CatchClause => {
                let ctx = dartq_parser::IdentifierContext::CatchParameter as u32;
                if c.children(current).any(|row| {
                    c.kind(row) == NodeKind::Identifier
                        && c.field(row, PayloadField::Context) == Some(ctx)
                        && c.lexeme(c.head_token(row)) == name
                }) {
                    return Finality::NotFinal;
                }
            }
            NodeKind::Method
            | NodeKind::Constructor
            | NodeKind::Factory
            | NodeKind::TopLevelMethod => {
                // The enclosing member's own parameters bind first.
                if params_binds(c, current, name) {
                    return Finality::NotFinal;
                }
                // The unit-level member of that name: a getter is the
                // accessor face (not final); a function takes the
                // `_ => true` fallback. The stamped-name read only —
                // matching any `Identifier` would bind a return type
                // spelled like the use (`foo bar()` reading `foo` as a
                // member).
                if c.kind(current) == NodeKind::TopLevelMethod
                    && crate::rules::member_name(c, current)
                        .is_some_and(|kid| c.lexeme(c.head_token(kid)) == name)
                {
                    return if get_set_token(c, current) == Some("get") {
                        Finality::NotFinal
                    } else {
                        Finality::Final
                    };
                }
            }
            NodeKind::ClassDeclaration
            | NodeKind::MixinDeclaration
            | NodeKind::EnumDeclaration
            | NodeKind::ExtensionDeclaration
            | NodeKind::ExtensionTypeDeclaration => {
                // The class's own name: a constructor, not a variable
                // — the creation faces decide.
                if crate::rules::class_declaration_name(c, current) == Some(name) {
                    return Finality::Unknown;
                }
                if let Some(finality) = class_member_finality(c, current, name) {
                    return finality;
                }
            }
            NodeKind::CompilationUnit => {
                for top in c.collect_direct(current) {
                    for inner in c.collect_direct(top) {
                        if c.kind(inner) == NodeKind::TopLevelFields
                            && let Some(finality) = fields_finality(c, inner, name)
                        {
                            return finality;
                        }
                    }
                }
                // The same-file substrate ended without a verdict: a
                // top-level function, a cross-file or unresolved name
                // — the signature gate's resolution decides.
                return Finality::Unknown;
            }
            _ => {}
        }
    }
}

/// The finality of a `final`/`const`/`var` keyword token: `late`
/// preceding it demotes the verdict.
fn keyword_finality(c: &Check<'_>, keyword: u32) -> Finality {
    match c.lexeme(keyword) {
        "final" | "const" => {
            let late = (keyword > 0)
                && c.unit
                    .tokens
                    .get(keyword as usize - 1)
                    .is_some_and(|_| c.lexeme(keyword - 1) == "late");
            if late {
                Finality::NotFinal
            } else {
                Finality::Final
            }
        }
        _ => Finality::NotFinal,
    }
}

/// The finality of the class-like `name` member: a getter is the
/// accessor face (not final); a field reads its keyword; a method or
/// constructor binds nothing for this face (`None` continues the
/// walk).
fn class_member_finality(c: &Check<'_>, class_row: u32, name: &str) -> Option<Finality> {
    for member in c.collect_direct(class_row) {
        match c.kind(member) {
            NodeKind::Fields => {
                if let Some(finality) = fields_finality(c, member, name) {
                    return Some(finality);
                }
            }
            NodeKind::Method => {
                let binds = c.collect_direct(member).into_iter().any(|kid| {
                    c.kind(kid) == NodeKind::Identifier && c.lexeme(c.head_token(kid)) == name
                });
                if binds {
                    // A getter is the accessor face (not final); a
                    // bound non-getter method is the tear-off face —
                    // the bare invocation resolves the container's own
                    // method first (implicit `this`), and the upstream
                    // executable default fires (`methodName.element
                    // .isFinal`, the extension's `_ => true` arm).
                    return Some(if get_set_token(c, member) == Some("get") {
                        Finality::NotFinal
                    } else {
                        Finality::Final
                    });
                }
            }
            _ => {}
        }
    }
    None
}

/// The finality of the variable `name` in a `Fields`/`TopLevelFields`
/// declaration: the declaration's keyword decides (the
/// `variables_declaration_keyword` face). Both member tilings scan —
/// the name as a direct `Identifier` child or inside a
/// `VariablesDeclaration`.
fn fields_finality(c: &Check<'_>, fields: u32, name: &str) -> Option<Finality> {
    let ctx = if c.kind(fields) == NodeKind::TopLevelFields {
        dartq_parser::IdentifierContext::TopLevelVariableDeclaration as u32
    } else {
        dartq_parser::IdentifierContext::FieldDeclaration as u32
    };
    let binds = c.collect_direct(fields).into_iter().any(|kid| {
        if c.kind(kid) == NodeKind::Identifier {
            return c.field(kid, PayloadField::Context) == Some(ctx)
                && c.lexeme(c.head_token(kid)) == name;
        }
        if c.kind(kid) != NodeKind::VariablesDeclaration {
            return false;
        }
        c.collect_direct(kid).into_iter().any(|var| {
            let name_row = if c.kind(var) == NodeKind::Identifier {
                Some(var)
            } else {
                c.collect_direct(var)
                    .into_iter()
                    .find(|&piece| c.kind(piece) == NodeKind::Identifier)
            };
            name_row.is_some_and(|row| {
                c.field(row, PayloadField::Context) == Some(ctx)
                    && c.lexeme(c.head_token(row)) == name
            })
        })
    });
    binds.then(|| {
        variables_declaration_keyword(c, fields)
            .map_or(Finality::NotFinal, |keyword| keyword_finality(c, keyword))
    })
}

/// The binding finality verdict: `NotFinal` bails; `Final` and
/// `Unknown` (the walk's terminal) proceed to the signature gate.
enum Finality {
    Final,
    NotFinal,
    Unknown,
}
