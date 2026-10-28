use super::{PayloadField, Check, NodeKind, RuleSpec, constructor_name_part, container_members, container_name_node, initializer_expression, modifier_keyword, parameter_name, parameter_rows};
use crate::rules::{NONE_TOKEN, SupertypeClause, initialized_identifier_name, supertypes, utf16_token_span, variables_declaration_keyword};
use dartq_ast::mod_flags;
use dartq_language::LanguageVersion;
use dartq_parser::{IdentifierContext, MemberKind};

/// `Feature.enhanced_enums` (`enabledIn: 2.17.0`) — the upstream
/// registration gate folds to the settled language version; `None`
/// (a caller without a settled face) is the latest-default posture.
const ENHANCED_ENUMS: LanguageVersion = LanguageVersion::new(2, 17);
/// Bridge `mod_bits` class-family `augment` / `abstract`.
const AUGMENT_BIT: u16 = 1 << 1;
const ABSTRACT_BIT: u16 = 1 << 2;

/// Official: `pkg/linter/lib/src/rules/use_enums.dart:137-223` — a
/// class that behaves like an enum reports at its name (:221): not
/// abstract, extending `Object` at most (:149), a const primary
/// constructor with a private-or-absent name and no
/// `hashCode`/`index`/`values` parameter when one is written
/// (:153-168), at least two `static const` fields initialized by
/// generative constructor invocations of the class (:183-200,
/// :212), no `hashCode`/`index`/`==`/`values` member (:98-105, :200),
/// every constructor factory-or-const with the public naming gate
/// (:204-208), no generative invocation inside the class outside the
/// candidate initializers (:61-89), and no outside use — no subclass,
/// interface or mixin, no generative invocation (:100-135).
///
/// Approximations (documented — the evaluator and cross-library cut):
/// `computeConstantValue() != null` (:196) drops — a constant error
/// inside a candidate initializer over-reports; the library walk is
/// the single-file face — a part-file use under-reports its bail; a
/// generative constructor that does not exist (erroneous code)
/// resolves by shape. The feature gate folds to the settled version
/// (upstream `isFeatureEnabled` at registration; the per-dispatch
/// consult is observationally identical). Message: `diagnostic.g.dart`
/// `useEnums`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "use_enums",
    &[NodeKind::ClassDeclaration],
    check,
);

fn check(c: &mut Check<'_>) {
    // `context.isFeatureEnabled(Feature.enhanced_enums)` (:32).
    if c.unit
        .version
        .is_some_and(|version| version < ENHANCED_ENUMS)
    {
        return;
    }
    let node = c.node();
    let flags = mod_flags(c.unit.ast, node);
    // `node.isAugmentation` (:142), `node.abstractKeyword != null` (:144).
    if flags & (AUGMENT_BIT | ABSTRACT_BIT) != 0 {
        return;
    }
    // Enums can only extend `Object` (:149) — no `extends`, or the
    // written `extends Object`.
    if supertypes(c, node)
        .into_iter()
        .any(|(name, clause)| clause == SupertypeClause::Extends && name != "Object")
    {
        return;
    }
    let Some(name_node) = container_name_node(c, node) else {
        return;
    };
    let class_name = c.lexeme(c.head_token(name_node));
    let public = !class_name.starts_with('_');
    // The primary-constructor header face (:153-168).
    if let Some(primary) = c
        .collect_direct(node)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::PrimaryConstructor)
        && !primary_ok(c, primary, public)
    {
        return;
    }
    // `body is! BlockClassBody` (:170-173) — the member container.
    let (body, members) = container_members(c, node);
    if c.kind(body) != NodeKind::DeclarationBody {
        return;
    }
    // The generative constructor names, hoisted out of the scans (the
    // member read is O(arena); the scans consult the set per row).
    let ctors = generative_ctor_names(c, node, &members);
    let mut candidates: Vec<u32> = Vec::new();
    if !member_walk(c, class_name, public, &members, &ctors, &mut candidates) {
        return;
    }
    // `candidateConstants.length < 2` (:212).
    if candidates.len() < 2 {
        return;
    }
    // The class-internal visitor (:61-89).
    if class_scan_bails(c, node, class_name, &ctors, &candidates) {
        return;
    }
    // The library visitor (:100-135).
    if !outside_clean(c, node, class_name, &ctors) {
        return;
    }
    // `rule.reportAtToken(node.namePart.typeName)` (:221).
    c.report(utf16_token_span(c, c.head_token(name_node)), &[]);
}

