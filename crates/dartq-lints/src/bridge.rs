//! The parser→AST production bridge: the [`ParserListener`] that translates
//! the parser's event stream into [`AstBuilder`] calls, plus the transient
//! [`ParsedUnit`] entry the consumption faces drive.
//!
//! Every node-producing event maps 1:1 onto the collapse-map verdict
//! (the #31 adjudication): Begin events open the End-kind
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

use dartq_ast::{AstBuilder, NodeKind, PayloadField, SyntaxTable, TOKEN_NONE};
use dartq_parser::{
    AssertKind, ConstructorReferenceContext, DeclarationHeaderKind, DeclarationKind,
    ExperimentalFlag, FormalParameterKind, IdentifierContext, MemberKind, ParserFeatures,
    ParserListener, TokenSpan,
};
use dartq_scanner::{ScanConfig, TokenTable, scan};
use dartq_source::LineIndex;

use crate::engine::{Features, Unit};

/// Modifier-set bit assignments (the two families; the parser lane
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

/// `RecoverDeclarationHeader.kind` discriminants.
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
    /// Borrows the unit substrate for one lint run. The workspace facts and
    /// the settled language version mount by assignment after the call
    /// (`Unit::workspace` / `Unit::version`) — the bridge itself carries
    /// neither face. The semantic query face mounts at the call it serves
    /// ([`Unit::run_with_semantics`]): it borrows the driver-built context
    /// tables, none of which this transient substrate holds.
    #[must_use]
    pub fn unit<'a>(&'a self, path: Option<&'a Path>) -> Unit<'a> {
        Unit {
            source: &self.source,
            tokens: &self.tokens,
            ast: &self.ast,
            lines: &self.lines,
            path,
            features: self.features,
            version: None,
            workspace: None,
            reachability: None,
            async_context_uses: None,
            library_type_uses: None,
        }
    }
}

/// Production scan posture: the latest language surface (the lint rules
/// target the Dart 3.15 linter; the scanner gates `>>>` and `augment`).
const SCAN_CONFIG: ScanConfig = ScanConfig {
    enable_triple_shift: true,
    enable_augmentations: true,
};

/// The scan-only half of [`parse_unit`]: the token table the comment faces
/// read (the `unignorable_ignore` validator needs comments, never an AST).
pub(crate) fn scan_tokens(source: &str, file: dartq_source::FileId) -> TokenTable {
    scan(source, SCAN_CONFIG, file).tokens
}

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
    parse_unit_with_tokens(source, scan(source, SCAN_CONFIG, file).tokens, features)
}

