//! The naming-style family: `camel_case_types`, `camel_case_extensions`,
//! `constant_identifier_names`,
//! `no_leading_underscores_for_local_identifiers`,
//! `non_constant_identifier_names`, `file_names`, and
//! `matching_super_parameters`.

use dartq_ast::{NodeKind, PayloadField};

use crate::engine::Check;
use crate::registry::RuleSpec;
use crate::rules::{has_leading_underscore, is_field_name_shortcut, is_just_underscores, is_lower_camel_case, is_upper_camel_case, utf16_token_span};

/// The domain's incoming registrations: a new standalone rule appends its
/// `SPEC` reference here (name order) and joins the registry through the
/// aggregated tail — adding a rule touches only this file. Batch close
/// promotes the block into the registry's frozen skeleton together with
/// the pin-test wave list (`tests/registry_pin.rs`).
pub(crate) static INCOMING_RULES: &[&RuleSpec] = &[];

/// UTF-16 offset of a token's start, `None` on the shared token-helper miss.
fn utf16_start_of(c: &Check<'_>, token: u32) -> Option<u32> {
    c.utf16_of_token(token)
}

/// Reports `token`/`lexeme` when the lowerCamelCase style check fails.
/// The official `reportAtToken(id)` spans the name token
/// (constant_identifier_names.dart:40-43) — the markup of every upstream
/// case marks the full identifier.
fn check_lower_camel(c: &mut Check<'_>, token: u32, lexeme: &str) {
    if is_lower_camel_case(lexeme) {
        return;
    }
    let Some(start) = utf16_start_of(c, token) else {
        return;
    };
    let end = c.utf16_end_of_token(token).unwrap_or(start);
    c.report(start..end, &[lexeme]);
}

pub(crate) mod camel_case_types {
    use super::{Check, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/camel_case_types.dart:33-88` —
    /// every type-introducing declaration's name must be `UpperCamelCase`
    /// (`isCamelCase`, `pkg/linter/lib/src/utils.dart:22`). Message:
    /// `diagnostic.g.dart` `camelCaseTypes`.
    ///
    /// Names of top-level declarations live in the enclosing
    /// [`NodeKind::TopLevelDeclaration`] wrapper (`TopLevelDeclarationEnd`
    /// layout, the name-Identifier container). The official
    /// augmentation skips are wave-1 inert: `augment` is gated off at the
    /// scanner, so no augmentation declaration reaches the AST.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "camel_case_types",
    &[
            NodeKind::ClassDeclaration,
            NodeKind::MixinDeclaration,
            NodeKind::EnumDeclaration,
            NodeKind::ExtensionTypeDeclaration,
            NodeKind::NamedMixinApplication,
            NodeKind::Typedef,
        ],
    check,
);

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
        let Some(start) = super::utf16_start_of(c, token) else {
            return;
        };
        c.report(start..start, &[lexeme]);
    }
}

pub(crate) mod camel_case_extensions {
    use super::{PayloadField, Check, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/camel_case_extensions.dart:31-42`
    /// — a named extension's name must be `UpperCamelCase`. The name is
    /// payload word 1 of [`NodeKind::ExtensionDeclaration`]
    /// (`extension: T, name: O, on: O`). Message:
    /// `diagnostic.g.dart` `camelCaseExtensions`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "camel_case_extensions",
    &[NodeKind::ExtensionDeclaration],
    check,
);

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // Anonymous extensions (`extension on X`) have no name: skip.
        let Some(name) = c.field(node, PayloadField::Name) else {
            return;
        };
        if name == crate::rules::NONE_TOKEN {
            return;
        }
        let lexeme = c.lexeme(name);
        if super::is_upper_camel_case(lexeme) {
            return;
        }
        let Some(start) = super::utf16_start_of(c, name) else {
            return;
        };
        c.report(start..start, &[lexeme]);
    }
}

