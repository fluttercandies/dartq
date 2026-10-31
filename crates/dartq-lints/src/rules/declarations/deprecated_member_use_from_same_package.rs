use super::{
    Check, NodeKind, PayloadField, RuleSpec, container_members, is_deprecated_metadata, narrow_u32,
    top_level_annotations,
};
use crate::rules::initialized_identifier_name;
use std::path::{Path, PathBuf};

/// Official: `pkg/linter/lib/src/rules/deprecated_member_use_from_same_package.dart`
/// — a use of a deprecated element from inside the declaring package
/// reports at the use. The official mount is one
/// `ElementUsageFrontierVisitor` per compilation unit (:102-113): every
/// declaration is pushed onto the frontier stack while its contents are
/// visited (`element_usage_frontier_visitor.dart`), so a use inside a
/// deprecated ancestor — the deprecated `library` directive, a
/// deprecated class/member/parameter — never reports (the
/// `test_deprecatedClass_usedInDeprecated*` family). Two message
/// variants under one name: the normalized `@Deprecated` tag with the
/// message, a bare `@deprecated` without.
///
/// The official usage faces are element-resolved
/// (`element_usage_detector.dart`): a `NamedType` reports the type
/// element at its name, a `SimpleIdentifier` the resolved element at the
/// identifier (skipping declaration contexts, `ConstructorName` and
/// `SuperConstructorInvocation` name positions, and `HideCombinator`
/// names), a `ConstructorName` the constructor element at the whole
/// `C.named`, the invocation handlers the argument-to-parameter mapping
/// (`_visitParametersAndArguments`: a named argument at its label, a
/// positional one at the argument), the operator handlers the whole
/// expression (`c + 1`, `c[1]`, `c += 1`), and the prefix/postfix
/// handlers the operand.
///
/// This implementation reads the same faces off the single-unit arena,
/// with the same-file declaration table standing in for element
/// resolution: a use is matched to a deprecated declaration of the same
/// name *and the same shape*. A bare name reaches top-level declarations
/// and the enclosing container's own members; a selector name reaches
/// members and enum constants; a constructor name reaches constructor
/// declarations; a call-site argument reaches the matching parameter.
/// The faces the arena cannot resolve stay silent (documented
/// residuals): cross-file same-package uses through other names, the
/// implicit-super and `super()`/`this()` constructor faces (no arena
/// node), the implicit `call` method of a function-typed receiver, and
/// the inferred-name object-pattern shorthand.
///
/// Messages: `diagnostic.g.dart`
/// `deprecatedMemberUseFromSamePackageWithMessage` and
/// `…WithoutMessage`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "deprecated_member_use_from_same_package",
    &[NodeKind::CompilationUnit],
    check,
);

const WITH_MESSAGE: &str = "'{0}' is deprecated and shouldn't be used. {1}";
const WITHOUT_MESSAGE: &str = "'{0}' is deprecated and shouldn't be used.";
const CORRECTION: &str = "Try replacing the use of the deprecated member with the replacement, \
         if a replacement is specified.";

/// What a deprecated declaration denotes — the shape gate that decides
/// which use site can name it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A class-like, typedef, extension type or extension: named by a type
    /// reference (and by a bare identifier, the extension-override face).
    Type,
    /// A top-level function, variable, getter or setter: named by a bare
    /// reference-context identifier.
    TopLevel,
    /// A member of a container: named by a selector (`a.f`, `super.f`) or,
    /// unqualified, only inside the declaring container
    /// (`test_deprecatedMethod`'s `m()`).
    Member,
    /// An enum constant: named by a selector on the enum type (`E.two`).
    EnumValue,
    /// A constructor: named by its type name (`C()`, `C.named()`).
    Constructor,
    /// A formal parameter: named by a call-site argument only (the
    /// declaring body's own uses are the upstream `_isLocalParameter`
    /// exemption).
    Parameter,
}

/// The element a reference reads — the upstream `readElement`/`writeElement`
/// split (`test_deprecatedGetter_usedInAssignment` is silent: a plain
/// assignment reads the setter, which the getter's `@deprecated` does not
/// cover).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Access {
    Method,
    Getter,
    Setter,
    Variable,
    Operator,
}

impl Access {
    fn readable(self) -> bool {
        matches!(self, Access::Method | Access::Getter | Access::Variable)
    }

    fn writable(self) -> bool {
        matches!(self, Access::Setter | Access::Variable)
    }
}

/// One same-file deprecated declaration: the name lexeme (the use-site
/// match key), the named-constructor selector (`C.named` — `None` for every
/// other shape), the normalized message (`None` = the bare `@deprecated`
/// shape), and the shape/owner the use gate reads.
struct DeprecatedDecl<'a> {
    name: &'a str,
    selector: Option<&'a str>,
    message: Option<String>,
    kind: Kind,
    access: Access,
    /// `Member`: the declaring container row (the unqualified-use window);
    /// `Parameter`: the declaring function/constructor row (the argument
    /// mapping's order anchor). `0` where the shape has no owner.
    owner: u32,
    /// `Parameter`: the declaring executable's name, the call-site callee
    /// key. Empty elsewhere.
    owner_name: &'a str,
    /// `Parameter`: written in a `{…}` named group.
    named: bool,
}

fn check(c: &mut Check<'_>) {
    // The frontier's root push (visitor `visitCompilationUnit`): a
    // deprecated `library` directive silences the whole unit.
    if unit_library_deprecated(c) {
        return;
    }
    // The directive faces — import/export of a deprecated library and
    // the `show` combinator names (workspace-gated: the URI resolution
    // and the `package:` display name are package facts).
    directive_faces(c);
    let decls = collect_deprecated(c);
    if decls.is_empty() {
        return;
    }
    let len = narrow_u32(c.unit.ast.len());
    let mut suppressed: Vec<core::ops::Range<usize>> = Vec::new();
    let mut hits: Vec<Hit> = Vec::new();
    for row in 0..len {
        if c.kind(row) == NodeKind::Identifier {
            identifier_face(c, row, &decls, &mut hits);
        } else if is_frontier_declaration(c.kind(row)) && frontier_deprecated(c, row) {
            suppressed.push(frontier_span(c, row));
        }
    }
    operator_faces(c, &decls, &mut hits);
    argument_faces(c, &decls, &mut hits);
    for hit in hits {
        if suppressed
            .iter()
            .any(|span| span.start <= hit.byte && hit.byte < span.end)
        {
            continue;
        }
        report(c, &hit, &decls[hit.decl]);
    }
}

