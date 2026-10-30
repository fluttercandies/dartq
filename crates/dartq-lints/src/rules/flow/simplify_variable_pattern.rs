use dartq_ast::NodeKind;

use super::{Check, PayloadField, RuleSpec};
use crate::rules::{MemberQuery, NONE_TOKEN, utf16_token_span};

/// Official: `upstream Dart SDK source,
/// pkg/linter/lib/src/rules/simplify_variable_pattern.dart` — a
/// pattern field with an explicit name whose variable pattern
/// declares the same name reports at the field name with the member
/// kind spelled into the message (`isEven: var isEven` → "The getter
/// identification 'isEven:' is redundant...", :41-80, :88-118).
///
/// Gate: the official registration bails without the patterns feature
/// (:32) — the same [`crate::engine::Features::patterns`] bit the
/// parser keys.
///
/// Faces: the field name is the identifier anchored immediately
/// before the colon payload token (the named-argument anchor face);
/// the sub-pattern root peels the parenthesized wrappers (whose
/// content rows leak into the field's window) and must be a declared
/// variable pattern (:42-45). The member kind resolves over the
/// containing pattern: a same-file class chain answers method, getter,
/// or field (:82-86); a same-file class WITHOUT the member stays
/// silent (the official `lookUpGetter`/`lookUpMethod` null bail,
/// :99-104). An unresolvable type name unwraps through the same-file
/// generic-type-alias chain and the nearest enclosing type-parameter
/// bound first: a record answers `field` on its declared named field
/// and stays silent on a miss, a function type spells `field` for the
/// `call` face and stays silent on every other name, a same-file class
/// joins the chain walk, and the bare name answers `field` for
/// `dynamic` and the unresolved `call` face (the official `_accessor`
/// falls to `_ => 'field'` wherever the resolved element is neither
/// method nor getter; conformance
/// lints/simplify_variable_pattern/{test_patternField_dynamic,
/// test_patternField_function,test_typeParameter_record,
/// test_typeParameter_functionType,test_typeParameter_typedef_record,
/// test_typedef_record}), answers getter for a named member of the
/// enumerated dart:core faces (`int`/`num`/`double`/`String`/`bool`/
/// `Object` — the official int.isEven / Object.hashCode corpus
/// getters) and stays silent on their miss
/// (test_patternField_unnexistingProperty), getter for every other
/// unresolvable face (an import, an unknown name).
///
/// Resolution guard (documented approximation): the official reports
/// only when the member exists (the guard is resolution); the
/// name-level face reports whenever the names match, except where the
/// plumbing above establishes existence — a record type (alias, bound,
/// or the record pattern's matched value resolved through the
/// scrutinee's written parameter type) and the dart:core member
/// tables — so code the analyzer already rejects there (an undefined
/// getter, a missing record field) stays silent where the official
/// defers to the compile error
/// (test_recordDestructuring_unnexistingField). Message:
/// `diagnostic.g.dart` `simplifyVariablePattern`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "simplify_variable_pattern",
    &[NodeKind::PatternField],
    check,
);

fn check(c: &mut Check<'_>) {
    // The registration bail (:32).
    if !c.unit.features.patterns {
        return;
    }
    let node = c.node();
    // `node.name == null` skips (:44). The row's `Colon` word carries
    // the field-name token for the named face (the parser hands the
    // `ensureIdentifier` token, patterns.rs object-pattern rest), the
    // colon itself for the name-less `(: sub)` form, and NONE_TOKEN
    // for a positional field — the name reads straight off the word.
    let Some(name) = c.field(node, PayloadField::Colon) else {
        return;
    };
    if name == NONE_TOKEN || c.lexeme(name) == ":" {
        return;
    }
    let lexeme = c.lexeme(name);
    let Some(pattern) = sub_pattern(c, node) else {
        return;
    };
    // The variable name equals the field name (:44-45).
    let Some(variable) = c.field(pattern, PayloadField::Variable) else {
        return;
    };
    if c.lexeme(variable) != lexeme {
        return;
    }
    let Some(container) = pattern_container(c, node) else {
        return;
    };
    let Some(accessor) = accessor(c, container, node, lexeme) else {
        return;
    };
    c.report(utf16_token_span(c, name), &[lexeme, accessor]);
}

