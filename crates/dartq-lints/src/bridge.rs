//! The parser→AST production bridge: the [`ParserListener`] that translates
//! the parser's event stream into [`AstBuilder`] calls, plus the transient
//! [`ParsedUnit`] entry the consumption faces drive.
//!
//! Every node-producing event maps 1:1 onto the collapse-map verdict
//! (`docs/facts/ast-collapse-map.md` §3): Begin events open the End-kind
//! container (R1), Handle events emit childless leaf/Fielded nodes, End
//! events seal. Events whose official node is dropped by the map (D1-D4
//! derivable, diagnostic-only) are explicit documented no-ops — never
//! silent skips. Two span conventions coexist in the parser (mirroring the
//! official call shapes): end events carry either the last consumed token
//! (inclusive; the bridge seals at `token + 1`) or the next token (peek;
//! exclusive; sealed as-is) — each handler below pins its event's call
//! site.
//!
//! Zero persistent memory: nothing here is retained after
//! [`ParsedUnit`] drops — the engine re-parses resident source text at
//! consumption time (#355 ruling 1).
//!
//! The `usize` to `u32` casts below mirror the builder's payload widths.

#![allow(clippy::cast_possible_truncation)]

use std::path::Path;

use dartq_ast::{AstBuilder, NodeKind, SyntaxTable, TOKEN_NONE};
use dartq_parser::{
    AssertKind, ConstructorReferenceContext, DeclarationHeaderKind, DeclarationKind,
    ExperimentalFlag, FormalParameterKind, IdentifierContext, MemberKind, ParserFeatures,
    ParserListener, TokenSpan,
};
use dartq_scanner::{ScanConfig, TokenTable, scan};
use dartq_source::LineIndex;

use crate::engine::{Features, Unit};

/// Modifier-set bit assignments (collapse map §1 families; the parser lane
/// owns the values, `NodeHeader.flags` bits 1..=7). A node carries exactly
/// one family — class-like declarations or members — so the two families
/// share positions. In every case the FIRST modifier in source order is the
/// node's span head (the official begin token is the first token after
/// metadata), which is what `derived::first_modifier` reads back.
mod mod_bits {
    pub(super) const AUGMENT: u16 = 1 << 1;
    pub(super) const ABSTRACT: u16 = 1 << 2;
    pub(super) const SEALED: u16 = 1 << 3;
    pub(super) const BASE: u16 = 1 << 4;
    pub(super) const INTERFACE: u16 = 1 << 5;
    pub(super) const FINAL: u16 = 1 << 6;
    pub(super) const MIXIN: u16 = 1 << 7;
    // Member family (shares positions; disjoint kinds):
    pub(super) const EXTERNAL: u16 = 1 << 2;
    pub(super) const STATIC: u16 = 1 << 3;
    pub(super) const COVARIANT: u16 = 1 << 4;
    pub(super) const LATE: u16 = 1 << 5;
    pub(super) const VAR_FINAL_OR_CONST: u16 = 1 << 6;
    pub(super) const REQUIRED: u16 = 1 << 7;
}

/// `RecoverDeclarationHeader.kind` discriminants (collapse map §3.2).
const HEADER_KIND_CLASS: u32 = 0;
const HEADER_KIND_EXTENSION_TYPE: u32 = 1;
const HEADER_KIND_MIXIN: u32 = 2;

/// One parsed unit: everything a [`Unit`] run needs, kept alive together.
/// Produced transiently per lint pass by [`parse_unit`] — never stored in
/// engine residency.
pub struct ParsedUnit {
    pub source: String,
    pub tokens: TokenTable,
    pub ast: SyntaxTable,
    pub lines: LineIndex,
    pub features: Features,
}

impl ParsedUnit {
    /// Borrows the unit substrate for one lint run. The workspace facts
    /// mount by assignment after the call (`Unit::workspace`) — the bridge
    /// itself carries no pubspec face.
    #[must_use]
    pub fn unit<'a>(&'a self, path: Option<&'a Path>) -> Unit<'a> {
        Unit {
            source: &self.source,
            tokens: &self.tokens,
            ast: &self.ast,
            lines: &self.lines,
            path,
            features: self.features,
            workspace: None,
        }
    }
}

/// Production scan posture: the latest language surface (the lint rules
/// target the Dart 3.15 linter; the scanner gates `>>>` and `augment`).
const SCAN_CONFIG: ScanConfig = ScanConfig {
    enable_triple_shift: true,
    enable_augmentations: true,
};

/// Production parse posture: Dart 3.15 — patterns (3.0) and primary
/// constructors (3.13) shipped, variance and enhanced parts unshipped.
const PARSE_FEATURES: ParserFeatures = ParserFeatures {
    patterns_enabled: true,
    variance_enabled: false,
    primary_constructors_enabled: true,
    augmentations_enabled: true,
    enhanced_parts_enabled: false,
    dot_shorthands_enabled: true,
};

/// Parses `source` once (scanner → parser → arena) and yields the transient
/// substrate one lint run reads. The language-version features that gate
/// rule *registration* travel in `features`; the grammar posture is the
/// fixed production surface above.
#[must_use]
pub fn parse_unit(source: &str, features: Features) -> ParsedUnit {
    // The file identity is engine-side (diagnostics carry it); the parse
    // needs a token-table stamp only, interned into a throwaway table.
    let mut files = dartq_source::FileTable::default();
    let file = files.intern(Path::new("unit.dart"));
    let scanned = scan(source, SCAN_CONFIG, file);
    let mut tokens = scanned.tokens;
    let base = tokens.len() as u32;
    let (listener, inserted) =
        dartq_parser::parse_unit_with(source, &tokens, PARSE_FEATURES, AstListener::new(base));
    tokens.append_inserted(inserted);
    let ast = listener.finish();
    let lines = LineIndex::new(source);
    ParsedUnit {
        source: source.to_owned(),
        tokens,
        ast,
        lines,
        features,
    }
}

/// The event→arena translator. Carries only the pending state the shared
/// event orders require (below); everything else goes straight into the
/// builder.
#[derive(Debug, Default)]
pub(crate) struct AstListener {
    b: AstBuilder,
    /// The scanned table length (Eof included) before the parse. Parser-side
    /// inserted-token ids start at `base`; the unified table packs them one
    /// index lower (the first insertion takes the re-seated Eof's former
    /// slot, `TokenTable::append_inserted`) — [`AstListener::t`] is the
    /// one-step translation every event token id goes through on its way
    /// into the builder.
    base: u32,
    /// `Import` prefix patches land while the node is open; a second prefix
    /// event (recovery) overwrites — same last-wins shape as the official
    /// duplicate-clause recovery.
    import_open: bool,
    /// `handle_catch_block` fires AFTER `end_catch_clause` (the catch body
    /// parses between them), so the `CatchClause` seal is deferred to it —
    /// the payload words (`catch`, `comma`) patch then, with the block as
    /// the last child.
    catch_open: bool,
    /// `handle_enum_elements` fires while the `EnumBody` wrapper is still
    /// open; the `EnumDeclaration` payload word 2 (`elements_end`) is
    /// stashed here and applied at `end_enum_declaration` (`None` writes
    /// the `TOKEN_NONE` sentinel for element-less enums).
    enum_elements_end: Option<u32>,
    /// Arena-tail snapshot at the previous `SwitchCase` boundary: the case's
    /// labels/expressions/guards complete BEFORE `begin_switch_case` fires,
    /// in a mix no sibling count can walk (collapse map §4 keeps
    /// `expression_count` precisely because the children are flat), so the
    /// listener tracks the segment start itself.
    switch_material_start: u32,
    /// `MetadataStar` opens with an anchor token and may seal empty (the
    /// recovery path fires the pair back-to-back); the anchor and the arena
    /// length at begin decide the exclusive end.
    metadata_star: Option<(u32, u32)>,
    /// Whether a `TopLevelDeclaration` wrapper is open (the recovery path in
    /// `parse_unit` fires `end_top_level_declaration` without a prelude).
    tld_open: bool,
    /// Which class-like declaration is open — `handle_implements`/
    /// `handle_class_extends`/`handle_mixin_on` patch only the kinds whose
    /// payload carries their word.
    class_decl: ClassDecl,
    /// The last `handle_async_modifier` words: both function-body kinds
    /// absorb them into their payload, but the modifier event fires BEFORE
    /// the body node opens (the grammar consumes `async` before `{`/`=>`).
    async_pending: (Option<u32>, Option<u32>),
    /// `ConstantPattern` deferral: the operand parses between the events and
    /// `const` (when present) is the span head, so the node seals at the end
    /// event with the operand adopted.
    pending_constant_pattern: Option<u32>,
    /// `ObjectPattern` fields-first deferral: the fields parse and seal
    /// BEFORE `handle_object_pattern` fires (official order,
    /// parser_impl.dart:11847-11848), so the fields event stashes the child
    /// window start (arena tail) and exclusive end; the object-pattern event
    /// pops the marker and reassembles, exactly the official builder's
    /// `_ObjectPatternFields` pop (ast_builder.dart:5362-5410).
    object_pattern_pending: Option<(u32, u32)>,
    /// `IsExpression`/`AsExpression` defer their seal to the operator handle
    /// event that immediately follows the end event and carries the word.
    deferred_seal: Option<NodeKind>,
}

/// The open class-like declaration family (payload-word routing).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ClassDecl {
    #[default]
    None,
    Class,
    Mixin,
    NamedMixinApplication,
}

impl AstListener {
    pub(crate) fn new(base: u32) -> Self {
        Self {
            base,
            ..Self::default()
        }
    }

    /// Parser-side token id → unified-table index (see `base`).
    fn t(&self, id: u32) -> u32 {
        if id >= self.base { id - 1 } else { id }
    }

    /// Consumes the listener into the sealed table.
    #[must_use]
    pub(crate) fn finish(self) -> SyntaxTable {
        self.b.finish()
    }

    fn begin(&mut self, kind: NodeKind, token: TokenSpan) {
        self.b.begin(kind, self.t(token.id));
    }

    /// Seals at an already-exclusive bound (the parser passed the next token).
    fn end_at(&mut self, kind: NodeKind, exclusive_end: u32) {
        self.b.end(kind, exclusive_end);
    }

    /// Seals just past a last-consumed token.
    fn end_incl(&mut self, kind: NodeKind, token: TokenSpan) {
        self.b.end(kind, self.t(token.id) + 1);
    }

    /// Seals right after the most recently completed child (for end events
    /// that carry no end token).
    fn end_last(&mut self, kind: NodeKind) {
        let end = self.b.last_end_token().expect("end_last before any node");
        self.b.end(kind, end);
    }

    /// Patch word with a token or the absent sentinel.
    fn patch_token(&mut self, field: usize, token: Option<TokenSpan>) {
        self.b
            .patch(field, token.map_or(TOKEN_NONE, |t: TokenSpan| self.t(t.id)));
    }

    fn set_mods(&mut self, bits: u16) {
        self.b.set_modifiers(bits);
    }

    /// Emits the `RecoverDeclarationHeader` sentinel: a zero-width Fielded
    /// node at the current position (a recovery-pass marker, no extent of
    /// its own).
    fn recover_header(&mut self, kind_discriminant: u32) {
        let anchor = self.b.last_end_token().expect("recover header anchor");
        self.b.begin(NodeKind::RecoverDeclarationHeader, anchor);
        self.b.patch(0, kind_discriminant);
        self.b.end(NodeKind::RecoverDeclarationHeader, anchor);
    }

    /// The member-family modifier bits shared by fields/methods/
    /// constructors/factories.
    fn member_bits(
        augment: Option<TokenSpan>,
        external: Option<TokenSpan>,
        static_kw: Option<TokenSpan>,
        covariant: Option<TokenSpan>,
        late_kw: Option<TokenSpan>,
        var_final_or_const: Option<TokenSpan>,
    ) -> u16 {
        let mut bits = 0;
        if augment.is_some() {
            bits |= mod_bits::AUGMENT;
        }
        if external.is_some() {
            bits |= mod_bits::EXTERNAL;
        }
        if static_kw.is_some() {
            bits |= mod_bits::STATIC;
        }
        if covariant.is_some() {
            bits |= mod_bits::COVARIANT;
        }
        if late_kw.is_some() {
            bits |= mod_bits::LATE;
        }
        if var_final_or_const.is_some() {
            bits |= mod_bits::VAR_FINAL_OR_CONST;
        }
        bits
    }
}

impl ParserListener for AstListener {
    // —— compilation unit ————————————————————————————————————————————
    fn begin_compilation_unit(&mut self, token: TokenSpan) {
        self.begin(NodeKind::CompilationUnit, token);
    }

    fn end_compilation_unit(&mut self, _count: usize, token: TokenSpan) {
        // Eof is the last token; the unit's exclusive end is past it
        // (span.rs special-cases the unit root to `[0, eof.end]`).
        self.end_incl(NodeKind::CompilationUnit, token);
    }

    // —— directives (#300) ———————————————————————————————————————————————
    fn begin_library_name(&mut self, token: TokenSpan) {
        self.begin(NodeKind::LibraryName, token);
    }

    fn end_library_name(
        &mut self,
        _library_keyword: TokenSpan,
        semicolon: TokenSpan,
        has_name: bool,
    ) {
        self.b.patch(0, u32::from(has_name));
        self.end_incl(NodeKind::LibraryName, semicolon);
    }

    fn begin_library_augmentation(&mut self, augment_kw: TokenSpan, _library_keyword: TokenSpan) {
        self.begin(NodeKind::LibraryAugmentation, augment_kw);
    }

    fn end_library_augmentation(
        &mut self,
        augment_kw: TokenSpan,
        library_keyword: TokenSpan,
        semicolon: TokenSpan,
    ) {
        self.b.patch(0, self.t(augment_kw.id));
        self.b.patch(1, self.t(library_keyword.id));
        self.end_incl(NodeKind::LibraryAugmentation, semicolon);
    }

    fn begin_import(&mut self, token: TokenSpan) {
        self.begin(NodeKind::Import, token);
        self.import_open = true;
    }

    fn end_import(&mut self, _import_keyword: TokenSpan, semicolon: Option<TokenSpan>) {
        match semicolon {
            Some(semicolon) => self.end_incl(NodeKind::Import, semicolon),
            // Out-of-order-clause recovery took the tail: the node seals
            // after its last completed child; the recovery pass emits the
            // `RecoverImport` sentinel separately.
            None => self.end_last(NodeKind::Import),
        }
        self.import_open = false;
    }