/// One use site: the byte start (the frontier-suppression window's
/// coordinate), the reported UTF-16 range, the declaration it reads, and
/// the optional display override (a named constructor's `C.named`).
struct Hit {
    byte: usize,
    range: core::ops::Range<u32>,
    decl: usize,
    display: Option<String>,
}

impl Hit {
    fn new(byte: usize, range: core::ops::Range<u32>, decl: usize) -> Self {
        Self {
            byte,
            range,
            decl,
            display: None,
        }
    }
}

/// Emits one hit in the declaration's message variant.
fn report(c: &mut Check<'_>, hit: &Hit, decl: &DeprecatedDecl<'_>) {
    let name: &str = hit.display.as_deref().unwrap_or(decl.name);
    match &decl.message {
        Some(message) => {
            c.report_texts(hit.range.clone(), WITH_MESSAGE, CORRECTION, &[name, message]);
        }
        None => c.report_texts(hit.range.clone(), WITHOUT_MESSAGE, CORRECTION, &[name]),
    }
}

// —— use faces —————————————————————————————————————————————————————————

/// The identifier faces: a reference-context identifier, a named-argument
/// label, and an object-pattern field label.
fn identifier_face(
    c: &Check<'_>,
    row: u32,
    decls: &[DeprecatedDecl<'_>],
    hits: &mut Vec<Hit>,
) {
    let Some(context) = c.field(row, PayloadField::Context) else {
        return;
    };
    let lexeme = c.lexeme(c.head_token(row));
    let byte = c.byte_span(row).start;
    if context == dartq_parser::IdentifierContext::PatternFieldName as u32 {
        // An object-pattern field label reads the field
        // (`PatternFieldImpl` → the field's name).
        let base = c.utf16_span(row);
        for (index, decl) in decls.iter().enumerate() {
            if decl.kind == Kind::Member && decl.name == lexeme && decl.access.readable() {
                hits.push(Hit::new(byte, base.clone(), index));
            }
        }
        return;
    }
    if !is_reference_context(context) {
        return;
    }
    let base = c.utf16_span(row);
    let lhs = assignment_lhs(c, row);
    if let Some(receiver) = selector_left(c, row) {
        // A selector use (`a.f`, `super.f`, `C.named`): the dot chain names
        // a member, an enum constant, or a named constructor.
        if let Some(receiver) = receiver {
            let receiver_name = c.lexeme(c.head_token(receiver));
            for (index, decl) in decls.iter().enumerate() {
                if decl.kind == Kind::Constructor
                    && decl.name == receiver_name
                    && decl.selector == Some(lexeme)
                {
                    let mut hit = Hit::new(byte, c.utf16_span(receiver).start..base.end, index);
                    hit.display = Some(format!("{receiver_name}.{lexeme}"));
                    hits.push(hit);
                }
            }
        }
        for (index, decl) in decls.iter().enumerate() {
            if decl.name != lexeme {
                continue;
            }
            match decl.kind {
                Kind::Member | Kind::EnumValue => {
                    if decl.kind == Kind::Member && !access_ok(decl, lhs) {
                        continue;
                    }
                    let range = increment_operand_span(c, row).unwrap_or(base.clone());
                    hits.push(Hit::new(byte, range, index));
                }
                Kind::Type => hits.push(Hit::new(byte, base.clone(), index)),
                _ => {}
            }
        }
        return;
    }
    for (index, decl) in decls.iter().enumerate() {
        if decl.name != lexeme {
            continue;
        }
        match decl.kind {
            Kind::Type => hits.push(Hit::new(byte, base.clone(), index)),
            Kind::TopLevel => {
                if !access_ok(decl, lhs) {
                    continue;
                }
                hits.push(Hit::new(byte, base.clone(), index));
            }
            Kind::Member => {
                // An unqualified member name reads the enclosing
                // container's member (the official implicit-`this` face).
                if decl.owner == 0 || !inside(c, byte, decl.owner) {
                    continue;
                }
                if !access_ok(decl, lhs) {
                    continue;
                }
                let range = increment_operand_span(c, row).unwrap_or(base.clone());
                hits.push(Hit::new(byte, range, index));
            }
            Kind::Constructor if decl.selector.is_none() => {
                // `C()` / `C(…)`: a constructor invocation reads the
                // unnamed constructor; a bare type annotation does not.
                if followed_by_arguments(c, row) {
                    hits.push(Hit::new(byte, base.clone(), index));
                }
            }
            _ => {}
        }
    }
}

/// The call-site argument faces: a positional argument reads the parameter
/// at its index (`_visitParametersAndArguments`).
fn argument_faces(c: &Check<'_>, decls: &[DeprecatedDecl<'_>], hits: &mut Vec<Hit>) {
    for row in 0..narrow_u32(c.unit.ast.len()) {
        if c.kind(row) == NodeKind::NamedArgument {
            // A named argument reads the parameter of the same name
            // (`_visitParametersAndArguments`); the label is the report
            // span (the zero-width marker carries the colon, the label is
            // the token before it).
            let Some(colon) = c.field(row, PayloadField::Colon) else {
                continue;
            };
            let Some(label) = colon.checked_sub(1) else {
                continue;
            };
            let lexeme = c.lexeme(label);
            let Some(offset) = c.unit.tokens.get(label as usize).map(|token| token.byte_offset)
            else {
                continue;
            };
            let range = crate::rules::utf16_token_span(c, label);
            for (index, decl) in decls.iter().enumerate() {
                if decl.kind == Kind::Parameter && decl.named && decl.name == lexeme {
                    hits.push(Hit::new(offset as usize, range.clone(), index));
                }
            }
            continue;
        }
        if c.kind(row) != NodeKind::Arguments {
            continue;
        }
        let Some(callee) = callee_name(c, row) else {
            continue;
        };
        // A named argument's value follows its zero-width `NamedArgument`
        // marker; both are skipped so the remaining rows are the
        // positional arguments in order.
        let positional_rows: Vec<u32> = c
            .collect_direct(row)
            .into_iter()
            .filter(|&kid| {
                c.kind(kid) != NodeKind::NamedArgument
                    && !(c.kind(kid) == NodeKind::Identifier
                        && c.field(kid, PayloadField::Context)
                            == Some(
                                dartq_parser::IdentifierContext::NamedArgumentReference as u32,
                            ))
            })
            .collect();
        let owner = decls.iter().find(|decl| {
            decl.kind == Kind::Parameter && !decl.named && decl.owner_name == callee
        });
        let Some(owner) = owner else {
            continue;
        };
        let owned: Vec<usize> = decls
            .iter()
            .enumerate()
            .filter(|(_, decl)| {
                decl.kind == Kind::Parameter && !decl.named && decl.owner == owner.owner
            })
            .map(|(index, _)| index)
            .collect();
        for (arg_index, arg) in positional_rows.iter().enumerate() {
            let Some(&decl_index) = owned.get(arg_index) else {
                break;
            };
            hits.push(Hit::new(
                c.byte_span(*arg).start,
                c.utf16_span(*arg),
                decl_index,
            ));
        }
    }
}