pub(crate) mod constant_identifier_names {
    use super::{PayloadField, Check, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/constant_identifier_names.dart:36-73`
    /// — `const` variables (fields, top-level, locals), enum constants, and
    /// pattern-bound variables must be lowerCamelCase (`isLowerCamelCase`,
    /// `pkg/linter/lib/src/utils.dart:31`). Object-pattern field-name
    /// shortcuts (`Foo(:name)`) are exempt. Message: `diagnostic.g.dart`
    /// `constantIdentifierNames`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "constant_identifier_names",
    &[
            NodeKind::VariablesDeclaration,
            NodeKind::EnumElement,
            NodeKind::DeclaredVariablePattern,
        ],
    check,
);

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        match c.kind(node) {
            NodeKind::VariablesDeclaration => {
                // `const`-ness: real parses carry only `late` in payload
                // word 0 (the collapse drops the `var`/`final`/`const`
                // keyword), so the keyword is read through the shared
                // declaration-keyword recovery — the raw word-0 read stayed
                // silent on every production shape (fields anchor the row at
                // the keyword, top-level and local declarations away from
                // it).
                let Some(keyword) = crate::rules::variables_declaration_keyword(c, node) else {
                    return;
                };
                if c.lexeme(keyword) != "const" {
                    return;
                }
                // Gather under the scratch borrow, then report (the report
                // path needs `&mut Check`). Allocates only on hits.
                let mut hits: Vec<u32> = Vec::new();
                for var in c.direct_children(node).iter().copied() {
                    if c.kind(var) == NodeKind::InitializedIdentifier {
                        // The variable's name is the `InitializedIdentifier`
                        // span head (width-0 kind: the name word was
                        // eliminated as derivable).
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
                // The constant name rides the element's `Name` word — the
                // production row is a childless handle over the element
                // extent (the name identifier tiles ahead of it, bridge
                // `handle_enum_element`); hand-built tables carry the same
                // word. `augment Xy;` elements are augmentation constants
                // and skip (`node.isAugmentation`,
                // constant_identifier_names.dart:59-61 — a new value inside
                // `augment enum E` carries no own `augment` and still
                // checks). The begin-token anchor of an annotated element
                // sits on the metadata, so only a real identifier token
                // names the constant.
                let Some(name) = c.field(node, PayloadField::Name) else {
                    return;
                };
                if name == crate::rules::NONE_TOKEN
                    || c
                        .field(node, PayloadField::AugmentKeyword)
                        .is_some_and(|kw| kw != crate::rules::NONE_TOKEN)
                    || !c
                        .unit
                        .tokens
                        .get(name as usize)
                        .is_some_and(|token| {
                            token.kind == dartq_scanner::TokenKind::Identifier
                        })
                {
                    return;
                }
                super::check_lower_camel(c, name, c.lexeme(name));
            }
            NodeKind::DeclaredVariablePattern => {
                // Name token is the pattern's `Variable` word (`variable: T`).
                if super::is_field_name_shortcut(c, node) {
                    return;
                }
                if let Some(name) = c.field(node, PayloadField::Variable)
                    && name != crate::rules::NONE_TOKEN
                {
                    super::check_lower_camel(c, name, c.lexeme(name));
                }
            }
            _ => {}
        }
    }
}

pub(crate) mod no_leading_underscores_for_local_identifiers {
    use super::{PayloadField, Check, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/no_leading_underscores_for_local_identifiers.dart:36-133`
    /// — local identifiers (local variables, catch parameters, non-named
    /// formal parameters other than field/super formals, local functions,
    /// pattern-bound variables) must not start with `_`; bare-underscore
    /// wildcard names are exempt. The official rule's second diagnostic
    /// variant (the *shadowing* rename suggestion, `:126-133`) needs the
    /// element model and is not part of wave 1. Message: `diagnostic.g.dart`
    /// `noLeadingUnderscoresForLocalIdentifiers`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "no_leading_underscores_for_local_identifiers",
    &[
            NodeKind::VariablesDeclaration,
            NodeKind::CatchClause,
            NodeKind::FormalParameters,
            NodeKind::OptionalFormalParameters,
            NodeKind::LocalFunctionDeclaration,
            NodeKind::DeclaredVariablePattern,
        ],
    check,
);

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        match c.kind(node) {
            NodeKind::VariablesDeclaration => {
                // Locals only: fields and top-level variables are not local
                // identifiers (the official visits variable *statements* and
                // for-parts initializers, never field declarations).
                if let Some(parent) = c.ancestor(1)
                    && matches!(c.kind(parent), NodeKind::Fields | NodeKind::TopLevelFields)
                {
                    return;
                }
                // Gather under the scratch borrow, then report.
                let mut hits: Vec<u32> = Vec::new();
                for var in c.direct_children(node).iter().copied() {
                    if c.kind(var) == NodeKind::InitializedIdentifier {
                        // The name is the span head (width-0 kind).
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
                        // name: T is the formal's `Name` word.
                        if let Some(name) = c.field(child, PayloadField::Name)
                            && name != crate::rules::NONE_TOKEN
                        {
                            hits.push(name);
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
                // opener is the section span head (width-0 kind).
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
                if let Some(name) = c.field(node, PayloadField::Variable)
                    && name != crate::rules::NONE_TOKEN
                {
                    report_underscored(c, name);
                }
            }
            _ => {}
        }
    }

    /// Checks the non-field formals of one parameters section (required or
    /// positional-optional — named sections never reach here). Field
    /// (`this.x`) and super formals are exempt (:99-104): the `Receiver`
    /// word is the `this`/`super` token when present. Hits are gathered
    /// under the scratch borrow, then reported.
    fn report_positional_formals(c: &mut Check<'_>, node: u32) {
        let mut hits: Vec<u32> = Vec::new();
        for child in c.direct_children(node).iter().copied() {
            if c.kind(child) != NodeKind::FormalParameter {
                continue;
            }
            if let Some(receiver) = c.field(child, PayloadField::Receiver)
                && receiver != crate::rules::NONE_TOKEN
            {
                continue;
            }
            if let Some(name) = c.field(child, PayloadField::Name)
                && name != crate::rules::NONE_TOKEN
            {
                hits.push(name);
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
        let Some(start) = super::utf16_start_of(c, token) else {
            return;
        };
        c.report(start..start, &[lexeme]);
    }
}

pub(crate) mod non_constant_identifier_names {
    use super::{PayloadField, Check, NodeKind, RuleSpec, is_just_underscores};
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
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "non_constant_identifier_names",
    &[
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
);

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
                // Real parses carry only `late` in payload word 0 (the
                // collapse drops the `var`/`final`/`const` keyword), so the
                // keyword is read through the shared declaration-keyword
                // recovery — the word-0 read would let every `const`
                // declaration through.
                if crate::rules::variables_declaration_keyword(c, node)
                    .is_some_and(|kw| c.lexeme(kw) == "const")
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
                        && let Some(name) = c.field(child, PayloadField::Name)
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
                if let Some(name) = c.field(node, PayloadField::Variable)
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
                    if c
                        .field(child, PayloadField::Receiver)
                        .is_some_and(|recv| recv != NONE_TOKEN && c.lexeme(recv) == "this")
                    {
                        continue;
                    }
                    // An unnamed formal has no name to check — the official
                    // `checkIdentifier(p.name)` nullability
                    // (non_constant_identifier_names.dart, the
                    // `visitFormalParameterList` arm). Real parses carry the
                    // `handleNoName` posture: the closer token sits in the
                    // name word of a generic function-type literal's
                    // formals (`void Function(int)`), and recovery inserts
                    // empty synthetic identifiers; both are skipped here, so
                    // only a written name (a real, non-synthetic identifier)
                    // is style-checked.
                    if let Some(name) = c.field(child, PayloadField::Name)
                        && name != NONE_TOKEN
                        && c.unit
                            .tokens
                            .get(name as usize)
                            .is_some_and(|token| token.kind == dartq_scanner::TokenKind::Identifier)
                        && !c.token_is_synthetic(name)
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
                    c.kind(ch) == NodeKind::Identifier && c.field(ch, PayloadField::Context) == Some(ctx)
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
                    c.kind(ch) == NodeKind::Identifier && c.field(ch, PayloadField::Context) == Some(ctx)
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
                    c.kind(ch) == NodeKind::Identifier && c.field(ch, PayloadField::Context) == Some(ctx)
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
                    let Some(colon) = c.field(field, PayloadField::Colon) else {
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
                c.kind(ch) == NodeKind::Identifier && c.field(ch, PayloadField::Context) == Some(ctx)
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
        let Some(start) = super::utf16_start_of(c, token) else {
            return;
        };
        c.report(start..start, &[lexeme]);
    }
}

pub(crate) mod file_names {
    use super::{PayloadField, Check, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/file_names.dart:24-42` — the file
    /// base name must be `lowercase_with_underscores`
    /// (`isValidDartFileName`, `util/ascii_utils.dart:20-52`), reported at
    /// offset 0. Runs once per compilation unit; a run without a path
    /// mirrors the official `element != null` guard by skipping. The
    /// official judges the *library's* defining file through the element
    /// model (`element.library.firstFragment.source.shortName`, :37-43), so
    /// a defining unit judges its own name and a `part of` unit re-judges
    /// the library's name into the part's own coordinates — the URI form
    /// (`part of 'foo.dart';`) carries the name lexically, the name form
    /// (`part of foo;`) is resolution-bound and stays silent.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "file_names",
    &[NodeKind::CompilationUnit],
    check,
);

    fn check(c: &mut Check<'_>) {
        // A `part of` unit never judges its own name: the official reads
        // the library's defining file through the element model, so the
        // part unit re-judges the LIBRARY's name into the part's own
        // coordinates (:37-43 — the URI form carries the name lexically;
        // the name form is resolution-bound and stays silent).
        if is_part_unit(c) {
            report_part_of_library_name(c);
            return;
        }
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

    /// Whether the unit carries a `part of` header. Directives wrap in a
    /// `TopLevelDeclaration` prelude, so each unit child is read through
    /// the wrapper (the same walk the annotation rules use).
    fn is_part_unit(c: &Check<'_>) -> bool {
        let Some(root) = c.unit.ast.root() else {
            return false;
        };
        c.collect_direct(root).into_iter().any(|kid| {
            let node = behind_directive_prelude(c, kid);
            c.kind(node) == NodeKind::PartOf
        })
    }

    /// The part-unit face (:37-43): the URI form of `part of` names the
    /// library file exactly, so its base name is judged into the part's
    /// own coordinates — the official resolves `element.library` to the
    /// same name. The name form (`part of foo;`) is resolution-bound and
    /// stays silent.
    fn report_part_of_library_name(c: &mut Check<'_>) {
        let Some(root) = c.unit.ast.root() else {
            return;
        };
        let part_of = c.collect_direct(root).into_iter().find_map(|kid| {
            let node = behind_directive_prelude(c, kid);
            (c.kind(node) == NodeKind::PartOf).then_some(node)
        });
        let Some(part_of) = part_of else {
            return;
        };
        let Some(name) = part_of_uri_library_name(c, part_of).map(str::to_owned) else {
            return;
        };
        if is_valid_dart_file_name(&name) {
            return;
        }
        c.report(0..0, &[name.as_str()]);
    }

    /// The library file name carried by a URI-form `part of` (`part of
    /// 'a/b/foo.dart';` — the first `String` token after the `of`
    /// keyword's base name).
    fn part_of_uri_library_name<'a>(c: &'a Check<'_>, part_of: u32) -> Option<&'a str> {
        let Some(of_token) = c.field(part_of, PayloadField::OfKeyword) else {
            return None;
        };
        let mut token = of_token;
        for _ in 0..4 {
            token += 1;
            let t = c.unit.tokens.get(token as usize)?;
            if t.kind == dartq_scanner::TokenKind::String {
                let uri = c.lexeme(token);
                let inner = uri.trim_matches(|ch| ch == '\'' || ch == '"');
                return inner.rsplit('/').next();
            }
        }
        None
    }

    /// The node behind a directive prelude wrapper — directives open a
    /// `TopLevelDeclaration` around themselves, so a unit child that is a
    /// wrapper is read through its first child.
    fn behind_directive_prelude(c: &Check<'_>, node: u32) -> u32 {
        if c.kind(node) != NodeKind::TopLevelDeclaration {
            return node;
        }
        c.collect_direct(node).first().copied().unwrap_or(node)
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
    use super::{PayloadField, Check, NodeKind, RuleSpec};
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
    /// `test_field`). Pattern-bound wildcard variables join the table —
    /// the pattern-variable face the official flags (the bare `_` pattern
    /// element binds no variable, so its uses are not wildcard-variable
    /// uses). Message:
    /// `diagnostic.g.dart` `noWildcardVariableUses`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "no_wildcard_variable_uses",
    &[NodeKind::Identifier],
    check,
);

    fn check(c: &mut Check<'_>) {
        // Feature gate: with wildcards on, `_` never binds, so there is no
        // wildcard use to flag — the official never registers (:36-38).
        if c.unit.features.wildcard_variables {
            return;
        }
        let node = c.node();
        // Uses only: the expression contexts (the official `test_localVar`
        // reports the `print(_)` use, never the declaration).
        let Some(ctx) = c.field(node, PayloadField::Context) else {
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
    use super::{PayloadField, Check, NodeKind, RuleSpec, utf16_start_of};

    /// The dart:core type names every scope resolves to a class (the bare
    /// lexical face of the official's element test — `ClassElement`
    /// membership). `dynamic` is excluded faithfully: the dynamic element is
    /// none of the official's accepted element kinds.
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
    /// The visibility test (`resolveNameInScope` + the element-kind check,
    /// :117-142) runs over the same-file channel: the dart:core built-ins
    /// resolve to classes, a same-file class / mixin / enum /
    /// extension-type / typedef declaration resolves to a `ClassElement` /
    /// `ExtensionTypeElement` / `TypeAliasElement`, and the in-scope type
    /// parameters along the ancestor path resolve to the accepted
    /// `TypeParameterElement` kind — the formal face reads the owning
    /// declaration's own list too (a parameter list resolves inside its
    /// function), the type-parameter face starts above its owner and
    /// suppresses a type-parameter match under a method-owned list (the
    /// `isShadowedByMethodTypeParameter` exemption, :87-98, :129-135), with
    /// the just-underscore wildcard gate standing in for
    /// `!element.isWildcardVariable`. Imported declarations stay out of the
    /// syntax face and under-report. The formal faces keep the official's
    /// `hasImplicitType` gate (:63-72 — a written type annotation opts out)
    /// and its field-formal exclusion (:68 — `this.x`/`super.x` parameters
    /// never report); the catch face (:49-59) applies neither. Primary-
    /// constructor declaring formals split the same way the element model
    /// does: a storage-marked declaring formal creates the field (the
    /// `FieldFormalParameterElement` gate — the official
    /// `test_constructor_primary_declaring` no-hit) and skips, an unmarked
    /// one keeps reporting (`test_constructor_primary_requiredPositional`).
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "avoid_types_as_parameter_names",
    &[NodeKind::FormalParameter, NodeKind::TypeVariable],
    check,
);

    fn check(c: &mut Check<'_>) {
        match c.kind(c.node()) {
            NodeKind::FormalParameter => check_parameter(c),
            NodeKind::TypeVariable => check_type_variable(c),
            _ => {}
        }
    }

    fn check_parameter(c: &mut Check<'_>) {
        let node = c.node();
        let Some(name_token) = c.field(node, PayloadField::Name) else {
            return;
        };
        let name = c.lexeme(name_token);
        // The name set: the built-ins and same-file declarations, plus the
        // in-scope type parameters (the `TypeParameterElement` arm — a
        // formal list resolves inside its function, so the owning
        // declaration's own type parameters are in scope and the accepted
        // kind matches; the wildcard gate mirrors
        // `!element.isWildcardVariable`).
        if !is_visible_type_name(c, name) && !type_param_in_scope(c, name, None) {
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
                .field(node, PayloadField::Receiver)
                .filter(|&token| token != crate::rules::NONE_TOKEN)
                .is_some_and(|token| c.lexeme(token) == "this");
            if typed || field_formal {
                return;
            }
            // A storage-marked primary-constructor declaring formal creates
            // the field (the field-formal gate, `final A` in
            // `class B(final A)` — the official no-hit); an unmarked one
            // keeps reporting (`class B(A)`).
            if primary_ctor_declaring_formal(c) && storage_marked(c, node) {
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
        let name = c.lexeme(name_token);
        // The visibility test over the type-parameter face: resolution
        // starts above the owning declaration (its own list is never in
        // scope at its own position), a type-parameter match shadows the
        // same-file and built-in faces (the innermost scope), and a
        // method-owned list suppresses a type-parameter match — the
        // `isShadowedByMethodTypeParameter` exemption (:129-135, the
        // `test_classTypeParameter_*MethodTypeParameter` no-hits).
        let type_param = type_param_in_scope(c, name, c.ancestor(1));
        let report = if type_param {
            !c.ancestor(2)
                .is_some_and(|owner| c.kind(owner) == NodeKind::Method)
        } else {
            is_visible_type_name(c, name)
        };
        if !report {
            return;
        }
        report_at_token(c, name_token, TYPE_PROBLEM, TYPE_CORRECTION);
    }

    /// The `_isTypeName` visibility test (:117-142) over the same-file
    /// channel: a dart:core built-in resolves to its class, a same-file
    /// class-like/typedef declaration resolves to a `ClassElement` /
    /// `ExtensionTypeElement` / `TypeAliasElement` — every kind the official
    /// accepts (plain extensions are rejected upstream).
    fn is_visible_type_name(c: &Check<'_>, name: &str) -> bool {
        BUILTIN_TYPES.contains(&name)
            || crate::rules::expressions::same_file::declares_type(c, name)
    }

    /// The `TypeParameterElement` arm of the accepted kinds (:125-141): a
    /// type parameter of an enclosing declaration resolves along the
    /// ancestor path. `skip` excludes one `TypeVariables` group (the
    /// checked type parameter's own list — a list is never in scope at its
    /// own position, the resolution scope starts above its owner). The
    /// wildcard gate mirrors `!element.isWildcardVariable`: with the
    /// feature on a just-underscore type parameter binds nothing.
    fn type_param_in_scope(c: &Check<'_>, name: &str, skip: Option<u32>) -> bool {
        if c.unit.features.wildcard_variables && super::is_just_underscores(name) {
            return false;
        }
        for &ancestor in c.path.iter().rev().skip(1) {
            let variables = c
                .collect_direct(ancestor)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::TypeVariables);
            let Some(variables) = variables else {
                continue;
            };
            if skip == Some(variables) {
                continue;
            }
            for variable in c.collect_direct(variables) {
                if c.kind(variable) == NodeKind::TypeVariable
                    && c.lexeme(c.head_token(variable)) == name
                {
                    return true;
                }
            }
        }
        false
    }

    /// Whether the checked formal is a primary-constructor declaring formal:
    /// the production tiling wraps the parsed `class C(...)` / `enum E(...)`
    /// head's parameter list in a [`NodeKind::PrimaryConstructor`] row (the
    /// bridge's `begin_primary_constructor` opens before the formals and
    /// `end_primary_constructor` closes after them); the class-like kinds
    /// cover a direct-adoption tiling.
    fn primary_ctor_declaring_formal(c: &Check<'_>) -> bool {
        let Some(idx) = c
            .path
            .iter()
            .rposition(|&n| c.kind(n) == NodeKind::FormalParameters)
        else {
            return false;
        };
        let Some(&owner) = idx.checked_sub(1).and_then(|i| c.path.get(i)) else {
            return false;
        };
        matches!(
            c.kind(owner),
            NodeKind::PrimaryConstructor
                | NodeKind::ClassDeclaration
                | NodeKind::EnumDeclaration
                | NodeKind::ExtensionTypeDeclaration
                | NodeKind::MixinDeclaration
        )
    }

    /// Whether the formal carries a storage keyword (`final`/`var`/`const`)
    /// — the `varFinalOrConst` `ModSet` bit (bridge `mod_flags`), the
    /// keyword recovered by a bounded scan past `required`/`covariant`.
    fn storage_marked(c: &Check<'_>, param: u32) -> bool {
        const VAR_FINAL_OR_CONST_BIT: u16 = 1 << 6;
        if dartq_ast::mod_flags(c.unit.ast, param) & VAR_FINAL_OR_CONST_BIT == 0 {
            return false;
        }
        let mut token = c.head_token(param);
        for _ in 0..3 {
            match c.lexeme(token) {
                "final" | "var" | "const" => return true,
                "required" | "covariant" => token += 1,
                _ => return false,
            }
        }
        false
    }

    /// `reportAtToken(name)` with the variant's message texts.
    fn report_at_token(c: &mut Check<'_>, token: u32, problem: &str, correction: &str) {
        let name = c.lexeme(token);
        let (Some(start), Some(end)) = (utf16_start_of(c, token), c.utf16_end_of_token(token))
        else {
            return;
        };
        c.report_texts(start..end, problem, correction, &[name]);
    }
}

pub(crate) mod use_super_parameters {
    use super::{PayloadField, Check, NodeKind, RuleSpec};
    use crate::rules::expressions::query_site;
    use crate::rules::types::leak_formal_rows;
    use crate::rules::{NONE_TOKEN, body_members};
    use dartq_semantic::model::{ArgListId, Ty};

    /// The body-child kinds of a constructor declaration (the declaration
    /// and body payload layouts).
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
    /// initializer list decides which parameters could be super parameters,
    /// reported on the constructor's name range. Two conversion phases feed
    /// one identifier list (:82-92): positional bare-identifier super
    /// arguments matched to constructor parameters in order (:107-190), and
    /// named parameters the super constructor also declares named, passed
    /// as `name: name` and unreferenced in the body (:196-271); one name
    /// reports the single code, several the joined form (:93-99).
    ///
    /// dartq faces. Both registration faces mount: the secondary
    /// constructor (`ConstructorDeclaration.errorRange`, the class-name
    /// through constructor-name span, `ast.dart:10800-10804`) and the
    /// primary constructor (`class C(...)` formals with the `this :` body,
    /// whose error range anchors on the `const` keyword when written, else
    /// the class name — `ast.dart:44668-44672`). The positional phase is
    /// syntax-level: the element tests proxy to names (an argument naming
    /// no parameter is the unresolved bad state, :155-156), and the
    /// j-scan's abort faces (:162-176) read every parameter up to each
    /// match — a `this.`-formal there kills the positional face (positional
    /// death keeps the named phase alive, `return []`), a `super.`-formal
    /// is the bad state that kills everything (`return null`).
    ///
    /// The named phase (:196-271) answers through the semantic read face:
    /// the super constructor resolves from the extends-clause type position
    /// keyed on the invocation's written selector (a bare `super(...)` is
    /// the unnamed face), a semantic-free run stays silent, and
    /// cross-library supers stay silent. The assignability gate
    /// (`typeSystem.isAssignableTo`, :259-266) is a written-spelling face:
    /// an implicit or `dynamic` super parameter passes (either direction
    /// of `dynamic` is mutual), and a named super type passes when our side
    /// is implicit or spells the same base name without type arguments —
    /// nullability differences keep the face (either direction of `?` is
    /// mutual assignability). The mutual-assignability remainder — subtype
    /// spellings (`Object`/`int`) and type-argumented types — suppresses.
    /// Body references stay name-level: every bare `Identifier` of the
    /// parameter's name counts except property positions and named-argument
    /// labels — a shadowing local of the same name over-counts
    /// (conservative, under-reports the conversion). The `super_parameters`
    /// feature gate reads the [`crate::engine::Features`] bit at check
    /// entry (:43-45).
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "use_super_parameters",
    &[NodeKind::Constructor, NodeKind::PrimaryConstructor],
    check,
);

    /// One constructor parameter: source-order index, written name, whether
    /// it sits in a `{...}` group (kind word 1|2), and the receiver keyword
    /// (`this.`/`super.`) when the formal is a receiver formal.
    struct Param<'a> {
        row: u32,
        name_token: u32,
        name: &'a str,
        index: usize,
        named: bool,
        receiver: Option<&'a str>,
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the check mirrors the official visitor's two-phase gate order; \
                  splitting it into helpers would scatter the pinned :ref mapping \
                  for no behavioral gain"
    )]
    fn check(c: &mut Check<'_>) {
        if !c.unit.features.super_parameters {
            return;
        }
        let node = c.node();
        // The two registration faces: formals owner, the initializer list
        // (the constructor's own, or the primary-constructor body's), the
        // body for the reference scan, and the report range.
        let (initializers, body, error_range, formals) = match c.kind(node) {
            NodeKind::Constructor => {
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
                    .rfind(|&kid| BODY_KINDS.contains(&c.kind(kid)));
                let Some(range) = constructor_error_range(c, &kids) else {
                    return;
                };
                (initializers, body, range, leak_formal_rows(c, parameters))
            }
            NodeKind::PrimaryConstructor => {
                let Some(class_row) = c.path.iter().rev().skip(1).copied().find(|&row| {
                    matches!(
                        c.kind(row),
                        NodeKind::ClassDeclaration | NodeKind::EnumDeclaration
                    )
                }) else {
                    return;
                };
                let body_row = body_members(c, class_row)
                    .into_iter()
                    .find(|&member| c.kind(member) == NodeKind::PrimaryConstructorBody);
                let initializers = body_row.and_then(|rows| {
                    c.collect_direct(rows)
                        .into_iter()
                        .find(|&kid| c.kind(kid) == NodeKind::Initializers)
                });
                let body = body_row.and_then(|rows| {
                    c.collect_direct(rows)
                        .into_iter()
                        .rfind(|&kid| BODY_KINDS.contains(&c.kind(kid)))
                });
                let Some(range) = primary_error_range(c, class_row, node) else {
                    return;
                };
                // The primary row's own formal list (the header `(...)`).
                let Some(parameters) = c
                    .collect_direct(node)
                    .into_iter()
                    .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
                else {
                    return;
                };
                (initializers, body, range, leak_formal_rows(c, parameters))
            }
            _ => return,
        };
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
        let params: Vec<Param<'_>> = formals
            .iter()
            .enumerate()
            .filter_map(|(index, &param)| {
                let receiver = c
                    .field(param, PayloadField::Receiver)
                    .filter(|&token| token != NONE_TOKEN)
                    .map(|token| c.lexeme(token));
                let name_token = c.field(param, PayloadField::Name)?;
                (name_token != NONE_TOKEN).then(|| Param {
                    row: param,
                    name_token,
                    name: c.lexeme(name_token),
                    index,
                    named: matches!(
                        c.field(param, PayloadField::FormalParameterKind),
                        Some(1 | 2)
                    ),
                    receiver,
                })
            })
            .collect();
        // —— phase 1: the positional conversion (:107-190) ——
        let (all_dead, mut identifiers) = positional_phase(c, &params, arguments, body);
        if all_dead {
            return;
        }
        // —— phase 2: the named conversion (:196-271) —— the resolved super
        // constructor's named formals; a semantic-free run contributes
        // nothing.
        if let Some(analysis) = c.semantics() {
            let class_row = c.path.iter().rev().skip(1).copied().find(|&row| {
                matches!(
                    c.kind(row),
                    NodeKind::ClassDeclaration | NodeKind::EnumDeclaration
                )
            });
            let type_row = class_row.and_then(|class_row| {
                super::matching_super_parameters::extends_type_row(c, class_row)
            });
            let selector = initializers
                .and_then(|list| {
                    super::matching_super_parameters::super_invocation_selector(c, list)
                })
                .map(|name| analysis.strings().intern(name));
            let binding = type_row
                .and_then(|type_row| {
                    analysis.resolve_constructor(query_site(c, type_row), selector)
                })
                .filter(|binding| binding.exists);
            if let Some(formals) = binding.and_then(|binding| binding.formals) {
                for (position, param) in params.iter().enumerate() {
                    // Field/super formals never convert (:201-202; a super
                    // formal dies at the matching-argument face).
                    if param.receiver.is_some() || !param.named {
                        continue;
                    }
                    // Unreferenced in the body (:204).
                    if body.is_some_and(|body| is_referenced(c, body, param.name)) {
                        continue;
                    }
                    // The super constructor declares the name named
                    // (:238-248).
                    let Some(super_formal) =
                        formals.named_formal(analysis.strings().intern(param.name))
                    else {
                        continue;
                    };
                    // Passed as `name: name` (:214-232).
                    if !named_argument_passes(c, arguments, param.name) {
                        continue;
                    }
                    // The written-spelling assignability gate (:259-266).
                    if !assignable_gate(analysis, super_formal.ty, written_type(c, param)) {
                        continue;
                    }
                    identifiers.push(position);
                }
            }
        }
        if identifiers.is_empty() {
            return;
        }
        // The official fills `{0}` with the bare parameter name (:93-98);
        // the multiple form fills it with the pre-quoted
        // `quotedAndCommaSeparatedWithAnd` list, so the rendered text
        // carries the quotes twice — mirrored verbatim.
        if identifiers.len() == 1 {
            let name = params[identifiers[0]].name;
            c.report_texts(error_range, SINGLE_PROBLEM, SINGLE_CORRECTION, &[name]);
        } else {
            let names: Vec<&str> = identifiers.iter().map(|&i| params[i].name).collect();
            let joined = quoted_and_comma_separated_with_and(&names);
            c.report_texts(
                error_range,
                MULTIPLE_PROBLEM,
                MULTIPLE_CORRECTION,
                &[&joined],
            );
        }
    }

    /// The super call's argument list as `(value, label)` pairs in source
    /// order, over the canonical argument grouping ([`crate::rules::slice::
    /// arguments`]): a named group flattens as `name value… NamedArgument`
    /// (the marker row anchors at the value head, `slice::named_arg_name`
    /// recovers the name), so its value face is the run's first piece; a
    /// positional group's value is its subtree-end row.
    fn argument_pairs(c: &Check<'_>, arguments: u32) -> Vec<(u32, Option<u32>)> {
        let pieces = c.collect_direct(arguments);
        crate::rules::slice::arguments(c, &pieces)
            .into_iter()
            .filter_map(|range| {
                if c.kind(pieces[range.end - 1]) == NodeKind::NamedArgument {
                    let name_idx = crate::rules::slice::named_arg_name(c, &pieces, range.end - 1)?;
                    Some((pieces[name_idx + 1], Some(pieces[name_idx])))
                } else {
                    Some((pieces[range.end - 1], None))
                }
            })
            .collect()
    }

    /// Whether `row` is a bare simple-identifier expression (the official
    /// `argument.expression is SimpleIdentifier`). The flattened window's
    /// subtree-end row for a lone name is the parseSend send leaf, so two
    /// shapes count: an `Identifier` row whose extent reaches the argument
    /// boundary (the next token is the `,` or the closing paren), and a
    /// `Send` leaf whose head is followed by that boundary (an invocation
    /// send's head is followed by `(`, a selector by `.`).
    fn bare_identifier_value(c: &Check<'_>, row: u32) -> bool {
        let boundary = |token: &dartq_scanner::Token| {
            matches!(
                token.kind,
                dartq_scanner::TokenKind::Comma | dartq_scanner::TokenKind::CloseParen
            )
        };
        match c.kind(row) {
            NodeKind::Identifier => c
                .unit
                .tokens
                .get(c.unit.ast.token_range(row).end as usize)
                .is_none_or(boundary),
            NodeKind::Send => c
                .unit
                .tokens
                .get(c.head_token(row) as usize + 1)
                .is_none_or(boundary),
            _ => false,
        }
    }

    /// The positional conversion (:107-190): bare-identifier super
    /// arguments matched to parameters in order. Returns whether the bad
    /// state killed the whole check (`return null`) and the converted
    /// parameter positions (`[]` = none — the named phase still runs).
    fn positional_phase(
        c: &Check<'_>,
        params: &[Param<'_>],
        arguments: u32,
        body: Option<u32>,
    ) -> (bool, Vec<usize>) {
        let mut converted: Vec<usize> = Vec::new();
        let mut matched_index = 0usize;
        let mut seen: Vec<&str> = Vec::new();
        let mut saw_positional = false;
        for (arg, label) in argument_pairs(c, arguments) {
            if label.is_some() {
                continue;
            }
            // A non-identifier positional argument leaves no convertible
            // list (:151-153 — `return []`).
            if !bare_identifier_value(c, arg) {
                return (false, Vec::new());
            }
            saw_positional = true;
            let name = c.lexeme(c.head_token(arg));
            // Passed more than once is the bad state (:159-161 — `return
            // null`).
            if seen.contains(&name) {
                return (true, Vec::new());
            }
            seen.push(name);
            // An argument that names no parameter is not a formal element —
            // the bad state (:155-156); a `{...}`-group parameter passed
            // positionally too (:157-158).
            let Some(position) = params.iter().position(|p| p.name == name) else {
                return (true, Vec::new());
            };
            if params[position].named {
                return (true, Vec::new());
            }
            // The j-scan inspects every parameter up to the match (:175-186):
            // a `this.`-formal there kills the positional face (:163-164),
            // a `super.`-formal is the bad state (:165-166), a referenced
            // one leaves nothing convertible (:168-169).
            for p in &params[..=position] {
                if p.receiver == Some("this") || p.receiver == Some("super") {
                    return (p.receiver == Some("super"), Vec::new());
                }
                if body.is_some_and(|body| is_referenced(c, body, p.name)) {
                    return (false, Vec::new());
                }
            }
            // Out-of-order parameters leave no convertible list (:178-180).
            let source_index = params[position].index;
            if source_index < matched_index {
                return (false, Vec::new());
            }
            matched_index = source_index;
            converted.push(position);
        }
        // No positional super arguments — nothing convertible (:149-150).
        if !saw_positional {
            return (false, Vec::new());
        }
        (false, converted)
    }

    /// The named phase's assignability face (:259-266) over the written
    /// spelling: an implicit or `dynamic` super parameter passes (either
    /// direction of `dynamic` is mutual assignability); a named super type
    /// passes when our side is implicit or spells the same base name
    /// without type arguments. Unmodeled supers and uncomparable written
    /// shapes suppress.
    fn assignable_gate(
        analysis: &dartq_semantic::Analysis<'_>,
        super_ty: Ty,
        written: Option<Option<&str>>,
    ) -> bool {
        match super_ty {
            Ty::Dynamic => true,
            Ty::Named { name, args, .. } => {
                if args != ArgListId::EMPTY {
                    return false;
                }
                match written {
                    None => false,
                    Some(None) => true,
                    Some(Some(base)) => base == analysis.strings().resolve(name),
                }
            }
            _ => false,
        }
    }

    /// The formal's written declared type, for the assignability gate:
    /// `None` — a written shape the gate cannot compare (function types,
    /// type arguments) — suppresses; `Some(None)` — no written type (the
    /// implicit-`dynamic` face); `Some(Some(base))` — a plain named
    /// spelling (`int`, `p.C?`), nullability dropped (either direction of
    /// `?` keeps the mutual-assignability face).
    fn written_type<'a>(c: &'a Check<'_>, param: &Param<'a>) -> Option<Option<&'a str>> {
        let mut base: Option<&str> = None;
        let mut token = c.head_token(param.row);
        while token < param.name_token {
            let lexeme = c.lexeme(token);
            match lexeme {
                "required" | "covariant" | "final" | "var" | "const" | "late" | "." | "?" => {}
                _ => {
                    let token_row = c.unit.tokens.get(token as usize)?;
                    if token_row.kind != dartq_scanner::TokenKind::Identifier {
                        return None;
                    }
                    base = Some(lexeme);
                }
            }
            token += 1;
        }
        Some(base)
    }

    /// The super invocation passes the parameter as `name: name` (:214-232):
    /// a named argument whose label names the parameter and whose value is
    /// the bare identifier of the same name (the official compares the
    /// expression's element to the parameter's).
    fn named_argument_passes(c: &Check<'_>, arguments: u32, name: &str) -> bool {
        argument_pairs(c, arguments).iter().any(|&(value, label)| {
            label.is_some_and(|label| c.lexeme(c.head_token(label)) == name)
                && bare_identifier_value(c, value)
                && c.lexeme(c.head_token(value)) == name
        })
    }

    /// `ConstructorDeclaration.errorRange` (ast.dart:10800-10804): the span
    /// from the return type (the first `Identifier` child) through the
    /// constructor name (the second, when written).
    fn constructor_error_range(c: &Check<'_>, kids: &[u32]) -> Option<std::ops::Range<u32>> {
        let names: Vec<u32> = kids
            .iter()
            .copied()
            .filter(|&kid| c.kind(kid) == NodeKind::Identifier)
            .collect();
        let &return_type = names.first()?;
        let end_token = c
            .unit
            .ast
            .token_range(names.get(1).copied().unwrap_or(return_type))
            .end
            - 1;
        Some(utf16_start(c, c.head_token(return_type))?..utf16_end(c, end_token)?)
    }

    /// `PrimaryConstructorDeclaration.errorRange` (ast.dart:44668-44672):
    /// the `const` keyword when written, else the class name; through the
    /// `.name` continuation when written (the primary row anchors on the
    /// `(`/`.` — the continuation form opens it at the `.`).
    fn primary_error_range(
        c: &Check<'_>,
        class_row: u32,
        primary: u32,
    ) -> Option<std::ops::Range<u32>> {
        let head = c.head_token(class_row);
        let mut t = head;
        let start = loop {
            t += 1;
            let token = c.unit.tokens.get(t as usize)?;
            if token.kind == dartq_scanner::TokenKind::Identifier {
                break if c.lexeme(t - 1) == "const" { t - 1 } else { t };
            }
        };
        let end = if c.lexeme(c.head_token(primary)) == "." {
            c.head_token(primary) + 1
        } else {
            start
        };
        Some(utf16_start(c, start)?..utf16_end(c, end)?)
    }

    /// Whether the initializer is a super-constructor invocation: it wraps a
    /// `super` expression (the named `super.named(...)` form included — the
    /// selector reads off the tokens after the `super` keyword).
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

    fn utf16_start(c: &Check<'_>, token: u32) -> Option<u32> {
        c.utf16_of_token(token)
    }

    /// Exclusive UTF-16 end of `token`'s lexeme.
    fn utf16_end(c: &Check<'_>, token: u32) -> Option<u32> {
        c.utf16_end_of_token(token)
    }

    #[cfg(test)]
    mod tests {
        use crate::testsupport::{Fixture, one_rule};

        use super::SPEC;

        /// The named-conversion face over the real parse with the semantic
        /// face mounted (`test_named`): both named parameters convert.
        #[test]
        fn named_conversion_reports() {
            let source = "class A {\n  const A({int? x, int? y});\n}\n\nclass B extends A {\n  \
                          const B({int? x, int? y}) : super(x: x, y: y);\n}\n";
            let lints = Fixture::parse(source).run_semantic(&one_rule(SPEC.name));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Parameters ''x' and 'y'' could be super parameters."
            );
        }

        /// A named parameter referenced in the body never converts
        /// (`test_named_someReferencedInBody`): `x` stays, `y` converts.
        #[test]
        fn named_referenced_in_body_stays() {
            let source = "class A {\n  A({int? x, int? y});\n}\n\nclass B extends A {\n  \
                          B({int? x, int? y}) : super(x: x, y: y) {\n    print(x);\n  }\n}\n";
            let lints = Fixture::parse(source).run_semantic(&one_rule(SPEC.name));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Parameter 'y' could be a super parameter."
            );
        }

        /// A field formal skips the named loop (`test_named_thisParameter`)
        /// and a renamed forwarded argument never matches
        /// (`test_named_oneWithNameChange`).
        #[test]
        fn named_this_parameter_and_name_change_faces() {
            let source = "class A {\n  A({int? x, int? y});\n}\n\nclass B extends A {\n  \
                          int? x;\n  B({this.x, int? y}) : super(x: x, y: y);\n}\n";
            let lints = Fixture::parse(source).run_semantic(&one_rule(SPEC.name));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Parameter 'y' could be a super parameter."
            );

            let source = "class A {\n  A({int? x, int? y});\n}\n\nclass B extends A {\n  \
                          B({int? x, int? z}) : super(x: x, y: z);\n}\n";
            let lints = Fixture::parse(source).run_semantic(&one_rule(SPEC.name));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Parameter 'x' could be a super parameter."
            );
        }

