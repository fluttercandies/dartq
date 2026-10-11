//! The declaration-shape family: `avoid_return_types_on_setters`,
//! `unnecessary_late`, `use_function_type_syntax_for_parameters`,
//! `prefer_typing_uninitialized_variables`,
//! `prefer_generic_function_type_aliases`, `unnecessary_constructor_name`,
//! `hash_and_equals`, `avoid_init_to_null`,
//! `prefer_function_declarations_over_variables`, `type_init_formals`.

use dartq_ast::NodeKind;

use crate::engine::Check;
use crate::registry::{Group, RuleSpec};
use crate::rules::{NONE_TOKEN, first_child, member_name, slice, type_before};

pub(crate) mod avoid_return_types_on_setters {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/avoid_return_types_on_setters.dart:38-56`
    /// — a setter with an explicit return type reports at the type.
    /// Setters carry the `set` keyword as their `get_set` payload
    /// (collapse map §3.3 Method/TopLevelMethod rows). Message:
    /// `diagnostic.g.dart` `avoidReturnTypesOnSetters`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "avoid_return_types_on_setters",
        group: Group::Recommended,
        problem: "Unnecessary return type on a setter.",
        correction: "Try removing the return type.",
        kinds: &[NodeKind::Method, NodeKind::TopLevelMethod],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        if super::get_set_token(c, node) != Some("set") {
            return;
        }
        let Some(name) = super::member_name(c, node) else {
            return;
        };
        let Some((type_node, kind)) = super::type_before(c, node, name) else {
            return;
        };
        if kind == NodeKind::NoType {
            return;
        }
        let range = c.utf16_span(type_node);
        c.report(range, &[]);
    }
}

pub(crate) mod unnecessary_late {
    use super::{Check, Group, NodeKind, RuleSpec};
    use dartq_parser::MemberKind;

    /// Official: `pkg/linter/lib/src/rules/unnecessary_late.dart:36-55` —
    /// `late` on a static field or top-level variable where every variable
    /// of the declaration has an initializer reports at `late`. Instance
    /// fields are excluded (lazy initialization is meaningful there). The
    /// `late` keyword is the declaration's span head when present
    /// (collapse map §3.3 VariablesDeclaration row, `head_is`). Message:
    /// `diagnostic.g.dart` `unnecessaryLate`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "unnecessary_late",
        group: Group::Recommended,
        problem: "Unnecessary 'late' modifier.",
        correction: "Try removing the 'late'.",
        kinds: &[NodeKind::VariablesDeclaration],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // `late` ⟺ span head is the keyword (#174 elimination).
        let head = c.head_token(node);
        if c.lexeme(head) != "late" {
            return;
        }
        // Static fields and top-level variables only (:36-45).
        let Some(parent) = c.ancestor(1) else {
            return;
        };
        match c.kind(parent) {
            NodeKind::TopLevelFields => {}
            NodeKind::Fields => {
                // Fields payload word 0 is the `MemberKind` discriminant
                // (§3.3 FieldsEnd row): only the static kind reports.
                if c.payload(parent).first() != Some(&(MemberKind::StaticField as u32)) {
                    return;
                }
            }
            _ => return,
        }
        // Every variable must have an initializer (:51-54: any missing
        // initializer skips the report).
        for var in c.direct_children(node).iter().copied() {
            if c.kind(var) == NodeKind::InitializedIdentifier && !has_initializer(c, var) {
                return;
            }
        }
        let range = c.utf16_span(node);
        let start = range.start;
        c.report(start..start, &[]);
    }

    /// An `InitializedIdentifier` has an initializer iff a
    /// `VariableInitializer` child is present (`NoVariableInitializer`
    /// collapses to absence, collapse map §3.3 D1 rows).
    fn has_initializer(c: &Check<'_>, var: u32) -> bool {
        c.children(var)
            .any(|ch| c.kind(ch) == NodeKind::VariableInitializer)
    }
}

pub(crate) mod use_function_type_syntax_for_parameters {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/use_function_type_syntax_for_parameters.dart:34-49`
    /// — a formal parameter written with the old function syntax
    /// (`void f()`) reports at the parameter. The old syntax is its own AST
    /// shape: [`NodeKind::FunctionTypedFormalParameter`] (collapse map
    /// §3.3). Message: `diagnostic.g.dart`
    /// `useFunctionTypeSyntaxForParameters`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "use_function_type_syntax_for_parameters",
        group: Group::Recommended,
        problem: "Use the generic function type syntax to declare the parameter '{0}'.",
        correction: "Try using the generic function type syntax.",
        kinds: &[NodeKind::FunctionTypedFormalParameter],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // `node.name?.lexeme ?? ''` (:44).
        let name = super::member_name(c, node)
            .filter(|&n| c.kind(n) == NodeKind::Identifier)
            .map(|n| c.lexeme(c.head_token(n)))
            .unwrap_or("");
        let range = c.utf16_span(node);
        c.report(range, &[name]);
    }
}

pub(crate) mod prefer_typing_uninitialized_variables {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/prefer_typing_uninitialized_variables.dart:31-51`
    /// — an untyped declaration (`node.type == null`) with an uninitialized
    /// variable reports at the variable; fields and locals carry different
    /// message texts (`...ForField` / `...ForLocalVariable`,
    /// `diagnostic.g.dart:2839/2852`). The type is the child before the
    /// first variable (`NoType` = absent, collapse map §3.3). Core set
    /// (`.third/lints/lib/core.yaml`).
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "prefer_typing_uninitialized_variables",
        group: Group::Core,
        problem: "An uninitialized variable should have an explicit type annotation.",
        correction: "Try adding a type annotation.",
        kinds: &[NodeKind::VariablesDeclaration],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // `node.type != null` skips the whole declaration (:34).
        match super::first_child(c, node) {
            Some(first) if c.kind(first) == NodeKind::NoType => {}
            Some(_) => return,
            None => return,
        }
        let is_field = c.ancestor(1).is_some_and(|parent| {
            matches!(c.kind(parent), NodeKind::Fields | NodeKind::TopLevelFields)
        });
        // Gather untyped variables under the scratch borrow, then report
        // (the report path needs `&mut Check`); allocates only on hits.
        let mut hits: Vec<u32> = Vec::new();
        for var in c.direct_children(node).iter().copied() {
            if c.kind(var) != NodeKind::InitializedIdentifier {
                continue;
            }
            let initialized = c
                .children(var)
                .any(|ch| c.kind(ch) == NodeKind::VariableInitializer);
            if !initialized {
                hits.push(var);
            }
        }
        for var in hits {
            if is_field {
                c.report_texts(
                    c.utf16_span(var),
                    "An uninitialized field should have an explicit type annotation.",
                    "Try adding a type annotation.",
                    &[],
                );
            } else {
                let range = c.utf16_span(var);
                c.report(range, &[]);
            }
        }
    }
}

pub(crate) mod prefer_generic_function_type_aliases {
    use super::{Check, Group, NONE_TOKEN, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/prefer_generic_function_type_aliases.dart:44-64`
    /// — every old-form `typedef` (no `=`; the analyzer's `FunctionTypeAlias`,
    /// vs the `GenericTypeAlias` that carries `=`) reports at its name with the
    /// generic-function-type rewrite spelled in the message. The replacement
    /// is assembled from source slices in the official order
    /// (`returnType + ' ' + 'Function' + typeParameters + parameters`); the
    /// slices carry the written tail verbatim — a comment inside the tail
    /// (between tokens of a sliced part) rides along in the message, where
    /// the official `toSource()` re-render drops it (the golden pin holds
    /// this written-tail face; the report position is unaffected). A
    /// synthetic semicolon (recovery) skips (:48-49). Message:
    /// `diagnostic.g.dart` `preferGenericFunctionTypeAliases`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "prefer_generic_function_type_aliases",
        group: Group::Core,
        problem: "Use the generic function type syntax in 'typedef's.",
        correction: "Try using the generic function type syntax ('{0}').",
        kinds: &[NodeKind::Typedef],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // Old form ⟺ no `=` token (collapse map §3.2 Typedef row).
        if c.payload(node).get(1).copied() != Some(NONE_TOKEN) {
            return;
        }
        // `node.semicolon.isSynthetic` bails (:48-49).
        if c.token_is_synthetic(crate::rules::tail_token(c, node)) {
            return;
        }
        let kids = c.collect_direct(node);
        let mut return_type = None;
        let mut name = None;
        let mut type_variables = None;
        let mut parameters = None;
        for kid in kids {
            match c.kind(kid) {
                NodeKind::Type | NodeKind::NoType if return_type.is_none() => {
                    return_type = Some(kid);
                }
                NodeKind::Identifier if name.is_none() => name = Some(kid),
                NodeKind::TypeVariables => type_variables = Some(kid),
                NodeKind::FormalParameters => parameters = Some(kid),
                _ => {}
            }
        }
        let Some(name) = name else {
            return;
        };
        let Some(parameters) = parameters else {
            return;
        };
        let slice = |id: u32| -> &str {
            let bytes = c.byte_span(id);
            &c.unit.source[bytes]
        };
        // `'${returnTypeSource}Function$typeParameterSource$parameterSource'`
        // (:53-62) — `returnType == null` (our `NoType`) contributes nothing.
        let mut replacement = String::new();
        if let Some(rt) = return_type.filter(|rt| c.kind(*rt) != NodeKind::NoType) {
            replacement.push_str(slice(rt));
            replacement.push(' ');
        }
        replacement.push_str("Function");
        if let Some(tv) = type_variables {
            replacement.push_str(slice(tv));
        }
        replacement.push_str(slice(parameters));
        let name_token = c.head_token(name);
        let start = c
            .unit
            .tokens
            .get(name_token as usize)
            .expect("name token")
            .utf16_offset;
        c.report(start..start, &[&replacement]);
    }
}

pub(crate) mod unnecessary_constructor_name {
    use super::{Check, Group, NodeKind, RuleSpec, slice};

    /// Official: `pkg/linter/lib/src/rules/unnecessary_constructor_name.dart`
    /// — a written `.new` constructor name reports at the name token. Faces
    /// (registry :35-39): the constructor declaration (:41-57), the primary
    /// constructor declaration (:70-80), and the instance creation
    /// (:59-62). The element guards are approximated on the declaration
    /// slices: the duplicate bail counts every constructor whose written
    /// name is absent or `new` (the canonical unnamed name, :43-50) —
    /// non-constructor members never count — and the extension-type
    /// exemption (:51-58) bails on a secondary constructor whose primary
    /// constructor carries no name. The instance-creation face (:59-62)
    /// reads the owner slice (`rules::slice`): the `.new` member invocation
    /// over a bare receiver, `Foo.new(…)` — a constructor tear-off is no
    /// instance creation, and prefixed or chained receivers fail the
    /// bare-receiver group check (the documented conservative family).
    /// Message: `diagnostic.g.dart` `unnecessaryConstructorName`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "unnecessary_constructor_name",
        group: Group::Recommended,
        problem: "Unnecessary '.new' constructor name.",
        correction: "Try removing the '.new'.",
        kinds: &[
            NodeKind::Constructor,
            NodeKind::PrimaryConstructor,
            NodeKind::Send,
        ],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        match c.kind(node) {
            NodeKind::Constructor => check_declaration(c, node),
            NodeKind::PrimaryConstructor => check_primary(c, node),
            NodeKind::Send => check_creation(c),
            _ => {}
        }
    }

    /// The written constructor name: the identifier in the
    /// method-continuation context (the class-name echo carries the method
    /// context), or `None` when unnamed.
    fn constructor_name(c: &Check<'_>, node: u32) -> Option<u32> {
        let ctx = dartq_parser::IdentifierContext::MethodDeclarationContinuation as u32;
        c.direct_children(node).iter().copied().find(|&kid| {
            c.kind(kid) == NodeKind::Identifier && c.payload(kid).first() == Some(&ctx)
        })
    }

    /// The written primary-constructor name (the primary-constructor
    /// context identifier), or `None` when unnamed.
    fn primary_name(c: &Check<'_>, node: u32) -> Option<u32> {
        let ctx = dartq_parser::IdentifierContext::PrimaryConstructorDeclaration as u32;
        c.direct_children(node).iter().copied().find(|&kid| {
            c.kind(kid) == NodeKind::Identifier && c.payload(kid).first() == Some(&ctx)
        })
    }

    /// Whether more than one constructor of the type carries the canonical
    /// unnamed name (`constructors.where((c) => c.name == 'new')`, :43-50):
    /// every unnamed constructor and every `new`-spelled one counts, on the
    /// body's secondary constructors and the primary itself.
    fn duplicate_new(c: &Check<'_>, owner: u32, primary: Option<u32>) -> bool {
        let mut count = 0usize;
        let members: Vec<u32> = c.direct_children(owner).iter().copied().collect();
        for member in members {
            match c.kind(member) {
                NodeKind::Constructor => {
                    if is_unnamed_or_new(c, member) {
                        count += 1;
                    }
                }
                NodeKind::DeclarationBody => {
                    let ctors: Vec<u32> = c
                        .direct_children(member)
                        .iter()
                        .copied()
                        .filter(|&kid| c.kind(kid) == NodeKind::Constructor)
                        .collect();
                    count += ctors
                        .into_iter()
                        .filter(|&ctor| is_unnamed_or_new(c, ctor))
                        .count();
                }
                _ => {}
            }
        }
        if let Some(primary) = primary
            && primary_name(c, primary).is_none_or(|name| c.lexeme(c.head_token(name)) == "new")
        {
            count += 1;
        }
        count > 1
    }

    /// No written name (the unnamed form, canonically `new`) or spelled
    /// `new`.
    fn is_unnamed_or_new(c: &Check<'_>, ctor: u32) -> bool {
        match constructor_name(c, ctor) {
            None => true,
            Some(name) => c.lexeme(c.head_token(name)) == "new",
        }
    }

    /// The owner type's written primary constructor, if any.
    fn owner_primary(c: &Check<'_>) -> Option<u32> {
        let owner = c.ancestor(2)?;
        c.direct_children(owner)
            .iter()
            .copied()
            .find(|&kid| c.kind(kid) == NodeKind::PrimaryConstructor)
    }

    fn check_declaration(c: &mut Check<'_>, node: u32) {
        let Some(name) = constructor_name(c, node) else {
            return; // unnamed constructor: `node.name == null`
        };
        if c.lexeme(c.head_token(name)) != "new" {
            return;
        }
        // The extension-type exemption (:51-58): a secondary constructor
        // whose primary constructor is unnamed needs the qualified form.
        if let Some(owner) = c.ancestor(2)
            && c.kind(owner) == NodeKind::ExtensionTypeDeclaration
            && owner_primary(c).is_some_and(|p| primary_name(c, p).is_none())
        {
            return;
        }
        // Duplicate unnamed constructors already error (:45-52).
        let body = c.ancestor(1).expect("constructor has an owner");
        if duplicate_new(c, body, owner_primary(c)) {
            return;
        }
        let token = c.head_token(name);
        let start = c
            .unit
            .tokens
            .get(token as usize)
            .expect("name token")
            .utf16_offset;
        c.report(start..start, &[]);
    }

    /// `visitPrimaryConstructorDeclaration` (:70-80): a written `.new`
    /// primary-constructor name reports at the name token, unless the
    /// duplicate count fires.
    fn check_primary(c: &mut Check<'_>, node: u32) {
        let Some(name) = primary_name(c, node) else {
            return; // the unnamed primary: `constructorName == null`
        };
        if c.lexeme(c.head_token(name)) != "new" {
            return;
        }
        let owner = c.ancestor(1).expect("primary constructor has an owner");
        if duplicate_new(c, owner, Some(node)) {
            return;
        }
        let token = c.head_token(name);
        let start = c
            .unit
            .tokens
            .get(token as usize)
            .expect("name token")
            .utf16_offset;
        c.report(start..start, &[]);
    }

    fn check_creation(c: &mut Check<'_>) {
        // The production tiling keeps no children on the send: the `.new`
        // invocation reads the owner slice — argument list at `pos - 1`
        // (`None` is a constructor tear-off, not an instance creation),
        // the `.new` name head before that, and the bare-receiver tear-off
        // group behind the name.
        let Some((kids, pos)) = slice::owner_slice(c) else {
            return;
        };
        let Some((head, args)) = slice::send_parts(c, &kids, pos) else {
            return;
        };
        let Some(_) = args else {
            return;
        };
        let Some(group_start) = head.checked_sub(4) else {
            return;
        };
        if !slice::is_tear_off_group(c, &kids, group_start) {
            return;
        }
        if c.kind(kids[head]) != NodeKind::Identifier || c.lexeme(c.head_token(kids[head])) != "new"
        {
            return;
        }
        let token = c.head_token(kids[head]);
        let start = c
            .unit
            .tokens
            .get(token as usize)
            .expect("name token")
            .utf16_offset;
        c.report(start..start, &[]);
    }
}

pub(crate) mod hash_and_equals {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/hash_and_equals.dart:45-80` — a
    /// class defining exactly one of `==` / `hashCode` reports at the
    /// defined member's name. The member scan runs over the
    /// `DeclarationBody` children: `==` is a `Method` whose `OperatorName`
    /// payload token lexemes to `==`, `hashCode` a `Method` name or a field
    /// variable of that name. The element re-checks
    /// (`node.hasMethod('==')` etc.) collapse into the same scan — the blind
    /// spot is members the syntax shape misclassifies (none today). Message:
    /// `diagnostic.g.dart` `hashAndEquals`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "hash_and_equals",
        group: Group::Core,
        problem: "Missing a corresponding override of '{0}'.",
        correction: "Try overriding '{0}' or removing '{1}'.",
        kinds: &[NodeKind::ClassDeclaration],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let Some(body) = c
            .direct_children(node)
            .iter()
            .copied()
            .find(|&kid| c.kind(kid) == NodeKind::DeclarationBody)
        else {
            return;
        };
        let mut eq = None;
        let mut hash = None;
        // Collect first: the members scan re-enters `direct_children` for
        // each member, and the scratch RefCell forbids a live outer guard.
        let members: Vec<u32> = c.direct_children(body).iter().copied().collect();
        for member in members {
            if is_equals(c, member) {
                eq = Some(member);
            } else if is_hash_code(c, member) {
                hash = Some(member);
            }
        }
        let (eq, hash) = match (eq, hash) {
            (None, None) => return,
            pair => pair,
        };
        if eq.is_none() {
            let hash = hash.expect("one side is defined past the pair match");
            if let Some(name) = member_or_field_name(c, hash) {
                report_at(c, name, &["==", "hashCode"]);
            }
        }
        if hash.is_none() {
            let eq = eq.expect("one side is defined past the pair match");
            if let Some(name) = member_name_of(c, eq) {
                report_at(c, name, &["hashCode", "=="]);
            }
        }
    }

    /// `isEquals` (`ast.dart`): a `Method` whose operator name lexemes `==`.
    fn is_equals(c: &Check<'_>, member: u32) -> bool {
        c.kind(member) == NodeKind::Method
            && c.direct_children(member).iter().copied().any(|kid| {
                c.kind(kid) == NodeKind::OperatorName
                    && c.lexeme(c.payload(kid).first().copied().unwrap_or(u32::MAX)) == "=="
            })
    }

    /// `isHashCode` (`ast.dart`): a `Method` named `hashCode` or a `Fields`
    /// declaring a `hashCode` variable.
    fn is_hash_code(c: &Check<'_>, member: u32) -> bool {
        match c.kind(member) {
            NodeKind::Method => member_name_of(c, member).is_some_and(|name| {
                c.kind(name) == NodeKind::Identifier && c.lexeme(c.head_token(name)) == "hashCode"
            }),
            NodeKind::Fields => c.children(member).any(|descendant| {
                c.kind(descendant) == NodeKind::InitializedIdentifier
                    && c.lexeme(c.head_token(descendant)) == "hashCode"
            }),
            _ => false,
        }
    }

    /// The report position: a `Method` reports at its name, a `Fields` at
    /// its `hashCode` variable (the official `getFieldName` fallback).
    fn member_or_field_name(c: &Check<'_>, member: u32) -> Option<u32> {
        match c.kind(member) {
            NodeKind::Method => member_name_of(c, member),
            NodeKind::Fields => c.children(member).find(|&descendant| {
                c.kind(descendant) == NodeKind::InitializedIdentifier
                    && c.lexeme(c.head_token(descendant)) == "hashCode"
            }),
            _ => None,
        }
    }

    /// The report position token: an `OperatorName` names through its
    /// payload token (the `==` itself — `head_token` would hit the
    /// `operator` keyword), an `Identifier`/variable through its span head.
    fn report_at(c: &mut Check<'_>, name: u32, args: &[&str]) {
        let token = if c.kind(name) == NodeKind::OperatorName {
            c.payload(name)
                .first()
                .copied()
                .expect("operator name payload")
        } else {
            c.head_token(name)
        };
        let start = c
            .unit
            .tokens
            .get(token as usize)
            .expect("name token")
            .utf16_offset;
        c.report(start..start, args);
    }

    /// The name child of a `Method` (`Identifier` or `OperatorName`).
    fn member_name_of(c: &Check<'_>, member: u32) -> Option<u32> {
        c.direct_children(member)
            .iter()
            .copied()
            .find(|&kid| matches!(c.kind(kid), NodeKind::Identifier | NodeKind::OperatorName))
    }
}

pub(crate) mod avoid_init_to_null {
    use super::{Check, Group, NodeKind, RuleSpec};
    use crate::rules::{NONE_TOKEN, first_child};

    /// Official: `pkg/linter/lib/src/rules/avoid_init_to_null.dart:59-88` —
    /// a variable whose initializer is the `null` literal reports at the
    /// declaration; const/final declarations are exempt (`!node.isConst &&
    /// !node.isFinal`, :62-63), as are the formal-parameter default-value
    /// faces (:74-87).
    ///
    /// Approximation (documented): the official additionally requires the
    /// declared type to be nullable (`typeSystem.isNullable`,
    /// `avoid_init_to_null.dart:41/:65`) — a resolution question; the lexical
    /// face reports regardless, so `int x = null;` (a compile error upstream)
    /// over-reports. The super-formal sub-check
    /// (`superConstructorParameter.defaultValueCode`, :78-84) is likewise
    /// resolution-bound and treated like the regular form. The keyword gate
    /// reads the declaration's `varFinalOrConst` token (collapse map §3.3
    /// `VariablesDeclaration` `modifier` word; the parameter's init range is
    /// its `init_start`/`init_end` words). Message: `diagnostic.g.dart`
    /// `avoidInitToNull`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "avoid_init_to_null",
        group: Group::Recommended,
        problem: "Redundant initialization to 'null'.",
        correction: "Try removing the initializer.",
        kinds: &[NodeKind::InitializedIdentifier, NodeKind::FormalParameter],
        check,
    };

    fn check(c: &mut Check<'_>) {
        match c.kind(c.node()) {
            NodeKind::InitializedIdentifier => check_variable(c),
            NodeKind::FormalParameter => check_parameter(c),
            _ => {}
        }
    }

    fn check_variable(c: &mut Check<'_>) {
        // The initializer is the `VariableInitializer` child; its operand is
        // the null literal.
        let initializer = c
            .collect_direct(c.node())
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::VariableInitializer);
        let Some(initializer) = initializer else {
            return;
        };
        let Some(value) = first_child(c, initializer) else {
            return;
        };
        if c.kind(value) != NodeKind::LiteralNull {
            return;
        }
        // `!node.isConst && !node.isFinal` (:62-63): the declaration
        // keyword is the VariablesDeclaration `modifier` word (collapse map
        // §3.3); a written type (no keyword) initializes the same way.
        if let Some(parent) = c.ancestor(1)
            && c.kind(parent) == NodeKind::VariablesDeclaration
            && let Some(&keyword) = c.payload(parent).first()
            && keyword != NONE_TOKEN
            && matches!(c.lexeme(keyword), "final" | "const")
        {
            return;
        }
        let range = c.utf16_span(c.node());
        c.report(range, &[]);
    }

    fn check_parameter(c: &mut Check<'_>) {
        // `{int x = null}` — the default value is delimited by the
        // `init_start`/`init_end` payload words (collapse map §3.3/§3.9);
        // exactly the `null` token qualifies under either end convention.
        let payload = c.payload(c.node());
        let Some(&start) = payload.get(2) else {
            return;
        };
        if start == NONE_TOKEN {
            return;
        }
        let end = payload.get(3).copied().unwrap_or(start);
        if c.lexeme(start) == "null" && end.saturating_sub(start) <= 1 {
            let range = c.utf16_span(c.node());
            c.report(range, &[]);
        }
    }
}