/// The operator faces: a binary expression reads its operator
/// (`binaryExpression` → the method element at the whole expression), a
/// compound assignment reads the stripped operator at the whole
/// assignment, and an index expression reads `[]` (`indexExpression` → the
/// whole `c[1]`).
fn operator_faces(c: &Check<'_>, decls: &[DeprecatedDecl<'_>], hits: &mut Vec<Hit>) {
    for row in 0..narrow_u32(c.unit.ast.len()) {
        let kind = c.kind(row);
        if !matches!(
            kind,
            NodeKind::BinaryExpression | NodeKind::Assignment | NodeKind::Indexed
        ) {
            continue;
        }
        let range = match kind {
            NodeKind::Indexed => indexed_span(c, row),
            _ => c.utf16_span(row),
        };
        let operator: String = if kind == NodeKind::Indexed {
            "[]".to_owned()
        } else {
            let Some(token) = c.field(row, PayloadField::Operator) else {
                continue;
            };
            let lexeme = c.lexeme(token);
            if kind == NodeKind::Assignment {
                // A plain `=` reads no operator element; a compound
                // assignment reads the underlying operator (`+=` -> `+`;
                // `??=` has none).
                if lexeme == "=" {
                    continue;
                }
                let stripped = lexeme.trim_end_matches('=');
                if stripped.is_empty() || stripped == "?" {
                    continue;
                }
                stripped.to_owned()
            } else {
                lexeme.to_owned()
            }
        };
        let byte = c.byte_span(row).start;
        for (index, decl) in decls.iter().enumerate() {
            if decl.kind == Kind::Member && decl.access == Access::Operator && decl.name == operator
            {
                hits.push(Hit::new(byte, range.clone(), index));
            }
        }
    }
}

// —— declaration table ——————————————————————————————————————————————————

/// The unit's deprecated declarations in declaration order.
fn collect_deprecated<'a>(c: &Check<'a>) -> Vec<DeprecatedDecl<'a>> {
    let mut out = Vec::new();
    for top in c.collect_direct(c.node()) {
        let row = if c.kind(top) == NodeKind::TopLevelDeclaration {
            match c.collect_direct(top).into_iter().find(|&kid| {
                matches!(
                    c.kind(kid),
                    NodeKind::ClassDeclaration
                        | NodeKind::MixinDeclaration
                        | NodeKind::ExtensionDeclaration
                        | NodeKind::ExtensionTypeDeclaration
                        | NodeKind::EnumDeclaration
                        | NodeKind::NamedMixinApplication
                        | NodeKind::Typedef
                        | NodeKind::TopLevelMethod
                        | NodeKind::TopLevelFields
                )
            }) {
                Some(decl) => decl,
                None => continue,
            }
        } else {
            top
        };
        match c.kind(row) {
            NodeKind::ClassDeclaration
            | NodeKind::MixinDeclaration
            | NodeKind::ExtensionDeclaration
            | NodeKind::ExtensionTypeDeclaration
            | NodeKind::EnumDeclaration => {
                let metas = top_level_annotations(c, row);
                let name = container_name(c, row, top);
                if let Some(name) = name {
                    push(&metas, name, None, Kind::Type, Access::Variable, 0, "", false, c, &mut out);
                }
                let (_, members) = container_members(c, row);
                collect_members(c, row, name, &members, &mut out);
            }
            NodeKind::NamedMixinApplication => {
                let metas = top_level_annotations(c, row);
                if let Some(name) = mixin_application_name(c, row) {
                    push(&metas, name, None, Kind::Type, Access::Variable, 0, "", false, c, &mut out);
                }
            }
            NodeKind::Typedef => {
                let metas = top_level_annotations(c, row);
                if let Some(name) = typedef_name(c, row) {
                    push(&metas, name, None, Kind::Type, Access::Variable, 0, "", false, c, &mut out);
                }
            }
            NodeKind::TopLevelMethod | NodeKind::TopLevelFields => {
                let metas = top_level_annotations(c, row);
                let (name, access) = top_level_entry(c, row);
                if let Some(name) = name {
                    push(&metas, name, None, Kind::TopLevel, access, 0, "", false, c, &mut out);
                    if c.kind(row) == NodeKind::TopLevelMethod {
                        collect_parameters(c, row, name, &mut out);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The container's declared name: the `Name` payload word for extensions and
/// extension types (no identifier row), the wrapper's hoisted identifier
/// for the class-likes and the enum.
fn container_name<'a>(c: &Check<'a>, row: u32, top: u32) -> Option<&'a str> {
    match c.kind(row) {
        NodeKind::ExtensionDeclaration | NodeKind::ExtensionTypeDeclaration => {
            payload_name(c, row)
        }
        _ => {
            let name_row = if c.kind(top) == NodeKind::TopLevelDeclaration {
                top
            } else {
                row
            };
            c.collect_direct(name_row)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::Identifier)
                .map(|kid| c.lexeme(c.head_token(kid)))
        }
    }
}

/// A payload `Name` word's lexeme (`None` when absent or the sentinel).
fn payload_name<'a>(c: &Check<'a>, row: u32) -> Option<&'a str> {
    let token = c
        .field(row, PayloadField::Name)
        .filter(|&token| token != crate::rules::NONE_TOKEN)?;
    Some(c.lexeme(token))
}

fn typedef_name<'a>(c: &Check<'a>, row: u32) -> Option<&'a str> {
    let ctx = dartq_parser::IdentifierContext::TypedefDeclaration as u32;
    c.collect_direct(row)
        .into_iter()
        .find(|&kid| {
            c.kind(kid) == NodeKind::Identifier
                && c.field(kid, PayloadField::Context) == Some(ctx)
        })
        .map(|kid| c.lexeme(c.head_token(kid)))
}

