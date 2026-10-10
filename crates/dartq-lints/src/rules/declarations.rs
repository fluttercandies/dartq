//! The declaration-shape family: `avoid_return_types_on_setters`,
//! `unnecessary_late`, `use_function_type_syntax_for_parameters`,
//! `prefer_typing_uninitialized_variables`,
//! `prefer_generic_function_type_aliases`, `unnecessary_constructor_name`,
//! `hash_and_equals`, `avoid_init_to_null`,
//! `prefer_function_declarations_over_variables`, `type_init_formals`.

use dartq_ast::NodeKind;

use crate::engine::Check;
use crate::registry::{Group, RuleSpec};
use crate::rules::{NONE_TOKEN, first_child, member_name, type_before};

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
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/unnecessary_constructor_name.dart:41-72`
    /// — a constructor (or instance-creation) `.new` name reports at the
    /// name token. Two element-based guards are approximated syntactically:
    /// the duplicate-unnamed-constructor bail counts unnamed constructors
    /// among the enclosing `DeclarationBody` siblings (each `Constructor`
    /// with at most one `Identifier` child is unnamed), and the
    /// extension-type primary-constructor exemption is skipped (primary
    /// constructors are outside the current substrate). The instance-creation
    /// face dispatches on `Send` with a `Qualified` callee — a syntax-only
    /// proxy for `InstanceCreationExpression` (a non-creation `X.new` access
    /// is not expressible Dart). Message: `diagnostic.g.dart`
    /// `unnecessaryConstructorName`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "unnecessary_constructor_name",
        group: Group::Recommended,
        problem: "Unnecessary '.new' constructor name.",
        correction: "Try removing the '.new'.",
        kinds: &[NodeKind::Constructor, NodeKind::Send],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        match c.kind(node) {
            NodeKind::Constructor => check_declaration(c, node),
            NodeKind::Send => check_creation(c, node),
            _ => {}
        }
    }

    /// The constructor-name `Identifier` (the second one; the first names
    /// the enclosing type), or `None` when unnamed.
    fn constructor_name(c: &Check<'_>, node: u32) -> Option<u32> {
        c.direct_children(node)
            .iter()
            .copied()
            .filter(|&kid| c.kind(kid) == NodeKind::Identifier)
            .nth(1)
    }

    fn check_declaration(c: &mut Check<'_>, node: u32) {
        // The constructor name is the second `Identifier` child (the first
        // names the enclosing type, collapse map §3.3).
        let Some(name) = constructor_name(c, node) else {
            return; // unnamed constructor: `node.name == null`
        };
        if c.lexeme(c.head_token(name)) != "new" {
            return;
        }
        // Duplicate unnamed constructors already error (:45-52) — an
        // unnamed constructor is one without a name `Identifier` or one
        // explicitly spelled `new` (the canonical name of the unnamed
        // form). The owner is the enclosing `DeclarationBody`.
        let body = c.ancestor(1).expect("constructor has an owner");
        // Collect first: the count re-enters `direct_children` per member
        // and the scratch RefCell forbids a live outer guard.
        let members: Vec<u32> = c.direct_children(body).iter().copied().collect();
        let unnamed = members
            .into_iter()
            .filter(|&member| {
                c.kind(member) != NodeKind::Constructor
                    || match constructor_name(c, member) {
                        None => true,
                        Some(name) => c.lexeme(c.head_token(name)) == "new",
                    }
            })
            .count();
        if unnamed > 1 {
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

    fn check_creation(c: &mut Check<'_>, node: u32) {
        let callee = c.direct_children(node).iter().copied().find(|&kid| {
            !matches!(
                c.kind(kid),
                NodeKind::Arguments | NodeKind::NoArguments | NodeKind::TypeApplication
            )
        });
        let Some(callee) = callee else {
            return;
        };
        if c.kind(callee) != NodeKind::Qualified {
            return;
        }
        let Some(name) = c
            .direct_children(callee)
            .iter()
            .copied()
            .filter(|&kid| c.kind(kid) == NodeKind::Identifier)
            .nth(1)
        else {
            return;
        };
        if c.lexeme(c.head_token(name)) != "new" {
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
    use crate::rules::{NONE_TOKEN, first_child, token_seq_equal};

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
        let Some(owner) = c.ancestor(1) else {
            return;
        };
        let fields = class_fields(c, owner);
        let initializers = kids
            .iter()
            .copied()
            .find(|&kid| c.kind(kid) == NodeKind::Initializers);
        // Face A: initializer-list entries `x = p` / `this.x = p` — the
        // initializer expression is the bare parameter.
        let mut candidates: Vec<(&str, &str, u32)> = Vec::new();
        if let Some(initializers) = initializers {
            face_initializers(c, initializers, &mut candidates);
        }
        // Face B: body statements of the exact form `this.x = p;`.
        let mut ref_roots: Vec<u32> = Vec::new();
        if let Some(initializers) = initializers {
            ref_roots.push(initializers);
        }
        if let Some(body) = kids
            .iter()
            .copied()
            .find(|&kid| matches!(c.kind(kid), NodeKind::Block | NodeKind::BlockBody))
        {
            ref_roots.push(body);
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
    fn class_fields<'a>(c: &Check<'a>, owner: u32) -> Vec<(&'a str, Option<u32>)> {
        let mut fields: Vec<(&str, Option<u32>)> = Vec::new();
        for member in c.collect_direct(owner) {
            if c.kind(member) != NodeKind::Fields
                || c.payload(member).first()
                    != Some(&(dartq_parser::MemberKind::NonStaticField as u32))
            {
                continue;
            }
            for decl in c.collect_direct(member) {
                if c.kind(decl) != NodeKind::VariablesDeclaration {
                    continue;
                }
                let written = c
                    .collect_direct(decl)
                    .into_iter()
                    .find(|&kid| c.kind(kid) == NodeKind::Type);
                for var in c.collect_direct(decl) {
                    if c.kind(var) != NodeKind::InitializedIdentifier {
                        continue;
                    }
                    if let Some(name) = crate::rules::member_name(c, var) {
                        fields.push((c.lexeme(c.head_token(name)), written));
                    }
                }
            }
        }
        fields
    }

    /// Face A (:71-85): each `Initializer` naming a field and initialized
    /// from a bare identifier pushes a candidate.
    fn face_initializers<'a>(
        c: &Check<'a>,
        initializers: u32,
        out: &mut Vec<(&'a str, &'a str, u32)>,
    ) {
        for initializer in c.collect_direct(initializers) {
            if c.kind(initializer) != NodeKind::Initializer {
                continue;
            }
            let initializer_kids = c.collect_direct(initializer);
            let Some(&field_name) = initializer_kids.iter().find(|&&kid| {
                c.kind(kid) == NodeKind::Identifier
                    && c.payload(kid).first()
                        == Some(&(dartq_parser::IdentifierContext::FieldInitializer as u32))
            }) else {
                continue;
            };
            let Some(field_init) = initializer_kids
                .iter()
                .copied()
                .find(|&kid| c.kind(kid) == NodeKind::FieldInitializer)
            else {
                continue;
            };
            let Some(value) = first_child(c, field_init) else {
                continue;
            };
            if c.kind(value) != NodeKind::Identifier {
                continue;
            }
            out.push((
                c.lexeme(c.head_token(field_name)),
                c.lexeme(c.head_token(value)),
                initializer,
            ));
        }
    }

    /// Face B (:87-102): body statements of the exact form `this.x = p;`.
    fn face_body<'a>(c: &Check<'a>, body: u32, out: &mut Vec<(&'a str, &'a str, u32)>) {
        let block = if c.kind(body) == NodeKind::Block {
            body
        } else {
            match c
                .collect_direct(body)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::Block)
            {
                Some(block) => block,
                None => return,
            }
        };
        for statement in c.collect_direct(block) {
            if c.kind(statement) != NodeKind::ExpressionStatement {
                continue;
            }
            let Some(expression) = first_child(c, statement) else {
                continue;
            };
            if c.kind(expression) != NodeKind::Assignment
                || c.lexeme(
                    c.payload(expression)
                        .first()
                        .copied()
                        .expect("assignment op"),
                ) != "="
            {
                continue;
            }
            let operands = c.collect_direct(expression);
            if operands.len() < 2 {
                continue;
            }
            let (lhs, rhs) = (operands[0], operands[1]);
            // `AssignmentExpression(leftHandSide: PropertyAccess(target:
            // ThisExpression))` (:92-102) — the exact form only.
            if c.kind(lhs) != NodeKind::DotAccess || c.kind(rhs) != NodeKind::Identifier {
                continue;
            }
            let lhs_kids = c.collect_direct(lhs);
            if lhs_kids.first().map(|&kid| c.kind(kid)) != Some(NodeKind::ThisExpression) {
                continue;
            }
            let Some(&field_name) = lhs_kids.last() else {
                continue;
            };
            if c.kind(field_name) != NodeKind::Identifier {
                continue;
            }
            out.push((
                c.lexeme(c.head_token(field_name)),
                c.lexeme(c.head_token(rhs)),
                expression,
            ));
        }
    }

    /// The shared gates and the report (:104-159): one candidate per field,
    /// a same-named non-private instance field of this class, written-type
    /// equality, and no other reference to the parameter.
    fn finish(
        c: &mut Check<'_>,
        params: &[(&str, Option<u32>)],
        fields: &[(&str, Option<u32>)],
        ref_roots: &[u32],
        candidates: &[(&str, &str, u32)],
    ) {
        for &(field_name, param_name, node) in candidates {
            // Multiple initializations of one field — the conversion is
            // ambiguous, none of them lints (:48-53, `_nodesToLintByField`).
            if candidates
                .iter()
                .filter(|(name, _, _)| *name == field_name)
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
            let references = count_expression_refs(c, ref_roots, param_name);
            if references != 1 {
                continue;
            }
            let range = c.utf16_span(node);
            c.report(range, &[field_name]);
        }
    }

    /// The expression-position references to `name` across the constructor's
    /// initializers and body. Only `Expression`-context identifiers count
    /// (initializer names bind fields, not parameters), and dot-access name
    /// sides are member reads, never parameter references.
    fn count_expression_refs(c: &Check<'_>, roots: &[u32], name: &str) -> u32 {
        let expression_ctx = dartq_parser::IdentifierContext::Expression as u32;
        let mut count = 0;
        for &root in roots {
            let window = c.children(root);
            for id in window.start..window.end {
                if c.kind(id) != NodeKind::Identifier
                    || c.payload(id).first() != Some(&expression_ctx)
                    || c.lexeme(c.head_token(id)) != name
                {
                    continue;
                }
                if let Some(parent) = c.unit.ast.parent(id)
                    && matches!(c.kind(parent), NodeKind::DotAccess | NodeKind::Qualified)
                    && c.children(parent).end - 1 == id
                {
                    continue;
                }
                count += 1;
            }
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

/// The `get`/`set` keyword token of a member declaration, by lexeme.
/// `Method` payload word 1 and `TopLevelMethod` payload word 0 are the
/// `get_set` option (collapse map §3.3).
fn get_set_token<'a>(c: &'a Check<'_>, node: u32) -> Option<&'a str> {
    let word = match c.kind(node) {
        NodeKind::Method => 1,
        NodeKind::TopLevelMethod => 0,
        _ => return None,
    };
    match c.payload(node).get(word) {
        Some(&token) if token != NONE_TOKEN => Some(c.lexeme(token)),
        _ => None,
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
        let source = "class A { A.new(); }";
        // class0 A1 {2 A3 .4 new5 (6 )7 ;8 }9
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
                b.patch(0, dartq_parser::MemberKind::NonStaticMethod as u32);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                identifier(b, 3, 4);
                identifier(b, 5, 6);
                b.begin(NodeKind::FormalParameters, 6);
                b.end(NodeKind::FormalParameters, 8);
                b.end(NodeKind::Constructor, 9);
                b.end(NodeKind::DeclarationBody, 10);
                b.end(NodeKind::ClassDeclaration, 10);
                b.end(NodeKind::TopLevelDeclaration, 10);
            }),
        );
        let lints = f.run(&one_rule("unnecessary_constructor_name"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "Unnecessary '.new' constructor name.");
        assert_eq!(lints[0].range.start, utf16_at(&f, 5));
    }

    /// An unnamed constructor has no name token to check.
    #[test]
    fn constructor_name_unnamed_passes() {
        let source = "class A { A(); }";
        // class0 A1 {2 A3 (4 )5 ;6 }7
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
                b.patch(0, dartq_parser::MemberKind::NonStaticMethod as u32);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                identifier(b, 3, 4);
                b.begin(NodeKind::FormalParameters, 4);
                b.end(NodeKind::FormalParameters, 6);
                b.end(NodeKind::Constructor, 7);
                b.end(NodeKind::DeclarationBody, 8);
                b.end(NodeKind::ClassDeclaration, 8);
                b.end(NodeKind::TopLevelDeclaration, 8);
            }),
        );
        assert!(f.run(&one_rule("unnecessary_constructor_name")).is_empty());
    }

    /// A named constructor other than `new` passes.
    #[test]
    fn constructor_name_named_other_passes() {
        let source = "class A { A.named(); }";
        // class0 A1 {2 A3 .4 named5 (6 )7 ;8 }9
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
                b.patch(0, dartq_parser::MemberKind::NonStaticMethod as u32);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                identifier(b, 3, 4);
                identifier(b, 5, 6);
                b.begin(NodeKind::FormalParameters, 6);
                b.end(NodeKind::FormalParameters, 8);
                b.end(NodeKind::Constructor, 9);
                b.end(NodeKind::DeclarationBody, 10);
                b.end(NodeKind::ClassDeclaration, 10);
                b.end(NodeKind::TopLevelDeclaration, 10);
            }),
        );
        assert!(f.run(&one_rule("unnecessary_constructor_name")).is_empty());
    }

    /// Duplicate unnamed constructors already error (:45-52) — no report
    /// (`A.new()` and `A()` are both the unnamed constructor).
    #[test]
    fn constructor_name_duplicate_unnamed_bails() {
        let source = "class A { A.new(); A(); }";
        // class0 A1 {2 A3 .4 new5 (6 )7 ;8 A9 (10 )11 ;12 }13
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
                b.patch(0, dartq_parser::MemberKind::NonStaticMethod as u32);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                identifier(b, 3, 4);
                identifier(b, 5, 6);
                b.begin(NodeKind::FormalParameters, 6);
                b.end(NodeKind::FormalParameters, 8);
                b.end(NodeKind::Constructor, 9);
                b.begin(NodeKind::Constructor, 9);
                b.patch(0, dartq_parser::MemberKind::NonStaticMethod as u32);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                identifier(b, 9, 10);
                b.begin(NodeKind::FormalParameters, 10);
                b.end(NodeKind::FormalParameters, 12);
                b.end(NodeKind::Constructor, 13);
                b.end(NodeKind::DeclarationBody, 14);
                b.end(NodeKind::ClassDeclaration, 14);
                b.end(NodeKind::TopLevelDeclaration, 14);
            }),
        );
        assert!(f.run(&one_rule("unnecessary_constructor_name")).is_empty());
    }

    /// The instance-creation face: `A.new()` reports at `.new`'s name.
    #[test]
    fn constructor_name_creation_reports_new() {
        let source = "A.new();";
        // A0 .1 new2 (3 )4 ;5
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::Send, 0);
                b.begin(NodeKind::Qualified, 0);
                b.patch(0, 1);
                identifier(b, 0, 1);
                identifier(b, 2, 3);
                b.end(NodeKind::Qualified, 3);
                b.begin(NodeKind::Arguments, 3);
                b.end(NodeKind::Arguments, 5);
                b.end(NodeKind::Send, 5);
                b.end(NodeKind::ExpressionStatement, 6);
            }),
        );
        let lints = f.run(&one_rule("unnecessary_constructor_name"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, 2));
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
    /// `test_declareFinal`-family positive).
    #[test]
    fn initializing_formals_reports_a_this_assignment() {
        let source = "class A { int x = 0; A(int x) { this.x = x; } }";
        // class0 A1 {2 int3 x4 =5 06 ;7 A8 (9 int10 x11 )12 {13 this14 .15 x16 =17 x18 ;19 }20 }21
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
                b.patch(0, NONE);
                b.leaf_span(NodeKind::Type, 3, 4);
                b.begin(NodeKind::InitializedIdentifier, 4);
                identifier(b, 4, 5);
                b.begin(NodeKind::VariableInitializer, 5);
                b.leaf(NodeKind::LiteralInt, 6);
                b.end(NodeKind::VariableInitializer, 7);
                b.end(NodeKind::InitializedIdentifier, 7);
                b.end(NodeKind::VariablesDeclaration, 8);
                b.end(NodeKind::Fields, 8);
                b.begin(NodeKind::Constructor, 8);
                b.patch(0, MemberKind::NonStaticMethod as u32);
                b.patch(1, NONE);
                b.patch(2, 9);
                b.patch(3, NONE);
                identifier(b, 8, 9);
                b.begin(NodeKind::FormalParameters, 9);
                b.begin(NodeKind::FormalParameter, 10);
                b.patch(0, NONE);
                b.patch(1, 11);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                b.leaf_span(NodeKind::Type, 10, 11);
                b.end(NodeKind::FormalParameter, 12);
                b.end(NodeKind::FormalParameters, 13);
                b.begin(NodeKind::Block, 13);
                b.begin(NodeKind::ExpressionStatement, 13);
                b.begin(NodeKind::Assignment, 14);
                b.patch(0, 17);
                b.begin(NodeKind::DotAccess, 14);
                b.patch(0, 15);
                b.patch(1, NONE);
                b.leaf(NodeKind::ThisExpression, 14);
                identifier(b, 16, 17);
                b.end(NodeKind::DotAccess, 17);
                identifier(b, 18, 19);
                b.end(NodeKind::Assignment, 19);
                b.end(NodeKind::ExpressionStatement, 20);
                b.end(NodeKind::Block, 21);
                b.end(NodeKind::Constructor, 21);
                b.end(NodeKind::DeclarationBody, 22);
                b.end(NodeKind::ClassDeclaration, 22);
                b.end(NodeKind::TopLevelDeclaration, 22);
            }),
        );
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
        assert_eq!(lints[0].range.start, utf16_at(&f, 14));
        assert_eq!(lints[0].range.end, utf16_at(&f, 19));
    }

    /// The initializer face `: x = x` reports over the whole initializer.
    #[test]
    fn initializing_formals_reports_an_initializer_entry() {
        let source = "class A { int x = 0; A(int x) : x = x; }";
        // class0 A1 {2 int3 x4 =5 06 ;7 A8 (9 int10 x11 )12 :13 x14 =15 x16 ;17 }18
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
                b.patch(0, NONE);
                b.leaf_span(NodeKind::Type, 3, 4);
                b.begin(NodeKind::InitializedIdentifier, 4);
                identifier(b, 4, 5);
                b.begin(NodeKind::VariableInitializer, 5);
                b.leaf(NodeKind::LiteralInt, 6);
                b.end(NodeKind::VariableInitializer, 7);
                b.end(NodeKind::InitializedIdentifier, 7);
                b.end(NodeKind::VariablesDeclaration, 8);
                b.end(NodeKind::Fields, 8);
                b.begin(NodeKind::Constructor, 8);
                b.patch(0, MemberKind::NonStaticMethod as u32);
                b.patch(1, NONE);
                b.patch(2, 9);
                b.patch(3, 13);
                identifier(b, 8, 9);
                b.begin(NodeKind::FormalParameters, 9);
                b.begin(NodeKind::FormalParameter, 10);
                b.patch(0, NONE);
                b.patch(1, 11);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                b.leaf_span(NodeKind::Type, 10, 11);
                b.end(NodeKind::FormalParameter, 12);
                b.end(NodeKind::FormalParameters, 13);
                b.begin(NodeKind::Initializers, 13);
                b.begin(NodeKind::Initializer, 14);
                b.begin(NodeKind::Identifier, 14);
                b.patch(0, dartq_parser::IdentifierContext::FieldInitializer as u32);
                b.end(NodeKind::Identifier, 15);
                b.begin(NodeKind::FieldInitializer, 15);
                b.patch(0, 15);
                identifier(b, 16, 17);
                b.end(NodeKind::FieldInitializer, 17);
                b.end(NodeKind::Initializer, 17);
                b.end(NodeKind::Initializers, 18);
                b.leaf(NodeKind::EmptyBody, 17);
                b.end(NodeKind::Constructor, 18);
                b.end(NodeKind::DeclarationBody, 19);
                b.end(NodeKind::ClassDeclaration, 19);
                b.end(NodeKind::TopLevelDeclaration, 19);
            }),
        );
        let lints = f.run(&one_rule("prefer_initializing_formals"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, 14));
        assert_eq!(lints[0].range.end, utf16_at(&f, 17));
    }

    /// A written-type mismatch (`num` field, `int` parameter — the
    /// initializing formal would not compile) passes (:114-118).
    #[test]
    fn initializing_formals_type_mismatch_passes() {
        let source = "class A { num x = 0; A(int x) { this.x = x; } }";
        // class0 A1 {2 num3 x4 =5 06 ;7 A8 (9 int10 x11 )12 {13 this14 .15 x16 =17 x18 ;19 }20 }21
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
                b.patch(0, NONE);
                b.leaf_span(NodeKind::Type, 3, 4);
                b.begin(NodeKind::InitializedIdentifier, 4);
                identifier(b, 4, 5);
                b.begin(NodeKind::VariableInitializer, 5);
                b.leaf(NodeKind::LiteralInt, 6);
                b.end(NodeKind::VariableInitializer, 7);
                b.end(NodeKind::InitializedIdentifier, 7);
                b.end(NodeKind::VariablesDeclaration, 8);
                b.end(NodeKind::Fields, 8);
                b.begin(NodeKind::Constructor, 8);
                b.patch(0, MemberKind::NonStaticMethod as u32);
                b.patch(1, NONE);
                b.patch(2, 9);
                b.patch(3, NONE);
                identifier(b, 8, 9);
                b.begin(NodeKind::FormalParameters, 9);
                b.begin(NodeKind::FormalParameter, 10);
                b.patch(0, NONE);
                b.patch(1, 11);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                b.leaf_span(NodeKind::Type, 10, 11);
                b.end(NodeKind::FormalParameter, 12);
                b.end(NodeKind::FormalParameters, 13);
                b.begin(NodeKind::Block, 13);
                b.begin(NodeKind::ExpressionStatement, 13);
                b.begin(NodeKind::Assignment, 14);
                b.patch(0, 17);
                b.begin(NodeKind::DotAccess, 14);
                b.patch(0, 15);
                b.patch(1, NONE);
                b.leaf(NodeKind::ThisExpression, 14);
                identifier(b, 16, 17);
                b.end(NodeKind::DotAccess, 17);
                identifier(b, 18, 19);
                b.end(NodeKind::Assignment, 19);
                b.end(NodeKind::ExpressionStatement, 20);
                b.end(NodeKind::Block, 21);
                b.end(NodeKind::Constructor, 21);
                b.end(NodeKind::DeclarationBody, 22);
                b.end(NodeKind::ClassDeclaration, 22);
                b.end(NodeKind::TopLevelDeclaration, 22);
            }),
        );
        assert!(f.run(&one_rule("prefer_initializing_formals")).is_empty());
    }

    /// A second reference to the parameter keeps the conversion potentially
    /// semantic — no report (:208-222, count > 1).
    #[test]
    fn initializing_formals_extra_reference_passes() {
        let source = "class A { int x = 0; A(int x) { this.x = x; print(x); } }";
        // class0 A1 {2 int3 x4 =5 06 ;7 A8 (9 int10 x11 )12 {13 this14 .15 x16 =17 x18 ;19 print20 (21 x22 )23 ;24 }25 }26
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
                b.patch(0, NONE);
                b.leaf_span(NodeKind::Type, 3, 4);
                b.begin(NodeKind::InitializedIdentifier, 4);
                identifier(b, 4, 5);
                b.begin(NodeKind::VariableInitializer, 5);
                b.leaf(NodeKind::LiteralInt, 6);
                b.end(NodeKind::VariableInitializer, 7);
                b.end(NodeKind::InitializedIdentifier, 7);
                b.end(NodeKind::VariablesDeclaration, 8);
                b.end(NodeKind::Fields, 8);
                b.begin(NodeKind::Constructor, 8);
                b.patch(0, MemberKind::NonStaticMethod as u32);
                b.patch(1, NONE);
                b.patch(2, 9);
                b.patch(3, NONE);
                identifier(b, 8, 9);
                b.begin(NodeKind::FormalParameters, 9);
                b.begin(NodeKind::FormalParameter, 10);
                b.patch(0, NONE);
                b.patch(1, 11);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                b.leaf_span(NodeKind::Type, 10, 11);
                b.end(NodeKind::FormalParameter, 12);
                b.end(NodeKind::FormalParameters, 13);
                b.begin(NodeKind::Block, 13);
                b.begin(NodeKind::ExpressionStatement, 13);
                b.begin(NodeKind::Assignment, 14);
                b.patch(0, 17);
                b.begin(NodeKind::DotAccess, 14);
                b.patch(0, 15);
                b.patch(1, NONE);
                b.leaf(NodeKind::ThisExpression, 14);
                identifier(b, 16, 17);
                b.end(NodeKind::DotAccess, 17);
                identifier(b, 18, 19);
                b.end(NodeKind::Assignment, 19);
                b.end(NodeKind::ExpressionStatement, 20);
                b.begin(NodeKind::ExpressionStatement, 20);
                b.begin(NodeKind::Send, 20);
                identifier(b, 20, 21);
                b.begin(NodeKind::Arguments, 21);
                identifier(b, 22, 23);
                b.end(NodeKind::Arguments, 24);
                b.end(NodeKind::Send, 24);
                b.end(NodeKind::ExpressionStatement, 25);
                b.end(NodeKind::Block, 26);
                b.end(NodeKind::Constructor, 26);
                b.end(NodeKind::DeclarationBody, 27);
                b.end(NodeKind::ClassDeclaration, 27);
                b.end(NodeKind::TopLevelDeclaration, 27);
            }),
        );
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
}