/// The sub-pattern root: the rows completing inside the field's own
/// span right before it (the production tiling seals the field
/// childless — the name identifier sits ahead of the span, the
/// sub-pattern pieces tile inside it), peeling the parenthesized
/// wrappers; `DeclaredVariablePattern` only (:43). A hand-built tree
/// keeps the rows as direct children instead.
fn sub_pattern(c: &Check<'_>, node: u32) -> Option<u32> {
    let span = c.byte_span(node);
    let kids = c.collect_direct(node);
    let mut rows: Vec<u32> = if kids.is_empty() {
        (0..node)
            .rev()
            .take_while(|&row| c.byte_span(row).start >= span.start)
            .collect()
    } else {
        kids
    };
    loop {
        let root = *rows.iter().max_by_key(|&&row| {
            let span = c.byte_span(row);
            // The `var`-no-type sentinel spans the variable region
            // identically to its `DeclaredVariablePattern` — a
            // pattern row outranks the marker on an exact tie.
            let pattern = matches!(
                c.kind(row),
                NodeKind::DeclaredVariablePattern | NodeKind::ParenthesizedPattern
            );
            (span.end, core::cmp::Reverse(span.start), pattern)
        })?;
        match c.kind(root) {
            NodeKind::ParenthesizedPattern => {
                let span = c.byte_span(root);
                rows.retain(|&row| {
                    row != root
                        && c.byte_span(row).start >= span.start
                        && c.byte_span(row).end <= span.end
                });
            }
            NodeKind::DeclaredVariablePattern => return Some(root),
            _ => return None,
        }
    }
}

/// The field's enclosing pattern: a hand-built tree answers the
/// parent row; the production tiling seals every field before its
/// container event, so the container is the nearest pattern row after
/// the field whose span covers it (sibling fields seal in between,
/// so `node + 1` alone does not suffice).
fn pattern_container(c: &Check<'_>, node: u32) -> Option<u32> {
    if let Some(parent) = c.ancestor(1)
        && matches!(
            c.kind(parent),
            NodeKind::ObjectPattern | NodeKind::RecordPattern
        )
    {
        return Some(parent);
    }
    let span = c.byte_span(node);
    (node + 1..c.unit.ast.len() as u32).find(|&row| {
        matches!(
            c.kind(row),
            NodeKind::ObjectPattern | NodeKind::RecordPattern
        ) && {
            let outer = c.byte_span(row);
            outer.start <= span.start && outer.end >= span.end
        }
    })
}

/// The member-kind word (`_accessor`, :82-86): the containing
/// `ObjectPattern` resolves the object type, a `RecordPattern` checks its
/// matched value's written record type, and any other container spells
/// `field`.
fn accessor(c: &Check<'_>, container: u32, node: u32, name: &str) -> Option<&'static str> {
    match c.kind(container) {
        NodeKind::ObjectPattern => object_member_kind(c, container, node, name),
        NodeKind::RecordPattern => record_pattern_field_kind(c, container, name),
        // Any other container spells `field`.
        _ => Some("field"),
    }
}

/// The member face of the object pattern's type: the same-file class
/// chain answers method/getter/field and stays silent without the
/// member (the official lookUp bail, :99-104); an unresolvable type
/// name unwraps through the alias/bound resolution first — a record
/// answers `field` on its declared named field, a function type spells
/// `field` for `call`, the bare name answers `field` for `dynamic` and
/// the unresolved `call` face and by dart:core membership otherwise.
fn object_member_kind<'a>(
    c: &Check<'a>,
    object: u32,
    node: u32,
    name: &str,
) -> Option<&'static str> {
    let head = c.field(object, PayloadField::FirstName)?;
    // A prefixed container (`prefix.A(...)`, the `Period` word set)
    // resolves outside the file.
    let type_name = if c
        .field(object, PayloadField::Period)
        .is_some_and(|dot| dot != NONE_TOKEN)
    {
        c.field(object, PayloadField::SecondName).unwrap_or(head)
    } else {
        head
    };
    match resolved_type(c, node, c.lexeme(type_name), 0) {
        Written::Class(row) => chain_member_kind(c, row, name),
        Written::Record(row) => record_named_field_kind(c, row, name),
        // A bound/alias to a function type: `call` is the only instance
        // face, spelled `field` (`_accessor`'s fall-through) — any other
        // name has no member.
        Written::Function => (name == "call").then_some("field"),
        Written::Named(unresolved) => {
            // Unresolved faces: `dynamic` carries no members and an
            // unresolved `call` field can only be the function-type call
            // (a declared `call` resolved through the chain above) — the
            // oracle spells `field` for both; the enumerated dart:core
            // faces answer by membership (the int.isEven / Object.hashCode
            // corpus getters, the `isEvenn` miss silent); every other
            // unresolvable face (an import, an unknown name) is a real
            // getter.
            if unresolved == "dynamic" || name == "call" {
                return Some("field");
            }
            core_getter(unresolved, name)
        }
    }
}