/// The mixin application's alias name rides the wrapper as the
/// declaration-context identifier left of the row.
fn mixin_application_name<'a>(c: &Check<'a>, row: u32) -> Option<&'a str> {
    let ctx = dartq_parser::IdentifierContext::ClassOrMixinOrExtensionDeclaration as u32;
    let parent = c.unit.ast.parent(row)?;
    let siblings = c.collect_direct(parent);
    let pos = siblings.iter().position(|&s| s == row)?;
    siblings[..pos]
        .iter()
        .rev()
        .copied()
        .find(|&kid| {
            c.kind(kid) == NodeKind::Identifier
                && c.field(kid, PayloadField::Context) == Some(ctx)
        })
        .map(|kid| c.lexeme(c.head_token(kid)))
}

/// A top-level function or variable declaration's name and access kind.
fn top_level_entry<'a>(c: &Check<'a>, row: u32) -> (Option<&'a str>, Access) {
    match c.kind(row) {
        NodeKind::TopLevelMethod => {
            let ctx = dartq_parser::IdentifierContext::TopLevelFunctionDeclaration as u32;
            let name = c
                .collect_direct(row)
                .into_iter()
                .find(|&kid| {
                    c.kind(kid) == NodeKind::Identifier
                        && c.field(kid, PayloadField::Context) == Some(ctx)
                })
                .map(|kid| c.lexeme(c.head_token(kid)));
            (name, method_access(c, row))
        }
        NodeKind::TopLevelFields => (first_field_name(c, row), Access::Variable),
        _ => (None, Access::Variable),
    }
}

/// The member rows of a container, in order: each member's metadata star is
/// its left sibling (an operator's `OperatorName` row sits between the star
/// and the `Method` row — `member_annotations`' layout).
fn collect_members<'a>(
    c: &Check<'a>,
    container: u32,
    class_name: Option<&'a str>,
    members: &[u32],
    out: &mut Vec<DeprecatedDecl<'a>>,
) {
    let mut index = 0;
    while index < members.len() {
        let member = members[index];
        // An operator's name row precedes its `Method` row.
        let (decl_row, operator) = if c.kind(member) == NodeKind::OperatorName {
            match members.get(index + 1) {
                Some(&next) if c.kind(next) == NodeKind::Method => (next, Some(member)),
                _ => {
                    index += 1;
                    continue;
                }
            }
        } else {
            (member, None)
        };
        let metas = member_metas(c, members, index);
        index += 1 + usize::from(operator.is_some());
        if metas.is_empty() {
            continue;
        }
        match c.kind(decl_row) {
            NodeKind::Method => {
                let Some(name) = method_name(c, decl_row, operator) else {
                    continue;
                };
                let access = if operator.is_some() {
                    Access::Operator
                } else {
                    method_access(c, decl_row)
                };
                push(
                    &metas,
                    name,
                    None,
                    Kind::Member,
                    access,
                    container,
                    "",
                    false,
                    c,
                    out,
                );
                collect_parameters(c, decl_row, name, out);
            }
            NodeKind::Constructor | NodeKind::Factory => {
                let Some((type_name, selector)) = constructor_names(c, decl_row, class_name)
                else {
                    continue;
                };
                push(
                    &metas,
                    type_name,
                    selector,
                    Kind::Constructor,
                    Access::Variable,
                    0,
                    "",
                    false,
                    c,
                    out,
                );
                collect_parameters(c, decl_row, type_name, out);
            }
            NodeKind::Fields => {
                for var in field_rows(c, decl_row) {
                    if let Some(token) = initialized_identifier_name(c, var) {
                        push(
                            &metas,
                            c.lexeme(token),
                            None,
                            Kind::Member,
                            Access::Variable,
                            container,
                            "",
                            false,
                            c,
                            out,
                        );
                    }
                }
            }
            // An enum constant's name is the `EnumValueDeclaration`
            // identifier row beside the childless `EnumElement` handle (the
            // handle's `Name` payload is its span-begin token, i.e. the
            // metadata `@` on a decorated constant).
            NodeKind::Identifier
                if c.field(decl_row, PayloadField::Context)
                    == Some(dartq_parser::IdentifierContext::EnumValueDeclaration as u32) =>
            {
                push(
                    &metas,
                    c.lexeme(c.head_token(decl_row)),
                    None,
                    Kind::EnumValue,
                    Access::Variable,
                    0,
                    "",
                    false,
                    c,
                    out,
                );
            }
            _ => {}
        }
    }
}

/// The metadata rows of the member at `index`: the `MetadataStar` left of it,
/// skipping the operator's name row when it intervenes.
fn member_metas(c: &Check<'_>, members: &[u32], index: usize) -> Vec<u32> {
    let Some(&star) = index.checked_sub(1).and_then(|i| members.get(i)) else {
        return Vec::new();
    };
    if c.kind(star) != NodeKind::MetadataStar {
        return Vec::new();
    }
    c.collect_direct(star)
        .into_iter()
        .filter(|&meta| c.kind(meta) == NodeKind::Metadata)
        .collect()
}

/// The `Method` row's declared name: the `MethodDeclaration`-context
/// identifier, or the `OperatorName`'s payload word (`[]` is written as two
/// tokens upstream but displays as one name).
fn method_name<'a>(c: &Check<'a>, row: u32, operator: Option<u32>) -> Option<&'a str> {
    if let Some(name_row) = operator {
        let lexeme = payload_name(c, name_row)?;
        return Some(if lexeme == "[" { "[]" } else { lexeme });
    }
    let ctx = dartq_parser::IdentifierContext::MethodDeclaration as u32;
    c.collect_direct(row)
        .into_iter()
        .find(|&kid| {
            c.kind(kid) == NodeKind::Identifier
                && c.field(kid, PayloadField::Context) == Some(ctx)
        })
        .map(|kid| c.lexeme(c.head_token(kid)))
}

/// The accessor kind of a `Method`/`TopLevelMethod` row: its `get`/`set`
/// payload word splits the accessor faces (absent = a plain method).
fn method_access(c: &Check<'_>, row: u32) -> Access {
    match c.field(row, PayloadField::GetOrSet) {
        Some(token) if token != crate::rules::NONE_TOKEN => match c.lexeme(token) {
            "get" => Access::Getter,
            "set" => Access::Setter,
            _ => Access::Method,
        },
        _ => Access::Method,
    }
}