    fn handle_import_prefix(&mut self, deferred_kw: Option<TokenSpan>, as_kw: Option<TokenSpan>) {
        if !self.import_open {
            return;
        }
        self.patch_token(0, as_kw);
        self.patch_token(1, deferred_kw);
    }

    fn handle_recover_import(&mut self, semicolon: Option<TokenSpan>) {
        // Recovery-pass sentinel (collapse map §3.1): anchored at the `;`
        // when the pass found one, zero-width at the current position
        // otherwise.
        if let Some(semicolon) = semicolon {
            self.b.begin(NodeKind::RecoverImport, self.t(semicolon.id));
            self.b.patch(0, self.t(semicolon.id));
            self.b
                .end(NodeKind::RecoverImport, self.t(semicolon.id) + 1);
        } else {
            let anchor = self.b.last_end_token().expect("recover import anchor");
            self.b.begin(NodeKind::RecoverImport, anchor);
            self.b.patch(0, TOKEN_NONE);
            self.b.end(NodeKind::RecoverImport, anchor);
        }
    }

    fn begin_conditional_uris(&mut self, _token: TokenSpan) {
        // Combinators/ConditionalUris/TypeList wrappers are dropped (collapse
        // map §3.1: End types D1 — the children delimit themselves).
    }

    fn end_conditional_uris(&mut self, _count: usize) {
        // See begin_conditional_uris: wrapper dropped.
    }

    fn begin_combinators(&mut self, _token: TokenSpan) {
        // Wrapper dropped (D1) — the Show/Hide children delimit themselves.
    }

    fn end_combinators(&mut self, _count: usize) {
        // See begin_combinators: wrapper dropped.
    }

    fn begin_conditional_uri(&mut self, if_kw: TokenSpan) {
        self.begin(NodeKind::ConditionalUri, if_kw);
    }

    fn end_conditional_uri(
        &mut self,
        _if_kw: TokenSpan,
        left_paren: TokenSpan,
        equal_sign: Option<TokenSpan>,
    ) {
        self.b.patch(0, self.t(left_paren.id));
        self.patch_token(1, equal_sign);
        self.end_last(NodeKind::ConditionalUri);
    }

    fn handle_dotted_name(&mut self, count: usize, first_identifier: TokenSpan) {
        // The chain's Identifiers completed before the event (children of the
        // enclosing directive); adopt them (collapse map §3.1 DottedName row).
        self.b.begin_adopting(
            NodeKind::DottedName,
            self.t(first_identifier.id),
            count as u32,
        );
        self.end_last(NodeKind::DottedName);
    }

    fn begin_export(&mut self, token: TokenSpan) {
        self.begin(NodeKind::Export, token);
    }

    fn end_export(&mut self, _export_keyword: TokenSpan, semicolon: TokenSpan) {
        self.end_incl(NodeKind::Export, semicolon);
    }

    fn begin_show(&mut self, show_keyword: TokenSpan) {
        self.begin(NodeKind::Show, show_keyword);
    }

    fn end_show(&mut self, _show_keyword: TokenSpan) {
        self.end_last(NodeKind::Show);
    }

    fn begin_hide(&mut self, hide_keyword: TokenSpan) {
        self.begin(NodeKind::Hide, hide_keyword);
    }

    fn end_hide(&mut self, _hide_keyword: TokenSpan) {
        self.end_last(NodeKind::Hide);
    }

    fn handle_identifier_list(&mut self, _count: usize) {
        // The name list's Identifier children delimit themselves (D1).
    }

    fn begin_part(&mut self, token: TokenSpan) {
        self.begin(NodeKind::Part, token);
    }

    fn end_part(&mut self, _part_keyword: TokenSpan, semicolon: TokenSpan) {
        self.end_incl(NodeKind::Part, semicolon);
    }

    fn begin_part_of(&mut self, token: TokenSpan) {
        self.begin(NodeKind::PartOf, token);
    }

    fn end_part_of(
        &mut self,
        _part_keyword: TokenSpan,
        of_kw: TokenSpan,
        semicolon: TokenSpan,
        has_name: bool,
    ) {
        self.b.patch(0, self.t(of_kw.id));
        self.b.patch(1, u32::from(has_name));
        self.end_incl(NodeKind::PartOf, semicolon);
    }

    // The script tag (`#!...`) is consumed by `parse_unit` without a listener
    // event, so the `ScriptTag` leaf is unproducible on this bridge; the
    // node needs a parser-side event (the only production scripts affected
    // are standalone `bin/` entrypoints no registered rule dispatches on).

    // —— metadata ————————————————————————————————————————————————————
    fn begin_metadata_star(&mut self, token: TokenSpan) {
        self.metadata_star = Some((self.t(token.id), self.b.len() as u32));
        self.begin(NodeKind::MetadataStar, token);
    }

    fn end_metadata_star(&mut self, _count: usize) {
        let (anchor, len_at_begin) = self.metadata_star.take().expect("metadata star pair");
        let end = if self.b.len() > len_at_begin as usize {
            self.b.last_end_token().expect("metadata star child")
        } else {
            // Empty star (the recovery path fires the pair back-to-back).
            anchor
        };
        self.b.end(NodeKind::MetadataStar, end);
    }

    fn begin_metadata(&mut self, token: TokenSpan) {
        self.begin(NodeKind::Metadata, token);
    }

    fn end_metadata(
        &mut self,
        _begin: TokenSpan,
        period_before_name: Option<TokenSpan>,
        _end: TokenSpan,
    ) {
        self.patch_token(0, period_before_name);
        self.end_last(NodeKind::Metadata);
    }

    // —— types ————————————————————————————————————————————————————————
    fn handle_type(&mut self, name: TokenSpan, question: Option<TokenSpan>) {
        // `Type` is width-0 (the `?` derives from the span tail, #174); the
        // extent includes the question mark when present.
        match question {
            Some(q) => self
                .b
                .leaf_span(NodeKind::Type, self.t(name.id), self.t(q.id) + 1),
            None => self
                .b
                .leaf_span(NodeKind::Type, self.t(name.id), self.t(name.id) + 1),
        }
    }

    fn handle_no_type(&mut self, last_consumed: TokenSpan) {
        self.b.leaf(NodeKind::NoType, self.t(last_consumed.id));
    }

    fn handle_qualified(&mut self, period: TokenSpan) {
        // Both Identifiers completed before the event; the marker's span
        // opens at the chain head and closes past the period (Handle =
        // childless, collapse map §3.5).
        let first = self.b.last_first_token().expect("qualified anchor");
        self.b.begin(NodeKind::Qualified, first);
        self.b.patch(0, self.t(period.id));
        self.b.end(NodeKind::Qualified, self.t(period.id) + 1);
    }

    fn handle_identifier(&mut self, token: TokenSpan, context: IdentifierContext) {
        self.b.begin(NodeKind::Identifier, self.t(token.id));
        self.b.patch(0, context as u32);
        self.b.end(NodeKind::Identifier, self.t(token.id) + 1);
    }

    fn begin_type_arguments(&mut self, token: TokenSpan) {
        self.begin(NodeKind::TypeArguments, token);
    }

    fn end_type_arguments(&mut self, _count: usize, _begin: TokenSpan, end: TokenSpan) {
        // The closer (`>` after `>>` splitting) is consumed — inclusive.
        self.end_incl(NodeKind::TypeArguments, end);
    }

    fn handle_no_type_arguments(&mut self, token: TokenSpan) {
        self.b.leaf(NodeKind::NoTypeArguments, self.t(token.id));
    }

    fn begin_type_variables(&mut self, token: TokenSpan) {
        self.begin(NodeKind::TypeVariables, token);
    }

    fn end_type_variables(&mut self, _begin: TokenSpan, end: TokenSpan) {
        self.end_incl(NodeKind::TypeVariables, end);
    }

    fn handle_no_type_variables(&mut self, token: TokenSpan) {
        self.b.leaf(NodeKind::NoTypeVariables, self.t(token.id));
    }

    fn handle_type_variables_defined(&mut self, _token: TokenSpan, _count: usize) {
        // Separator marker, count derivable from the children (D1).
    }

    fn begin_type_variable(&mut self, token: TokenSpan) {
        self.begin(NodeKind::TypeVariable, token);
    }

    fn end_type_variable(
        &mut self,
        _token: TokenSpan,
        _index: usize,
        extends_or_super: Option<TokenSpan>,
        variance: Option<TokenSpan>,
    ) {
        self.patch_token(0, extends_or_super);
        self.patch_token(1, variance);
        self.end_last(NodeKind::TypeVariable);
    }

    fn begin_record_type(&mut self, left_paren: TokenSpan) {
        self.begin(NodeKind::RecordType, left_paren);
    }

    fn end_record_type(
        &mut self,
        left_paren: TokenSpan,
        question: Option<TokenSpan>,
        _count: usize,
        _has_named_fields: bool,
    ) {
        self.b.patch(0, self.t(left_paren.id));
        self.patch_token(1, question);
        self.end_last(NodeKind::RecordType);
    }

    fn begin_record_type_entry(&mut self, first: TokenSpan) {
        self.begin(NodeKind::RecordTypeEntry, first);
    }

    fn end_record_type_entry(&mut self, last: TokenSpan) {
        self.end_incl(NodeKind::RecordTypeEntry, last);
    }

    fn handle_no_name(&mut self, token: TokenSpan) {
        self.b.leaf(NodeKind::NoName, self.t(token.id));
    }

    fn begin_record_type_named_fields(&mut self, left_brace: TokenSpan) {
        self.begin(NodeKind::RecordTypeNamedFields, left_brace);
    }

    fn end_record_type_named_fields(&mut self, _count: usize, left_brace: TokenSpan) {
        self.end_incl(NodeKind::RecordTypeNamedFields, left_brace);
    }

    fn begin_function_type(&mut self, begin: TokenSpan) {
        self.begin(NodeKind::FunctionType, begin);
    }

    fn end_function_type(&mut self, function: TokenSpan, question: Option<TokenSpan>) {
        self.b.patch(0, self.t(function.id));
        self.patch_token(1, question);
        self.end_last(NodeKind::FunctionType);
    }

    fn handle_void_keyword(&mut self, token: TokenSpan) {
        self.b.leaf(NodeKind::VoidType, self.t(token.id));
    }

    fn handle_void_keyword_with_type_arguments(&mut self, _token: TokenSpan) {
        // `void<T>` is always an error: the VoidType leaf plus the sibling
        // TypeArguments carry the shape; the diagnostic goes through the
        // recovery channel (collapse map §3.8 D3).
    }

    fn handle_experiment_not_enabled(
        &mut self,
        flag: ExperimentalFlag,
        begin: TokenSpan,
        end: TokenSpan,
    ) {
        let _ = end;
        self.b
            .begin(NodeKind::ExperimentNotEnabled, self.t(begin.id));
        self.b.patch(0, flag as u32);
        self.b
            .end(NodeKind::ExperimentNotEnabled, self.t(begin.id) + 1);
    }

    // —— declarations (top-level dispatch, #18) ——————————————————————————
    fn end_top_level_declaration(&mut self, end: TokenSpan) {
        // parse_unit fires this after every declaration; the recovery path
        // (no prelude begin) has nothing to close — tolerate the mismatch.
        // The bound is the next token (peek) — exclusive.
        if self.tld_open {
            self.end_at(NodeKind::TopLevelDeclaration, self.t(end.id));
            self.tld_open = false;
        }
    }

    fn handle_invalid_top_level_declaration(&mut self, end: TokenSpan) {
        self.b.leaf_span(
            NodeKind::InvalidTopLevelDeclaration,
            self.t(end.id),
            self.t(end.id) + 1,
        );
    }

    fn begin_uncategorized_top_level_declaration(&mut self, token: TokenSpan) {
        // Any of the five preludes opens the `TopLevelDeclaration` name
        // wrapper (collapse map §2 #1-5); the prelude begin itself is R1.
        self.begin(NodeKind::TopLevelDeclaration, token);
        self.tld_open = true;
    }

    fn begin_top_level_member(&mut self, token: TokenSpan) {
        self.begin(NodeKind::TopLevelDeclaration, token);
        self.tld_open = true;
    }

    fn begin_top_level_method(
        &mut self,
        last_consumed: TokenSpan,
        augment: Option<TokenSpan>,
        external: Option<TokenSpan>,
    ) {
        self.begin(NodeKind::TopLevelMethod, last_consumed);
        let mut bits = 0;
        if augment.is_some() {
            bits |= mod_bits::AUGMENT;
        }
        if external.is_some() {
            bits |= mod_bits::EXTERNAL;
        }
        self.set_mods(bits);
    }

    fn end_top_level_method(
        &mut self,
        _begin: TokenSpan,
        get_or_set: Option<TokenSpan>,
        end: TokenSpan,
    ) {
        self.patch_token(0, get_or_set);
        // `parse_function_body` returns its last consumed token — inclusive.
        self.end_incl(NodeKind::TopLevelMethod, end);
    }

    fn end_top_level_fields(
        &mut self,
        _augment_kw: Option<TokenSpan>,
        _abstract_kw: Option<TokenSpan>,
        _external_kw: Option<TokenSpan>,
        _static_kw: Option<TokenSpan>,
        _covariant_kw: Option<TokenSpan>,
        _late_kw: Option<TokenSpan>,
        _var_final_or_const: Option<TokenSpan>,
        _count: usize,
        _begin: TokenSpan,
        end: TokenSpan,
    ) {
        // Shared `Fields` begin (collapse map §2 #6): the word exists for
        // slot hygiene even though the narrower seal cannot read it.
        self.b.patch(0, DeclarationKind::TopLevel as u32);
        self.end_incl(NodeKind::TopLevelFields, end);
    }

    fn begin_typedef(&mut self, token: TokenSpan) {
        self.begin(NodeKind::Typedef, token);
    }

    fn end_typedef(
        &mut self,
        augment: Option<TokenSpan>,
        typedef: TokenSpan,
        equals: Option<TokenSpan>,
        end: TokenSpan,
    ) {
        if augment.is_some() {
            // `augment` precedes `typedef`, so the span head (the begin
            // token) is exactly `augment` — the ModSet head contract holds.
            self.set_mods(mod_bits::AUGMENT);
        }
        self.b.patch(0, self.t(typedef.id));
        self.patch_token(1, equals);
        self.end_incl(NodeKind::Typedef, end);
    }

    // —— class/mixin/extension/enum members (#18) ————————————————————————
    fn begin_member(&mut self) {
        // MemberEnd is a pure wrapper (D1); members are the body's direct
        // children matched by kind. Both events are no-ops.
    }