pub(crate) mod prefer_function_declarations_over_variables {
    use super::{Check, Group, NodeKind, RuleSpec, member_name};
    use crate::rules::{NONE_TOKEN, last_child};

    /// Official: `pkg/linter/lib/src/rules/prefer_function_declarations_over_variables.dart:40-58`
    /// — a variable initialized with a function literal reports. Fields and
    /// top-level variables only when final (:42-49); locals unless the
    /// variable is potentially mutated in its scope (:50-55).
    ///
    /// The mutation face (`isPotentiallyMutatedInScope`, :53) is the
    /// in-rule write scan: any unqualified write to the name after the
    /// declaration, anywhere in the enclosing function body (nested
    /// closures included) — an assignment target (`=`, `??=`, compound) —
    /// keeps the variable a variable and passes. Loop-part rebinding
    /// (`for (f in ...)`) is a declaration, not a write, and never
    /// suppresses, mirroring the official's declaration face. Residual
    /// (documented): a write that element resolution would bind to a
    /// shadowing redeclaration between the write and this declaration
    /// still counts (the erroneous / user-shadowed family under-reports);
    /// `++`/`--` operands and pattern-assignment writes (`(f, _) = ...`)
    /// are not scanned, so a variable mutated only through those
    /// over-reports. Message:
    /// `diagnostic.g.dart` `preferFunctionDeclarationsOverVariables`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "prefer_function_declarations_over_variables",
        group: Group::Recommended,
        problem: "Use a function declaration rather than a variable assignment to bind a function to a name.",
        correction: "Try rewriting the closure assignment as a function declaration.",
        kinds: &[NodeKind::InitializedIdentifier],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let Some(initializer) = c
            .collect_direct(c.node())
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::VariableInitializer)
        else {
            return;
        };
        // The value expression is the initializer's LAST direct child (it
        // completes last); a lambda's absent-return marker
        // (`NoTypeVariables`) may precede the `FunctionExpression` inside
        // the completion window, so the first-child read is not the value.
        let Some(value) = last_child(c, initializer) else {
            return;
        };
        if c.kind(value) != NodeKind::FunctionExpression {
            return;
        }
        // No enclosing function body ⟹ field/top-level: final only (:42-49).
        let Some(declaration) = c.ancestor(1) else {
            return;
        };
        if c.kind(declaration) != NodeKind::VariablesDeclaration {
            return;
        }
        let is_member = c.ancestor(2).is_some_and(|owner| {
            matches!(c.kind(owner), NodeKind::Fields | NodeKind::TopLevelFields)
        });
        if is_member {
            let keyword = c
                .payload(declaration)
                .first()
                .copied()
                .unwrap_or(NONE_TOKEN);
            if keyword == NONE_TOKEN || c.lexeme(keyword) != "final" {
                return;
            }
        } else if is_potentially_mutated(c) {
            // `isPotentiallyMutatedInScope` (:53) — a later write to the
            // name in the enclosing function body keeps it a variable.
            return;
        }
        let range = c.utf16_span(c.node());
        c.report(range, &[]);
    }

    /// Whether any unqualified write to this declaration's name sits at or
    /// after the declaration inside the enclosing function body (the
    /// nearest `BlockBody` ancestor — the official's `FunctionBody` scope).
    fn is_potentially_mutated(c: &Check<'_>) -> bool {
        let Some(name) = member_name(c, c.node()) else {
            return false;
        };
        let name = c.lexeme(c.head_token(name));
        let after = c.byte_span(c.node()).end;
        let Some(body) = c
            .path
            .iter()
            .rev()
            .copied()
            .find(|&ancestor| c.kind(ancestor) == NodeKind::BlockBody)
        else {
            return false;
        };
        is_mutated_after(c, body, name, after)
    }

    /// The write scan over `root`'s subtree: an assignment whose target is
    /// the bare name, positioned at or after `after` (byte offset — the
    /// declaration's own initializer is before it). Subtrees that end
    /// before `after` prune.
    fn is_mutated_after(c: &Check<'_>, root: u32, name: &str, after: usize) -> bool {
        for kid in c.collect_direct(root) {
            if c.byte_span(kid).end <= after {
                continue;
            }
            if c.byte_span(kid).start >= after && writes_name(c, kid, name) {
                return true;
            }
            if is_mutated_after(c, kid, name, after) {
                return true;
            }
        }
        false
    }

    /// The write faces: an assignment whose first operand is the bare name
    /// (the op payload word may be any of `=` / `??=` / compound).
    fn writes_name(c: &Check<'_>, node: u32, name: &str) -> bool {
        c.kind(node) == NodeKind::Assignment
            && c.collect_direct(node).first().is_some_and(|&lhs| {
                c.kind(lhs) == NodeKind::Identifier && c.lexeme(c.head_token(lhs)) == name
            })
    }
}

pub(crate) mod type_init_formals {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/type_init_formals.dart:37-64` — an
    /// initializing formal (`this.x` / `super.x`) written with a type
    /// annotation reports at the type (:37-48/:52-64).
    ///
    /// Approximation (documented): the official re-checks through the
    /// element model — the field/super-parameter must exist and its type
    /// must equal the written one (type_init_formals.dart:41-46/:56-62), so
    /// only valid declarations lint. The lexical face reports any typed
    /// receiver formal: a nonexistent field or a type mismatch (both compile
    /// errors upstream) over-reports. `this.x`/`super.x` is the
    /// `FormalParameter` `receiver` payload word (collapse map §3.3). Message:
    /// `diagnostic.g.dart` `typeInitFormals`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "type_init_formals",
        group: Group::Recommended,
        problem: "Don't needlessly type annotate initializing formals.",
        correction: "Try removing the type.",
        kinds: &[NodeKind::FormalParameter],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // `nodeType == null` bails (:38-39); `this.`/`super.` makes the
        // parameter an initializing formal by grammar.
        if c.payload(node).first().copied() == Some(crate::rules::NONE_TOKEN) {
            return;
        }
        // The written type: the Type/FunctionType/RecordType child (a
        // `NoType` child means unwritten, collapse map §3.8).
        let Some(written) = c.collect_direct(node).into_iter().find(|&kid| {
            matches!(
                c.kind(kid),
                NodeKind::Type | NodeKind::FunctionType | NodeKind::RecordType
            )
        }) else {
            return;
        };
        let range = c.utf16_span(written);
        c.report(range, &[]);
    }
}

pub(crate) mod recursive_getters {
    use super::{Check, Group, NodeKind, RuleSpec, get_set_token, path_shadowed};

    /// Official: `pkg/linter/lib/src/rules/recursive_getters.dart:31-95` — a
    /// getter (no parameter list) whose body references itself reports at
    /// the reference. The element identity (`node.element != element`,
    /// :57-60) is the in-rule shadow walk (`path_shadowed`): a
    /// same-lexeme read captured between the read and the getter — by a
    /// local (`var x`), a parameter or catch parameter, a for-loop
    /// variable, a switch-case pattern variable, or a nested function of
    /// that name — resolves to that binder, not the getter, and passes.
    /// Residual (documented): pattern variables bound by an if-case
    /// condition (`if (v case var x)`) are not walked, so a read inside the
    /// then-branch of that name over-reports; the const-literal subtree
    /// skip (:61-70) covers erroneous code only and is dropped. Message:
    /// `diagnostic.g.dart` `recursiveGetters`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "recursive_getters",
        group: Group::Recommended,
        problem: "The getter '{0}' recursively returns itself.",
        correction: "Try changing the value being returned.",
        kinds: &[NodeKind::Identifier],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // Reads only (the official flags `SimpleIdentifier` references; a
        // declaration or label position never reads the getter).
        let Some(&ctx) = c.payload(node).first() else {
            return;
        };
        if ctx != dartq_parser::IdentifierContext::Expression as u32 {
            return;
        }
        // `isSelfReference` parent guards (:57-64): a `prefixed`/`obj.x`
        // access never self-references; `this.x` does.
        if let Some(parent) = c.ancestor(1) {
            match c.kind(parent) {
                NodeKind::DotAccess => {
                    let kids = c.collect_direct(parent);
                    let name_side = kids.last() == Some(&node);
                    let target_this = kids
                        .first()
                        .is_some_and(|&target| c.kind(target) == NodeKind::ThisExpression);
                    if !(name_side && target_this) {
                        return;
                    }
                }
                NodeKind::Qualified => return,
                _ => {}
            }
        }
        // The nearest enclosing getter with no parameter list
        // (:78-85: `parameters != null` bails).
        let Some(getter_index) = c.path.iter().rposition(|&ancestor| {
            matches!(
                c.kind(ancestor),
                NodeKind::Method | NodeKind::TopLevelMethod
            )
        }) else {
            return;
        };
        let getter = c.path[getter_index];
        if get_set_token(c, getter) != Some("get") {
            return;
        }
        // No parameters: no FormalParameters child (a getter's grammar has
        // none; the official's `node.parameters != null`).
        if c.children(getter)
            .any(|kid| c.kind(kid) == NodeKind::FormalParameters)
        {
            return;
        }
        // The reference must sit in the getter's body (:86-89 walk the body).
        let body = c
            .collect_direct(getter)
            .into_iter()
            .find(|&kid| matches!(c.kind(kid), NodeKind::ExpressionBody | NodeKind::BlockBody));
        let Some(body) = body else {
            return;
        };
        let name = c.lexeme(c.head_token(node));
        // The getter's own name, through the name child (the span head can
        // be the return type / `get` keyword).
        let Some(getter_name) = super::member_name(c, getter) else {
            return;
        };
        if c.lexeme(c.head_token(getter_name)) != name {
            return;
        }
        let body_span = c.byte_span(body);
        if !body_span.contains(&c.byte_span(node).start) {
            return;
        }
        // The shadow walk — the `isSelfReference` element identity (:57-60):
        // a same-lexeme binder between the read and the getter captures it.
        if path_shadowed(c, getter_index, name) {
            return;
        }
        let range = c.utf16_span(node);
        c.report(range, &[name]);
    }
}

pub(crate) mod unnecessary_getters_setters {
    use super::{Check, Group, NodeKind, RuleSpec, get_set_token, parameter_name, parameters_bind};
    use crate::rules::first_child;

    /// Official: `pkg/linter/lib/src/rules/unnecessary_getters_setters.dart:41-97`
    /// — a getter/setter pair of the same name whose bodies are the trivial
    /// field wrap (getter reads a private same-class field, setter assigns
    /// it from the lone parameter) reports at the getter name; neither
    /// member may carry annotations.
    ///
    /// Approximation (documented): the field identity (`staticElement
    /// isGetterElement` + same enclosing class, ast.dart
    /// `_checkForSimpleGetter`) is taken as a same-lexeme private field
    /// declared in the same class body, with the shared shadow walk: the
    /// setter's assignment target resolves through its body scope, so a
    /// same-lexeme parameter captures it and the pair passes (the
    /// `_checkForSimpleSetter` `writeElement` face — a parameter write is
    /// not the field's implicit setter). The trivial-body shapes cannot
    /// carry a same-named local (the sole-statement extraction), so no
    /// further shadow face exists there. The setter's type-agreement guard
    /// (`expression.writeType != rightHandSide.staticType`, ast.dart) is
    /// resolution-bound and stays dropped: a wrap whose written types
    /// disagree over-reports. Message: `diagnostic.g.dart`
    /// `unnecessaryGettersSetters`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "unnecessary_getters_setters",
        group: Group::Recommended,
        problem: "Unnecessary use of getter and setter to wrap a field.",
        correction: "Try removing the getter and setter and renaming the field.",
        kinds: &[
            NodeKind::ClassDeclaration,
            NodeKind::ExtensionTypeDeclaration,
        ],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // `node.body case BlockClassBody body` (:55-58) — the member list.
        let Some(body) = c
            .collect_direct(node)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::DeclarationBody)
        else {
            return;
        };
        let members = c.collect_direct(body);
        // Build the getter/setter maps (:62-72) and the private field names.
        let mut fields: Vec<&str> = Vec::new();
        for &member in &members {
            if c.kind(member) == NodeKind::Fields {
                collect_field_names(c, member, &mut fields);
            }
        }
        for (i, &member) in members.iter().enumerate() {
            if c.kind(member) != NodeKind::Method || get_set_token(c, member) != Some("get") {
                continue;
            }
            // Names come from the name children (the span head may be the
            // return type / `get` keyword).
            let Some(name_node) = super::member_name(c, member) else {
                continue;
            };
            let getter_name = c.lexeme(c.head_token(name_node));
            let Some(setter) = members.iter().copied().find(|&other| {
                c.kind(other) == NodeKind::Method
                    && get_set_token(c, other) == Some("set")
                    && super::member_name(c, other)
                        .is_some_and(|name| c.lexeme(c.head_token(name)) == getter_name)
            }) else {
                continue;
            };
            // `metadata.annotations.isEmpty` on both (:96-99) — the star
            // sibling before the member.
            if has_annotations(c, &members, i) || has_annotations_of(c, &members, setter) {
                continue;
            }
            if !is_simple_getter(c, member, &fields) || !is_simple_setter(c, setter, &fields) {
                continue;
            }
            // The getter's name reports (:101).
            let start = c
                .unit
                .tokens
                .get(c.head_token(name_node) as usize)
                .expect("name outside the token table")
                .utf16_offset;
            c.report(start..start, &[]);
        }
    }

    /// The private field names declared by a `Fields` member.
    fn collect_field_names<'a>(c: &Check<'a>, fields: u32, out: &mut Vec<&'a str>) {
        for decl in c.collect_direct(fields) {
            if c.kind(decl) != NodeKind::VariablesDeclaration {
                continue;
            }
            for var in c.collect_direct(decl) {
                if c.kind(var) != NodeKind::InitializedIdentifier {
                    continue;
                }
                if let Some(name) = crate::rules::member_name(c, var) {
                    let lexeme = c.lexeme(c.head_token(name));
                    if lexeme.starts_with('_') {
                        out.push(lexeme);
                    }
                }
            }
        }
    }

    /// Whether the member at `index`'s left sibling is a metadata star.
    fn has_annotations(c: &Check<'_>, members: &[u32], index: usize) -> bool {
        index > 0 && c.kind(members[index - 1]) == NodeKind::MetadataStar
    }

    fn has_annotations_of(c: &Check<'_>, members: &[u32], member: u32) -> bool {
        members
            .iter()
            .position(|&m| m == member)
            .is_some_and(|index| has_annotations(c, members, index))
    }

    /// The expression a trivial body yields: an expression body's expression,
    /// or the sole statement's child (`statement_kind` = the required single
    /// statement).
    fn sole_body_expression(c: &Check<'_>, kids: &[u32], statement_kind: NodeKind) -> Option<u32> {
        if let Some(&body) = kids
            .iter()
            .find(|&&kid| c.kind(kid) == NodeKind::ExpressionBody)
        {
            return first_child(c, body);
        }
        let body = kids
            .iter()
            .find(|&&kid| c.kind(kid) == NodeKind::BlockBody)?;
        // The statements live under the body's `Block` child.
        let block = c
            .collect_direct(*body)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::Block)?;
        let statements = c.collect_direct(block);
        (statements.len() == 1 && c.kind(statements[0]) == statement_kind)
            .then(|| first_child(c, statements[0]))
            .flatten()
    }

    /// `isSimpleGetter` (ast.dart): the body expression (or a single
    /// return's expression) is a bare private same-class field identifier.
    fn is_simple_getter(c: &Check<'_>, getter: u32, fields: &[&str]) -> bool {
        let kids = c.collect_direct(getter);
        let Some(expression) = sole_body_expression(c, &kids, NodeKind::ReturnStatement) else {
            return false;
        };
        simple_field_read(c, expression, fields)
    }

    /// `isSimpleSetter` (ast.dart): the body statement assigns a bare
    /// private same-class field from the lone parameter.
    fn is_simple_setter(c: &Check<'_>, setter: u32, fields: &[&str]) -> bool {
        let kids = c.collect_direct(setter);
        let Some(expression) = sole_body_expression(c, &kids, NodeKind::ExpressionStatement) else {
            return false;
        };
        if c.kind(expression) != NodeKind::Assignment
            || c.lexeme(
                c.payload(expression)
                    .first()
                    .copied()
                    .expect("assignment op"),
            ) != "="
        {
            return false;
        }
        let operands = c.collect_direct(expression);
        if operands.len() < 2 {
            return false;
        }
        let (lhs, rhs) = (operands[0], operands[1]);
        // `rightElement == parameters.first` (:92-96 in ast.dart); the same
        // list is the body scope of the target-shadow face below.
        let Some(params) = kids
            .iter()
            .copied()
            .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
        else {
            return false;
        };
        let param_list = c.collect_direct(params);
        if param_list.len() != 1 {
            return false;
        }
        let Some(param_name) = parameter_name(c, param_list[0]) else {
            return false;
        };
        // The assignment target resolves through the body scope: a
        // parameter of the same name captures it (the `writeElement` face —
        // a parameter write is not the field's implicit setter).
        let target_shadowed = c.kind(lhs) == NodeKind::Identifier
            && parameters_bind(c, params, c.lexeme(c.head_token(lhs)));
        !target_shadowed
            && simple_field_read(c, lhs, fields)
            && c.kind(rhs) == NodeKind::Identifier
            && c.lexeme(c.head_token(rhs)) == param_name
    }

    /// A bare identifier naming a private field of this class.
    fn simple_field_read(c: &Check<'_>, expression: u32, fields: &[&str]) -> bool {
        c.kind(expression) == NodeKind::Identifier && {
            let lexeme = c.lexeme(c.head_token(expression));
            lexeme.starts_with('_') && fields.contains(&lexeme)
        }
    }
}

pub(crate) mod unnecessary_nullable_for_final_variable_declarations {
    use super::{Check, Group, NodeKind, RuleSpec};
    use crate::rules::first_child;
    use dartq_parser::MemberKind;

    /// Official: `pkg/linter/lib/src/rules/unnecessary_nullable_for_final_variable_declarations.dart:35-117`
    /// — a `final`/`const` declaration with a nullable written type whose
    /// initializer is non-nullable reports at the variable name.
    ///
    /// Approximation (documented): the initializer face
    /// (`initializerType.isNonNullable`) is taken as a non-null literal —
    /// numbers, strings, booleans, collection literals, const expressions —
    /// so `final int? x = compute()` (a non-null non-literal) under-reports
    /// until type inference lands (the `no_duplicate_case_values`
    /// constant-evaluation precedent). Fields keep the official
    /// private-or-static gate (:83-88). Message: `diagnostic.g.dart`
    /// `unnecessaryNullableForFinalVariableDeclarations`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "unnecessary_nullable_for_final_variable_declarations",
        group: Group::Recommended,
        problem: "Type could be non-nullable.",
        correction: "Try changing the type to be non-nullable.",
        kinds: &[NodeKind::VariablesDeclaration],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // `isFinal || isConst` (:97): the head token, or the token after a
        // `late` head.
        let head = c.head_token(node);
        let keyword = match c.lexeme(head) {
            "final" | "const" => head,
            "late" => head + 1,
            _ => return,
        };
        if !matches!(c.lexeme(keyword), "final" | "const") {
            return;
        }
        // `declaredElement.type is DynamicType` bails (:101-103) — an
        // unwritten type is the `NoType` child.
        let Some(type_node) = first_child(c, node) else {
            return;
        };
        if c.kind(type_node) != NodeKind::Type {
            return;
        }
        // The written `?` is the type's last span token (collapse map §3.8
        // Type row, #174).
        let type_range = c.unit.ast.token_range(type_node);
        if type_range.is_empty() || c.lexeme(type_range.end - 1) != "?" {
            return;
        }
        // Fields: private or static only (:83-88).
        let is_field = c
            .ancestor(1)
            .is_some_and(|parent| c.kind(parent) == NodeKind::Fields);
        if is_field {
            let parent = c.ancestor(1).expect("checked above");
            let is_static = c.payload(parent).first() == Some(&(MemberKind::StaticField as u32));
            let is_private = c
                .collect_direct(node)
                .iter()
                .any(|&var| c.lexeme(c.head_token(var)).starts_with('_'));
            if !is_static && !is_private {
                return;
            }
        }
        for var in c.collect_direct(node) {
            if c.kind(var) != NodeKind::InitializedIdentifier {
                continue;
            }
            // `variable.initializer?.staticType` (:105) — an uninitialized
            // variable bails per-variable.
            let Some(initializer) = c
                .collect_direct(var)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::VariableInitializer)
                .and_then(|init| first_child(c, init))
            else {
                continue;
            };
            if !is_non_null_literal(c, initializer) {
                continue;
            }
            // `rule.reportAtToken(variable.name)` (:110).
            let Some(name_node) = crate::rules::member_name(c, var) else {
                continue;
            };
            let start = c
                .unit
                .tokens
                .get(c.head_token(name_node) as usize)
                .expect("name outside the token table")
                .utf16_offset;
            c.report(start..start, &[]);
        }
    }

    /// The non-null literal face: any literal form but `null`.
    fn is_non_null_literal(c: &Check<'_>, expression: u32) -> bool {
        matches!(
            c.kind(expression),
            NodeKind::LiteralInt
                | NodeKind::LiteralDouble
                | NodeKind::LiteralBool
                | NodeKind::StringLiteral
                | NodeKind::ListLiteral
                | NodeKind::SetOrMapLiteral
                | NodeKind::RecordLiteral
                | NodeKind::ConstExpression
                | NodeKind::ConstLiteral
        )
    }
}

pub(crate) mod prefer_initializing_formals {
    use super::{Check, Group, NodeKind, RuleSpec};
    use crate::rules::{NONE_TOKEN, token_seq_equal};