/// A constructor's type name and named-constructor selector. The unnamed
/// constructor is written either `new()` or as the class name (`C()`); the
/// own name token is the selector only when it differs from both.
fn constructor_names<'a>(
    c: &Check<'a>,
    row: u32,
    class_name: Option<&'a str>,
) -> Option<(&'a str, Option<&'a str>)> {
    let type_ctx = dartq_parser::IdentifierContext::MethodDeclaration as u32;
    let continuation_ctx =
        dartq_parser::IdentifierContext::MethodDeclarationContinuation as u32;
    let mut type_name = None;
    let mut selector = None;
    for kid in c.collect_direct(row) {
        if c.kind(kid) != NodeKind::Identifier {
            continue;
        }
        match c.field(kid, PayloadField::Context) {
            Some(ctx) if ctx == type_ctx => {
                type_name = Some(c.lexeme(c.head_token(kid)));
            }
            Some(ctx) if ctx == continuation_ctx => {
                selector = Some(c.lexeme(c.head_token(kid)));
            }
            _ => {}
        }
    }
    // `new()` carries no own-name identifier: the unnamed constructor keys
    // on the class name.
    Some((type_name.or(class_name)?, selector))
}

/// The declared parameters of a function/method/constructor: kind
/// `Parameter`, owned by `row`, keyed by the executable's name for the
/// call-site positional mapping.
fn collect_parameters<'a>(
    c: &Check<'a>,
    row: u32,
    owner_name: &'a str,
    out: &mut Vec<DeprecatedDecl<'a>>,
) {
    let Some(outer) = c.collect_direct(row).into_iter().find(|&kid| {
        matches!(
            c.kind(kid),
            NodeKind::FormalParameters
                | NodeKind::OptionalFormalParameters
                | NodeKind::NoFormalParameters
        )
    }) else {
        return;
    };
    // The optional group (`{…}`/`[…]`) nests inside the required list; its
    // own head decides named-vs-positional.
    let list = if c.kind(outer) == NodeKind::FormalParameters {
        c.collect_direct(outer)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::OptionalFormalParameters)
            .unwrap_or(outer)
    } else {
        outer
    };
    let named = c.lexeme(c.head_token(list)) == "{";
    for param in c.collect_direct(list) {
        // The function-typed parameter carries no `Name` payload word (its
        // schema is `[Question]`); its own name is not a call-site key.
        if c.kind(param) != NodeKind::FormalParameter {
            continue;
        }
        let Some(name) = payload_name(c, param) else {
            continue;
        };
        let metas = parameter_metas(c, param);
        push(
            &metas,
            name,
            None,
            Kind::Parameter,
            Access::Variable,
            row,
            owner_name,
            named,
            c,
            out,
        );
    }
}

/// A parameter's metadata star is its left sibling in the parameter list.
fn parameter_metas(c: &Check<'_>, param: u32) -> Vec<u32> {
    let Some(parent) = c.unit.ast.parent(param) else {
        return Vec::new();
    };
    let kids = c.collect_direct(parent);
    let Some(pos) = kids.iter().position(|&kid| kid == param) else {
        return Vec::new();
    };
    let Some(&star) = pos.checked_sub(1).and_then(|i| kids.get(i)) else {
        return Vec::new();
    };
    if c.kind(star) != NodeKind::MetadataStar {
        return Vec::new();
    }
    c.collect_direct(star)
        .into_iter()
        .filter(|&meta| c.kind(meta) == NodeKind::Metadata)
        .collect()
}

/// The first declared variable's name of a top-level field row.
fn first_field_name<'a>(c: &Check<'a>, row: u32) -> Option<&'a str> {
    field_rows(c, row)
        .into_iter()
        .find_map(|var| initialized_identifier_name(c, var))
        .map(|token| c.lexeme(token))
}

/// The `InitializedIdentifier` rows of a field declaration.
fn field_rows(c: &Check<'_>, row: u32) -> Vec<u32> {
    let mut out = Vec::new();
    for decl in c.collect_direct(row) {
        if c.kind(decl) != NodeKind::VariablesDeclaration {
            continue;
        }
        for var in c.collect_direct(decl) {
            if c.kind(var) == NodeKind::InitializedIdentifier {
                out.push(var);
            }
        }
    }
    out
}

/// One table entry per deprecated metadata row of `metas`.
#[allow(clippy::too_many_arguments)]
fn push<'a>(
    metas: &[u32],
    name: &'a str,
    selector: Option<&'a str>,
    kind: Kind,
    access: Access,
    owner: u32,
    owner_name: &'a str,
    named: bool,
    c: &Check<'_>,
    out: &mut Vec<DeprecatedDecl<'a>>,
) {
    for &meta in metas {
        if !is_deprecated_metadata(c, meta) {
            continue;
        }
        out.push(DeprecatedDecl {
            name,
            selector,
            message: deprecation_message(c, meta),
            kind,
            access,
            owner,
            owner_name,
            named,
        });
    }
}

/// The `@Deprecated('…')` tag: the first argument's string value run
/// through `normalizeDeprecationMessage`
/// (deprecated_member_use_verifier.dart:18-29) — trimmed; empty and `.`
/// normalize to `None` (the without-message variant); a trailing `.`,
/// `?` or `!` stays as written, anything else gains a `.`. `None`
/// without the outer face for a bare `@deprecated` (no arguments) or an
/// unreadable tag.
fn deprecation_message(c: &Check<'_>, meta: u32) -> Option<String> {
    let argument = c
        .collect_direct(meta)
        .into_iter()
        .find(|&piece| c.kind(piece) == NodeKind::Arguments)
        .and_then(|args| c.collect_direct(args).into_iter().next())
        .filter(|&lit| c.kind(lit) == NodeKind::StringLiteral);
    let value = crate::rules::directives::string_value(c, argument?)?;
    normalize_deprecation_message(Some(&value))
}

// —— use-site shapes ————————————————————————————————————————————————————

/// Whether `byte` sits inside `row`'s span (the enclosing-container and
/// frontier windows).
fn inside(c: &Check<'_>, byte: usize, row: u32) -> bool {
    let span = c.byte_span(row);
    span.start <= byte && byte < span.end
}

/// The receiver of a selector use (`a.f` → `a`), `None` when the dot marker
/// hugs this identifier's left edge but the receiver is not an identifier
/// (`super.f`, `this.f`, an index chain) — the selector face still applies.
fn selector_left(c: &Check<'_>, row: u32) -> Option<Option<u32>> {
    let parent = c.unit.ast.parent(row)?;
    let kids = c.collect_direct(parent);
    let span = c.byte_span(row);
    // The dot chain is a `DotAccess` row beginning at this name (the row is
    // sealed after its operands; its span starts at the selector name).
    kids.iter().copied().find(|&kid| {
        c.kind(kid) == NodeKind::DotAccess && c.byte_span(kid).start == span.start
    })?;
    let receiver = kids
        .iter()
        .copied()
        .filter(|&kid| {
            c.kind(kid) == NodeKind::Identifier && c.byte_span(kid).end <= span.start
        })
        .max_by_key(|&kid| c.utf16_span(kid).start);
    Some(receiver)
}

