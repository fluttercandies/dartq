//! The naming-style family: `camel_case_types`, `camel_case_extensions`,
//! `constant_identifier_names`,
//! `no_leading_underscores_for_local_identifiers`,
//! `non_constant_identifier_names`, `file_names`.

use dartq_ast::NodeKind;

use crate::engine::Check;
use crate::registry::{Group, RuleSpec};
use crate::rules::{
    has_leading_underscore, is_field_name_shortcut, is_just_underscores, is_lower_camel_case,
    is_upper_camel_case,
};

/// UTF-16 offset of a token's start.
fn utf16_start_of(c: &Check<'_>, token: u32) -> u32 {
    c.unit
        .tokens
        .get(token as usize)
        .expect("payload token outside the token table")
        .utf16_offset
}

/// Reports `token`/`lexeme` when the lowerCamelCase style check fails.
fn check_lower_camel(c: &mut Check<'_>, token: u32, lexeme: &str) {
    if is_lower_camel_case(lexeme) {
        return;
    }
    let start = utf16_start_of(c, token);
    c.report(start..start, &[lexeme]);
}

pub(crate) mod camel_case_types {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/camel_case_types.dart:33-88` —
    /// every type-introducing declaration's name must be UpperCamelCase
    /// (`isCamelCase`, `pkg/linter/lib/src/utils.dart:22`). Message:
    /// `diagnostic.g.dart` `camelCaseTypes`.
    ///
    /// Names of top-level declarations live in the enclosing
    /// [`NodeKind::TopLevelDeclaration`] wrapper (collapse map §3.1:
    /// TopLevelDeclarationEnd, the name-Identifier container). The official
    /// augmentation skips are wave-1 inert: `augment` is gated off at the
    /// scanner, so no augmentation declaration reaches the AST.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "camel_case_types",
        group: Group::Core,
        problem: "The type name '{0}' isn't an UpperCamelCase identifier.",
        correction: "Try changing the name to follow the UpperCamelCase style.",
        kinds: &[
            NodeKind::ClassDeclaration,
            NodeKind::MixinDeclaration,
            NodeKind::EnumDeclaration,
            NodeKind::ExtensionTypeDeclaration,
            NodeKind::NamedMixinApplication,
            NodeKind::Typedef,
        ],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let Some(parent) = c.ancestor(1) else {
            return;
        };
        if c.kind(parent) != NodeKind::TopLevelDeclaration {
            return;
        }
        // The wrapper's first child is the name Identifier.
        let name = c
            .direct_children(parent)
            .iter()
            .copied()
            .find(|&ch| c.kind(ch) == NodeKind::Identifier);
        let Some(name) = name else {
            return;
        };
        let token = c.head_token(name);
        let lexeme = c.lexeme(token);
        if super::is_upper_camel_case(lexeme) {
            return;
        }
        let start = super::utf16_start_of(c, token);
        c.report(start..start, &[lexeme]);
    }
}

pub(crate) mod camel_case_extensions {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/camel_case_extensions.dart:31-42`
    /// — a named extension's name must be UpperCamelCase. The name is
    /// payload word 1 of [`NodeKind::ExtensionDeclaration`]
    /// (`extension: T, name: O, on: O`, collapse map §3.2). Message:
    /// `diagnostic.g.dart` `camelCaseExtensions`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "camel_case_extensions",
        group: Group::Core,
        problem: "The extension name '{0}' isn't an UpperCamelCase identifier.",
        correction: "Try changing the name to follow the UpperCamelCase style.",
        kinds: &[NodeKind::ExtensionDeclaration],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // Anonymous extensions (`extension on X`) have no name: skip.
        let Some(&name) = c.payload(node).get(1) else {
            return;
        };
        if name == crate::rules::NONE_TOKEN {
            return;
        }
        let lexeme = c.lexeme(name);
        if super::is_upper_camel_case(lexeme) {
            return;
        }
        let start = super::utf16_start_of(c, name);
        c.report(start..start, &[lexeme]);
    }
}

