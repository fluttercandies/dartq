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
    /// (`returnType + ' ' + 'Function' + typeParameters + parameters`);
    /// slices instead of `toSource()` differ only when a comment sits inside
    /// the typedef tail (message-text blind spot, no report-position
    /// impact). A synthetic semicolon (recovery) skips (:48-49). Message:
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
    use super::{Check, Group, NodeKind, RuleSpec};
    use crate::rules::{NONE_TOKEN, first_child};

    /// Official: `pkg/linter/lib/src/rules/prefer_function_declarations_over_variables.dart:40-58`
    /// — a variable initialized with a function literal reports. Fields and
    /// top-level variables only when final (:42-49); locals unless the
    /// variable is mutated in scope (:50-55).
    ///
    /// Approximation (documented): `isPotentiallyMutatedInScope`
    /// (`prefer_function_declarations_over_variables.dart:53`) needs
    /// usage analysis and is dropped — locals always report, so a later
    /// reassignment (`var f = () {}; f = ...;`) over-reports. Message:
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
        let Some(value) = first_child(c, initializer) else {
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
        if c.ancestor(2).is_some_and(|owner| {
            matches!(c.kind(owner), NodeKind::Fields | NodeKind::TopLevelFields)
        }) {
            let keyword = c
                .payload(declaration)
                .first()
                .copied()
                .unwrap_or(NONE_TOKEN);
            if keyword == NONE_TOKEN || c.lexeme(keyword) != "final" {
                return;
            }
        }
        let range = c.utf16_span(c.node());
        c.report(range, &[]);
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

// —— shared helpers ———————————————————————————————————————————————————————

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
}