    /// Official: `pkg/linter/lib/src/rules/prefer_initializing_formals.dart`
    /// — a constructor initializer (`: x = p`) or a constructor-body
    /// assignment (`this.x = p;`) that assigns a constructor parameter to
    /// the same-named instance field of this class, with no other reference
    /// to the parameter, reports at the initializer (:66-159). The
    /// conversion must be safe: the field must be an actual (non-static,
    /// non-setter) field of this class, and the parameter type must be a
    /// subtype of the field type (:114-118).
    ///
    /// Approximations (documented): the field identity (`FieldElement` +
    /// `enclosingElement == constructor's class`, :119-124) is taken as a
    /// non-static field declared in the same class body (a constructor and
    /// its class always share a file); the subtype check
    /// (`typeSystem.isSubtypeOf`, :118) is approximated by written-type
    /// lexeme equality — differing but subtype-related spellings
    /// (`int` param for a `num` field) under-report, and equal spellings
    /// are subtype-equal. The single-reference count
    /// (`_ReferenceCounter`, :208-222) counts expression-position
    /// identifiers with the parameter's lexeme (dot-access name sides
    /// excluded — they name members), so a same-lexeme local declaration
    /// in the body over-counts and skips (conservative). Private fields
    /// skip — the `private_named_parameters` face is feature-gated off.
    /// Primary constructors are outside the substrate. Message:
    /// `diagnostic.g.dart` `preferInitializingFormals`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "prefer_initializing_formals",
        group: Group::Recommended,
        problem: "Use an initializing formal to assign a parameter to a field.",
        correction: "Try using an initializing formal ('this.{0}') to initialize the field.",
        kinds: &[NodeKind::Constructor],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let kids = c.collect_direct(node);
        let Some(params_node) = kids
            .iter()
            .copied()
            .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
        else {
            return;
        };
        let params = constructor_params(c, params_node);
        // The instance fields of this class (same file by construction).
        let fields = class_fields(c);
        let initializers = kids
            .iter()
            .copied()
            .find(|&kid| c.kind(kid) == NodeKind::Initializers);
        // Face A: initializer-list entries `x = p` / `this.x = p` — the
        // initializer expression is the bare parameter.
        let mut candidates: Vec<(&str, &str, u32, u32)> = Vec::new();
        if let Some(initializers) = initializers {
            face_initializers(c, initializers, &mut candidates);
        }
        // Face B: body statements of the exact form `this.x = p;`. The
        // reference roots carry whether initializer entries' field-name
        // sides are excluded from the count (the initializer face).
        let mut ref_roots: Vec<(u32, bool)> = Vec::new();
        if let Some(initializers) = initializers {
            ref_roots.push((initializers, true));
        }
        if let Some(body) = kids
            .iter()
            .copied()
            .find(|&kid| matches!(c.kind(kid), NodeKind::Block | NodeKind::BlockBody))
        {
            ref_roots.push((body, false));
            face_body(c, body, &mut candidates);
        }
        finish(c, &params, &fields, &ref_roots, &candidates);
    }

    /// The constructor's convertible parameters: `(name, written type)` for
    /// plain formals only — receiver formals (`this.`/`super.`) are already
    /// initializing/super formals, function-typed formals have no comparable
    /// written type.
    fn constructor_params<'a>(c: &Check<'a>, params_node: u32) -> Vec<(&'a str, Option<u32>)> {
        let mut params: Vec<(&str, Option<u32>)> = Vec::new();
        for param in c.collect_direct(params_node) {
            if c.kind(param) != NodeKind::FormalParameter {
                continue;
            }
            let payload = c.payload(param);
            if payload.first().copied().unwrap_or(NONE_TOKEN) != NONE_TOKEN {
                continue; // this.x / super.x formal
            }
            let Some(&name) = payload.get(1) else {
                continue;
            };
            if name == NONE_TOKEN {
                continue;
            }
            let written = c.collect_direct(param).into_iter().find(|&kid| {
                matches!(
                    c.kind(kid),
                    NodeKind::Type | NodeKind::FunctionType | NodeKind::RecordType
                )
            });
            params.push((c.lexeme(name), written));
        }
        params
    }

    /// The non-static instance fields of the owner body: `(name, written
    /// type)` shared per declaration.
    fn class_fields<'a>(c: &Check<'a>) -> Vec<(&'a str, Option<u32>)> {
        let mut fields: Vec<(&str, Option<u32>)> = Vec::new();
        // Member discovery anchors on the class row's subtree: a subtree
        // occupies the contiguous row range from its first child slot to
        // its root (the class row completes last), so member rows sit in
        // the range under every tiling layout — the body container's
        // window can open later and the derived parent pointer can link a
        // field row to either neighbor.
        let Some(&class_row) = c
            .path
            .iter()
            .rev()
            .find(|&&a| c.kind(a) == NodeKind::ClassDeclaration)
        else {
            return fields;
        };
        for member in c.children(class_row).start..=class_row {
            if c.kind(member) != NodeKind::Fields
                || c.payload(member).first()
                    != Some(&(dartq_parser::MemberKind::NonStaticField as u32))
            {
                continue;
            }
            // Production tiling: the written type tiles as a `Fields`-row
            // sibling before the declaration (the declaration row opens at
            // the name); hand-built tables nest the `Type` under it.
            let group_type = c
                .collect_direct(member)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::Type);
            for decl in c.collect_direct(member) {
                if c.kind(decl) != NodeKind::VariablesDeclaration {
                    continue;
                }
                let written = group_type.or_else(|| {
                    c.collect_direct(decl)
                        .into_iter()
                        .find(|&kid| c.kind(kid) == NodeKind::Type)
                });
                for var in c.collect_direct(decl) {
                    if c.kind(var) != NodeKind::InitializedIdentifier {
                        continue;
                    }
                    // The production row is width-0 at the name, so the
                    // name is the span head (see
                    // [`crate::rules::initialized_identifier_name`]).
                    if let Some(name_token) = crate::rules::initialized_identifier_name(c, var) {
                        fields.push((c.lexeme(name_token), written));
                    }
                }
            }
        }
        fields
    }

    /// Face A (:71-85): each initializer entry whose expression is the
    /// convertible assignment `x = p` / `this.x = p` pushes a candidate.
    /// Production shape: the `Initializer` container completes last in its
    /// window and its entry expression is an `Assignment` whose operand
    /// pieces tile in the entry window (the parser re-reads the entry
    /// through the expression engine; the `FieldInitializer` node is never
    /// emitted on a production parse). The candidate carries the report
    /// range — the entry window head through the assignment's end: the
    /// assignment row's own span starts at the flattened operand and drops
    /// the `this.` prefix.
    fn face_initializers<'a>(
        c: &Check<'a>,
        initializers: u32,
        out: &mut Vec<(&'a str, &'a str, u32, u32)>,
    ) {
        for initializer in c.collect_direct(initializers) {
            if c.kind(initializer) != NodeKind::Initializer {
                continue;
            }
            // An `assert(...)` entry or a super/this redirection has no
            // assignment shape: the entry expression is the window's last
            // row.
            let window = c.children(initializer);
            if window.start >= window.end {
                continue;
            }
            let entry = window.end - 1;
            if c.kind(entry) != NodeKind::Assignment
                || c.lexeme(c.payload(entry).first().copied().expect("assignment op")) != "="
            {
                continue;
            }
            let pieces = assignment_pieces(c, initializer, entry);
            let Some((field, param, start_row, end_row)) = convertible_assignment(c, &pieces)
            else {
                continue;
            };
            out.push((field, param, start_row, end_row));
        }
    }

    /// The operand pieces of an `Assignment` row: the owner's subtree
    /// roots below the assignment's adopted window (the leak of its
    /// flattened left operand) plus the window rows themselves. Earlier
    /// statements' rows stay in the run — the convertible matcher reads
    /// the tail only.
    fn assignment_pieces(c: &Check<'_>, owner: u32, assignment: u32) -> Vec<u32> {
        let window = c.children(assignment);
        let mut pieces: Vec<u32> = c
            .collect_direct(owner)
            .into_iter()
            .take_while(|&kid| kid != assignment)
            .filter(|&kid| kid < window.start)
            .collect();
        pieces.extend(window.start..window.end);
        pieces
    }

    /// Face B (:87-102): body statements of the exact form `this.x = p;` —
    /// a plain `x = p;` body assignment assigns a local or parameter, not a
    /// field, so only the `this`-targeted form converts. The constructor
    /// body is the `BlockBody` function-body container (no inner `Block` row
    /// on a production parse); a hand-built `Block` keeps its face.
    fn face_body<'a>(c: &Check<'a>, body: u32, out: &mut Vec<(&'a str, &'a str, u32, u32)>) {
        let container = if c.kind(body) == NodeKind::Block {
            body
        } else {
            match c
                .collect_direct(body)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::Block)
            {
                Some(block) => block,
                None => body,
            }
        };
        for assignment in c.collect_direct(container) {
            if c.kind(assignment) != NodeKind::Assignment
                || c.lexeme(
                    c.payload(assignment)
                        .first()
                        .copied()
                        .expect("assignment op"),
                ) != "="
            {
                continue;
            }
            // The statement rows are childless (`ExpressionStatement` is a
            // span leaf), so the operand pieces assemble at the body
            // container.
            let pieces = assignment_pieces(c, container, assignment);
            let Some((field, param, start_row, end_row)) = convertible_assignment(c, &pieces)
            else {
                continue;
            };
            // Only the `this.x = p;` form converts in a body — a plain
            // `x = p;` body assignment assigns a local or parameter, not a
            // field.
            if c.kind(start_row) != NodeKind::ThisExpression {
                continue;
            }
            out.push((field, param, start_row, end_row));
        }
    }

    /// The assignment's `(field name, parameter name, report start row,
    /// report end row)` when it has a convertible shape and a
    /// bare-identifier value. Production tiling: the value closes the run
    /// as a tear-off group `[Id, wrappers, NoArguments, Send]`, and the
    /// target closes at the piece fifth from the end — the bare form's
    /// adopted `Send` leaf (its `[Id, wrappers, NoArguments]` leak precedes
    /// it) or the `this.` form's childless `DotAccess` marker (its
    /// `[this leaf, …, Send]` access run precedes it). Earlier statements'
    /// rows may sit in the run, so only the tail is read. Both forms report
    /// from the entry name / `this` leaf through the VALUE row (see the
    /// branch notes — the official `reportAtNode` node ends at the assigned
    /// value's last token); the marker only names the field. Anything else
    /// (computed targets, prefixed values) is not convertible.
    fn convertible_assignment<'a>(
        c: &Check<'a>,
        pieces: &[u32],
    ) -> Option<(&'a str, &'a str, u32, u32)> {
        let len = pieces.len();
        if len < 8 {
            return None;
        }
        let value = len - 4;
        if !matches!(
            (
                c.kind(pieces[value]),
                c.kind(pieces[value + 1]),
                c.kind(pieces[value + 2]),
                c.kind(pieces[value + 3]),
            ),
            (
                NodeKind::Identifier,
                NodeKind::NoTypeArguments | NodeKind::TypeArguments,
                NodeKind::NoArguments,
                NodeKind::Send
            )
        ) {
            return None;
        }
        let param = c.lexeme(c.head_token(pieces[value]));
        match c.kind(pieces[value - 1]) {
            // `x = p` — the leaked LHS group [Id, wrappers, NoArguments]
            // precedes the adopted Send.
            NodeKind::Send => {
                let base = value - 4;
                if !matches!(
                    (
                        c.kind(pieces[base]),
                        c.kind(pieces[base + 1]),
                        c.kind(pieces[base + 2]),
                    ),
                    (
                        NodeKind::Identifier,
                        NodeKind::NoTypeArguments | NodeKind::TypeArguments,
                        NodeKind::NoArguments,
                    )
                ) {
                    return None;
                }
                let field = c.lexeme(c.head_token(pieces[base]));
                // The report end is the VALUE row: the official range is
                // `reportAtNode`'s node — the whole `ConstructorFieldInitializer`
                // (:93-104) or `AssignmentExpression` (:110-117) — ending at
                // the value's last token, and the value is pinned to a bare
                // identifier (:111 `rightHandSide is SimpleIdentifier`), so
                // `pieces[value]` is that last operand on every lintable
                // candidate. The assignment row's own span runs one token
                // past it (the bare-name send face adopts its follower, so
                // on statement faces it carries the `;`).
                Some((field, param, pieces[base], pieces[value]))
            }
            // `this.x = p` — the access run [this, …, Send] precedes the
            // marker.
            NodeKind::DotAccess => {
                if value < 6 {
                    return None;
                }
                let target = value - 6;
                if c.kind(pieces[target]) != NodeKind::ThisExpression
                    || c.kind(pieces[target + 4]) != NodeKind::Send
                {
                    return None;
                }
                let marker = pieces[value - 1];
                let field = c.lexeme(c.head_token(marker));
                // The report end is the VALUE row (see the bare branch's
                // note): the official `AssignmentExpression` span ends at
                // the value's last token (:110-117), while the assignment
                // row's adopted window anchors on the value group's trailing
                // `Send` and its span carries the statement's `;`.
                Some((field, param, pieces[target], pieces[value]))
            }
            _ => None,
        }
    }

    /// The shared gates and the report (:104-159): one candidate per field,
    /// a same-named non-private instance field of this class, written-type
    /// equality, and no other reference to the parameter.
    fn finish(
        c: &mut Check<'_>,
        params: &[(&str, Option<u32>)],
        fields: &[(&str, Option<u32>)],
        ref_roots: &[(u32, bool)],
        candidates: &[(&str, &str, u32, u32)],
    ) {
        for &(field_name, param_name, range_start, range_end) in candidates {
            // Multiple initializations of one field — the conversion is
            // ambiguous, none of them lints (:48-53, `_nodesToLintByField`).
            if candidates
                .iter()
                .filter(|(name, ..)| *name == field_name)
                .count()
                > 1
            {
                continue;
            }
            if field_name.starts_with('_') {
                continue; // private_named_parameters is off
            }
            let Some(&(_, param_type)) = params.iter().find(|(name, _)| *name == param_name) else {
                continue;
            };
            let Some(&(_, field_type)) = fields.iter().find(|(name, _)| *name == field_name) else {
                continue;
            };
            let (Some(param_type), Some(field_type)) = (param_type, field_type) else {
                continue; // an unwritten type has no comparable spelling
            };
            if !token_seq_equal(c, param_type, field_type) {
                continue;
            }
            let references: u32 = ref_roots
                .iter()
                .map(|&(root, initializer_face)| {
                    count_expression_refs(c, root, param_name, initializer_face)
                })
                .sum();
            if references != 1 {
                continue;
            }
            // The entry window head through the entry expression's end —
            // the assignment row's own span drops the `this.` prefix.
            let start = c.utf16_span(range_start).start;
            let end = c.utf16_span(range_end).end;
            c.report(start..end, &[field_name]);
        }
    }

    /// The expression-position references to `name` within one constructor
    /// reference root (the initializer list or the body). Only
    /// `Expression`-context identifiers count. A name side of a property
    /// access is a member read, never a parameter reference — its selector
    /// is covered by the access's trailing `DotAccess` marker (childless in
    /// the production tiling). On the initializer face the entry
    /// assignment's left-hand side names the field (the official
    /// `ConstructorFieldInitializer` field name), never the parameter.
    fn count_expression_refs(c: &Check<'_>, root: u32, name: &str, initializer_face: bool) -> u32 {
        let expression_ctx = dartq_parser::IdentifierContext::Expression as u32;
        let window = c.children(root);
        let access_markers: Vec<_> = (window.start..window.end)
            .filter(|&piece| c.kind(piece) == NodeKind::DotAccess)
            .map(|piece| c.utf16_span(piece))
            .collect();
        // On the initializer face the entry assignment's left-hand side
        // names the field (the official `ConstructorFieldInitializer` field
        // name), never the parameter. The assignment's adopted child window
        // cannot draw that line — `begin_with_first_child` anchors on the
        // last completed subtree, the value group's trailing `Send` leaf,
        // which sits ABOVE the value's own rows — so the split is the
        // operator token: an identifier closing at or before its `=` is on
        // the field-name side.
        let assignment_ops: Vec<u32> = if initializer_face {
            (window.start..window.end)
                .filter(|&piece| c.kind(piece) == NodeKind::Assignment)
                .filter_map(|piece| c.payload(piece).first().copied())
                .filter(|&op| op != NONE_TOKEN)
                .filter_map(|op| c.unit.tokens.get(op as usize).map(|t| t.utf16_offset))
                .collect()
        } else {
            Vec::new()
        };
        let mut count = 0;
        for id in window.start..window.end {
            if c.kind(id) != NodeKind::Identifier
                || c.payload(id).first() != Some(&expression_ctx)
                || c.lexeme(c.head_token(id)) != name
            {
                continue;
            }
            let span = c.utf16_span(id);
            if assignment_ops.iter().any(|&op| span.end <= op) {
                continue;
            }
            if access_markers
                .iter()
                .any(|access| access.start <= span.start && access.end >= span.end)
            {
                continue;
            }
            count += 1;
        }
        count
    }
}

// —— shared helpers ———————————————————————————————————————————————————————

// —— shadow-check face ——————————————————————————————————————————————————
//
// Shared by `recursive_getters` (read-vs-getter identity) and
// `unnecessary_getters_setters` (assignment-target-vs-field identity): the
// official checks are element identities, so a same-lexeme read or target
// captured by a nearer binding is a different element and must not report.
// The binders the walk honors: local variables, local/named functions (name
// and parameters), catch parameters, for-loop variables, switch-case
// pattern variables, and the enclosing member's own parameters.

/// Whether `stmt` declares a binding of `name` at its own level (a block
/// sibling, or the initializer of a `for` parts list).
fn statement_binds(c: &Check<'_>, stmt: u32, name: &str) -> bool {
    match c.kind(stmt) {
        NodeKind::VariablesDeclaration => c.collect_direct(stmt).into_iter().any(|decl| {
            c.kind(decl) == NodeKind::InitializedIdentifier
                && super::member_name(c, decl).is_some_and(|n| c.lexeme(c.head_token(n)) == name)
        }),
        NodeKind::LocalFunctionDeclaration | NodeKind::NamedFunctionExpression => {
            function_name_binds(c, stmt, name)
        }
        NodeKind::PatternVarDeclStatement => pattern_binds(c, stmt, name),
        NodeKind::LabeledStatement => c
            .collect_direct(stmt)
            .into_iter()
            .any(|kid| c.kind(kid) != NodeKind::Label && statement_binds(c, kid, name)),
        _ => false,
    }
}

/// The name a local/named function binds (`FunctionName` child).
fn function_name_binds(c: &Check<'_>, node: u32, name: &str) -> bool {
    c.collect_direct(node)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::FunctionName)
        .is_some_and(|wrapper| {
            super::member_name(c, wrapper).is_some_and(|n| c.lexeme(c.head_token(n)) == name)
        })
}

/// Whether a pattern subtree declares `name` (`DeclaredVariablePattern`
/// payload word 0 carries the variable token, collapse map §3.7).
fn pattern_binds(c: &Check<'_>, pattern: u32, name: &str) -> bool {
    c.children(pattern).any(|descendant| {
        c.kind(descendant) == NodeKind::DeclaredVariablePattern
            && c.payload(descendant)
                .first()
                .is_some_and(|&v| v != NONE_TOKEN && c.lexeme(v) == name)
    })
}

/// Whether a formal-parameter list declares `name` (plain and
/// function-typed formals, through the shared [`parameter_name`]).
fn parameters_bind(c: &Check<'_>, params: u32, name: &str) -> bool {
    c.collect_direct(params)
        .into_iter()
        .any(|param| parameter_name(c, param) == Some(name))
}

/// The for-in loop variable binds inside the loop: the bare form is an
/// `Identifier` child completing before the parts' `in` token, the `var x`
/// form a `DeclaredVariablePattern` child (the parts node is childless —
/// collapse map §3.4 `ForInLoopPartsHandle`).
fn for_in_binds(c: &Check<'_>, scope: u32, name: &str) -> bool {
    let kids = c.collect_direct(scope);
    let Some(in_token) = kids
        .iter()
        .copied()
        .find(|&kid| c.kind(kid) == NodeKind::ForInParts)
        .and_then(|parts| c.payload(parts).get(3).copied())
        .filter(|&token| token != NONE_TOKEN)
    else {
        return false;
    };
    kids.into_iter().any(|kid| match c.kind(kid) {
        NodeKind::DeclaredVariablePattern => c
            .payload(kid)
            .first()
            .is_some_and(|&v| v < in_token && c.lexeme(v) == name),
        NodeKind::Identifier => {
            c.unit.ast.token_range(kid).end <= in_token && c.lexeme(c.head_token(kid)) == name
        }
        _ => false,
    })
}

/// The read-to-member shadow walk over the ancestor path: at every scope
/// level between `read_index` (exclusive) and the read, a same-lexeme
/// binder on the path captures it. Block siblings bind from their
/// declaration; parameters, catch parameters, for-loop variables, and
/// switch-case pattern variables bind through their scope node; a nested
/// function's own name binds inside its body.
fn path_shadowed(c: &Check<'_>, top_index: usize, name: &str) -> bool {
    let mut child = *c.path.last().expect("read node");
    for &scope in &c.path[top_index + 1..c.path.len() - 1] {
        let binds = match c.kind(scope) {
            NodeKind::Block => c
                .collect_direct(scope)
                .into_iter()
                .any(|stmt| stmt != child && statement_binds(c, stmt, name)),
            NodeKind::CatchClause | NodeKind::FunctionExpression => c
                .collect_direct(scope)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
                .is_some_and(|params| parameters_bind(c, params, name)),
            NodeKind::LocalFunctionDeclaration | NodeKind::NamedFunctionExpression => {
                function_name_binds(c, scope, name)
                    || c.collect_direct(scope)
                        .into_iter()
                        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
                        .is_some_and(|params| parameters_bind(c, params, name))
            }
            NodeKind::ForIn | NodeKind::ForControlFlow => for_in_binds(c, scope, name),
            NodeKind::ForStatement => c
                .collect_direct(scope)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::ForParts)
                .is_some_and(|parts| {
                    c.collect_direct(parts)
                        .into_iter()
                        .any(|stmt| statement_binds(c, stmt, name))
                }),
            NodeKind::SwitchCase | NodeKind::SwitchExpressionCase => {
                c.collect_direct(scope).into_iter().any(|kid| {
                    // Case patterns complete as flat direct children
                    // (collapse map §4), so the variable is the child's
                    // own payload token.
                    c.kind(kid) == NodeKind::DeclaredVariablePattern
                        && c.payload(kid)
                            .first()
                            .is_some_and(|&v| v != NONE_TOKEN && c.lexeme(v) == name)
                })
            }
            _ => false,
        };
        if binds {
            return true;
        }
        child = scope;
    }
    false
}

/// A formal parameter's name (`FormalParameter` word 1, or the
/// function-typed form's identifier child).
fn parameter_name<'a>(c: &'a Check<'_>, parameter: u32) -> Option<&'a str> {
    match c.kind(parameter) {
        NodeKind::FormalParameter => {
            let name = c.payload(parameter).get(1).copied()?;
            (name != NONE_TOKEN).then(|| c.lexeme(name))
        }
        NodeKind::FunctionTypedFormalParameter => {
            let name = member_name(c, parameter)?;
            Some(c.lexeme(c.head_token(name)))
        }
        _ => None,
    }
}

pub(crate) use crate::rules::get_set_token;

pub(crate) mod annotate_overrides {
    use super::{Check, Group, NodeKind, RuleSpec};
    use crate::rules::{MemberQuery, has_override_annotation, member_matches, same_file_ancestors};