pub(crate) mod constant_identifier_names {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/constant_identifier_names.dart:36-73`
    /// — `const` variables (fields, top-level, locals), enum constants, and
    /// pattern-bound variables must be lowerCamelCase (`isLowerCamelCase`,
    /// `pkg/linter/lib/src/utils.dart:31`). Object-pattern field-name
    /// shortcuts (`Foo(:name)`) are exempt. Message: `diagnostic.g.dart`
    /// `constantIdentifierNames`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "constant_identifier_names",
        group: Group::Recommended,
        problem: "The constant name '{0}' isn't a lowerCamelCase identifier.",
        correction: "Try changing the name to follow the lowerCamelCase style.",
        kinds: &[
            NodeKind::VariablesDeclaration,
            NodeKind::EnumElement,
            NodeKind::DeclaredVariablePattern,
        ],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        match c.kind(node) {
            NodeKind::VariablesDeclaration => {
                // `const`-ness: payload word 0 is the `final`/`const`/`var`
                // keyword token (collapse map §3.3 VariablesDeclaration row).
                let Some(&modifier) = c.payload(node).first() else {
                    return;
                };
                if modifier == crate::rules::NONE_TOKEN || c.lexeme(modifier) != "const" {
                    return;
                }
                // Gather under the scratch borrow, then report (the report
                // path needs `&mut Check`). Allocates only on hits.
                let mut hits: Vec<u32> = Vec::new();
                for var in c.direct_children(node).iter().copied() {
                    if c.kind(var) == NodeKind::InitializedIdentifier {
                        // The variable's name is the `InitializedIdentifier`
                        // span head (width-0 kind: the name word was
                        // eliminated as derivable, collapse map §1).
                        let name = c.head_token(var);
                        if name != crate::rules::NONE_TOKEN {
                            hits.push(name);
                        }
                    }
                }
                for name in hits {
                    super::check_lower_camel(c, name, c.lexeme(name));
                }
            }
            NodeKind::EnumElement => {
                // The constant name is the first direct Identifier child
                // (collapse map §3.2 EnumElement row: name via children).
                let name = c
                    .direct_children(node)
                    .iter()
                    .copied()
                    .find(|&ch| c.kind(ch) == NodeKind::Identifier);
                if let Some(name) = name {
                    let token = c.head_token(name);
                    super::check_lower_camel(c, token, c.lexeme(token));
                }
            }
            NodeKind::DeclaredVariablePattern => {
                // Name token is payload word 0 (`variable: T`, §3.7).
                if super::is_field_name_shortcut(c, node) {
                    return;
                }
                if let Some(&name) = c.payload(node).first() {
                    if name != crate::rules::NONE_TOKEN {
                        super::check_lower_camel(c, name, c.lexeme(name));
                    }
                }
            }
            _ => {}
        }
    }
}

pub(crate) mod no_leading_underscores_for_local_identifiers {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/no_leading_underscores_for_local_identifiers.dart:36-133`
    /// — local identifiers (local variables, catch parameters, non-named
    /// formal parameters other than field/super formals, local functions,
    /// pattern-bound variables) must not start with `_`; bare-underscore
    /// wildcard names are exempt. The official rule's second diagnostic
    /// variant (the *shadowing* rename suggestion, `:126-133`) needs the
    /// element model and is not part of wave 1. Message: `diagnostic.g.dart`
    /// `noLeadingUnderscoresForLocalIdentifiers`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "no_leading_underscores_for_local_identifiers",
        group: Group::Recommended,
        problem: "The local variable '{0}' starts with an underscore.",
        correction: "Try renaming the variable to not start with an underscore.",
        kinds: &[
            NodeKind::VariablesDeclaration,
            NodeKind::CatchClause,
            NodeKind::FormalParameters,
            NodeKind::OptionalFormalParameters,
            NodeKind::LocalFunctionDeclaration,
            NodeKind::DeclaredVariablePattern,
        ],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        match c.kind(node) {
            NodeKind::VariablesDeclaration => {
                // Locals only: fields and top-level variables are not local
                // identifiers (the official visits variable *statements* and
                // for-parts initializers, never field declarations).
                if let Some(parent) = c.ancestor(1) {
                    if matches!(c.kind(parent), NodeKind::Fields | NodeKind::TopLevelFields) {
                        return;
                    }
                }
                // Gather under the scratch borrow, then report.
                let mut hits: Vec<u32> = Vec::new();
                for var in c.direct_children(node).iter().copied() {
                    if c.kind(var) == NodeKind::InitializedIdentifier {
                        // The name is the span head (width-0 kind, §1).
                        let name = c.head_token(var);
                        if name != crate::rules::NONE_TOKEN {
                            hits.push(name);
                        }
                    }
                }
                for name in hits {
                    report_underscored(c, name);
                }
            }
            NodeKind::CatchClause => {
                let mut hits: Vec<u32> = Vec::new();
                for child in c.direct_children(node).iter().copied() {
                    if c.kind(child) == NodeKind::FormalParameter {
                        // name: T is payload word 1 (§3.3 FormalParameter row).
                        if let Some(&name) = c.payload(child).get(1) {
                            if name != crate::rules::NONE_TOKEN {
                                hits.push(name);
                            }
                        }
                    }
                }
                for name in hits {
                    report_underscored(c, name);
                }
            }
            NodeKind::FormalParameters => report_positional_formals(c, node),
            NodeKind::OptionalFormalParameters => {
                // Named parameters (the `{...}` section) are call-site API
                // surface and skipped (`!parameter.isNamed`, :101-112);
                // positional-optional (`[...]`) sections keep the check. The
                // opener is the section span head (width-0 kind, §1).
                if !c.lexeme(c.head_token(node)).starts_with('{') {
                    report_positional_formals(c, node);
                }
            }
            NodeKind::LocalFunctionDeclaration => {
                let name = c
                    .direct_children(node)
                    .iter()
                    .copied()
                    .find(|&ch| c.kind(ch) == NodeKind::Identifier);
                if let Some(name) = name {
                    report_underscored(c, c.head_token(name));
                }
            }
            NodeKind::DeclaredVariablePattern => {
                if super::is_field_name_shortcut(c, node) {
                    return;
                }
                if let Some(&name) = c.payload(node).first() {
                    if name != crate::rules::NONE_TOKEN {
                        report_underscored(c, name);
                    }
                }
            }
            _ => {}
        }
    }

    /// Checks the non-field formals of one parameters section (required or
    /// positional-optional — named sections never reach here). Field
    /// (`this.x`) and super formals are exempt (:99-104): receiver payload
    /// word 0 is the `this`/`super` token when present. Hits are gathered
    /// under the scratch borrow, then reported.
    fn report_positional_formals(c: &mut Check<'_>, node: u32) {
        let mut hits: Vec<u32> = Vec::new();
        for child in c.direct_children(node).iter().copied() {
            if c.kind(child) != NodeKind::FormalParameter {
                continue;
            }
            if let Some(&receiver) = c.payload(child).first() {
                if receiver != crate::rules::NONE_TOKEN {
                    continue;
                }
            }
            if let Some(&name) = c.payload(child).get(1) {
                if name != crate::rules::NONE_TOKEN {
                    hits.push(name);
                }
            }
        }
        for name in hits {
            report_underscored(c, name);
        }
    }

    /// `hasLeadingUnderscore && !isJustUnderscores` (ascii_utils.dart:58-79).
    fn report_underscored(c: &mut Check<'_>, token: u32) {
        let lexeme = c.lexeme(token);
        if !super::has_leading_underscore(lexeme) || super::is_just_underscores(lexeme) {
            return;
        }
        let start = super::utf16_start_of(c, token);
        c.report(start..start, &[lexeme]);
    }
}