/// The primary-constructor face (:153-168): the header `const`
/// (the row's `ConstKeyword` word, bridge.rs `end_primary_constructor`), the
/// public-class naming gate (:158-161 — the name must be written and
/// private), and no `hashCode`/`index`/`values` parameter (:162-167).
fn primary_ok(c: &Check<'_>, primary: u32, public: bool) -> bool {
    if c.field(primary, PayloadField::ConstKeyword).unwrap_or(NONE_TOKEN) == NONE_TOKEN {
        return false;
    }
    if public {
        let named = c.field(primary, PayloadField::HasConstructorName).unwrap_or(0) != 0;
        let name = c.collect_direct(primary).into_iter().find(|&kid| {
            c.kind(kid) == NodeKind::Identifier
                && c.field(kid, PayloadField::Context)
                    == Some(IdentifierContext::PrimaryConstructorDeclaration as u32)
        });
        let private = name.is_some_and(|n| c.lexeme(c.head_token(n)).starts_with('_'));
        if !named || !private {
            return false;
        }
    }
    if let Some(params) = c
        .collect_direct(primary)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
    {
        for parameter in parameter_rows(c, params) {
            if matches!(
                parameter_name(c, parameter),
                Some("hashCode" | "index" | "values")
            ) {
                return false;
            }
        }
    }
    true
}

/// The member walk (:177-210): `false` — a disqualifier hit. The
/// `isHashCode`/`isIndex`/`isEquals`/`isValues` names (:98-105, :200)
/// return; a generative constructor must be const with the public
/// naming gate (:204-208); the static const fields whose initializers
/// are class creations collect (:183-200).
fn member_walk(
    c: &Check<'_>,
    class_name: &str,
    public: bool,
    members: &[u32],
    ctors: &[Option<&str>],
    candidates: &mut Vec<u32>,
) -> bool {
    for member in members {
        match c.kind(*member) {
            NodeKind::Method => {
                if method_disqualifies(c, *member) {
                    return false;
                }
            }
            NodeKind::Fields => {
                if fields_disqualify(c, *member) {
                    return false;
                }
                // `member.isStatic` (:184).
                if c.field(*member, PayloadField::MemberKind)
                    != Some(MemberKind::StaticField as u32)
                {
                    continue;
                }
                collect_candidates(c, class_name, *member, ctors, candidates);
            }
            NodeKind::Constructor => {
                // `!isFactory && !isConst` (:204) — a `Constructor` row
                // anchors at the declared name, so the header `const`
                // sits in the modifier run before the span head.
                if modifier_keyword(c, *member, "const").is_none()
                    || (public && !private_named(c, *member))
                {
                    return false;
                }
            }
            NodeKind::Factory
                // `isFactory` passes the first gate; the naming gate
                // holds (:205-208).
                if public && !private_named(c, *member) => {
                    return false;
                }
            _ => {}
        }
    }
    true
}

/// `isHashCode`/`isIndex`/`isValues` (:102-105, :200) — the field
/// element's name is the first declarator's.
fn fields_disqualify(c: &Check<'_>, fields: u32) -> bool {
    c.collect_direct(fields)
        .into_iter()
        .find(|&decl| c.kind(decl) == NodeKind::VariablesDeclaration)
        .and_then(|decl| {
            c.collect_direct(decl)
                .into_iter()
                .find(|&var| c.kind(var) == NodeKind::InitializedIdentifier)
        })
        .and_then(|var| initialized_identifier_name(c, var))
        .is_some_and(|name| matches!(c.lexeme(name), "hashCode" | "index" | "values"))
}

/// `isEquals`/`isHashCode`/`isIndex`/`isValues` (:98-105, :200) — the
/// declared name: the `MethodDeclaration`-context identifier (the
/// return type's `TypeReference` identifier tiles as an earlier
/// sibling and must not win the first-identifier read), an
/// `OperatorName` naming through its payload token (the `==` itself;
/// the span head would hit the `operator` keyword). The operator row
/// tiles as the method's left sibling in the body container, not as
/// its child.
fn method_disqualifies(c: &Check<'_>, member: u32) -> bool {
    let ctx = IdentifierContext::MethodDeclaration as u32;
    let is_name = |kid: u32| {
        c.kind(kid) == NodeKind::OperatorName
            || (c.kind(kid) == NodeKind::Identifier && c.field(kid, PayloadField::Context) == Some(ctx))
    };
    let name = c
        .collect_direct(member)
        .into_iter()
        .find(|&kid| is_name(kid))
        .or_else(|| {
            let parent = c.unit.ast.parent(member)?;
            let siblings = c.collect_direct(parent);
            let pos = siblings.iter().position(|&kid| kid == member)?;
            siblings[..pos]
                .iter()
                .copied()
                .rev()
                .find(|&kid| is_name(kid))
        });
    name.is_some_and(|name| {
        let spelled = match c.kind(name) {
            NodeKind::OperatorName => c
                .field(name, PayloadField::Name)
                .map_or("", |token| c.lexeme(token)),
            _ => c.lexeme(c.head_token(name)),
        };
        matches!(spelled, "==" | "hashCode" | "index" | "values")
    })
}