    fn end_member(&mut self) {
        // See begin_member: wrapper dropped.
    }

    fn handle_invalid_member(&mut self, end: TokenSpan) {
        self.b
            .leaf_span(NodeKind::InvalidMember, self.t(end.id), self.t(end.id) + 1);
    }

    fn begin_fields(
        &mut self,
        kind: DeclarationKind,
        augment_kw: Option<TokenSpan>,
        _abstract_kw: Option<TokenSpan>,
        external_kw: Option<TokenSpan>,
        static_kw: Option<TokenSpan>,
        covariant_kw: Option<TokenSpan>,
        late_kw: Option<TokenSpan>,
        var_final_or_const: Option<TokenSpan>,
        last_consumed: TokenSpan,
    ) {
        // Canonical open of the shared `Fields` group (§2 #6): the wider
        // `Fields` kind, sealed as `TopLevelFields` for top-level fields.
        let _ = kind;
        self.begin(NodeKind::Fields, last_consumed);
        self.set_mods(Self::member_bits(
            augment_kw,
            external_kw,
            static_kw,
            covariant_kw,
            late_kw,
            var_final_or_const,
        ));
    }

    fn end_fields(
        &mut self,
        _kind: DeclarationKind,
        _abstract_kw: Option<TokenSpan>,
        _augment_kw: Option<TokenSpan>,
        _external_kw: Option<TokenSpan>,
        static_kw: Option<TokenSpan>,
        _covariant_kw: Option<TokenSpan>,
        _late_kw: Option<TokenSpan>,
        _var_final_or_const: Option<TokenSpan>,
        _count: usize,
        _begin: TokenSpan,
        end: TokenSpan,
    ) {
        // Word 0 is the `MemberKind` discriminant (collapse map §3.3; the
        // official builder derives it the same way from the event's
        // declaration family + the static modifier, member_kind.dart —
        // fields only have the static/instance pair here; top-level fields
        // seal as `TopLevelFields` in `end_top_level_fields`).
        let member_kind = if static_kw.is_some() {
            MemberKind::StaticField
        } else {
            MemberKind::NonStaticField
        };
        self.b.patch(0, member_kind as u32);
        self.end_incl(NodeKind::Fields, end);
    }

    fn begin_constructor(
        &mut self,
        kind: DeclarationKind,
        augment_kw: Option<TokenSpan>,
        external_kw: Option<TokenSpan>,
        static_kw: Option<TokenSpan>,
        covariant_kw: Option<TokenSpan>,
        var_final_or_const: Option<TokenSpan>,
        _get_or_set: Option<TokenSpan>,
        _new_kw: Option<TokenSpan>,
        name: TokenSpan,
        _enclosing_declaration_name: Option<&str>,
    ) {
        // Anchored at the declared name — the event carries no head-modifier
        // token, so a modified constructor's flag bits do not satisfy the
        // first-modifier-is-span-head contract (the parser never passes the
        // constructor head; a known deviation on modified constructors).
        self.begin(NodeKind::Constructor, name);
        self.set_mods(Self::member_bits(
            augment_kw,
            external_kw,
            static_kw,
            covariant_kw,
            None,
            var_final_or_const,
        ));
        self.b.patch(0, kind as u32);
    }

    fn end_constructor(
        &mut self,
        _kind: DeclarationKind,
        _begin: TokenSpan,
        new_kw: Option<TokenSpan>,
        begin_param: TokenSpan,
        begin_initializers: Option<TokenSpan>,
        end: TokenSpan,
    ) {
        self.patch_token(1, new_kw);
        self.patch_token(2, Some(begin_param));
        self.patch_token(3, begin_initializers);
        self.end_incl(NodeKind::Constructor, end);
    }

    fn begin_method(
        &mut self,
        kind: DeclarationKind,
        augment_kw: Option<TokenSpan>,
        external_kw: Option<TokenSpan>,
        static_kw: Option<TokenSpan>,
        covariant_kw: Option<TokenSpan>,
        var_final_or_const: Option<TokenSpan>,
        _get_or_set: Option<TokenSpan>,
        name: TokenSpan,
        _enclosing_declaration_name: Option<&str>,
    ) {
        self.begin(NodeKind::Method, name);
        self.set_mods(Self::member_bits(
            augment_kw,
            external_kw,
            static_kw,
            covariant_kw,
            None,
            var_final_or_const,
        ));
        self.b.patch(0, kind as u32);
    }

    fn end_method(
        &mut self,
        _kind: DeclarationKind,
        get_or_set: Option<TokenSpan>,
        _begin: TokenSpan,
        begin_param: TokenSpan,
        begin_initializers: Option<TokenSpan>,
        end: TokenSpan,
    ) {
        self.patch_token(1, get_or_set);
        self.patch_token(2, Some(begin_param));
        self.patch_token(3, begin_initializers);
        self.end_incl(NodeKind::Method, end);
    }

    fn begin_factory(
        &mut self,
        kind: DeclarationKind,
        last_consumed: TokenSpan,
        augment_kw: Option<TokenSpan>,
        external_kw: Option<TokenSpan>,
        const_keyword: Option<TokenSpan>,
    ) {
        self.begin(NodeKind::Factory, last_consumed);
        let mut bits = 0;
        if augment_kw.is_some() {
            bits |= mod_bits::AUGMENT;
        }
        if external_kw.is_some() {
            bits |= mod_bits::EXTERNAL;
        }
        if const_keyword.is_some() {
            bits |= mod_bits::VAR_FINAL_OR_CONST;
        }
        self.set_mods(bits);
        self.b.patch(0, kind as u32);
    }

    fn end_factory(
        &mut self,
        _kind: DeclarationKind,
        _begin: TokenSpan,
        factory: TokenSpan,
        end: TokenSpan,
    ) {
        self.b.patch(1, self.t(factory.id));
        self.end_incl(NodeKind::Factory, end);
    }

    fn handle_operator_name(&mut self, operator: TokenSpan, name: TokenSpan) {
        // `operator` is the span head (head-when-present, #174); the words
        // are the name token and the invalid flag (§3.5 OperatorName).
        self.b.begin(NodeKind::OperatorName, self.t(operator.id));
        self.b.patch(0, self.t(name.id));
        self.b.patch(1, 0);
        self.b.end(NodeKind::OperatorName, self.t(name.id) + 1);
    }

    fn handle_invalid_operator_name(&mut self, operator: TokenSpan, name: TokenSpan) {
        self.b.begin(NodeKind::OperatorName, self.t(operator.id));
        self.b.patch(0, self.t(name.id));
        self.b.patch(1, 1);
        self.b.end(NodeKind::OperatorName, self.t(name.id) + 1);
    }

    fn handle_no_identifier(&mut self, token: TokenSpan, _context: IdentifierContext) {
        self.b.leaf(NodeKind::NoIdentifier, self.t(token.id));
    }

    // —— field initializers and constructor initializers —————————————————
    fn begin_field_initializer(&mut self, token: TokenSpan) {
        self.begin(NodeKind::FieldInitializer, token);
        self.b.patch(0, self.t(token.id));
    }

    fn end_field_initializer(&mut self, _assignment: TokenSpan, end: TokenSpan) {
        self.end_incl(NodeKind::FieldInitializer, end);
    }

    fn handle_no_field_initializer(&mut self, _token: TokenSpan) {
        // =init None derivable from the payload's absent initializer words (D1).
    }

    fn begin_initializers(&mut self, token: TokenSpan) {
        self.begin(NodeKind::Initializers, token);
    }

    fn end_initializers(&mut self, _count: usize, _begin: TokenSpan, last_end: TokenSpan) {
        self.end_incl(NodeKind::Initializers, last_end);
    }

    fn handle_no_initializers(&mut self) {
        // `:` absent marker (D1) — the Constructor payload's absent
        // begin_initializers word carries the fact.
    }

    // —— async modifiers and function bodies —————————————————————————————
    fn handle_async_modifier(&mut self, async_kw: Option<TokenSpan>, star: Option<TokenSpan>) {
        // The modifier event fires before the body node opens (the grammar
        // consumes `async` before `{`/`=>`), so the words ride to whichever
        // body kind absorbs them (`AsyncModifierHandle` F→, §3.9).
        self.async_pending = (async_kw.map(|t| self.t(t.id)), star.map(|t| self.t(t.id)));
    }

    fn begin_block_function_body(&mut self, token: TokenSpan) {
        self.begin(NodeKind::BlockBody, token);
        // The contract writes both words even when the modifier event never
        // fired (a sync body) — absent is the explicit sentinel.
        let (async_kw, star) = self.async_pending;
        self.async_pending = (None, None);
        self.b.patch(0, async_kw.unwrap_or(TOKEN_NONE));
        self.b.patch(1, star.unwrap_or(TOKEN_NONE));
    }

    fn end_block_function_body(&mut self, _count: usize, _begin: TokenSpan, end: TokenSpan) {
        self.end_incl(NodeKind::BlockBody, end);
    }

    fn handle_no_function_body(&mut self, token: TokenSpan) {
        self.b.leaf(NodeKind::EmptyBody, self.t(token.id));
    }

    fn handle_function_body_skipped(
        &mut self,
        begin: TokenSpan,
        end: TokenSpan,
        is_expression_body: bool,
    ) {
        self.b.begin(NodeKind::SkippedBody, self.t(begin.id));
        self.b.patch(0, u32::from(is_expression_body));
        self.b.end(NodeKind::SkippedBody, self.t(end.id) + 1);
    }

    fn handle_empty_function_body(&mut self, semicolon: TokenSpan) {
        self.b.leaf(NodeKind::EmptyBody, self.t(semicolon.id));
    }

    fn handle_invalid_function_body(&mut self, token: TokenSpan) {
        self.b.leaf(NodeKind::InvalidFunctionBody, self.t(token.id));
    }

    fn handle_expression_function_body(&mut self, arrow: TokenSpan, end: Option<TokenSpan>) {
        // The body's expression completed before this event (preceding
        // sibling — `ExpressionBody` is a childless Handle), and the async
        // words ride in from the modifier event. Word 1 is the `;` when the
        // member flavor sealed with one, else the absent sentinel.
        self.b.begin(NodeKind::ExpressionBody, self.t(arrow.id));
        self.b.patch(0, self.t(arrow.id));
        self.patch_token(1, end);
        self.b
            .patch(2, self.async_pending.0.take().unwrap_or(TOKEN_NONE));
        self.b
            .patch(3, self.async_pending.1.take().unwrap_or(TOKEN_NONE));
        match end {
            Some(end) => self.end_incl(NodeKind::ExpressionBody, end),
            None => self.b.end(NodeKind::ExpressionBody, self.t(arrow.id) + 1),
        }
    }

    fn handle_native_clause(&mut self, native: TokenSpan, _has_name: bool) {
        // Childless Handle; the name string completed before the event
        // (preceding sibling), the span covers it.
        let end = self.b.last_end_token().unwrap_or(self.t(native.id) + 1);
        self.b.leaf_span(
            NodeKind::NativeClause,
            self.t(native.id),
            end.max(self.t(native.id) + 1),
        );
    }

    fn handle_native_function_body_skipped(&mut self, native: TokenSpan, semicolon: TokenSpan) {
        self.b.leaf_span(
            NodeKind::NativeFunctionBody,
            self.t(native.id),
            self.t(semicolon.id) + 1,
        );
    }

    fn handle_native_function_body_ignored(&mut self, native: TokenSpan, next: TokenSpan) {
        self.b.leaf_span(
            NodeKind::NativeFunctionBody,
            self.t(native.id),
            self.t(next.id),
        );
    }

    // —— declaration headers —————————————————————————————————————————————
    fn begin_type_list(&mut self, _token: TokenSpan) {
        // TypeListEnd D1: the extends/with/implements lists are delimited by
        // the declaration's payload words.
    }

    fn end_type_list(&mut self, _count: usize) {
        // See begin_type_list: wrapper dropped.
    }

    fn handle_named_mixin_application_with_clause(&mut self, with_keyword: TokenSpan) {
        self.b.patch(2, self.t(with_keyword.id));
    }

    fn handle_class_with_clause(&mut self, with_keyword: TokenSpan) {
        self.b.patch(2, self.t(with_keyword.id));
    }

    fn handle_class_no_with_clause(&mut self) {
        self.b.patch(2, TOKEN_NONE);
    }

    fn handle_enum_with_clause(&mut self, _with_keyword: TokenSpan) {
        // Enums have no with clause; the event exists only for the grammar
        // error — diagnostic face (collapse map §3.2 D3).
    }

    fn handle_enum_no_with_clause(&mut self) {
        // Absence marker; nothing to record (D1).
    }

    fn handle_implements(&mut self, implements: Option<TokenSpan>, _count: usize) {
        // The implements word exists on `ClassDeclaration` and
        // `NamedMixinApplication` only; the mixin family has none (its
        // errored clause types stay as children without a word).
        if matches!(
            self.class_decl,
            ClassDecl::Class | ClassDecl::NamedMixinApplication
        ) {
            self.patch_token(3, implements);
        }
    }

    fn handle_class_extends(&mut self, extends: Option<TokenSpan>, _count: usize) {
        if self.class_decl == ClassDecl::Class {
            self.patch_token(1, extends);
        }
    }

    fn handle_class_header(
        &mut self,
        _begin: TokenSpan,
        class: TokenSpan,
        _native: Option<TokenSpan>,
    ) {
        // Word 0 of `ClassDeclaration` is the `class` keyword (the native
        // clause rides its own event).
        if self.class_decl == ClassDecl::Class {
            self.b.patch(0, self.t(class.id));
        }
    }

    fn handle_mixin_header(&mut self, _mixin: TokenSpan) {
        // Token already in the span/words (D1).
    }

    fn handle_mixin_on(&mut self, on: Option<TokenSpan>, _count: usize) {
        if self.class_decl == ClassDecl::Mixin {
            self.patch_token(1, on);
        }
    }

    fn handle_recover_declaration_header(&mut self, kind: DeclarationHeaderKind) {
        let discriminant = match kind {
            DeclarationHeaderKind::Class => HEADER_KIND_CLASS,
            DeclarationHeaderKind::ExtensionType => HEADER_KIND_EXTENSION_TYPE,
        };
        self.recover_header(discriminant);
    }

    fn handle_recover_mixin_header(&mut self) {
        // `RecoverMixinHeaderHandle` merges into `RecoverDeclarationHeader`
        // with the Mixin kind (collapse map §3.2).
        self.recover_header(HEADER_KIND_MIXIN);
    }