pub(crate) mod non_constant_identifier_names {
    use super::{Check, Group, NodeKind, RuleSpec, is_just_underscores};
    use crate::rules::{NONE_TOKEN, member_name};

    /// Official: `pkg/linter/lib/src/rules/non_constant_identifier_names.dart:36-169`
    /// — every non-constant identifier (variables, functions, methods,
    /// parameters, catch parameters, constructor names, pattern variables,
    /// record field names) must be lowerCamelCase. Positions exempting
    /// just-underscores (`underscoresOk`): catch parameters, constructor
    /// names, all parameters (:57-66, 70-72, 97-107). Message:
    /// `diagnostic.g.dart` `nonConstantIdentifierNames`.
    ///
    /// Positions not dispatched (approximation, documented): pattern-field
    /// variables report through the pattern dispatch alone (the official
    /// double-visits the same name), and extension-type /
    /// primary-constructor names need the primary-constructor substrate.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "non_constant_identifier_names",
        group: Group::Core,
        problem: "The variable name '{0}' isn't a lowerCamelCase identifier.",
        correction: "Try changing the name to follow the lowerCamelCase style.",
        kinds: &[
            NodeKind::VariablesDeclaration,
            NodeKind::CatchClause,
            NodeKind::Constructor,
            NodeKind::DeclaredVariablePattern,
            NodeKind::FormalParameters,
            NodeKind::OptionalFormalParameters,
            NodeKind::TopLevelMethod,
            NodeKind::LocalFunctionDeclaration,
            NodeKind::Method,
            NodeKind::RecordLiteral,
            NodeKind::RecordType,
        ],
        check,
    };

    // One dispatch over the checked declaration kinds; the arms stay in one
    // function so the shared collect-then-check pattern reads linearly.
    #[allow(clippy::too_many_lines)]
    fn check(c: &mut Check<'_>) {
        let node = c.node();
        match c.kind(node) {
            NodeKind::VariablesDeclaration => {
                // `!node.isConst` (:159-164): the constant rule owns those.
                if c.payload(node)
                    .first()
                    .is_some_and(|&kw| kw != NONE_TOKEN && c.lexeme(kw) == "const")
                {
                    return;
                }
                let mut hits: Vec<u32> = Vec::new();
                for var in c.direct_children(node).iter().copied() {
                    if c.kind(var) == NodeKind::InitializedIdentifier {
                        let name = c.head_token(var);
                        if name != NONE_TOKEN {
                            hits.push(name);
                        }
                    }
                }
                for name in hits {
                    check_plain(c, name, false);
                }
            }
            NodeKind::CatchClause => {
                let mut hits: Vec<u32> = Vec::new();
                for child in c.direct_children(node).iter().copied() {
                    if c.kind(child) == NodeKind::FormalParameter
                        && let Some(&name) = c.payload(child).get(1)
                        && name != NONE_TOKEN
                    {
                        hits.push(name);
                    }
                }
                for name in hits {
                    check_plain(c, name, true);
                }
            }
            NodeKind::Constructor => {
                // The constructor's own name is the Identifier after the
                // class-name echo — absent on unnamed constructors (the
                // official `node.name` is null there).
                let identifiers: Vec<u32> = c
                    .direct_children(node)
                    .iter()
                    .copied()
                    .filter(|&ch| c.kind(ch) == NodeKind::Identifier)
                    .collect();
                let _class_name = identifiers.first();
                if let Some(&name) = identifiers.get(1) {
                    check_plain(c, c.head_token(name), true);
                }
            }
            NodeKind::DeclaredVariablePattern => {
                if super::is_field_name_shortcut(c, node) {
                    return;
                }
                if let Some(&name) = c.payload(node).first()
                    && name != NONE_TOKEN
                {
                    check_plain(c, name, false);
                }
            }
            NodeKind::FormalParameters | NodeKind::OptionalFormalParameters => {
                // Named parameters are checked too (only augmentations skip,
                // :101-103 — inert: augment is gated off at the scanner).
                let mut hits: Vec<u32> = Vec::new();
                for child in c.direct_children(node).iter().copied() {
                    if c.kind(child) != NodeKind::FormalParameter {
                        continue;
                    }
                    // Field formals (`this.x`) are skipped (:103-105); super
                    // formals keep the check (official skips only
                    // `FieldFormalParameter`).
                    if c.payload(child)
                        .first()
                        .copied()
                        .is_some_and(|recv| recv != NONE_TOKEN && c.lexeme(recv) == "this")
                    {
                        continue;
                    }
                    if let Some(&name) = c.payload(child).get(1)
                        && name != NONE_TOKEN
                    {
                        hits.push(name);
                    }
                }
                for name in hits {
                    check_plain(c, name, true);
                }
            }
            NodeKind::TopLevelMethod | NodeKind::LocalFunctionDeclaration => {
                let name = c
                    .direct_children(node)
                    .iter()
                    .copied()
                    .find(|&ch| c.kind(ch) == NodeKind::Identifier);
                if let Some(name) = name {
                    check_plain(c, c.head_token(name), false);
                }
            }
            NodeKind::Method => {
                // Operators keep their punctuation names (:135-138): only
                // Identifier names are checked.
                if let Some(name) =
                    member_name(c, node).filter(|&n| c.kind(n) == NodeKind::Identifier)
                {
                    check_plain(c, c.head_token(name), false);
                }
            }
            NodeKind::RecordLiteral => {
                let mut hits: Vec<u32> = Vec::new();
                for field in c.direct_children(node).iter().copied() {
                    if c.kind(field) != NodeKind::NamedRecordField {
                        continue;
                    }
                    if let Some(name) = c
                        .direct_children(field)
                        .iter()
                        .copied()
                        .find(|&ch| c.kind(ch) == NodeKind::Identifier)
                    {
                        hits.push(c.head_token(name));
                    }
                }
                for name in hits {
                    check_plain(c, name, false);
                }
            }
            NodeKind::RecordType => {
                let mut hits: Vec<u32> = Vec::new();
                for entry in c.direct_children(node).iter().copied() {
                    match c.kind(entry) {
                        NodeKind::RecordTypeEntry => collect_entry_name(c, entry, &mut hits),
                        NodeKind::RecordTypeNamedFields => {
                            for sub in c.direct_children(entry).iter().copied() {
                                if c.kind(sub) == NodeKind::RecordTypeEntry {
                                    collect_entry_name(c, sub, &mut hits);
                                }
                            }
                        }
                        _ => {}
                    }
                }
                for name in hits {
                    check_plain(c, name, false);
                }
            }
            _ => {}
        }
    }

    fn collect_entry_name(c: &Check<'_>, entry: u32, hits: &mut Vec<u32>) {
        if let Some(name) = c
            .direct_children(entry)
            .iter()
            .copied()
            .find(|&ch| c.kind(ch) == NodeKind::Identifier)
        {
            hits.push(c.head_token(name));
        }
    }

    /// `checkIdentifier` (:53-65): the lowerCamelCase gate; `underscores_ok`
    /// exempts just-underscore names (`___` in callbacks).
    fn check_plain(c: &mut Check<'_>, token: u32, underscores_ok: bool) {
        let lexeme = c.lexeme(token);
        if underscores_ok && is_just_underscores(lexeme) {
            return;
        }
        if super::is_lower_camel_case(lexeme) {
            return;
        }
        let start = super::utf16_start_of(c, token);
        c.report(start..start, &[lexeme]);
    }
}