        /// The positional/named interaction faces: a non-identifier
        /// positional argument kills only the positional face
        /// (`test_requiredPositional_nonConvertible_withNamed`), a field
        /// formal before the match kills only the positional face
        /// (`test_requiredPositional_fieldFormal_withNamed`).
        #[test]
        fn positional_death_keeps_named_face() {
            let source = "class A {\n  A(int x, {int? y});\n}\n\nclass B extends A {\n  \
                          B(int x, {int? y}) : super(1, y: y);\n}\n";
            let lints = Fixture::parse(source).run_semantic(&one_rule(SPEC.name));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Parameter 'y' could be a super parameter."
            );

            let source = "class A {\n  A(int x, {int? y});\n}\n\nclass B extends A {\n  int z;\n  \
                          B(this.z, {int? y}) : super(z, y: y);\n}\n";
            let lints = Fixture::parse(source).run_semantic(&one_rule(SPEC.name));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Parameter 'y' could be a super parameter."
            );
        }

        /// The no-hit named shapes: not passed (`test_noLint_named_
        /// notPassed_unreferenced`), positional-super's named parameter
        /// (`test_noLint_requiredPositional_namedInSuper`), out-of-order
        /// forwarding, and a repeated argument (`test_repeatedParam`).
        #[test]
        fn named_no_hit_faces_stay_silent() {
            let f = |source| {
                Fixture::parse(source)
                    .run_semantic(&one_rule(SPEC.name))
                    .is_empty()
            };
            assert!(f(
                "class A {\n  A({int x = 0});\n}\n\nclass B extends A {\n  B({int x = 0}) : \
                 super(x: 0);\n}\n"
            ));
            assert!(f(
                "class A {\n  A({int? x});\n}\n\nclass B extends A {\n  B(int x) : super(x: \
                 x);\n}\n"
            ));
            assert!(f(
                "class B {\n  final int x;\n  final int y;\n  B(this.x, this.y);\n}\n\nclass C \
                 extends B {\n  C(int x, int y) : super(y, x);\n}\n"
            ));
            assert!(f(
                "class Rect {\n  Rect(this.width, this.height);\n\n  final double width;\n  \
                 final double height;\n}\n\nclass Square extends Rect {\n  Square(double \
                 dimension) : super(dimension, dimension);\n}\n"
            ));
        }

        /// The primary-constructor face (`test_named_primaryConstructor`):
        /// the `class const B(...)` header formals convert, reported on the
        /// `const` keyword (the primary error range).
        #[test]
        fn primary_constructor_named_conversion_reports() {
            let source = "class A {\n  const A({int? x, int? y});\n}\n\nclass const B({int? x, \
                          int? y}) extends A {\n  this : super(x: x, y: y);\n}\n";
            let lints = Fixture::parse(source).run_semantic(&one_rule(SPEC.name));
            assert_eq!(lints.len(), 1, "{lints:?}");
        }

        /// The plain-run posture: the named face is the semantic channel's
        /// alone, so a named-only super call stays silent without it.
        #[test]
        fn semantic_free_named_only_run_stays_silent() {
            let source = "class A {\n  A({int? x});\n}\n\nclass B extends A {\n  B({int? x}) : \
                          super(x: x);\n}\n";
            assert!(Fixture::parse(source).run(&one_rule(SPEC.name)).is_empty());
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::narrow_u32;
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
        // Upstream `reportAtToken(name)` spans the identifier
        // (constant_identifier_names_test.dart `test_topLevel_allCaps`
        // marks `[!PI!]`), so the end pins the token's tail.
        assert_eq!(lints[0].range.start, utf16_at(&f, 1));
        assert_eq!(lints[0].range.end, utf16_at(&f, 1) + 2);
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

    /// Enum constants are checked (`test_enumValue_upperFirstLetter` in
    /// the official test — the name is the element's `Name` word; the
    /// production row is a childless handle).
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
        // The full name span (`test_enumValue_upperFirstLetter` marks
        // `[!Baz!]`).
        assert_eq!(lints[0].range.end, utf16_at(&f, 3) + 13);
    }

    /// Production-bridge pins: the childless element row reads its name
    /// through the `Name` word, and a new value inside `augment enum` (no
    /// own `augment`) still checks (`test_augmentationEnum`). An
    /// `augment Xy;` element would skip via the `AugmentKeyword` word
    /// (`node.isAugmentation`, constant_identifier_names.dart:59-61), but
    /// the parser does not yet bind element-level `augment` — the same
    /// posture as the upstream analyzer, whose own
    /// `test_augmentedEnumValue` is `@FailingTest` (issue 56174); no pin
    /// until either side implements it.
    #[test]
    fn constant_identifier_names_production_enum_faces() {
        let f = Fixture::parse("enum Foo {\n  bar,\n  Baz,\n}\n");
        let lints = f.run(&one_rule("constant_identifier_names"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].range.end - lints[0].range.start,
            "Baz".encode_utf16().count() as u32
        );

        let f = Fixture::parse("part of 'a.dart';\n\naugment enum E {\n  Xy;\n}\n");
        let lints = f.run(&one_rule("constant_identifier_names"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].range.end - lints[0].range.start, 2);
    }

    /// Production-bridge pins for the pattern-field faces: a labeled
    /// field's variable reports spanning the name
    /// (`test_destructuredObjectField_switch`), the field-name shortcut
    /// `:var x` and its null-aware variants stay clean
    /// (`test_destructuredObjectField_switch_ok` — the official
    /// `isFieldNameShortcut` guard over the label-less field).
    #[test]
    fn constant_identifier_names_production_pattern_faces() {
        let f = Fixture::parse(
            "class A {\n  var a;\n}\n\nf(A a) {\n  switch (a) {\n    case A(a: int a_b):\n  }\n}\n",
        );
        let lints = f.run(&one_rule("constant_identifier_names"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].range.end - lints[0].range.start,
            "a_b".encode_utf16().count() as u32
        );

        let f = Fixture::parse(
            "class A {\n  var a_b;\n}\n\nf(A a) {\n  switch (a) {\n    case A(:var a_b):\n  }\n  switch (a) {\n    case A(:var a_b?):\n    case A(:var a_b!):\n  }\n}\n",
        );
        assert!(f.run(&one_rule("constant_identifier_names")).is_empty());
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

    /// `_` alone is the wildcard name — allowed (`ascii_utils.dart:61`).
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

    /// A `part of` unit is never the judged file (the official reads the
    /// library's defining file through the element model): a misnamed part
    /// stays silent.
    #[test]
    fn file_names_part_unit_passes() {
        let f = Fixture::parse("part of 'a.dart';");
        let mut f = f;
        f.path = std::path::PathBuf::from("BadPartName.dart");
        assert!(f.run(&one_rule("file_names")).is_empty());
    }

    /// The part unit judges the LIBRARY's file name into its own
    /// coordinates (:37-43): a URI-form `part of` under a bad-named
    /// library reports the library's name at offset 0.
    #[test]
    fn file_names_part_unit_reports_library_name() {
        let f = Fixture::parse("part of 'BadLib.dart';");
        let mut f = f;
        f.path = std::path::PathBuf::from("good_part.dart");
        let lints = f.run(&one_rule("file_names"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The file name 'BadLib.dart' isn't a lower_case_with_underscores identifier."
        );
        assert_eq!(lints[0].range.start, 0);

        // A well-named library keeps the part silent; a sub-directory URI
        // judges the base name only.
        let f = Fixture::parse("part of 'a.dart';");
        let mut f = f;
        f.path = std::path::PathBuf::from("good_part.dart");
        assert!(f.run(&one_rule("file_names")).is_empty());

        let f = Fixture::parse("part of 'lib/BadLib.dart';");
        let mut f = f;
        f.path = std::path::PathBuf::from("good_part.dart");
        assert_eq!(f.run(&one_rule("file_names")).len(), 1);
    }

    /// The name form of `part of` is resolution-bound (the library's file
    /// is unknowable from the syntax) and stays silent.
    #[test]
    fn file_names_part_unit_name_form_passes() {
        let f = Fixture::parse("part of foo;");
        let mut f = f;
        f.path = std::path::PathBuf::from("good_part.dart");
        assert!(f.run(&one_rule("file_names")).is_empty());
    }

    /// A defining unit — with or without `part` directives, which name
    /// other files — keeps the judgment.
    #[test]
    fn file_names_defining_unit_reports() {
        let mut f = Fixture::parse("part 'a.dart';\nvar x;");
        f.path = std::path::PathBuf::from("BadLibName.dart");
        assert_eq!(f.run(&one_rule("file_names")).len(), 1);
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
    // Fixtures over the bridge's `FormalParameter` contracts: the
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
            b.end(NodeKind::CompilationUnit, narrow_u32(t.len()));
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
            b.end(NodeKind::CompilationUnit, narrow_u32(t.len()));
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
            b.end(NodeKind::CompilationUnit, narrow_u32(t.len()));
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
            b.end(NodeKind::CompilationUnit, narrow_u32(t.len()));
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
            b.end(NodeKind::CompilationUnit, narrow_u32(t.len()));
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
            b.end(NodeKind::CompilationUnit, narrow_u32(t.len()));
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
            b.end(NodeKind::CompilationUnit, narrow_u32(t.len()));
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

    /// A same-file class name is visible (`resolveNameInScope` landing on a
    /// `ClassElement`): a parameter named like it reports; a same-file
    /// typedef lands on the accepted `TypeAliasElement` and reports too. An
    /// enum name resolves to an `EnumElement` — outside the accepted kinds,
    /// so it stays silent.
    #[test]
    fn same_file_type_names_report() {
        let f = Fixture::parse("class MyErr {}\nvoid f(MyErr) {}");
        let lints = avoid_hits(&f);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The parameter name 'MyErr' matches a visible type name."
        );

        let f = Fixture::parse("typedef T = int;\nvoid f(T) {}");
        assert_eq!(avoid_hits(&f).len(), 1);

        // An enum name resolves to an `EnumElement` upstream — not one of
        // the accepted kinds (`ClassElement` / `ExtensionTypeElement` /
        // `TypeAliasElement`), so a parameter named like it stays silent.
        let f = Fixture::parse("enum Color { red }\nvoid f(Color) {}");
        assert!(avoid_hits(&f).is_empty());

        // The type-parameter face reads the same-file set as well: a type
        // parameter named like a same-file class resolves to the class (the
        // parameter never shadows itself), so it reports.
        let f = Fixture::parse("class A {}\nvoid g<A>() {}");
        let lints = avoid_hits(&f);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The type parameter name 'A' matches a visible type name."
        );
    }

    /// A plain extension is NOT a visible type (upstream rejects
    /// `ExtensionElement`): a parameter named like one stays silent.
    #[test]
    fn same_file_extension_name_passes() {
        let f = Fixture::parse("extension E on int {}\nvoid f(E) {}");
        assert!(avoid_hits(&f).is_empty());
    }

    /// Primary-constructor declaring formals split the field-formal gate:
    /// a storage-marked declaring formal creates the field (the official
    /// `test_constructor_primary_declaring` no-hit) and stays silent; an
    /// unmarked one keeps reporting
    /// (`test_constructor_primary_requiredPositional`).
    #[test]
    fn primary_ctor_declaring_formals_split_on_storage() {
        let f = Fixture::parse("class A {}\nclass B(final A);");
        assert!(
            avoid_hits(&f).is_empty(),
            "the storage-marked declaring formal creates the field"
        );

        let f = Fixture::parse("class A {}\nclass B(A);");
        let lints = avoid_hits(&f);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The parameter name 'A' matches a visible type name."
        );
    }

    /// The `TypeParameterElement` arm over the formal face: a formal named
    /// like an in-scope type parameter reports — the owning function's own
    /// list (`test_parameter_shadowingTypeParameter`) and an enclosing
    /// class's list (`test_factoryParameter_shadowingTypeParameter`).
    #[test]
    fn formal_named_like_type_parameter_reports() {
        let f = Fixture::parse("void f<X>(X) {}");
        let lints = avoid_hits(&f);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The parameter name 'X' matches a visible type name."
        );

        let f = Fixture::parse("class C<X> {\n  factory C(X) => C.name();\n  C.name();\n}");
        let lints = avoid_hits(&f);
        assert_eq!(lints.len(), 1, "{lints:?}");
    }

    /// The type-parameter face over the in-scope arm: a class type
    /// parameter named like a built-in reports
    /// (`test_typeParameter_class`), and a method's type parameter named
    /// like a same-file class reports (the exemption covers
    /// `TypeParameterElement` matches only —
    /// `test_typeParameter_instanceMethod_class`).
    #[test]
    fn type_parameter_named_like_visible_type_reports() {
        let f = Fixture::parse("class A<Object> {}");
        assert_eq!(avoid_hits(&f).len(), 1);

        let f = Fixture::parse("class A<X> {\n  C instanceMethod<C>(C c) => c;\n}\n\nclass C {}");
        assert_eq!(avoid_hits(&f).len(), 1);
    }

    /// The method shadow exemption (:129-135): a method's own type
    /// parameter matching an enclosing declaration's type parameter stays
    /// silent for instance (`test_classTypeParameter_instanceMethodType
    /// Parameter`) and static
    /// (`test_classTypeParameter_staticMethodTypeParameter`) methods.
    #[test]
    fn method_shadowed_type_parameters_pass() {
        let f = Fixture::parse("class A<X> {\n  X instanceMethod<X>(X x) => x;\n}");
        assert!(avoid_hits(&f).is_empty());

        let f = Fixture::parse("class A<X> {\n  static X staticMethod<X>(X x) => x;\n}");
        assert!(avoid_hits(&f).is_empty());
    }

    /// The wildcard gate (`!element.isWildcardVariable`): with the feature
    /// on, a just-underscore type parameter binds nothing, so
    /// `class C<_> { var _; C.c(this._, _); }` stays silent
    /// (`test_typeParameter_wildcard`).
    #[test]
    fn wildcard_type_parameter_passes() {
        let f = Fixture::parse("class C<_> {\n  var _;\n  C.c(this._, _);\n}");
        assert!(avoid_hits(&f).is_empty());
    }

    // —— use_super_parameters ————————————————————————————————————————————

    fn super_hits(f: &Fixture) -> Vec<crate::engine::Lint> {
        f.run(&one_rule("use_super_parameters"))
    }

    /// The generic function-type-literal face over the real parse: an
    /// unnamed type-literal formal has no name to check (the official
    /// `p.name` nullability), so `void Function(int)` parameter lists stay
    /// silent — the gate's L-C rows (`lib/avoid_types_on_closure_parameters.
    /// dart` 14:18/19:18/24:22, `lib/avoid_final_parameters.dart` 20:25,
    /// including the `final`-recovery synthetic on the typedef line).
    #[test]
    fn non_constant_function_type_literal_unnamed_params_pass() {
        let f = Fixture::parse("void Function(int) cb = (int e) {};");
        assert!(f.run(&one_rule("non_constant_identifier_names")).is_empty());

        let f = Fixture::parse("void j(void Function(int) cb) {}");
        assert!(f.run(&one_rule("non_constant_identifier_names")).is_empty());

        let f = Fixture::parse("void Function(dynamic) dyn = (dynamic e) {};");
        assert!(f.run(&one_rule("non_constant_identifier_names")).is_empty());

        let f = Fixture::parse("typedef F = void Function(int inner);");
        assert!(f.run(&one_rule("non_constant_identifier_names")).is_empty());
    }

    /// A named parameter inside a function-type literal keeps the check:
    /// only the unnamed face is exempt, so a written misnamed formal
    /// reports at its name.
    #[test]
    fn non_constant_function_type_literal_named_param_reports() {
        let f = Fixture::parse("void Function(int Xo) handler = (int e) {};");
        let lints = f.run(&one_rule("non_constant_identifier_names"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The variable name 'Xo' isn't a lowerCamelCase identifier."
        );
    }

    /// The production `const` faces read the declaration keyword through
    /// the shared recovery (real parses carry only `late` in payload word
    /// 0): top-level, field, and local `const` declarations report in
    /// `constant_identifier_names` and stay out of
    /// `non_constant_identifier_names` — the gate's `lib/names.dart` 8:11
    /// face.
    #[test]
    fn const_declaration_faces_read_the_real_keyword() {
        let f = Fixture::parse("const int MAX_LEVEL = 9;");
        let lints = f.run(&one_rule("constant_identifier_names"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The constant name 'MAX_LEVEL' isn't a lowerCamelCase identifier."
        );
        assert!(f.run(&one_rule("non_constant_identifier_names")).is_empty());

        let f = Fixture::parse("class C {\n  static const MAX_LEVEL = 1;\n}");
        assert_eq!(f.run(&one_rule("constant_identifier_names")).len(), 1);

        let f = Fixture::parse("void main() {\n  const TIMEOUT = 3;\n}");
        assert_eq!(f.run(&one_rule("constant_identifier_names")).len(), 1);
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
                b.end(NodeKind::CompilationUnit, narrow_u32(t.len()));
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
                b.end(NodeKind::CompilationUnit, narrow_u32(t.len()));
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

    /// A `this.`-formal scanned before the matched parameter kills the
    /// positional conversion (`test_noLint_positionalThisParameter`; the
    /// j-scan inspects every parameter up to each match).
    #[test]
    fn field_formal_aborts() {
        let f = Fixture::parse(
            "class A {\n  A(int x, int y);\n}\n\nclass B extends A {\n  int x;\n  B(this.x, int y) \
             : super(x, y);\n}\n",
        );
        assert!(super_hits(&f).is_empty());
    }

    /// A `this.`-formal after the last matched parameter never enters the
    /// j-scan (it stops at each match), so the positional face still
    /// reports — `B(int x, this.z) : super(x)` reports `x`.
    #[test]
    fn field_formal_after_match_reports() {
        let f = Fixture::parse(
            "class A {\n  A(int x);\n}\n\nclass B extends A {\n  int z;\n  B(int x, this.z) : \
             super(x);\n}\n",
        );
        let lints = super_hits(&f);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Parameter 'x' could be a super parameter."
        );
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
            // The `{...}` group stamps `OptionalNamed` on the formal's kind
            // word (the production payload contract — `formal` pins the
            // positional face, so the row is spelled out here).
            b.begin(NodeKind::FormalParameter, 6);
            b.patch(0, NONE);
            b.patch(1, 7);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.patch(4, dartq_parser::FormalParameterKind::OptionalNamed as u32);
            b.leaf_span(NodeKind::Type, 6, 7);
            b.end(NodeKind::FormalParameter, 8);
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
            b.end(NodeKind::CompilationUnit, narrow_u32(t.len()));
        });
        assert!(super_hits(&f).is_empty());
    }

    /// The positional bare-identifier super-argument face over the real
    /// parse (the `lib/constructor_family.dart` `SuperGuest` shape):
    /// `B(int first, int second) : super(first, second);` with an
    /// unreferenced body reports both parameters on the class-name echo
    /// range, exactly the oracle's row.
    #[test]
    fn super_parameter_positional_real_parse_reports() {
        let f = Fixture::parse(
            "class A {\n  A(int first, int second);\n}\n\nclass B extends A {\n  B(int first, int \
             second) : super(first, second);\n}\n",
        );
        let lints = super_hits(&f);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Parameters ''first' and 'second'' could be super parameters."
        );
        // The report anchors on the constructor's `B` name token.
        assert_eq!(lints[0].range.start, utf16_at(&f, 18));
    }
}