/// The member face of a resolved same-file class chain: method, getter,
/// or field (:82-86), silent without the member (the official lookUp
/// bail, :99-104). The depth cap bounds alias-cycle supertype chains at
/// the getter default.
fn chain_member_kind(c: &Check<'_>, class_row: u32, name: &str) -> Option<&'static str> {
    let mut seen = 0usize;
    let mut current = Some(class_row);
    while let Some(row) = current {
        seen += 1;
        if seen > 8 {
            return Some("getter");
        }
        for member in crate::rules::body_members(c, row) {
            if !crate::rules::member_matches(c, member, MemberQuery::Method, name).is_empty() {
                return Some("method");
            }
            if !crate::rules::member_matches(c, member, MemberQuery::Getter, name).is_empty() {
                return Some("getter");
            }
            if !crate::rules::member_matches(c, member, MemberQuery::Variable, name).is_empty() {
                return Some("field");
            }
        }
        current = None;
        for (supertype, _) in crate::rules::supertypes(c, row) {
            if let Some(row) = crate::rules::same_file_declaration(c, supertype) {
                current = Some(row);
                break;
            }
        }
    }
    None
}

/// The written-type verdict of a container type name: the same-file
/// class-like the name declares, the record or function type an alias or
/// bound unwraps to, or the bare name when nothing resolves (the
/// bounded-hop cycle answer).
enum Written<'a> {
    Class(u32),
    Record(u32),
    Function,
    Named(&'a str),
}

/// The resolution walk over a container type name: the same-file
/// declaration index first, then the same-file generic-type-alias chain
/// (`typedef R = ({int value});`) and the nearest enclosing
/// type-parameter bound (`<R extends ({int value})>`), each named hop
/// recursing — bounded, so an alias/bound cycle answers the bare name.
fn resolved_type<'a>(c: &Check<'a>, node: u32, name: &'a str, depth: u8) -> Written<'a> {
    if depth > 8 {
        return Written::Named(name);
    }
    if let Some(row) = crate::rules::same_file_declaration(c, name) {
        return Written::Class(row);
    }
    if let Some(target) = same_file_alias_target(c, name) {
        return match c.kind(target) {
            NodeKind::RecordType => Written::Record(target),
            NodeKind::FunctionType => Written::Function,
            _ => resolved_type(c, node, c.lexeme(c.head_token(target)), depth + 1),
        };
    }
    if let Some(bound) = type_parameter_bound(c, node, name)
        && let Some(row) = first_type_row(c, bound)
    {
        return match c.kind(row) {
            NodeKind::RecordType => Written::Record(row),
            NodeKind::FunctionType => Written::Function,
            NodeKind::Type => resolved_type(c, node, c.lexeme(c.head_token(row)), depth + 1),
            // A `void` bound never resolves a member.
            _ => Written::Named(name),
        };
    }
    Written::Named(name)
}

/// The aliased-type row of the same-file generic alias `typedef name = …`
/// — the direct `Type`/`FunctionType`/`RecordType` child (the old
/// function-typedef form carries no `=` and aliases nothing).
fn same_file_alias_target(c: &Check<'_>, name: &str) -> Option<u32> {
    let root = c.unit.ast.root()?;
    for top in c.collect_direct(root) {
        for inner in c.collect_direct(top) {
            if c.kind(inner) != NodeKind::Typedef
                || c.field(inner, PayloadField::Equals).unwrap_or(NONE_TOKEN) == NONE_TOKEN
            {
                continue;
            }
            let kids = c.collect_direct(inner);
            let named = kids.iter().any(|&kid| {
                c.kind(kid) == NodeKind::Identifier && c.lexeme(c.head_token(kid)) == name
            });
            if !named {
                continue;
            }
            return kids.into_iter().find(|&kid| {
                matches!(
                    c.kind(kid),
                    NodeKind::Type | NodeKind::FunctionType | NodeKind::RecordType
                )
            });
        }
    }
    None
}

/// The nearest enclosing declaration binding `name` as a type variable:
/// the parent walk reads each ancestor's `TypeVariables` children, a
/// variable's name being the `Identifier` sibling before its
/// `TypeVariable` row (the row itself holds only the bound/`NoType`) —
/// the innermost binding wins. `Some(the TypeVariable row)`.
fn type_parameter_bound(c: &Check<'_>, from: u32, name: &str) -> Option<u32> {
    let mut row = from;
    loop {
        row = c.unit.ast.parent(row)?;
        for group in c.collect_direct(row) {
            if c.kind(group) != NodeKind::TypeVariables {
                continue;
            }
            let parameters = c.collect_direct(group);
            for (index, &candidate) in parameters.iter().enumerate() {
                if c.kind(candidate) != NodeKind::TypeVariable {
                    continue;
                }
                let declared = parameters[..index]
                    .iter()
                    .rev()
                    .copied()
                    .find(|&kid| c.kind(kid) == NodeKind::Identifier);
                if declared.is_some_and(|declared| c.lexeme(c.head_token(declared)) == name) {
                    return Some(candidate);
                }
            }
        }
    }
}

/// The first written-type row among the direct children (`Type`,
/// `VoidType`, `FunctionType`, `RecordType` — the family bounds and alias
/// bodies tile as).
fn first_type_row(c: &Check<'_>, row: u32) -> Option<u32> {
    c.collect_direct(row).into_iter().find(|&kid| {
        matches!(
            c.kind(kid),
            NodeKind::Type | NodeKind::VoidType | NodeKind::FunctionType | NodeKind::RecordType
        )
    })
}

/// The record-pattern face: the matched value's written record type
/// resolves when the scrutinee is a plain identifier bound by an
/// enclosing callable's parameter (the declaration form of record
/// destructuring) — then the named field must exist on the record; any
/// other scrutinee shape keeps the name-level `field` (the documented
/// approximation — the official defers a missing record field to the
/// compile error).
fn record_pattern_field_kind(c: &Check<'_>, container: u32, name: &str) -> Option<&'static str> {
    match scrutinee_record_type(c, container) {
        Some(record) => record_named_field_kind(c, record, name),
        None => Some("field"),
    }
}

/// The written record type of the record pattern's matched value: the
/// pattern declaration's `=`-following initializer is a plain identifier
/// bound by a written parameter of the enclosing callable — the record
/// type itself, or a name resolving through [`resolved_type`] to a record.
fn scrutinee_record_type(c: &Check<'_>, container: u32) -> Option<u32> {
    let stmt = record_pattern_statement(c, container)?;
    let equals = c.field(stmt, PayloadField::Equals).unwrap_or(NONE_TOKEN);
    if equals == NONE_TOKEN {
        return None;
    }
    // The statement event seals childless (its pieces tile as sibling
    // rows — the sub-pattern read): the rows before it whose start sits
    // inside its span, the plain identifier after `=` being the
    // scrutinee.
    let span = c.byte_span(stmt);
    let init = (0..stmt)
        .rev()
        .take_while(|&row| c.byte_span(row).start >= span.start)
        .find(|&row| c.kind(row) == NodeKind::Identifier && c.head_token(row) > equals)?;
    let value = c.lexeme(c.head_token(init));
    let holder = enclosing_callable(c, stmt)?;
    let parameters = c
        .collect_direct(holder)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)?;
    for parameter in c.collect_direct(parameters) {
        let named = c
            .collect_direct(parameter)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::Identifier)
            .map(|kid| c.lexeme(c.head_token(kid)));
        if named.as_deref() != Some(value) {
            continue;
        }
        return match first_type_row(c, parameter) {
            Some(row) if c.kind(row) == NodeKind::RecordType => Some(row),
            Some(row) if c.kind(row) == NodeKind::Type => {
                match resolved_type(c, container, c.lexeme(c.head_token(row)), 0) {
                    Written::Record(record) => Some(record),
                    _ => None,
                }
            }
            _ => None,
        };
    }
    None
}