/// The identifier's assignment-LHS face: `Some(true)` for a plain `=` (the
/// write element only), `Some(false)` for a compound assignment (the read
/// and the write element — `assignmentExpression` checks both), `None` for a
/// plain read.
fn assignment_lhs(c: &Check<'_>, row: u32) -> Option<bool> {
    // The assignment row's tiling begins at the operator's operand, not at
    // the target; the write face reads the operator token right after the
    // name instead.
    let token = c.head_token(row);
    let next = c.unit.tokens.get(token as usize + 1)?;
    if next.kind == dartq_scanner::TokenKind::Eof {
        return None;
    }
    let lexeme = c.lexeme(token + 1);
    if lexeme == "=" {
        return Some(true);
    }
    if lexeme.len() > 1
        && lexeme.ends_with('=')
        && !matches!(lexeme, "==" | "=>" | "!=" | "<=" | ">=")
    {
        return Some(false);
    }
    None
}

/// Whether the declaration answers the use's access face.
fn access_ok(decl: &DeprecatedDecl<'_>, lhs: Option<bool>) -> bool {
    match lhs {
        Some(true) => decl.access.writable(),
        Some(false) => decl.access.readable() || decl.access.writable(),
        None => decl.access.readable(),
    }
}

/// Whether the identifier is immediately followed by an argument list (a
/// constructor invocation `C()` rather than a type annotation `C c`).
fn followed_by_arguments(c: &Check<'_>, row: u32) -> bool {
    let Some(parent) = c.unit.ast.parent(row) else {
        return false;
    };
    let kids = c.collect_direct(parent);
    let Some(pos) = kids.iter().position(|&kid| kid == row) else {
        return false;
    };
    kids[pos + 1..].iter().any(|&kid| c.kind(kid) == NodeKind::Arguments)
}

/// The callee name of an argument list: the last identifier before the
/// `Arguments` row inside the invocation.
fn callee_name<'a>(c: &Check<'a>, arguments: u32) -> Option<&'a str> {
    let parent = c.unit.ast.parent(arguments)?;
    let kids = c.collect_direct(parent);
    let pos = kids.iter().position(|&kid| kid == arguments)?;
    kids[..pos]
        .iter()
        .rev()
        .copied()
        .find(|&kid| c.kind(kid) == NodeKind::Identifier)
        .map(|kid| c.lexeme(c.head_token(kid)))
}

/// The increment face's operand span (`++a.f` / `a.f++` report at `a.f`):
/// the receiver start through the selector end when an increment marker hugs
/// the operand.
fn increment_operand_span(c: &Check<'_>, row: u32) -> Option<core::ops::Range<u32>> {
    let mut node = row;
    for _ in 0..4 {
        let parent = c.unit.ast.parent(node)?;
        let kids = c.collect_direct(parent);
        let increments = kids.iter().any(|&kid| {
            matches!(c.kind(kid), NodeKind::UnaryPrefix | NodeKind::UnaryPostfix)
                && matches!(c.lexeme(c.head_token(kid)), "++" | "--")
        });
        if increments {
            // The operand starts at the chain's leftmost identifier
            // (`a` in `a.f++`).
            let start = kids
                .iter()
                .filter(|&&kid| {
                    c.kind(kid) == NodeKind::Identifier
                        && c.byte_span(kid).end <= c.byte_span(row).start
                })
                .map(|&kid| c.utf16_span(kid).start)
                .max()
                .unwrap_or_else(|| c.utf16_span(kids[0]).start);
            return Some(start..c.utf16_span(row).end);
        }
        node = parent;
    }
    None
}

/// The index expression's report span: the receiver start through the
/// closing bracket (the `Indexed` row itself starts at `[`).
fn indexed_span(c: &Check<'_>, row: u32) -> core::ops::Range<u32> {
    let own = c.utf16_span(row);
    let Some(parent) = c.unit.ast.parent(row) else {
        return own;
    };
    let start = c.byte_span(row).start;
    let receiver = c
        .collect_direct(parent)
        .into_iter()
        .filter(|&kid| c.byte_span(kid).end == start)
        .max_by_key(|&kid| c.utf16_span(kid).start);
    match receiver {
        Some(receiver) => c.utf16_span(receiver).start..own.end,
        None => own,
    }
}

// —— directive faces (workspace-gated) ———————————————————————————————————

/// The directive faces over the unit's `Import`/`Export` rows. Silent
/// without the workspace facts (no URI resolution, no package URI).
fn directive_faces(c: &mut Check<'_>) {
    let Some(workspace) = c.unit.workspace else {
        return;
    };
    let Some(path) = c.unit.path else {
        return;
    };
    for row in 0..narrow_u32(c.unit.ast.len()) {
        let kind = c.kind(row);
        if kind != NodeKind::Import && kind != NodeKind::Export {
            continue;
        }
        let kids = c.collect_direct(row);
        let Some(uri_row) = kids
            .iter()
            .copied()
            .find(|&kid| c.kind(kid) == NodeKind::StringLiteral)
        else {
            continue;
        };
        let Some(uri) = string_literal_value(c, c.head_token(uri_row)) else {
            continue;
        };
        let Some(target) = resolve_same_package(workspace, path, uri) else {
            continue;
        };
        let Ok(source) = std::fs::read_to_string(&target) else {
            continue;
        };
        let face = scan_target_deprecations(&source, &target);
        // The library face: the whole directive reports with the
        // target's package URI as the display name (upstream
        // `displayName = element.uri.toString()`).
        if let Some(message) = face.library {
            if let Some(display) = package_uri(workspace, &target) {
                let range = c.utf16_span(row);
                match message {
                    Some(message) => {
                        c.report_texts(range, WITH_MESSAGE, CORRECTION, &[&display, &message]);
                    }
                    None => c.report_texts(range, WITHOUT_MESSAGE, CORRECTION, &[&display]),
                }
            }
        }
        // The `show` combinator face: a shown name matching a deprecated
        // top-level class-family declaration of the target reports at
        // the name (`hide` is forgiving — no `Hide` walk). The rows
        // materialize first: the report borrows `c` mutably.
        let shows: Vec<u32> = kids
            .iter()
            .copied()
            .filter(|&kid| c.kind(kid) == NodeKind::Show)
            .collect();
        for show in shows {
            let names: Vec<u32> = c
                .collect_direct(show)
                .into_iter()
                .filter(|&kid| c.kind(kid) == NodeKind::Identifier)
                .collect();
            for name in names {
                let lexeme = c.lexeme(c.head_token(name));
                let Some((_, message)) = face
                    .declared
                    .iter()
                    .find(|(declared, _)| declared.as_str() == lexeme)
                else {
                    continue;
                };
                let range = c.utf16_span(name);
                match message {
                    Some(message) => {
                        c.report_texts(range, WITH_MESSAGE, CORRECTION, &[lexeme, message]);
                    }
                    None => c.report_texts(range, WITHOUT_MESSAGE, CORRECTION, &[lexeme]),
                }
            }
        }
    }
}