    // —— class declarations ————————————————————————————————————————————
    fn begin_class_or_mixin_or_named_mixin_application_prelude(&mut self, token: TokenSpan) {
        self.begin(NodeKind::TopLevelDeclaration, token);
        self.tld_open = true;
    }

    fn begin_class_declaration(
        &mut self,
        begin: TokenSpan,
        abstract_kw: Option<TokenSpan>,
        sealed_kw: Option<TokenSpan>,
        base_kw: Option<TokenSpan>,
        interface_kw: Option<TokenSpan>,
        final_kw: Option<TokenSpan>,
        augment_kw: Option<TokenSpan>,
        mixin_kw: Option<TokenSpan>,
        _name: TokenSpan,
    ) {
        self.begin(NodeKind::ClassDeclaration, begin);
        let mut bits = 0;
        if augment_kw.is_some() {
            bits |= mod_bits::AUGMENT;
        }
        if abstract_kw.is_some() {
            bits |= mod_bits::ABSTRACT;
        }
        if sealed_kw.is_some() {
            bits |= mod_bits::SEALED;
        }
        if base_kw.is_some() {
            bits |= mod_bits::BASE;
        }
        if interface_kw.is_some() {
            bits |= mod_bits::INTERFACE;
        }
        if final_kw.is_some() {
            bits |= mod_bits::FINAL;
        }
        if mixin_kw.is_some() {
            bits |= mod_bits::MIXIN;
        }
        self.set_mods(bits);
        self.class_decl = ClassDecl::Class;
    }

    fn end_class_declaration(&mut self, _begin: TokenSpan, end: TokenSpan) {
        self.class_decl = ClassDecl::None;
        // The body's `}` completed inside — the bound is the next token.
        self.end_at(NodeKind::ClassDeclaration, self.t(end.id));
    }

    fn begin_named_mixin_application(
        &mut self,
        begin: TokenSpan,
        abstract_kw: Option<TokenSpan>,
        sealed_kw: Option<TokenSpan>,
        base_kw: Option<TokenSpan>,
        interface_kw: Option<TokenSpan>,
        final_kw: Option<TokenSpan>,
        augment_kw: Option<TokenSpan>,
        mixin_kw: Option<TokenSpan>,
        _name: TokenSpan,
    ) {
        self.begin(NodeKind::NamedMixinApplication, begin);
        let mut bits = 0;
        if augment_kw.is_some() {
            bits |= mod_bits::AUGMENT;
        }
        if abstract_kw.is_some() {
            bits |= mod_bits::ABSTRACT;
        }
        if sealed_kw.is_some() {
            bits |= mod_bits::SEALED;
        }
        if base_kw.is_some() {
            bits |= mod_bits::BASE;
        }
        if interface_kw.is_some() {
            bits |= mod_bits::INTERFACE;
        }
        if final_kw.is_some() {
            bits |= mod_bits::FINAL;
        }
        if mixin_kw.is_some() {
            bits |= mod_bits::MIXIN;
        }
        self.set_mods(bits);
        self.class_decl = ClassDecl::NamedMixinApplication;
    }

    fn end_named_mixin_application(
        &mut self,
        _begin: TokenSpan,
        class: TokenSpan,
        equals: TokenSpan,
        implements: Option<TokenSpan>,
        end: TokenSpan,
    ) {
        self.b.patch(0, self.t(class.id));
        self.b.patch(1, self.t(equals.id));
        self.patch_token(3, implements);
        self.class_decl = ClassDecl::None;
        self.end_incl(NodeKind::NamedMixinApplication, end);
    }

    fn begin_class_or_mixin_or_extension_body(&mut self, _kind: DeclarationKind, token: TokenSpan) {
        self.begin(NodeKind::DeclarationBody, token);
    }

    fn end_class_or_mixin_or_extension_body(
        &mut self,
        _kind: DeclarationKind,
        _count: usize,
        _begin: TokenSpan,
        end: TokenSpan,
    ) {
        // The bound is the token after `}` (peek) — exclusive.
        self.end_at(NodeKind::DeclarationBody, self.t(end.id));
    }

    fn handle_no_class_body(&mut self, _semicolon: TokenSpan) {
        // `class C;`: the No*Body shape is the span tail `;` (D2); no node.
    }

    // —— mixin declarations —————————————————————————————————————————————
    fn begin_mixin_declaration(
        &mut self,
        begin: TokenSpan,
        augment_kw: Option<TokenSpan>,
        base_kw: Option<TokenSpan>,
        mixin: TokenSpan,
        _name: TokenSpan,
    ) {
        self.begin(NodeKind::MixinDeclaration, begin);
        let mut bits = 0;
        if augment_kw.is_some() {
            bits |= mod_bits::AUGMENT;
        }
        if base_kw.is_some() {
            bits |= mod_bits::BASE;
        }
        self.set_mods(bits);
        self.b.patch(0, self.t(mixin.id));
        self.class_decl = ClassDecl::Mixin;
    }

    fn end_mixin_declaration(&mut self, _begin: TokenSpan, end: TokenSpan) {
        // No with-clause event fires on the happy path; the word is written
        // explicitly here.
        self.b.patch(2, TOKEN_NONE);
        self.class_decl = ClassDecl::None;
        self.end_at(NodeKind::MixinDeclaration, self.t(end.id));
    }

    fn handle_no_mixin_body(&mut self, _semicolon: TokenSpan) {
        // Span-tail `;` shape (D2); no node.
    }

    // —— enum declarations ——————————————————————————————————————————————
    fn begin_enum_declaration_prelude(&mut self, enum_keyword: TokenSpan) {
        self.begin(NodeKind::TopLevelDeclaration, enum_keyword);
        self.tld_open = true;
    }

    fn begin_enum_declaration(
        &mut self,
        begin: TokenSpan,
        augment_kw: Option<TokenSpan>,
        enum_keyword: TokenSpan,
        _name: TokenSpan,
    ) {
        self.begin(NodeKind::EnumDeclaration, begin);
        if augment_kw.is_some() {
            self.set_mods(mod_bits::AUGMENT);
        }
        self.b.patch(0, self.t(enum_keyword.id));
    }

    fn end_enum_declaration(
        &mut self,
        _begin: TokenSpan,
        _enum_keyword: TokenSpan,
        _left_brace: TokenSpan,
        _member_count: usize,
        end: TokenSpan,
    ) {
        // Word 2 (elements end) is applied here: handle_enum_elements fires
        // while the EnumBody wrapper is still open, so patching there would
        // hit the wrong node's slot.
        self.b
            .patch(2, self.enum_elements_end.take().unwrap_or(TOKEN_NONE));
        self.end_at(NodeKind::EnumDeclaration, self.t(end.id));
    }

    fn handle_enum_header(
        &mut self,
        _augment: Option<TokenSpan>,
        _enum_keyword: TokenSpan,
        left_brace: TokenSpan,
    ) {
        self.b.patch(1, self.t(left_brace.id));
    }

    fn begin_enum_body(&mut self, token: TokenSpan) {
        self.begin(NodeKind::EnumBody, token);
    }

    fn end_enum_body(&mut self, _begin: TokenSpan, end: TokenSpan) {
        self.end_incl(NodeKind::EnumBody, end);
    }

    fn handle_enum_elements(&mut self, elements_end: TokenSpan, _count: usize) {
        // EnumDeclaration is not the innermost open node here (EnumBody is);
        // the word 2 write is deferred to end_enum_declaration.
        self.enum_elements_end = Some(self.t(elements_end.id));
    }

    fn handle_enum_element(&mut self, begin: TokenSpan, augment: Option<TokenSpan>) {
        // Childless Handle over the element extent; the name/arguments
        // completed before the event (preceding siblings).
        self.b.begin(NodeKind::EnumElement, self.t(begin.id));
        self.b.patch(0, self.t(begin.id));
        self.patch_token(1, augment);
        let end = self.b.last_end_token().unwrap_or(self.t(begin.id) + 1);
        self.b
            .end(NodeKind::EnumElement, end.max(self.t(begin.id) + 1));
    }

    fn handle_no_enum_body(&mut self, _semicolon: TokenSpan) {
        // `enum E;`: span-tail `;` shape (D2); no node.
    }

    // —— extension and extension type declarations ————————————————————————
    fn begin_extension_declaration_prelude(&mut self, extension_keyword: TokenSpan) {
        self.begin(NodeKind::TopLevelDeclaration, extension_keyword);
        self.tld_open = true;
    }

    fn begin_extension_declaration(
        &mut self,
        augment: Option<TokenSpan>,
        extension_keyword: TokenSpan,
        name: Option<TokenSpan>,
    ) {
        self.begin(NodeKind::ExtensionDeclaration, extension_keyword);
        if augment.is_some() {
            // `augment` precedes `extension`, so the head contract holds.
            self.set_mods(mod_bits::AUGMENT);
        }
        self.b.patch(0, self.t(extension_keyword.id));
        self.patch_token(1, name);
    }

    fn end_extension_declaration(
        &mut self,
        _begin: TokenSpan,
        _extension_keyword: TokenSpan,
        on: Option<TokenSpan>,
        end: TokenSpan,
    ) {
        self.patch_token(2, on);
        self.end_at(NodeKind::ExtensionDeclaration, self.t(end.id));
    }

    fn handle_no_extension_body(&mut self, _semicolon: TokenSpan) {
        // Span-tail `;` shape (D2); no node.
    }

    fn begin_extension_type_declaration(
        &mut self,
        augment_kw: Option<TokenSpan>,
        extension_keyword: TokenSpan,
        name: TokenSpan,
    ) {
        self.begin(NodeKind::ExtensionTypeDeclaration, extension_keyword);
        if augment_kw.is_some() {
            self.set_mods(mod_bits::AUGMENT);
        }
        self.b.patch(0, self.t(extension_keyword.id));
        self.b.patch(2, self.t(name.id));
    }

    fn end_extension_type_declaration(
        &mut self,
        _begin: TokenSpan,
        _augment_kw: Option<TokenSpan>,
        _extension_keyword: TokenSpan,
        type_keyword: TokenSpan,
        end: TokenSpan,
    ) {
        self.b.patch(1, self.t(type_keyword.id));
        self.end_at(NodeKind::ExtensionTypeDeclaration, self.t(end.id));
    }

    fn handle_no_extension_type_body(&mut self, _semicolon: TokenSpan) {
        // Span-tail `;` shape (D2); no node.
    }

    fn begin_redirecting_factory_body(&mut self, equals: TokenSpan) {
        self.begin(NodeKind::RedirectingFactoryBody, equals);
    }

    fn end_redirecting_factory_body(&mut self, _equals: TokenSpan, end: TokenSpan) {
        self.end_incl(NodeKind::RedirectingFactoryBody, end);
    }

    fn handle_no_type_name_in_constructor_reference(&mut self, token: TokenSpan) {
        self.b.leaf(NodeKind::NoTypeNameInCtorRef, self.t(token.id));
    }

    fn handle_mixin_with_clause(&mut self, with_keyword: TokenSpan) {
        // Invalid mixin with-clause recovery: the `MixinDeclaration` with
        // word back-fills (collapse map §3.2 MixinWithClauseHandle F→).
        if self.class_decl == ClassDecl::Mixin {
            self.b.patch(2, self.t(with_keyword.id));
        }
    }

    fn begin_primary_constructor_body(&mut self, token: TokenSpan, augment: Option<TokenSpan>) {
        self.begin(NodeKind::PrimaryConstructorBody, token);
        self.b.patch(0, self.t(token.id));
        self.patch_token(2, augment);
    }

    fn end_primary_constructor_body(
        &mut self,
        _begin: TokenSpan,
        _this: TokenSpan,
        begin_initializers: Option<TokenSpan>,
        end: TokenSpan,
    ) {
        self.patch_token(1, begin_initializers);
        // The bound is the next token (peek) — exclusive.
        self.end_at(NodeKind::PrimaryConstructorBody, self.t(end.id));
    }

    // —— primary constructors ———————————————————————————————————————————
    fn begin_primary_constructor(&mut self, begin: TokenSpan) {
        self.begin(NodeKind::PrimaryConstructor, begin);
    }

    fn end_primary_constructor(
        &mut self,
        kind: DeclarationKind,
        _begin: TokenSpan,
        end: TokenSpan,
        const_keyword: Option<TokenSpan>,
        has_constructor_name: bool,
    ) {
        self.b.patch(0, kind as u32);
        self.patch_token(1, const_keyword);
        self.b.patch(2, u32::from(has_constructor_name));
        self.end_at(NodeKind::PrimaryConstructor, self.t(end.id));
    }

    fn handle_no_primary_constructor(
        &mut self,
        _kind: DeclarationKind,
        token: TokenSpan,
        const_keyword: Option<TokenSpan>,
    ) {
        self.b
            .begin(NodeKind::NoPrimaryConstructor, self.t(token.id));
        self.patch_token(0, const_keyword);
        self.b
            .end(NodeKind::NoPrimaryConstructor, self.t(token.id) + 1);
    }

    // —— formal parameters ——————————————————————————————————————————————
    fn begin_formal_parameters(&mut self, token: TokenSpan, _kind: dartq_parser::MemberKind) {
        self.begin(NodeKind::FormalParameters, token);
    }

    fn end_formal_parameters(
        &mut self,
        _count: usize,
        _begin: TokenSpan,
        end: TokenSpan,
        _kind: dartq_parser::MemberKind,
    ) {
        self.end_incl(NodeKind::FormalParameters, end);
    }

    fn begin_optional_formal_parameters(&mut self, token: TokenSpan) {
        self.begin(NodeKind::OptionalFormalParameters, token);
    }

    fn end_optional_formal_parameters(
        &mut self,
        _count: usize,
        _begin: TokenSpan,
        end: TokenSpan,
        _kind: dartq_parser::MemberKind,
    ) {
        self.end_incl(NodeKind::OptionalFormalParameters, end);
    }

    fn begin_formal_parameter(
        &mut self,
        token: TokenSpan,
        _kind: dartq_parser::MemberKind,
        required: Option<TokenSpan>,
        covariant_kw: Option<TokenSpan>,
        var_final_or_const: Option<TokenSpan>,
    ) {
        self.begin(NodeKind::FormalParameter, token);
        let mut bits = 0;
        if required.is_some() {
            bits |= mod_bits::REQUIRED;
        }
        if covariant_kw.is_some() {
            bits |= mod_bits::COVARIANT;
        }
        if var_final_or_const.is_some() {
            bits |= mod_bits::VAR_FINAL_OR_CONST;
        }
        self.set_mods(bits);
    }