/// The pattern declaration owning the record pattern: a hand-built tree
/// answers the parent row; the production tiling seals the pattern before
/// the statement event, so the statement is the nearest
/// `PatternVarDeclStatement` after the pattern whose span covers it (the
/// [`pattern_container`] read).
fn record_pattern_statement(c: &Check<'_>, container: u32) -> Option<u32> {
    if let Some(parent) = c.unit.ast.parent(container)
        && c.kind(parent) == NodeKind::PatternVarDeclStatement
    {
        return Some(parent);
    }
    let span = c.byte_span(container);
    (container + 1..c.unit.ast.len() as u32).find(|&row| {
        c.kind(row) == NodeKind::PatternVarDeclStatement && {
            let outer = c.byte_span(row);
            outer.start <= span.start && outer.end >= span.end
        }
    })
}

/// The enclosing callable of a statement (method, top-level function,
/// local function, constructor, factory, function expression) — its
/// `FormalParameters` child binds the scrutinee.
fn enclosing_callable(c: &Check<'_>, from: u32) -> Option<u32> {
    let mut row = from;
    loop {
        row = c.unit.ast.parent(row)?;
        if matches!(
            c.kind(row),
            NodeKind::Method
                | NodeKind::TopLevelMethod
                | NodeKind::LocalFunctionDeclaration
                | NodeKind::Constructor
                | NodeKind::Factory
                | NodeKind::FunctionExpression
        ) {
            return Some(row);
        }
    }
}