/// The public-class naming gate (:159, :206): the constructor must be
/// named and private (`name == null || !name.isPrivateName` bails).
fn private_named(c: &Check<'_>, ctor: u32) -> bool {
    constructor_name_part(c, ctor)
        .is_some_and(|part| c.lexeme(c.head_token(part)).starts_with('_'))
}

/// The candidate collection (:185-200): each `static const`
/// declarator whose initializer's outer creation invokes a generative
/// constructor of the class joins (`isFactory` and the foreign-class
/// faces drop, :192-195; `computeConstantValue() != null` drops with
/// the evaluator — the documented approximation).
fn collect_candidates(
    c: &Check<'_>,
    class_name: &str,
    fields: u32,
    ctors: &[Option<&str>],
    candidates: &mut Vec<u32>,
) {
    for decl in c.collect_direct(fields) {
        if c.kind(decl) != NodeKind::VariablesDeclaration
            || variables_declaration_keyword(c, decl).is_none_or(|kw| c.lexeme(kw) != "const")
        {
            continue;
        }
        for var in c.collect_direct(decl) {
            if c.kind(var) != NodeKind::InitializedIdentifier {
                continue;
            }
            // `initializer is! InstanceCreationExpression` (:190) — the
            // creation row under the initializer (the send face, or the
            // `const` wrapper's content). The candidate exempts the
            // initializer's outer tail row itself (the class scan meets
            // the wrapper, not the inner callee row).
            let Some(expr) = initializer_expression(c, var) else {
                continue;
            };
            let Some(creation) = outer_creation(c, expr) else {
                continue;
            };
            if creation_invokes(c, class_name, ctors, creation) {
                candidates.push(expr);
            }
        }
    }
}

/// The generative-constructor names of the class: `None` for the
/// unnamed constructor, `Some(name)` for each named one. The primary
/// constructor joins (the header face is the class's own generative
/// constructor — `A._(1)` inside a `class const A._(…)` resolves to
/// it); `Factory` rows are not generative (:51). `members` is the
/// already-read member list (the read is O(arena)).
fn generative_ctor_names<'a>(
    c: &'a Check<'_>,
    class_row: u32,
    members: &[u32],
) -> Vec<Option<&'a str>> {
    let mut names: Vec<Option<&'a str>> = Vec::new();
    for kid in c.collect_direct(class_row) {
        if c.kind(kid) != NodeKind::PrimaryConstructor {
            continue;
        }
        let has_name = c.field(kid, PayloadField::HasConstructorName).unwrap_or(0) != 0;
        let name = c.collect_direct(kid).into_iter().find(|&inner| {
            c.kind(inner) == NodeKind::Identifier
                && c.field(inner, PayloadField::Context)
                    == Some(IdentifierContext::PrimaryConstructorDeclaration as u32)
        });
        names.push(match (has_name, name) {
            (true, Some(name)) => Some(c.lexeme(c.head_token(name))),
            _ => None,
        });
    }
    for member in members {
        if c.kind(*member) == NodeKind::Constructor {
            names.push(
                constructor_name_part(c, *member).map(|part| c.lexeme(c.head_token(part))),
            );
        }
    }
    names
}

/// Whether the selector names a generative constructor (`None`/`new`
/// = the unnamed one).
fn selector_resolves(ctors: &[Option<&str>], selector: Option<&str>) -> bool {
    match selector {
        None | Some("new") => ctors.iter().any(Option::is_none),
        Some(name) => ctors.contains(&Some(name)),
    }
}