/// One `Import`/`Export` row's directive-face verdicts, read off the
/// target file: the deprecated `library` directive (`Some(message)` —
/// `None` inside for the bare `@deprecated` shape) and the deprecated
/// top-level class-family names for the combinator face.
#[derive(Default)]
struct TargetDeprecations {
    library: Option<Option<String>>,
    declared: Vec<(String, Option<String>)>,
}

/// The URI literal's value: the lexeme with one level of quotes
/// stripped (interpolation segments stay unreported).
fn unquote(raw: &str) -> Option<&str> {
    let bytes = raw.as_bytes();
    if bytes.len() >= 2
        && (bytes[0] == b'\'' || bytes[0] == b'"')
        && bytes[0] == bytes[bytes.len() - 1]
    {
        Some(&raw[1..raw.len() - 1])
    } else {
        None
    }
}

/// The URI literal's value off the unit's own token table.
fn string_literal_value<'a>(c: &'a Check<'_>, token: u32) -> Option<&'a str> {
    unquote(c.lexeme(token))
}

/// The file a same-package URI resolves to: `package:<own>/…` through
/// the package's URI directory, anything else relative to the importing
/// file. Cross-package and SDK URIs answer `None` (those uses are the
/// analyzer's `DEPRECATED_MEMBER_USE`, not this rule's); a relative
/// target that escapes the analyzed package too (the `isInSamePackage`
/// gate) answers `None`.
fn resolve_same_package(
    workspace: &crate::WorkspaceFacts,
    path: &Path,
    uri: &str,
) -> Option<PathBuf> {
    if let Some(rest) = uri.strip_prefix("package:") {
        let own = format!("{}/", workspace.own_package);
        let relative = rest.strip_prefix(&own)?;
        return Some(workspace.lib_dir.join(relative));
    }
    if uri.starts_with("dart:") {
        return None;
    }
    let base = path.parent()?;
    let target = base.join(uri);
    target.starts_with(&workspace.root).then_some(target)
}

/// The target's `package:` display URI — `package:<own>/<path under the
/// package URI directory>`; a target outside it has no package URI and
/// stays unreported (the official face's `element.uri` has no
/// `package:` form for it either).
fn package_uri(workspace: &crate::WorkspaceFacts, target: &Path) -> Option<String> {
    let relative = target.strip_prefix(&workspace.lib_dir).ok()?;
    let mut display = format!("package:{}", workspace.own_package);
    for component in relative.components() {
        display.push('/');
        display.push_str(&component.as_os_str().to_string_lossy());
    }
    Some(display)
}

/// The token-level deprecation scan of a target file: the deprecation
/// annotation directly preceding a `library` directive, and the
/// deprecated top-level class-family declarations (the shapes the
/// directive faces read). Token-level so comments and strings never
/// answer; the `library` face wins over the per-declaration list.
fn scan_target_deprecations(source: &str, path: &Path) -> TargetDeprecations {
    let mut face = TargetDeprecations::default();
    let mut files = dartq_source::FileTable::default();
    let tokens = dartq_scanner::scan(
        source,
        dartq_scanner::ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        files.intern(path),
    )
    .tokens;
    let lexeme = |index: usize| tokens.lexeme(source, index);
    let kind = |index: usize| tokens.get(index).map(|token| token.kind);
    let mut index = 0;
    while index < tokens.len() {
        if lexeme(index) != "@" {
            index += 1;
            continue;
        }
        // The annotation name must hug the `@`.
        let Some(name_token) = (tokens.get(index + 1))
            .filter(|token| token.kind == dartq_scanner::TokenKind::Identifier)
            .map(|_| index + 1)
        else {
            index += 1;
            continue;
        };
        let name = lexeme(name_token);
        if name != "deprecated" && name != "Deprecated" {
            index += 1;
            continue;
        }
        // `@Deprecated('…')`: the first string argument is the message,
        // run through the official normalization.
        let mut cursor = name_token + 1;
        let mut message = None;
        if kind(cursor) == Some(dartq_scanner::TokenKind::OpenParen) {
            let mut depth = 0usize;
            let mut close = cursor;
            while close < tokens.len() {
                match kind(close) {
                    Some(dartq_scanner::TokenKind::OpenParen) => depth += 1,
                    Some(dartq_scanner::TokenKind::CloseParen) => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    Some(dartq_scanner::TokenKind::Eof) => break,
                    Some(dartq_scanner::TokenKind::String) if depth == 1 && message.is_none() => {
                        let token = tokens.get(close);
                        let raw = token.map(|token| {
                            &source[token.byte_offset as usize
                                ..(token.byte_offset + token.byte_len) as usize]
                        });
                        message = normalize_deprecation_message(raw.and_then(unquote));
                    }
                    _ => {}
                }
                close += 1;
            }
            cursor = close + 1;
        }
        // The annotated head decides the face: `library` (the directive
        // face) or the class-family keyword run (the combinator face).
        // Every read clamps to the table (the Eof terminator's empty
        // lexeme answers every keyword probe) — a malformed target
        // degrades to silence, never out of bounds.
        let last = tokens.len() - 1;
        let cursor = cursor.min(last);
        let head = lexeme(cursor);
        if head == "library" {
            face.library = Some(message);
        } else if matches!(
            head,
            "class" | "mixin" | "enum" | "extension" | "type" | "typedef"
        ) {
            let mut head = cursor;
            while head < last
                && matches!(
                    lexeme(head),
                    "class"
                        | "mixin"
                        | "enum"
                        | "extension"
                        | "type"
                        | "typedef"
                        | "sealed"
                        | "base"
                        | "abstract"
                        | "final"
                        | "augment"
                )
            {
                head += 1;
            }
            if kind(head) == Some(dartq_scanner::TokenKind::Identifier) {
                face.declared.push((lexeme(head).to_owned(), message));
            }
        }
        index = cursor.max(index + 1);
    }
    face
}