/// The record face: `field` when the record type declares the named
/// field, silent on a miss (the official reports only on resolution — a
/// missing record field defers to the compile error).
fn record_named_field_kind(c: &Check<'_>, record: u32, name: &str) -> Option<&'static str> {
    let named = c
        .collect_direct(record)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::RecordTypeNamedFields)?;
    for entry in c.collect_direct(named) {
        if c.kind(entry) != NodeKind::RecordTypeEntry {
            continue;
        }
        if c.collect_direct(entry)
            .into_iter()
            .any(|kid| c.kind(kid) == NodeKind::Identifier && c.lexeme(c.head_token(kid)) == name)
        {
            return Some("field");
        }
    }
    None
}

/// The dart:core instance-member enumeration behind the unresolved-name
/// guard: a named member of `int`/`num`/`double`/`String`/`bool`/`Object`
/// is a real getter (the int.isEven / Object.hashCode corpus faces) while
/// a misspelled one has no member and stays silent (the official lookUp
/// null bail). Operators and statics never answer a pattern field.
const NUM_MEMBERS: [&str; 22] = [
    "compareTo",
    "remainder",
    "abs",
    "sign",
    "clamp",
    "floor",
    "ceil",
    "round",
    "truncate",
    "floorToDouble",
    "ceilToDouble",
    "roundToDouble",
    "truncateToDouble",
    "toDouble",
    "toInt",
    "isFinite",
    "isInfinite",
    "isNegative",
    "isNaN",
    "toStringAsFixed",
    "toStringAsPrecision",
    "toStringAsExponential",
];

/// `int`'s own instance members on top of [`NUM_MEMBERS`].
const INT_MEMBERS: [&str; 9] = [
    "bitLength",
    "isEven",
    "isOdd",
    "gcd",
    "modPow",
    "modInverse",
    "toRadixString",
    "toSigned",
    "toUnsigned",
];

/// `String`'s instance members (beyond [`OBJECT_MEMBERS`]).
const STRING_MEMBERS: [&str; 29] = [
    "length",
    "isEmpty",
    "isNotEmpty",
    "codeUnits",
    "runes",
    "codeUnitAt",
    "indexOf",
    "lastIndexOf",
    "contains",
    "startsWith",
    "endsWith",
    "substring",
    "trim",
    "trimLeft",
    "trimRight",
    "padLeft",
    "padRight",
    "replaceFirst",
    "replaceFirstMapped",
    "replaceAll",
    "replaceAllMapped",
    "replaceRange",
    "split",
    "splitMapJoin",
    "toLowerCase",
    "toUpperCase",
    "compareTo",
    "allMatches",
    "matchAsPrefix",
];