pub(crate) mod file_names {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/file_names.dart:24-42` — the file
    /// base name must be `lowercase_with_underscores`
    /// (`isValidDartFileName`, `util/ascii_utils.dart:20-52`), reported at
    /// offset 0. Runs once per compilation unit; a run without a path
    /// mirrors the official `element != null` guard by skipping. The
    /// official checks the *library*'s file (via the element model); for
    /// part files that diverges from this unit-level check.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "file_names",
        group: Group::Core,
        problem: "The file name '{0}' isn't a lower_case_with_underscores identifier.",
        correction: "Try changing the name to follow the lower_case_with_underscores style.",
        kinds: &[NodeKind::CompilationUnit],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let Some(path) = c.unit.path else {
            return;
        };
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            return;
        };
        if is_valid_dart_file_name(name) {
            return;
        }
        c.report(0..0, &[name]);
    }

    /// `isValidDartFileName` (ascii_utils.dart:20-52): short names and
    /// non-`.dart` names pass; a dot inside the stem (a prefixed suffix
    /// like `.g.dart`) passes; the stem itself must be `[a-z_]` with digits
    /// allowed from position 1.
    // The official `isValidDartFileName` comparison is case-sensitive.
    #[allow(clippy::case_sensitive_file_extension_comparisons)]
    fn is_valid_dart_file_name(name: &str) -> bool {
        let bytes = name.as_bytes();
        if name.len() < 6 || !name.ends_with(".dart") {
            return true;
        }
        let stem = name.len() - 5;
        // With a 1- or 2-char stem the official `for (i = 1; i < length - 1)`
        // loop body never runs — the guarded range mirrors that empty scan.
        if stem > 1 {
            for &b in &bytes[1..stem - 1] {
                if b == b'.' {
                    return true;
                }
            }
        }
        for (i, &b) in bytes[..stem].iter().enumerate() {
            if b.is_ascii_lowercase() || b == b'_' {
                continue;
            }
            if b.is_ascii_digit() {
                if i == 0 {
                    return false;
                }
                continue;
            }
            return false;
        }
        true
    }
}

pub(crate) mod no_wildcard_variable_uses {
    use super::{Check, Group, NodeKind, RuleSpec};
    use dartq_parser::IdentifierContext;

    /// Official: `pkg/linter/lib/src/rules/no_wildcard_variable_uses.dart:36-77`
    /// — with the wildcard-variables feature OFF (`hasWildcardVariablesFeature
    /// Enabled` bails the registration, :36-38) a use of a `_`-named local
    /// variable or formal parameter reports at the use.
    ///
    /// The element test (`LocalVariableElement` / `FormalParameterElement`,
    /// :72-76) is the binding test: the use reports only when a wildcard
    /// local/parameter binds over it. The engine collects that table in the
    /// same DFS ([`crate::engine::WildcardScope`] — locals over their innermost
    /// block/for-in/catch clause, parameters over their function, catch
    /// parameters over the catch clause; fields and getters never bind, so
    /// `print(_)` beside a `_` field stays silent exactly like the official
    /// `test_field`). Pattern-bound wildcard variables under-report
    /// (declaration-context identifiers are excluded, mirroring the official
    /// `test_localVar` shape where only the use reports). Message:
    /// `diagnostic.g.dart` `noWildcardVariableUses`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "no_wildcard_variable_uses",
        group: Group::Core,
        problem: "The referenced identifier is a wildcard.",
        correction: "Use an identifier name that is not a wildcard.",
        kinds: &[NodeKind::Identifier],
        check,
    };

    fn check(c: &mut Check<'_>) {
        // Feature gate: with wildcards on, `_` never binds, so there is no
        // wildcard use to flag — the official never registers (:36-38).
        if c.unit.features.wildcard_variables {
            return;
        }
        let node = c.node();
        // Uses only: the expression contexts (the official `test_localVar`
        // reports the `print(_)` use, never the declaration).
        let Some(&ctx) = c.payload(node).first() else {
            return;
        };
        if ctx != IdentifierContext::Expression as u32
            && ctx != IdentifierContext::ExpressionContinuation as u32
        {
            return;
        }
        if !super::is_just_underscores(c.lexeme(c.head_token(node))) {
            return;
        }
        let name = c.lexeme(c.head_token(node));
        let use_start = c.byte_span(node).start;
        let bound = c.wildcards.iter().any(|w| {
            &c.unit.source[w.name.clone()] == name
                && w.scope.start <= use_start
                && use_start < w.scope.end
        });
        if !bound {
            return;
        }
        let range = c.utf16_span(node);
        c.report(range, &[]);
    }
}