/// The class-like owner's member rows in source order: the class /
/// extension-type [`NodeKind::DeclarationBody`] children, or the enum
/// [`NodeKind::EnumBody`] children after the constants' `;` (the official
/// `body.members` list carries only `ClassMember`s). The production tiling
/// also interleaves each member's [`NodeKind::MetadataStar`] carrier as a
/// body child — annotations belong to the member upstream, never to the
/// member list, so the star rows drop out here (annotation reads go through
/// [`crate::rules::member_annotations`] on the raw body children). The
/// enum constant rows drop too: the [`NodeKind::EnumElement`] handle is
/// childless (its name/argument rows tile as body siblings), so the kind
/// filter cannot see them — the position cut at the elements-end token
/// (the `;`, `EnumDeclaration` payload word 2) does.
pub(crate) fn class_like_members(c: &Check<'_>, owner: u32) -> Vec<u32> {
    let Some(body) = c
        .collect_direct(owner)
        .into_iter()
        .find(|&kid| matches!(c.kind(kid), NodeKind::DeclarationBody | NodeKind::EnumBody))
    else {
        return Vec::new();
    };
    // The enum constants' rows all close before the `;` — cut there. Without
    // members there is no `;` (the row carries the sentinel) and the cut is
    // inert; class-like owners carry no elements-end word at all (the cut
    // is enum-only).
    let cut = match c.kind(owner) {
        NodeKind::EnumDeclaration => {
            match c.field(owner, PayloadField::ElementsEnd) {
                Some(token) if token != crate::rules::NONE_TOKEN => {
                    utf16_token_span(c, token).end
                }
                _ => 0,
            }
        }
        _ => 0,
    };
    c.collect_direct(body)
        .into_iter()
        .filter(|&member| {
            !matches!(
                c.kind(member),
                NodeKind::EnumElement | NodeKind::MetadataStar
            )
        })
        .filter(|&member| cut == 0 || c.utf16_span(member).start >= cut)
        .collect()
}