/// The `Object` instance members every core type inherits.
const OBJECT_MEMBERS: [&str; 4] = ["hashCode", "toString", "noSuchMethod", "runtimeType"];

/// The getter verdict of an unresolvable named type: the enumerated
/// dart:core faces answer by membership (present → `getter`, absent →
/// silent), every other face keeps the getter default.
fn core_getter(type_name: &str, name: &str) -> Option<&'static str> {
    let known = match type_name {
        "int" => NUM_MEMBERS.contains(&name) || INT_MEMBERS.contains(&name),
        "num" | "double" => NUM_MEMBERS.contains(&name),
        "String" => STRING_MEMBERS.contains(&name),
        "bool" | "Object" => false,
        _ => return Some("getter"),
    };
    (known || OBJECT_MEMBERS.contains(&name)).then_some("getter")
}

#[cfg(test)]
mod tests {
    use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

    fn run(source: &str) -> Vec<crate::engine::Lint> {
        Fixture::parse(source).run(&one_rule("simplify_variable_pattern"))
    }

    /// `test_patternField_explicit` — the getter face, message
    /// verbatim. Pin re-derived to the oracle text: `int.isEven`
    /// resolves to a getter element, so the official `_accessor`
    /// substitutes `getter` (census test_patternField_explicit passes
    /// with exactly this text).
    #[test]
    fn named_field_with_same_name_reports() {
        let source = "void f(Object o) {\n  if (o case int(isEven:var isEven)) {}\n}\n";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("simplify_variable_pattern"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The getter identification 'isEven:' is redundant and can be removed."
        );
        assert_eq!(
            lints[0].correction.as_deref(),
            Some("Try removing the redundant getter identification.")
        );
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(source, &f.tokens, "isEven", 0))
        );
    }

    /// `test_patternField_dynamic` — the unresolved dynamic face
    /// spells `field` (the official `_accessor` falls to
    /// `_ => 'field'` without a method/getter element).
    #[test]
    fn dynamic_container_answers_the_field_word() {
        let lints = run("void f(Object o) {\n  if (o case dynamic(isEven:var isEven)) {}\n}\n");
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The field identification 'isEven:' is redundant and can be removed."
        );
    }

    /// `test_patternField_function` — the unresolved function-type
    /// `call` face spells `field`.
    #[test]
    fn function_type_call_answers_the_field_word() {
        let lints = run("void f(Object o) {\n  if (o case Function(call:var call)) {}\n}\n");
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The field identification 'call:' is redundant and can be removed."
        );
    }

    /// `test_noPatternField` / `test_patternField_implicit` /
    /// `test_patternField_otherName` — the name-less, positional, and
    /// mismatched forms stay silent.
    #[test]
    fn other_field_shapes_are_silent() {
        assert!(run("void f(Object o) {\n  if (o case int i) {}\n}\n").is_empty());
        assert!(run("void f(Object o) {\n  if (o case int(:var isEven)) {}\n}\n").is_empty());
        assert!(run("void f(Object o) {\n  if (o case int(isEven:var other)) {}\n}\n").is_empty());
    }

    /// `test_patternField_inheritedMethod` — the same-file chain
    /// answers `method`.
    #[test]
    fn inherited_method_answers_the_method_word() {
        let source = "class A {\n  void m() {}\n}\nclass B extends A {}\n\nvoid f(Object o) {\n  if (o case B(m:var m)) {}\n}\n";
        let lints = run(source);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The method identification 'm:' is redundant and can be removed."
        );
    }

    /// `test_patternField_inheritedGetter` — the chain answers
    /// `getter`.
    #[test]
    fn inherited_getter_answers_the_getter_word() {
        let source = "class A {\n  int get x => 0;\n}\nclass B extends A {}\n\nvoid f(Object o) {\n  if (o case B(x:var x)) {}\n}\n";
        let lints = run(source);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The getter identification 'x:' is redundant and can be removed."
        );
    }

    /// `test_patternField_inheritedGetter_mixin` — the `with` chain
    /// resolves like `extends`.
    #[test]
    fn mixin_chain_resolves() {
        let source = "mixin M {\n  int get x => 0;\n}\nclass C with M {}\n\nvoid f(Object o) {\n  if (o case C(x:var x)) {}\n}\n";
        assert_eq!(run(source).len(), 1);
    }

    /// A same-file class without the member stays silent (the
    /// official lookUp bail, :99-104).
    #[test]
    fn missing_member_is_silent() {
        let source =
            "class B {\n  int y = 0;\n}\n\nvoid f(Object o) {\n  if (o case B(m:var m)) {}\n}\n";
        assert!(run(source).is_empty());
    }

    /// `test_recordDestructuring` — the record face spells `field`.
    #[test]
    fn record_destructuring_answers_the_field_word() {
        let source = "void f((int, {String name}) record) {\n  var (x, name: name) = record;\n}\n";
        let lints = run(source);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The field identification 'name:' is redundant and can be removed."
        );
    }

    /// `test_patternField_parenthesized` — the peel walks every
    /// wrapper.
    #[test]
    fn parenthesized_pattern_reports() {
        assert_eq!(
            run("void f(Object o) {\n  if (o case int(isEven:((var isEven)))) {}\n}\n").len(),
            1
        );
    }

    /// The gate (:32): with the feature off the rule is silent.
    #[test]
    fn patterns_feature_off_is_silent() {
        let mut f =
            Fixture::parse("void f(Object o) {\n  if (o case int(isEven:var isEven)) {}\n}\n");
        f.features.patterns = false;
        assert!(f.run(&one_rule("simplify_variable_pattern")).is_empty());
    }

    /// `test_typeParameter_record` — the type-parameter bound unwraps to
    /// the record type; its declared named field answers `field`.
    #[test]
    fn type_parameter_record_bound_answers_field() {
        let lints = run(
            "void f<R extends ({int value})>(R o) {\n  if (o case R(value: var value)) {}\n}\n",
        );
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The field identification 'value:' is redundant and can be removed."
        );
    }

    /// `test_typeParameter_functionType` — a function-typed bound answers
    /// the `call` face with `field` and has no other member (a miss is
    /// silent, the lookUp null bail).
    #[test]
    fn type_parameter_function_bound_answers_call_field() {
        let lints = run(
            "void f<F extends void Function()>(F o) {\n  if (o case F(call: var call)) {}\n}\n",
        );
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The field identification 'call:' is redundant and can be removed."
        );
        assert!(run(
            "void f<F extends void Function()>(F o) {\n  if (o case F(length: var length)) {}\n}\n",
        )
        .is_empty());
    }

    /// `test_typeParameter_typedef_record` — the bound names a record
    /// alias; the alias hop unwraps to the record type.
    #[test]
    fn type_parameter_typedef_record_bound_answers_field() {
        let lints = run(
            "typedef R = ({int value});\n\nvoid f<T extends R>(T o) {\n  if (o case T(value: var value)) {}\n}\n",
        );
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The field identification 'value:' is redundant and can be removed."
        );
    }

    /// `test_typedef_record` — the container type itself is a record
    /// alias.
    #[test]
    fn typedef_record_container_answers_field() {
        let lints = run(
            "typedef R = ({int value});\n\nvoid f(Object o) {\n  if (o case R(value: var value)) {}\n}\n",
        );
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The field identification 'value:' is redundant and can be removed."
        );
    }

    /// `test_patternField_unnexistingProperty` — the misspelled member is
    /// in no core-type enumeration: silent (the official lookUp null
    /// bail).
    #[test]
    fn misspelled_core_member_is_silent() {
        assert!(
            run("void f(Object o) {\n  if (o case int(isEvenn:var isEvenn) when isEvenn) {}\n}\n",)
                .is_empty()
        );
    }

    /// `test_recordDestructuring_unnexistingField` — the matched record's
    /// written type resolves through the scrutinee parameter: a named
    /// field the record type does not declare stays silent.
    #[test]
    fn record_destructuring_missing_field_is_silent() {
        assert!(
            run("void f((int, {String name}) record) {\n  var (x, namee: namee) = record;\n}\n",)
                .is_empty()
        );
    }
}