#[cfg(test)]
mod tests {
    use dartq_ast::NodeKind;

    use crate::testsupport::{Fixture, NONE, identifier, one_rule, tok, unit_of, utf16_at};

    // Tests share tree shapes per collapse-map section; each cites the
    // official case it pins (`test/rules/<rule>_test.dart`).

    /// `test/rules/camel_case_types_test.dart` `test_class_alias`-family —
    /// every declaration form reports at its name.
    #[test]
    fn camel_case_types_reports_a_bad_class_name() {
        let source = "class c {}";
        // class0 c1 {2 }3
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
                b.end(NodeKind::ClassDeclaration, 4);
                b.end(NodeKind::TopLevelDeclaration, 4);
            }),
        );
        let lints = f.run(&one_rule("camel_case_types"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "The type name 'c' isn't an UpperCamelCase identifier."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 1));
    }

    #[test]
    fn camel_case_types_passes_an_upper_name() {
        let source = "class C {}";
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
                b.end(NodeKind::ClassDeclaration, 4);
                b.end(NodeKind::TopLevelDeclaration, 4);
            }),
        );
        assert!(f.run(&one_rule("camel_case_types")).is_empty());
    }

    /// Enum declarations report too (`test_enum` in the official test).
    #[test]
    fn camel_case_types_reports_a_bad_enum_name() {
        let source = "enum e { x }";
        // enum0 e1 {2 x3 }4
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelDeclaration, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::EnumDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, 2);
                b.patch(2, NONE);
                b.begin(NodeKind::EnumBody, 2);
                b.begin(NodeKind::EnumElement, 3);
                b.patch(0, 3);
                b.patch(1, NONE);
                identifier(b, 3, 4);
                b.end(NodeKind::EnumElement, 4);
                b.end(NodeKind::EnumBody, 5);
                b.end(NodeKind::EnumDeclaration, 5);
                b.end(NodeKind::TopLevelDeclaration, 5);
            }),
        );
        let lints = f.run(&one_rule("camel_case_types"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, 1));
    }

    /// `test/rules/camel_case_extensions_test.dart` `test_extension`.
    #[test]
    fn camel_case_extensions_reports_a_bad_name() {
        let source = "extension e on int {}";
        // extension0 e1 on2 int3 {4 }5
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExtensionDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, 1);
                b.patch(2, 2);
                b.end(NodeKind::ExtensionDeclaration, 6);
            }),
        );
        let lints = f.run(&one_rule("camel_case_extensions"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "The extension name 'e' isn't an UpperCamelCase identifier."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 1));
    }

    /// Anonymous extensions have no name to check.
    #[test]
    fn camel_case_extensions_anonymous_passes() {
        let source = "extension on int {}";
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExtensionDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, 1);
                b.end(NodeKind::ExtensionDeclaration, 5);
            }),
        );
        assert!(f.run(&one_rule("camel_case_extensions")).is_empty());
    }

    /// `test/rules/constant_identifier_names_test.dart` `test_const`.
    #[test]
    fn constant_identifier_names_reports_a_const_variable() {
        let source = "const PI = 3.14;";
        // const0 PI1 =2 3.143 ;4
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, tok(source, t, "const", 0));
                b.begin(NodeKind::InitializedIdentifier, 1);
                b.begin(NodeKind::VariableInitializer, 2);
                b.leaf(NodeKind::LiteralDouble, 3);
                b.end(NodeKind::VariableInitializer, 4);
                b.end(NodeKind::InitializedIdentifier, 4);
                b.end(NodeKind::VariablesDeclaration, 5);
            }),
        );
        let lints = f.run(&one_rule("constant_identifier_names"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "The constant name 'PI' isn't a lowerCamelCase identifier."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 1));
    }

    #[test]
    fn constant_identifier_names_lower_camel_passes() {
        let source = "const pi = 3.14;";
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, tok(source, t, "const", 0));
                b.begin(NodeKind::InitializedIdentifier, 1);
                b.begin(NodeKind::VariableInitializer, 2);
                b.leaf(NodeKind::LiteralDouble, 3);
                b.end(NodeKind::VariableInitializer, 4);
                b.end(NodeKind::InitializedIdentifier, 4);
                b.end(NodeKind::VariablesDeclaration, 5);
            }),
        );
        assert!(f.run(&one_rule("constant_identifier_names")).is_empty());
    }

    /// Non-`const` declarations are not constants.
    #[test]
    fn constant_identifier_names_non_const_passes() {
        let source = "var PI = 1;";
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, tok(source, t, "var", 0));
                b.begin(NodeKind::InitializedIdentifier, 1);
                b.end(NodeKind::InitializedIdentifier, 2);
                b.end(NodeKind::VariablesDeclaration, 2);
            }),
        );
        assert!(f.run(&one_rule("constant_identifier_names")).is_empty());
    }

    /// Enum constants are checked (`test_enum` in the official test).
    #[test]
    fn constant_identifier_names_reports_a_bad_enum_constant() {
        let source = "enum C { DEFAULT_COLOR }";
        // enum0 C1 {2 DEFAULT_COLOR3 }4
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelDeclaration, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::EnumDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, 2);
                b.patch(2, NONE);
                b.begin(NodeKind::EnumBody, 2);
                b.begin(NodeKind::EnumElement, 3);
                b.patch(0, 3);
                b.patch(1, NONE);
                identifier(b, 3, 4);
                b.end(NodeKind::EnumElement, 4);
                b.end(NodeKind::EnumBody, 5);
                b.end(NodeKind::EnumDeclaration, 5);
                b.end(NodeKind::TopLevelDeclaration, 5);
            }),
        );
        let lints = f.run(&one_rule("constant_identifier_names"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, 3));
    }

    /// `test/rules/no_leading_underscores_for_local_identifiers_test.dart`
    /// `test_localVariable`.
    #[test]
    fn no_leading_underscores_reports_a_local() {
        let source = "var _x = 1;";
        // var0 _x1 =2 13 ;4 — a local sits in a block
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::Block, 0);
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, 0);
                b.begin(NodeKind::InitializedIdentifier, 1);
                b.begin(NodeKind::VariableInitializer, 2);
                b.leaf(NodeKind::LiteralInt, 3);
                b.end(NodeKind::VariableInitializer, 4);
                b.end(NodeKind::InitializedIdentifier, 4);
                b.end(NodeKind::VariablesDeclaration, 5);
                b.end(NodeKind::Block, 5);
            }),
        );
        let lints = f.run(&one_rule("no_leading_underscores_for_local_identifiers"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "The local variable '_x' starts with an underscore."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 1));
    }

    /// `_` alone is the wildcard name — allowed (ascii_utils.dart:61).
    #[test]
    fn no_leading_underscores_bare_underscore_passes() {
        let source = "var _ = 1;";
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::Block, 0);
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, 0);
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
            f.run(&one_rule("no_leading_underscores_for_local_identifiers"))
                .is_empty()
        );
    }

    /// `test_catchParameter` in the official test.
    #[test]
    fn no_leading_underscores_reports_a_catch_parameter() {
        let source = "try {} catch (_e) {}";
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TryStatement, 0);
                b.patch(0, NONE);
                b.leaf_span(NodeKind::Block, 1, 3);
                b.begin(NodeKind::CatchClause, 3);
                b.patch(0, 3);
                b.patch(1, NONE);
                b.begin(NodeKind::FormalParameter, 5);
                b.patch(0, NONE);
                b.patch(1, 5);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                b.end(NodeKind::FormalParameter, 6);
                b.begin(NodeKind::Block, 7);
                b.end(NodeKind::Block, 9);
                b.end(NodeKind::CatchClause, 9);
                b.end(NodeKind::TryStatement, 9);
            }),
        );
        let lints = f.run(&one_rule("no_leading_underscores_for_local_identifiers"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, 5));
    }

    /// Positional formals report (`test_parameter`).
    #[test]
    fn no_leading_underscores_reports_a_positional_parameter() {
        let source = "f(_x) {}";
        // f0 (1 _x2 )3 {4 }5
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, NONE);
                b.leaf(NodeKind::NoType, 0);
                identifier(b, 0, 1);
                b.begin(NodeKind::FormalParameters, 1);
                b.begin(NodeKind::FormalParameter, 2);
                b.patch(0, NONE);
                b.patch(1, 2);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                b.end(NodeKind::FormalParameter, 3);
                b.end(NodeKind::FormalParameters, 4);
                b.begin(NodeKind::Block, 4);
                b.end(NodeKind::Block, 6);
                b.end(NodeKind::TopLevelMethod, 6);
            }),
        );
        let lints = f.run(&one_rule("no_leading_underscores_for_local_identifiers"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, 2));
    }

    /// Named parameters are API surface — skipped (:101-112).
    #[test]
    fn no_leading_underscores_named_section_passes() {
        let source = "f({_x}) {}";
        // f0 (1 {2 _x3 }4 )5 {6 }7
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, NONE);
                b.leaf(NodeKind::NoType, 0);
                identifier(b, 0, 1);
                b.begin(NodeKind::FormalParameters, 1);
                b.begin(NodeKind::OptionalFormalParameters, 2);
                b.begin(NodeKind::FormalParameter, 3);
                b.patch(0, NONE);
                b.patch(1, 3);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                b.end(NodeKind::FormalParameter, 4);
                b.end(NodeKind::OptionalFormalParameters, 5);
                b.end(NodeKind::FormalParameters, 6);
                b.begin(NodeKind::Block, 6);
                b.end(NodeKind::Block, 8);
                b.end(NodeKind::TopLevelMethod, 8);
            }),
        );
        assert!(
            f.run(&one_rule("no_leading_underscores_for_local_identifiers"))
                .is_empty()
        );
    }

    /// Positional-optional (`[...]`) sections keep the check.
    #[test]
    fn no_leading_underscores_optional_positional_reports() {
        let source = "f([_x]) {}";
        // f0 (1 [2 _x3 ]4 )5 {6 }7
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, NONE);
                b.leaf(NodeKind::NoType, 0);
                identifier(b, 0, 1);
                b.begin(NodeKind::FormalParameters, 1);
                b.begin(NodeKind::OptionalFormalParameters, 2);
                b.begin(NodeKind::FormalParameter, 3);
                b.patch(0, NONE);
                b.patch(1, 3);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                b.end(NodeKind::FormalParameter, 4);
                b.end(NodeKind::OptionalFormalParameters, 5);
                b.end(NodeKind::FormalParameters, 6);
                b.begin(NodeKind::Block, 6);
                b.end(NodeKind::Block, 8);
                b.end(NodeKind::TopLevelMethod, 8);
            }),
        );
        let lints = f.run(&one_rule("no_leading_underscores_for_local_identifiers"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, 3));
    }

    /// `this.x` field formals are members, not locals (:99-104).
    #[test]
    fn no_leading_underscores_field_formal_passes() {
        let source = "f(this._x) {}";
        // f0 (1 this2 .3 _x4 )5 {6 }7
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, NONE);
                b.leaf(NodeKind::NoType, 0);
                identifier(b, 0, 1);
                b.begin(NodeKind::FormalParameters, 1);
                b.begin(NodeKind::FormalParameter, 2);
                b.patch(0, 2); // the `this` receiver token
                b.patch(1, 4);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                b.end(NodeKind::FormalParameter, 6);
                b.end(NodeKind::FormalParameters, 6);
                b.begin(NodeKind::Block, 6);
                b.end(NodeKind::Block, 8);
                b.end(NodeKind::TopLevelMethod, 8);
            }),
        );
        assert!(
            f.run(&one_rule("no_leading_underscores_for_local_identifiers"))
                .is_empty()
        );
    }

    /// Field declarations are not local identifiers.
    #[test]
    fn no_leading_underscores_field_declaration_passes() {
        let source = "class A { var _x; }";
        // class0 A1 {2 var3 _x4 ;5 }6
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ClassDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                identifier(b, 1, 2);
                b.begin(NodeKind::DeclarationBody, 2);
                b.begin(NodeKind::Fields, 2);
                b.patch(0, dartq_parser::MemberKind::NonStaticField as u32);
                b.begin(NodeKind::VariablesDeclaration, 3);
                b.patch(0, 3);
                b.begin(NodeKind::InitializedIdentifier, 4);
                b.end(NodeKind::InitializedIdentifier, 5);
                b.end(NodeKind::VariablesDeclaration, 6);
                b.end(NodeKind::Fields, 6);
                b.end(NodeKind::DeclarationBody, 7);
                b.end(NodeKind::ClassDeclaration, 7);
            }),
        );
        assert!(
            f.run(&one_rule("no_leading_underscores_for_local_identifiers"))
                .is_empty()
        );
    }

    // —— non_constant_identifier_names ———————————————————————————————————

    use crate::testsupport::tok as tok_at;

    /// `test/rules/non_constant_identifier_names_test.dart` `test_variable`.
    #[test]
    fn non_constant_reports_a_bad_local() {
        let source = "var Xo = 1;";
        // var0 Xo1 =2 13 ;4
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::Block, 0);
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, tok_at(source, t, "var", 0));
                b.begin(NodeKind::InitializedIdentifier, 1);
                b.begin(NodeKind::VariableInitializer, 2);
                b.leaf(NodeKind::LiteralInt, 3);
                b.end(NodeKind::VariableInitializer, 4);
                b.end(NodeKind::InitializedIdentifier, 4);
                b.end(NodeKind::VariablesDeclaration, 5);
                b.end(NodeKind::Block, 5);
            }),
        );
        let lints = f.run(&one_rule("non_constant_identifier_names"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "The variable name 'Xo' isn't a lowerCamelCase identifier."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 1));
    }

    /// A `const` declaration belongs to `constant_identifier_names` (:159-164).
    #[test]
    fn non_constant_const_declaration_passes() {
        let source = "const Xo = 1;";
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, tok_at(source, t, "const", 0));
                b.begin(NodeKind::InitializedIdentifier, 1);
                b.end(NodeKind::InitializedIdentifier, 2);
                b.end(NodeKind::VariablesDeclaration, 3);
            }),
        );
        assert!(f.run(&one_rule("non_constant_identifier_names")).is_empty());
    }

    /// `___` parameters pass (underscores-ok, :97-107).
    #[test]
    fn non_constant_underscore_parameter_passes() {
        let source = "f(___) {}";
        // f0 (1 ___2 )3 {4 }5
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, NONE);
                b.leaf(NodeKind::NoType, 0);
                identifier(b, 0, 1);
                b.begin(NodeKind::FormalParameters, 1);
                b.begin(NodeKind::FormalParameter, 2);
                b.patch(0, NONE);
                b.patch(1, 2);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                b.end(NodeKind::FormalParameter, 4);
                b.end(NodeKind::FormalParameters, 4);
                b.begin(NodeKind::Block, 4);
                b.end(NodeKind::Block, 6);
                b.end(NodeKind::TopLevelMethod, 6);
            }),
        );
        assert!(f.run(&one_rule("non_constant_identifier_names")).is_empty());
    }

    /// A bad parameter name reports.
    #[test]
    fn non_constant_reports_a_parameter() {
        let source = "f(Xo) {}";
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, NONE);
                b.leaf(NodeKind::NoType, 0);
                identifier(b, 0, 1);
                b.begin(NodeKind::FormalParameters, 1);
                b.begin(NodeKind::FormalParameter, 2);
                b.patch(0, NONE);
                b.patch(1, 2);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                b.end(NodeKind::FormalParameter, 4);
                b.end(NodeKind::FormalParameters, 4);
                b.begin(NodeKind::Block, 4);
                b.end(NodeKind::Block, 6);
                b.end(NodeKind::TopLevelMethod, 6);
            }),
        );
        let lints = f.run(&one_rule("non_constant_identifier_names"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "The variable name 'Xo' isn't a lowerCamelCase identifier."
        );
    }

    /// `this.x` field formals are exempt (:103-105).
    #[test]
    fn non_constant_field_formal_passes() {
        let source = "f(this.Xo) {}";
        // f0 (1 this2 .3 Xo4 )5 {6 }7
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, NONE);
                b.leaf(NodeKind::NoType, 0);
                identifier(b, 0, 1);
                b.begin(NodeKind::FormalParameters, 1);
                b.begin(NodeKind::FormalParameter, 2);
                b.patch(0, 2);
                b.patch(1, 4);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                b.end(NodeKind::FormalParameter, 6);
                b.end(NodeKind::FormalParameters, 6);
                b.begin(NodeKind::Block, 6);
                b.end(NodeKind::Block, 8);
                b.end(NodeKind::TopLevelMethod, 8);
            }),
        );
        assert!(f.run(&one_rule("non_constant_identifier_names")).is_empty());
    }

    /// Operators keep their punctuation names (:135-138).
    #[test]
    fn non_constant_operator_passes() {
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
                b.patch(0, dartq_parser::MemberKind::NonStaticMethod as u32);
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
        assert!(f.run(&one_rule("non_constant_identifier_names")).is_empty());
    }

    // —— file_names ——————————————————————————————————————————————————————

    #[test]
    fn file_names_reports_a_mixed_case_name() {
        let mut f = Fixture::build("var x;", unit_of(|_b, _t| {}));
        f.path = std::path::PathBuf::from("MyFile.dart");
        let lints = f.run(&one_rule("file_names"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "The file name 'MyFile.dart' isn't a lower_case_with_underscores identifier."
        );
        assert_eq!(lints[0].range, 0..0);
    }

    #[test]
    fn file_names_lowercase_passes() {
        let mut f = Fixture::build("var x;", unit_of(|_b, _t| {}));
        f.path = std::path::PathBuf::from("my_file.dart");
        assert!(f.run(&one_rule("file_names")).is_empty());
    }

    /// A prefixed suffix (`.g.dart`) passes (ascii_utils.dart:28-33).
    #[test]
    fn file_names_generated_suffix_passes() {
        let mut f = Fixture::build("var x;", unit_of(|_b, _t| {}));
        f.path = std::path::PathBuf::from("my_file.g.dart");
        assert!(f.run(&one_rule("file_names")).is_empty());
    }

    /// Short and non-`.dart` names pass (ascii_utils.dart:21-23).
    #[test]
    fn file_names_non_dart_passes() {
        let mut f = Fixture::build("var x;", unit_of(|_b, _t| {}));
        f.path = std::path::PathBuf::from("README.md");
        assert!(f.run(&one_rule("file_names")).is_empty());
    }

    /// A leading digit fails (ascii_utils.dart:38-41).
    #[test]
    fn file_names_leading_digit_reports() {
        let mut f = Fixture::build("var x;", unit_of(|_b, _t| {}));
        f.path = std::path::PathBuf::from("1file.dart");
        let lints = f.run(&one_rule("file_names"));
        assert_eq!(lints.len(), 1);
    }

    /// Digits after position 0 pass.
    #[test]
    fn file_names_digit_after_head_passes() {
        let mut f = Fixture::build("var x;", unit_of(|_b, _t| {}));
        f.path = std::path::PathBuf::from("file1.dart");
        assert!(f.run(&one_rule("file_names")).is_empty());
    }

    // —— no_wildcard_variable_uses —————————————————————————————————————————

    /// `test/rules/no_wildcard_variable_uses_test.dart` `test_localVar` —
    /// with the wildcard feature off, a use of a `_` parameter reports.
    #[test]
    fn wildcard_use_reports_with_feature_off() {
        let source = "f(int _) { _; }";
        // f0 (1 int2 _3 )4 {5 _6 ;7 }8
        let mut f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::LocalFunctionDeclaration, 0);
                b.begin(NodeKind::FormalParameters, 1);
                b.begin(NodeKind::FormalParameter, 1);
                b.patch(0, NONE);
                b.patch(1, 3);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                b.begin(NodeKind::Identifier, 3);
                b.patch(
                    0,
                    dartq_parser::IdentifierContext::FormalParameterDeclaration as u32,
                );
                b.end(NodeKind::Identifier, 4);
                b.end(NodeKind::FormalParameter, 4);
                b.end(NodeKind::FormalParameters, 4);
                b.begin(NodeKind::Block, 5);
                b.begin(NodeKind::ExpressionStatement, 6);
                identifier(b, 6, 7);
                b.end(NodeKind::ExpressionStatement, 8);
                b.end(NodeKind::Block, 8);
                b.end(NodeKind::LocalFunctionDeclaration, 9);
            }),
        );
        f.features.wildcard_variables = false;
        let lints = f.run(&one_rule("no_wildcard_variable_uses"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "The referenced identifier is a wildcard.");
        assert_eq!(lints[0].range.start, utf16_at(&f, 6));
    }

    /// With the feature on (the shipped default) `_` never binds and the
    /// rule stays silent; a bare `_` with no wildcard declaration is also
    /// silent (the field/getter faces of the official `test_field`).
    #[test]
    fn wildcard_use_passes_with_feature_on_or_without_binding() {
        let source = "f(int _) { _; }";
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::LocalFunctionDeclaration, 0);
                b.begin(NodeKind::FormalParameters, 1);
                b.begin(NodeKind::FormalParameter, 1);
                b.patch(0, NONE);
                b.patch(1, 3);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                b.begin(NodeKind::Identifier, 3);
                b.patch(
                    0,
                    dartq_parser::IdentifierContext::FormalParameterDeclaration as u32,
                );
                b.end(NodeKind::Identifier, 4);
                b.end(NodeKind::FormalParameter, 4);
                b.end(NodeKind::FormalParameters, 4);
                b.begin(NodeKind::Block, 5);
                b.begin(NodeKind::ExpressionStatement, 6);
                identifier(b, 6, 7);
                b.end(NodeKind::ExpressionStatement, 8);
                b.end(NodeKind::Block, 8);
                b.end(NodeKind::LocalFunctionDeclaration, 9);
            }),
        );
        // The shipped default: wildcards on, rule inert.
        assert!(f.run(&one_rule("no_wildcard_variable_uses")).is_empty());

        let unbound = "_;";
        let mut f = Fixture::build(
            unbound,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                identifier(b, 0, 1);
                b.end(NodeKind::ExpressionStatement, 2);
            }),
        );
        f.features.wildcard_variables = false;
        assert!(f.run(&one_rule("no_wildcard_variable_uses")).is_empty());
    }
}