/// The written secondary-constructor name (the
/// `MethodDeclarationContinuation` identifier), `None` for the unnamed
/// form. The class-name identifier carries a different context, so the
/// filter is exact.
fn constructor_name(c: &Check<'_>, ctor: u32) -> Option<u32> {
    let ctx = dartq_parser::IdentifierContext::MethodDeclarationContinuation as u32;
    c.collect_direct(ctor)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::Identifier && c.field(kid, PayloadField::Context) == Some(ctx))
}

/// No written name (the unnamed form, canonically `new`) or spelled
/// `new` — the official element-name `== 'new'` test
/// (`constructorDeclaration.name.lexeme == 'new'` through the element).
fn is_unnamed_or_new(c: &Check<'_>, ctor: u32) -> bool {
    match constructor_name(c, ctor) {
        None => true,
        Some(name) => c.lexeme(c.head_token(name)) == "new",
    }
}

pub(crate) mod sort_unnamed_constructors_first {
    use super::{Check, NodeKind, RuleSpec};

    /// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/sort_unnamed_constructors_first.dart`
    /// — an unnamed constructor (element name `new`) that follows a named
    /// constructor in a class, enum, or extension-type body reports
    /// (`_Visitor.check`: a named constructor arms the flag;
    /// non-constructor members neither arm nor clear it). Factories carry
    /// the same constructor element shape, so an unnamed factory
    /// arms/reports identically. Message: `diagnostic.g.dart`
    /// `sortUnnamedConstructorsFirst`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "sort_unnamed_constructors_first",
    &[
            NodeKind::ClassDeclaration,
            NodeKind::EnumDeclaration,
            NodeKind::ExtensionTypeDeclaration,
        ],
    check,
);

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let mut seen_named = false;
        let mut hits: Vec<u32> = Vec::new();
        for member in super::class_like_members(c, node) {
            if matches!(c.kind(member), NodeKind::Constructor | NodeKind::Factory) {
                if super::is_unnamed_or_new(c, member) {
                    if seen_named {
                        hits.push(member);
                    }
                } else {
                    // Only a named constructor arms the flag; non-constructor
                    // members neither arm nor clear it.
                    seen_named = true;
                }
            }
        }
        for member in hits {
            let range = c.utf16_span(member);
            c.report(range, &[]);
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::testsupport::{Fixture, one_rule};

        /// `test/rules/sort_unnamed_constructors_first_test.dart`
        /// `test_unnamedConstructorAfterNamed`.
        #[test]
        fn reports_an_unnamed_constructor_after_a_named_one() {
            let f = Fixture::parse("class A { A.named(); A(); }");
            let lints = f.run(&one_rule("sort_unnamed_constructors_first"));
            assert_eq!(lints.len(), 1);
            assert_eq!(
                lints[0].message,
                "Invalid location for the unnamed constructor."
            );
        }

        #[test]
        fn ignores_a_sorted_class() {
            let f = Fixture::parse("class A { A(); A.named(); }");
            assert!(
                f.run(&one_rule("sort_unnamed_constructors_first"))
                    .is_empty()
            );
        }

        /// An unnamed factory carries the `new` element name and reports
        /// like the unnamed constructor.
        #[test]
        fn reports_an_unnamed_factory_after_a_named_one() {
            let f = Fixture::parse("class A { A.named(); factory A() => A.named(); }");
            let lints = f.run(&one_rule("sort_unnamed_constructors_first"));
            assert_eq!(lints.len(), 1);
        }

        /// `test_enum` face: constants are not members; the member walk
        /// starts after them.
        #[test]
        fn reports_in_an_enum_body() {
            let f = Fixture::parse("enum E { a; E.named(); E(); }");
            let lints = f.run(&one_rule("sort_unnamed_constructors_first"));
            assert_eq!(lints.len(), 1);
        }

        #[test]
        fn ignores_a_class_without_constructors() {
            let f = Fixture::parse("class A { int x = 0; void m() {} }");
            assert!(
                f.run(&one_rule("sort_unnamed_constructors_first"))
                    .is_empty()
            );
        }

        /// Non-constructor members do not arm the flag: an unnamed
        /// constructor after a field or method stays silent — only the
        /// constructor-to-constructor order is judged.
        #[test]
        fn ignores_an_unnamed_constructor_after_a_non_constructor_member() {
            let f = Fixture::parse("class A { int x = 0; void m() {} A(); }");
            assert!(
                f.run(&one_rule("sort_unnamed_constructors_first"))
                    .is_empty()
            );
        }
    }
}

