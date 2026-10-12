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
    use crate::rules::NONE_TOKEN;

    /// Official: `pkg/linter/lib/src/rules/non_constant_identifier_names.dart:36-169`
    /// — every non-constant identifier (variables, functions, methods,
    /// parameters, catch parameters, constructor names, pattern variables,
    /// record field names) must be lowerCamelCase. Positions exempting
    /// just-underscores (`underscoresOk`): catch parameters, constructor
    /// names, all parameters (:57-66, 70-72, 97-107). Message:
    /// `diagnostic.g.dart` `nonConstantIdentifierNames`.
    ///
    /// Pattern-field variables (:124-131) report through the pattern
    /// dispatch alone — the official double-visits the same name
    /// (`visitPatternField` and `visitDeclaredVariablePattern` both check
    /// it), one report per name is the same surface. The primary-constructor
    /// name positions
    /// (:87-91, :123-125): an extension-type representation-constructor
    /// name checks without the underscore exemption, a class primary
    /// constructor name with it.
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
            NodeKind::PrimaryConstructor,
            NodeKind::TopLevelMethod,
            NodeKind::LocalFunctionDeclaration,
            NodeKind::Method,
            NodeKind::RecordLiteral,
            NodeKind::RecordType,
        ],
        check,
    };

    #[expect(
        clippy::too_many_lines,
        reason = "one dispatch over all checked declaration kinds; splitting would scatter the \
                  shared collect-then-check pattern"
    )]
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
                        // A recovery-inserted name is an empty-lexeme token;
                        // a real declared name always carries text.
                        if name != NONE_TOKEN && !c.lexeme(name).is_empty() {
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
            NodeKind::TopLevelMethod => {
                // The name rides the top-level-function context word (the
                // production slice keeps a typed return's name identifier
                // ahead of it).
                let ctx = dartq_parser::IdentifierContext::TopLevelFunctionDeclaration as u32;
                let name = c.direct_children(node).iter().copied().find(|&ch| {
                    c.kind(ch) == NodeKind::Identifier && c.payload(ch).first() == Some(&ctx)
                });
                if let Some(name) = name {
                    check_plain(c, c.head_token(name), false);
                }
            }
            NodeKind::LocalFunctionDeclaration => {
                // The production shape carries the name in a `FunctionName`
                // node owning no children.
                let name = c
                    .direct_children(node)
                    .iter()
                    .copied()
                    .find(|&ch| c.kind(ch) == NodeKind::FunctionName);
                if let Some(name) = name {
                    check_plain(c, c.head_token(name), false);
                }
            }
            NodeKind::Method => {
                // Operators keep their punctuation names (:135-138): only
                // method-context identifier names are checked.
                let ctx = dartq_parser::IdentifierContext::MethodDeclaration as u32;
                let name = c.direct_children(node).iter().copied().find(|&ch| {
                    c.kind(ch) == NodeKind::Identifier && c.payload(ch).first() == Some(&ctx)
                });
                if let Some(name) = name {
                    check_plain(c, c.head_token(name), false);
                }
            }
            NodeKind::PrimaryConstructor => {
                // The representation-constructor name (`:87-91` extension
                // types — no underscore exemption; `:123-125` class primary
                // constructors — just-underscores exempt).
                let ctx = dartq_parser::IdentifierContext::PrimaryConstructorDeclaration as u32;
                let name = c.direct_children(node).iter().copied().find(|&ch| {
                    c.kind(ch) == NodeKind::Identifier && c.payload(ch).first() == Some(&ctx)
                });
                if let Some(name) = name {
                    let underscores_ok = c
                        .ancestor(1)
                        .is_none_or(|owner| c.kind(owner) != NodeKind::ExtensionTypeDeclaration);
                    check_plain(c, c.head_token(name), underscores_ok);
                }
            }
            NodeKind::RecordLiteral => {
                // Collect first: the field scan re-enters `direct_children`,
                // and the scratch RefCell forbids a live outer guard.
                let mut hits: Vec<u32> = Vec::new();
                for field in c.collect_direct(node) {
                    if c.kind(field) != NodeKind::NamedRecordField {
                        continue;
                    }
                    // The childless marker anchors at the VALUE head (the
                    // event fires after the value parses, bridge
                    // `handle_named_record_field`); the colon payload and
                    // the name sit immediately before it.
                    let Some(&colon) = c.payload(field).first() else {
                        continue;
                    };
                    let Some(name) = colon.checked_sub(1) else {
                        continue;
                    };
                    let Some(token) = c.unit.tokens.get(name as usize) else {
                        continue;
                    };
                    if token.kind == dartq_scanner::TokenKind::Identifier
                        && !c.token_is_synthetic(name)
                    {
                        hits.push(name);
                    }
                }
                for name in hits {
                    check_plain(c, name, false);
                }
            }
            NodeKind::RecordType => {
                // Collect first: `collect_entry_name` re-enters
                // `direct_children`, and the scratch RefCell forbids a live
                // outer guard.
                let mut hits: Vec<u32> = Vec::new();
                for entry in c.collect_direct(node) {
                    match c.kind(entry) {
                        NodeKind::RecordTypeEntry => collect_entry_name(c, entry, &mut hits),
                        NodeKind::RecordTypeNamedFields => {
                            for sub in c.collect_direct(entry) {
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
        // The field name is the entry's RecordFieldDeclaration-context
        // identifier; the type's identifiers carry type-reference contexts
        // and must not be scanned.
        let ctx = dartq_parser::IdentifierContext::RecordFieldDeclaration as u32;
        if let Some(name) =
            c.direct_children(entry).iter().copied().find(|&ch| {
                c.kind(ch) == NodeKind::Identifier && c.payload(ch).first() == Some(&ctx)
            })
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
    #[expect(
        clippy::case_sensitive_file_extension_comparisons,
        reason = "the official `isValidDartFileName` check is case-sensitive; matching \
                  case-insensitively would break oracle parity"
    )]
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

pub(crate) mod avoid_types_as_parameter_names {
    use super::{Check, Group, NodeKind, RuleSpec, utf16_start_of};

    /// The dart:core type names every scope resolves to a class (the bare
    /// lexical face of the official's element test — `ClassElement`
    /// membership). The official additionally matches user-declared types,
    /// typedefs, extension types, and in-scope type parameters through
    /// resolution (`resolveNameInScope`, avoid_types_as_parameter_names.dart:
    /// 118-131); those are out of the syntax face and under-report.
    /// `dynamic` is excluded faithfully: the dynamic element is none of the
    /// official's accepted element kinds.
    const BUILTIN_TYPES: &[&str] = &[
        "int", "double", "num", "bool", "String", "List", "Map", "Set", "Iterable", "Object",
        "Symbol", "Type", "Function", "Never", "Record", "Enum",
    ];

    const FORMAL_PROBLEM: &str = "The parameter name '{0}' matches a visible type name.";
    const FORMAL_CORRECTION: &str = "Try adding a name for the parameter or changing the parameter name to not match an existing type.";
    const TYPE_PROBLEM: &str = "The type parameter name '{0}' matches a visible type name.";
    const TYPE_CORRECTION: &str =
        "Try changing the type parameter name to not match an existing type.";

    /// Official: `pkg/linter/lib/src/rules/avoid_types_as_parameter_names.dart`
    /// — a parameter named like a visible type reports at the name. Three
    /// registration faces (:42-47): formal parameter lists, catch clauses
    /// (the exception parameter only), and type parameter lists; the two
    /// diagnostic codes split formal vs type-parameter
    /// (`avoidTypesAsParameterNamesFormalParameter` /
    /// `...TypeParameter`, `diagnostic.g.dart`).
    ///
    /// Approximation (documented): the official resolves the name in scope
    /// and accepts class / extension-type / type-alias / non-wildcard
    /// type-parameter elements; the lexical face matches the dart:core
    /// built-in type list (`BUILTIN_TYPES`), so user-declared type names
    /// under-report. The formal faces keep the official's `hasImplicitType`
    /// gate (:63-72 — a written type annotation opts out) and its
    /// field-formal exclusion (:68 — `this.x`/`super.x` parameters never
    /// report); the catch face (:49-59) applies neither. The method-type-
    /// parameter shadow exemption (:87-98) is resolution-bound and vanishes:
    /// built-ins resolve to classes, never to a shadowed type parameter.
    /// Primary-constructor declaring formals (whose name creates a field,
    /// the official `test_constructor_primary_declaring` no-hit) over-report.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "avoid_types_as_parameter_names",
        group: Group::Core,
        problem: FORMAL_PROBLEM,
        correction: FORMAL_CORRECTION,
        kinds: &[NodeKind::FormalParameter, NodeKind::TypeVariable],
        check,
    };

    fn check(c: &mut Check<'_>) {
        match c.kind(c.node()) {
            NodeKind::FormalParameter => check_parameter(c),
            NodeKind::TypeVariable => check_type_variable(c),
            _ => {}
        }
    }

    fn check_parameter(c: &mut Check<'_>) {
        let node = c.node();
        let name_token = c.payload(node).get(1).copied();
        let Some(name_token) = name_token else {
            return;
        };
        let name = c.lexeme(name_token);
        if !BUILTIN_TYPES.contains(&name) {
            return;
        }
        let catch_clause = c
            .path
            .iter()
            .rev()
            .skip(1)
            .take(2)
            .any(|&ancestor| c.kind(ancestor) == NodeKind::CatchClause);
        if catch_clause {
            // The catch face checks the exception parameter only (:49-59) —
            // the first formal under the clause (the stack-trace parameter
            // is never named-checked).
            let clause = c
                .path
                .iter()
                .rev()
                .copied()
                .find(|&ancestor| c.kind(ancestor) == NodeKind::CatchClause);
            let Some(clause) = clause else { return };
            let window = c.children(clause);
            let first =
                (window.start..window.end).find(|&id| c.kind(id) == NodeKind::FormalParameter);
            if first != Some(node) {
                return;
            }
        } else {
            // `hasImplicitType` (:63-72): a written type annotation (the
            // `Type`/`FunctionType`/`RecordType` child) opts out; `NoType`
            // marks the implicit face. Only `this.`-formals are excluded
            // (:68 — `FieldFormalParameterElement`); a `super.`-formal is a
            // `SuperFormalParameterElement` and keeps reporting (the
            // official `test_super`).
            let typed = c.collect_direct(node).into_iter().any(|kid| {
                matches!(
                    c.kind(kid),
                    NodeKind::Type | NodeKind::FunctionType | NodeKind::RecordType
                )
            });
            let field_formal = c
                .payload(node)
                .first()
                .copied()
                .filter(|&token| token != crate::rules::NONE_TOKEN)
                .is_some_and(|token| c.lexeme(token) == "this");
            if typed || field_formal {
                return;
            }
        }
        report_at_token(c, name_token, FORMAL_PROBLEM, FORMAL_CORRECTION);
    }

    fn check_type_variable(c: &mut Check<'_>) {
        let node = c.node();
        // The name is the variable's identifier — the first Identifier-kind
        // token of the span (a variance prefix would precede it).
        let range = c.unit.ast.token_range(node);
        let Some(name_token) = (range.start..range.end).find(|&t| {
            c.unit
                .tokens
                .get(t as usize)
                .is_some_and(|token| token.kind == dartq_scanner::TokenKind::Identifier)
        }) else {
            return;
        };
        if !BUILTIN_TYPES.contains(&c.lexeme(name_token)) {
            return;
        }
        report_at_token(c, name_token, TYPE_PROBLEM, TYPE_CORRECTION);
    }

    /// `reportAtToken(name)` with the variant's message texts.
    fn report_at_token(c: &mut Check<'_>, token: u32, problem: &str, correction: &str) {
        let name = c.lexeme(token);
        let start = utf16_start_of(c, token);
        let end = start + name.encode_utf16().count() as u32;
        c.report_texts(start..end, problem, correction, &[name]);
    }
}

pub(crate) mod use_super_parameters {
    use super::{Check, Group, NodeKind, RuleSpec};
    use crate::rules::NONE_TOKEN;

    /// The body-child kinds of a constructor declaration (collapse map
    /// §3.3/§3.9).
    const BODY_KINDS: [NodeKind; 8] = [
        NodeKind::BlockBody,
        NodeKind::Block,
        NodeKind::EmptyBody,
        NodeKind::ExpressionBody,
        NodeKind::NoBody,
        NodeKind::SkippedBody,
        NodeKind::NativeFunctionBody,
        NodeKind::InvalidFunctionBody,
    ];

    const SINGLE_PROBLEM: &str = "Parameter '{0}' could be a super parameter.";
    const SINGLE_CORRECTION: &str = "Trying converting '{0}' to a super parameter.";
    const MULTIPLE_PROBLEM: &str = "Parameters '{0}' could be super parameters.";
    const MULTIPLE_CORRECTION: &str = "Trying converting '{0}' to super parameters.";

    /// Official: `pkg/linter/lib/src/rules/use_super_parameters.dart:64-107`
    /// — the last super-constructor invocation of a constructor's
    /// initializer list decides: positional bare-identifier arguments that
    /// name constructor parameters — unreferenced in the body, in source
    /// order, never repeated — could be super parameters, reported on the
    /// constructor's name range (`ConstructorDeclaration.errorRange`, the
    /// class-name-plus-name span).
    ///
    /// Approximation (documented): the named-parameter conversion face
    /// (`_checkNamedParameter`, :232-271) needs the super constructor's own
    /// parameter list (element resolution) and the type-assignability check
    /// (`typeSystem.isAssignableTo`, :260-266) — both out of the syntax
    /// face, so named arguments never contribute (under-report). Body
    /// references are name-level: every bare `Identifier` of the parameter's
    /// name counts except property positions (`obj.a`, prefixed names) and
    /// named-argument labels — a shadowing local of the same name
    /// over-counts (conservative, under-reports the conversion). A same-
    /// named field beside the parameter resolves to the parameter in the
    /// official too, so the name match is exact there. The `super_parameters`
    /// feature gate (shipped, :43) is always on.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "use_super_parameters",
        group: Group::Recommended,
        problem: SINGLE_PROBLEM,
        correction: SINGLE_CORRECTION,
        kinds: &[NodeKind::Constructor],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let kids = c.collect_direct(node);
        let Some(parameters) = kids
            .iter()
            .copied()
            .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
        else {
            return;
        };
        let initializers = kids
            .iter()
            .copied()
            .find(|&kid| c.kind(kid) == NodeKind::Initializers);
        let body = kids
            .iter()
            .copied()
            .filter(|&kid| BODY_KINDS.contains(&c.kind(kid)))
            .next_back();
        // The last super invocation decides (:79-84 — `initializers.reversed`
        // first `SuperConstructorInvocation`).
        let Some(super_invocation) = initializers.and_then(|list| {
            c.collect_direct(list)
                .into_iter()
                .rev()
                .find(|&init| is_super_invocation(c, init))
        }) else {
            return;
        };
        let Some(arguments) = c
            .collect_direct(super_invocation)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::Arguments)
        else {
            return;
        };
        // The parameter table in source order: `(name, index, in a named
        // group)`. Any field formal bails the positional conversion entirely
        // (`parameter is FieldFormalParameter → return []`, :164); any super
        // formal is a bad state (`is SuperFormalParameter → return null`,
        // :166) — under the name-level face both are receiver formals, and
        // either kind present means the report never fires.
        let mut table: Vec<(&str, usize, bool)> = Vec::new();
        let mut index = 0usize;
        let mut receiver_formal = false;
        let mut collect = |param: u32, named_group: bool| {
            if c.payload(param).first() != Some(&NONE_TOKEN) {
                receiver_formal = true;
                return;
            }
            let Some(&name_token) = c.payload(param).get(1) else {
                return;
            };
            table.push((c.lexeme(name_token), index, named_group));
            index += 1;
        };
        for kid in c.collect_direct(parameters) {
            match c.kind(kid) {
                NodeKind::FormalParameter => collect(kid, false),
                NodeKind::OptionalFormalParameters => {
                    let named_group = c.lexeme(c.head_token(kid)) == "{";
                    for param in c.collect_direct(kid) {
                        collect(param, named_group);
                    }
                }
                _ => {}
            }
        }
        if receiver_formal {
            return;
        }
        // Positional bare-identifier super arguments only (:150-153 — other
        // expression shapes leave no convertible positional parameters).
        let mut converted: Vec<&str> = Vec::new();
        let mut matched_index = 0usize;
        let mut seen: Vec<&str> = Vec::new();
        for &arg in c.collect_direct(arguments).iter() {
            if c.kind(arg) == NodeKind::NamedArgument {
                continue;
            }
            if c.kind(arg) != NodeKind::Identifier {
                return;
            }
            let name = c.lexeme(c.head_token(arg));
            // Used more than once (:159-161).
            if seen.contains(&name) {
                return;
            }
            seen.push(name);
            // An argument that names no parameter is not a formal element —
            // a bad state (:155-156).
            let Some((param_index, named_group)) = table
                .iter()
                .find(|entry| entry.0 == name)
                .map(|e| (e.1, e.2))
            else {
                return;
            };
            if named_group {
                // `paramPassedToSuper.isNamed → return null` (:157-158).
                return;
            }
            if body.is_some_and(|body| is_referenced(c, body, name)) {
                // `referencedParameters.contains → return []` (:169).
                return;
            }
            // Out-of-order parameters leave no convertible list (:178-180).
            if param_index < matched_index {
                return;
            }
            matched_index = param_index;
            converted.push(name);
        }
        if converted.is_empty() {
            return;
        }
        // `ConstructorDeclaration.errorRange` (ast.dart:10801-10804): the
        // span from the return type (the first `Identifier` child) through
        // the constructor name (the second, when written).
        let names: Vec<u32> = kids
            .iter()
            .copied()
            .filter(|&kid| c.kind(kid) == NodeKind::Identifier)
            .collect();
        let Some(&return_type) = names.first() else {
            return;
        };
        let end_token = c
            .unit
            .ast
            .token_range(names.get(1).copied().unwrap_or(return_type))
            .end
            - 1;
        let range = utf16_start(c, c.head_token(return_type))..utf16_end(c, end_token);
        // The official fills `{0}` with the bare parameter name (:93-98);
        // the multiple form fills it with the pre-quoted
        // `quotedAndCommaSeparatedWithAnd` list, so the rendered text
        // carries the quotes twice — mirrored verbatim.
        if converted.len() == 1 {
            let name = converted[0];
            c.report_texts(range, SINGLE_PROBLEM, SINGLE_CORRECTION, &[name]);
        } else {
            let joined = quoted_and_comma_separated_with_and(&converted);
            c.report_texts(range, MULTIPLE_PROBLEM, MULTIPLE_CORRECTION, &[&joined]);
        }
    }

    /// Whether the initializer is a super-constructor invocation: it wraps a
    /// `super` expression (the named `super.named(...)` form stays out of
    /// the name-level face — an identifier/dot chain beside `super` bails
    /// through the missing bare-`Arguments`-only shape).
    fn is_super_invocation(c: &Check<'_>, init: u32) -> bool {
        c.kind(init) == NodeKind::Initializer
            && c.collect_direct(init)
                .into_iter()
                .any(|kid| c.kind(kid) == NodeKind::SuperExpression)
    }

    /// The name-level body-reference scan: any bare `Identifier` of `name`
    /// in `body`'s subtree, except property positions (`obj.a`, prefixed
    /// names) and named-argument labels.
    fn is_referenced(c: &Check<'_>, body: u32, name: &str) -> bool {
        for kid in c.collect_direct(body) {
            if c.kind(kid) == NodeKind::Identifier
                && c.lexeme(c.head_token(kid)) == name
                && !is_property_position(c, kid, body)
            {
                return true;
            }
            if is_referenced(c, kid, name) {
                return true;
            }
        }
        false
    }

    /// Whether `node` sits in a non-reference identifier position under its
    /// parent: a property name after a dot, or a named-argument label.
    fn is_property_position(c: &Check<'_>, node: u32, root: u32) -> bool {
        if node == root {
            return false;
        }
        let Some(parent) = c.unit.ast.parent(node) else {
            return false;
        };
        match c.kind(parent) {
            NodeKind::DotAccess | NodeKind::Qualified => {
                c.collect_direct(parent).first() != Some(&node)
            }
            NodeKind::NamedArgument => c.collect_direct(parent).first() == Some(&node),
            _ => false,
        }
    }

    /// `IterableOfStringExtension.quotedAndCommaSeparatedWithAnd`
    /// (`pkg/analyzer/lib/src/utilities/extensions/string.dart:88-135`):
    /// `'a'` / `'a' and 'b'` / `'a', 'b', and 'c'`.
    fn quoted_and_comma_separated_with_and(names: &[&str]) -> String {
        match names.len() {
            0 => String::new(),
            1 => format!("'{}'", names[0]),
            2 => format!("'{}' and '{}'", names[0], names[1]),
            _ => {
                let mut joined = String::new();
                for (i, name) in names.iter().enumerate() {
                    if i > 0 {
                        joined.push_str(", ");
                    }
                    if i + 1 == names.len() {
                        joined.push_str("and ");
                    }
                    joined.push('\'');
                    joined.push_str(name);
                    joined.push('\'');
                }
                joined
            }
        }
    }

    fn utf16_start(c: &Check<'_>, token: u32) -> u32 {
        c.unit
            .tokens
            .get(token as usize)
            .expect("token outside the table")
            .utf16_offset
    }

    /// Exclusive UTF-16 end of `token`'s lexeme.
    fn utf16_end(c: &Check<'_>, token: u32) -> u32 {
        let start = utf16_start(c, token);
        start + c.lexeme(token).encode_utf16().count() as u32
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

    /// The context-name faces (production-bridge fixtures): a typed
    /// return's method / top-level function name still reports (the name
    /// identifier rides its context word past the type), a local function
    /// name reports through its `FunctionName` node, and a primary
    /// constructor name reports (:87-91 extension types, :123-125 class
    /// primary constructors).
    #[test]
    fn non_constant_context_name_positions() {
        let f = Fixture::parse("class C { String MY() => ''; }");
        assert_eq!(f.run(&one_rule("non_constant_identifier_names")).len(), 1);

        let f = Fixture::parse("String FY() => '';");
        assert_eq!(f.run(&one_rule("non_constant_identifier_names")).len(), 1);

        let f = Fixture::parse("void main() { void LY() {} }");
        assert_eq!(f.run(&one_rule("non_constant_identifier_names")).len(), 1);

        let f = Fixture::parse("class C.Foo(int x) {}");
        assert_eq!(f.run(&one_rule("non_constant_identifier_names")).len(), 1);

        let f = Fixture::parse("extension type E.Foo(int x) {}");
        assert_eq!(f.run(&one_rule("non_constant_identifier_names")).len(), 1);
    }

    /// Lower-camel primary constructor names pass on both owners (the
    /// camel gate; the class-side underscore exemption is unobservable —
    /// just-underscore names are valid lowerCamelCase either way).
    #[test]
    fn non_constant_primary_camel_passes() {
        let f = Fixture::parse("class C.foo(int x) {}");
        assert!(f.run(&one_rule("non_constant_identifier_names")).is_empty());

        let f = Fixture::parse("extension type E.foo(int x) {}");
        assert!(f.run(&one_rule("non_constant_identifier_names")).is_empty());

        let f = Fixture::parse("extension type E(int x) {}");
        assert!(f.run(&one_rule("non_constant_identifier_names")).is_empty());
    }

    // —— file_names ——————————————————————————————————————————————————————

    /// Regression pin: the record-literal and record-type field scans
    /// re-enter `direct_children` per field, so both run over an owned
    /// collect — these shapes panicked on a nested scratch borrow before
    /// the collect-first fix (M0 gate). Expectations follow the official
    /// gate: `isLowerCamelCase` passes a lone uppercase identifier
    /// (utils.dart:68-71, `id.length == 1 && isUpperCase(...)`), so the
    /// reporting face is a multi-character violation — the official
    /// `test_recordFields` pins `(x: 1)` clean against `([!XX!]: 1)`.
    #[test]
    fn non_constant_record_field_scans_survive_nested_children() {
        let f = Fixture::parse("void main() { const r = (n: 1, m: 2); }");
        assert!(f.run(&one_rule("non_constant_identifier_names")).is_empty());

        let f = Fixture::parse("void main() { const r = (XX: 1, YY: 2); }");
        let lints = f.run(&one_rule("non_constant_identifier_names"));
        assert_eq!(
            lints.len(),
            2,
            "both multi-char uppercase field names report"
        );
    }

    /// The record-type-annotation face (official `test_recordFields`, type
    /// side): record-type entry names report like record-literal names — a
    /// multi-char upper-camel name reports (the lone-uppercase pass,
    /// utils.dart:68-71, keeps single letters clean), lowercase passes. This
    /// face was untestable until the entry events carried their token
    /// bounds: the bridge anchored the first entry at the last sealed node,
    /// tripping the builder's span assert on every positional record type.
    #[test]
    fn non_constant_record_type_annotation_fields_report() {
        let f = Fixture::parse("void main() { (int NN, int MM) t = (1, 2); }");
        assert_eq!(
            f.run(&one_rule("non_constant_identifier_names")).len(),
            2,
            "both positional record-type field names report"
        );

        // The named shape exercises the recovery VariablesDeclaration (the
        // parser lane's record-type-entry scope): the synthetic empty name
        // must not report — see the empty-lexeme guard in the
        // VariablesDeclaration arm.
        let f = Fixture::parse("void main() { ({int NN}) t; }");
        assert_eq!(
            f.run(&one_rule("non_constant_identifier_names")).len(),
            1,
            "the named record-type field name reports"
        );

        let f = Fixture::parse("void main() { (int n, int m) t = (1, 2); }");
        assert!(f.run(&one_rule("non_constant_identifier_names")).is_empty());
    }

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

    // —— avoid_types_as_parameter_names / use_super_parameters ————————————
    // Fixtures over the collapse-map §3.3 contracts: the `FormalParameter`
    // payload words (`receiver`/`name`/`init_start`/`init_end`/`kind`), the
    // `Type`/`NoType` child, and the `Constructor` child order
    // (`Identifier`s, `FormalParameters`, `Initializers`, body).

    use dartq_ast::AstBuilder;
    use dartq_parser::FormalParameterKind;

    /// One `FormalParameter` in a `FormalParameters` wrapper: `type_at` is
    /// the written type token or `NONE` (implicit), `receiver_at` the
    /// `this.`/`super.` token or `NONE`.
    fn formal(b: &mut AstBuilder, type_at: u32, name_at: u32, receiver_at: u32, end_at: u32) {
        // The span head is the leftmost written piece: type, receiver, name.
        let head = if type_at != NONE {
            type_at
        } else if receiver_at != NONE {
            receiver_at
        } else {
            name_at
        };
        b.begin(NodeKind::FormalParameter, head);
        b.patch(0, receiver_at);
        b.patch(1, name_at);
        b.patch(2, NONE);
        b.patch(3, NONE);
        b.patch(4, FormalParameterKind::RequiredPositional as u32);
        if type_at == NONE {
            b.leaf(NodeKind::NoType, name_at);
        } else {
            b.leaf_span(NodeKind::Type, type_at, type_at + 1);
        }
        b.end(NodeKind::FormalParameter, end_at);
    }

    fn avoid_hits(f: &Fixture) -> Vec<crate::engine::Lint> {
        f.run(&one_rule("avoid_types_as_parameter_names"))
    }

    /// A built-in type name as an untyped function-typed parameter reports
    /// with the formal message (`test_functionTypedParameter_missingName`).
    #[test]
    fn types_as_params_report() {
        // v0 o1 i2 d3 f4 (5 v6 o7 i8 d9 g10 (11 int12 )13 )14
        let f = Fixture::build("void f(void g(int)) {}", |b, t| {
            b.begin(NodeKind::CompilationUnit, 0);
            b.begin(NodeKind::FormalParameters, 5);
            formal(b, NONE, 6, NONE, 7);
            b.end(NodeKind::FormalParameters, 8);
            b.end(NodeKind::CompilationUnit, t.len() as u32);
        });
        let lints = avoid_hits(&f);
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "The parameter name 'int' matches a visible type name."
        );
        assert_eq!(
            lints[0].correction,
            Some(
                "Try adding a name for the parameter or changing the parameter name to not match an existing type."
                    .to_owned()
            )
        );
        // The report sits on the name token `int` (bytes 14..17).
        assert_eq!(lints[0].range.start, 14);
        assert_eq!(lints[0].range.end, 17);
    }

    /// A typed parameter of the same name opts out
    /// (`test_functionTypedParameter_noShadowing` — `hasImplicitType`).
    #[test]
    fn typed_parameters_pass() {
        // The written type occupies the name slot's token in this minimal
        // shape; the `Type` child is what the read contract keys on.
        let f = Fixture::build("void f(void g(int)) {}", |b, t| {
            b.begin(NodeKind::CompilationUnit, 0);
            b.begin(NodeKind::FormalParameters, 5);
            formal(b, 6, 6, NONE, 7);
            b.end(NodeKind::FormalParameters, 8);
            b.end(NodeKind::CompilationUnit, t.len() as u32);
        });
        assert!(avoid_hits(&f).is_empty());
    }

    /// `this.`-formals are excluded (the `FieldFormalParameterElement` test,
    /// the official `test_fieldFormalParameter_missingType`).
    #[test]
    fn field_formals_pass() {
        // C0 (1 this2 .3 String4 )5
        let f = Fixture::build("C(this.String);", |b, t| {
            b.begin(NodeKind::CompilationUnit, 0);
            b.begin(NodeKind::FormalParameters, 1);
            formal(b, NONE, 4, 2, 6);
            b.end(NodeKind::FormalParameters, 6);
            b.end(NodeKind::CompilationUnit, t.len() as u32);
        });
        assert!(avoid_hits(&f).is_empty());
    }

    /// `super.`-formals keep reporting (a `SuperFormalParameterElement` is
    /// not a field formal — the official `test_super`).
    #[test]
    fn super_formals_report() {
        // B0 (1 super2 .3 String4 )5
        let f = Fixture::build("B(super.String);", |b, t| {
            b.begin(NodeKind::CompilationUnit, 0);
            b.begin(NodeKind::FormalParameters, 1);
            formal(b, NONE, 4, 2, 6);
            b.end(NodeKind::FormalParameters, 6);
            b.end(NodeKind::CompilationUnit, t.len() as u32);
        });
        let lints = avoid_hits(&f);
        assert_eq!(lints.len(), 1);
        assert!(lints[0].message.contains("'String'"));
    }

    /// The catch face checks the exception parameter only — the
    /// stack-trace parameter never reports.
    #[test]
    fn catch_exception_parameter_reports() {
        // try0 {1 }2 catch3 (4 int5 )6
        let f = Fixture::build("try {} catch (int) {}", |b, t| {
            b.begin(NodeKind::CompilationUnit, 0);
            b.begin(NodeKind::CatchClause, 3);
            b.patch(0, NONE);
            b.patch(1, NONE);
            formal(b, NONE, 5, NONE, 6);
            b.end(NodeKind::CatchClause, 7);
            b.end(NodeKind::CompilationUnit, t.len() as u32);
        });
        assert_eq!(avoid_hits(&f).len(), 1);
        // try0 {1 }2 catch3 (4 e5 ,6 int7 )8 — the second parameter.
        let stack_trace = Fixture::build("try {} catch (e, int) {}", |b, t| {
            b.begin(NodeKind::CompilationUnit, 0);
            b.begin(NodeKind::CatchClause, 3);
            b.patch(0, NONE);
            b.patch(1, NONE);
            formal(b, NONE, 5, NONE, 6);
            formal(b, NONE, 7, NONE, 8);
            b.end(NodeKind::CatchClause, 9);
            b.end(NodeKind::CompilationUnit, t.len() as u32);
        });
        assert!(avoid_hits(&stack_trace).is_empty());
    }

    /// The type-parameter face reports with its own message
    /// (`test_typeParameter_function`).
    #[test]
    fn type_parameters_report() {
        // v0 o1 i2 d3 f4 <5 int6 >7 — the name token sits at 3.
        let f = Fixture::build("void f<int>() {}", |b, t| {
            b.begin(NodeKind::CompilationUnit, 0);
            b.begin(NodeKind::TypeVariable, 3);
            b.patch(0, NONE);
            b.patch(1, NONE);
            b.end(NodeKind::TypeVariable, 4);
            b.end(NodeKind::CompilationUnit, t.len() as u32);
        });
        let lints = avoid_hits(&f);
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "The type parameter name 'int' matches a visible type name."
        );
        assert_eq!(
            lints[0].correction,
            Some("Try changing the type parameter name to not match an existing type.".to_owned())
        );
        assert_eq!(lints[0].range.start, 7);
        assert_eq!(lints[0].range.end, 10);
    }

    // —— use_super_parameters ————————————————————————————————————————————

    fn super_hits(f: &Fixture) -> Vec<crate::engine::Lint> {
        f.run(&one_rule("use_super_parameters"))
    }

    /// Class `A` with `A(int x);`, then the `B` constructor the rule reads.
    /// `constructor` builds `B`'s `Constructor` node (open at the caller).
    fn single_super_fixture(constructor: impl FnOnce(&mut AstBuilder)) -> Fixture {
        Fixture::build(
            "class A { A(int x); }\nclass B extends A {\n  B(int x) : super(x);\n}\n",
            |b, t| {
                b.begin(NodeKind::CompilationUnit, 0);
                // —— class A ——
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
                identifier(b, 3, 4);
                b.begin(NodeKind::FormalParameters, 4);
                formal(b, 5, 6, NONE, 7);
                b.end(NodeKind::FormalParameters, 7);
                b.leaf(NodeKind::EmptyBody, 8);
                b.end(NodeKind::Method, 9);
                b.end(NodeKind::DeclarationBody, 9);
                b.end(NodeKind::ClassDeclaration, 10);
                b.end(NodeKind::TopLevelDeclaration, 11);
                // —— class B ——
                b.begin(NodeKind::TopLevelDeclaration, 10);
                identifier(b, 11, 12);
                b.begin(NodeKind::ClassDeclaration, 10);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.begin(NodeKind::DeclarationBody, 14);
                b.begin(NodeKind::Constructor, 15);
                b.patch(0, dartq_parser::MemberKind::NonStaticMethod as u32);
                b.patch(1, NONE);
                b.patch(2, 16);
                b.patch(3, 20);
                constructor(b);
                b.end(NodeKind::Constructor, 26);
                b.end(NodeKind::DeclarationBody, 26);
                b.end(NodeKind::ClassDeclaration, 27);
                b.end(NodeKind::TopLevelDeclaration, 27);
                b.end(NodeKind::CompilationUnit, t.len() as u32);
            },
        )
    }

    /// `B(int x) : super(x);` — the default constructor body the fixture
    /// builder mounts (identifier, parameters, the super invocation, empty
    /// body).
    fn default_b_constructor(b: &mut AstBuilder) {
        identifier(b, 15, 16);
        b.begin(NodeKind::FormalParameters, 16);
        formal(b, 17, 18, NONE, 19);
        b.end(NodeKind::FormalParameters, 19);
        b.begin(NodeKind::Initializers, 20);
        b.begin(NodeKind::Initializer, 21);
        b.leaf(NodeKind::SuperExpression, 21);
        b.begin(NodeKind::Arguments, 22);
        identifier(b, 23, 24);
        b.end(NodeKind::Arguments, 25);
        b.end(NodeKind::Initializer, 25);
        b.end(NodeKind::Initializers, 25);
        b.leaf(NodeKind::EmptyBody, 25);
    }

    /// A single convertible positional parameter reports on the
    /// constructor's name range (`positionalParameters_single`).
    #[test]
    fn super_parameter_single_reports() {
        let f = single_super_fixture(default_b_constructor);
        let lints = super_hits(&f);
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Parameter 'x' could be a super parameter."
        );
        assert_eq!(
            lints[0].correction,
            Some("Trying converting 'x' to a super parameter.".to_owned())
        );
        // The error range is the constructor's `B` name token (bytes 44..45).
        assert_eq!(lints[0].range.start, 44);
        assert_eq!(lints[0].range.end, 45);
    }

    /// Two convertible parameters join with the official comma-and form.
    #[test]
    fn super_parameter_multiple_join() {
        let f = Fixture::build(
            "class A { A(int x, int y); }\nclass B extends A {\n  B(int x, int y) : super(x, y);\n}\n",
            |b, t| {
                b.begin(NodeKind::CompilationUnit, 0);
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
                identifier(b, 3, 4);
                b.begin(NodeKind::FormalParameters, 4);
                formal(b, 5, 6, NONE, 7);
                formal(b, 8, 9, NONE, 10);
                b.end(NodeKind::FormalParameters, 11);
                b.leaf(NodeKind::EmptyBody, 11);
                b.end(NodeKind::Method, 12);
                b.end(NodeKind::DeclarationBody, 12);
                b.end(NodeKind::ClassDeclaration, 13);
                b.end(NodeKind::TopLevelDeclaration, 13);
                b.begin(NodeKind::TopLevelDeclaration, 13);
                identifier(b, 14, 15);
                b.begin(NodeKind::ClassDeclaration, 13);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.begin(NodeKind::DeclarationBody, 17);
                b.begin(NodeKind::Constructor, 18);
                b.patch(0, dartq_parser::MemberKind::NonStaticMethod as u32);
                b.patch(1, NONE);
                b.patch(2, 19);
                b.patch(3, 26);
                identifier(b, 18, 19);
                b.begin(NodeKind::FormalParameters, 19);
                formal(b, 20, 21, NONE, 22);
                formal(b, 23, 24, NONE, 25);
                b.end(NodeKind::FormalParameters, 25);
                b.begin(NodeKind::Initializers, 26);
                b.begin(NodeKind::Initializer, 27);
                b.leaf(NodeKind::SuperExpression, 27);
                b.begin(NodeKind::Arguments, 28);
                identifier(b, 29, 30);
                identifier(b, 31, 32);
                b.end(NodeKind::Arguments, 33);
                b.end(NodeKind::Initializer, 33);
                b.end(NodeKind::Initializers, 33);
                b.leaf(NodeKind::EmptyBody, 33);
                b.end(NodeKind::Constructor, 34);
                b.end(NodeKind::DeclarationBody, 34);
                b.end(NodeKind::ClassDeclaration, 35);
                b.end(NodeKind::TopLevelDeclaration, 35);
                b.end(NodeKind::CompilationUnit, t.len() as u32);
            },
        );
        let lints = super_hits(&f);
        assert_eq!(lints.len(), 1);
        // The official argument fill puts the pre-quoted list into the
        // quoted `{0}` slot, so the rendered text doubles the quotes.
        assert_eq!(
            lints[0].message,
            "Parameters ''x' and 'y'' could be super parameters."
        );
    }

    /// A body reference blocks the conversion (`_referencedParameters`).
    #[test]
    fn referenced_parameter_passes() {
        let f = single_super_fixture(|b| {
            identifier(b, 15, 16);
            b.begin(NodeKind::FormalParameters, 16);
            formal(b, 17, 18, NONE, 19);
            b.end(NodeKind::FormalParameters, 19);
            b.begin(NodeKind::Initializers, 20);
            b.begin(NodeKind::Initializer, 21);
            b.leaf(NodeKind::SuperExpression, 21);
            b.begin(NodeKind::Arguments, 22);
            identifier(b, 23, 24);
            b.end(NodeKind::Arguments, 25);
            b.end(NodeKind::Initializer, 25);
            b.end(NodeKind::Initializers, 25);
            // The body references `x` (token 23): `{ x; }`.
            b.begin(NodeKind::BlockBody, 25);
            b.patch(0, NONE);
            b.patch(1, NONE);
            b.begin(NodeKind::Block, 25);
            b.begin(NodeKind::ExpressionStatement, 25);
            identifier(b, 23, 24);
            b.end(NodeKind::ExpressionStatement, 26);
            b.end(NodeKind::Block, 26);
            b.end(NodeKind::BlockBody, 26);
        });
        assert!(super_hits(&f).is_empty());
    }

    /// A `this.`-formal anywhere aborts the positional conversion
    /// (`parameter is FieldFormalParameter → return []`).
    #[test]
    fn field_formal_aborts() {
        let f = Fixture::build(
            "class A { A(int x); }\nclass B extends A {\n  B(int x, this.y) : super(x);\n}\n",
            |b, t| {
                b.begin(NodeKind::CompilationUnit, 0);
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
                identifier(b, 3, 4);
                b.begin(NodeKind::FormalParameters, 4);
                formal(b, 5, 6, NONE, 7);
                b.end(NodeKind::FormalParameters, 7);
                b.leaf(NodeKind::EmptyBody, 8);
                b.end(NodeKind::Method, 9);
                b.end(NodeKind::DeclarationBody, 9);
                b.end(NodeKind::ClassDeclaration, 10);
                b.end(NodeKind::TopLevelDeclaration, 11);
                b.begin(NodeKind::TopLevelDeclaration, 10);
                identifier(b, 11, 12);
                b.begin(NodeKind::ClassDeclaration, 10);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.begin(NodeKind::DeclarationBody, 14);
                b.begin(NodeKind::Constructor, 15);
                b.patch(0, dartq_parser::MemberKind::NonStaticMethod as u32);
                b.patch(1, NONE);
                b.patch(2, 16);
                b.patch(3, 24);
                identifier(b, 15, 16);
                b.begin(NodeKind::FormalParameters, 16);
                formal(b, 17, 18, NONE, 19);
                // `this.y` — the receiver token is `this`.
                b.begin(NodeKind::FormalParameter, 20);
                b.patch(0, 20);
                b.patch(1, 22);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, FormalParameterKind::RequiredPositional as u32);
                b.leaf(NodeKind::NoType, 22);
                b.end(NodeKind::FormalParameter, 23);
                b.end(NodeKind::FormalParameters, 23);
                b.begin(NodeKind::Initializers, 24);
                b.begin(NodeKind::Initializer, 25);
                b.leaf(NodeKind::SuperExpression, 25);
                b.begin(NodeKind::Arguments, 26);
                identifier(b, 27, 28);
                b.end(NodeKind::Arguments, 29);
                b.end(NodeKind::Initializer, 29);
                b.end(NodeKind::Initializers, 29);
                b.leaf(NodeKind::EmptyBody, 29);
                b.end(NodeKind::Constructor, 30);
                b.end(NodeKind::DeclarationBody, 30);
                b.end(NodeKind::ClassDeclaration, 31);
                b.end(NodeKind::TopLevelDeclaration, 31);
                b.end(NodeKind::CompilationUnit, t.len() as u32);
            },
        );
        assert!(super_hits(&f).is_empty());
    }

    /// A non-identifier positional argument leaves no convertible
    /// parameters.
    #[test]
    fn non_identifier_argument_passes() {
        let f = single_super_fixture(|b| {
            identifier(b, 15, 16);
            b.begin(NodeKind::FormalParameters, 16);
            formal(b, 17, 18, NONE, 19);
            b.end(NodeKind::FormalParameters, 19);
            b.begin(NodeKind::Initializers, 20);
            b.begin(NodeKind::Initializer, 21);
            b.leaf(NodeKind::SuperExpression, 21);
            b.begin(NodeKind::Arguments, 22);
            identifier(b, 23, 24);
            b.leaf(NodeKind::LiteralInt, 24);
            b.end(NodeKind::Arguments, 25);
            b.end(NodeKind::Initializer, 25);
            b.end(NodeKind::Initializers, 25);
            b.leaf(NodeKind::EmptyBody, 25);
        });
        assert!(super_hits(&f).is_empty());
    }

    /// An argument naming a `{...}`-group parameter is a bad state
    /// (`paramPassedToSuper.isNamed → return null`).
    #[test]
    fn named_group_parameter_passes() {
        // c0 l1 a2 s3 s4 ␣5 B6 {7 ␣8 B9 (10 {11 int12 x13 }14 )15 :16
        // super17 (18 x19 )20 ;21 ␣}22
        let f = Fixture::build("class B {\n  B({int x}) : super(x);\n}\n", |b, t| {
            b.begin(NodeKind::CompilationUnit, 0);
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
            b.patch(2, 4);
            b.patch(3, 10);
            identifier(b, 3, 4);
            b.begin(NodeKind::FormalParameters, 4);
            b.begin(NodeKind::OptionalFormalParameters, 5);
            formal(b, 6, 7, NONE, 8);
            b.end(NodeKind::OptionalFormalParameters, 8);
            b.end(NodeKind::FormalParameters, 9);
            b.begin(NodeKind::Initializers, 10);
            b.begin(NodeKind::Initializer, 11);
            b.leaf(NodeKind::SuperExpression, 11);
            b.begin(NodeKind::Arguments, 12);
            identifier(b, 13, 14);
            b.end(NodeKind::Arguments, 15);
            b.end(NodeKind::Initializer, 15);
            b.end(NodeKind::Initializers, 15);
            b.leaf(NodeKind::EmptyBody, 15);
            b.end(NodeKind::Constructor, 16);
            b.end(NodeKind::DeclarationBody, 16);
            b.end(NodeKind::ClassDeclaration, 17);
            b.end(NodeKind::TopLevelDeclaration, 17);
            b.end(NodeKind::CompilationUnit, t.len() as u32);
        });
        assert!(super_hits(&f).is_empty());
    }
}