    /// Official: `pkg/linter/lib/src/rules/annotate_overrides.dart` — an
    /// instance field or method (getters/setters/operators ride the same
    /// `MethodDeclaration` face) that overrides an inherited member without
    /// a `@override` annotation reports at the member's name token, with
    /// the inherited member's name as the argument (:40-72).
    ///
    /// Approximations (documented): `overriddenMember`
    /// (`extensions.dart:389-406` — `getInheritedMember`, abstract or
    /// concrete, interfaces included) is the same-file ancestor chain built
    /// from the written supertype names (extends/with/on/implements,
    /// `same_file_ancestors(concrete: false)`). Cross-file ancestors
    /// (imports, the SDK) resolve to no row and never report — the dominant
    /// under-report face. The static skips (:57, :68), the augmentation
    /// skip (augmentations are scanner-gated off), the extension-type skip
    /// (:58, :69) and the `@override` check (:42) port directly. The
    /// primary-constructor parameter face (:74-93) stays unported. A
    /// field's lookup answers the getter name namespace (:391 — the
    /// field's getter); a same-named ancestor *method* (a member-namespace
    /// conflict, erroneous code) stays silent here. Message:
    /// `diagnostic.g.dart` `annotateOverrides`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "annotate_overrides",
        group: Group::Recommended,
        problem: "The member '{0}' overrides an inherited member but isn't annotated with '@override'.",
        correction: "Try adding the '@override' annotation.",
        kinds: &[NodeKind::Fields, NodeKind::Method],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        if !is_instance_member(c, node) {
            return;
        }
        // The body: the member's parent — the MemberEnd collapse enters
        // members into the body's children directly (collapse map §3.3
        // MemberEnd D1); hand-built tables may skip the wrapper.
        let Some(owner) = c.ancestor(1) else {
            return;
        };
        if !matches!(
            c.kind(owner),
            NodeKind::DeclarationBody
                | NodeKind::ClassDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::EnumDeclaration
                | NodeKind::ExtensionDeclaration
        ) {
            return;
        }
        // `element.metadata.hasOverride` (:42).
        if has_override_annotation(c, owner, node) {
            return;
        }
        let Some(&class_row) = class_row_of(c) else {
            return;
        };
        let query = match c.kind(node) {
            NodeKind::Method => MemberQuery::Method,
            _ => MemberQuery::Getter,
        };
        // The chain is a per-container invariant — derive it once, not per
        // member name (#758).
        let ancestors = same_file_ancestors(c, class_row, false);
        for name_row in member_names(c, node) {
            // Both faces name at the row's span head (the method's
            // `Identifier`, the field's width-0-at-name
            // `InitializedIdentifier`); the official report anchors at the
            // name token.
            let name_token = c.head_token(name_row);
            let name = c.lexeme(name_token);
            let inherited = ancestors
                .iter()
                .find_map(|&ancestor| inherited_member(c, ancestor, query, name));
            if inherited.is_some() {
                let range = crate::rules::utf16_token_span(c, name_token);
                c.report(range, &[name]);
            }
        }
    }

    /// The first member of `ancestor`'s body matching `query` by `name`.
    fn inherited_member(
        c: &Check<'_>,
        ancestor: u32,
        query: MemberQuery,
        name: &str,
    ) -> Option<u32> {
        crate::rules::body_members(c, ancestor)
            .into_iter()
            .find(|&member| !member_matches(c, member, query, name).is_empty())
    }

    /// The instance-member gates shared with the other override rules:
    /// fields by their kind payload, methods by the header ModSet bit
    /// (collapse map §1 — static is bit 3), extension-type members out of
    /// face (:58, :69).
    fn is_instance_member(c: &Check<'_>, node: u32) -> bool {
        const STATIC_BIT: u16 = 1 << 3;
        match c.kind(node) {
            NodeKind::Fields => {
                c.payload(node).first() == Some(&(dartq_parser::MemberKind::NonStaticField as u32))
                    && !in_extension_type(c)
            }
            NodeKind::Method => {
                dartq_ast::mod_flags(c.unit.ast, node) & STATIC_BIT == 0 && !in_extension_type(c)
            }
            _ => false,
        }
    }

    fn in_extension_type(c: &Check<'_>) -> bool {
        c.path
            .iter()
            .any(|&a| c.kind(a) == NodeKind::ExtensionTypeDeclaration)
    }

    /// The nearest enclosing class-like row (extension types excluded —
    /// their members never report).
    fn class_row_of<'a>(c: &'a Check<'a>) -> Option<&'a u32> {
        c.path.iter().rev().find(|&&a| {
            matches!(
                c.kind(a),
                NodeKind::ClassDeclaration
                    | NodeKind::MixinDeclaration
                    | NodeKind::EnumDeclaration
                    | NodeKind::ExtensionDeclaration
            )
        })
    }

    /// The checked name rows: the method's name (`Identifier` in the
    /// method context, or the operator name), a field group's every
    /// declared variable (:60-62 — the `InitializedIdentifier` rows, whose
    /// span head is the name token).
    fn member_names(c: &Check<'_>, node: u32) -> Vec<u32> {
        match c.kind(node) {
            NodeKind::Method => c
                .collect_direct(node)
                .into_iter()
                .filter(|&kid| {
                    matches!(c.kind(kid), NodeKind::OperatorName)
                        || (c.kind(kid) == NodeKind::Identifier
                            && c.payload(kid).first()
                                == Some(
                                    &(dartq_parser::IdentifierContext::MethodDeclaration as u32),
                                ))
                })
                .collect(),
            _ => {
                let mut names = Vec::new();
                for decl in c.collect_direct(node) {
                    if c.kind(decl) != NodeKind::VariablesDeclaration {
                        continue;
                    }
                    for var in c.collect_direct(decl) {
                        if c.kind(var) == NodeKind::InitializedIdentifier
                            && crate::rules::initialized_identifier_name(c, var).is_some()
                        {
                            names.push(var);
                        }
                    }
                }
                names
            }
        }
    }
}

pub(crate) mod avoid_renaming_method_parameters {
    use super::{Check, Group, NodeKind, RuleSpec};
    use crate::rules::{
        MemberQuery, has_doc_comment, member_is_concrete, member_matches, same_file_ancestors,
        utf16_token_span,
    };