    fn begin_function_typed_formal_parameter(&mut self, token: TokenSpan) {
        self.begin(NodeKind::FunctionTypedFormalParameter, token);
    }

    fn end_function_typed_formal_parameter(
        &mut self,
        _token: TokenSpan,
        question: Option<TokenSpan>,
    ) {
        self.patch_token(0, question);
        self.end_last(NodeKind::FunctionTypedFormalParameter);
    }

    fn begin_formal_parameter_default_value_expression(&mut self, _token: TokenSpan) {
        // `FormalParameterDefaultValueExpressionEnd` is D1: the default
        // value expression is delimited by the FormalParameter init words.
    }

    fn end_formal_parameter_default_value_expression(&mut self, _token: TokenSpan) {
        // See begin_formal_parameter_default_value_expression: dropped.
    }

    fn end_formal_parameter(
        &mut self,
        _var_or_final: Option<TokenSpan>,
        this_keyword: Option<TokenSpan>,
        super_keyword: Option<TokenSpan>,
        _period_after_this_or_super: Option<TokenSpan>,
        name: TokenSpan,
        initializer_start: Option<TokenSpan>,
        initializer_end: Option<TokenSpan>,
        kind: FormalParameterKind,
        _member_kind: dartq_parser::MemberKind,
    ) {
        // `receiver` merges the mutually exclusive this/super tokens; the
        // extent includes the default value when present (its expression is
        // a child delimited by the init words).
        self.patch_token(0, this_keyword.or(super_keyword));
        self.b.patch(1, self.t(name.id));
        self.patch_token(2, initializer_start);
        self.patch_token(3, initializer_end);
        self.b.patch(4, kind as u32);
        match initializer_end {
            Some(end) => self.end_incl(NodeKind::FormalParameter, end),
            None => self.end_incl(NodeKind::FormalParameter, name),
        }
    }

    fn handle_valued_formal_parameter(
        &mut self,
        _equals: TokenSpan,
        _end: TokenSpan,
        _kind: FormalParameterKind,
    ) {
        // =init Some derivable from the init words (D1).
    }

    fn handle_formal_parameter_without_value(&mut self, _token: TokenSpan) {
        // =init None derivable (D1).
    }

    fn handle_no_formal_parameters(&mut self, token: TokenSpan, _kind: dartq_parser::MemberKind) {
        self.b.leaf(NodeKind::NoFormalParameters, self.t(token.id));
    }

    // —— statements (#19) ————————————————————————————————————————————————
    fn begin_block(&mut self, token: TokenSpan, _kind: dartq_parser::BlockKind) {
        self.begin(NodeKind::Block, token);
    }

    fn end_block(
        &mut self,
        _count: usize,
        _begin: TokenSpan,
        end: TokenSpan,
        _kind: dartq_parser::BlockKind,
    ) {
        // The `}` was consumed by the parser before the event — inclusive.
        self.end_incl(NodeKind::Block, end);
    }

    fn handle_expression_statement(&mut self, begin: TokenSpan, end: TokenSpan) {
        // Span-only leaf covering the statement through its `;`.
        self.b.leaf_span(
            NodeKind::ExpressionStatement,
            self.t(begin.id),
            self.t(end.id) + 1,
        );
    }

    fn handle_empty_statement(&mut self, token: TokenSpan) {
        self.b.leaf(NodeKind::EmptyStatement, self.t(token.id));
    }

    fn handle_invalid_statement(&mut self, begin: TokenSpan) {
        self.b.leaf_span(
            NodeKind::InvalidStatement,
            self.t(begin.id),
            self.t(begin.id) + 1,
        );
    }

    fn begin_if_statement(&mut self, token: TokenSpan) {
        self.begin(NodeKind::IfStatement, token);
    }

    fn begin_then_statement(&mut self, _token: TokenSpan) {
        // ThenStatementEnd D1: the then-branch follows the condition child.
    }

    fn end_then_statement(&mut self, _begin: TokenSpan, _end: TokenSpan) {
        // See begin_then_statement: wrapper dropped.
    }

    fn begin_else_statement(&mut self, _token: TokenSpan) {
        // ElseStatementEnd D1: the else-branch is delimited by the `else`
        // payload word.
    }

    fn end_else_statement(&mut self, _begin: TokenSpan, _end: TokenSpan) {
        // See begin_else_statement: wrapper dropped.
    }

    fn end_if_statement(&mut self, _token: TokenSpan, else_kw: Option<TokenSpan>, end: TokenSpan) {
        self.patch_token(0, else_kw);
        self.end_incl(NodeKind::IfStatement, end);
    }

    fn begin_for_statement(&mut self, token: TokenSpan) {
        // Canonical open of the shared for/for-in group (§2 #7): a for-in
        // body seals the same node as `ForIn`.
        self.begin(NodeKind::ForStatement, token);
    }

    fn handle_for_initializer_empty_statement(&mut self, _token: TokenSpan) {
        // Initializer shape derivable from the first child kind (D1).
    }

    fn handle_for_initializer_expression_statement(&mut self, _token: TokenSpan, _for_in: bool) {
        // ForIn vs ForParts is pinned by the parts variant itself (D1).
    }

    fn handle_for_initializer_local_variable_declaration(
        &mut self,
        _token: TokenSpan,
        _for_in: bool,
    ) {
        // See handle_for_initializer_expression_statement (D1).
    }

    fn handle_for_initializer_pattern_variable_assignment(
        &mut self,
        _pattern_keyword: TokenSpan,
        _equals: TokenSpan,
    ) {
        // See handle_for_initializer_expression_statement (D1).
    }

    fn begin_for_statement_body(&mut self, _token: TokenSpan) {
        // ForStatementBodyEnd D1: the body follows the parts child.
    }

    fn end_for_statement_body(&mut self, _end: TokenSpan) {
        // See begin_for_statement_body: wrapper dropped.
    }

    fn end_for_statement(&mut self, end: TokenSpan) {
        self.end_at(NodeKind::ForStatement, self.t(end.id));
    }

    fn handle_for_loop_parts(
        &mut self,
        for_kw: TokenSpan,
        left_paren: TokenSpan,
        left_separator: TokenSpan,
        right_separator: TokenSpan,
        expression_count: usize,
    ) {
        // Childless Fielded leaf; `for` is the span head (#174).
        self.b.begin(NodeKind::ForParts, self.t(for_kw.id));
        self.b.patch(0, self.t(left_paren.id));
        self.b.patch(1, self.t(left_separator.id));
        self.b.patch(2, self.t(right_separator.id));
        self.b
            .patch(3, expression_count.min(u32::MAX as usize) as u32);
        let end = self
            .b
            .last_end_token()
            .unwrap_or(self.t(right_separator.id) + 1);
        self.b
            .end(NodeKind::ForParts, end.max(self.t(right_separator.id) + 1));
    }

    fn begin_for_in_body(&mut self, _token: TokenSpan) {
        // ForInBodyEnd D1: the body follows the parts child.
    }

    fn end_for_in_body(&mut self, _end: TokenSpan) {
        // See begin_for_in_body: wrapper dropped.
    }

    fn end_for_in(&mut self, end: TokenSpan) {
        // Seals the shared begin opened as `ForStatement` (§2 #7).
        self.end_at(NodeKind::ForIn, self.t(end.id));
    }

    fn handle_for_in_loop_parts(
        &mut self,
        await_kw: Option<TokenSpan>,
        for_kw: TokenSpan,
        left_paren: TokenSpan,
        pattern_keyword: Option<TokenSpan>,
        in_kw: TokenSpan,
    ) {
        // Childless Fielded leaf; `await` present ⇒ it is the span head and
        // `for` follows it (`derived::for_token`).
        let head = await_kw.unwrap_or(for_kw);
        self.b.begin(NodeKind::ForInParts, self.t(head.id));
        self.patch_token(0, await_kw);
        self.b.patch(1, self.t(left_paren.id));
        self.patch_token(2, pattern_keyword);
        self.b.patch(3, self.t(in_kw.id));
        let end = self.b.last_end_token().unwrap_or(self.t(in_kw.id) + 1);
        self.b
            .end(NodeKind::ForInParts, end.max(self.t(in_kw.id) + 1));
    }

    fn begin_for_in_expression(&mut self, _token: TokenSpan) {
        // ForInExpressionEnd D1: the iterable is the last segment before the
        // parts child.
    }

    fn end_for_in_expression(&mut self, _end: TokenSpan) {
        // See begin_for_in_expression: wrapper dropped.
    }

    fn begin_while_statement(&mut self, token: TokenSpan) {
        self.begin(NodeKind::WhileStatement, token);
    }

    fn begin_while_statement_body(&mut self, _token: TokenSpan) {
        // WhileStatementBodyEnd D1.
    }

    fn end_while_statement_body(&mut self, _end: TokenSpan) {
        // See begin_while_statement_body: wrapper dropped.
    }

    fn end_while_statement(&mut self, _token: TokenSpan, end: TokenSpan) {
        // The bound is the token after the body (peek) — exclusive.
        self.end_at(NodeKind::WhileStatement, self.t(end.id));
    }

    fn begin_do_while_statement(&mut self, token: TokenSpan) {
        self.begin(NodeKind::DoWhileStatement, token);
    }

    fn begin_do_while_statement_body(&mut self, _token: TokenSpan) {
        // DoWhileStatementBodyEnd D1.
    }

    fn end_do_while_statement_body(&mut self, _end: TokenSpan) {
        // See begin_do_while_statement_body: wrapper dropped.
    }

    fn end_do_while_statement(&mut self, _token: TokenSpan, _while_kw: TokenSpan, end: TokenSpan) {
        self.end_incl(NodeKind::DoWhileStatement, end);
    }

    fn begin_return_statement(&mut self, token: TokenSpan) {
        self.begin(NodeKind::ReturnStatement, token);
    }

    fn end_return_statement(&mut self, _has_expression: bool, _begin: TokenSpan, end: TokenSpan) {
        self.end_incl(NodeKind::ReturnStatement, end);
    }

    fn begin_yield_statement(&mut self, token: TokenSpan) {
        self.begin(NodeKind::YieldStatement, token);
    }

    fn end_yield_statement(&mut self, _begin: TokenSpan, star: Option<TokenSpan>, end: TokenSpan) {
        self.patch_token(0, star);
        self.end_incl(NodeKind::YieldStatement, end);
    }

    fn end_invalid_yield_statement(
        &mut self,
        _begin: TokenSpan,
        star: Option<TokenSpan>,
        end: TokenSpan,
    ) {
        // The error code is resolution-domain, not an AST fact (§2 #11).
        self.patch_token(0, star);
        self.end_incl(NodeKind::InvalidYieldStatement, end);
    }

    fn begin_labeled_statement(&mut self, token: TokenSpan, label_count: usize) {
        // Each label contributed an Identifier + `Label` leaf pair before
        // the event; adopt 2·label_count siblings as the leading children.
        self.b.begin_adopting(
            NodeKind::LabeledStatement,
            self.t(token.id),
            (label_count * 2) as u32,
        );
    }

    fn end_labeled_statement(&mut self, _label_count: usize) {
        self.end_last(NodeKind::LabeledStatement);
    }

    fn handle_label(&mut self, token: TokenSpan) {
        self.b.leaf(NodeKind::Label, self.t(token.id));
    }

    fn begin_local_function_declaration(&mut self, token: TokenSpan) {
        self.begin(NodeKind::LocalFunctionDeclaration, token);
    }

    fn end_local_function_declaration(&mut self, end: TokenSpan) {
        self.end_incl(NodeKind::LocalFunctionDeclaration, end);
    }

    fn begin_function_name(&mut self, token: TokenSpan) {
        self.begin(NodeKind::FunctionName, token);
    }

    fn end_function_name(
        &mut self,
        _begin: TokenSpan,
        name_end: TokenSpan,
        is_function_expression: bool,
    ) {
        self.b.patch(0, u32::from(is_function_expression));
        // `name_end` is the token after the name (peek) — exclusive.
        self.end_at(NodeKind::FunctionName, self.t(name_end.id));
    }

    fn begin_variables_declaration(
        &mut self,
        token: TokenSpan,
        late_kw: Option<TokenSpan>,
        _var_final_or_const: Option<TokenSpan>,
    ) {
        self.begin(NodeKind::VariablesDeclaration, token);
        self.patch_token(0, late_kw);
    }

    fn end_variables_declaration(&mut self, _count: usize, end: Option<TokenSpan>) {
        match end {
            Some(end) => self.end_incl(NodeKind::VariablesDeclaration, end),
            None => self.end_last(NodeKind::VariablesDeclaration),
        }
    }

    fn begin_initialized_identifier(&mut self, token: TokenSpan) {
        // Width-0: the name is the span head (#174).
        self.begin(NodeKind::InitializedIdentifier, token);
    }

    fn end_initialized_identifier(&mut self, name: TokenSpan) {
        // Tail = the initializer subtree when one parsed; otherwise the name
        // itself bounds the node. Sealed state alone can regress behind the
        // begin: a recovered name returns a span with no identifier event
        // (recovery `ensure_identifier` flavors), so nothing seals at the
        // name and the last seal sits before it (#654 class).
        let name_end = self.t(name.id) + 1;
        let end = self
            .b
            .last_end_token()
            .map_or(name_end, |tail| tail.max(name_end));
        self.end_at(NodeKind::InitializedIdentifier, end);
    }

    fn handle_no_variable_initializer(&mut self, _token: TokenSpan) {
        // =init None derivable from the child shape (D1).
    }

    fn begin_variable_initializer(&mut self, token: TokenSpan) {
        // Width-0: the operator is the span head (#174).
        self.begin(NodeKind::VariableInitializer, token);
    }

    fn end_variable_initializer(&mut self, _token: TokenSpan) {
        self.end_last(NodeKind::VariableInitializer);
    }

    fn begin_try_statement(&mut self, token: TokenSpan) {
        self.begin(NodeKind::TryStatement, token);
    }

    fn end_try_statement(
        &mut self,
        _count: usize,
        _begin: TokenSpan,
        finally_kw: Option<TokenSpan>,
        end: TokenSpan,
    ) {
        self.patch_token(0, finally_kw);
        self.end_incl(NodeKind::TryStatement, end);
    }

    fn begin_catch_clause(&mut self, token: TokenSpan) {
        self.begin(NodeKind::CatchClause, token);
        self.catch_open = true;
    }