/// `normalizeDeprecationMessage`
/// (deprecated_member_use_verifier.dart:18-29): trimmed; empty and `.`
/// normalize to `None`; a trailing `.`, `?` or `!` stays as written,
/// anything else gains a `.`.
fn normalize_deprecation_message(value: Option<&str>) -> Option<String> {
    let trimmed = value?.trim();
    if trimmed.is_empty() || trimmed == "." {
        return None;
    }
    if trimmed.ends_with(['.', '?', '!']) {
        Some(trimmed.to_owned())
    } else {
        Some(format!("{trimmed}."))
    }
}

// —— frontier suppression ————————————————————————————————————————————————

/// Whether `row` carries `@deprecated` metadata — the metadata star is
/// the row's left sibling in its parent's children (the uniform
/// `parseMetadataStar` emission order for declarations, members, formal
/// parameters and directives).
fn row_is_deprecated(c: &Check<'_>, row: u32) -> bool {
    let Some(parent) = c.unit.ast.parent(row) else {
        return false;
    };
    let siblings = c.collect_direct(parent);
    let Some(pos) = siblings.iter().position(|&s| s == row) else {
        return false;
    };
    let Some(&star) = pos.checked_sub(1).and_then(|i| siblings.get(i)) else {
        return false;
    };
    if c.kind(star) != NodeKind::MetadataStar {
        return false;
    }
    c.collect_direct(star)
        .into_iter()
        .any(|meta| is_deprecated_metadata(c, meta))
}

/// Whether any `library` directive of the unit carries `@deprecated` —
/// the deprecated-library face that silences the whole unit (the
/// frontier's root push). Position-independent: a raw row walk over the
/// `LibraryName` rows. The star read is recovery-tolerant — a mid-file
/// `library` directive (the ordering diagnostic) emits a
/// `RecoverableError` row between the metadata star and the wrapper, so
/// the scan skips recovery rows before requiring the star; nothing but
/// recovery markers can interleave there (the star parses immediately
/// before the directive opens).
fn unit_library_deprecated(c: &Check<'_>) -> bool {
    for row in 0..narrow_u32(c.unit.ast.len()) {
        if c.kind(row) != NodeKind::LibraryName {
            continue;
        }
        if row_is_deprecated(c, row) {
            return true;
        }
        let Some(wrapper) = c.unit.ast.parent(row) else {
            continue;
        };
        if c.kind(wrapper) != NodeKind::TopLevelDeclaration {
            continue;
        }
        let Some(parent) = c.unit.ast.parent(wrapper) else {
            continue;
        };
        let siblings = c.collect_direct(parent);
        let Some(pos) = siblings.iter().position(|&s| s == wrapper) else {
            continue;
        };
        for prev in siblings[..pos].iter().rev().copied() {
            match c.kind(prev) {
                NodeKind::RecoverableError => continue,
                NodeKind::MetadataStar => {
                    if c.collect_direct(prev)
                        .into_iter()
                        .any(|meta| is_deprecated_metadata(c, meta))
                    {
                        return true;
                    }
                    break;
                }
                _ => break,
            }
        }
    }
    false
}

/// The declaration shapes the upstream frontier visitor pushes while
/// their contents are visited (`element_usage_frontier_visitor.dart`:
/// the `_withDeclaration` class/enum/extension/mixin/typedef/function
/// overrides, the constructor/method overrides, the field and top-level
/// variable pushes, and the formal-parameter overrides). Enum constants,
/// locals and closures are not pushed upstream and not here.
fn is_frontier_declaration(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::ClassDeclaration
            | NodeKind::MixinDeclaration
            | NodeKind::ExtensionDeclaration
            | NodeKind::ExtensionTypeDeclaration
            | NodeKind::EnumDeclaration
            | NodeKind::NamedMixinApplication
            | NodeKind::Typedef
            | NodeKind::TopLevelMethod
            | NodeKind::TopLevelFields
            | NodeKind::Method
            | NodeKind::Constructor
            | NodeKind::Factory
            | NodeKind::Fields
            | NodeKind::FormalParameter
            | NodeKind::FunctionTypedFormalParameter
    )
}

/// Whether `row` sits under a deprecated frontier annotation: the
/// metadata star is the row's left sibling (the uniform
/// `parseMetadataStar` emission order — the star parses before the row
/// opens), and class-likes behind a `TopLevelDeclaration` wrapper carry
/// the star beside the wrapper.
fn frontier_deprecated(c: &Check<'_>, row: u32) -> bool {
    if row_is_deprecated(c, row) {
        return true;
    }
    match c.unit.ast.parent(row) {
        Some(wrapper) if c.kind(wrapper) == NodeKind::TopLevelDeclaration => {
            row_is_deprecated(c, wrapper)
        }
        _ => false,
    }
}

/// The suppression span of a frontier declaration: the wrapper's span
/// for the wrapper-nested top-level rows (it covers the hoisted name),
/// the row's own span otherwise — extended to the row's subtree extent,
/// because some tilings seal the row short of its children (the
/// function-typed parameter's outer row seals at `void p` while the
/// nested parameter list rides beside it).
fn frontier_span(c: &Check<'_>, row: u32) -> core::ops::Range<usize> {
    let (base, base_row) = match c.unit.ast.parent(row) {
        Some(wrapper) if c.kind(wrapper) == NodeKind::TopLevelDeclaration => {
            (c.byte_span(wrapper), wrapper)
        }
        _ => (c.byte_span(row), row),
    };
    let window = c.unit.ast.children(base_row);
    let end = (window.start..window.end)
        .map(|kid| c.byte_span(kid).end)
        .max()
        .unwrap_or(base.end)
        .max(base.end);
    base.start..end
}

/// The reference-context faces of the identifier-context payload: bare
/// expressions and member selectors, type references (plain, prefixed
/// and continuations), constructor references. Declaration names,
/// parameters, metadata, labels and symbols never report.
fn is_reference_context(context: u32) -> bool {
    use dartq_parser::IdentifierContext;
    context == IdentifierContext::Expression as u32
        || context == IdentifierContext::ExpressionContinuation as u32
        || context == IdentifierContext::TypeReference as u32
        || context == IdentifierContext::PrefixedTypeReference as u32
        || context == IdentifierContext::TypeReferenceContinuation as u32
        || context == IdentifierContext::ConstructorReference as u32
        || context == IdentifierContext::ConstructorReferenceContinuation as u32
        || context == IdentifierContext::ConstructorReferenceContinuationAfterTypeArguments as u32
}
