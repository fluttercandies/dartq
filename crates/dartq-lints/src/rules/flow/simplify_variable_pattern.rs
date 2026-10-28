use dartq_ast::NodeKind;

use super::{PayloadField, Check, RuleSpec};
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
/// :99-104); every unresolvable type (dynamic, typedef, type
/// parameter, an import) answers getter, matching the official
/// dynamic/typedef/type-parameter corpus.
///
/// Approximation (documented): the official reports only when the
/// member exists (the guard is resolution); the name-level face here
/// reports whenever the names match, so code the analyzer already
/// rejects (an undefined getter, a missing record field) reports
/// where the official defers to the compile error — the lexical face
/// is exact on valid code, where the pattern's member necessarily
/// exists. Message: `diagnostic.g.dart` `simplifyVariablePattern`.
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
    let Some(accessor) = accessor(c, container, lexeme) else {
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
/// `ObjectPattern` resolves the same-file class chain (method/getter/
/// field, silent without the member); a record pattern or any other
/// container spells `field`.
fn accessor(c: &Check<'_>, container: u32, name: &str) -> Option<&'static str> {
    if c.kind(container) == NodeKind::ObjectPattern {
        return object_member_kind(c, container, name);
    }
    // A record pattern (or any other container) spells `field`.
    Some("field")
}

/// The member face of the object pattern's type: the same-file class
/// chain answers method/getter/field and stays silent without the
/// member (the official lookUp bail, :99-104); an unresolvable type —
/// dynamic, a typedef, a type parameter, an import — answers getter.
fn object_member_kind(c: &Check<'_>, object: u32, name: &str) -> Option<&'static str> {
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
    let Some(class_row) = crate::rules::same_file_declaration(c, c.lexeme(type_name)) else {
        return Some("getter");
    };
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
            if !crate::rules::member_matches(c, member, MemberQuery::Variable, name).is_empty()
            {
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

#[cfg(test)]
mod tests {
    use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

    fn run(source: &str) -> Vec<crate::engine::Lint> {
        Fixture::parse(source).run(&one_rule("simplify_variable_pattern"))
    }

    /// `test_patternField_explicit` — the getter face, message
    /// verbatim.
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

    /// `test_noPatternField` / `test_patternField_implicit` /
    /// `test_patternField_otherName` — the name-less, positional, and
    /// mismatched forms stay silent.
    #[test]
    fn other_field_shapes_are_silent() {
        assert!(run("void f(Object o) {\n  if (o case int i) {}\n}\n").is_empty());
        assert!(run("void f(Object o) {\n  if (o case int(:var isEven)) {}\n}\n").is_empty());
        assert!(
            run("void f(Object o) {\n  if (o case int(isEven:var other)) {}\n}\n").is_empty()
        );
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
        let source = "class B {\n  int y = 0;\n}\n\nvoid f(Object o) {\n  if (o case B(m:var m)) {}\n}\n";
        assert!(run(source).is_empty());
    }

    /// `test_recordDestructuring` — the record face spells `field`.
    #[test]
    fn record_destructuring_answers_the_field_word() {
        let source =
            "void f((int, {String name}) record) {\n  var (x, name: name) = record;\n}\n";
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
}