    fn end_catch_clause(&mut self, _token: TokenSpan) {
        // Deferred: `handle_catch_block` fires after the catch body parsed
        // and carries the payload words; the seal happens there.
    }

    fn handle_catch_block(
        &mut self,
        on_kw: Option<TokenSpan>,
        catch_kw: Option<TokenSpan>,
        comma: Option<TokenSpan>,
    ) {
        if !self.catch_open {
            return;
        }
        self.catch_open = false;
        // `on` is head-when-present (the span head when the on-form ran);
        // the payload words are `catch` and `comma`.
        self.patch_token(0, catch_kw);
        self.patch_token(1, comma);
        let _ = on_kw;
        self.end_last(NodeKind::CatchClause);
    }

    fn handle_finally_block(&mut self, finally_kw: TokenSpan) {
        self.b.patch(0, self.t(finally_kw.id));
    }

    fn begin_switch_statement(&mut self, token: TokenSpan) {
        self.begin(NodeKind::SwitchStatement, token);
    }

    fn end_switch_statement(&mut self, _token: TokenSpan, end: TokenSpan) {
        self.end_incl(NodeKind::SwitchStatement, end);
    }

    fn begin_switch_block(&mut self, token: TokenSpan) {
        self.begin(NodeKind::SwitchBlock, token);
        // The first case's material (labels/expressions) starts right after
        // this node's own arena row.
        self.switch_material_start = self.b.len() as u32 + 1;
    }

    fn end_switch_block(&mut self, _count: usize, _begin: TokenSpan, end: TokenSpan) {
        // The `}` was consumed before the event — inclusive.
        self.end_incl(NodeKind::SwitchBlock, end);
    }

    fn begin_case_expression(&mut self, case_kw: TokenSpan) {
        self.begin(NodeKind::CaseExpression, case_kw);
    }

    fn end_case_expression(
        &mut self,
        _case_kw: TokenSpan,
        when: Option<TokenSpan>,
        colon: TokenSpan,
    ) {
        self.patch_token(0, when);
        self.b.patch(1, self.t(colon.id));
        self.end_incl(NodeKind::CaseExpression, colon);
    }

    fn begin_switch_case(
        &mut self,
        _label_count: usize,
        _expression_count: usize,
        token: TokenSpan,
    ) {
        self.b.begin_with_first_child(
            NodeKind::SwitchCase,
            self.t(token.id),
            self.switch_material_start,
        );
    }

    fn end_switch_case(
        &mut self,
        _label_count: usize,
        expression_count: usize,
        default_kw: Option<TokenSpan>,
        colon_after_default: Option<TokenSpan>,
        _statement_count: usize,
        _begin: TokenSpan,
        end: TokenSpan,
    ) {
        self.patch_token(0, default_kw);
        self.patch_token(1, colon_after_default);
        self.b
            .patch(2, expression_count.min(u32::MAX as usize) as u32);
        self.switch_material_start = self.b.len() as u32;
        // The bound is the token after the case's statements (peek) — the
        // next case's head or the block's `}`.
        self.end_at(NodeKind::SwitchCase, self.t(end.id));
    }

    fn begin_switch_case_when_clause(&mut self, token: TokenSpan) {
        self.begin(NodeKind::SwitchCaseWhenClause, token);
    }

    fn end_switch_case_when_clause(&mut self, _token: TokenSpan) {
        self.end_last(NodeKind::SwitchCaseWhenClause);
    }

    fn handle_switch_case_no_when_clause(&mut self, _token: TokenSpan) {
        // No when ⇒ no SwitchCaseWhenClause child (D1).
    }

    fn handle_break_statement(&mut self, has_target: bool, break_kw: TokenSpan, end: TokenSpan) {
        let _ = end;
        self.b.begin(NodeKind::Break, self.t(break_kw.id));
        self.b.patch(0, u32::from(has_target));
        self.b.end(NodeKind::Break, self.t(break_kw.id) + 1);
    }

    fn handle_continue_statement(
        &mut self,
        has_target: bool,
        continue_kw: TokenSpan,
        end: TokenSpan,
    ) {
        let _ = end;
        self.b.begin(NodeKind::Continue, self.t(continue_kw.id));
        self.b.patch(0, u32::from(has_target));
        self.b.end(NodeKind::Continue, self.t(continue_kw.id) + 1);
    }

    fn begin_rethrow_statement(&mut self, token: TokenSpan) {
        self.begin(NodeKind::RethrowStatement, token);
    }

    fn end_rethrow_statement(&mut self, _rethrow_kw: TokenSpan, end: TokenSpan) {
        self.end_incl(NodeKind::RethrowStatement, end);
    }

    fn begin_assert(&mut self, token: TokenSpan, _kind: AssertKind) {
        self.begin(NodeKind::Assert, token);
    }

    fn end_assert(
        &mut self,
        _begin: TokenSpan,
        kind: AssertKind,
        left_paren: TokenSpan,
        comma: Option<TokenSpan>,
        end: TokenSpan,
    ) {
        self.b.patch(0, kind as u32);
        self.b.patch(1, self.t(left_paren.id));
        self.patch_token(2, comma);
        self.end_incl(NodeKind::Assert, end);
    }

    fn begin_initializer(&mut self, token: TokenSpan) {
        self.begin(NodeKind::Initializer, token);
    }

    fn end_initializer(&mut self, end: TokenSpan) {
        self.end_incl(NodeKind::Initializer, end);
    }

    fn handle_parenthesized_condition(
        &mut self,
        begin: TokenSpan,
        case_kw: Option<TokenSpan>,
        when: Option<TokenSpan>,
    ) {
        // The condition/pattern completed before the event; adopt it as the
        // only child (`ParenthesizedCondition` delimits the condition segment).
        self.b
            .begin_adopting(NodeKind::ParenthesizedCondition, self.t(begin.id), 1);
        self.patch_token(0, case_kw);
        self.patch_token(1, when);
        self.end_last(NodeKind::ParenthesizedCondition);
    }

    fn begin_pattern_guard(&mut self, token: TokenSpan) {
        self.begin(NodeKind::PatternGuard, token);
    }

    fn end_pattern_guard(&mut self, _token: TokenSpan) {
        self.end_last(NodeKind::PatternGuard);
    }

    fn handle_pattern_variable_declaration_statement(
        &mut self,
        keyword: TokenSpan,
        equals: TokenSpan,
        semicolon: TokenSpan,
    ) {
        self.b
            .begin(NodeKind::PatternVarDeclStatement, self.t(keyword.id));
        self.b.patch(0, self.t(equals.id));
        self.b
            .end(NodeKind::PatternVarDeclStatement, self.t(semicolon.id) + 1);
    }

    // —— expressions ————————————————————————————————————————————————————
    fn handle_literal_int(&mut self, token: TokenSpan) {
        self.b.leaf(NodeKind::LiteralInt, self.t(token.id));
    }

    fn handle_literal_int_with_separators(&mut self, token: TokenSpan) {
        // Merged into `LiteralInt` (separators read from the lexeme).
        self.b.leaf(NodeKind::LiteralInt, self.t(token.id));
    }

    fn handle_literal_double(&mut self, token: TokenSpan) {
        self.b.leaf(NodeKind::LiteralDouble, self.t(token.id));
    }

    fn handle_literal_double_with_separators(&mut self, token: TokenSpan) {
        // Merged into `LiteralDouble` (separators read from the lexeme).
        self.b.leaf(NodeKind::LiteralDouble, self.t(token.id));
    }

    fn handle_literal_bool(&mut self, token: TokenSpan) {
        self.b.leaf(NodeKind::LiteralBool, self.t(token.id));
    }

    fn handle_literal_null(&mut self, token: TokenSpan) {
        self.b.leaf(NodeKind::LiteralNull, self.t(token.id));
    }

    fn begin_literal_string(&mut self, token: TokenSpan) {
        self.begin(NodeKind::StringLiteral, token);
    }

    fn handle_interpolation_expression(&mut self, _left: TokenSpan, _right: Option<TokenSpan>) {
        // Interpolation is carried by the token layer's string segments (D4).
    }

    fn end_literal_string(&mut self, _interpolation_count: usize, begin: TokenSpan) {
        // The event carries only the literal's head token, and interpolation
        // segments produce no child nodes (D4 token-layer carry) — the span
        // anchors at the head; report ranges key off the head offset.
        self.b.end(NodeKind::StringLiteral, self.t(begin.id) + 1);
    }

    fn handle_adjacent_string_literals(&mut self, _start: TokenSpan, _literal_count: usize) {
        // Adjacent concatenation = consecutive StringLiteral siblings (D1).
    }

    fn begin_literal_symbol(&mut self, token: TokenSpan) {
        self.begin(NodeKind::SymbolLiteral, token);
    }

    fn handle_symbol_void(&mut self, token: TokenSpan) {
        self.b.begin(NodeKind::SymbolVoid, self.t(token.id));
        self.b.patch(0, self.t(token.id));
        self.b.end(NodeKind::SymbolVoid, self.t(token.id) + 1);
    }

    fn end_literal_symbol(&mut self, _hash: TokenSpan, _identifier_count: usize) {
        // Width-0: `#` is the span head (#174).
        self.end_last(NodeKind::SymbolLiteral);
    }

    fn handle_operator(&mut self, token: TokenSpan) {
        self.b.leaf(NodeKind::Operator, self.t(token.id));
    }

    fn handle_this_expression(&mut self, token: TokenSpan, _context: IdentifierContext) {
        self.b.leaf(NodeKind::ThisExpression, self.t(token.id));
    }

    fn handle_super_expression(&mut self, token: TokenSpan, _context: IdentifierContext) {
        self.b.leaf(NodeKind::SuperExpression, self.t(token.id));
    }

    fn handle_unary_prefix_expression(&mut self, operator: TokenSpan) {
        // Token leaf over the operator; the operand is the preceding sibling
        // (Handle = childless, collapse map §3.5 / kind.rs TokenLeaf set).
        self.b.leaf(NodeKind::UnaryPrefix, self.t(operator.id));
    }

    fn handle_non_null_assert_expression(&mut self, bang: TokenSpan) {
        self.b.leaf(NodeKind::NonNullAssert, self.t(bang.id));
    }

    fn begin_binary_expression(&mut self, _operator: TokenSpan) {
        // The left operand completed before the event (operands×2 contract,
        // collapse map §3.5); the node opens adopting it, anchored at its
        // head token.
        let first = self.b.last_first_token().expect("binary left operand");
        let start = self.b.last_subtree_start().expect("binary left operand");
        self.b
            .begin_with_first_child(NodeKind::BinaryExpression, first, start);
    }

    fn end_binary_expression(&mut self, operator: TokenSpan, end: TokenSpan) {
        self.b.patch(0, self.t(operator.id));
        self.end_incl(NodeKind::BinaryExpression, end);
    }

    fn begin_const_expression(&mut self, const_keyword: TokenSpan) {
        self.begin(NodeKind::ConstExpression, const_keyword);
    }

    fn end_const_expression(&mut self, token: TokenSpan) {
        // `const` is the span head (width-0, #174).
        self.end_incl(NodeKind::ConstExpression, token);
    }

    fn begin_const_literal(&mut self, token: TokenSpan) {
        self.begin(NodeKind::ConstLiteral, token);
    }

    fn end_const_literal(&mut self, end: TokenSpan) {
        // `const` is the span head (width-0, #174).
        self.end_incl(NodeKind::ConstLiteral, end);
    }

    fn handle_literal_list(
        &mut self,
        _count: usize,
        left: TokenSpan,
        const_keyword: Option<TokenSpan>,
        right: TokenSpan,
    ) {
        // The const word survives elimination (the bracket is the official
        // span head — `const` sits outside it, collapse map §3.5).
        self.b.begin(NodeKind::ListLiteral, self.t(left.id));
        self.patch_token(0, const_keyword);
        self.b.end(NodeKind::ListLiteral, self.t(right.id) + 1);
    }

    fn handle_literal_set_or_map(
        &mut self,
        _count: usize,
        left: TokenSpan,
        const_keyword: Option<TokenSpan>,
        right: TokenSpan,
        has_set_entry: bool,
    ) {
        self.b.begin(NodeKind::SetOrMapLiteral, self.t(left.id));
        self.patch_token(0, const_keyword);
        self.b.patch(1, u32::from(has_set_entry));
        self.b.end(NodeKind::SetOrMapLiteral, self.t(right.id) + 1);
    }

    fn handle_literal_map_entry(
        &mut self,
        colon: TokenSpan,
        end: TokenSpan,
        null_aware_key: Option<TokenSpan>,
        null_aware_value: Option<TokenSpan>,
    ) {
        // Childless Handle: key/value expressions are preceding siblings.
        let first = self.b.last_first_token().expect("map entry anchor");
        self.b.begin(NodeKind::MapEntry, first);
        self.b.patch(0, self.t(colon.id));
        self.patch_token(1, null_aware_key);
        self.patch_token(2, null_aware_value);
        self.b.end(NodeKind::MapEntry, self.t(end.id) + 1);
    }

    fn begin_new_expression(&mut self, token: TokenSpan) {
        self.begin(NodeKind::NewExpression, token);
    }

    fn end_new_expression(&mut self, token: TokenSpan) {
        // `new` is the span head (width-0, #174).
        self.end_incl(NodeKind::NewExpression, token);
    }

    fn begin_parenthesized_expression_or_record_literal(&mut self, token: TokenSpan) {
        // Canonical open of the shared group (§2 #12/#13): the wider
        // `RecordLiteral` kind.
        self.begin(NodeKind::RecordLiteral, token);
    }

    fn end_parenthesized_expression(&mut self, end: TokenSpan) {
        // Narrow seal from the canonical open (accepts_begin #12).
        self.end_incl(NodeKind::ParenthesizedExpression, end);
    }

    fn end_record_literal(
        &mut self,
        end: TokenSpan,
        _count: usize,
        const_keyword: Option<TokenSpan>,
    ) {
        self.patch_token(0, const_keyword);
        self.end_incl(NodeKind::RecordLiteral, end);
    }

    fn handle_named_record_field(&mut self, colon: TokenSpan) {
        // Childless Handle anchored at the value head (the event fires
        // after the field's value parses; same anchor face as
        // `handle_named_argument`). The colon payload and the name token
        // sit immediately before the anchor.
        let first = self.b.last_first_token().expect("record field anchor");
        self.b.begin(NodeKind::NamedRecordField, first);
        self.b.patch(0, self.t(colon.id));
        self.b.end(NodeKind::NamedRecordField, self.t(colon.id) + 1);
    }