    /// Official: `pkg/linter/lib/src/rules/avoid_renaming_method_parameters.dart`
    /// — an overriding method whose positional parameter names differ from
    /// the inherited concrete method's reports at each renamed parameter
    /// token, with `(name, inherited name)` (:90-114).
    ///
    /// Approximations (documented): the inherited concrete member
    /// (`getInheritedConcreteMember`, :76-79) is the same-file
    /// extends/with/on chain, concrete members only (an abstract
    /// declaration never counts). The lib-directory gate (:37
    /// `isInLibDir` — the file lives under the package's `lib/`) reads the
    /// unit path's `lib` component. The skips port: static methods (:56),
    /// methods with a documentation comment (:57), getters/setters/
    /// operators (no parameter list, :59-61), named parameters (positional
    /// only, :90). Wildcard `_` parameters skip on both sides (:97-106,
    /// feature-gated, the shipped default). The augmentation branch
    /// (:82-88) stays unported (augmentations are scanner-gated off).
    /// Cross-file inherited methods under-report. Message:
    /// `diagnostic.g.dart` `avoidRenamingMethodParameters`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "avoid_renaming_method_parameters",
        group: Group::Recommended,
        problem: "The parameter name '{0}' doesn't match the name '{1}' in the overridden method.",
        correction: "Try changing the name to '{1}'.",
        kinds: &[NodeKind::Method],
        check,
    };

    fn check(c: &mut Check<'_>) {
        // `if (!context.isInLibDir) return` (:37).
        let in_lib = c
            .unit
            .path
            .is_some_and(|path| path.components().any(|part| part.as_os_str() == "lib"));
        if !in_lib {
            return;
        }
        let node = c.node();
        // Static skip (:56) — header ModSet bit (static is bit 3).
        const STATIC_BIT: u16 = 1 << 3;
        if dartq_ast::mod_flags(c.unit.ast, node) & STATIC_BIT != 0 {
            return;
        }
        // A documentation comment on the override skips (:57).
        if has_doc_comment(c, node) {
            return;
        }
        let Some(params) = c
            .collect_direct(node)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
        else {
            return; // getters/setters/operators have no parameters (:59-61)
        };
        let Some(&class_row) = c.path.iter().rev().find(|&&a| {
            matches!(
                c.kind(a),
                NodeKind::ClassDeclaration
                    | NodeKind::MixinDeclaration
                    | NodeKind::EnumDeclaration
                    | NodeKind::ExtensionDeclaration
            )
        }) else {
            return;
        };
        let Some(name_row) = c.collect_direct(node).into_iter().find(|&kid| {
            c.kind(kid) == NodeKind::Identifier
                && c.payload(kid).first()
                    == Some(&(dartq_parser::IdentifierContext::MethodDeclaration as u32))
        }) else {
            return;
        };
        let name = c.lexeme(c.head_token(name_row));
        let Some(inherited) = same_file_ancestors(c, class_row, true)
            .into_iter()
            .find_map(|ancestor| {
                crate::rules::body_members(c, ancestor)
                    .into_iter()
                    .find(|&member| {
                        // The concreteness test reads the member container —
                        // `member_matches` answers name rows, not the member.
                        c.kind(member) == NodeKind::Method
                            && member_is_concrete(c, member)
                            && !member_matches(c, member, MemberQuery::Method, name).is_empty()
                    })
            })
        else {
            return;
        };
        let inherited_names: Vec<&str> = inherited_positional_names(c, inherited);
        let mine = positional_params(c, params);
        let count = mine.len().min(inherited_names.len());
        let mut renames: Vec<(u32, &str, &str)> = Vec::new();
        for i in 0..count {
            let (_, mine_token) = mine[i];
            let mine_name = c.lexeme(mine_token);
            let inherited_name = inherited_names[i];
            if is_wildcard(c, mine_name) || is_wildcard(c, inherited_name) {
                continue;
            }
            if mine_name != inherited_name {
                renames.push((mine_token, mine_name, inherited_name));
            }
        }
        // The names borrow `c`; own them before reporting.
        let renames: Vec<(u32, String, String)> = renames
            .into_iter()
            .map(|(t, m, i)| (t, m.to_owned(), i.to_owned()))
            .collect();
        for (token, mine_name, inherited_name) in renames {
            let range = utf16_token_span(c, token);
            c.report(range, &[mine_name.as_str(), inherited_name.as_str()]);
        }
    }

    /// The `_` wildcard face (:51-52, :97-106) — feature-gated, the
    /// shipped default is on.
    fn is_wildcard(c: &Check<'_>, name: &str) -> bool {
        c.unit.features.wildcard_variables && name == "_"
    }

    /// The `(receiver, name-token)` pairs of the positional parameters,
    /// required group then `[]` group, in order (:90 — named parameters
    /// stay out).
    fn positional_params(c: &Check<'_>, params: u32) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        for section in c.collect_direct(params) {
            match c.kind(section) {
                NodeKind::FormalParameter => {
                    if let Some(pair) = param_pair(c, section) {
                        out.push(pair);
                    }
                }
                NodeKind::OptionalFormalParameters => {
                    // The `[]` group is positional; the `{}` group is named.
                    if c.lexeme(c.head_token(section)) != "[" {
                        continue;
                    }
                    for param in c.collect_direct(section) {
                        if c.kind(param) == NodeKind::FormalParameter
                            && let Some(pair) = param_pair(c, param)
                        {
                            out.push(pair);
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// A formal's `(receiver, name)` tokens (receiver formals keep their
    /// written name — `this.x` names `x`).
    fn param_pair(c: &Check<'_>, param: u32) -> Option<(u32, u32)> {
        let payload = c.payload(param);
        let name = payload.get(1).copied()?;
        (name != crate::rules::NONE_TOKEN).then_some((0, name))
    }

    /// The inherited method's positional parameter names, in order (the
    /// member row is the lookup's answer — no second body scan).
    fn inherited_positional_names<'a>(c: &'a Check<'_>, member: u32) -> Vec<&'a str> {
        let Some(params) = c
            .collect_direct(member)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
        else {
            return Vec::new();
        };
        positional_params(c, params)
            .into_iter()
            .map(|(_, token)| c.lexeme(token))
            .collect()
    }
}

pub(crate) mod overridden_fields {
    use super::{Check, Group, NodeKind, RuleSpec};
    use crate::rules::{MemberQuery, member_matches, same_file_ancestors};

    /// Official: `pkg/linter/lib/src/rules/overridden_fields.dart` — an
    /// instance field whose inherited concrete member is a variable-backed
    /// getter reports at the field name, with the defining interface's name
    /// as the argument (:66-81).
    ///
    /// Approximations (documented): `getInheritedConcreteMember` (:69-71)
    /// is the same-file extends/with/on chain, concrete members only; the
    /// variable-backed getter test (`isOriginVariable`, :72-73) is the
    /// ancestor's `Fields` rows exclusively (a declared getter never
    /// matches). The defining interface (:74-78) is the nearest ancestor
    /// class row declaring the field — the most-specific concrete
    /// declaration, as the official lookup resolves. Static fields skip
    /// (:40), augmentations are scanner-gated off, and the
    /// primary-constructor parameter face (:50-64) stays unported.
    /// Cross-file ancestors under-report. Message: `diagnostic.g.dart`
    /// `overriddenFields`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "overridden_fields",
        group: Group::Recommended,
        problem: "Field overrides a field inherited from '{0}'.",
        correction: "Try removing the field, overriding the getter and setter if necessary.",
        kinds: &[NodeKind::Fields],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // `node.isStatic` (:40).
        if c.payload(node).first() != Some(&(dartq_parser::MemberKind::NonStaticField as u32)) {
            return;
        }
        let Some(&class_row) = c.path.iter().rev().find(|&&a| {
            matches!(
                c.kind(a),
                NodeKind::ClassDeclaration
                    | NodeKind::MixinDeclaration
                    | NodeKind::EnumDeclaration
                    | NodeKind::ExtensionDeclaration
            )
        }) else {
            return;
        };
        let chain = same_file_ancestors(c, class_row, true);
        let fields: Vec<(&str, u32)> = field_names(c, node)
            .into_iter()
            .map(|(n, r)| (n, r))
            .collect();
        // The nearest concrete variable-backed declaration in the chain
        // supplies the defining interface (:69-78); names collect owned —
        // the lookup borrows `c` immutably across the report.
        let mut hits: Vec<(u32, String)> = Vec::new();
        for (name, name_row) in fields {
            if let Some(defining) = chain.iter().find_map(|&ancestor| {
                crate::rules::body_members(c, ancestor)
                    .into_iter()
                    .find(|&member| {
                        c.kind(member) == NodeKind::Fields
                            && !member_matches(c, member, MemberQuery::Variable, name).is_empty()
                    })
                    .and_then(|_| declaration_name(c, ancestor))
            }) {
                hits.push((name_row, defining.to_owned()));
            }
        }
        for (name_row, defining) in hits {
            let range = c.utf16_span(name_row);
            c.report(range, &[defining.as_str()]);
        }
    }

    /// The `(name lexeme, name row)` pairs of one `Fields` group.
    fn field_names<'a>(c: &Check<'a>, node: u32) -> Vec<(&'a str, u32)> {
        let mut out = Vec::new();
        for decl in c.collect_direct(node) {
            if c.kind(decl) != NodeKind::VariablesDeclaration {
                continue;
            }
            for var in c.collect_direct(decl) {
                if c.kind(var) != NodeKind::InitializedIdentifier {
                    continue;
                }
                if let Some(row) = crate::rules::initialized_identifier_name(c, var) {
                    out.push((c.lexeme(row), row));
                }
            }
        }
        out
    }

    /// The declared class-like name of the ancestor row.
    fn declaration_name<'a>(c: &'a Check<'a>, class_row: u32) -> Option<&'a str> {
        crate::rules::class_declaration_name(c, class_row)
    }
}
pub(crate) mod unnecessary_overrides {
    use super::{Check, Group, NodeKind, RuleSpec};
    use crate::rules::{
        MemberQuery, has_any_annotation, has_doc_comment, member_is_concrete, member_matches,
        same_file_ancestors, token_seq_equal, type_before,
    };

    /// Official: `pkg/linter/lib/src/rules/unnecessary_overrides.dart` —
    /// an override whose body is exactly a super forward with the same
    /// parameters reports at the method name (:52-113, the four faces).
    ///
    /// Approximations (documented): the inherited concrete member is the
    /// same-file extends/with/on chain (an abstract declaration never
    /// counts; cross-file under-reports). The signature-equality guard
    /// (`_haveSameDeclaration`, :133-155 — element types) reads written
    /// shapes: same written return type (token run equality or both
    /// absent), same parameter count, per-parameter kind group, name,
    /// written prefix run (type + `covariant`), and default value run —
    /// differing-but-subtype spellings and resolved defaults under-report.
    /// The metadata guard (:117-131) skips on any annotation except
    /// `@override` (the `@protected` pairing reads the written name). The
    /// noSuchMethod and doc-comment skips (:76-79), the body-shape walk
    /// (:52-113: one statement / arrow body, no comments before the
    /// `return`, super target, same-name selector, arguments naming the
    /// parameters) and the setter write face (:313-326) port structurally;
    /// the operator faces (:245-292) stay unported (operator forwards have
    /// no parameter-name face to preserve). Message: `diagnostic.g.dart`
    /// `unnecessaryOverrides`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "unnecessary_overrides",
        group: Group::Core,
        problem: "Unnecessary override.",
        correction: "Try adding behavior in the overriding member or removing the override.",
        kinds: &[NodeKind::Method],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // `node.isStatic` (:332) — header ModSet bit.
        const STATIC_BIT: u16 = 1 << 3;
        if dartq_ast::mod_flags(c.unit.ast, node) & STATIC_BIT != 0 {
            return;
        }
        let Some(name_row) = c.collect_direct(node).into_iter().find(|&kid| {
            c.kind(kid) == NodeKind::Identifier
                && c.payload(kid).first()
                    == Some(&(dartq_parser::IdentifierContext::MethodDeclaration as u32))
        }) else {
            return; // operators stay unported (documented)
        };
        let name = c.lexeme(c.head_token(name_row));
        // 'noSuchMethod' is mandatory to proxify (:76).
        if name == "noSuchMethod" {
            return;
        }
        // It's ok to override to have better documentation (:79).
        if has_doc_comment(c, node) {
            return;
        }
        let get_set = super::get_set_token(c, node);
        let query = match get_set {
            Some("get") => MemberQuery::Getter,
            Some("set") => MemberQuery::Setter,
            _ => MemberQuery::Method,
        };
        let Some(&class_row) = c.path.iter().rev().find(|&&a| {
            matches!(
                c.kind(a),
                NodeKind::ClassDeclaration
                    | NodeKind::MixinDeclaration
                    | NodeKind::EnumDeclaration
                    | NodeKind::ExtensionDeclaration
            )
        }) else {
            return;
        };
        // The inherited concrete member (:81-82).
        let Some(inherited) = same_file_ancestors(c, class_row, true)
            .into_iter()
            .find_map(|ancestor| {
                crate::rules::body_members(c, ancestor)
                    .into_iter()
                    .find(|&member| {
                        c.kind(member) == NodeKind::Method
                            && member_is_concrete(c, member)
                            && !member_matches(c, member, query, name).is_empty()
                    })
            })
        else {
            return;
        };
        // It's ok to override to add annotations (:87, :117-131).
        let Some(owner) = c.ancestor(1) else {
            return;
        };
        if has_any_annotation(c, owner, node) {
            return;
        }
        // It's ok to override to change the signature (:90, :133-155).
        if !same_declaration(c, node, name_row, inherited) {
            return;
        }
        // The body walk (:95, :52-113).
        if forwards_to_super(c, node, query, name) {
            let range = c.utf16_span(name_row);
            c.report(range, &[]);
        }
    }

    /// `_haveSameDeclaration` (:133-155) over written shapes: return type,
    /// then every parameter's name, kind group, receiver, prefix run
    /// (written type + modifiers) and default run.
    fn same_declaration(c: &Check<'_>, mine: u32, mine_name: u32, inherited: u32) -> bool {
        // The inherited method's name row (the query matched it). The
        // member-name identifier carries the `MethodDeclaration` context —
        // a written return type's own name tiles before it in the
        // completion order, so the first `Identifier` is not the name.
        let kids = c.collect_direct(inherited);
        let Some(inherited_name) = kids
            .iter()
            .copied()
            .find(|&kid| {
                c.kind(kid) == NodeKind::Identifier
                    && c.payload(kid).first()
                        == Some(&(dartq_parser::IdentifierContext::MethodDeclaration as u32))
            })
            .or_else(|| {
                kids.iter()
                    .copied()
                    .find(|&kid| c.kind(kid) == NodeKind::OperatorName)
            })
        else {
            return false;
        };
        // Return types: same written spelling or both absent (:138-140).
        match (
            type_before(c, mine, mine_name),
            type_before(c, inherited, inherited_name),
        ) {
            (None, None) => {}
            (Some(a), Some(b)) if token_seq_equal(c, a.0, b.0) => {}
            _ => return false,
        }
        let (Some(mine_params), Some(inherited_params)) = (
            c.collect_direct(mine)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::FormalParameters),
            c.collect_direct(inherited)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::FormalParameters),
        ) else {
            // Both sides are accessors — no parameter lists to differ.
            return true;
        };
        let a = flat_params(c, mine_params);
        let b = flat_params(c, inherited_params);
        a.len() == b.len()
            && a.iter().zip(b.iter()).all(|(p, q)| {
                p.name == q.name
                    && p.group == q.group
                    && p.receiver == q.receiver
                    && prefix_run(c, p.name_token) == prefix_run(c, q.name_token)
                    && default_run(c, p.name_token) == default_run(c, q.name_token)
            })
    }

    /// One parameter's comparable face: receiver lexeme (`this`/`super`/
    /// `covariant`), name lexeme, kind group (0 required, 1 optional
    /// positional, 2 named).
    struct ParamFace<'a> {
        receiver: Option<&'a str>,
        name: &'a str,
        name_token: u32,
        group: u8,
    }

    /// Every parameter of a list in order, all groups.
    fn flat_params<'a>(c: &'a Check<'_>, params: u32) -> Vec<ParamFace<'a>> {
        let mut out = Vec::new();
        for section in c.collect_direct(params) {
            match c.kind(section) {
                NodeKind::FormalParameter => {
                    if let Some(face) = param_face(c, section, 0) {
                        out.push(face);
                    }
                }
                NodeKind::OptionalFormalParameters => {
                    let group: u8 = if c.lexeme(c.head_token(section)) == "{" {
                        2
                    } else {
                        1
                    };
                    for param in c.collect_direct(section) {
                        if c.kind(param) == NodeKind::FormalParameter
                            && let Some(face) = param_face(c, param, group)
                        {
                            out.push(face);
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

    fn param_face<'a>(c: &'a Check<'_>, param: u32, group: u8) -> Option<ParamFace<'a>> {
        let payload = c.payload(param);
        let receiver = payload.first().copied().unwrap_or(crate::rules::NONE_TOKEN);
        let name_token = payload.get(1).copied()?;
        if name_token == crate::rules::NONE_TOKEN {
            return None;
        }
        let receiver = (receiver != crate::rules::NONE_TOKEN).then(|| c.lexeme(receiver));
        Some(ParamFace {
            receiver,
            name: c.lexeme(name_token),
            name_token,
            group,
        })
    }

    /// The written tokens before a parameter's name, back to the enclosing
    /// boundary — the type-and-modifier run (`covariant int?`).
    fn prefix_run<'a>(c: &'a Check<'_>, name_token: u32) -> Vec<&'a str> {
        let mut run = Vec::new();
        let mut token = name_token;
        for _ in 0..64 {
            let Some(prev) = token.checked_sub(1) else {
                break;
            };
            token = prev;
            match c.lexeme(prev) {
                "," | "(" | ")" | "{" | "}" | "[" | "]" | ";" | "=" | "<" => break,
                lex => run.push(lex),
            }
        }
        run.reverse();
        run
    }

    /// The written tokens after a parameter's name up to the next
    /// boundary — the default value run (presence and spelling).
    fn default_run<'a>(c: &'a Check<'_>, name_token: u32) -> Vec<&'a str> {
        let mut run = Vec::new();
        let mut token = name_token;
        for _ in 0..64 {
            token += 1;
            if c.unit.tokens.get(token as usize).is_none() {
                break;
            }
            match c.lexeme(token) {
                "," | ")" | "}" | "]" | ";" | "{" => break,
                lex => run.push(lex),
            }
        }
        run
    }

    /// The body walk (:52-113): an arrow body's expression — the childless
    /// `ExpressionBody` handle keeps the expression as the member window's
    /// trailing rows — or a block body's single statement, whose rows tile
    /// flat into the block (the statement row itself trails the
    /// expression; the `return`-comment guard :105, no guard on a bare
    /// expression statement :69-71); then the face shape.
    fn forwards_to_super(c: &Check<'_>, node: u32, query: MemberQuery, name: &str) -> bool {
        let kids = c.collect_direct(node);
        let Some(&body) = kids
            .iter()
            .find(|&&kid| matches!(c.kind(kid), NodeKind::BlockBody | NodeKind::ExpressionBody))
        else {
            return false;
        };
        let rows: Vec<u32> = match c.kind(body) {
            NodeKind::ExpressionBody => {
                // The expression completes before the childless handle
                // opens (bridge `handle_expression_function_body`): the
                // member-window pieces before the handle, from the arrow
                // on — the signature rows tile before the arrow and drop
                // out.
                let arrow_end = crate::rules::byte_span_of_token(c, c.head_token(body)).end;
                let pos = kids.iter().position(|&kid| kid == body).unwrap_or(0);
                kids[..pos]
                    .iter()
                    .copied()
                    .filter(|&row| c.byte_span(row).start >= arrow_end)
                    .collect()
            }
            _ => {
                // Statement tiling: the statement rows (and their expression
                // pieces) are the body row's own children on production
                // parses — a `Block` wrapper row exists only in hand-built
                // trees.
                let block_rows = match c
                    .collect_direct(body)
                    .into_iter()
                    .find(|&kid| c.kind(kid) == NodeKind::Block)
                {
                    Some(block) => c.collect_direct(block),
                    None => c.collect_direct(body),
                };
                // Exactly one statement (:52-56): the statement's own row
                // is the block window's last row, and no other statement
                // row precedes it.
                let Some((&last, head)) = block_rows.split_last() else {
                    return false;
                };
                if head
                    .iter()
                    .any(|&row| crate::rules::is_statement_row(c, row))
                {
                    return false;
                }
                match c.kind(last) {
                    NodeKind::ExpressionStatement => head.to_vec(),
                    NodeKind::ReturnStatement => {
                        if c.comments.has_attached(c.head_token(last) as usize) {
                            return false;
                        }
                        c.collect_direct(last)
                    }
                    _ => return false,
                }
            }
        };
        match query {
            MemberQuery::Method => super_call_forwards(c, &rows, name, node),
            MemberQuery::Getter => super_getter_forwards(c, &rows, name),
            MemberQuery::Setter => {
                let Some(param) = sole_parameter(c, node) else {
                    return false;
                };
                super_setter_forwards(c, &rows, name, param)
            }
            MemberQuery::Variable => false,
        }
    }

    /// The setter's single parameter name (:314-316 — one parameter).
    fn sole_parameter<'a>(c: &'a Check<'_>, node: u32) -> Option<&'a str> {
        let params = c
            .collect_direct(node)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)?;
        let faces = flat_params(c, params);
        match faces.as_slice() {
            [one] => Some(one.name),
            _ => None,
        }
    }

    /// The method face (:232-242): a super invocation of `name` whose
    /// arguments name the parameters (`argumentsMatchParameters`,
    /// `dart_type_utilities.dart:13-49`).
    fn super_call_forwards(c: &Check<'_>, rows: &[u32], name: &str, node: u32) -> bool {
        let Some(send_pos) = rows.iter().position(|&row| c.kind(row) == NodeKind::Send) else {
            return false;
        };
        // The callee head and the argument group (flat production tiling).
        let Some((head, arguments)) = crate::rules::slice::send_parts(c, rows, send_pos) else {
            return false;
        };
        // The target is `super` and the selector names the member.
        if head == 0 || c.kind(rows[head - 1]) != NodeKind::SuperExpression {
            return false;
        }
        if c.lexeme(c.head_token(rows[head])) != name {
            return false;
        }
        // The invocation is the whole expression: nothing after the send
        // but its own access marker.
        let trailing = &rows[send_pos + 1..];
        if trailing
            .iter()
            .any(|&row| !matches!(c.kind(row), NodeKind::DotAccess | NodeKind::Qualified))
        {
            return false;
        }
        let Some(arguments) = arguments else {
            return false;
        };
        // The parameter names in order: positional group then named group.
        let params = c
            .collect_direct(node)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::FormalParameters);
        let Some(params) = params else {
            return false;
        };
        let faces = flat_params(c, params);
        let positional: Vec<&str> = faces
            .iter()
            .filter(|f| f.group != 2)
            .map(|f| f.name)
            .collect();
        let named: Vec<&str> = faces
            .iter()
            .filter(|f| f.group == 2)
            .map(|f| f.name)
            .collect();
        arguments_match_parameters(c, arguments, &positional, &named)
    }

    /// `argumentsMatchParameters` (:13-49): each positional argument is
    /// the bare identifier of the corresponding positional parameter; the
    /// named arguments cover the named parameters one-to-one, each value
    /// the bare identifier of its parameter.
    fn arguments_match_parameters(
        c: &Check<'_>,
        arguments: u32,
        positional: &[&str],
        named: &[&str],
    ) -> bool {
        let rows = c.collect_direct(arguments);
        // Split the flattened rows into arguments: a named group runs from
        // its label to the marker; a positional argument is one row. The
        // bare-name send group's markers tile beside each argument root and
        // stay out of the split.
        let mut pos_args: Vec<&str> = Vec::new();
        let mut named_args: Vec<(&str, &str)> = Vec::new();
        let mut i = 0;
        while i < rows.len() {
            if crate::rules::slice::is_send_group_piece(c, rows[i]) {
                i += 1;
                continue;
            }
            if c.kind(rows[i]) == NodeKind::NamedArgument {
                // The label is the identifier two tokens back; the value is
                // the root row before the marker (its send group trails).
                let Some(label) = crate::rules::slice::named_arg_name(c, &rows, i) else {
                    return false;
                };
                let Some(value_row) = (0..i)
                    .rev()
                    .find(|&j| !crate::rules::slice::is_send_group_piece(c, rows[j]))
                    .map(|j| rows[j])
                else {
                    return false;
                };
                if c.kind(value_row) != NodeKind::Identifier {
                    return false;
                }
                named_args.push((
                    c.lexeme(c.head_token(rows[label])),
                    c.lexeme(c.head_token(value_row)),
                ));
                i += 1;
                continue;
            }
            if c.kind(rows[i]) != NodeKind::Identifier {
                return false;
            }
            pos_args.push(c.lexeme(c.head_token(rows[i])));
            i += 1;
        }
        if pos_args.len() != positional.len()
            || pos_args.iter().zip(positional).any(|(a, p)| a != p)
        {
            return false;
        }
        if named_args.len() != named.len() {
            return false;
        }
        named.iter().all(|param| {
            named_args
                .iter()
                .any(|(label, value)| *label == *param && *value == *param)
        })
    }

    /// The getter face (:204-208): a super property read of `name` —
    /// `super.name`, nothing else. The trailing send group (the bare-name
    /// `parse_send` markers and leaf) tiles beside the chain rows.
    fn super_getter_forwards(c: &Check<'_>, rows: &[u32], name: &str) -> bool {
        // The access marker spans the whole read; the send group trails it.
        let mut end = rows.len();
        while end > 0
            && (matches!(
                c.kind(rows[end - 1]),
                NodeKind::DotAccess | NodeKind::Qualified
            ) || crate::rules::slice::is_send_group_piece(c, rows[end - 1]))
        {
            end -= 1;
        }
        let [target, property] = &rows[..end] else {
            return false;
        };
        c.kind(*target) == NodeKind::SuperExpression
            && c.kind(*property) == NodeKind::Identifier
            && c.lexeme(c.head_token(*property)) == name
    }

    /// The setter face (:313-326): an assignment whose right side is the
    /// bare parameter and whose left side is the super property.
    fn super_setter_forwards(c: &Check<'_>, rows: &[u32], name: &str, param: &str) -> bool {
        let Some((idx, &assignment)) = rows
            .iter()
            .enumerate()
            .find(|(_, row)| c.kind(**row) == NodeKind::Assignment)
        else {
            return false;
        };
        let Some(&op) = c.payload(assignment).first() else {
            return false;
        };
        if c.lexeme(op) != "=" {
            return false;
        }
        let Some(op_start) = c
            .unit
            .tokens
            .get(op as usize)
            .map(|t| t.byte_offset as usize)
        else {
            return false;
        };
        // The operand runs: rows left/right of the operator byte, from the
        // statement's own rows and the assignment's adopted window (the
        // right side's pieces tile before the adopted window opens).
        let window = c.children(assignment);
        let mut lhs: Vec<u32> = rows[..idx]
            .iter()
            .copied()
            .filter(|&row| c.byte_span(row).end <= op_start)
            .collect();
        lhs.extend((window.start..window.end).filter(|&row| c.byte_span(row).end <= op_start));
        let mut rhs: Vec<u32> = rows
            .iter()
            .copied()
            .filter(|&row| row != assignment && c.byte_span(row).start >= op_start)
            .collect();
        rhs.extend((window.start..window.end).filter(|&row| c.byte_span(row).start >= op_start));
        // LHS: `super.name` (an access marker and the bare-name send group
        // may trail).
        while lhs.len() > 2
            && (matches!(
                c.kind(lhs[lhs.len() - 1]),
                NodeKind::DotAccess | NodeKind::Qualified
            ) || crate::rules::slice::is_send_group_piece(c, lhs[lhs.len() - 1]))
        {
            lhs.pop();
        }
        let [target, property] = lhs.as_slice() else {
            return false;
        };
        if c.kind(*target) != NodeKind::SuperExpression
            || c.kind(*property) != NodeKind::Identifier
            || c.lexeme(c.head_token(*property)) != name
        {
            return false;
        }
        // RHS: the bare parameter identifier (the send group trails).
        while rhs.len() > 1
            && (matches!(
                c.kind(rhs[rhs.len() - 1]),
                NodeKind::DotAccess | NodeKind::Qualified
            ) || crate::rules::slice::is_send_group_piece(c, rhs[rhs.len() - 1]))
        {
            rhs.pop();
        }
        // The left selector's own send-group markers anchor right of the
        // operator byte (the width-0 `NoArguments` sits at the value head),
        // so they leak into the rhs head — they carry no face here.
        let lead = rhs
            .iter()
            .position(|&row| !crate::rules::slice::is_send_group_piece(c, row))
            .unwrap_or(rhs.len());
        rhs.drain(..lead);
        matches!(rhs.as_slice(), [only] if c.kind(*only) == NodeKind::Identifier && c.lexeme(c.head_token(*only)) == param)
    }
}

#[cfg(test)]
mod tests {
    use dartq_ast::NodeKind;

    use crate::testsupport::{Fixture, NONE, identifier, one_rule, tok, unit_of, utf16_at};
    use dartq_parser::MemberKind;

    /// `test/rules/avoid_return_types_on_setters_test.dart` `test_typedSet`.
    #[test]
    fn avoid_return_types_reports_a_typed_setter() {
        let source = "void set x(v) {}";
        // void0 set1 x2 (3 v4 )5 {6 }7
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, tok(source, t, "set", 0));
                b.leaf_span(NodeKind::Type, 0, 1);
                identifier(b, 2, 3);
                b.begin(NodeKind::FormalParameters, 3);
                b.end(NodeKind::FormalParameters, 6);
                b.begin(NodeKind::Block, 6);
                b.end(NodeKind::Block, 8);
                b.end(NodeKind::TopLevelMethod, 8);
            }),
        );
        let lints = f.run(&one_rule("avoid_return_types_on_setters"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "Unnecessary return type on a setter.");
        assert_eq!(lints[0].range.start, utf16_at(&f, 0));
        assert_eq!(lints[0].range.end, utf16_at(&f, 0) + 4); // after `void`
    }

    #[test]
    fn avoid_return_types_untyped_setter_passes() {
        let source = "set x(v) {}";
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, tok(source, t, "set", 0));
                b.leaf(NodeKind::NoType, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::FormalParameters, 2);
                b.end(NodeKind::FormalParameters, 5);
                b.begin(NodeKind::Block, 5);
                b.end(NodeKind::Block, 7);
                b.end(NodeKind::TopLevelMethod, 7);
            }),
        );
        assert!(f.run(&one_rule("avoid_return_types_on_setters")).is_empty());
    }

    /// Getters never report — only the `set` keyword qualifies.
    #[test]
    fn avoid_return_types_getter_passes() {
        let source = "int get x => 1;";
        // int0 get1 x2 =>3 14 ;5
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, tok(source, t, "get", 0));
                b.leaf_span(NodeKind::Type, 0, 1);
                identifier(b, 2, 3);
                b.leaf(NodeKind::LiteralInt, 4);
                b.end(NodeKind::TopLevelMethod, 6);
            }),
        );
        assert!(f.run(&one_rule("avoid_return_types_on_setters")).is_empty());
    }

    /// `test/rules/unnecessary_late_test.dart` `test_topLevelVariable`.
    #[test]
    fn unnecessary_late_reports_a_top_level_variable() {
        let source = "late var x = 1;";
        // late0 var1 x2 =3 14 ;5 — `late` is the span head (#174)
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::TopLevelFields, 0);
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, tok(source, t, "var", 0));
                b.begin(NodeKind::InitializedIdentifier, 2);
                b.begin(NodeKind::VariableInitializer, 3);
                b.leaf(NodeKind::LiteralInt, 4);
                b.end(NodeKind::VariableInitializer, 5);
                b.end(NodeKind::InitializedIdentifier, 5);
                b.end(NodeKind::VariablesDeclaration, 6);
                b.end(NodeKind::TopLevelFields, 6);
            }),
        );
        let lints = f.run(&one_rule("unnecessary_late"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "Unnecessary 'late' modifier.");
        assert_eq!(lints[0].range.start, utf16_at(&f, 0));
    }

    /// An uninitialized declaration keeps `late` meaningful (:51-54).
    #[test]
    fn unnecessary_late_uninitialized_passes() {
        let source = "late var x;";
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::TopLevelFields, 0);
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, tok(source, t, "var", 0));
                b.begin(NodeKind::InitializedIdentifier, 2);
                b.end(NodeKind::InitializedIdentifier, 3);
                b.end(NodeKind::VariablesDeclaration, 3);
                b.end(NodeKind::TopLevelFields, 3);
            }),
        );
        assert!(f.run(&one_rule("unnecessary_late")).is_empty());
    }

    /// Instance fields are excluded — lazy init is meaningful there.
    #[test]
    fn unnecessary_late_instance_field_passes() {
        let source = "class A { late var x = 1; }";
        // class0 A1 {2 late3 var4 x5 =6 17 ;8 }9
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelDeclaration, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::ClassDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.begin(NodeKind::DeclarationBody, 2);
                b.begin(NodeKind::Fields, 2);
                b.patch(0, MemberKind::NonStaticField as u32);
                b.begin(NodeKind::VariablesDeclaration, 3);
                b.patch(0, 4);
                b.begin(NodeKind::InitializedIdentifier, 5);
                b.begin(NodeKind::VariableInitializer, 6);
                b.leaf(NodeKind::LiteralInt, 7);
                b.end(NodeKind::VariableInitializer, 8);
                b.end(NodeKind::InitializedIdentifier, 8);
                b.end(NodeKind::VariablesDeclaration, 9);
                b.end(NodeKind::Fields, 9);
                b.end(NodeKind::DeclarationBody, 10);
                b.end(NodeKind::ClassDeclaration, 10);
                b.end(NodeKind::TopLevelDeclaration, 10);
            }),
        );
        assert!(f.run(&one_rule("unnecessary_late")).is_empty());
    }

    /// The static-field face through the production bridge: the bridge's
    /// `Fields` word 0 is the `MemberKind` discriminant and the `late`
    /// head contract holds on real parses; the instance face on the same
    /// parse path stays silent.
    #[test]
    fn unnecessary_late_static_field_reports_on_real_parse() {
        let f = Fixture::parse("class A {\n  static late var x = 1;\n}\n");
        let lints = f.run(&one_rule("unnecessary_late"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "Unnecessary 'late' modifier.");
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "late", 0))
        );

        let f = Fixture::parse("class A {\n  late var x = 1;\n}\n");
        assert!(f.run(&one_rule("unnecessary_late")).is_empty());
    }

    /// Static fields report (`unnecessary_late.dart:36-45`).
    #[test]
    fn unnecessary_late_static_field_reports() {
        let source = "class A { late var x = 1; }";
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelDeclaration, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::ClassDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.begin(NodeKind::DeclarationBody, 2);
                b.begin(NodeKind::Fields, 2);
                b.patch(0, MemberKind::StaticField as u32);
                b.begin(NodeKind::VariablesDeclaration, 3);
                b.patch(0, 4);
                b.begin(NodeKind::InitializedIdentifier, 5);
                b.begin(NodeKind::VariableInitializer, 6);
                b.leaf(NodeKind::LiteralInt, 7);
                b.end(NodeKind::VariableInitializer, 8);
                b.end(NodeKind::InitializedIdentifier, 8);
                b.end(NodeKind::VariablesDeclaration, 9);
                b.end(NodeKind::Fields, 9);
                b.end(NodeKind::DeclarationBody, 10);
                b.end(NodeKind::ClassDeclaration, 10);
                b.end(NodeKind::TopLevelDeclaration, 10);
            }),
        );
        let lints = f.run(&one_rule("unnecessary_late"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, 3));
    }

    /// `test/rules/use_function_type_syntax_for_parameters_test.dart`
    /// `test_functionTypedParameter`.
    #[test]
    fn function_type_syntax_reports_the_old_form() {
        let source = "f(void g()) {}";
        // f0 (1 void2 g3 (4 )5 )6 {7 }8
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, NONE);
                b.leaf(NodeKind::NoType, 0);
                identifier(b, 0, 1);
                b.begin(NodeKind::FormalParameters, 1);
                b.begin(NodeKind::FunctionTypedFormalParameter, 2);
                b.patch(0, NONE);
                b.leaf_span(NodeKind::Type, 2, 3);
                identifier(b, 3, 4);
                b.begin(NodeKind::FormalParameters, 4);
                b.end(NodeKind::FormalParameters, 6);
                b.end(NodeKind::FunctionTypedFormalParameter, 6);
                b.end(NodeKind::FormalParameters, 7);
                b.begin(NodeKind::Block, 7);
                b.end(NodeKind::Block, 9);
                b.end(NodeKind::TopLevelMethod, 9);
            }),
        );
        let lints = f.run(&one_rule("use_function_type_syntax_for_parameters"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Use the generic function type syntax to declare the parameter 'g'."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 2));
    }

    #[test]
    fn function_type_syntax_generic_form_passes() {
        let source = "f(void Function() g) {}";
        // f0 (1 void2 Function3 (4 )5 g6 )7 {8 }9
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, NONE);
                b.leaf(NodeKind::NoType, 0);
                identifier(b, 0, 1);
                b.begin(NodeKind::FormalParameters, 1);
                b.begin(NodeKind::FormalParameter, 6);
                b.patch(0, NONE);
                b.patch(1, 6);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                b.leaf_span(NodeKind::Type, 2, 6);
                b.end(NodeKind::FormalParameter, 7);
                b.end(NodeKind::FormalParameters, 8);
                b.begin(NodeKind::Block, 8);
                b.end(NodeKind::Block, 10);
                b.end(NodeKind::TopLevelMethod, 10);
            }),
        );
        assert!(
            f.run(&one_rule("use_function_type_syntax_for_parameters"))
                .is_empty()
        );
    }

    /// `test/rules/prefer_typing_uninitialized_variables_test.dart`
    /// `test_untypedLocalWithoutInitializer`.
    #[test]
    fn prefer_typing_reports_an_untyped_local() {
        let source = "var x;";
        // var0 x1 ;2
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::Block, 0);
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, 0);
                b.leaf(NodeKind::NoType, 0);
                b.begin(NodeKind::InitializedIdentifier, 1);
                b.end(NodeKind::InitializedIdentifier, 2);
                b.end(NodeKind::VariablesDeclaration, 2);
                b.end(NodeKind::Block, 3);
            }),
        );
        let lints = f.run(&one_rule("prefer_typing_uninitialized_variables"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "An uninitialized variable should have an explicit type annotation."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 1));
    }

    /// Fields carry the second official message variant
    /// (`diagnostic.g.dart` `preferTypingUninitializedVariablesForField`).
    #[test]
    fn prefer_typing_reports_an_untyped_field_with_field_text() {
        let source = "class A { var x; }";
        // class0 A1 {2 var3 x4 ;5 }6
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelDeclaration, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::ClassDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.begin(NodeKind::DeclarationBody, 2);
                b.begin(NodeKind::Fields, 2);
                b.patch(0, MemberKind::NonStaticField as u32);
                b.begin(NodeKind::VariablesDeclaration, 3);
                b.patch(0, 3);
                b.leaf(NodeKind::NoType, 3);
                b.begin(NodeKind::InitializedIdentifier, 4);
                b.end(NodeKind::InitializedIdentifier, 5);
                b.end(NodeKind::VariablesDeclaration, 6);
                b.end(NodeKind::Fields, 6);
                b.end(NodeKind::DeclarationBody, 7);
                b.end(NodeKind::ClassDeclaration, 7);
                b.end(NodeKind::TopLevelDeclaration, 7);
            }),
        );
        let lints = f.run(&one_rule("prefer_typing_uninitialized_variables"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "An uninitialized field should have an explicit type annotation."
        );
    }

    #[test]
    fn prefer_typing_typed_declaration_passes() {
        let source = "int x;";
        // int0 x1 ;2
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::Block, 0);
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, NONE);
                b.leaf_span(NodeKind::Type, 0, 1);
                b.begin(NodeKind::InitializedIdentifier, 1);
                b.end(NodeKind::InitializedIdentifier, 2);
                b.end(NodeKind::VariablesDeclaration, 2);
                b.end(NodeKind::Block, 3);
            }),
        );
        assert!(
            f.run(&one_rule("prefer_typing_uninitialized_variables"))
                .is_empty()
        );
    }

    #[test]
    fn prefer_typing_initialized_variable_passes() {
        let source = "var x = 1;";
        // var0 x1 =2 13 ;4
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::Block, 0);
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, 0);
                b.leaf(NodeKind::NoType, 0);
                b.begin(NodeKind::InitializedIdentifier, 1);
                b.begin(NodeKind::VariableInitializer, 2);
                b.leaf(NodeKind::LiteralInt, 3);
                b.end(NodeKind::VariableInitializer, 4);
                b.end(NodeKind::InitializedIdentifier, 4);
                b.end(NodeKind::VariablesDeclaration, 5);
                b.end(NodeKind::Block, 5);
            }),
        );
        assert!(
            f.run(&one_rule("prefer_typing_uninitialized_variables"))
                .is_empty()
        );
    }

    // —— prefer_generic_function_type_aliases —————————————————————————————

    /// `test/rules/prefer_generic_function_type_aliases_test.dart`
    /// `test_typedef_genericFunctionType` — the rewrite is spelled in the
    /// correction.
    #[test]
    fn generic_function_alias_reports_the_old_form() {
        let source = "typedef int F(int x);";
        // typedef0 int1 F2 (3 int4 x5 )6 ;7
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::Typedef, 0);
                b.patch(0, tok(source, t, "typedef", 0));
                b.patch(1, NONE); // old form: no `=`
                b.leaf_span(NodeKind::Type, 1, 2);
                identifier(b, 2, 3);
                b.begin(NodeKind::FormalParameters, 3);
                b.end(NodeKind::FormalParameters, 7);
                b.end(NodeKind::Typedef, 8);
            }),
        );
        let lints = f.run(&one_rule("prefer_generic_function_type_aliases"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Use the generic function type syntax in 'typedef's."
        );
        assert_eq!(
            lints[0].correction,
            Some("Try using the generic function type syntax ('int Function(int x)').".to_owned())
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 2));
    }

    /// A missing return type contributes nothing to the rewrite
    /// (`returnType == null` → `''`).
    #[test]
    fn generic_function_alias_without_return_type() {
        let source = "typedef F(int x);";
        // typedef0 F1 (2 int3 x4 )5 ;6
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::Typedef, 0);
                b.patch(0, tok(source, t, "typedef", 0));
                b.patch(1, NONE);
                b.leaf(NodeKind::NoType, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::FormalParameters, 2);
                b.end(NodeKind::FormalParameters, 6);
                b.end(NodeKind::Typedef, 7);
            }),
        );
        let lints = f.run(&one_rule("prefer_generic_function_type_aliases"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].correction,
            Some("Try using the generic function type syntax ('Function(int x)').".to_owned())
        );
    }

    /// Type parameters carry into the rewrite after `Function`.
    #[test]
    fn generic_function_alias_with_type_parameters() {
        let source = "typedef F<T>(T x);";
        // typedef0 F1 <2 T3 >4 (5 T6 x7 )8 ;9
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::Typedef, 0);
                b.patch(0, tok(source, t, "typedef", 0));
                b.patch(1, NONE);
                identifier(b, 1, 2);
                b.leaf_span(NodeKind::TypeVariables, 2, 5);
                b.begin(NodeKind::FormalParameters, 5);
                b.end(NodeKind::FormalParameters, 9);
                b.end(NodeKind::Typedef, 10);
            }),
        );
        let lints = f.run(&one_rule("prefer_generic_function_type_aliases"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].correction,
            Some("Try using the generic function type syntax ('Function<T>(T x)').".to_owned())
        );
    }

    /// The `=` form is the recommended syntax — it never dispatches.
    #[test]
    fn generic_function_alias_new_form_passes() {
        let source = "typedef F = void Function();";
        // typedef0 F1 =2 void3 Function4 (5 )6 ;7
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::Typedef, 0);
                b.patch(0, tok(source, t, "typedef", 0));
                b.patch(1, tok(source, t, "=", 0));
                identifier(b, 1, 2);
                b.leaf_span(NodeKind::Type, 3, 7);
                b.end(NodeKind::Typedef, 8);
            }),
        );
        assert!(
            f.run(&one_rule("prefer_generic_function_type_aliases"))
                .is_empty()
        );
    }

    // —— unnecessary_constructor_name —————————————————————————————————————

    /// `test/rules/unnecessary_constructor_name_test.dart` `test_withNew`.
    #[test]
    fn constructor_name_reports_new() {
        let f = Fixture::parse("class A { A.new(); }");
        let lints = f.run(&one_rule("unnecessary_constructor_name"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "Unnecessary '.new' constructor name.");
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "new", 0))
        );
    }

    /// An unnamed constructor has no name token to check; a differently
    /// named constructor is not `.new`.
    #[test]
    fn constructor_name_unnamed_and_named_other_pass() {
        let f = Fixture::parse("class A { A(); }");
        assert!(f.run(&one_rule("unnecessary_constructor_name")).is_empty());

        let f = Fixture::parse("class A { A.named(); }");
        assert!(f.run(&one_rule("unnecessary_constructor_name")).is_empty());
    }

    /// Duplicate unnamed constructors already error (:45-52): with a
    /// second unnamed (or `new`-spelled) constructor the bail fires.
    /// Non-constructor members never count toward the bail — the flagged
    /// count divergence (a field sibling no longer silences the report).
    #[test]
    fn constructor_name_duplicate_unnamed_bails() {
        let f = Fixture::parse("class A { A(); A.new(); }");
        assert!(
            f.run(&one_rule("unnecessary_constructor_name")).is_empty(),
            "two constructors named `new` — the duplicate error owns it"
        );

        let f = Fixture::parse("class A { int a = 0; A.new(); }");
        assert_eq!(
            f.run(&one_rule("unnecessary_constructor_name")).len(),
            1,
            "a field sibling is not an unnamed constructor"
        );
    }

    /// `Foo.new(1)` reports at the `new` name token through the production
    /// bridge (the creation face reads the owner slice: the `.new` member
    /// invocation over a bare receiver, `rules::slice`).
    #[test]
    fn creation_face_reports_dot_new() {
        let f = Fixture::parse("void f() { Foo.new(1); }\n");
        let lints = f.run(&one_rule("unnecessary_constructor_name"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "new", 0))
        );
    }

    /// A constructor tear-off (`Foo.new` without the invocation) is not an
    /// instance creation, and a chained receiver fails the bare-receiver
    /// group check — the documented conservative family.
    #[test]
    fn creation_face_tearoff_and_chained_pass() {
        let f = Fixture::parse("void f() { g(Foo.new); }\n");
        assert!(f.run(&one_rule("unnecessary_constructor_name")).is_empty());

        let f = Fixture::parse("void f() { a.b.new(1); }\n");
        assert!(f.run(&one_rule("unnecessary_constructor_name")).is_empty());
    }

    /// The primary-constructor face (:70-80): a `.new` primary name
    /// reports; the unnamed primary has no name token.
    #[test]
    fn constructor_name_primary_faces() {
        let f = Fixture::parse("extension type E.new(int x) {}");
        let lints = f.run(&one_rule("unnecessary_constructor_name"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "new", 0))
        );

        let f = Fixture::parse("class C.new(int x);");
        assert_eq!(f.run(&one_rule("unnecessary_constructor_name")).len(), 1);

        let f = Fixture::parse("extension type E(int x) {}");
        assert!(f.run(&one_rule("unnecessary_constructor_name")).is_empty());

        let f = Fixture::parse("extension type E.named(int x) {}");
        assert!(f.run(&one_rule("unnecessary_constructor_name")).is_empty());
    }

    /// The extension-type exemption (:51-58): a secondary constructor
    /// `.new` inside an extension type whose primary constructor is
    /// unnamed stays — the qualified form is necessary there.
    #[test]
    fn constructor_name_extension_type_unnamed_primary_bails() {
        let f = Fixture::parse("extension type E(int x) { E.other(); E.new(); }");
        assert!(f.run(&one_rule("unnecessary_constructor_name")).is_empty());
    }

    /// The instance-creation face: `A.new()` (empty argument list) reports
    /// at `.new`'s name, through the production bridge (the owner slice
    /// reads the bare-receiver tear-off group behind the `.new` name).
    #[test]
    fn constructor_name_creation_reports_new() {
        let f = Fixture::parse("void f() { A.new(); }\n");
        let lints = f.run(&one_rule("unnecessary_constructor_name"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "new", 0))
        );
    }

    // —— hash_and_equals ——————————————————————————————————————————————————

    /// `test/rules/hash_and_equals_test.dart` `test_onlyEquals` — the
    /// missing `hashCode` reports at `==`.
    #[test]
    fn hash_and_equals_reports_missing_hash_code() {
        let source = "class A { bool operator ==(o) => true; }";
        // class0 A1 {2 bool3 operator4 ==5 (6 o7 )8 =>9 true10 ;11 }12
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelDeclaration, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::ClassDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.begin(NodeKind::DeclarationBody, 2);
                b.begin(NodeKind::Method, 3);
                b.patch(0, MemberKind::NonStaticMethod as u32);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.leaf_span(NodeKind::Type, 3, 4);
                b.begin(NodeKind::OperatorName, 4);
                b.patch(0, 5);
                b.patch(1, 0);
                b.end(NodeKind::OperatorName, 6);
                b.begin(NodeKind::FormalParameters, 6);
                b.end(NodeKind::FormalParameters, 9);
                b.begin(NodeKind::ExpressionBody, 9);
                b.patch(0, 9);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.leaf(NodeKind::LiteralBool, 10);
                b.end(NodeKind::ExpressionBody, 12);
                b.end(NodeKind::Method, 12);
                b.end(NodeKind::DeclarationBody, 13);
                b.end(NodeKind::ClassDeclaration, 13);
                b.end(NodeKind::TopLevelDeclaration, 13);
            }),
        );
        let lints = f.run(&one_rule("hash_and_equals"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Missing a corresponding override of 'hashCode'."
        );
        assert_eq!(
            lints[0].correction,
            Some("Try overriding 'hashCode' or removing '=='.".to_owned())
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 5));
    }

    /// `test_onlyHashCode` — the getter form reports at the name.
    #[test]
    fn hash_and_equals_reports_missing_equals() {
        let source = "class A { int get hashCode => 0; }";
        // class0 A1 {2 int3 get4 hashCode5 =>6 07 ;8 }9
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::TopLevelDeclaration, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::ClassDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.begin(NodeKind::DeclarationBody, 2);
                b.begin(NodeKind::Method, 3);
                b.patch(0, MemberKind::NonStaticMethod as u32);
                b.patch(1, tok(source, t, "get", 0));
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.leaf_span(NodeKind::Type, 3, 4);
                identifier(b, 5, 6);
                b.begin(NodeKind::ExpressionBody, 6);
                b.patch(0, 6);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.leaf(NodeKind::LiteralInt, 7);
                b.end(NodeKind::ExpressionBody, 9);
                b.end(NodeKind::Method, 9);
                b.end(NodeKind::DeclarationBody, 10);
                b.end(NodeKind::ClassDeclaration, 10);
                b.end(NodeKind::TopLevelDeclaration, 10);
            }),
        );
        let lints = f.run(&one_rule("hash_and_equals"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Missing a corresponding override of '=='."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 5));
    }

    /// `test_onlyGetHashCodeDeclaration` — the field form reports at the
    /// variable name (the official `getFieldName` fallback).
    #[test]
    fn hash_and_equals_field_form_reports() {
        let source = "class A { final hashCode = 0; }";
        // class0 A1 {2 final3 hashCode4 =5 06 ;7 }8
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelDeclaration, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::ClassDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.begin(NodeKind::DeclarationBody, 2);
                b.begin(NodeKind::Fields, 2);
                b.patch(0, MemberKind::NonStaticField as u32);
                b.begin(NodeKind::VariablesDeclaration, 3);
                b.patch(0, 3);
                b.begin(NodeKind::InitializedIdentifier, 4);
                b.begin(NodeKind::VariableInitializer, 5);
                b.leaf(NodeKind::LiteralInt, 6);
                b.end(NodeKind::VariableInitializer, 7);
                b.end(NodeKind::InitializedIdentifier, 7);
                b.end(NodeKind::VariablesDeclaration, 8);
                b.end(NodeKind::Fields, 8);
                b.end(NodeKind::DeclarationBody, 9);
                b.end(NodeKind::ClassDeclaration, 9);
                b.end(NodeKind::TopLevelDeclaration, 9);
            }),
        );
        let lints = f.run(&one_rule("hash_and_equals"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, 4));
    }

    #[test]
    fn hash_and_equals_both_present_passes() {
        let source = "class A { bool operator ==(o) => true; int get hashCode => 0; }";
        // class0 A1 {2 bool3 operator4 ==5 (6 o7 )8 =>9 true10 ;11 int12
        // get13 hashCode14 =>15 016 ;17 }18
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::TopLevelDeclaration, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::ClassDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.begin(NodeKind::DeclarationBody, 2);
                b.begin(NodeKind::Method, 3);
                b.patch(0, MemberKind::NonStaticMethod as u32);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.leaf_span(NodeKind::Type, 3, 4);
                b.begin(NodeKind::OperatorName, 4);
                b.patch(0, 5);
                b.patch(1, 0);
                b.end(NodeKind::OperatorName, 6);
                b.begin(NodeKind::FormalParameters, 6);
                b.end(NodeKind::FormalParameters, 9);
                b.begin(NodeKind::ExpressionBody, 9);
                b.patch(0, 9);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.leaf(NodeKind::LiteralBool, 10);
                b.end(NodeKind::ExpressionBody, 12);
                b.end(NodeKind::Method, 12);
                b.begin(NodeKind::Method, 12);
                b.patch(0, MemberKind::NonStaticMethod as u32);
                b.patch(1, tok(source, t, "get", 0));
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.leaf_span(NodeKind::Type, 12, 13);
                identifier(b, 14, 15);
                b.begin(NodeKind::ExpressionBody, 15);
                b.patch(0, 15);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.leaf(NodeKind::LiteralInt, 16);
                b.end(NodeKind::ExpressionBody, 18);
                b.end(NodeKind::Method, 18);
                b.end(NodeKind::DeclarationBody, 19);
                b.end(NodeKind::ClassDeclaration, 19);
                b.end(NodeKind::TopLevelDeclaration, 19);
            }),
        );
        assert!(f.run(&one_rule("hash_and_equals")).is_empty());
    }

    #[test]
    fn hash_and_equals_neither_present_passes() {
        let source = "class A { int x = 0; }";
        // class0 A1 {2 int3 x4 =5 06 ;7 }8
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelDeclaration, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::ClassDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.begin(NodeKind::DeclarationBody, 2);
                b.begin(NodeKind::Fields, 2);
                b.patch(0, MemberKind::NonStaticField as u32);
                b.begin(NodeKind::VariablesDeclaration, 3);
                b.patch(0, 3);
                b.leaf_span(NodeKind::Type, 3, 4);
                b.begin(NodeKind::InitializedIdentifier, 4);
                b.begin(NodeKind::VariableInitializer, 5);
                b.leaf(NodeKind::LiteralInt, 6);
                b.end(NodeKind::VariableInitializer, 7);
                b.end(NodeKind::InitializedIdentifier, 7);
                b.end(NodeKind::VariablesDeclaration, 8);
                b.end(NodeKind::Fields, 8);
                b.end(NodeKind::DeclarationBody, 9);
                b.end(NodeKind::ClassDeclaration, 9);
                b.end(NodeKind::TopLevelDeclaration, 9);
            }),
        );
        assert!(f.run(&one_rule("hash_and_equals")).is_empty());
    }

    // —— avoid_init_to_null ———————————————————————————————————————————————

    /// `test/rules/avoid_init_to_null_test.dart` `test_varInitializeNull` —
    /// a `var x = null` local reports at the declaration.
    #[test]
    fn init_to_null_reports_a_local() {
        let source = "var x = null;";
        // var0 x1 =2 null3 ;4
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::Block, 0);
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, tok(source, t, "var", 0));
                b.begin(NodeKind::InitializedIdentifier, 1);
                b.begin(NodeKind::VariableInitializer, 2);
                b.leaf(NodeKind::LiteralNull, 3);
                b.end(NodeKind::VariableInitializer, 5);
                b.end(NodeKind::InitializedIdentifier, 5);
                b.end(NodeKind::VariablesDeclaration, 5);
                b.end(NodeKind::Block, 5);
            }),
        );
        let lints = f.run(&one_rule("avoid_init_to_null"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "Redundant initialization to 'null'.");
        assert_eq!(
            lints[0].correction,
            Some("Try removing the initializer.".to_owned())
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 1));
    }

    /// A typed variable reports; `final`/`const` declarations are exempt
    /// (`!node.isConst && !node.isFinal`, :62-63).
    #[test]
    fn init_to_null_final_and_const_pass() {
        let typed = "int? x = null;";
        // int0 ?1 x2 =3 null4 ;5
        let f = Fixture::build(
            typed,
            unit_of(|b, _t| {
                b.begin(NodeKind::Block, 0);
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, NONE);
                b.leaf_span(NodeKind::Type, 0, 2);
                b.begin(NodeKind::InitializedIdentifier, 2);
                b.begin(NodeKind::VariableInitializer, 3);
                b.leaf(NodeKind::LiteralNull, 4);
                b.end(NodeKind::VariableInitializer, 6);
                b.end(NodeKind::InitializedIdentifier, 6);
                b.end(NodeKind::VariablesDeclaration, 6);
                b.end(NodeKind::Block, 6);
            }),
        );
        assert_eq!(f.run(&one_rule("avoid_init_to_null")).len(), 1);

        for keyword in ["final", "const"] {
            let source = format!("{keyword} x = null;");
            // final0 x1 =2 null3 ;4
            let f = Fixture::build(
                &source,
                unit_of(|b, t| {
                    b.begin(NodeKind::Block, 0);
                    b.begin(NodeKind::VariablesDeclaration, 0);
                    b.patch(0, tok(&source, t, keyword, 0));
                    b.begin(NodeKind::InitializedIdentifier, 1);
                    b.begin(NodeKind::VariableInitializer, 2);
                    b.leaf(NodeKind::LiteralNull, 3);
                    b.end(NodeKind::VariableInitializer, 5);
                    b.end(NodeKind::InitializedIdentifier, 5);
                    b.end(NodeKind::VariablesDeclaration, 5);
                    b.end(NodeKind::Block, 5);
                }),
            );
            assert!(
                f.run(&one_rule("avoid_init_to_null")).is_empty(),
                "{source}"
            );
        }
    }

    /// A field reports (`test_defaultNull` family).
    #[test]
    fn init_to_null_reports_a_field() {
        let source = "class A { var x = null; }";
        // class0 A1 {2 var3 x4 =5 null6 ;7 }8
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::TopLevelDeclaration, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::ClassDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.begin(NodeKind::DeclarationBody, 2);
                b.begin(NodeKind::Fields, 2);
                b.patch(0, MemberKind::NonStaticField as u32);
                b.begin(NodeKind::VariablesDeclaration, 3);
                b.patch(0, tok(source, t, "var", 0));
                b.begin(NodeKind::InitializedIdentifier, 4);
                b.begin(NodeKind::VariableInitializer, 5);
                b.leaf(NodeKind::LiteralNull, 6);
                b.end(NodeKind::VariableInitializer, 8);
                b.end(NodeKind::InitializedIdentifier, 8);
                b.end(NodeKind::VariablesDeclaration, 8);
                b.end(NodeKind::Fields, 8);
                b.end(NodeKind::DeclarationBody, 9);
                b.end(NodeKind::ClassDeclaration, 9);
                b.end(NodeKind::TopLevelDeclaration, 9);
            }),
        );
        assert_eq!(f.run(&one_rule("avoid_init_to_null")).len(), 1);
    }

    /// The parameter default-value face: `{int x = null}` reports, a
    /// non-null default does not.
    #[test]
    fn init_to_null_parameter_default() {
        let null_default = "f({int x = null}) {}";
        // f0 (1 {2 int3 x4 =5 null6 }7 )8 {9 }10
        let f = Fixture::build(
            null_default,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, NONE);
                b.leaf(NodeKind::NoType, 0);
                identifier(b, 0, 1);
                b.begin(NodeKind::FormalParameters, 1);
                b.begin(NodeKind::OptionalFormalParameters, 2);
                b.begin(NodeKind::FormalParameter, 3);
                b.patch(0, NONE);
                b.patch(1, 4);
                b.patch(2, 6);
                b.patch(3, 7);
                b.patch(4, 0);
                b.leaf_span(NodeKind::Type, 3, 4);
                b.end(NodeKind::FormalParameter, 7);
                b.end(NodeKind::OptionalFormalParameters, 8);
                b.end(NodeKind::FormalParameters, 9);
                b.begin(NodeKind::Block, 9);
                b.end(NodeKind::Block, 11);
                b.end(NodeKind::TopLevelMethod, 11);
            }),
        );
        let lints = f.run(&one_rule("avoid_init_to_null"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, 3));

        let int_default = "f({int x = 1}) {}";
        // f0 (1 {2 int3 x4 =5 16 }7 )8 {9 }10
        let f = Fixture::build(
            int_default,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, NONE);
                b.leaf(NodeKind::NoType, 0);
                identifier(b, 0, 1);
                b.begin(NodeKind::FormalParameters, 1);
                b.begin(NodeKind::OptionalFormalParameters, 2);
                b.begin(NodeKind::FormalParameter, 3);
                b.patch(0, NONE);
                b.patch(1, 4);
                b.patch(2, 6);
                b.patch(3, 7);
                b.patch(4, 0);
                b.leaf_span(NodeKind::Type, 3, 4);
                b.end(NodeKind::FormalParameter, 7);
                b.end(NodeKind::OptionalFormalParameters, 8);
                b.end(NodeKind::FormalParameters, 9);
                b.begin(NodeKind::Block, 9);
                b.end(NodeKind::Block, 11);
                b.end(NodeKind::TopLevelMethod, 11);
            }),
        );
        assert!(f.run(&one_rule("avoid_init_to_null")).is_empty());
    }

    /// A non-null initializer never reports.
    #[test]
    fn init_to_non_null_passes() {
        let source = "var x = 1;";
        // var0 x1 =2 13 ;4
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::Block, 0);
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, tok(source, t, "var", 0));
                b.begin(NodeKind::InitializedIdentifier, 1);
                b.begin(NodeKind::VariableInitializer, 2);
                b.leaf(NodeKind::LiteralInt, 3);
                b.end(NodeKind::VariableInitializer, 5);
                b.end(NodeKind::InitializedIdentifier, 5);
                b.end(NodeKind::VariablesDeclaration, 5);
                b.end(NodeKind::Block, 5);
            }),
        );
        assert!(f.run(&one_rule("avoid_init_to_null")).is_empty());
    }

    // —— prefer_function_declarations_over_variables ——————————————————————

    /// `test/rules/prefer_function_declarations_over_variables_test.dart`
    /// `test_local` — a final local initialized with a closure reports.
    #[test]
    fn function_variable_local_reports() {
        let source = "final f = () {};";
        // final0 f1 =2 (3 )4 {5 }6 ;7
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::Block, 0);
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, tok(source, t, "final", 0));
                b.begin(NodeKind::InitializedIdentifier, 1);
                b.begin(NodeKind::VariableInitializer, 2);
                b.begin(NodeKind::FunctionExpression, 3);
                b.begin(NodeKind::FormalParameters, 3);
                b.end(NodeKind::FormalParameters, 5);
                b.begin(NodeKind::Block, 5);
                b.end(NodeKind::Block, 7);
                b.end(NodeKind::FunctionExpression, 7);
                b.end(NodeKind::VariableInitializer, 7);
                b.end(NodeKind::InitializedIdentifier, 7);
                b.end(NodeKind::VariablesDeclaration, 8);
                b.end(NodeKind::Block, 8);
            }),
        );
        let lints = f.run(&one_rule("prefer_function_declarations_over_variables"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Use a function declaration rather than a variable assignment to bind a function to a name."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 1));
    }

    /// A non-closure initializer passes.
    #[test]
    fn function_variable_non_closure_passes() {
        let source = "final f = 1;";
        // final0 f1 =2 13 ;4
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::Block, 0);
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, tok(source, t, "final", 0));
                b.begin(NodeKind::InitializedIdentifier, 1);
                b.begin(NodeKind::VariableInitializer, 2);
                b.leaf(NodeKind::LiteralInt, 3);
                b.end(NodeKind::VariableInitializer, 5);
                b.end(NodeKind::InitializedIdentifier, 5);
                b.end(NodeKind::VariablesDeclaration, 5);
                b.end(NodeKind::Block, 5);
            }),
        );
        assert!(
            f.run(&one_rule("prefer_function_declarations_over_variables"))
                .is_empty()
        );
    }

    /// Fields and top-level variables report only when final (:42-49).
    #[test]
    fn function_variable_fields_require_final() {
        let field = |keyword: &str| {
            let source = format!("class A {{ {keyword} f = () {{}}; }}");
            // class0 A1 {2 var3 f4 =5 (6 )7 {8 }9 ;10 }11
            Fixture::build(
                &source,
                unit_of(|b, t| {
                    b.begin(NodeKind::TopLevelDeclaration, 0);
                    identifier(b, 1, 2);
                    b.begin(NodeKind::ClassDeclaration, 0);
                    b.patch(0, 0);
                    b.patch(1, NONE);
                    b.patch(2, NONE);
                    b.patch(3, NONE);
                    b.begin(NodeKind::DeclarationBody, 2);
                    b.begin(NodeKind::Fields, 2);
                    b.patch(0, MemberKind::NonStaticField as u32);
                    b.begin(NodeKind::VariablesDeclaration, 3);
                    b.patch(0, tok(&source, t, keyword, 0));
                    b.begin(NodeKind::InitializedIdentifier, 4);
                    b.begin(NodeKind::VariableInitializer, 5);
                    b.begin(NodeKind::FunctionExpression, 6);
                    b.begin(NodeKind::FormalParameters, 6);
                    b.end(NodeKind::FormalParameters, 8);
                    b.begin(NodeKind::Block, 8);
                    b.end(NodeKind::Block, 10);
                    b.end(NodeKind::FunctionExpression, 10);
                    b.end(NodeKind::VariableInitializer, 10);
                    b.end(NodeKind::InitializedIdentifier, 10);
                    b.end(NodeKind::VariablesDeclaration, 11);
                    b.end(NodeKind::Fields, 11);
                    b.end(NodeKind::DeclarationBody, 12);
                    b.end(NodeKind::ClassDeclaration, 12);
                    b.end(NodeKind::TopLevelDeclaration, 12);
                }),
            )
        };
        assert_eq!(
            field("final")
                .run(&one_rule("prefer_function_declarations_over_variables"))
                .len(),
            1
        );
        assert!(
            field("var")
                .run(&one_rule("prefer_function_declarations_over_variables"))
                .is_empty()
        );

        // Top-level: final reports.
        let top = "final f = () {};";
        // final0 f1 =2 (3 )4 {5 }6 ;7
        let f = Fixture::build(
            top,
            unit_of(|b, t| {
                b.begin(NodeKind::TopLevelFields, 0);
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, tok(top, t, "final", 0));
                b.begin(NodeKind::InitializedIdentifier, 1);
                b.begin(NodeKind::VariableInitializer, 2);
                b.begin(NodeKind::FunctionExpression, 3);
                b.begin(NodeKind::FormalParameters, 3);
                b.end(NodeKind::FormalParameters, 5);
                b.begin(NodeKind::Block, 5);
                b.end(NodeKind::Block, 7);
                b.end(NodeKind::FunctionExpression, 7);
                b.end(NodeKind::VariableInitializer, 7);
                b.end(NodeKind::InitializedIdentifier, 7);
                b.end(NodeKind::VariablesDeclaration, 8);
                b.end(NodeKind::TopLevelFields, 8);
            }),
        );
        assert_eq!(
            f.run(&one_rule("prefer_function_declarations_over_variables"))
                .len(),
            1
        );
    }

    // —— type_init_formals —————————————————————————————————————————————————

    /// `test/rules/type_init_formals_test.dart` `test_optionalParameters` —
    /// a typed `this.x` formal reports at the type.
    #[test]
    fn typed_initializing_formal_reports() {
        let source = "class C { C(int this.x); }";
        // class0 C1 {2 C3 (4 int5 this6 .7 x8 )9 ;10 }11
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelDeclaration, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::ClassDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.begin(NodeKind::DeclarationBody, 2);
                b.begin(NodeKind::Constructor, 3);
                b.patch(0, MemberKind::NonStaticMethod as u32);
                b.patch(1, NONE);
                b.patch(2, 4);
                b.patch(3, NONE);
                identifier(b, 3, 4);
                b.begin(NodeKind::FormalParameters, 4);
                b.begin(NodeKind::FormalParameter, 5);
                b.patch(0, 6); // the `this` receiver
                b.patch(1, 8);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                b.leaf_span(NodeKind::Type, 5, 6);
                b.end(NodeKind::FormalParameter, 9);
                b.end(NodeKind::FormalParameters, 10);
                b.end(NodeKind::Constructor, 11);
                b.end(NodeKind::DeclarationBody, 12);
                b.end(NodeKind::ClassDeclaration, 12);
                b.end(NodeKind::TopLevelDeclaration, 12);
            }),
        );
        let lints = f.run(&one_rule("type_init_formals"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Don't needlessly type annotate initializing formals."
        );
        assert_eq!(
            lints[0].correction,
            Some("Try removing the type.".to_owned())
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 5));
    }

    /// An untyped initializing formal and a typed plain formal pass
    /// (`node.type == null` / not an initializing formal).
    #[test]
    fn untyped_and_plain_formals_pass() {
        let untyped = "class C { C(this.x); }";
        // class0 C1 {2 C3 (4 this5 .6 x7 )8 ;9 }10
        let f = Fixture::build(
            untyped,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelDeclaration, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::ClassDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.begin(NodeKind::DeclarationBody, 2);
                b.begin(NodeKind::Constructor, 3);
                b.patch(0, MemberKind::NonStaticMethod as u32);
                b.patch(1, NONE);
                b.patch(2, 4);
                b.patch(3, NONE);
                identifier(b, 3, 4);
                b.begin(NodeKind::FormalParameters, 4);
                b.begin(NodeKind::FormalParameter, 5);
                b.patch(0, 5); // the `this` receiver
                b.patch(1, 7);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                b.end(NodeKind::FormalParameter, 8);
                b.end(NodeKind::FormalParameters, 9);
                b.end(NodeKind::Constructor, 10);
                b.end(NodeKind::DeclarationBody, 11);
                b.end(NodeKind::ClassDeclaration, 11);
                b.end(NodeKind::TopLevelDeclaration, 11);
            }),
        );
        assert!(f.run(&one_rule("type_init_formals")).is_empty());

        let plain = "f(int x) {}";
        // f0 (1 int2 x3 )4 {5 }6
        let f = Fixture::build(
            plain,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, NONE);
                b.leaf(NodeKind::NoType, 0);
                identifier(b, 0, 1);
                b.begin(NodeKind::FormalParameters, 1);
                b.begin(NodeKind::FormalParameter, 2);
                b.patch(0, NONE);
                b.patch(1, 3);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                b.leaf_span(NodeKind::Type, 2, 3);
                b.end(NodeKind::FormalParameter, 4);
                b.end(NodeKind::FormalParameters, 5);
                b.begin(NodeKind::Block, 5);
                b.end(NodeKind::Block, 7);
                b.end(NodeKind::TopLevelMethod, 7);
            }),
        );
        assert!(f.run(&one_rule("type_init_formals")).is_empty());
    }

    // —— recursive_getters —————————————————————————————————————————————————

    /// `test/rules/recursive_getters_test.dart` `test_recursiveGetter` — a
    /// getter returning its own name reports at the reference.
    #[test]
    fn recursive_getter_reports_the_self_reference() {
        let source = "int get x => x;";
        // int0 get1 x2 =>3 x4 ;5
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, tok(source, t, "get", 0));
                b.leaf_span(NodeKind::Type, 0, 1);
                identifier(b, 2, 3);
                b.begin(NodeKind::ExpressionBody, 3);
                b.patch(0, 3);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                identifier(b, 4, 5);
                b.end(NodeKind::ExpressionBody, 6);
                b.end(NodeKind::TopLevelMethod, 6);
            }),
        );
        let lints = f.run(&one_rule("recursive_getters"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "The getter 'x' recursively returns itself."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 4));
    }

    /// A different name passes, and a property access on another target is
    /// never a self-reference (`isSelfReference`, :57-64).
    #[test]
    fn recursive_getter_other_names_pass() {
        let source = "int get x => y;";
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, tok(source, t, "get", 0));
                b.leaf_span(NodeKind::Type, 0, 1);
                identifier(b, 2, 3);
                b.begin(NodeKind::ExpressionBody, 3);
                b.patch(0, 3);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                identifier(b, 4, 5);
                b.end(NodeKind::ExpressionBody, 6);
                b.end(NodeKind::TopLevelMethod, 6);
            }),
        );
        assert!(f.run(&one_rule("recursive_getters")).is_empty());
    }

    // —— unnecessary_getters_setters ———————————————————————————————————————

    /// `test/rules/unnecessary_getters_setters_test.dart` — the trivial
    /// private-field wrap reports at the getter name.
    #[test]
    fn trivial_getter_setter_pair_reports() {
        let source = "class C { int _x = 0; int get x => _x; set x(int v) { _x = v; } }";
        // class0 C1 {2 int3 _x4 =5 06 ;7 int8 get9 x10 =>11 _x12 ;13 set14 x15 (16 int17 v18 )19 {20 _x21 =22 v23 ;24 }25 }26
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::TopLevelDeclaration, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::ClassDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.begin(NodeKind::DeclarationBody, 2);
                // int _x = 0;
                b.begin(NodeKind::Fields, 3);
                b.patch(0, MemberKind::NonStaticField as u32);
                b.begin(NodeKind::VariablesDeclaration, 3);
                b.patch(0, NONE);
                b.begin(NodeKind::InitializedIdentifier, 4);
                identifier(b, 4, 5);
                b.begin(NodeKind::VariableInitializer, 5);
                b.leaf(NodeKind::LiteralInt, 6);
                b.end(NodeKind::VariableInitializer, 7);
                b.end(NodeKind::InitializedIdentifier, 7);
                b.end(NodeKind::VariablesDeclaration, 8);
                b.end(NodeKind::Fields, 8);
                // int get x => _x;
                b.begin(NodeKind::Method, 8);
                b.patch(0, MemberKind::NonStaticMethod as u32);
                b.patch(1, tok(source, t, "get", 0));
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.leaf_span(NodeKind::Type, 8, 9);
                identifier(b, 10, 11);
                b.begin(NodeKind::ExpressionBody, 11);
                b.patch(0, 11);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                identifier(b, 12, 13);
                b.end(NodeKind::ExpressionBody, 14);
                b.end(NodeKind::Method, 14);
                // set x(int v) { _x = v; }
                b.begin(NodeKind::Method, 14);
                b.patch(0, MemberKind::NonStaticMethod as u32);
                b.patch(1, tok(source, t, "set", 0));
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.leaf(NodeKind::NoType, 14);
                identifier(b, 15, 16);
                b.begin(NodeKind::FormalParameters, 16);
                b.begin(NodeKind::FormalParameter, 16);
                b.patch(0, NONE);
                b.patch(1, 18);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                b.leaf_span(NodeKind::Type, 17, 18);
                identifier(b, 18, 19);
                b.end(NodeKind::FormalParameter, 20);
                b.end(NodeKind::FormalParameters, 20);
                b.begin(NodeKind::BlockBody, 20);
                b.patch(0, NONE);
                b.patch(1, NONE);
                b.begin(NodeKind::Block, 20);
                b.begin(NodeKind::ExpressionStatement, 20);
                b.begin(NodeKind::Assignment, 20);
                b.patch(0, 22);
                identifier(b, 21, 22);
                identifier(b, 23, 24);
                b.end(NodeKind::Assignment, 25);
                b.end(NodeKind::ExpressionStatement, 25);
                b.end(NodeKind::Block, 26);
                b.end(NodeKind::BlockBody, 26);
                b.end(NodeKind::Method, 26);
                b.end(NodeKind::DeclarationBody, 26);
                b.end(NodeKind::ClassDeclaration, 27);
                b.end(NodeKind::TopLevelDeclaration, 27);
            }),
        );
        let lints = f.run(&one_rule("unnecessary_getters_setters"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Unnecessary use of getter and setter to wrap a field."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 10));
    }

    /// A getter not reading the field the setter writes passes
    /// (`_checkForSimpleGetter`'s identity face).
    #[test]
    fn getter_returning_something_else_passes() {
        let source = "class C { int _x = 0; int get x => _other; }";
        // class0 C1 {2 int3 _x4 =5 06 ;7 int8 get9 x10 =>11 _other12 ;13 }14
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::TopLevelDeclaration, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::ClassDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.begin(NodeKind::DeclarationBody, 2);
                b.begin(NodeKind::Fields, 3);
                b.patch(0, MemberKind::NonStaticField as u32);
                b.begin(NodeKind::VariablesDeclaration, 3);
                b.patch(0, NONE);
                b.begin(NodeKind::InitializedIdentifier, 4);
                identifier(b, 4, 5);
                b.begin(NodeKind::VariableInitializer, 5);
                b.leaf(NodeKind::LiteralInt, 6);
                b.end(NodeKind::VariableInitializer, 7);
                b.end(NodeKind::InitializedIdentifier, 7);
                b.end(NodeKind::VariablesDeclaration, 8);
                b.end(NodeKind::Fields, 8);
                b.begin(NodeKind::Method, 8);
                b.patch(0, MemberKind::NonStaticMethod as u32);
                b.patch(1, tok(source, t, "get", 0));
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.leaf_span(NodeKind::Type, 8, 9);
                identifier(b, 10, 11);
                b.begin(NodeKind::ExpressionBody, 11);
                b.patch(0, 11);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                identifier(b, 12, 13);
                b.end(NodeKind::ExpressionBody, 14);
                b.end(NodeKind::Method, 14);
                b.end(NodeKind::DeclarationBody, 14);
                b.end(NodeKind::ClassDeclaration, 15);
                b.end(NodeKind::TopLevelDeclaration, 15);
            }),
        );
        // No setter pair at all — the getter alone never reports.
        assert!(f.run(&one_rule("unnecessary_getters_setters")).is_empty());
    }

    // —— unnecessary_nullable_for_final_variable_declarations ——————————————

    /// A `final` declaration with a nullable type and a non-null literal
    /// initializer reports at the name.
    #[test]
    fn unnecessary_nullable_reports_literal_initializer() {
        let source = "final int? x = 1;";
        // final0 int1 ?2 x3 =4 15 ;6
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, NONE);
                b.leaf_span(NodeKind::Type, 1, 3);
                b.begin(NodeKind::InitializedIdentifier, 3);
                identifier(b, 3, 4);
                b.begin(NodeKind::VariableInitializer, 4);
                b.leaf(NodeKind::LiteralInt, 5);
                b.end(NodeKind::VariableInitializer, 6);
                b.end(NodeKind::InitializedIdentifier, 6);
                b.end(NodeKind::VariablesDeclaration, 7);
            }),
        );
        let lints = f.run(&one_rule(
            "unnecessary_nullable_for_final_variable_declarations",
        ));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "Type could be non-nullable.");
        assert_eq!(lints[0].range.start, utf16_at(&f, 3));
    }

    /// A null initializer and a non-literal initializer pass (the
    /// documented literal-subset face).
    #[test]
    fn unnecessary_nullable_null_and_dynamic_initializers_pass() {
        let source = "final int? x = null;";
        // final0 int1 ?2 x3 =4 null5 ;6
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, NONE);
                b.leaf_span(NodeKind::Type, 1, 3);
                b.begin(NodeKind::InitializedIdentifier, 3);
                identifier(b, 3, 4);
                b.begin(NodeKind::VariableInitializer, 4);
                b.leaf(NodeKind::LiteralNull, 5);
                b.end(NodeKind::VariableInitializer, 6);
                b.end(NodeKind::InitializedIdentifier, 6);
                b.end(NodeKind::VariablesDeclaration, 7);
            }),
        );
        assert!(
            f.run(&one_rule(
                "unnecessary_nullable_for_final_variable_declarations"
            ))
            .is_empty()
        );

        let computed = "final int? x = y;";
        let f = Fixture::build(
            computed,
            unit_of(|b, _t| {
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, NONE);
                b.leaf_span(NodeKind::Type, 1, 3);
                b.begin(NodeKind::InitializedIdentifier, 3);
                identifier(b, 3, 4);
                b.begin(NodeKind::VariableInitializer, 4);
                identifier(b, 5, 6);
                b.end(NodeKind::VariableInitializer, 6);
                b.end(NodeKind::InitializedIdentifier, 6);
                b.end(NodeKind::VariablesDeclaration, 7);
            }),
        );
        assert!(
            f.run(&one_rule(
                "unnecessary_nullable_for_final_variable_declarations"
            ))
            .is_empty()
        );
    }

    // —— prefer_initializing_formals ————————————————————————————————————————

    /// `A(int x) { this.x = x; }` — the exact body form with a same-class
    /// field, matching written types, and a single reference reports at the
    /// assignment (`test/rules/prefer_initializing_formals_test.dart`
    /// `test_declareFinal`-family positive). Production-bridge fixture over
    /// the landed `Fields` `MemberKind::NonStaticField` face.
    #[test]
    fn initializing_formals_reports_a_this_assignment() {
        let f = Fixture::parse("class A { int x = 0; A(int x) { this.x = x; } }");
        let lints = f.run(&one_rule("prefer_initializing_formals"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Use an initializing formal to assign a parameter to a field."
        );
        assert_eq!(
            lints[0].correction.as_deref(),
            Some("Try using an initializing formal ('this.x') to initialize the field.")
        );
        let this = tok(&f.source, &f.tokens, "this", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, this));
        assert_eq!(lints[0].range.end, lints[0].range.start + 10);
    }

    /// The initializer face reports over the whole entry — the bare `x = x`
    /// sugar and the explicit `this.x = x` form alike (both are the
    /// official `ConstructorFieldInitializer`). Production-bridge fixture.
    #[test]
    fn initializing_formals_reports_an_initializer_entry() {
        let f = Fixture::parse("class A { int x = 0; A(int x) : x = x; }");
        let lints = f.run(&one_rule("prefer_initializing_formals"));
        assert_eq!(lints.len(), 1);
        let entry = tok(&f.source, &f.tokens, "x", 2);
        assert_eq!(lints[0].range.start, utf16_at(&f, entry));
        assert_eq!(lints[0].range.end, lints[0].range.start + 5);

        let f = Fixture::parse("class A { int x = 0; A(int x) : this.x = x; }");
        let lints = f.run(&one_rule("prefer_initializing_formals"));
        assert_eq!(lints.len(), 1);
        let this = tok(&f.source, &f.tokens, "this", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, this));
        assert_eq!(lints[0].range.end, lints[0].range.start + 10);
    }

    /// A written-type mismatch (`num` field, `int` parameter — the
    /// initializing formal would not compile) and an unwritten parameter
    /// type (no comparable spelling, :1589-1591 of the port) pass.
    /// Production-bridge fixtures.
    #[test]
    fn initializing_formals_type_mismatch_passes() {
        let f = Fixture::parse("class A { num x = 0; A(int x) { this.x = x; } }");
        assert!(f.run(&one_rule("prefer_initializing_formals")).is_empty());

        let f = Fixture::parse("class A { int x = 0; A(x) { this.x = x; } }");
        assert!(f.run(&one_rule("prefer_initializing_formals")).is_empty());
    }

    /// A second reference to the parameter keeps the conversion potentially
    /// semantic — no report (:208-222, count > 1). Production-bridge
    /// fixture.
    #[test]
    fn initializing_formals_extra_reference_passes() {
        let f = Fixture::parse("class A { int x = 0; A(int x) { this.x = x; print(x); } }");
        assert!(f.run(&one_rule("prefer_initializing_formals")).is_empty());
    }

    /// Production-bridge verification of the landed `Fields`
    /// `MemberKind::NonStaticField` face: the exact body form reports at the
    /// assignment on a real parse, and a written-type mismatch
    /// (`String` parameter for an `int` field — the `isSubtypeOf`
    /// approximation, :114-118) stays silent.
    #[test]
    fn initializing_formals_reports_on_real_parse() {
        let f = Fixture::parse("class A { int x = 0; A(int x) { this.x = x; } }");
        let lints = f.run(&one_rule("prefer_initializing_formals"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Use an initializing formal to assign a parameter to a field."
        );
        let this = tok(&f.source, &f.tokens, "this", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, this));
        // `this.x = x` — the whole assignment through the value parameter.
        assert_eq!(lints[0].range.end, lints[0].range.start + 10);

        let f = Fixture::parse("class A { int x = 0; A(String x) { this.x = x; } }");
        assert!(f.run(&one_rule("prefer_initializing_formals")).is_empty());
    }

    // —— batch A2 divergence pins (#466) ———————————————————————————————————

    /// An identifier with a declaration context (the local/parameter name
    /// faces; the read gate keys on the expression context).
    fn declared_identifier(
        b: &mut dartq_ast::AstBuilder,
        context: dartq_parser::IdentifierContext,
        first: u32,
        end: u32,
    ) {
        b.begin(NodeKind::Identifier, first);
        b.patch(0, context as u32);
        b.end(NodeKind::Identifier, end);
    }

    /// `int get x { var x = 1; return x; }` — the body local captures the
    /// read, so it is not the getter reference (`isSelfReference`'s element
    /// identity, recursive_getters.dart:57-60).
    #[test]
    fn recursive_getter_shadowed_local_passes() {
        let source = "int get x { var x = 1; return x; }";
        // int0 get1 x2 {3 var4 x5 =6 17 ;8 return9 x10 ;11 }12
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, tok(source, t, "get", 0));
                b.leaf_span(NodeKind::Type, 0, 1);
                identifier(b, 2, 3);
                b.begin(NodeKind::BlockBody, 3);
                b.patch(0, NONE);
                b.patch(1, NONE);
                b.begin(NodeKind::Block, 3);
                b.begin(NodeKind::VariablesDeclaration, 3);
                b.patch(0, tok(source, t, "var", 0));
                b.begin(NodeKind::InitializedIdentifier, 4);
                declared_identifier(
                    b,
                    dartq_parser::IdentifierContext::LocalVariableDeclaration,
                    5,
                    6,
                );
                b.begin(NodeKind::VariableInitializer, 6);
                b.leaf(NodeKind::LiteralInt, 7);
                b.end(NodeKind::VariableInitializer, 8);
                b.end(NodeKind::InitializedIdentifier, 8);
                b.end(NodeKind::VariablesDeclaration, 9);
                b.begin(NodeKind::ReturnStatement, 9);
                identifier(b, 10, 11);
                b.end(NodeKind::ReturnStatement, 12);
                b.end(NodeKind::Block, 13);
                b.end(NodeKind::BlockBody, 13);
                b.end(NodeKind::TopLevelMethod, 13);
            }),
        );
        assert!(f.run(&one_rule("recursive_getters")).is_empty());
    }

    /// `int get x { var h = (x) { return x; }; return 1; }` — the closure
    /// parameter captures the read inside the closure body.
    #[test]
    fn recursive_getter_shadowed_parameter_passes() {
        let source = "int get x { var h = (x) { return x; }; return 1; }";
        // int0 get1 x2 {3 var4 h5 =6 (7 x8 )9 {10 return11 x12 ;13 }14 ;15 return16 117 ;18 }19
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, tok(source, t, "get", 0));
                b.leaf_span(NodeKind::Type, 0, 1);
                identifier(b, 2, 3);
                b.begin(NodeKind::BlockBody, 3);
                b.patch(0, NONE);
                b.patch(1, NONE);
                b.begin(NodeKind::Block, 3);
                b.begin(NodeKind::VariablesDeclaration, 3);
                b.patch(0, tok(source, t, "var", 0));
                b.begin(NodeKind::InitializedIdentifier, 4);
                declared_identifier(
                    b,
                    dartq_parser::IdentifierContext::LocalVariableDeclaration,
                    5,
                    6,
                );
                b.begin(NodeKind::VariableInitializer, 6);
                b.begin(NodeKind::FunctionExpression, 7);
                b.begin(NodeKind::FormalParameters, 7);
                b.begin(NodeKind::FormalParameter, 7);
                b.patch(0, NONE);
                b.patch(1, tok(source, t, "x", 1));
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                declared_identifier(
                    b,
                    dartq_parser::IdentifierContext::FormalParameterDeclaration,
                    8,
                    9,
                );
                b.end(NodeKind::FormalParameter, 10);
                b.end(NodeKind::FormalParameters, 10);
                b.begin(NodeKind::BlockBody, 10);
                b.patch(0, NONE);
                b.patch(1, NONE);
                b.begin(NodeKind::Block, 10);
                b.begin(NodeKind::ReturnStatement, 11);
                identifier(b, 12, 13);
                b.end(NodeKind::ReturnStatement, 14);
                b.end(NodeKind::Block, 15);
                b.end(NodeKind::BlockBody, 15);
                b.end(NodeKind::FunctionExpression, 15);
                b.end(NodeKind::VariableInitializer, 15);
                b.end(NodeKind::InitializedIdentifier, 15);
                b.end(NodeKind::VariablesDeclaration, 16);
                b.begin(NodeKind::ReturnStatement, 16);
                b.leaf(NodeKind::LiteralInt, 17);
                b.end(NodeKind::ReturnStatement, 19);
                b.end(NodeKind::Block, 20);
                b.end(NodeKind::BlockBody, 20);
                b.end(NodeKind::TopLevelMethod, 20);
            }),
        );
        assert!(f.run(&one_rule("recursive_getters")).is_empty());
    }

    /// `set x(int _x) { _x = _x; }` — the assignment target resolves to the
    /// setter's parameter, not the field (`_checkForSimpleSetter`'s
    /// `writeElement` face), so the pair passes.
    #[test]
    fn getter_setter_parameter_shadow_passes() {
        let source = "class C { int _x = 0; int get x => _x; set x(int _x) { _x = _x; } }";
        // class0 C1 {2 int3 _x4 =5 06 ;7 int8 get9 x10 =>11 _x12 ;13 set14 x15 (16 int17 _x18 )19 {20 _x21 =22 _x23 ;24 }25 }26
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::TopLevelDeclaration, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::ClassDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.begin(NodeKind::DeclarationBody, 2);
                b.begin(NodeKind::Fields, 3);
                b.patch(0, MemberKind::NonStaticField as u32);
                b.begin(NodeKind::VariablesDeclaration, 3);
                b.patch(0, NONE);
                b.begin(NodeKind::InitializedIdentifier, 4);
                identifier(b, 4, 5);
                b.begin(NodeKind::VariableInitializer, 5);
                b.leaf(NodeKind::LiteralInt, 6);
                b.end(NodeKind::VariableInitializer, 7);
                b.end(NodeKind::InitializedIdentifier, 7);
                b.end(NodeKind::VariablesDeclaration, 8);
                b.end(NodeKind::Fields, 8);
                b.begin(NodeKind::Method, 8);
                b.patch(0, MemberKind::NonStaticMethod as u32);
                b.patch(1, tok(source, t, "get", 0));
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.leaf_span(NodeKind::Type, 8, 9);
                identifier(b, 10, 11);
                b.begin(NodeKind::ExpressionBody, 11);
                b.patch(0, 11);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                identifier(b, 12, 13);
                b.end(NodeKind::ExpressionBody, 14);
                b.end(NodeKind::Method, 14);
                b.begin(NodeKind::Method, 14);
                b.patch(0, MemberKind::NonStaticMethod as u32);
                b.patch(1, tok(source, t, "set", 0));
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.leaf(NodeKind::NoType, 14);
                identifier(b, 15, 16);
                b.begin(NodeKind::FormalParameters, 16);
                b.begin(NodeKind::FormalParameter, 16);
                b.patch(0, NONE);
                b.patch(1, tok(source, t, "_x", 1));
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                b.leaf_span(NodeKind::Type, 17, 18);
                identifier(b, 18, 19);
                b.end(NodeKind::FormalParameter, 20);
                b.end(NodeKind::FormalParameters, 20);
                b.begin(NodeKind::BlockBody, 20);
                b.patch(0, NONE);
                b.patch(1, NONE);
                b.begin(NodeKind::Block, 20);
                b.begin(NodeKind::ExpressionStatement, 20);
                b.begin(NodeKind::Assignment, 21);
                b.patch(0, tok(source, t, "=", 1));
                identifier(b, 21, 22);
                identifier(b, 23, 24);
                b.end(NodeKind::Assignment, 24);
                b.end(NodeKind::ExpressionStatement, 25);
                b.end(NodeKind::Block, 26);
                b.end(NodeKind::BlockBody, 26);
                b.end(NodeKind::Method, 26);
                b.end(NodeKind::DeclarationBody, 26);
                b.end(NodeKind::ClassDeclaration, 27);
                b.end(NodeKind::TopLevelDeclaration, 27);
            }),
        );
        assert!(f.run(&one_rule("unnecessary_getters_setters")).is_empty());
    }

    /// A getter whose body declares a local named exactly like the (absent)
    /// field never wraps a field — no report.
    #[test]
    fn getter_setter_body_local_passes() {
        let source = "class C { int get x { var x = 1; return x; } set x(int v) { x = v; } }";
        // class0 C1 {2 int3 get4 x5 {6 var7 x8 =9 110 ;11 return12 x13 ;14 }15 set16 x17 (18 int19 v20 )21 {22 x23 =24 v25 ;26 }27 }28
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::TopLevelDeclaration, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::ClassDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.begin(NodeKind::DeclarationBody, 2);
                b.begin(NodeKind::Method, 3);
                b.patch(0, MemberKind::NonStaticMethod as u32);
                b.patch(1, tok(source, t, "get", 0));
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.leaf_span(NodeKind::Type, 3, 4);
                identifier(b, 5, 6);
                b.begin(NodeKind::BlockBody, 6);
                b.patch(0, NONE);
                b.patch(1, NONE);
                b.begin(NodeKind::Block, 6);
                b.begin(NodeKind::VariablesDeclaration, 6);
                b.patch(0, tok(source, t, "var", 0));
                b.begin(NodeKind::InitializedIdentifier, 7);
                declared_identifier(
                    b,
                    dartq_parser::IdentifierContext::LocalVariableDeclaration,
                    8,
                    9,
                );
                b.begin(NodeKind::VariableInitializer, 9);
                b.leaf(NodeKind::LiteralInt, 10);
                b.end(NodeKind::VariableInitializer, 11);
                b.end(NodeKind::InitializedIdentifier, 12);
                b.end(NodeKind::VariablesDeclaration, 12);
                b.begin(NodeKind::ReturnStatement, 12);
                identifier(b, 13, 14);
                b.end(NodeKind::ReturnStatement, 15);
                b.end(NodeKind::Block, 16);
                b.end(NodeKind::BlockBody, 16);
                b.end(NodeKind::Method, 16);
                b.begin(NodeKind::Method, 16);
                b.patch(0, MemberKind::NonStaticMethod as u32);
                b.patch(1, tok(source, t, "set", 0));
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.leaf(NodeKind::NoType, 16);
                identifier(b, 17, 18);
                b.begin(NodeKind::FormalParameters, 18);
                b.begin(NodeKind::FormalParameter, 18);
                b.patch(0, NONE);
                b.patch(1, tok(source, t, "v", 0));
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                b.leaf_span(NodeKind::Type, 19, 20);
                identifier(b, 20, 21);
                b.end(NodeKind::FormalParameter, 22);
                b.end(NodeKind::FormalParameters, 22);
                b.begin(NodeKind::BlockBody, 22);
                b.patch(0, NONE);
                b.patch(1, NONE);
                b.begin(NodeKind::Block, 22);
                b.begin(NodeKind::ExpressionStatement, 22);
                b.begin(NodeKind::Assignment, 23);
                b.patch(0, tok(source, t, "=", 0));
                identifier(b, 23, 24);
                identifier(b, 25, 26);
                b.end(NodeKind::Assignment, 27);
                b.end(NodeKind::ExpressionStatement, 27);
                b.end(NodeKind::Block, 28);
                b.end(NodeKind::BlockBody, 28);
                b.end(NodeKind::Method, 28);
                b.end(NodeKind::DeclarationBody, 28);
                b.end(NodeKind::ClassDeclaration, 29);
                b.end(NodeKind::TopLevelDeclaration, 29);
            }),
        );
        assert!(f.run(&one_rule("unnecessary_getters_setters")).is_empty());
    }

    /// The correction spells the written tail verbatim — a comment inside
    /// the tail rides along in the message text.
    #[test]
    fn generic_function_alias_correction_keeps_comments() {
        let source = "typedef void F(/* hi */ int);";
        // typedef0 void1 F2 (3 int4 )5 ;6
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::Typedef, 0);
                b.patch(0, tok(source, t, "typedef", 0));
                b.patch(1, NONE); // old form: no `=`
                b.leaf_span(NodeKind::Type, 1, 2);
                identifier(b, 2, 3);
                b.begin(NodeKind::FormalParameters, 3);
                b.end(NodeKind::FormalParameters, 6);
                b.end(NodeKind::Typedef, 7);
            }),
        );
        let lints = f.run(&one_rule("prefer_generic_function_type_aliases"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].correction,
            Some(
                "Try using the generic function type syntax ('void Function(/* hi */ int)')."
                    .to_owned()
            )
        );
    }

    /// `f = null;` after the declaration is a later write — the variable is
    /// potentially mutated in scope and passes (:53).
    #[test]
    fn function_variable_mutated_later_passes() {
        let source = "void m() { var f = () {}; f = null; }";
        // void0 m1 (2 )3 {4 var5 f6 =7 (8 )9 {10 }11 ;12 f13 =14 null15 ;16 }17
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, NONE);
                b.leaf(NodeKind::NoType, 0);
                identifier(b, 0, 1);
                b.begin(NodeKind::FormalParameters, 1);
                b.end(NodeKind::FormalParameters, 4);
                b.begin(NodeKind::BlockBody, 4);
                b.patch(0, NONE);
                b.patch(1, NONE);
                b.begin(NodeKind::Block, 4);
                b.begin(NodeKind::VariablesDeclaration, 4);
                b.patch(0, tok(source, t, "var", 0));
                b.begin(NodeKind::InitializedIdentifier, 6);
                identifier(b, 6, 7);
                b.begin(NodeKind::VariableInitializer, 7);
                b.begin(NodeKind::FunctionExpression, 7);
                b.begin(NodeKind::FormalParameters, 7);
                b.end(NodeKind::FormalParameters, 10);
                b.begin(NodeKind::Block, 10);
                b.end(NodeKind::Block, 12);
                b.end(NodeKind::FunctionExpression, 12);
                b.end(NodeKind::VariableInitializer, 12);
                b.end(NodeKind::InitializedIdentifier, 12);
                b.end(NodeKind::VariablesDeclaration, 13);
                b.begin(NodeKind::ExpressionStatement, 13);
                b.begin(NodeKind::Assignment, 13);
                b.patch(0, tok(source, t, "=", 0));
                identifier(b, 13, 14);
                b.leaf(NodeKind::LiteralNull, 15);
                b.end(NodeKind::Assignment, 16);
                b.end(NodeKind::ExpressionStatement, 17);
                b.end(NodeKind::Block, 18);
                b.end(NodeKind::BlockBody, 18);
                b.end(NodeKind::TopLevelMethod, 18);
            }),
        );
        assert!(
            f.run(&one_rule("prefer_function_declarations_over_variables"))
                .is_empty()
        );
    }

    /// A write inside a nested closure is a potential mutation too.
    #[test]
    fn function_variable_mutated_in_nested_closure_passes() {
        let source = "void m() { var f = () {}; g(() { f = 1; }); }";
        // void0 m1 (2 )3 {4 var5 f6 =7 (8 )9 {10 }11 ;12 g13 (14 (15 )16 {17 f18 =19 120 ;21 }22 )23 ;24 }25
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, NONE);
                b.leaf(NodeKind::NoType, 0);
                identifier(b, 0, 1);
                b.begin(NodeKind::FormalParameters, 1);
                b.end(NodeKind::FormalParameters, 4);
                b.begin(NodeKind::BlockBody, 4);
                b.patch(0, NONE);
                b.patch(1, NONE);
                b.begin(NodeKind::Block, 4);
                b.begin(NodeKind::VariablesDeclaration, 4);
                b.patch(0, tok(source, t, "var", 0));
                b.begin(NodeKind::InitializedIdentifier, 6);
                identifier(b, 6, 7);
                b.begin(NodeKind::VariableInitializer, 7);
                b.begin(NodeKind::FunctionExpression, 7);
                b.begin(NodeKind::FormalParameters, 7);
                b.end(NodeKind::FormalParameters, 10);
                b.begin(NodeKind::Block, 10);
                b.end(NodeKind::Block, 12);
                b.end(NodeKind::FunctionExpression, 12);
                b.end(NodeKind::VariableInitializer, 12);
                b.end(NodeKind::InitializedIdentifier, 12);
                b.end(NodeKind::VariablesDeclaration, 13);
                b.begin(NodeKind::ExpressionStatement, 13);
                b.begin(NodeKind::Send, 13);
                identifier(b, 13, 14);
                b.begin(NodeKind::Arguments, 14);
                b.begin(NodeKind::FunctionExpression, 14);
                b.begin(NodeKind::FormalParameters, 14);
                b.end(NodeKind::FormalParameters, 17);
                b.begin(NodeKind::BlockBody, 17);
                b.patch(0, NONE);
                b.patch(1, NONE);
                b.begin(NodeKind::Block, 17);
                b.begin(NodeKind::ExpressionStatement, 17);
                b.begin(NodeKind::Assignment, 18);
                b.patch(0, tok(source, t, "=", 0));
                identifier(b, 18, 19);
                b.leaf(NodeKind::LiteralInt, 20);
                b.end(NodeKind::Assignment, 21);
                b.end(NodeKind::ExpressionStatement, 22);
                b.end(NodeKind::Block, 23);
                b.end(NodeKind::BlockBody, 23);
                b.end(NodeKind::FunctionExpression, 23);
                b.end(NodeKind::Arguments, 24);
                b.end(NodeKind::Send, 24);
                b.end(NodeKind::ExpressionStatement, 25);
                b.end(NodeKind::Block, 26);
                b.end(NodeKind::BlockBody, 26);
                b.end(NodeKind::TopLevelMethod, 26);
            }),
        );
        assert!(
            f.run(&one_rule("prefer_function_declarations_over_variables"))
                .is_empty()
        );
    }

    /// Never reassigned — the local still reports (preserved face).
    #[test]
    fn function_variable_unmutated_local_reports() {
        let source = "void m() { var f = () {}; g(f); }";
        // void0 m1 (2 )3 {4 var5 f6 =7 (8 )9 {10 }11 ;12 g13 (14 f15 )16 ;17 }18
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, NONE);
                b.leaf(NodeKind::NoType, 0);
                identifier(b, 0, 1);
                b.begin(NodeKind::FormalParameters, 1);
                b.end(NodeKind::FormalParameters, 4);
                b.begin(NodeKind::BlockBody, 4);
                b.patch(0, NONE);
                b.patch(1, NONE);
                b.begin(NodeKind::Block, 4);
                b.begin(NodeKind::VariablesDeclaration, 4);
                b.patch(0, tok(source, t, "var", 0));
                b.begin(NodeKind::InitializedIdentifier, 6);
                identifier(b, 6, 7);
                b.begin(NodeKind::VariableInitializer, 7);
                b.begin(NodeKind::FunctionExpression, 7);
                b.begin(NodeKind::FormalParameters, 7);
                b.end(NodeKind::FormalParameters, 10);
                b.begin(NodeKind::Block, 10);
                b.end(NodeKind::Block, 12);
                b.end(NodeKind::FunctionExpression, 12);
                b.end(NodeKind::VariableInitializer, 12);
                b.end(NodeKind::InitializedIdentifier, 12);
                b.end(NodeKind::VariablesDeclaration, 13);
                b.begin(NodeKind::ExpressionStatement, 13);
                b.begin(NodeKind::Send, 13);
                identifier(b, 13, 14);
                b.begin(NodeKind::Arguments, 14);
                identifier(b, 15, 16);
                b.end(NodeKind::Arguments, 17);
                b.end(NodeKind::Send, 17);
                b.end(NodeKind::ExpressionStatement, 18);
                b.end(NodeKind::Block, 19);
                b.end(NodeKind::BlockBody, 19);
                b.end(NodeKind::TopLevelMethod, 19);
            }),
        );
        let lints = f.run(&one_rule("prefer_function_declarations_over_variables"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, 6));
    }

    // ---- #586 standard-set completions (production-bridge fixtures) ----

    /// `annotate_overrides` (:58-64): a member overriding without
    /// `@override` reports its name. The message is the current official
    /// template (`diagnostic.g.dart` `annotateOverrides` problemMessage:
    /// "The member '{0}' overrides an inherited member but isn't annotated
    /// with '@override'.") with the member name as `{0}`.
    #[test]
    fn annotate_overrides_reports_unannotated_member() {
        let f = Fixture::parse(
            "class A { int x = 0; void foo() {} }\nclass B extends A {\n  int x = 1;\n  void foo() {}\n  void own() {}\n}",
        );
        let lints = f.run(&one_rule("annotate_overrides"));
        assert_eq!(lints.len(), 2, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The member 'x' overrides an inherited member but isn't annotated with '@override'."
        );
        assert_eq!(
            lints[1].message,
            "The member 'foo' overrides an inherited member but isn't annotated with '@override'."
        );
    }

    /// `@override`-annotated members and members with no inherited
    /// namesake stay silent (:42, :96-99).
    #[test]
    fn annotate_overrides_annotation_and_non_overrides_pass() {
        let f = Fixture::parse(
            "class A { int x = 0; void foo() {} }\nclass B extends A {\n  @override\n  int x = 1;\n  @override\n  void foo() {}\n}",
        );
        assert!(f.run(&one_rule("annotate_overrides")).is_empty());
    }

    /// The `lib/` gate (`rule_context.dart:15-21`) plus the wildcard and
    /// doc-comment skips of `avoid_renaming_method_parameters`.
    #[test]
    fn avoid_renaming_reports_renamed_parameters_in_lib() {
        let mut f = Fixture::parse(
            "class A { void foo(int aa, String bb) {} }\nclass B extends A {\n  void foo(int cc, String dd) {}\n}",
        );
        f.path = "lib/a.dart".into();
        let lints = f.run(&one_rule("avoid_renaming_method_parameters"));
        assert_eq!(lints.len(), 2, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The parameter name 'cc' doesn't match the name 'aa' in the overridden method."
        );
        assert_eq!(
            lints[1].message,
            "The parameter name 'dd' doesn't match the name 'bb' in the overridden method."
        );
    }

    #[test]
    fn avoid_renaming_gates() {
        let source =
            "class A { void foo(int aa) {} }\nclass B extends A {\n  void foo(int cc) {}\n}";
        // Outside `lib/` the official rule never registers its visitors
        // (`avoid_renaming_method_parameters.dart:33` —
        // `if (!context.isInLibDir) return;`).
        let mut f = Fixture::parse(source);
        f.path = "tool/gen.dart".into();
        assert!(
            f.run(&one_rule("avoid_renaming_method_parameters"))
                .is_empty()
        );

        // Matching names, wildcards, and a documented rename stay silent.
        let mut f = Fixture::parse(
            "class A { void foo(int aa, int bb, int cc) {} }\nclass B extends A {\n  /// Keep `cc` renamed for docs.\n  void foo(int aa, int _, int cc) {}\n}",
        );
        f.path = "lib/a.dart".into();
        assert!(
            f.run(&one_rule("avoid_renaming_method_parameters"))
                .is_empty()
        );
    }

    /// An abstract inherited method has no concrete parameter names to
    /// match (`getInheritedConcreteMember` face) — no report.
    #[test]
    fn avoid_renaming_abstract_inherited_passes() {
        let mut f = Fixture::parse(
            "abstract class A { void foo(int aa); }\nclass B extends A {\n  void foo(int cc) {}\n}",
        );
        f.path = "lib/a.dart".into();
        assert!(
            f.run(&one_rule("avoid_renaming_method_parameters"))
                .is_empty()
        );
    }

    /// `overridden_fields` (:41-47): a field redeclaring an inherited
    /// instance field reports.
    #[test]
    fn overridden_fields_reports_redeclared_field() {
        let f = Fixture::parse("class A { int x = 0; }\nclass B extends A { int x = 1; }");
        let lints = f.run(&one_rule("overridden_fields"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Field overrides a field inherited from 'A'."
        );
    }

    /// Declared getters/setters are not variable-backed fields
    /// (`isOriginVariable` face) and static members are not instance
    /// members — neither reports.
    #[test]
    fn overridden_fields_setter_and_static_passes() {
        let f = Fixture::parse(
            "class A { set x(int v) {} static int s = 0; }\nclass B extends A { int x = 1; static int s = 1; }",
        );
        assert!(f.run(&one_rule("overridden_fields")).is_empty());
    }

    /// `unnecessary_overrides` (:52-113): a pure `super` forward reports;
    /// abstract inherited methods have nothing to forward to.
    #[test]
    fn unnecessary_overrides_reports_bare_super_forward() {
        let f = Fixture::parse(
            "class A { void foo(int aa) {} }\nclass B extends A {\n  void foo(int aa) { super.foo(aa); }\n}",
        );
        let lints = f.run(&one_rule("unnecessary_overrides"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, "Unnecessary override.");
    }

    #[test]
    fn unnecessary_overrides_abstract_and_extra_work_passes() {
        // Abstract inherited: no concrete member to forward to.
        let f = Fixture::parse(
            "abstract class A { void foo(int aa); }\nclass B extends A {\n  void foo(int aa) { super.foo(aa); }\n}",
        );
        assert!(f.run(&one_rule("unnecessary_overrides")).is_empty());

        // Two statements: the override does more than forward (:52-56).
        let f = Fixture::parse(
            "class A { void foo(int aa) {} }\nclass B extends A {\n  void foo(int aa) { super.foo(aa); aa++; }\n}",
        );
        assert!(f.run(&one_rule("unnecessary_overrides")).is_empty());
    }

    /// Getter/setter forwards through every body form.
    #[test]
    fn unnecessary_overrides_accessors() {
        let f = Fixture::parse(
            "class A {\n  int get g => 1;\n  set s(int v) {}\n}\nclass B extends A {\n  int get g => super.g;\n  set s(int v) { super.s = v; }\n}",
        );
        let lints = f.run(&one_rule("unnecessary_overrides"));
        assert_eq!(lints.len(), 2, "{lints:?}");
        assert_eq!(lints[0].message, "Unnecessary override.");
        assert_eq!(lints[1].message, "Unnecessary override.");
    }
}