pub(crate) mod sort_constructors_first {
    use super::{Check, NodeKind, RuleSpec};

    /// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/sort_constructors_first.dart`
    /// — a constructor that follows a non-constructor member in a class,
    /// enum, or extension-type body reports (`_Visitor.check`: the first
    /// non-constructor member arms the flag; every constructor past it
    /// reports). Factories are constructor declarations in the official
    /// member list and join the family. The official's
    /// `PrimaryConstructorBody` arm (`reportAtToken(member.thisKeyword)`)
    /// is inert here: a primary body is part of the declaration header, not
    /// a body member, so it never appears in the member walk. Message:
    /// `diagnostic.g.dart` `sortConstructorsFirst`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "sort_constructors_first",
    &[
            NodeKind::ClassDeclaration,
            NodeKind::EnumDeclaration,
            NodeKind::ExtensionTypeDeclaration,
        ],
    check,
);

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let mut other = false;
        let mut hits: Vec<u32> = Vec::new();
        for member in super::class_like_members(c, node) {
            if matches!(c.kind(member), NodeKind::Constructor | NodeKind::Factory) {
                if other {
                    hits.push(member);
                }
            } else {
                other = true;
            }
        }
        for member in hits {
            let range = c.utf16_span(member);
            c.report(range, &[]);
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::testsupport::{Fixture, one_rule};

        /// `test/rules/sort_constructors_first_test.dart` `test_constructor`.
        #[test]
        fn reports_a_constructor_after_a_field() {
            let f = Fixture::parse("class A { int x = 0; A(); }");
            let lints = f.run(&one_rule("sort_constructors_first"));
            assert_eq!(lints.len(), 1);
            assert_eq!(
                lints[0].message,
                "Constructor declarations should be before non-constructor declarations."
            );
        }

        #[test]
        fn ignores_a_sorted_class() {
            let f = Fixture::parse("class A { A(); int x = 0; }");
            assert!(f.run(&one_rule("sort_constructors_first")).is_empty());
        }

        /// A factory is a constructor declaration in the official member
        /// list: it neither arms the flag nor reports once armed.
        #[test]
        fn treats_a_factory_as_a_constructor() {
            let f = Fixture::parse("class A { A.n(); factory A() => A.n(); int x = 0; }");
            assert!(f.run(&one_rule("sort_constructors_first")).is_empty());
        }

        /// The enum's constants are not `body.members` upstream — the
        /// first body constructor after the `;` is the member list's head
        /// and stays silent; only the constructor past a non-constructor
        /// member reports.
        #[test]
        fn reports_in_an_enum_body() {
            let f = Fixture::parse("enum E { a; E(); int x = 0; E.named(); }");
            let lints = f.run(&one_rule("sort_constructors_first"));
            assert_eq!(lints.len(), 1, "{lints:?}");
        }

        /// The corpus shape (`lib/unnecessary_const_in_enum_constructor.dart`
        /// `E`): constants only before the `;` — the first constructor never
        /// reports on them.
        #[test]
        fn enum_constants_do_not_arm_the_flag() {
            let f = Fixture::parse("enum E {\n  a;\n  const E();\n}");
            assert!(f.run(&one_rule("sort_constructors_first")).is_empty());
        }
    }
}