    fn handle_positional_record_field(&mut self, _token: TokenSpan) {
        // Positional field = the expression child itself (D1).
    }

    fn begin_arguments(&mut self, token: TokenSpan) {
        self.begin(NodeKind::Arguments, token);
    }

    fn end_arguments(&mut self, _count: usize, _begin: TokenSpan, end: TokenSpan) {
        self.end_incl(NodeKind::Arguments, end);
    }

    fn handle_no_arguments(&mut self, _token: TokenSpan) {
        // Distinguishes `x` from `x()` (collapse map §3.5: a real sentinel).
        self.b.leaf_span(
            NodeKind::NoArguments,
            self.b.last_end_token().unwrap_or(0),
            self.b.last_end_token().unwrap_or(0),
        );
    }

    fn handle_named_argument(&mut self, colon: TokenSpan) {
        // Childless Handle over `name :`; the value expression follows as a
        // sibling inside `Arguments` (no end event exists to nest it).
        let first = self.b.last_first_token().expect("named argument anchor");
        self.b.begin(NodeKind::NamedArgument, first);
        self.b.patch(0, self.t(colon.id));
        self.b.end(NodeKind::NamedArgument, self.t(colon.id) + 1);
    }

    fn handle_positional_argument(&mut self, _token: TokenSpan) {
        // Positional argument = the expression child itself (D1).
    }

    fn handle_send(&mut self, begin: TokenSpan, end: TokenSpan) {
        self.b
            .leaf_span(NodeKind::Send, self.t(begin.id), self.t(end.id) + 1);
    }

    fn handle_send_without_arguments(
        &mut self,
        begin: TokenSpan,
        end: TokenSpan,
        _next: TokenSpan,
    ) {
        // Merged into `Send` (argument presence reads from the children).
        self.b
            .leaf_span(NodeKind::Send, self.t(begin.id), self.t(end.id) + 1);
    }

    fn handle_dot_access(&mut self, token: TokenSpan, end: TokenSpan, is_null_aware: bool) {
        // Childless Handle over the selector extent (`token` = the dot).
        let first = self.b.last_first_token().expect("dot access anchor");
        self.b.begin(NodeKind::DotAccess, first);
        self.b.patch(0, self.t(token.id));
        self.b.patch(1, u32::from(is_null_aware));
        self.b.end(NodeKind::DotAccess, self.t(end.id) + 1);
    }

    fn handle_indexed_expression(
        &mut self,
        question: Option<TokenSpan>,
        open: TokenSpan,
        close: TokenSpan,
    ) {
        // Childless Handle: `question` None ⇒ `open` is the span head
        // (`derived::indexed_open`); `?` present ⇒ the head is the `?`.
        let head = question.unwrap_or(open);
        self.b.begin(NodeKind::Indexed, self.t(head.id));
        self.patch_token(0, question);
        self.b.end(NodeKind::Indexed, self.t(close.id) + 1);
    }

    fn begin_constructor_reference(&mut self, start: TokenSpan) {
        self.begin(NodeKind::ConstructorReference, start);
    }

    fn handle_no_constructor_reference_continuation_after_type_arguments(
        &mut self,
        token: TokenSpan,
    ) {
        self.b
            .leaf(NodeKind::NoCtorRefContinuation, self.t(token.id));
    }

    fn end_constructor_reference(
        &mut self,
        _start: TokenSpan,
        period_before_name: Option<TokenSpan>,
        end: TokenSpan,
        context: ConstructorReferenceContext,
    ) {
        self.patch_token(0, period_before_name);
        self.b.patch(1, context as u32);
        // Both call sites pass the next token (peek) — exclusive.
        self.end_at(NodeKind::ConstructorReference, self.t(end.id));
    }

    fn begin_await_expression(&mut self, token: TokenSpan) {
        self.begin(NodeKind::AwaitExpression, token);
    }

    fn end_await_expression(&mut self, _begin: TokenSpan, end: TokenSpan) {
        // `await` is the span head (width-0, #174).
        self.end_incl(NodeKind::AwaitExpression, end);
    }

    fn end_invalid_await_expression(&mut self, _begin: TokenSpan, end: TokenSpan) {
        // Shared begin with `AwaitExpression` (§2 #10); the error code is
        // resolution-domain.
        self.end_incl(NodeKind::InvalidAwaitExpression, end);
    }

    fn handle_throw_expression(&mut self, throw: TokenSpan, _end: TokenSpan) {
        // Childless Handle: the operand is the preceding sibling.
        self.b.leaf(NodeKind::ThrowExpression, self.t(throw.id));
    }

    fn handle_assignment_expression(&mut self, operator: TokenSpan, end: TokenSpan) {
        // Both operands completed before the event (operands×2).
        let first = self.b.last_first_token().expect("assignment lhs");
        let start = self.b.last_subtree_start().expect("assignment lhs");
        self.b
            .begin_with_first_child(NodeKind::Assignment, first, start);
        self.b.patch(0, self.t(operator.id));
        self.b.end(NodeKind::Assignment, self.t(end.id) + 1);
    }

    fn begin_cascade(&mut self, operator: TokenSpan) {
        self.begin(NodeKind::Cascade, operator);
    }

    fn handle_cascade_access(&mut self, operator: TokenSpan, end: TokenSpan, is_null_aware: bool) {
        let first = self.b.last_first_token().expect("cascade access anchor");
        self.b.begin(NodeKind::CascadeAccess, first);
        self.b.patch(0, self.t(operator.id));
        self.b.patch(1, u32::from(is_null_aware));
        self.b.end(NodeKind::CascadeAccess, self.t(end.id) + 1);
    }

    fn end_cascade(&mut self) {
        self.end_last(NodeKind::Cascade);
    }

    fn begin_conditional_expression(&mut self, question: TokenSpan) {
        self.begin(NodeKind::Conditional, question);
    }

    fn handle_conditional_expression_colon(&mut self) {
        // The colon lands in the payload at the end event (D1).
    }

    fn end_conditional_expression(
        &mut self,
        question: TokenSpan,
        colon: TokenSpan,
        end: TokenSpan,
    ) {
        self.b.patch(0, self.t(question.id));
        self.b.patch(1, self.t(colon.id));
        self.end_incl(NodeKind::Conditional, end);
    }

    fn handle_unary_prefix_assignment_expression(&mut self, operator: TokenSpan) {
        // Merged into `UnaryPrefix` (`-x` vs `++x` reads the op kind).
        self.b.leaf(NodeKind::UnaryPrefix, self.t(operator.id));
    }

    fn handle_unary_postfix_assignment_expression(&mut self, operator: TokenSpan) {
        self.b.leaf(NodeKind::UnaryPostfix, self.t(operator.id));
    }

    fn begin_function_expression(&mut self, token: TokenSpan) {
        self.begin(NodeKind::FunctionExpression, token);
    }

    fn end_function_expression(&mut self, _begin: TokenSpan, end: TokenSpan) {
        // Width-0: the begin token is the span head (#174).
        self.end_incl(NodeKind::FunctionExpression, end);
    }

    fn begin_named_function_expression(&mut self, token: TokenSpan) {
        self.begin(NodeKind::NamedFunctionExpression, token);
    }

    fn end_named_function_expression(&mut self, end: TokenSpan) {
        self.end_incl(NodeKind::NamedFunctionExpression, end);
    }

    fn begin_implicit_creation_expression(&mut self, token: TokenSpan) {
        self.begin(NodeKind::ImplicitCreation, token);
    }

    fn end_implicit_creation_expression(&mut self, _begin: TokenSpan, open_angle: TokenSpan) {
        // Word 0 is the `<` when type arguments were applied.
        self.patch_token(0, Some(open_angle));
        self.end_last(NodeKind::ImplicitCreation);
    }

    fn handle_augment_super_expression(
        &mut self,
        _augment: TokenSpan,
        super_kw: TokenSpan,
        _context: IdentifierContext,
    ) {
        // `augment super` postdates the official 377-node census — no
        // collapse-map variant exists; the SuperExpression leaf carries the
        // position.
        self.b.leaf(NodeKind::SuperExpression, self.t(super_kw.id));
    }

    fn handle_dot_shorthand_head(&mut self, dot: TokenSpan) {
        self.b.leaf(NodeKind::DotShorthandHead, self.t(dot.id));
    }

    fn handle_dot_shorthand_context(&mut self, _dot: TokenSpan) {
        // Context rides the parent expression (D1).
    }

    fn begin_const_dot_shorthand(&mut self, const_keyword: TokenSpan) {
        self.begin(NodeKind::ConstDotShorthand, const_keyword);
    }

    fn end_const_dot_shorthand(&mut self, const_keyword: TokenSpan) {
        self.end_incl(NodeKind::ConstDotShorthand, const_keyword);
    }

    fn handle_invalid_type_arguments(&mut self, token: TokenSpan) {
        self.b
            .leaf(NodeKind::InvalidTypeArguments, self.t(token.id));
    }

    fn handle_pattern_assignment(&mut self, equals: TokenSpan) {
        // Childless Handle over the `=` (the pattern precedes, the assigned
        // expression follows as siblings).
        let first = self
            .b
            .last_first_token()
            .expect("pattern assignment anchor");
        self.b.begin(NodeKind::PatternAssignment, first);
        self.b.patch(0, self.t(equals.id));
        self.b
            .end(NodeKind::PatternAssignment, self.t(equals.id) + 1);
    }

    fn begin_is_operator_type(&mut self, operator: TokenSpan) {
        self.begin(NodeKind::IsExpression, operator);
        self.deferred_seal = Some(NodeKind::IsExpression);
    }

    fn end_is_operator_type(&mut self, _operator: TokenSpan) {
        // Seal deferred to handle_is_operator, which carries the `not` word.
    }

    fn handle_is_operator(&mut self, _is_operator: TokenSpan, not: Option<TokenSpan>) {
        if self.deferred_seal == Some(NodeKind::IsExpression) {
            self.deferred_seal = None;
            // The payload word is the `not` flag (collapse map §3.5).
            self.b.patch(0, u32::from(not.is_some()));
            self.end_last(NodeKind::IsExpression);
        }
    }

    fn begin_as_operator_type(&mut self, operator: TokenSpan) {
        self.begin(NodeKind::AsExpression, operator);
        self.deferred_seal = Some(NodeKind::AsExpression);
    }

    fn end_as_operator_type(&mut self, _operator: TokenSpan) {
        // Seal deferred to handle_as_operator, which carries the `as` word.
    }

    fn handle_as_operator(&mut self, operator: TokenSpan) {
        // Merged with end_as_operator_type (§3.5: End+Handle double source).
        if self.deferred_seal == Some(NodeKind::AsExpression) {
            self.deferred_seal = None;
            self.b.patch(0, self.t(operator.id));
            self.end_last(NodeKind::AsExpression);
        }
    }

    fn handle_type_argument_application(&mut self, open_angle: TokenSpan) {
        // `f<int>()` marker leaf over the applied type arguments.
        let first = self.b.last_first_token().expect("type application anchor");
        self.b.begin(NodeKind::TypeApplication, first);
        self.b.patch(0, self.t(open_angle.id));
        self.b
            .end(NodeKind::TypeApplication, self.t(open_angle.id) + 1);
    }

    // —— patterns ——————————————————————————————————————————————————————
    fn begin_pattern(&mut self, _token: TokenSpan) {
        // Pattern wrapper dropped (collapse map §3.7 D1): pattern nodes land
        // directly.
    }

    fn end_pattern(&mut self, _token: TokenSpan) {
        // See begin_pattern: wrapper dropped.
    }

    fn begin_constant_pattern(&mut self, const_keyword: Option<TokenSpan>) {
        // Deferred to end_constant_pattern: the operand parses between the
        // events and `const` (when present) is the span head — the node is
        // sealed there with the operand adopted.
        self.pending_constant_pattern = const_keyword.map(|t| self.t(t.id));
    }

    fn end_constant_pattern(&mut self, const_keyword: Option<TokenSpan>) {
        let pending = self.pending_constant_pattern.take();
        let head = pending.or(const_keyword.map(|t| self.t(t.id)));
        if let Some(head) = head {
            let start = self
                .b
                .last_subtree_start()
                .expect("constant pattern operand");
            self.b
                .begin_with_first_child(NodeKind::ConstantPattern, head, start);
        } else {
            let first = self.b.last_first_token().expect("constant pattern operand");
            self.b.begin_adopting(NodeKind::ConstantPattern, first, 1);
        }
        self.end_last(NodeKind::ConstantPattern);
    }

    fn handle_relational_pattern(&mut self, operator: TokenSpan) {
        // Fires after the operand parsed (childless Handle; `op` word).
        let first = self
            .b
            .last_first_token()
            .expect("relational pattern anchor");
        self.b.begin(NodeKind::RelationalPattern, first);
        self.b.patch(0, self.t(operator.id));
        self.end_last(NodeKind::RelationalPattern);
    }

    fn handle_cast_pattern(&mut self, operator: TokenSpan) {
        let first = self.b.last_first_token().expect("cast pattern anchor");
        self.b.begin(NodeKind::CastPattern, first);
        self.b.patch(0, self.t(operator.id));
        self.end_last(NodeKind::CastPattern);
    }

    fn handle_null_assert_pattern(&mut self, bang: TokenSpan) {
        self.b.leaf(NodeKind::NullAssertPattern, self.t(bang.id));
    }

    fn handle_null_check_pattern(&mut self, question: TokenSpan) {
        self.b.leaf(NodeKind::NullCheckPattern, self.t(question.id));
    }

    fn begin_binary_pattern(&mut self, _operator: TokenSpan) {
        // The left operand completed before the event (operands×2).
        let first = self.b.last_first_token().expect("binary pattern lhs");
        let start = self.b.last_subtree_start().expect("binary pattern lhs");
        self.b
            .begin_with_first_child(NodeKind::BinaryPattern, first, start);
    }

    fn end_binary_pattern(&mut self, operator: TokenSpan) {
        self.b.patch(0, self.t(operator.id));
        self.end_last(NodeKind::BinaryPattern);
    }

    fn handle_list_pattern(&mut self, _count: usize, left: TokenSpan, right: TokenSpan) {
        // Brackets are the span endpoints (#174); elements are preceding
        // siblings (childless Handle).
        self.b
            .leaf_span(NodeKind::ListPattern, self.t(left.id), self.t(right.id) + 1);
    }