/// The `invokesGenerativeConstructor` predicate (:48-53): the
/// creation's type name binds the class, the selector resolves against
/// the generative set, and an argument list is present (a tear-off is
/// no creation).
fn creation_invokes(
    c: &Check<'_>,
    class_name: &str,
    ctors: &[Option<&str>],
    creation: u32,
) -> bool {
    match c.kind(creation) {
        NodeKind::Send => send_creation(c, class_name, ctors, creation),
        NodeKind::NewExpression
        | NodeKind::ImplicitCreation
        | NodeKind::ConstructorReference => structured_creation(c, class_name, ctors, creation),
        // A written-`const` creation spells no send — its callee is
        // the `ConstructorReference` row inside the wrapper.
        NodeKind::ConstExpression | NodeKind::ConstLiteral => c
            .collect_direct(creation)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::ConstructorReference)
            .is_some_and(|reference| structured_creation(c, class_name, ctors, reference)),
        _ => false,
    }
}

/// The send face (`A._(1)`): the owner run cut at the last statement
/// row, the chain's endpoints resolved by span contiguity — each `Send`
/// segment's span butts the next one's (`A .` → `._()`). The head
/// segment's span head names the type, the tail's `DotAccess` marker
/// names the selector, and the argument list sits immediately before
/// the tail send. A receiver past the first pair (`a.A._()`, `x.A()`)
/// or a headless shape under-reports (the
/// `const_ability::parse_callee` face).
fn send_creation(c: &Check<'_>, class_name: &str, ctors: &[Option<&str>], send: u32) -> bool {
    let Some(parent) = c.unit.ast.parent(send) else {
        return false;
    };
    let run = crate::rules::slice::statement_run(c, &c.collect_direct(parent));
    let Some(pos) = run.iter().position(|&kid| kid == send) else {
        return false;
    };
    let range = |row: u32| c.unit.ast.token_range(row);
    // The chain head: the leftmost `Send` segment whose span butts the
    // next one's (`A .` → `._()`); `send` itself is the invoke send —
    // a receiver segment reads `NoArguments` before it and answers
    // false here, so the class scan never mistakes `A.` for a
    // creation.
    let mut head = pos;
    let mut segments = 1usize;
    loop {
        let start = range(run[head]).start;
        match run[..head]
            .iter()
            .rposition(|&row| c.kind(row) == NodeKind::Send && range(row).end == start)
        {
            Some(prev) => {
                head = prev;
                segments += 1;
            }
            None => break,
        }
    }
    // More than the receiver+selector pair is a chain past the first
    // selector (`a.A._()`), and a `Qualified` prefix under-reports.
    if segments > 2
        || run[..head]
            .iter()
            .any(|&row| c.kind(row) == NodeKind::Qualified)
    {
        return false;
    }
    // An `InstanceCreationExpression` always carries an argument list
    // (the invoke send's left neighbor).
    if pos == 0 || c.kind(run[pos - 1]) != NodeKind::Arguments {
        return false;
    }
    // The head name is the receiver segment's span head.
    if c.lexeme(range(run[head]).start) != class_name {
        return false;
    }
    let selector = run
        .get(pos + 1)
        .filter(|&&row| c.kind(row) == NodeKind::DotAccess)
        .map(|&marker| selector_lexeme(c, marker));
    selector_resolves(ctors, selector)
}

/// The structured face (`new A._()`, `A<int>.named(1)`, the
/// `ConstructorReference` callee of a written-`const` creation): the
/// callee identifiers tile inside the creation's own window — the
/// type first, the constructor last; the `Qualified` marker is the
/// `.` separator row between them (a real package prefix fails the
/// type-name check all the same, so it needs no bail of its own).
fn structured_creation(
    c: &Check<'_>,
    class_name: &str,
    ctors: &[Option<&str>],
    creation: u32,
) -> bool {
    let mut ids = Vec::new();
    for kid in c.collect_direct(creation) {
        match c.kind(kid) {
            NodeKind::Identifier | NodeKind::NewAsIdentifier => ids.push(kid),
            NodeKind::ConstructorReference => {
                for inner in c.collect_direct(kid) {
                    if matches!(
                        c.kind(inner),
                        NodeKind::Identifier | NodeKind::NewAsIdentifier
                    ) {
                        ids.push(inner);
                    }
                }
            }
            _ => {}
        }
    }
    let Some(first) = ids.first() else {
        return false;
    };
    if c.lexeme(c.head_token(*first)) != class_name {
        return false;
    }
    let selector = (ids.len() > 1).then(|| c.lexeme(c.head_token(ids[ids.len() - 1])));
    selector_resolves(ctors, selector)
}