/// The scan-less core of [`parse_unit`]: the same parser → arena walk over an
/// already-scanned table, adopted by value. The semantic fold's context build
/// scans each linted file with this bridge's production [`SCAN_CONFIG`] over
/// the job's own bytes — adopting that table seals the unit with the parser's
/// inserted tokens exactly as a fresh scan would (`TokenTable` is a pure
/// function of the source bytes and the config), and removes the second scan
/// the per-file job otherwise paid. The caller keeps its own table when the
/// fold faces still read it (the raw pre-parse stream); the adoption here
/// takes an owned copy, never a shared borrow — the fan-out stays lock-free.
///
/// # Contract
///
/// `tokens` must come from `scan(source, SCAN_CONFIG, ..)` over these exact
/// bytes. Any other posture (a relaxed gate, a different file's table) is a
/// silent divergence the type system cannot see — the commands side pins the
/// posture with a const assert at the adoption site.
#[must_use]
pub fn parse_unit_with_tokens(source: &str, tokens: TokenTable, features: Features) -> ParsedUnit {
    let mut tokens = tokens;
    let base = tokens.len() as u32;
    let mut listener = AstListener::new(base);
    let inserted = dartq_parser::parse_unit_with(source, &tokens, PARSE_FEATURES, &mut listener);
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
    /// duplicate-clause recovery. No nesting: directives are sequential and
    /// a URI is a string token, so imports never interleave.
    import_open: bool,
    /// `handle_enum_elements` fires while the `EnumBody` wrapper is still
    /// open; the `EnumDeclaration` payload word 2 (`elements_end`) is
    /// stashed here and applied at `end_enum_declaration` (`None` writes
    /// the `TOKEN_NONE` sentinel for element-less enums). No nesting: enum
    /// declarations do not nest and the event fires once per body.
    enum_elements_end: Option<u32>,
    /// Arena-tail snapshot at the previous `SwitchCase` boundary: the case's
    /// labels/expressions/guards complete BEFORE `begin_switch_case` fires,
    /// in a mix no sibling count can walk (the payload keeps
    /// `expression_count` precisely because the children are flat), so the
    /// listener tracks the segment start itself. Nested switches are safe:
    /// the enclosing case's `end_switch_case` fires after its body (nested
    /// switch included) and re-anchors the snapshot before the next case.
    switch_material_start: u32,
    /// `MetadataStar` opens with an anchor token and may seal empty (the
    /// recovery path fires the pair back-to-back); the anchor and the arena
    /// length at begin decide the exclusive end. Pairs NEST on legal Dart:
    /// an annotated star's window spans its annotations' argument parses
    /// (the full expression engine), and any re-entry — a generic function
    /// literal's type variables, a record-type cast's fields, annotated
    /// formals — fires the pair again, so the frames live on a stack. One
    /// `Vec` reused across the unit keeps the hot path allocation-free
    /// (pushed/popped per pair; the listener drops with the unit).
    metadata_star_stack: Vec<(u32, u32)>,
    /// Whether a `TopLevelDeclaration` wrapper is open (the recovery path in
    /// `parse_unit` fires `end_top_level_declaration` without a prelude).
    /// No nesting: top-level declarations are sequential by grammar.
    tld_open: bool,
    /// Which class-like declaration is open — `handle_implements`/
    /// `handle_class_extends`/`handle_mixin_on` patch only the kinds whose
    /// payload carries their word. No nesting: class-like declarations do
    /// not nest (no local classes), and the patch events fire only in the
    /// open declaration's header.
    class_decl: ClassDecl,
    /// The `handle_async_modifier` words in flight: both function-body kinds
    /// absorb them into their payload, but the modifier event fires BEFORE
    /// the body node opens (the grammar consumes `async` before `{`/`=>`).
    /// Every body carries exactly one modifier event (the parser fires the
    /// sentinel pair for sync bodies too, mirroring the official
    /// `parseAsyncModifierOpt`), so the frames sit on a stack: an arrow
    /// body's window spans its whole expression, and a nested literal's
    /// modifier/body events would clobber a single slot (`f() async =>
    /// g(() async => 1);` lost the outer `async`). Bodies that absorb no
    /// words (redirecting factories, `native`, invalid/empty bodies) leave
    /// their frame unpopped — it stays buried below later frames under LIFO,
    /// never misattributed, and is bounded by the unit.
    async_pending_stack: Vec<(Option<u32>, Option<u32>)>,
    /// `ConstantPattern` deferral: the operand parses between the events and
    /// `const` (when present) is the span head, so the node seals at the end
    /// event with the operand adopted. No nesting: constant patterns occur
    /// only in pattern position and the window holds a single operand
    /// expression, which fires no pattern events.
    pending_constant_pattern: Option<u32>,
    /// `ObjectPattern` fields-first deferral: the fields parse and seal
    /// BEFORE `handle_object_pattern` fires (official order,
    /// parser_impl.dart:11847-11848), so the fields event stashes the child
    /// window start (arena tail) and exclusive end; the object-pattern event
    /// pops the marker and reassembles, exactly the official builder's
    /// `_ObjectPatternFields` pop (ast_builder.dart:5362-5410). No nesting:
    /// a nested object pattern completes its own fields/event pair before
    /// the enclosing fields event fires.
    object_pattern_pending: Option<(u32, u32)>,
    /// `IsExpression`/`AsExpression` defer their seal to the operator handle
    /// event that immediately follows the end event and carries the word.
    /// No nesting: the deferred window holds only the type parse, which
    /// fires no is/as events.
    deferred_seal: Option<NodeKind>,
    /// First parser↔bridge pairing miss this unit saw, as the violated
    /// invariant's text (`None` while the stream is healthy). The
    /// structured fallback ledger for the [`AstListener::expect_paired`]
    /// contract: two words of fixed capacity, dropped with the unit, read
    /// by nothing on the lint-row path.
    pairing_miss: Option<&'static str>,
    /// The unified-table head index per open string literal. The parser's
    /// `end_literal_string` event carries the literal's LAST String segment
    /// (upstream `endLiteralString`'s endToken), but the production tiling
    /// anchors `StringLiteral` at its head token (D4 token-layer carry —
    /// interpolation segments produce no child nodes), so the head latched
    /// at begin decides the seal. Strings nest through interpolations
    /// (`"$a${'b'}"`), so the frames stack LIFO like
    /// [`AstListener::metadata_star_stack`].
    string_heads: Vec<u32>,
    /// Total pairing misses, saturating — the bounded count face of the
    /// ledger (repeated mismatches never grow anything past this word).
    pairing_miss_count: u32,
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

    /// The pairing contract behind every audited anchor read and
    /// pending-state pop: the parser emits well-formed event streams, so
    /// `found` is always `Some` on healthy input and the fallback below is
    /// observably unreachable. `None` is only-on-parser-bug; the miss is
    /// recorded (bounded — see [`AstListener::pairing_miss`]) and the
    /// caller degrades the affected event (skip its remainder, or seal
    /// with the documented fallback) instead of panicking the lint pass.
    /// `what` is the invariant text the former `.expect` carried, kept
    /// verbatim so field debugging retains the context.
    fn expect_paired<T>(&mut self, what: &'static str, found: Option<T>) -> Option<T> {
        if found.is_none() {
            self.pairing_miss(what);
        }
        found
    }

    /// Records one pairing miss into the fixed-capacity ledger: the first
    /// miss's invariant text is retained, later misses only bump the
    /// saturating count. Release builds stay silent — the miss only
    /// degrades the affected event, never the lint rows; debug builds note
    /// the first miss once on stderr (the crate has no log facade and none
    /// is added for this).
    fn pairing_miss(&mut self, what: &'static str) {
        if self.pairing_miss.is_none() {
            self.pairing_miss = Some(what);
            #[cfg(debug_assertions)]
            eprintln!("dartq bridge: parser/bridge pairing miss: {what}");
        }
        self.pairing_miss_count = self.pairing_miss_count.saturating_add(1);
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
    /// that carry no end token). An `end_last` seal always follows a
    /// completed child node; the no-completed-node face is only-on-parser-bug
    /// and seals zero-width at the stream head — the seal itself must still
    /// happen (the node is already open; skipping it would desynchronize
    /// the builder's open stack).
    fn end_last(&mut self, kind: NodeKind) {
        let end = match self.expect_paired("end_last before any node", self.b.last_end_token()) {
            Some(end) => end,
            None => 0,
        };
        self.b.end(kind, end);
    }

    /// Patch the named word with a token or the absent sentinel — the
    /// token-valued face of [`AstBuilder::patch_field`] (the field name
    /// resolves against the open node's kind through the payload-field
    /// table).
    fn patch_token(&mut self, field: PayloadField, token: Option<TokenSpan>) {
        self.b.patch_field(
            field,
            token.map_or(TOKEN_NONE, |t: TokenSpan| self.t(t.id)),
        );
    }

    fn set_mods(&mut self, bits: u16) {
        self.b.set_modifiers(bits);
    }

    /// Emits the `RecoverDeclarationHeader` sentinel: a zero-width Fielded
    /// node at the current position (a recovery-pass marker, no extent of
    /// its own). The sentinel fires after some node completed; on the
    /// pairing-miss face the marker emission is skipped entirely (it is
    /// one atomic begin+end, so skipping keeps the builder balanced).
    fn recover_header(&mut self, kind_discriminant: u32) {
        let Some(anchor) = self.expect_paired("recover header anchor", self.b.last_end_token())
        else {
            return;
        };
        self.b.begin(NodeKind::RecoverDeclarationHeader, anchor);
        self.b
            .patch_field(PayloadField::HeaderKind, kind_discriminant);
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
        self.b
            .patch_field(PayloadField::HasName, u32::from(has_name));
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
        self.b
            .patch_field(PayloadField::AugmentKeyword, self.t(augment_kw.id));
        self.b
            .patch_field(PayloadField::LibraryKeyword, self.t(library_keyword.id));
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
        self.patch_token(PayloadField::AsKeyword, as_kw);
        self.patch_token(PayloadField::DeferredKeyword, deferred_kw);
    }

    fn handle_recover_import(&mut self, semicolon: Option<TokenSpan>) {
        // Recovery-pass sentinel: anchored at the `;`
        // when the pass found one, zero-width at the current position
        // otherwise. The anchor-miss face (no completed node to anchor at)
        // skips the marker emission (one atomic begin+end).
        if let Some(semicolon) = semicolon {
            self.b.begin(NodeKind::RecoverImport, self.t(semicolon.id));
            self.b
                .patch_field(PayloadField::Semicolon, self.t(semicolon.id));
            self.b
                .end(NodeKind::RecoverImport, self.t(semicolon.id) + 1);
        } else {
            let Some(anchor) = self.expect_paired("recover import anchor", self.b.last_end_token())
            else {
                return;
            };
            self.b.begin(NodeKind::RecoverImport, anchor);
            self.b
                .patch_field(PayloadField::Semicolon, TOKEN_NONE);
            self.b.end(NodeKind::RecoverImport, anchor);
        }
    }

    fn begin_conditional_uris(&mut self, _token: TokenSpan) {
        // Combinators/ConditionalUris/TypeList wrappers are dropped
        // (derivable: the children delimit themselves).
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
        self.b
            .patch_field(PayloadField::LeftParen, self.t(left_paren.id));
        self.patch_token(PayloadField::EqualSign, equal_sign);
        self.end_last(NodeKind::ConditionalUri);
    }

    fn handle_dotted_name(&mut self, count: usize, first_identifier: TokenSpan) {
        // The chain's Identifiers completed before the event (children of the
        // enclosing directive); adopt them.
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
        self.b.patch_field(PayloadField::OfKeyword, self.t(of_kw.id));
        self.b
            .patch_field(PayloadField::HasName, u32::from(has_name));
        self.end_incl(NodeKind::PartOf, semicolon);
    }

    // The script tag (`#!...`) is consumed by `parse_unit` without a listener
    // event, so the `ScriptTag` leaf is unproducible on this bridge; the
    // node needs a parser-side event (the only production scripts affected
    // are standalone `bin/` entrypoints no registered rule dispatches on).

    // —— metadata ————————————————————————————————————————————————————
    fn begin_metadata_star(&mut self, token: TokenSpan) {
        self.metadata_star_stack
            .push((self.t(token.id), self.b.len() as u32));
        self.begin(NodeKind::MetadataStar, token);
    }

    fn end_metadata_star(&mut self, _count: usize) {
        // Unpaired end (only-on-parser-bug): the star node never opened, so
        // sealing here would pop an unrelated open node — the event's
        // remainder is skipped instead.
        let popped = self.metadata_star_stack.pop();
        let Some((anchor, len_at_begin)) = self.expect_paired("metadata star pair", popped) else {
            return;
        };
        let end = if self.b.len() > len_at_begin as usize {
            match self.expect_paired("metadata star child", self.b.last_end_token()) {
                Some(end) => end,
                // Unreachable while the pair held (a grown star implies a
                // completed node); skipping the seal keeps the degrade
                // bounded.
                None => return,
            }
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
        end: TokenSpan,
    ) {
        self.patch_token(PayloadField::PeriodBeforeName, period_before_name);
        // Seal at the parser-supplied end (last consumed: the `)` or the
        // annotation name) — `end_last` would trust `last_end_token`,
        // polluted by `NoTypeArguments`/`NoArguments` sentinel leaves.
        self.end_incl(NodeKind::Metadata, end);
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
        // opens at the chain head and closes past the period (a childless
        // Handle node). The anchor-miss face skips the marker emission
        // (one atomic begin+end; the identifiers remain as siblings).
        let Some(first) = self.expect_paired("qualified anchor", self.b.last_first_token()) else {
            return;
        };
        self.b.begin(NodeKind::Qualified, first);
        self.b.patch_field(PayloadField::Period, self.t(period.id));
        self.b.end(NodeKind::Qualified, self.t(period.id) + 1);
    }

    fn handle_identifier(&mut self, token: TokenSpan, context: IdentifierContext) {
        self.b.begin(NodeKind::Identifier, self.t(token.id));
        self.b.patch_field(PayloadField::Context, context as u32);
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
        self.patch_token(PayloadField::ExtendsOrSuper, extends_or_super);
        self.patch_token(PayloadField::Variance, variance);
        self.end_last(NodeKind::TypeVariable);
    }

    fn begin_record_type(&mut self, left_paren: TokenSpan) {
        self.begin(NodeKind::RecordType, left_paren);
    }

    fn end_record_type(
        &mut self,
        left_paren: TokenSpan,
        question: Option<TokenSpan>,
        count: usize,
        has_named_fields: bool,
    ) {
        self.b
            .patch_field(PayloadField::LeftParen, self.t(left_paren.id));
        self.patch_token(PayloadField::Question, question);
        // The event carries no right paren (the official `endRecordType`
        // shape): a `?` suffix is the last token, a populated type seals
        // after its last entry, and the empty `()` — no entry to seal
        // after — closes at the token following `(`.
        match question {
            Some(question) => self.end_incl(NodeKind::RecordType, question),
            None if count == 0 && !has_named_fields => {
                self.end_at(NodeKind::RecordType, self.t(left_paren.id) + 2);
            }
            None => self.end_last(NodeKind::RecordType),
        }
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
        self.b
            .patch_field(PayloadField::FunctionKeyword, self.t(function.id));
        self.patch_token(PayloadField::Question, question);
        self.end_last(NodeKind::FunctionType);
    }

    fn handle_void_keyword(&mut self, token: TokenSpan) {
        self.b.leaf(NodeKind::VoidType, self.t(token.id));
    }

    fn handle_void_keyword_with_type_arguments(&mut self, _token: TokenSpan) {
        // `void<T>` is always an error: the VoidType leaf plus the sibling
        // TypeArguments carry the shape; the diagnostic goes through the
        // recovery channel.
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
        self.b.patch_field(PayloadField::ExperimentalFlag, flag as u32);
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
        // wrapper; the prelude begin itself is R1.
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
        self.patch_token(PayloadField::GetOrSet, get_or_set);
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
        // No member-kind word here: the width-0 `TopLevelFields` seal cannot
        // read one (the ModSet lives in the header bits, #174), and patching
        // anyway materialized a dead pool word + undo entry per declaration.
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
        self.b
            .patch_field(PayloadField::TypedefKeyword, self.t(typedef.id));
        self.patch_token(PayloadField::Equals, equals);
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
        // Canonical open of the shared `Fields` group: the wider
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
        // Word 0 is the `MemberKind` discriminant (the
        // official builder derives it the same way from the event's
        // declaration family + the static modifier, member_kind.dart —
        // fields only have the static/instance pair here; top-level fields
        // seal as `TopLevelFields` in `end_top_level_fields`).
        let member_kind = if static_kw.is_some() {
            MemberKind::StaticField
        } else {
            MemberKind::NonStaticField
        };
        self.b.patch_field(PayloadField::MemberKind, member_kind as u32);
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
        self.b.patch_field(PayloadField::DeclarationKind, kind as u32);
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
        self.patch_token(PayloadField::NewKeyword, new_kw);
        self.patch_token(PayloadField::ParamStart, Some(begin_param));
        self.patch_token(PayloadField::InitializersStart, begin_initializers);
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
        self.b.patch_field(PayloadField::DeclarationKind, kind as u32);
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
        self.patch_token(PayloadField::GetOrSet, get_or_set);
        self.patch_token(PayloadField::ParamStart, Some(begin_param));
        self.patch_token(PayloadField::InitializersStart, begin_initializers);
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
        self.b.patch_field(PayloadField::DeclarationKind, kind as u32);
    }

    fn end_factory(
        &mut self,
        _kind: DeclarationKind,
        _begin: TokenSpan,
        factory: TokenSpan,
        end: TokenSpan,
    ) {
        self.b
            .patch_field(PayloadField::FactoryKeyword, self.t(factory.id));
        self.end_incl(NodeKind::Factory, end);
    }

    fn handle_operator_name(&mut self, operator: TokenSpan, name: TokenSpan) {
        // `operator` is the span head (head-when-present, #174); the words
        // are the name token and the invalid flag (the OperatorName payload).
        self.b.begin(NodeKind::OperatorName, self.t(operator.id));
        self.b.patch_field(PayloadField::Name, self.t(name.id));
        self.b.patch_field(PayloadField::IsInvalid, 0);
        self.b.end(NodeKind::OperatorName, self.t(name.id) + 1);
    }

    fn handle_invalid_operator_name(&mut self, operator: TokenSpan, name: TokenSpan) {
        self.b.begin(NodeKind::OperatorName, self.t(operator.id));
        self.b.patch_field(PayloadField::Name, self.t(name.id));
        self.b.patch_field(PayloadField::IsInvalid, 1);
        self.b.end(NodeKind::OperatorName, self.t(name.id) + 1);
    }

    fn handle_no_identifier(&mut self, token: TokenSpan, _context: IdentifierContext) {
        self.b.leaf(NodeKind::NoIdentifier, self.t(token.id));
    }

    // —— field initializers and constructor initializers —————————————————
    fn begin_field_initializer(&mut self, token: TokenSpan) {
        self.begin(NodeKind::FieldInitializer, token);
        self.b.patch_field(PayloadField::Name, self.t(token.id));
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
        // body kind absorbs them (see the field: LIFO frames — the sentinel
        // pair fires for sync bodies too).
        self.async_pending_stack
            .push((async_kw.map(|t| self.t(t.id)), star.map(|t| self.t(t.id))));
    }

    fn begin_block_function_body(&mut self, token: TokenSpan) {
        self.begin(NodeKind::BlockBody, token);
        // The contract writes both words even when the modifier words are
        // absent (a sync body's sentinel frame) — absent is the explicit
        // sentinel. The unpaired face (only-on-parser-bug) patches the same
        // absent sentinels, so the body node itself is unaffected.
        let popped = self.async_pending_stack.pop();
        let (async_kw, star) = self
            .expect_paired("async modifier pair", popped)
            .unwrap_or((None, None));
        self.b
            .patch_field(PayloadField::AsyncKeyword, async_kw.unwrap_or(TOKEN_NONE));
        self.b.patch_field(PayloadField::Star, star.unwrap_or(TOKEN_NONE));
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
        self.b.patch_field(
            PayloadField::IsExpressionBody,
            u32::from(is_expression_body),
        );
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
        self.b.patch_field(PayloadField::Arrow, self.t(arrow.id));
        self.patch_token(PayloadField::Semicolon, end);
        // Unpaired face (only-on-parser-bug) rides the absent sentinels,
        // same as `begin_block_function_body`.
        let popped = self.async_pending_stack.pop();
        let (async_kw, star) = self
            .expect_paired("async modifier pair", popped)
            .unwrap_or((None, None));
        self.b
            .patch_field(PayloadField::AsyncKeyword, async_kw.unwrap_or(TOKEN_NONE));
        self.b.patch_field(PayloadField::Star, star.unwrap_or(TOKEN_NONE));
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
        self.b
            .patch_field(PayloadField::WithKeyword, self.t(with_keyword.id));
    }

    fn handle_class_with_clause(&mut self, with_keyword: TokenSpan) {
        self.b
            .patch_field(PayloadField::WithKeyword, self.t(with_keyword.id));
    }

    fn handle_class_no_with_clause(&mut self) {
        self.b.patch_field(PayloadField::WithKeyword, TOKEN_NONE);
    }

    fn handle_enum_with_clause(&mut self, _with_keyword: TokenSpan) {
        // Enums have no with clause; the event exists only for the grammar
        // error — diagnostic face only.
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
            self.patch_token(PayloadField::ImplementsKeyword, implements);
        }
    }

    fn handle_class_extends(&mut self, extends: Option<TokenSpan>, _count: usize) {
        if self.class_decl == ClassDecl::Class {
            self.patch_token(PayloadField::ExtendsKeyword, extends);
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
            self.b.patch_field(PayloadField::ClassKeyword, self.t(class.id));
        }
    }

    fn handle_mixin_header(&mut self, _mixin: TokenSpan) {
        // Token already in the span/words (D1).
    }

    fn handle_mixin_on(&mut self, on: Option<TokenSpan>, _count: usize) {
        if self.class_decl == ClassDecl::Mixin {
            self.patch_token(PayloadField::OnKeyword, on);
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
        // with the Mixin kind.
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
        self.b.patch_field(PayloadField::ClassKeyword, self.t(class.id));
        self.b.patch_field(PayloadField::Equals, self.t(equals.id));
        self.patch_token(PayloadField::ImplementsKeyword, implements);
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
        self.b
            .patch_field(PayloadField::MixinKeyword, self.t(mixin.id));
        self.class_decl = ClassDecl::Mixin;
    }

    fn end_mixin_declaration(&mut self, _begin: TokenSpan, end: TokenSpan) {
        // No with-clause event fires on the happy path; the word is written
        // explicitly here.
        self.b.patch_field(PayloadField::WithKeyword, TOKEN_NONE);
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
        self.b
            .patch_field(PayloadField::EnumKeyword, self.t(enum_keyword.id));
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
        self.b.patch_field(
            PayloadField::ElementsEnd,
            self.enum_elements_end.take().unwrap_or(TOKEN_NONE),
        );
        self.end_at(NodeKind::EnumDeclaration, self.t(end.id));
    }

    fn handle_enum_header(
        &mut self,
        _augment: Option<TokenSpan>,
        _enum_keyword: TokenSpan,
        left_brace: TokenSpan,
    ) {
        self.b
            .patch_field(PayloadField::LeftBrace, self.t(left_brace.id));
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
        self.b.patch_field(PayloadField::Name, self.t(begin.id));
        self.patch_token(PayloadField::AugmentKeyword, augment);
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
        self.b
            .patch_field(PayloadField::ExtensionKeyword, self.t(extension_keyword.id));
        self.patch_token(PayloadField::Name, name);
    }

    fn end_extension_declaration(
        &mut self,
        _begin: TokenSpan,
        _extension_keyword: TokenSpan,
        on: Option<TokenSpan>,
        end: TokenSpan,
    ) {
        self.patch_token(PayloadField::OnKeyword, on);
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
        self.b
            .patch_field(PayloadField::ExtensionKeyword, self.t(extension_keyword.id));
        self.b.patch_field(PayloadField::Name, self.t(name.id));
    }

    fn end_extension_type_declaration(
        &mut self,
        _begin: TokenSpan,
        _augment_kw: Option<TokenSpan>,
        _extension_keyword: TokenSpan,
        type_keyword: TokenSpan,
        end: TokenSpan,
    ) {
        self.b
            .patch_field(PayloadField::TypeKeyword, self.t(type_keyword.id));
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
        // word back-fills the `MixinWithClauseHandle` slot.
        if self.class_decl == ClassDecl::Mixin {
            self.b
                .patch_field(PayloadField::WithKeyword, self.t(with_keyword.id));
        }
    }

    fn begin_primary_constructor_body(&mut self, token: TokenSpan, augment: Option<TokenSpan>) {
        self.begin(NodeKind::PrimaryConstructorBody, token);
        self.b.patch_field(PayloadField::ThisKeyword, self.t(token.id));
        self.patch_token(PayloadField::AugmentKeyword, augment);
    }

    fn end_primary_constructor_body(
        &mut self,
        _begin: TokenSpan,
        _this: TokenSpan,
        begin_initializers: Option<TokenSpan>,
        end: TokenSpan,
    ) {
        self.patch_token(PayloadField::InitializersStart, begin_initializers);
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
        _name: Option<TokenSpan>,
    ) {
        self.b.patch_field(PayloadField::DeclarationKind, kind as u32);
        self.patch_token(PayloadField::ConstKeyword, const_keyword);
        self.b.patch_field(
            PayloadField::HasConstructorName,
            u32::from(has_constructor_name),
        );
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
        self.patch_token(PayloadField::ConstKeyword, const_keyword);
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
        self.patch_token(PayloadField::Question, question);
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
        _covariant_kw: Option<TokenSpan>,
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
        self.patch_token(PayloadField::Receiver, this_keyword.or(super_keyword));
        self.b.patch_field(PayloadField::Name, self.t(name.id));
        self.patch_token(PayloadField::InitializerStart, initializer_start);
        self.patch_token(PayloadField::InitializerEnd, initializer_end);
        self.b
            .patch_field(PayloadField::FormalParameterKind, kind as u32);
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
        self.patch_token(PayloadField::ElseKeyword, else_kw);
        self.end_incl(NodeKind::IfStatement, end);
    }

    fn begin_for_statement(&mut self, token: TokenSpan) {
        // Canonical open of the shared for/for-in group: a for-in
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
        // The parser passes the body's last consumed token (inclusive —
        // official parser_impl.dart:10339-10340 `parseForRest` returns the
        // statement's last token), and the official `ForStatementImpl`
        // spans to the body's end token (ast_builder.dart endForStatement:
        // the `body` child decides the tail), so a block body's `}` seals
        // in.
        self.end_incl(NodeKind::ForStatement, end);
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
        self.b
            .patch_field(PayloadField::LeftParen, self.t(left_paren.id));
        self.b
            .patch_field(PayloadField::LeftSeparator, self.t(left_separator.id));
        self.b
            .patch_field(PayloadField::RightSeparator, self.t(right_separator.id));
        self.b.patch_field(
            PayloadField::ExpressionCount,
            expression_count.min(u32::MAX as usize) as u32,
        );
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
        // Seals the shared begin opened as `ForStatement`.
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
        self.patch_token(PayloadField::AwaitKeyword, await_kw);
        self.b
            .patch_field(PayloadField::LeftParen, self.t(left_paren.id));
        self.patch_token(PayloadField::PatternKeyword, pattern_keyword);
        self.b.patch_field(PayloadField::InKeyword, self.t(in_kw.id));
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
        self.patch_token(PayloadField::Star, star);
        self.end_incl(NodeKind::YieldStatement, end);
    }

    fn end_invalid_yield_statement(
        &mut self,
        _begin: TokenSpan,
        star: Option<TokenSpan>,
        end: TokenSpan,
    ) {
        // The error code is resolution-domain, not an AST fact.
        self.patch_token(PayloadField::Star, star);
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
        self.b.patch_field(
            PayloadField::IsFunctionExpression,
            u32::from(is_function_expression),
        );
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
        self.patch_token(PayloadField::LateKeyword, late_kw);
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
        self.patch_token(PayloadField::FinallyKeyword, finally_kw);
        self.end_incl(NodeKind::TryStatement, end);
    }

    fn begin_catch_clause(&mut self, token: TokenSpan) {
        self.begin(NodeKind::CatchClause, token);
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
        // Unconditional: the parser fires exactly one `begin_catch_clause`
        // per `handle_catch_block` (parseTryStatement's loop body always
        // opens the clause before reaching the post-body handle event), and
        // speculative forks swap in their own listener — so this handler
        // always seals its own innermost open `CatchClause`, reentrantly
        // (nested try-in-catch), with the payload words (`catch`, `comma`).
        self.patch_token(PayloadField::CatchKeyword, catch_kw);
        self.patch_token(PayloadField::Comma, comma);
        let _ = on_kw;
        self.end_last(NodeKind::CatchClause);
    }

    fn handle_finally_block(&mut self, finally_kw: TokenSpan) {
        self.b
            .patch_field(PayloadField::FinallyKeyword, self.t(finally_kw.id));
    }

    fn begin_switch_statement(&mut self, token: TokenSpan) {
        self.begin(NodeKind::SwitchStatement, token);
    }

    fn end_switch_statement(&mut self, _token: TokenSpan, end: TokenSpan) {
        self.end_incl(NodeKind::SwitchStatement, end);
    }

    fn begin_switch_block(&mut self, token: TokenSpan) {
        self.begin(NodeKind::SwitchBlock, token);
        // Rows land at seal time (post-order), so the arena tail here is
        // already the first member's first material row — the same face the
        // `end_switch_case` re-anchor below snapshots.
        self.switch_material_start = self.b.len() as u32;
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
        self.patch_token(PayloadField::WhenKeyword, when);
        self.b.patch_field(PayloadField::Colon, self.t(colon.id));
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
        self.patch_token(PayloadField::DefaultKeyword, default_kw);
        self.patch_token(PayloadField::ColonAfterDefault, colon_after_default);
        self.b.patch_field(
            PayloadField::ExpressionCount,
            expression_count.min(u32::MAX as usize) as u32,
        );
        // The bound is the token after the case's statements (peek) — the
        // next case's head or the block's `}`.
        self.end_at(NodeKind::SwitchCase, self.t(end.id));
        // Snapshot AFTER the seal: rows land at seal time (post-order), so
        // the next case's first material row is one past the just-sealed
        // SwitchCase row. A pre-seal snapshot named the SwitchCase row
        // itself, anchoring the next case strictly inside its predecessor
        // (the builder's well-nestedness assert; nested cases in release).
        self.switch_material_start = self.b.len() as u32;
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
        self.b.patch_field(PayloadField::HasTarget, u32::from(has_target));
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
        self.b
            .patch_field(PayloadField::HasTarget, u32::from(has_target));
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
        self.b.patch_field(PayloadField::AssertKind, kind as u32);
        self.b
            .patch_field(PayloadField::LeftParen, self.t(left_paren.id));
        self.patch_token(PayloadField::Comma, comma);
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
        self.patch_token(PayloadField::CaseKeyword, case_kw);
        self.patch_token(PayloadField::WhenKeyword, when);
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
        self.b.patch_field(PayloadField::Equals, self.t(equals.id));
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
        self.string_heads.push(self.t(token.id));
        self.begin(NodeKind::StringLiteral, token);
    }

    fn handle_interpolation_expression(&mut self, _left: TokenSpan, _right: Option<TokenSpan>) {
        // Interpolation is carried by the token layer's string segments (D4).
    }

    fn end_literal_string(&mut self, _interpolation_count: usize, _end: TokenSpan) {
        // The event's token is the literal's last String segment (upstream
        // `endLiteralString`'s endToken contract); the head latched at begin
        // seals the node — interpolation segments produce no child nodes
        // (D4 token-layer carry), so the span anchors at the head and
        // report ranges key off the head offset.
        let popped = self.string_heads.pop();
        let head = self
            .expect_paired("end_literal_string without begin", popped)
            .unwrap_or(0);
        self.b.end(NodeKind::StringLiteral, head + 1);
    }

    fn handle_adjacent_string_literals(&mut self, _start: TokenSpan, _literal_count: usize) {
        // Adjacent concatenation = consecutive StringLiteral siblings (D1).
    }

    fn begin_literal_symbol(&mut self, token: TokenSpan) {
        self.begin(NodeKind::SymbolLiteral, token);
    }

    fn handle_symbol_void(&mut self, token: TokenSpan) {
        self.b.begin(NodeKind::SymbolVoid, self.t(token.id));
        self.b.patch_field(PayloadField::VoidKeyword, self.t(token.id));
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
        // (a childless Handle node, the `TokenLeaf` kind set).
        self.b.leaf(NodeKind::UnaryPrefix, self.t(operator.id));
    }

    fn handle_non_null_assert_expression(&mut self, bang: TokenSpan) {
        self.b.leaf(NodeKind::NonNullAssert, self.t(bang.id));
    }

    fn begin_binary_expression(&mut self, operator: TokenSpan) {
        // The left operand completed before the event (operands×2 contract);
        // the node opens adopting it, anchored at its
        // head token. The anchor-miss face (only-on-parser-bug) opens the
        // node at the operator with no adopted child — the begin/end
        // pairing with the later seal stays balanced, the span degrades.
        match (
            self.expect_paired("binary left operand", self.b.last_first_token()),
            self.expect_paired("binary left operand", self.b.last_subtree_start()),
        ) {
            (Some(first), Some(start)) => {
                self.b
                    .begin_with_first_child(NodeKind::BinaryExpression, first, start);
            }
            _ => self.begin(NodeKind::BinaryExpression, operator),
        }
    }

    fn end_binary_expression(&mut self, operator: TokenSpan, end: TokenSpan) {
        self.b
            .patch_field(PayloadField::Operator, self.t(operator.id));
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
        // span head — `const` sits outside it).
        self.b.begin(NodeKind::ListLiteral, self.t(left.id));
        self.patch_token(PayloadField::ConstKeyword, const_keyword);
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
        self.patch_token(PayloadField::ConstKeyword, const_keyword);
        self.b
            .patch_field(PayloadField::HasSetEntry, u32::from(has_set_entry));
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
        // The anchor-miss face skips the entry emission (one atomic
        // begin+end; the expressions remain siblings of the literal).
        //
        // Span contract: the row covers `[value-head, follower-inclusive]`
        // — `last_first_token` is the VALUE's head (the last expression to
        // complete before the entry event), while the key always stays a
        // preceding sibling and never enters the covered span. The colon
        // payload word (`payload[0]`) is therefore the only reliable cut
        // between the two sides: a consumer splitting key from value must
        // walk off the colon, never off the covered start (a member or
        // send value's head is itself a tile anchor — `k: a.b` opens at
        // `b`'s selector completion). This differs from the
        // `NamedArgument`/`NamedRecordField` markers, which emit
        // zero-width at colon+1 (the value head itself, `handle_*`
        // below): those rows carry no coverage at all, while this row
        // carries the value side's whole extent.
        let Some(first) = self.expect_paired("map entry anchor", self.b.last_first_token()) else {
            return;
        };
        self.b.begin(NodeKind::MapEntry, first);
        self.b.patch_field(PayloadField::Colon, self.t(colon.id));
        self.patch_token(PayloadField::NullAwareKey, null_aware_key);
        self.patch_token(PayloadField::NullAwareValue, null_aware_value);
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
        // Canonical open of the shared group: the wider
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
        self.patch_token(PayloadField::ConstKeyword, const_keyword);
        self.end_incl(NodeKind::RecordLiteral, end);
    }

    fn handle_named_record_field(&mut self, colon: TokenSpan) {
        // Childless Handle over `name :`; the value expression follows as a
        // sibling inside `RecordLiteral` (no end event exists to nest it).
        // The event fires AFTER the value parse (literals.rs:514-516), so
        // `last_first_token` is the value's last-completed node — anchoring
        // there inverted the span on selector-chain values (`x: a.b` seals
        // a `DotAccess` at the selector `b`, past the colon) and on cascade
        // values (`x: a..b` opens `Cascade` at `..`). The marker contract
        // anchors at the value head — the token right after the colon —
        // with the name token at colon-1 (names.rs:576-585), so the
        // emission is zero-width there (one atomic begin+end). Same defect
        // shape and fix as `handle_named_argument`.
        let head = self.t(colon.id) + 1;
        self.b.begin(NodeKind::NamedRecordField, head);
        self.b.patch_field(PayloadField::Colon, self.t(colon.id));
        self.b.end(NodeKind::NamedRecordField, head);
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
        // Distinguishes `x` from `x()` (a real sentinel):
        // a zero-width leaf anchored just past the expression that completed
        // before the empty argument list. No completed anchor is a bridge
        // bug, not token 0 — the event always follows a parsed receiver; the
        // miss face skips the sentinel leaf.
        let Some(anchor) = self.expect_paired(
            "NoArguments without a completed anchor node",
            self.b.last_end_token(),
        ) else {
            return;
        };
        self.b.leaf_span(NodeKind::NoArguments, anchor, anchor);
    }

    fn handle_named_argument(&mut self, colon: TokenSpan) {
        // Childless Handle over `name :`; the value expression follows as a
        // sibling inside `Arguments` (no end event exists to nest it). The
        // event fires AFTER the value parse (:9211), so `last_first_token`
        // is the value's last-completed node — anchoring there inverted the
        // span on send-chain values (`x: a.b` seals a `DotAccess` at the
        // dot). The marker contract anchors at the value head — the token
        // right after the colon — so the emission is zero-width there (one
        // atomic begin+end).
        let head = self.t(colon.id) + 1;
        self.b.begin(NodeKind::NamedArgument, head);
        self.b.patch_field(PayloadField::Colon, self.t(colon.id));
        self.b.end(NodeKind::NamedArgument, head);
    }

    fn handle_positional_argument(&mut self, _token: TokenSpan) {
        // Positional argument = the expression child itself (D1).
    }

    fn handle_send(&mut self, begin: TokenSpan, end: TokenSpan) {
        // A well-formed send spans forward; the case-head recovery shapes
        // (`case true ? 1 : 2:`, `case {1}:`) fire the event with the end
        // token behind the begin — the degenerate face skips the emission
        // (one atomic begin+end), mirroring the anchor-miss skips.
        let (begin_token, end_token) = (self.t(begin.id), self.t(end.id));
        if end_token < begin_token {
            return;
        }
        self.b.leaf_span(NodeKind::Send, begin_token, end_token + 1);
    }

    fn handle_send_without_arguments(
        &mut self,
        begin: TokenSpan,
        end: TokenSpan,
        _next: TokenSpan,
    ) {
        // Merged into `Send` (argument presence reads from the children).
        // The same degenerate face as `handle_send` skips the emission.
        let (begin_token, end_token) = (self.t(begin.id), self.t(end.id));
        if end_token < begin_token {
            return;
        }
        self.b.leaf_span(NodeKind::Send, begin_token, end_token + 1);
    }

    fn handle_dot_access(&mut self, token: TokenSpan, end: TokenSpan, is_null_aware: bool) {
        // Childless Handle over the selector extent (`token` = the dot).
        // The anchor-miss face skips the node emission (one atomic
        // begin+end; the receiver remains a sibling).
        let Some(first) = self.expect_paired("dot access anchor", self.b.last_first_token()) else {
            return;
        };
        self.b.begin(NodeKind::DotAccess, first);
        self.b.patch_field(PayloadField::Operator, self.t(token.id));
        self.b
            .patch_field(PayloadField::IsNullAware, u32::from(is_null_aware));
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
        self.patch_token(PayloadField::Question, question);
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
        self.patch_token(PayloadField::PeriodBeforeName, period_before_name);
        self.b
            .patch_field(PayloadField::ConstructorContext, context as u32);
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
        // Shared begin with `AwaitExpression`; the error code is
        // resolution-domain.
        self.end_incl(NodeKind::InvalidAwaitExpression, end);
    }

    fn handle_throw_expression(&mut self, throw: TokenSpan, _end: TokenSpan) {
        // Childless Handle: the operand is the preceding sibling.
        self.b.leaf(NodeKind::ThrowExpression, self.t(throw.id));
    }

    fn handle_assignment_expression(&mut self, operator: TokenSpan, end: TokenSpan) {
        // Both operands completed before the event (operands×2). The
        // anchor-miss face (only-on-parser-bug) opens the node at the
        // operator with no adopted child and seals the same way — the
        // emission stays balanced, the span degrades.
        match (
            self.expect_paired("assignment lhs", self.b.last_first_token()),
            self.expect_paired("assignment lhs", self.b.last_subtree_start()),
        ) {
            (Some(first), Some(start)) => {
                self.b
                    .begin_with_first_child(NodeKind::Assignment, first, start);
            }
            _ => self.begin(NodeKind::Assignment, operator),
        }
        self.b
            .patch_field(PayloadField::Operator, self.t(operator.id));
        self.b.end(NodeKind::Assignment, self.t(end.id) + 1);
    }

    fn begin_cascade(&mut self, operator: TokenSpan) {
        self.begin(NodeKind::Cascade, operator);
    }

    fn handle_cascade_access(&mut self, operator: TokenSpan, end: TokenSpan, is_null_aware: bool) {
        // The anchor-miss face skips the node emission (one atomic
        // begin+end; the cascade target remains a sibling).
        let Some(first) = self.expect_paired("cascade access anchor", self.b.last_first_token())
        else {
            return;
        };
        self.b.begin(NodeKind::CascadeAccess, first);
        self.b
            .patch_field(PayloadField::Operator, self.t(operator.id));
        self.b
            .patch_field(PayloadField::IsNullAware, u32::from(is_null_aware));
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
        self.b
            .patch_field(PayloadField::Question, self.t(question.id));
        self.b.patch_field(PayloadField::Colon, self.t(colon.id));
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
        self.patch_token(PayloadField::OpenAngle, Some(open_angle));
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
        // expression follows as siblings). The anchor-miss face skips the
        // node emission (one atomic begin+end).
        let Some(first) =
            self.expect_paired("pattern assignment anchor", self.b.last_first_token())
        else {
            return;
        };
        self.b.begin(NodeKind::PatternAssignment, first);
        self.b.patch_field(PayloadField::Equals, self.t(equals.id));
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
            // The payload word is the `not` flag.
            self.b
                .patch_field(PayloadField::IsNegated, u32::from(not.is_some()));
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
        // Merged with end_as_operator_type (the End event and the Handle
        // event seal through this one path).
        if self.deferred_seal == Some(NodeKind::AsExpression) {
            self.deferred_seal = None;
            self.b
                .patch_field(PayloadField::Operator, self.t(operator.id));
            self.end_last(NodeKind::AsExpression);
        }
    }

    fn handle_type_argument_application(&mut self, open_angle: TokenSpan) {
        // `f<int>()` marker leaf over the applied type arguments. The
        // anchor-miss face skips the marker emission (one atomic
        // begin+end).
        let Some(first) = self.expect_paired("type application anchor", self.b.last_first_token())
        else {
            return;
        };
        self.b.begin(NodeKind::TypeApplication, first);
        self.b
            .patch_field(PayloadField::OpenAngle, self.t(open_angle.id));
        self.b
            .end(NodeKind::TypeApplication, self.t(open_angle.id) + 1);
    }

    // —— patterns ——————————————————————————————————————————————————————
    fn begin_pattern(&mut self, _token: TokenSpan) {
        // Pattern wrapper dropped (derivable): pattern nodes land
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
            // The anchor-miss face skips the pattern emission (one atomic
            // begin+end).
            let Some(start) =
                self.expect_paired("constant pattern operand", self.b.last_subtree_start())
            else {
                return;
            };
            self.b
                .begin_with_first_child(NodeKind::ConstantPattern, head, start);
        } else {
            // Same miss face; reached only with a completed node (the
            // adoption below requires one).
            let Some(first) =
                self.expect_paired("constant pattern operand", self.b.last_first_token())
            else {
                return;
            };
            self.b.begin_adopting(NodeKind::ConstantPattern, first, 1);
        }
        self.end_last(NodeKind::ConstantPattern);
    }

    fn handle_relational_pattern(&mut self, operator: TokenSpan) {
        // Fires after the operand parsed (childless Handle; `op` word). The
        // anchor-miss face skips the pattern emission (one atomic
        // begin+end).
        let Some(first) =
            self.expect_paired("relational pattern anchor", self.b.last_first_token())
        else {
            return;
        };
        self.b.begin(NodeKind::RelationalPattern, first);
        self.b
            .patch_field(PayloadField::Operator, self.t(operator.id));
        self.end_last(NodeKind::RelationalPattern);
    }

    fn handle_cast_pattern(&mut self, operator: TokenSpan) {
        // The cast's type parses inside the same as-operator-type window an
        // expression cast does (`beginAsOperatorType`/`endAsOperatorType`,
        // parser_impl.dart's `parseCastPattern`), which opened the deferred
        // `AsExpression` — but no `handleAsOperator` follows a pattern, so
        // the deferred seal closes here or the open stack desynchronizes
        // (`end CaseExpression does not pair with open AsExpression`).
        if self.deferred_seal == Some(NodeKind::AsExpression) {
            self.deferred_seal = None;
            self.b
                .patch_field(PayloadField::Operator, self.t(operator.id));
            self.end_last(NodeKind::AsExpression);
        }
        // The anchor-miss face skips the pattern emission (one atomic
        // begin+end).
        let Some(first) = self.expect_paired("cast pattern anchor", self.b.last_first_token())
        else {
            return;
        };
        self.b.begin(NodeKind::CastPattern, first);
        self.b
            .patch_field(PayloadField::Operator, self.t(operator.id));
        self.end_last(NodeKind::CastPattern);
    }

    fn handle_null_assert_pattern(&mut self, bang: TokenSpan) {
        self.b.leaf(NodeKind::NullAssertPattern, self.t(bang.id));
    }

    fn handle_null_check_pattern(&mut self, question: TokenSpan) {
        self.b.leaf(NodeKind::NullCheckPattern, self.t(question.id));
    }

    fn begin_binary_pattern(&mut self, operator: TokenSpan) {
        // The left operand completed before the event (operands×2). The
        // anchor-miss face (only-on-parser-bug) opens the node at the
        // operator with no adopted child — the begin/end pairing with the
        // later seal stays balanced, the span degrades.
        match (
            self.expect_paired("binary pattern lhs", self.b.last_first_token()),
            self.expect_paired("binary pattern lhs", self.b.last_subtree_start()),
        ) {
            (Some(first), Some(start)) => {
                self.b
                    .begin_with_first_child(NodeKind::BinaryPattern, first, start);
            }
            _ => self.begin(NodeKind::BinaryPattern, operator),
        }
    }

    fn end_binary_pattern(&mut self, operator: TokenSpan) {
        self.b
            .patch_field(PayloadField::Operator, self.t(operator.id));
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
        // The anchor-miss face skips the entry emission (one atomic
        // begin+end).
        let Some(first) = self.expect_paired("map pattern entry anchor", self.b.last_first_token())
        else {
            return;
        };
        self.b.begin(NodeKind::MapPatternEntry, first);
        self.b.patch_field(PayloadField::Colon, self.t(colon.id));
        self.b.end(NodeKind::MapPatternEntry, self.t(end.id) + 1);
    }

    fn handle_rest_pattern(&mut self, dots: TokenSpan, has_sub_pattern: bool) {
        self.b.begin(NodeKind::RestPattern, self.t(dots.id));
        self.b.patch_field(PayloadField::Dots, self.t(dots.id));
        self.b
            .patch_field(PayloadField::HasSubPattern, u32::from(has_sub_pattern));
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
        // The anchor-miss face skips the field emission (one atomic
        // begin+end).
        let Some(first) = self.expect_paired("pattern field anchor", self.b.last_first_token())
        else {
            return;
        };
        self.b.begin(NodeKind::PatternField, first);
        self.patch_token(PayloadField::Colon, colon);
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
        // (ast_builder.dart:5362-5397). The unpaired face (only-on-parser-bug:
        // no fields event preceded) skips the node emission entirely — the
        // reassembly window is exactly what is missing, so there is no
        // bounded seal to fabricate, and nothing was begun to leave open.
        let pending = self.object_pattern_pending.take();
        let Some((first_child, end)) = self.expect_paired(
            "handle_object_pattern without a preceding fields event",
            pending,
        ) else {
            return;
        };
        self.b.begin_with_first_child(
            NodeKind::ObjectPattern,
            self.t(first_identifier.id),
            first_child,
        );
        self.b
            .patch_field(PayloadField::FirstName, self.t(first_identifier.id));
        self.patch_token(PayloadField::Period, dot);
        self.patch_token(PayloadField::SecondName, second_identifier);
        self.b.end(NodeKind::ObjectPattern, end);
    }

    fn handle_object_pattern_fields(&mut self, _count: usize, _begin: TokenSpan, end: TokenSpan) {
        self.object_pattern_pending = Some((self.b.len() as u32, self.t(end.id) + 1));
    }

    fn handle_wildcard_pattern(&mut self, keyword: Option<TokenSpan>, wildcard: TokenSpan) {
        let head = keyword.unwrap_or(wildcard);
        self.b.begin(NodeKind::WildcardPattern, self.t(head.id));
        self.b.patch_field(PayloadField::Wildcard, self.t(wildcard.id));
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
        self.b.patch_field(PayloadField::Variable, self.t(variable.id));
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

    fn handle_else_control_flow(&mut self, else_kw: TokenSpan, _else_start: TokenSpan) {
        self.b.leaf(NodeKind::ElseControlFlow, self.t(else_kw.id));
    }

    fn end_if_control_flow(&mut self, token: TokenSpan) {
        self.end_incl(NodeKind::IfControlFlow, token);
    }

    fn end_if_else_control_flow(&mut self, token: TokenSpan) {
        // Same kind seal — the else segment reads from the ElseControlFlow
        // sentinel child.
        self.end_incl(NodeKind::IfControlFlow, token);
    }

    fn begin_for_control_flow(&mut self, await_kw: Option<TokenSpan>, for_kw: TokenSpan) {
        let head = await_kw.unwrap_or(for_kw);
        self.begin(NodeKind::ForControlFlow, head);
        self.patch_token(PayloadField::AwaitKeyword, await_kw);
    }

    fn end_for_control_flow(&mut self, token: TokenSpan) {
        self.b.patch_field(PayloadField::IsForIn, 0);
        self.end_incl(NodeKind::ForControlFlow, token);
    }

    fn end_for_in_control_flow(&mut self, token: TokenSpan) {
        self.b.patch_field(PayloadField::IsForIn, 1);
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
        self.patch_token(PayloadField::WhenKeyword, when);
        self.b.patch_field(PayloadField::Arrow, self.t(arrow.id));
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
        // error bit on the enclosing nodes (the
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
        run(&unit, &set, &rustc_hash::FxHashSet::default())
    }

    /// Walks the arena collecting (kind, utf16 start) pairs. `children`
    /// spans whole subtrees, so dedupe by node id — each node is visited
    /// exactly once.
    fn walk(source: &str) -> Vec<(NodeKind, u32)> {
        let parsed = parse_unit(source, Features::default());
        let mut out = Vec::new();
        let mut seen = rustc_hash::FxHashSet::default();
        let mut stack = vec![parsed.ast.root().expect("root")];
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
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
    fn collection_for_bare_identifier_element_nests_the_loop_variable() {
        // The collection-for element face under a top-level field: the loop
        // variable's `VariablesDeclaration` row must nest inside the
        // `ForControlFlow` container — a row landing directly under the
        // `TopLevelFields` wrapper would read as a ghost top-level variable
        // (the #1993/#2013 face).
        let parsed = parse_unit("var c = [for (var i in [1]) i];\n", Features::default());
        let ast = &parsed.ast;
        let mut top_level_decls = Vec::new();
        let mut loop_var_parent = None;
        for row in 0..ast.len() as u32 {
            if ast.kind(row) != NodeKind::VariablesDeclaration {
                continue;
            }
            match ast.parent(row).map(|p| (p, ast.kind(p))) {
                Some((_, NodeKind::TopLevelFields)) => top_level_decls.push(row),
                Some((p, NodeKind::ForControlFlow)) => loop_var_parent = Some(p),
                _ => {}
            }
        }
        assert_eq!(
            top_level_decls.len(),
            1,
            "exactly the `c` row under TopLevelFields"
        );
        assert!(
            loop_var_parent.is_some(),
            "loop variable nests in ForControlFlow"
        );
    }

    #[test]
    fn for_in_bare_identifier_body_keeps_one_declaration_row() {
        // The statement face with a bare-identifier body statement: exactly
        // one `VariablesDeclaration` row (the loop variable), nested in the
        // for container — no recovery artifact under the enclosing block.
        let parsed = parse_unit(
            "void f(List<int> l) {\n  for (final s in l) {\n    s;\n  }\n}\n",
            Features::default(),
        );
        let ast = &parsed.ast;
        let decls: Vec<u32> = (0..ast.len() as u32)
            .filter(|&row| ast.kind(row) == NodeKind::VariablesDeclaration)
            .collect();
        assert_eq!(decls.len(), 1, "exactly the loop variable: {decls:?}");
        let parent_kind = decls
            .first()
            .copied()
            .and_then(|row| ast.parent(row))
            .map(|p| ast.kind(p));
        assert!(
            matches!(
                parent_kind,
                Some(NodeKind::ForIn) | Some(NodeKind::ForControlFlow)
            ),
            "loop variable nests in the for container: {parent_kind:?}"
        );
    }

    #[test]
    fn nested_try_in_catch_seals_both_catch_clauses() {
        // Regression (#991): the deferred `CatchClause` seal used a single
        // non-reentrant flag — an inner try-in-catch cleared it, the outer
        // `handle_catch_block` guard-skipped, and the outer clause never
        // sealed (pairing assert in debug, silent mis-root in release).
        // The seal is now unconditional, so both clauses close.
        let source = "void f() {\n  try {\n    g();\n  } catch (e) {\n    try {\n      h();\n    } catch (e2) {\n      p();\n    }\n  }\n}\n";
        // `children` spans whole subtrees, so dedupe by node id — each node
        // is counted exactly once (the shared `walk` helper only serves
        // `contains` checks).
        let parsed = parse_unit(source, Features::default());
        let ast = &parsed.ast;
        let mut seen = rustc_hash::FxHashSet::default();
        let mut stack = vec![ast.root().expect("root")];
        let mut tries = 0u32;
        let mut catches = 0u32;
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            match ast.kind(id) {
                NodeKind::TryStatement => tries += 1,
                NodeKind::CatchClause => catches += 1,
                _ => {}
            }
            stack.extend(ast.children(id).rev());
        }
        assert_eq!(tries, 2, "both try statements present");
        assert_eq!(catches, 2, "both catch clauses sealed");
        // The clause-consuming rule runs over the fully sealed tree.
        assert!(hits(source, "empty_catches").is_empty());
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
        let mut seen = rustc_hash::FxHashSet::default();
        let mut stack = vec![parsed.ast.root().expect("root")];
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
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

    // —— metadata-star / async-marker nesting pins ———————————————————————
    // The star pair and the async-modifier pair fire NESTED on legal Dart
    // (annotation arguments re-enter the star grammar; nested arrow bodies
    // re-enter the modifier pair), so both frames live on stacks. Running a
    // shape through the bridge exercises the pop-side `expect`s — an
    // unbalanced pair panics the parse — and each test pins the OUTER seal:
    // the nested completion must not shrink or move it.

    /// Every `MetadataStar` extent (UTF-16). The star grammar re-enters
    /// through annotation arguments (type variables, annotated formals,
    /// record-type fields, annotation type arguments), so one unit carries
    /// nested pairs: the outermost spans the annotation union, count-0
    /// nested pairs seal zero-width at their anchor.
    fn metadata_star_spans(source: &str) -> Vec<core::ops::Range<u32>> {
        let parsed = parse_unit(source, Features::default());
        let mut out = Vec::new();
        // `children` spans whole subtrees; dedupe by node id so each star is
        // counted exactly once.
        let mut seen = rustc_hash::FxHashSet::default();
        let mut stack = vec![parsed.ast.root().expect("root")];
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            if parsed.ast.kind(id) == NodeKind::MetadataStar {
                out.push(utf16_span(&parsed.source, &parsed.tokens, &parsed.ast, id));
            }
            stack.extend(parsed.ast.children(id).rev());
        }
        out
    }

    #[test]
    fn metadata_star_nested_gft_type_vars_keeps_outer_end() {
        // `@A(<T>() => 0)`: the literal's `<T>` takes the simple
        // type-variables path, which fires a count-0 star pair inside the
        // annotation's argument window. With the pre-#1283 single slot the
        // inner completion clobbered the outer frame and the outer seal
        // snapped to the inner pair's end.
        let source = "@A(<T>() => 0)\nvar x = 1;\n";
        let spans = metadata_star_spans(source);
        assert!(
            spans.contains(&(0..14)),
            "outer star spans `@A(<T>() => 0)`, got {spans:?}"
        );
        assert!(
            spans.contains(&(4..4)),
            "count-0 pair fired inside `<T>`, got {spans:?}"
        );
    }

    #[test]
    fn metadata_star_nested_annotated_parameter_keeps_outer_end() {
        // `@A((@meta int x) => x)`: every formal opens with the star grammar
        // (upstream Dart SDK source, pkg/_fe_analyzer_shared/lib/src/parser/
        // parser_impl.dart `parseFormalParameter`), so the literal's
        // annotated parameter fires a NON-empty nested pair (`@meta`).
        let source = "@A((@meta int x) => x)\nvar x = 1;\n";
        let spans = metadata_star_spans(source);
        assert!(
            spans.contains(&(0..22)),
            "outer star spans the whole annotation, got {spans:?}"
        );
        assert!(
            spans.contains(&(4..9)),
            "nested star spans the formal's `@meta`, got {spans:?}"
        );
    }

    #[test]
    fn metadata_star_nested_record_type_cast_keeps_outer_end() {
        // `@A(x as (int, String))`: every record-type field opens its own
        // star pair (`recordTypeField ::= metadata type identifier?`), two
        // count-0 pairs inside the cast type, inside the argument window.
        let source = "@A(x as (int, String))\nvar x = 1;\n";
        let spans = metadata_star_spans(source);
        assert!(
            spans.contains(&(0..22)),
            "outer star spans the whole annotation, got {spans:?}"
        );
        assert!(
            spans.contains(&(9..9)) && spans.contains(&(14..14)),
            "a count-0 pair per record field, got {spans:?}"
        );
    }

    #[test]
    fn metadata_star_nested_annotation_type_args_keeps_outer_end() {
        // `@A<void Function<T>()>()`: the annotation's own type-argument
        // parse carries the GFT's `<T>` pair entirely between the outer
        // star's begin and end events — the seal must still cover name +
        // type args + arguments.
        let source = "@A<void Function<T>()>()\nvar x = 1;\n";
        let spans = metadata_star_spans(source);
        assert!(
            spans.contains(&(0..24)),
            "outer star spans name + type args + args, got {spans:?}"
        );
        assert!(
            spans.contains(&(17..17)),
            "count-0 pair fired inside `<T>`, got {spans:?}"
        );
    }

    #[test]
    fn async_arrow_nested_async_arrow_keeps_outer_marker() {
        // `void f() async => g(() async => 1);`: the outer arrow body's
        // window spans the whole argument expression, so the inner literal's
        // modifier/body events nest inside it — with the pre-#1283 single
        // pending slot the outer `async` was lost to the inner body.
        // Async-ness rides the ExpressionBody payload (word 2 = `async`
        // keyword, word 3 = `*`, TOKEN_NONE sentinel): every body must carry
        // its OWN keyword.
        let source = "void f() async => g(() async => 1);\n";
        let parsed = parse_unit(source, Features::default());
        // `children` spans whole subtrees; dedupe by node id so each body is
        // counted exactly once.
        let mut seen = rustc_hash::FxHashSet::default();
        let mut stack = vec![parsed.ast.root().expect("root")];
        // (body span start, `async` keyword offset; u32::MAX = lost marker)
        let mut bodies: Vec<(u32, u32)> = Vec::new();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            if parsed.ast.kind(id) == NodeKind::ExpressionBody {
                let async_word =
                    parsed.ast.field(id, PayloadField::AsyncKeyword).expect(
                        "ExpressionBody carries the async modifier word",
                    );
                let offset = if async_word == TOKEN_NONE {
                    u32::MAX
                } else {
                    parsed
                        .tokens
                        .get(async_word as usize)
                        .expect("async word token")
                        .utf16_offset
                };
                let span = utf16_span(&parsed.source, &parsed.tokens, &parsed.ast, id);
                bodies.push((span.start, offset));
            }
            stack.extend(parsed.ast.children(id).rev());
        }
        bodies.sort_unstable();
        assert_eq!(
            bodies,
            [(15, 9), (29, 23)],
            "outer (arrow at 15) and inner (arrow at 29) bodies each keep their own `async`"
        );
    }

    // —— parser↔bridge pairing-miss pins —————————————————————————————————————
    // The audited pairing `expect`s degrade through `expect_paired` instead
    // of panicking the lint pass. These pins drive `AstListener` directly
    // with deliberately mismatched event streams (the production parser
    // cannot emit them — that is the invariant) and assert the degrade
    // shape: no panic, the affected emission skipped or re-anchored, and
    // bounded bookkeeping. A base above every synthetic token id makes
    // `t()` the identity.

    fn listener() -> AstListener {
        AstListener::new(u32::MAX)
    }

    fn span(id: u32) -> TokenSpan {
        TokenSpan {
            id,
            range: dartq_source::SourceRange {
                offset: id * 2,
                length: 1,
            },
        }
    }

    #[test]
    fn unpaired_metadata_star_end_skips_and_records() {
        // bridge.rs `end_metadata_star`: an end without a begin must not
        // seal (it would pop an unrelated open node) — it records the miss
        // and skips, leaving the builder untouched.
        let mut l = listener();
        l.end_metadata_star(0);
        assert_eq!(l.pairing_miss, Some("metadata star pair"));
        assert_eq!(l.pairing_miss_count, 1);
        assert!(l.b.is_empty(), "no node was fabricated");
        // Repeated mismatches only bump the saturating count; the first
        // invariant text is retained (bounded bookkeeping, never a Vec).
        for _ in 0..1000 {
            l.end_metadata_star(0);
        }
        assert_eq!(l.pairing_miss, Some("metadata star pair"));
        assert_eq!(l.pairing_miss_count, 1001);
    }

    #[test]
    fn unpaired_async_modifier_defaults_to_absent_sentinels() {
        // `begin_block_function_body`: the unpaired face (no modifier event
        // fired) patches the same TOKEN_NONE sentinels a sync body's
        // sentinel frame writes, and the body node still begins/seals (the
        // later end event stays paired).
        let mut l = listener();
        l.begin_block_function_body(span(1));
        assert_eq!(l.pairing_miss, Some("async modifier pair"));
        l.end_block_function_body(0, span(1), span(2));
        assert_eq!(l.b.len(), 1, "the body node sealed balanced");
    }

    #[test]
    fn anchor_miss_skips_the_no_arguments_sentinel() {
        // `handle_no_arguments`: without a completed anchor node the
        // zero-width sentinel is skipped, never fabricated at token 0.
        let mut l = listener();
        l.handle_no_arguments(span(3));
        assert_eq!(
            l.pairing_miss,
            Some("NoArguments without a completed anchor node")
        );
        assert!(l.b.is_empty(), "the sentinel leaf was skipped");
    }

    #[test]
    fn object_pattern_without_fields_event_skips_the_node() {
        // `handle_object_pattern`: the pending fields window is exactly
        // what the reassembly needs; without it the emission is skipped
        // and nothing is left open.
        let mut l = listener();
        l.handle_object_pattern(span(1), None, None);
        assert_eq!(
            l.pairing_miss,
            Some("handle_object_pattern without a preceding fields event")
        );
        assert!(l.b.is_empty(), "the node emission was skipped");
    }

    #[test]
    fn end_last_without_a_child_seals_zero_width_at_the_head() {
        // `end_last` (the `end_show` route): the node is already open, so
        // the seal must still happen — it degrades to a zero-width seal at
        // the stream head instead of panicking.
        let mut l = listener();
        l.begin_show(span(0));
        l.end_show(span(0));
        assert_eq!(l.pairing_miss, Some("end_last before any node"));
        assert_eq!(l.b.len(), 1, "the opened node was sealed, not dropped");
    }

    #[test]
    fn binary_begin_without_lhs_reanchors_at_the_operator() {
        // `begin_binary_expression`: the later end event must find its
        // open node, so the miss face opens at the operator with no
        // adopted child instead of skipping the begin.
        let mut l = listener();
        l.begin_binary_expression(span(1));
        assert_eq!(l.pairing_miss, Some("binary left operand"));
        assert_eq!(l.pairing_miss_count, 2, "first-token and subtree reads");
        l.end_binary_expression(span(1), span(2));
        assert_eq!(l.b.len(), 1, "the node sealed balanced");
    }

    #[test]
    fn healthy_stream_records_zero_pairing_misses() {
        // The fallback is observably unreachable on well-formed event
        // streams: full production parses over the constructs each audited
        // site serves must record no miss at all (and the existing
        // healthy-input pin tests above pin the unchanged rows).
        for source in [
            "void f() {\n  if (b) ; else ;\n}\n",
            "@A(<T>() => 0)\nvar x = 1;\n",
            "void f(Object o) {\n  if (o case Rect(w: var v)) print(1);\n}\n",
            "void f() async => g(() async => 1);\n",
            "void f() { a * b + c; }\n",
            "import 'a.dart' deferred as a;\nvar m = {1: 2, 3: 4};\nvar r = (x: 1, 2);\nvar s = switch (r) { (x: 1, _) => 0, _ => 1 };\nclass C extends B implements I {\n  int x = 1, y = 2;\n}\n",
        ] {
            let mut files = dartq_source::FileTable::default();
            let file = files.intern(Path::new("unit.dart"));
            let scanned = scan(source, SCAN_CONFIG, file);
            let mut tokens = scanned.tokens;
            let base = tokens.len() as u32;
            let mut listener = AstListener::new(base);
            let inserted =
                dartq_parser::parse_unit_with(source, &tokens, PARSE_FEATURES, &mut listener);
            tokens.append_inserted(inserted);
            assert!(
                listener.pairing_miss.is_none(),
                "healthy parse of {source:?} recorded {:?}",
                listener.pairing_miss
            );
            assert_eq!(listener.pairing_miss_count, 0);
            let _ = listener.finish();
        }
    }

    #[test]
    fn parse_unit_with_tokens_matches_a_fresh_scan_parse() {
        // The lint fan-out adopts the semantic fold's scanned table into the
        // parse instead of rescanning the file (identical scan posture, the
        // same job-local bytes — the commands side pins the posture with a
        // const assert). The adopted-table parse must be indistinguishable
        // from the fresh-scan parse: unified token table elementwise (the
        // fold faces read that stream by index), begin/end pairing included,
        // and the full AST kind/span walk. The fixture forces parser
        // insertions (missing `]` closer, missing `)` in the if-header), so
        // `append_inserted` runs on both sides — the comparison covers the
        // unified tables, not a clean-file coincidence.
        let source = "var r = switch (x) { _ => [1, 2 };\nvoid f() { if (a { g(); }\n";
        let mut files = dartq_source::FileTable::default();
        let file = files.intern(Path::new("unit.dart"));
        let scanned = scan(source, SCAN_CONFIG, file).tokens;
        let adopted = parse_unit_with_tokens(source, scanned, Features::default());
        let fresh = parse_unit(source, Features::default());
        assert_eq!(adopted.tokens.len(), fresh.tokens.len(), "table lengths");
        for (index, (a, f)) in adopted
            .tokens
            .as_slice()
            .iter()
            .zip(fresh.tokens.as_slice())
            .enumerate()
        {
            assert_eq!(a, f, "token {index} diverges");
            assert_eq!(
                adopted.tokens.matching_end(index),
                fresh.tokens.matching_end(index),
                "pairing of token {index} diverges"
            );
        }
        let mut seen_a = rustc_hash::FxHashSet::default();
        let mut seen_f = rustc_hash::FxHashSet::default();
        let mut stack_a = vec![adopted.ast.root().expect("root")];
        let mut stack_f = vec![fresh.ast.root().expect("root")];
        while let (Some(a), Some(f)) = (stack_a.pop(), stack_f.pop()) {
            if seen_a.insert(a) {
                stack_a.extend(adopted.ast.children(a));
            }
            if seen_f.insert(f) {
                stack_f.extend(fresh.ast.children(f));
            }
            assert_eq!(adopted.ast.kind(a), fresh.ast.kind(f), "node kind");
            assert_eq!(
                adopted.ast.token_range(a),
                fresh.ast.token_range(f),
                "node span"
            );
        }
        assert!(
            stack_a.is_empty() && stack_f.is_empty(),
            "tree shapes diverge"
        );
    }

    #[test]
    fn plugin_parity_corpus_records_zero_pairing_misses() {
        // The plugin-parity corpus (tests/plugin-validation/parity/**/lib)
        // is the input set the daemon arm was walking when the pre-#1786
        // `metadata star pair` `expect` panicked the parse. These pins parse
        // every corpus trigger file through the real scanner/parser/listener
        // stack and assert the audited pairing contract holds — the corpus
        // is the production boundary, so it must record no miss.
        let parity =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/plugin-validation/parity");
        let mut files: Vec<std::path::PathBuf> = Vec::new();
        let mut dirs = vec![parity];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).expect("corpus dir") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "dart") {
                    files.push(path);
                }
            }
        }
        files.sort();
        assert!(
            files.len() >= 16,
            "the parity corpus ships >= 16 triggers, found {}",
            files.len()
        );
        for path in files {
            let source = std::fs::read_to_string(&path).expect("corpus file");
            let mut files = dartq_source::FileTable::default();
            let file = files.intern(Path::new("unit.dart"));
            let scanned = scan(&source, SCAN_CONFIG, file);
            let mut tokens = scanned.tokens;
            let base = tokens.len() as u32;
            let mut listener = AstListener::new(base);
            let inserted =
                dartq_parser::parse_unit_with(&source, &tokens, PARSE_FEATURES, &mut listener);
            tokens.append_inserted(inserted);
            assert!(
                listener.pairing_miss.is_none(),
                "corpus file {} recorded {:?}",
                path.display(),
                listener.pairing_miss
            );
            assert_eq!(listener.pairing_miss_count, 0);
            let _ = listener.finish();
        }
    }

    #[test]
    fn named_record_field_anchors_at_the_value_head() {
        // Regression: `handle_named_record_field` fired AFTER the value
        // parse, so anchoring at `last_first_token` landed inside the value
        // — `a.b` seals `DotAccess` at the selector `b`, `a..b` opens
        // `Cascade` at `..`, both past the colon — and the seal's
        // `end < first` tripped the pairing assertion. The marker contract
        // is zero-width at the value head (the token right after the
        // colon), with the colon payload and the name token at colon-1.
        let source = "var r = (x: a.b, y: a..add(1), z: double.infinity, w: 1);\n";
        let parsed = parse_unit(source, Features::default());
        // `children` spans whole subtrees; dedupe by node id so each node
        // is counted exactly once.
        let mut seen = rustc_hash::FxHashSet::default();
        let mut fields: Vec<(u32, core::ops::Range<u32>)> = Vec::new();
        let mut stack = vec![parsed.ast.root().expect("root")];
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            if parsed.ast.kind(id) == NodeKind::NamedRecordField {
                let range = parsed.ast.token_range(id);
                fields.push((id, range));
            }
            stack.extend(parsed.ast.children(id).rev());
        }
        assert_eq!(fields.len(), 4, "one marker per named field");
        // Value-head utf16 offsets: `a` of `a.b` at 12, `a` of `a..add(1)`
        // at 20, `double` at 34, `1` at 54.
        let mut spans: Vec<u32> = fields
            .iter()
            .map(|&(_, ref range)| {
                assert_eq!(range.start, range.end, "zero-width marker");
                parsed
                    .tokens
                    .get(range.start as usize)
                    .expect("anchor token")
                    .utf16_offset
            })
            .collect();
        spans.sort_unstable();
        assert_eq!(spans, [12, 20, 34, 54]);
        for (id, range) in fields {
            let colon = parsed
                .ast
                .field(id, PayloadField::Colon)
                .expect("NamedRecordField carries the colon word") as usize;
            assert_eq!(
                parsed.tokens.get(colon).map(|t| t.kind),
                Some(dartq_scanner::TokenKind::Colon),
                "payload word 0 is the colon"
            );
            assert_eq!(
                parsed.tokens.get(colon - 1).map(|t| t.kind),
                Some(dartq_scanner::TokenKind::Identifier),
                "the name token sits right before the colon"
            );
            assert_eq!(
                range.start as usize,
                colon + 1,
                "the marker anchors at the token right after the colon"
            );
        }
        assert!(parsed.ast.root().is_some());
    }

    #[test]
    fn named_record_field_cascade_value_seals_past_the_colon() {
        // The cascade value `a..b` opens `Cascade` at `..` (bridge
        // `begin_cascade`), strictly after the colon — the pre-fix anchor
        // sealed `end < first` here. A real-parse pin: the parse completing
        // at all is the assertion in debug builds.
        let parsed = parse_unit("var r = (x: a..b);\n", Features::default());
        let mut found = false;
        let mut seen = rustc_hash::FxHashSet::default();
        let mut stack = vec![parsed.ast.root().expect("root")];
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            if parsed.ast.kind(id) == NodeKind::NamedRecordField {
                found = true;
                let range = parsed.ast.token_range(id);
                assert_eq!(range.start, range.end, "zero-width at the value head");
            }
            stack.extend(parsed.ast.children(id).rev());
        }
        assert!(found, "the field marker emitted");
    }
}