    fn handle_map_pattern(&mut self, _count: usize, left: TokenSpan, right: TokenSpan) {
        self.b
            .leaf_span(NodeKind::MapPattern, self.t(left.id), self.t(right.id) + 1);
    }

    fn handle_map_pattern_entry(&mut self, colon: TokenSpan, end: TokenSpan) {
        let first = self.b.last_first_token().expect("map pattern entry anchor");
        self.b.begin(NodeKind::MapPatternEntry, first);
        self.b.patch(0, self.t(colon.id));
        self.b.end(NodeKind::MapPatternEntry, self.t(end.id) + 1);
    }

    fn handle_rest_pattern(&mut self, dots: TokenSpan, has_sub_pattern: bool) {
        self.b.begin(NodeKind::RestPattern, self.t(dots.id));
        self.b.patch(0, self.t(dots.id));
        self.b.patch(1, u32::from(has_sub_pattern));
        self.b.end(NodeKind::RestPattern, self.t(dots.id) + 1);
    }

    fn handle_record_pattern(&mut self, left_paren: TokenSpan, _count: usize) {
        let end = self.b.last_end_token().unwrap_or(self.t(left_paren.id) + 1);
        self.b.leaf_span(
            NodeKind::RecordPattern,
            self.t(left_paren.id),
            end.max(self.t(left_paren.id) + 1),
        );
    }

    fn handle_parenthesized_pattern(&mut self, left_paren: TokenSpan) {
        let end = self.b.last_end_token().unwrap_or(self.t(left_paren.id) + 1);
        self.b.leaf_span(
            NodeKind::ParenthesizedPattern,
            self.t(left_paren.id),
            end.max(self.t(left_paren.id) + 1),
        );
    }

    fn handle_pattern_field(&mut self, colon: Option<TokenSpan>) {
        let first = self.b.last_first_token().expect("pattern field anchor");
        self.b.begin(NodeKind::PatternField, first);
        self.patch_token(0, colon);
        self.end_last(NodeKind::PatternField);
    }

    fn handle_object_pattern(
        &mut self,
        first_identifier: TokenSpan,
        dot: Option<TokenSpan>,
        second_identifier: Option<TokenSpan>,
    ) {
        // The fields sealed before this event (official
        // parser_impl.dart:11848 fires it after parseObjectPatternRest);
        // reassemble the node over the stashed field window — the official
        // builder pops its `_ObjectPatternFields` marker here
        // (ast_builder.dart:5362-5397).
        let (first_child, end) = self
            .object_pattern_pending
            .take()
            .expect("handle_object_pattern without a preceding fields event");
        self.b.begin_with_first_child(
            NodeKind::ObjectPattern,
            self.t(first_identifier.id),
            first_child,
        );
        self.b.patch(0, self.t(first_identifier.id));
        self.patch_token(1, dot);
        self.patch_token(2, second_identifier);
        self.b.end(NodeKind::ObjectPattern, end);
    }

    fn handle_object_pattern_fields(&mut self, _count: usize, _begin: TokenSpan, end: TokenSpan) {
        self.object_pattern_pending = Some((self.b.len() as u32, self.t(end.id) + 1));
    }

    fn handle_wildcard_pattern(&mut self, keyword: Option<TokenSpan>, wildcard: TokenSpan) {
        let head = keyword.unwrap_or(wildcard);
        self.b.begin(NodeKind::WildcardPattern, self.t(head.id));
        self.b.patch(0, self.t(wildcard.id));
        self.b
            .end(NodeKind::WildcardPattern, self.t(wildcard.id) + 1);
    }

    fn handle_assigned_variable_pattern(&mut self, variable: TokenSpan) {
        self.b
            .leaf(NodeKind::AssignedVariablePattern, self.t(variable.id));
    }

    fn handle_declared_variable_pattern(
        &mut self,
        keyword: Option<TokenSpan>,
        variable: TokenSpan,
        _in_assignment_pattern: bool,
    ) {
        let head = keyword.unwrap_or(variable);
        self.b
            .begin(NodeKind::DeclaredVariablePattern, self.t(head.id));
        self.b.patch(0, self.t(variable.id));
        self.b
            .end(NodeKind::DeclaredVariablePattern, self.t(variable.id) + 1);
    }

    // —— collection control-flow elements ————————————————————————————————
    fn begin_if_control_flow(&mut self, token: TokenSpan) {
        self.begin(NodeKind::IfControlFlow, token);
    }

    fn handle_then_control_flow(&mut self, token: TokenSpan) {
        self.b.leaf(NodeKind::ThenControlFlow, self.t(token.id));
    }

    fn handle_else_control_flow(&mut self, token: TokenSpan) {
        self.b.leaf(NodeKind::ElseControlFlow, self.t(token.id));
    }

    fn end_if_control_flow(&mut self, token: TokenSpan) {
        self.end_incl(NodeKind::IfControlFlow, token);
    }

    fn end_if_else_control_flow(&mut self, token: TokenSpan) {
        // Same kind seal — the else segment reads from the ElseControlFlow
        // sentinel child (§2 #9).
        self.end_incl(NodeKind::IfControlFlow, token);
    }

    fn begin_for_control_flow(&mut self, await_kw: Option<TokenSpan>, for_kw: TokenSpan) {
        let head = await_kw.unwrap_or(for_kw);
        self.begin(NodeKind::ForControlFlow, head);
        self.patch_token(0, await_kw);
    }

    fn end_for_control_flow(&mut self, token: TokenSpan) {
        self.b.patch(1, 0);
        self.end_incl(NodeKind::ForControlFlow, token);
    }

    fn end_for_in_control_flow(&mut self, token: TokenSpan) {
        self.b.patch(1, 1);
        self.end_incl(NodeKind::ForControlFlow, token);
    }

    fn handle_spread_expression(&mut self, operator: TokenSpan) {
        self.b.leaf(NodeKind::Spread, self.t(operator.id));
    }

    fn handle_null_aware_element(&mut self, token: TokenSpan) {
        self.b.leaf(NodeKind::NullAwareElement, self.t(token.id));
    }

    // —— switch expressions ——————————————————————————————————————————————
    fn begin_switch_expression(&mut self, token: TokenSpan) {
        self.begin(NodeKind::SwitchExpression, token);
    }

    fn end_switch_expression(&mut self, _token: TokenSpan, end: TokenSpan) {
        // Both call shapes pass a `}`-anchored bound; treated as exclusive
        // (the synthetic-drain path passes the `}` itself — one token short
        // only in that recovery shape).
        self.end_at(NodeKind::SwitchExpression, self.t(end.id));
    }

    fn begin_switch_expression_block(&mut self, token: TokenSpan) {
        self.begin(NodeKind::SwitchExpressionBlock, token);
    }

    fn end_switch_expression_block(&mut self, _count: usize, _begin: TokenSpan, end: TokenSpan) {
        self.end_at(NodeKind::SwitchExpressionBlock, self.t(end.id));
    }

    fn begin_switch_expression_case(&mut self) {
        let anchor = self.b.last_end_token().unwrap_or(0);
        self.b.begin(NodeKind::SwitchExpressionCase, anchor);
    }

    fn handle_switch_expression_case_pattern(&mut self, _token: TokenSpan) {
        // The pattern is the leading child (D1).
    }

    fn end_switch_expression_case(
        &mut self,
        _begin: TokenSpan,
        when: Option<TokenSpan>,
        arrow: TokenSpan,
        end: TokenSpan,
    ) {
        self.patch_token(0, when);
        self.b.patch(1, self.t(arrow.id));
        self.end_at(NodeKind::SwitchExpressionCase, self.t(end.id));
    }

    // —— recovery ——————————————————————————————————————————————————————
    fn handle_recoverable_error(
        &mut self,
        _message: dartq_parser::Message,
        start: TokenSpan,
        _end: TokenSpan,
    ) {
        // Diagnostics flow through the parser's own channel; the bridge
        // emits the official `RecoverableError` sentinel node and marks the
        // error bit on the enclosing nodes (collapse map §3.9 — the
        // sentinel is the differential-parity face of the error).
        self.b.leaf(NodeKind::RecoverableError, self.t(start.id));
        self.b.mark_error();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{EnableSet, rule_by_name};
    use crate::{Lint, run};
    use dartq_ast::utf16_span;

    /// Parses `source` through the real bridge and runs one rule.
    fn hits(source: &str, rule: &str) -> Vec<Lint> {
        let parsed = parse_unit(source, Features::default());
        let unit = parsed.unit(None);
        let (set, unknown) = EnableSet::resolve([rule]);
        assert!(unknown.is_empty(), "unknown rule {rule}");
        run(&unit, &set)
    }

    /// Walks the arena collecting (kind, utf16 start) pairs.
    fn walk(source: &str) -> Vec<(NodeKind, u32)> {
        let parsed = parse_unit(source, Features::default());
        let mut out = Vec::new();
        let mut stack = vec![parsed.ast.root().expect("root")];
        while let Some(id) = stack.pop() {
            out.push((
                parsed.ast.kind(id),
                utf16_span(&parsed.source, &parsed.tokens, &parsed.ast, id).start,
            ));
            stack.extend(parsed.ast.children(id).rev());
        }
        out
    }

    #[test]
    fn real_parse_reports_avoid_empty_else() {
        // `else ;` — the empty else statement sits at UTF-16 offset 27.
        let hits = hits("void f() {\n  if (b) ; else ;\n}\n", "avoid_empty_else");
        assert_eq!(hits.len(), 1, "the else `;` reports through the real tree");
        assert_eq!(hits[0].range, 27..28);
        assert_eq!(
            hits[0].message,
            "Empty statements are not allowed in an 'else' clause."
        );
    }

    #[test]
    fn real_parse_reports_unnecessary_new() {
        let hits = hits("var x = new Object();\n", "unnecessary_new");
        assert_eq!(hits.len(), 1);
        // The official reports at the keyword with a zero-width span.
        assert_eq!(hits[0].range, 8..8);
    }

    #[test]
    fn bisect_finish_leaks() {
        for (source, label) in [
            ("void f() { }\n", "empty fn"),
            ("void f() { var x = 1; }\n", "local var"),
            ("class C { }\n", "empty class"),
            ("class C { void m() { } }\n", "class+method"),
            ("var g = () {};\n", "fn literal"),
            (
                "void f() { List<List<int>> x = const []; }\n",
                "generic local",
            ),
            ("void f() { print(1); }\n", "send"),
        ] {
            let parsed = parse_unit(source, Features::default());
            let _ = parsed.unit(None);
            let _ = label;
        }
    }

    #[test]
    fn object_pattern_with_fields_survives_the_bridge() {
        // Regression: object-pattern fields sealed before the container
        // event (official order, parser_impl.dart:11848); the bridge must
        // reassemble fields-first or every case expression panics. The tree
        // must carry the ObjectPattern with its PatternFields, and the lint
        // face must read pattern variables through it.
        let source = "void f(Object o) {\n  if (o case Rect(w: var BadName)) print(1);\n}\n";
        let kinds: Vec<NodeKind> = walk(source).into_iter().map(|(kind, _)| kind).collect();
        assert!(
            kinds.contains(&NodeKind::ObjectPattern),
            "object pattern produced"
        );
        assert!(kinds.contains(&NodeKind::PatternField), "field produced");
        let hits = hits(source, "non_constant_identifier_names");
        assert_eq!(hits.len(), 1, "pattern variable name checked");
    }

    #[test]
    fn empty_object_pattern_survives_the_bridge() {
        // The zero-field variant seals with an empty child window.
        let source = "void f(Object o) {\n  if (o case Rect()) print(1);\n}\n";
        let kinds: Vec<NodeKind> = walk(source).into_iter().map(|(kind, _)| kind).collect();
        assert!(kinds.contains(&NodeKind::ObjectPattern));
        assert!(!kinds.contains(&NodeKind::PatternField));
    }

    #[test]
    fn real_parse_reports_empty_statement() {
        let empty = hits("void f() {\n  ;\n}\n", "empty_statements");
        assert_eq!(empty.len(), 1, "the lone `;` statement reports");
        assert_eq!(empty[0].range, 13..14);
    }

    #[test]
    fn generics_split_runs_through_the_unified_table() {
        // `>>` closer splits land at parser-inserted token ids: span reads
        // through the appended table are the sequencing contract.
        let source = "void f() {\n  List<List<int>> x = const [];\n}\n";
        let parsed = parse_unit(source, Features::default());
        let mut stack = vec![parsed.ast.root().expect("root")];
        while let Some(id) = stack.pop() {
            let _ = dartq_ast::byte_span(&parsed.tokens, &parsed.ast, id);
            let _ = utf16_span(&parsed.source, &parsed.tokens, &parsed.ast, id);
            stack.extend(parsed.ast.children(id).rev());
        }
    }

    #[test]
    fn class_declaration_shape_matches_the_read_contracts() {
        let source = "class C extends B implements I {\n  void m() {\n    print(1);\n  }\n}\n";
        let kinds: Vec<NodeKind> = walk(source).into_iter().map(|(kind, _)| kind).collect();
        for expected in [
            NodeKind::ClassDeclaration,
            NodeKind::DeclarationBody,
            NodeKind::Method,
            NodeKind::BlockBody,  // the method's `{ ... }` body
            NodeKind::Identifier, // the method name and more
        ] {
            assert!(
                kinds.contains(&expected),
                "{expected:?} missing from the walk"
            );
        }
        // `empty_catches` stays quiet on catch-free code (smoke over the
        // statement layer through the same tree).
        assert!(hits(source, "empty_catches").is_empty());
    }

    #[test]
    fn prefer_function_declarations_sees_the_local_initializer() {
        // The rule dispatches on `InitializedIdentifier` (a
        // `VariableInitializer` child wrapping the function literal): the
        // local-declaration path.
        let smoke = hits(
            "void f() {\n  g = () sync* {};\n}\n",
            "no_duplicate_case_values",
        );
        let _ = smoke; // smoke: the switch-family rule runs without panic
        let rows = hits(
            "void f() {\n  var g = () {};\n}\n",
            "prefer_function_declarations_over_variables",
        );
        assert_eq!(rows.len(), 1, "function-literal initializer reports");
    }

    #[test]
    fn rule_by_name_index_agrees_with_the_run() {
        // A registry-parallel sanity: the dispatched rule id is the queried
        // one (dispatch/registry drift would show as zero rows everywhere).
        let id = rule_by_name("avoid_empty_else").expect("wired");
        let hits = hits("void f() {\n  if (b) ; else ;\n}\n", "avoid_empty_else");
        assert_eq!(hits[0].rule, id);
    }
}