/// The name an access marker spells (the dot rides the row's `Operator`
/// word; the name follows it — the `const_ability::selector_name` read).
fn selector_lexeme<'a>(c: &'a Check<'a>, marker: u32) -> &'a str {
    let dot = c.field(marker, PayloadField::Operator).unwrap_or(NONE_TOKEN);
    if dot == NONE_TOKEN {
        ""
    } else {
        c.lexeme(dot + 1)
    }
}

/// The top-level creation row of an initializer expression: the
/// creation-shaped rows carry the verdict; a `const` wrapper keeps its
/// content inside its own window — the send/`new` faces complete last
/// there (the `literal_faces` posture of `const_ability::rows_within`),
/// while a `const A()` creation spells a `ConstructorReference` callee
/// row (the written-const creation face, no send).
fn outer_creation(c: &Check<'_>, expr: u32) -> Option<u32> {
    match c.kind(expr) {
        NodeKind::Send | NodeKind::NewExpression | NodeKind::ImplicitCreation => Some(expr),
        NodeKind::ConstExpression | NodeKind::ConstLiteral => c
            .collect_direct(expr)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::ConstructorReference)
            .or_else(|| creation_rows_within(c, expr).last().copied()),
        _ => None,
    }
}

/// The creation-shaped rows nesting inside `row`'s token extent (its
/// completion window plus the parent's, span-filtered).
fn creation_rows_within(c: &Check<'_>, row: u32) -> Vec<u32> {
    let span = c.unit.ast.token_range(row);
    let mut out = Vec::new();
    let mut current = Some(row);
    for _ in 0..2 {
        let Some(node) = current else {
            break;
        };
        for candidate in c.unit.ast.children(node) {
            let other = c.unit.ast.token_range(candidate);
            if other.start >= span.start
                && other.end <= span.end
                && matches!(
                    c.kind(candidate),
                    NodeKind::Send | NodeKind::NewExpression | NodeKind::ImplicitCreation
                )
            {
                out.push(candidate);
            }
        }
        current = c.unit.ast.parent(node);
    }
    out
}

/// The `_EnumVisitor` face (:61-89): `true` — a generative invocation
/// of the class inside its own window outside the candidate
/// initializers. Only the candidate rows are exempt (the visitor's
/// `inConstantDeclaration` resets after the first creation, so a
/// nested invocation bails); a `const`-written candidate's wrapper
/// content *is* the candidate row (the outermost creation), and
/// default values inside the class bail through the same scan
/// (`test_no_lint_constructorUsedInConstructor`).
fn class_scan_bails(
    c: &Check<'_>,
    class_row: u32,
    class_name: &str,
    ctors: &[Option<&str>],
    candidates: &[u32],
) -> bool {
    for row in c.unit.ast.children(class_row) {
        if candidates.contains(&row)
            || !matches!(
                c.kind(row),
                NodeKind::Send
                    | NodeKind::NewExpression
                    | NodeKind::ImplicitCreation
                    | NodeKind::ConstExpression
                    | NodeKind::ConstLiteral
            )
        {
            continue;
        }
        if creation_invokes(c, class_name, ctors, row) {
            return true;
        }
    }
    false
}

/// The `_NonEnumVisitor` face (:100-135): `true` — no creation outside
/// the class invokes a generative constructor, and no other class-like
/// declaration uses the name as a supertype (extends, with or
/// implements; the mixin application's superclass `Type` rows join
/// through the same read).
fn outside_clean(
    c: &Check<'_>,
    class_row: u32,
    class_name: &str,
    ctors: &[Option<&str>],
) -> bool {
    let Some(root) = c.unit.ast.root() else {
        return true;
    };
    let span = c.unit.ast.token_range(class_row);
    for row in c.unit.ast.children(root) {
        match c.kind(row) {
            NodeKind::Send
            | NodeKind::NewExpression
            | NodeKind::ImplicitCreation
            | NodeKind::ConstExpression
            | NodeKind::ConstLiteral => {
                let other = c.unit.ast.token_range(row);
                if other.start >= span.start && other.end <= span.end {
                    continue; // the class domain is the other visitor's
                }
                if creation_invokes(c, class_name, ctors, row) {
                    return false;
                }
            }
            NodeKind::ClassDeclaration | NodeKind::NamedMixinApplication
                if row != class_row
                    && supertypes(c, row)
                        .into_iter()
                        .any(|(name, _)| name == class_name) =>
            {
                return false;
            }
            _ => {}
        }
    }
    true
}