pub(crate) mod matching_super_parameters {
    use super::{PayloadField, Check, NodeKind, RuleSpec};
    use crate::rules::expressions::query_site;
    use crate::rules::types::leak_formal_rows;
    use crate::rules::{NONE_TOKEN, body_members};

    /// Official: `upstream Dart SDK source,
    /// pkg/linter/lib/src/rules/matching_super_parameters.dart` — a
    /// positional super formal whose name differs from the super
    /// constructor's positional formal at the same index reports at the
    /// formal (`reportAtNode(superParameter)`, :89-100), the names as
    /// `[ours, super's]`. Only positional super formals participate
    /// (:55-64); the super constructor comes from an explicit
    /// `super(...)`/`super.named(...)` invocation's resolved element, else
    /// the supertype's unnamed constructor (:65-79); more positional
    /// formals passed than the super has is an error face and stays
    /// silent (:84-88).
    ///
    /// Approximation (documented): the resolved element face is the
    /// semantic query — the extends-clause type position resolves the
    /// container, the invocation's selector names the constructor (a bare
    /// `super(...)` and the no-initializer fallback both resolve the
    /// unnamed constructor). A semantic-free plain run stays silent (the
    /// resolved face is the rule's only channel), and the invocation face
    /// keys on the written selector, not the resolved invocation element.
    /// Enum and extension-type containers carry no extends type and stay
    /// silent — upstream's fallback only answers for `ClassDeclaration`
    /// (:69-78). Message: `diagnostic.g.dart` `matchingSuperParameters` —
    /// the pinned oracle (Dart 3.13.1) renders the name slot with a doubled
    /// closing apostrophe (`named 'y''`), the shipped 3.13 template's
    /// quirk since normalized in main's `#p0` scheme (the vendored main
    /// template carries single quotes); mirrored verbatim, the gate
    /// oracle is 3.13.1.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "matching_super_parameters",
    &[NodeKind::Constructor, NodeKind::PrimaryConstructor],
    check,
);

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // The formal list owner: the declaration row's `(...)` child (the
        // secondary constructor's parameter list, or the primary row's
        // header formals).
        let Some(parameters) = c
            .collect_direct(node)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
        else {
            return;
        };
        // The positional super formals (:55-64) — the `super` receiver
        // keyword and a positional kind discriminant.
        let ours: Vec<u32> = leak_formal_rows(c, parameters)
            .into_iter()
            .filter(|&param| {
                c.field(param, PayloadField::Receiver)
                    .is_some_and(|kw| kw != NONE_TOKEN && c.lexeme(kw) == "super")
                    && c
                        .field(param, PayloadField::FormalParameterKind)
                        .is_some_and(|kind| matches!(kind, 0 | 3))
            })
            .collect();
        if ours.is_empty() {
            return;
        }
        let Some(class_row) = c.path.iter().rev().skip(1).copied().find(|&row| {
            matches!(
                c.kind(row),
                NodeKind::ClassDeclaration | NodeKind::EnumDeclaration
            )
        }) else {
            return;
        };
        // The initializer list: the constructor's own, or the paired
        // primary-constructor body's (:41-48).
        let initializers = match c.kind(node) {
            NodeKind::Constructor => c
                .collect_direct(node)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::Initializers),
            _ => body_members(c, class_row)
                .into_iter()
                .find(|&member| c.kind(member) == NodeKind::PrimaryConstructorBody)
                .and_then(|body| {
                    c.collect_direct(body)
                        .into_iter()
                        .find(|&kid| c.kind(kid) == NodeKind::Initializers)
                }),
        };
        // The constructor selector: an explicit super invocation's name,
        // else the unnamed constructor (the upstream `superInvocation
        // ?.element`, else `supertype.unnamedConstructor`, :52-64).
        let selector = initializers.and_then(|rows| super_invocation_selector(c, rows));
        let Some(analysis) = c.semantics() else {
            return;
        };
        let Some(type_row) = extends_type_row(c, class_row) else {
            return;
        };
        let selector_text = selector;
        let selector = selector_text.map(|name| analysis.strings().intern(name));
        let Some(binding) = analysis.resolve_constructor(query_site(c, type_row), selector) else {
            return;
        };
        // The super constructor's positional formal names. The element
        // table is the primary face (it resolves cross-file and across the
        // in-body + unnamed-primary spellings). When the table can't serve
        // the written face — a `!exists` selector hit, or a resolved ctor
        // whose `ctor_formals` entry never tabulated — the face is the
        // primary constructor (`class C.named(...)`/`class C(...)` register
        // no `ctor_named`/`ctor_formals` entry): the fallback reads the
        // selector-matched primary header's own formal list off the
        // same-file declaration, names verbatim. Anything the fallback
        // cannot see (non-existent ctor, non-same-file container) stays
        // silent (under-report).
        let super_positionals: Vec<String> = match binding.exists.then(|| binding.formals).flatten()
        {
            Some(formals) => formals
                .positional
                .iter()
                .map(|formal| analysis.strings().resolve(formal.name).to_string())
                .collect(),
            None => {
                let Some(names) = primary_super_positionals(
                    c,
                    analysis.strings().resolve(binding.container),
                    selector_text,
                ) else {
                    return;
                };
                names
            }
        };
        // More positional formals passed to super than it has — an error
        // upstream too (:84-88).
        if super_positionals.len() < ours.len() {
            return;
        }
        for (index, &param) in ours.iter().enumerate() {
            let Some(name_token) = c.field(param, PayloadField::Name) else {
                continue;
            };
            if name_token == NONE_TOKEN {
                continue;
            }
            let our_name = c.lexeme(name_token);
            let Some(super_name) = super_positionals.get(index) else {
                continue;
            };
            if our_name != super_name {
                c.report(c.utf16_span(param), &[our_name, super_name]);
            }
        }
    }

    /// The positional formal names of `container`'s primary constructor,
    /// read off the same-file `ClassDeclaration`'s `PrimaryConstructor`
    /// child — the face the element table's `ctor_formals` omits. `selector`
    /// selects the written ctor name: `None` wants the unnamed primary face
    /// (`class C(...)`), `Some(n)` the `class C.n(...)` head. `None`
    /// under-reports: the container is not same-file, has no matching
    /// primary ctor, or names a face the table already served.
    fn primary_super_positionals(
        c: &Check<'_>,
        container: &str,
        selector: Option<&str>,
    ) -> Option<Vec<String>> {
        let class_row = crate::rules::same_file_declaration(c, container)?;
        let primary =
            c.children(class_row).find(|&kid| {
                if c.kind(kid) != NodeKind::PrimaryConstructor {
                    return false;
                }
                // `HasConstructorName` decides named vs unnamed; the
                // selector itself is the `Identifier` child with the
                // PrimaryConstructorDeclaration context.
                let named =
                    c.field(kid, PayloadField::HasConstructorName) == Some(1);
                match selector {
                    None => !named,
                    Some(want) => named && c.children(kid).any(|name_row| {
                        c.kind(name_row) == NodeKind::Identifier
                            && c.field(name_row, PayloadField::Context)
                                == Some(
                                    dartq_parser::IdentifierContext::PrimaryConstructorDeclaration
                                        as u32,
                                )
                            && c.lexeme(c.head_token(name_row)) == want
                    }),
                }
            })?;
        let params = c
            .collect_direct(primary)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)?;
        Some(
            leak_formal_rows(c, params)
                .into_iter()
                .filter(|&param| {
                    matches!(
                        c.field(param, PayloadField::FormalParameterKind),
                        Some(0 | 3)
                    )
                })
                .filter_map(|param| {
                    let token = c.field(param, PayloadField::Name)?;
                    (token != NONE_TOKEN).then(|| c.lexeme(token).to_string())
                })
                .collect(),
        )
    }

    /// The selector name of the initializers' first super invocation: the
    /// token after the `super` keyword names it through `.`/`?.`; a bare
    /// `super(...)` answers `None`.
    pub(super) fn super_invocation_selector<'a>(
        c: &'a Check<'_>,
        initializers: u32,
    ) -> Option<&'a str> {
        let super_row = c
            .children(initializers)
            .find(|&row| c.kind(row) == NodeKind::SuperExpression)?;
        let super_token = c.head_token(super_row);
        match c.lexeme(super_token + 1) {
            "." | "?." => Some(c.lexeme(super_token + 2)),
            _ => None,
        }
    }

    /// The extends-clause type row of the class-like: the first `Type`
    /// child whose header scan reaches an `extends` keyword (`with`/`on`/
    /// `implements` types are not the superclass).
    pub(super) fn extends_type_row(c: &Check<'_>, class_row: u32) -> Option<u32> {
        let start = c.head_token(class_row);
        c.collect_direct(class_row)
            .into_iter()
            .filter(|&kid| c.kind(kid) == NodeKind::Type)
            .find(|&kid| {
                let mut token = c.head_token(kid);
                while token > start {
                    token -= 1;
                    match c.lexeme(token) {
                        "extends" => return true,
                        "with" | "on" | "implements" => return false,
                        _ => {}
                    }
                }
                false
            })
    }

    #[cfg(test)]
    mod tests {
        use dartq_parser::ParserFeatures;
        use dartq_resolution::{LibraryClassModifiers, LibraryGraph, LibraryGraphBuilder, Namespaces};
        use dartq_semantic::{ElementIndex, MicroKind, StrictModes, analyze_unit, collect};
        use dartq_source::{FileTable, StringTable};

        use crate::engine::Lint;
        use crate::testsupport::{Fixture, one_rule};

        use super::SPEC;

        /// Runs the rule over `source` with the semantic face mounted: the
        /// source folded as the sole analyzed file of `package:t` — the
        /// driver's per-file build shape. The context tables leak
        /// (`'static`); the tests are one-shot processes.
        fn run_semantic(source: &str) -> Vec<Lint> {
            let fixture = Fixture::parse(source);
            let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
            let mut files = FileTable::new();
            let file = files.intern(std::path::Path::new("lint_semantic.dart"));
            let features =
                ParserFeatures::from_feature_set(dartq_language::FeatureSet::for_language_version(
                    dartq_language::LanguageVersion::new(3, 15),
                ));
            let scanned = dartq_scanner::scan(source, crate::testsupport::CONFIG, file);
            assert!(
                scanned.diagnostics.is_empty(),
                "scanner diagnostics: {source:?}"
            );
            let elements = collect(source, &scanned.tokens, features, strings);
            let mut builder = LibraryGraphBuilder::new(strings);
            let lib = builder.intern("package:t/lint_semantic.dart");
            for (name, declaration) in elements.declarations() {
                if declaration.kind == MicroKind::TypeParam {
                    continue;
                }
                builder.declare(lib, name, file);
            }
            let mut index = ElementIndex::default();
            index.insert(file, elements);
            let graph: &'static LibraryGraph = Box::leak(Box::new(builder.finish()));
            let namespaces: &'static Namespaces = Box::leak(Box::new(Namespaces::build(graph)));
            let index: &'static ElementIndex = Box::leak(Box::new(index));
            let analysis = analyze_unit(
                source,
                &scanned.tokens,
                features,
                strings,
                file,
                lib,
                graph,
                namespaces,
                index,
                StrictModes::default(),
                &LibraryClassModifiers::default(),
            );
            let unit = crate::engine::Unit {
                source: &fixture.source,
                tokens: &fixture.tokens,
                ast: &fixture.ast,
                lines: &fixture.lines,
                path: Some(&fixture.path),
                features: fixture.features,
                version: fixture.version,
                workspace: None,
                reachability: None,
                async_context_uses: None,
                library_type_uses: None,
            };
            unit.run_with_semantics(
                &one_rule(SPEC.name),
                &rustc_hash::FxHashSet::default(),
                Some(&analysis),
            )
        }

        /// A positional super formal renamed against the super
        /// constructor's formal reports with both names.
        #[test]
        fn renamed_positional_super_formal_reports() {
            let source = "class A {\n  A(int xyz);\n}\n\nclass B extends A {\n  B(super.abc);\n}\n";
            let lints = run_semantic(source);
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "The super parameter named 'abc'' does not share the same name as the corresponding parameter in the super constructor, 'xyz'."
            );
        }

        /// A matching name, a named super formal, and a positional-count
        /// error all stay silent.
        #[test]
        fn matching_and_non_faces_stay_silent() {
            let source = "class A {\n  A(int xyz);\n}\n\nclass B extends A {\n  B(super.xyz);\n}\n";
            assert!(run_semantic(source).is_empty());

            let source = "class A {\n  A(int x, {int y});\n}\n\nclass B extends A {\n  B(super.x, {super.y});\n}\n";
            assert!(run_semantic(source).is_empty());

            let source = "class A {\n  A();\n}\n\nclass B extends A {\n  B(super.x);\n}\n";
            assert!(run_semantic(source).is_empty());
        }

        /// The explicit-invocation face: `super.named()` selects the named
        /// constructor's formals; a no-initializer constructor falls back
        /// to the unnamed super constructor.
        #[test]
        fn invocation_selector_and_unnamed_fallback() {
            let source = "class A {\n  A.named(int xyz);\n}\n\nclass B extends A {\n  B(super.abc) : super.named();\n}\n";
            let lints = run_semantic(source);
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "The super parameter named 'abc'' does not share the same name as the corresponding parameter in the super constructor, 'xyz'."
            );

            let source = "class A {\n  A(int xyz);\n}\n\nclass B extends A {\n  B(super.abc);\n}\n";
            assert_eq!(run_semantic(source).len(), 1);
        }

        /// The primary-constructor face: `super.`-formals in the class
        /// header pair against the extends type's constructor (upstream
        /// `test_primaryToInBody_implicitInvocation_*` — only
        /// `SuperFormalParameter`s participate; a plain header formal is
        /// not a super parameter and stays silent).
        #[test]
        fn primary_constructor_face() {
            let source = "class A {\n  A(int xyz);\n}\n\nclass B(super.abc) extends A {}\n";
            let lints = run_semantic(source);
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert!(source.contains("super.abc"));
        }

        /// No extends clause, or a plain semantic-free run (the wave
        /// posture), stays silent.
        #[test]
        fn no_extends_and_plain_run_stay_silent() {
            let source = "class B {\n  B(super.x);\n}\n";
            assert!(run_semantic(source).is_empty());

            let source = "class A {\n  A(int xyz);\n}\n\nclass B extends A {\n  B(super.abc);\n}\n";
            let f = Fixture::parse(source);
            assert!(f.run(&one_rule(SPEC.name)).is_empty());
        }

        /// The corpus faces over all three miss shapes — explicit
        /// `super.named()` invocation, implicit in-body invocation, and the
        /// primary-constructor header + `this :` body. Exercises the
        /// same-file primary-constructor fallback (the element table omits
        /// `ctor_formals` for `class C.named(...)` and `class C(...)`
        /// primary spellings).
        #[test]
        fn corpus_faces_report() {
            // Explicit invocation: D4/C4 (`: super.named()`).
            let source = "class D4 extends C4 {\n  final String w;\n\n  D4(\n    this.w,\n    super.y,\n    super.x,\n  ) : super.named();\n}\n\nclass C4 {\n  final int x;\n  final int y;\n\n  C4.named(this.x, this.y);\n}\n";
            let lints = run_semantic(source);
            assert_eq!(lints.len(), 2, "D4: {lints:?}");

            // Implicit invocation: D7/C7 (no initializer list).
            let source = "class D7 extends C7 {\n  final String w;\n\n  D7(\n    this.w,\n    super.y,\n    super.x,\n  );\n}\n\nclass C7 {\n  final int x;\n  final int y;\n\n  C7(this.x, this.y);\n}\n";
            let lints = run_semantic(source);
            assert_eq!(lints.len(), 2, "D7: {lints:?}");

            // Omitted-optional implicit: D8/C8 (super has `[this.y]`).
            let source = "class D8 extends C8 {\n  final String w;\n\n  D8(\n    this.w,\n    super.y,\n  );\n}\n\nclass C8 {\n  final int x;\n  final int? y;\n\n  C8(this.x, [this.y]);\n}\n";
            let lints = run_semantic(source);
            assert_eq!(lints.len(), 1, "D8: {lints:?}");

            // Primary ctor header to in-body super, explicit invocation.
            let source = "class D23(this.w, super.y, super.x) extends C23 {\n  final String w;\n\n  this : super.named();\n}\n\nclass C23 {\n  final int x;\n  final int y;\n\n  C23.named(this.x, this.y);\n}\n";
            let lints = run_semantic(source);
            assert_eq!(lints.len(), 2, "D23: {lints:?}");

            // Primary ctor header, implicit invocation.
            let source = "class D26(this.w, super.y, super.x) extends C26 {\n  final String w;\n}\n\nclass C26 {\n  final int x;\n  final int y;\n\n  C26(this.x, this.y);\n}\n";
            let lints = run_semantic(source);
            assert_eq!(lints.len(), 2, "D26: {lints:?}");

            // Primary ctor header, omitted-optional.
            let source = "class D27(this.w, super.y) extends C27 {\n  final String w;\n}\n\nclass C27 {\n  final int x;\n  final int? y;\n\n  C27(this.x, [this.y]);\n}\n";
            let lints = run_semantic(source);
            assert_eq!(lints.len(), 1, "D27: {lints:?}");

            // Primary ctor header to primary super, explicit.
            let source = "class D32(this.w, super.y, super.x) extends C32 {\n  final String w;\n\n  this : super.named();\n}\n\nclass C32.named(this.x, this.y) {\n  final int x;\n  final int y;\n}\n";
            let lints = run_semantic(source);
            assert_eq!(lints.len(), 2, "D32: {lints:?}");

            // Primary ctor header to primary super, implicit.
            let source = "class D35(this.w, super.y, super.x) extends C35 {\n  final String w;\n}\n\nclass C35(this.x, this.y) {\n  final int x;\n  final int y;\n}\n";
            let lints = run_semantic(source);
            assert_eq!(lints.len(), 2, "D35: {lints:?}");

            // Primary ctor header to primary super, omitted-optional.
            let source = "class D36(this.w, super.y) extends C36 {\n  final String w;\n}\n\nclass C36(this.x, [this.y]) {\n  final int x;\n  final int? y;\n}\n";
            let lints = run_semantic(source);
            assert_eq!(lints.len(), 1, "D36: {lints:?}");
        }
    }
}
