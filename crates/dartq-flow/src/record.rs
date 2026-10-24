//! The recording half of the engine: [`Flow`] implements
//! [`ParserListener`], buffers the flow-relevant events of each executable
//! (method, top-level function, constructor, factory, local function,
//! closure), and runs the replay pipeline when the executable ends.

use dartq_parser::{
    AssertKind, BlockKind, DeclarationKind, ExperimentalFlag, FormalParameterKind,
    IdentifierContext, MemberKind, Message, ParserListener,
};
// The listener trait spells its token spans `Token` (the parser's own
// alias of `TokenSpan`).
use dartq_parser::TokenSpan as Token;
use dartq_parser::TokenSpan;
use dartq_scanner::TokenTable;
use dartq_source::saturate_slot_u32;

use rustc_hash::FxHashMap;

use crate::ev::{BinOp, Ev};
use crate::facts::FlowFact;
use crate::interp::Interp;
use crate::ty::NameTypeOps;

/// The flow-analysis engine over one compilation unit. Construct one per
/// parse; [`Flow::take_facts`] collects every fact after the parse.
///
/// `'t` borrows the token table and source so operator tokens classify and
/// identifier names intern at record time (parser-inserted synthetic
/// tokens classify conservatively and get unique synthetic names).
pub struct Flow<'t> {
    tokens: &'t TokenTable,
    source: &'t str,
    /// Lexeme to interned name id (dense `u32` for the interpreter).
    names: FxHashMap<Box<str>, u32>,
    /// Interned name ids to their lexemes — the mirror of `names` in id
    /// order (dense, one push per intern), so a later stage can recover
    /// `Base::Name(id)` spellings through [`Flow::name_lexeme`].
    names_by_id: Vec<Box<str>>,
    /// The right operand's true end offset of the binary closing next:
    /// `handle_binary_rhs_end` sets it, `end_binary_expression` consumes
    /// it — the close's own `end` param is the send-tiling follower for a
    /// bare-send operand, never `rightOperand.end`.
    binary_rhs_end: Option<u32>,
    /// Open type-argument groups (nested annotations stack). Each group
    /// collects the argument rows recorded inside it; the parser fires
    /// `begin_type_arguments` at `<` and `end_type_arguments` immediately
    /// before the head's own row (`type_info_impl.dart`), so the group's
    /// entries are complete when it closes.
    type_arg_groups: Vec<Vec<crate::ty::ArgTy>>,
    /// Interned written-argument lists; index 0 is the reserved
    /// argument-less list ([`crate::ty::EMPTY_ARGS`]). Deduplicated
    /// structurally, so equal spellings share one entry and the promotion
    /// gate's id comparison is tree equality (#611 step 4).
    pub(crate) type_args: Vec<Vec<crate::ty::ArgTy>>,
    /// The interned non-promotion entry rows of the unit; index 0 is the
    /// reserved empty row (upstream's empty `whyNotPromoted` map — a
    /// consumer renders nothing). Each row is the ordered
    /// `(type, args, cause)` list behind one fact row's `entries` id —
    /// the `computeWhyNotPromotedMessages` producer's data. Per
    /// executable the interpreter folds its own pool into here at
    /// `exec_end`, rebasing the facts' ids.
    non_promotions: Vec<Vec<crate::facts::NonPromotionEntry>>,
    /// The interned argument list of the type row that fires next:
    /// `end_type_arguments` sets it, `handle_type` consumes it,
    /// `handle_no_type_arguments` clears it — all three fire immediately
    /// before the row (official `parseArguments`,
    /// `type_info_impl.dart:1158/1191`).
    pending_type_args: u32,
    /// Inferred locals' declared types keyed on the declarator name
    /// token's offset (#2140): the semantic layer derives a `var`/`final`
    /// declarator's static type from its initializer (the property-read
    /// and callee faces live there), and stages it here before this
    /// executable's replay. The fold adopts the staged type at
    /// `InitializerEnd` when the written header left the slot unknown, so
    /// the promotion machinery answers on inferred locals exactly like
    /// annotated ones. Entries whose executable never replays are inert
    /// (bounded by the unit's declarator count).
    inferred_locals: FxHashMap<u32, (crate::ty::Ty, u32)>,
    /// Assignment right-hand-side types beside their written-argument
    /// arena index, keyed on the write target token's offset: the
    /// semantic layer resolves an assignment's RHS shape its own rows
    /// carry (an SDK static's declared return — `RangeError
    /// .checkValidRange` — is invisible to the name-level flow walk, whose
    /// RHS reads `Unknown`; an argument-bearing literal — `<Issue>[]` —
    /// spells a tree the bare `Ty` cannot carry, #2364), and stages it
    /// here before the assignment's fold runs. The fold adopts it when
    /// its own answer is the unknown face; a resolved RHS keeps its own
    /// type. Entries whose assignment never folds are inert (bounded by
    /// the unit's write count).
    staged_rhs: FxHashMap<u32, (crate::ty::Ty, u32)>,
    /// `this`-read types keyed on the `this` token's offset (#2292): the
    /// semantic layer's resolved extended type for a `this` read inside an
    /// extension/extension-type member, staged before the executable's
    /// replay. Entries whose `this` never replays are inert (bounded by
    /// the unit's `this`-read count).
    staged_this: FxHashMap<u32, crate::ty::Ty>,
    /// The most recent label-reference identifier (a break/continue
    /// target's name, consumed by the next break/continue event).
    pending_label_ref: Option<u32>,
    /// Label-declaration names since the last consuming event (a labeled
    /// statement begin, or a switch case begin — case labels are counted
    /// on `CaseBegin` and never attach to a labeled statement).
    pending_labels: Vec<u32>,
    /// The FIRST label-declaration token since the last consuming event.
    /// `beginLabeledStatement` carries the statement past the labels
    /// (official fasta shape), but the official dead row anchors at the
    /// labeled statement's `firstToken` — the first label
    /// (`dead_code_verifier.dart:253`) — so the begin latches this.
    pending_label_token: Option<Token>,
    /// Recording buffer of the outermost active executable, if any.
    buffer: Option<Vec<Ev>>,
    /// Open `Ev::Block` event indices, for end-of-block span widening.
    block_events: Vec<usize>,
    /// The do-while body's last token: distinguishes the block-body
    /// `while`-part start (the closing brace) from the keyword start.
    do_body_end: Option<Token>,
    /// The open do-while body is a block (its `{` opened the body).
    do_body_is_block: bool,
    /// Executable nesting depth inside the active recording.
    exec_depth: usize,
    /// The then-arm boundary latched by
    /// [`Flow::end_conditional_expression_then_arm`], consumed by the
    /// argless colon call that follows it.
    cond_then: Option<(TokenSpan, u32)>,
    /// Declared-bound nullability faces by interned type-parameter name
    /// (see [`crate::ty::Boundary`]). Commits accumulate within one
    /// top-level declaration (class members see the class's parameters;
    /// later commits shadow earlier ones — the name-level model is
    /// spelling-keyed) and the table clears at the declaration's end, so
    /// an executable's replay always sees exactly its enclosing
    /// declaration's parameters. Absent rows keep the pre-#1077 behavior.
    boundaries: Vec<Option<crate::ty::Boundary>>,
    /// Open type-variable completions (`begin_type_variable` brackets):
    /// gates the bound-shape capture and pairs begun names with their
    /// ends, which complete in reverse declaration order (the parser
    /// rewinds per bound, `type_args.rs` completion loop) — a pop pairs
    /// correctly in both the single-variable and the group protocol.
    tv_open: u32,
    /// Begun type-variable names, innermost last (`end_type_variable`'s
    /// token argument is the name only in the group protocol — the
    /// single-variable path passes the closer — so the begin latches).
    tv_names: Vec<u32>,
    /// The completing variable's captured bound face. A bound's shape can
    /// arrive as several events (`void` return keyword, nested argument
    /// ends, the head row); the last writer of the completion step wins —
    /// the head row and `end_function_type` always fire last for their
    /// own type.
    tv_bound: Option<crate::ty::Boundary>,
    /// Saved `tv_bound` of the enclosing type-variable completion: a
    /// generic-function-type bound parses its own type-variable group
    /// inside the outer completion, and the inner group's commits must
    /// not consume the outer bound's capture.
    tv_saved: Vec<Option<crate::ty::Boundary>>,
    /// Inside a record-type spelling: its field rows are head rows (no
    /// argument group) and must not capture the enclosing bound's face —
    /// a record bound has no head row of its own, so it degrades.
    tv_in_record: bool,
    /// The unit's captured top-level variable declarations (name id → the
    /// written type face): the fold's top-level read fallback, the lexical
    /// chain's outermost scope (official unqualified-name lookup: the
    /// scope chain first, the implicit-`this` face only past it — upstream
    /// Dart SDK source,
    /// pkg/analyzer/lib/src/dart/resolver/property_element_resolver.dart:2137-2139).
    /// Top-level declarations parse with no recording buffer open, so
    /// their events reach no replay; the recorder captures them as a
    /// parse side-effect. Class-like members never enter (the capture is
    /// gated on [`Flow::container_depth`]) — instance and static fields
    /// keep the implicit-`this` read face. A declaration parsed after an
    /// executable (a forward reference) is captured too late for that
    /// executable's replay and under-reports there.
    top_level: FxHashMap<u32, TopVar>,
    /// The header type row of the declaration now parsing outside any
    /// executable: `Type`/`NoType` fire immediately before
    /// `begin_variables_declaration`, and a function-type/void header
    /// clears the latch (its own return/parameter rows must not leak), so
    /// an untyped or function-typed header answers unknown.
    top_last_type: Option<(crate::ty::Ty, u32)>,
    /// The head token of the most recent [`ParserListener::handle_type`]
    /// row (the annotation's first token — a qualified chain keys its
    /// first element). [`ParserListener::handle_no_type`] clears it. Read
    /// at `begin_variables_declaration`: the declaration statement's node
    /// begins at its earliest header token (`late`? modifier or annotation
    /// head), and the official's first-dead-node anchor for a dead
    /// declaration statement spans from there, not from the first
    /// declared name (dead_code_verifier's node offset — the `Token` of
    /// `Token token = ...;`, oracle-probed).
    type_head: Option<Token>,
    /// Inside a local-function statement (between its `begin` and `end`
    /// hooks): gates [`Ev::LocalFnName`] to the statement's own name —
    /// the name event fires before the body, so nested declarations
    /// latch correctly even though the flag is a plain bool.
    in_local_fn: bool,
    /// The captured header face of the top-level variables declaration
    /// now open (`begin_variables_declaration` .. `end_variables_declaration`);
    /// every declarator of the list reads it.
    top_decl: Option<TopVar>,
    /// Open class-like containers (class/mixin/enum/extension/extension
    /// type): a variables declaration under one is a member, not a
    /// top-level declaration.
    container_depth: usize,
    /// The closed-declaration capture the replay's switch-end
    /// exhaustiveness verdict reads (see [`crate::ty::ClosedNames`]):
    /// `sealed` class and enum names interned as their declarations parse.
    /// Like [`Flow::top_level`], a declaration parsed after the executable
    /// that switches over it is captured too late — the verdict
    /// under-reports there.
    closed: crate::ty::ClosedNames,
    /// The written `on`-type head of the extension header now parsing:
    /// `(interned head id, written-? flag)` — the last depth-0
    /// [`ParserListener::handle_type`] before the body opens (the header's
    /// primary-constructor parameter types arrive first, the on-type's own
    /// type arguments nest at depth 1, so the last depth-0 head is the
    /// `on` clause's). `None` outside extension headers.
    extension_on_head: Option<(u32, bool)>,
    /// Whether the type annotations still arriving belong to an extension
    /// header (`begin_extension_declaration` opens,
    /// [`ParserListener::begin_class_or_mixin_or_extension_body`] /
    /// `handle_no_extension_body` / `end_extension_declaration` close).
    in_extension_header: bool,
    /// The resolved type-operations back-end ([`crate::ty::TypeOps`]) the
    /// consumer mounts through [`Flow::set_ops`] before the parse — the
    /// official `FlowAnalysisTypeOperations` slot. `None` keeps the
    /// name-level default: `exec_end` falls back to a stack-local
    /// [`NameTypeOps`], so tests and unmounted callers replay unchanged.
    ops: Option<&'t dyn crate::ty::TypeOps>,
    /// The recording [`Flow::begin_variable_initializer`] opened for a
    /// declaration outside any executable (a field or top-level
    /// initializer is its own executable — the official folds it):
    /// [`Flow::end_variable_initializer`] closes it. The count, not a
    /// flag, because initializers nest inside the world — a local
    /// declared in a closure within the initializer opens its own
    /// bracket, whose end must close only that bracket (#2241's
    /// initializer-closure promotions died on the early close: the
    /// local's end fired the world's `exec_end` mid-closure-body and the
    /// replay popped the closure's frame and scope at the local's `=`).
    initializer_world: u32,
    /// Buffer indices of open constructor initializer entries
    /// (`begin_initializer` pushes, `end_initializer` pops): the field
    /// write `f = e` (or `this.f = e`) parses as a plain expression whose
    /// `Assign` event ends exactly at `end_initializer`'s end token —
    /// marked `field_init` there so the fold never writes a like-named
    /// local (upstream `ConstructorFieldInitializer` writes `this.f`).
    init_starts: Vec<usize>,
    facts: Vec<FlowFact>,
}

/// One captured top-level variable declaration's read face: the written
/// type (name-level). Untyped headers answer unknown — the initializer's
/// inferred type is the type layer's increment.
#[derive(Clone, Copy)]
pub(crate) struct TopVar {
    pub(crate) ty: crate::ty::Ty,
    pub(crate) args: u32,
}

impl<'t> Flow<'t> {
    #[must_use]
    pub fn new(tokens: &'t TokenTable, source: &'t str) -> Self {
        Self {
            tokens,
            source,
            names: FxHashMap::default(),
            names_by_id: Vec::new(),
            binary_rhs_end: None,
            type_arg_groups: Vec::new(),
            type_args: vec![Vec::new()],
            non_promotions: vec![Vec::new()],
            pending_type_args: crate::ty::EMPTY_ARGS,
            inferred_locals: FxHashMap::default(),
            staged_rhs: FxHashMap::default(),
            staged_this: FxHashMap::default(),
            extension_on_head: None,
            in_extension_header: false,
            pending_label_ref: None,
            pending_labels: Vec::new(),
            pending_label_token: None,
            buffer: None,
            block_events: Vec::new(),
            do_body_end: None,
            do_body_is_block: false,
            exec_depth: 0,
            cond_then: None,
            boundaries: Vec::new(),
            tv_open: 0,
            tv_names: Vec::new(),
            tv_bound: None,
            tv_saved: Vec::new(),
            tv_in_record: false,
            top_level: FxHashMap::default(),
            top_last_type: None,
            type_head: None,
            in_local_fn: false,
            top_decl: None,
            container_depth: 0,
            closed: crate::ty::ClosedNames::default(),
            ops: None,
            initializer_world: 0,
            init_starts: Vec::new(),
            facts: Vec::new(),
        }
    }

    /// The span of the token at table `index` (the recorder's token-table
    /// navigation for arm boundaries the listener surface carries only by
    /// adjacency).
    #[expect(
        clippy::cast_possible_truncation,
        reason = "SourceRange is u32 by the scanner's own offset/length \
                  convention; source files fit that width"
    )]
    fn span_at(&self, index: usize) -> Option<TokenSpan> {
        self.tokens.get(index).map(|token| {
            let range = token.byte_range();
            TokenSpan {
                id: index as u32,
                range: dartq_source::SourceRange {
                    offset: range.start as u32,
                    length: (range.end - range.start) as u32,
                },
            }
        })
    }

    /// Interns a written-argument list structurally: equal trees share one
    /// arena entry, so the interpreter's id equality is tree equality.
    /// Lists are tiny (the unit's distinct generic spellings) and
    /// argument-less types never reach here, so the linear scan is bounded
    /// by unit size, not corpus size.
    fn intern_args(&mut self, entries: Vec<crate::ty::ArgTy>) -> u32 {
        if entries.is_empty() {
            return crate::ty::EMPTY_ARGS;
        }
        if let Some(index) = self
            .type_args
            .iter()
            .position(|existing| *existing == entries)
        {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "arena index; a unit with 2^32 distinct generic \
                          spellings cannot parse"
            )]
            return index as u32;
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "arena index; a unit with 2^32 distinct generic \
                      spellings cannot parse"
        )]
        let id = self.type_args.len() as u32;
        self.type_args.push(entries);
        id
    }

    /// Interns a staged written-argument tree (#2364) into the same arena
    /// as the written ones: a named spelling's lexeme joins the lexeme
    /// table (`Base::Name`), `dynamic`/invalid spell their special bases —
    /// equal trees share one entry.
    fn intern_staged_args(&mut self, args: &[crate::ty::StagedArg]) -> u32 {
        use crate::ty::StagedArg;
        if args.is_empty() {
            return crate::ty::EMPTY_ARGS;
        }
        let entries = args
            .iter()
            .map(|arg| match arg {
                StagedArg::Named {
                    lexeme,
                    nullable,
                    args,
                } => crate::ty::ArgTy {
                    base: crate::ty::Base::Name(self.intern_lexeme(lexeme)),
                    nullable: *nullable,
                    args: self.intern_staged_args(args),
                },
                StagedArg::Dynamic => crate::ty::ArgTy {
                    base: crate::ty::Base::Dynamic,
                    nullable: false,
                    args: crate::ty::EMPTY_ARGS,
                },
                StagedArg::Invalid => crate::ty::ArgTy {
                    base: crate::ty::Base::Unknown,
                    nullable: false,
                    args: crate::ty::EMPTY_ARGS,
                },
            })
            .collect();
        self.intern_args(entries)
    }

    /// One token's lexeme, when the token table carries it.
    fn token_text(&self, token: TokenSpan) -> Option<&str> {
        self.tokens
            .get(token.id as usize)
            .map(|_| self.tokens.lexeme(self.source, token.id as usize))
    }

    /// Interns a name token's lexeme to a dense id. Synthetic tokens
    /// (parser-inserted) get a unique id keyed on the token handle.
    fn name(&mut self, token: TokenSpan) -> u32 {
        let index = token.id as usize;
        // The lookup borrows the lexeme: identifiers are the hottest flow
        // event and a repeat occurrence (the common case) must not
        // allocate; the owning key is built only on first sight.
        if self.tokens.get(index).is_some() {
            let lexeme = self.tokens.lexeme(self.source, index);
            self.intern_lexeme(lexeme)
        } else {
            // Parser-inserted synthetic tokens key on the token handle.
            let key: Box<str> = format!("#{}", token.id).into();
            if let Some(&id) = self.names.get(&key) {
                return id;
            }
            let id = saturate_slot_u32(self.names.len());
            self.names_by_id.push(key.clone());
            self.names.insert(key, id);
            id
        }
    }

    /// Interns a lexeme directly — the core-name face for spellings the
    /// file may never write (the implicit `Object?` bound's promotion
    /// target). Written occurrences share the id, so `Base::Name` equality
    /// stays lexeme equality.
    fn intern_lexeme(&mut self, lexeme: &str) -> u32 {
        if let Some(&id) = self.names.get(lexeme) {
            return id;
        }
        let id = saturate_slot_u32(self.names.len());
        let key: Box<str> = lexeme.into();
        self.names_by_id.push(key.clone());
        self.names.insert(key, id);
        id
    }

    /// Whether a bound-shape event may capture: inside a type-variable
    /// completion and not within a record-type spelling.
    fn tv_capturing(&self) -> bool {
        self.tv_open > 0 && !self.tv_in_record
    }

    /// Captures the completing variable's bound face from a head type row
    /// (`handle_type` with no open argument group) — the last shape event
    /// of a non-function bound, so it wins over any nested argument's
    /// earlier capture. Undecidable shapes capture nothing: the parameter
    /// keeps the pre-#1077 behavior.
    fn capture_bound_type(&mut self, base: crate::ty::Base, nullable: bool, args: u32) {
        let Some((class, target)) = self.bound_face(base, nullable, args) else {
            return;
        };
        self.tv_bound = Some(crate::ty::Boundary {
            class,
            target,
            bound: (crate::ty::Ty { base, nullable }, args),
        });
    }

    /// The nullability face of one type spelling beside its promotion
    /// target — the name-level reduction of the official subtype question
    /// (`T <: Object` per upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/element/subtype.dart:303-325, `FutureOr<S>
    /// <: Object` iff `S <: Object`, top types excluded by :290-296).
    /// `None` when the spelling carries no derivable face.
    fn bound_face(
        &self,
        base: crate::ty::Base,
        nullable: bool,
        args: u32,
    ) -> Option<(crate::ty::Classification, Option<(crate::ty::Ty, u32)>)> {
        use crate::ty::Base as B;
        use crate::ty::Classification as C;
        if nullable {
            // `B? <: Object` fails; the non-null face is the bound
            // without its `?` (type_system.dart:1497).
            return Some(match base {
                B::Dynamic | B::Void | B::Unknown | B::Null => (C::PotentiallyNullable, None),
                B::Never => (C::NullOrEquivalent, None),
                // A signature-less function head subtypes `Object?` the
                // same way; its non-null face is the suffix-less head.
                other @ (B::Name(_) | B::Function) => (
                    C::PotentiallyNullable,
                    Some((
                        crate::ty::Ty {
                            base: other,
                            nullable: false,
                        },
                        args,
                    )),
                ),
            });
        }
        match base {
            // Top types include `null` among their values; an unknown
            // shape classifies potentially-nullable like the official
            // invalid-type arm.
            B::Dynamic | B::Void | B::Unknown => Some((C::PotentiallyNullable, None)),
            // `Null <: Null`.
            B::Null => Some((C::NullOrEquivalent, None)),
            B::Name(id) if self.name_lexeme(id) == "FutureOr" => {
                // The face follows the single argument's; a nested
                // `FutureOr` recurses. `FutureOr<Null>` reduces to
                // potentially-nullable here (its null-or-equivalent face
                // is left on the table — conservative in the live
                // direction). The promotion target is none: the bound is
                // `?`-less, so `NonNull` collapses to itself.
                let row = self.type_args.get(args as usize)?;
                if row.len() != 1 {
                    return None; // malformed `FutureOr` spelling: degrade
                }
                let argument = row[0];
                let class =
                    match self.bound_face(argument.base, argument.nullable, argument.args)? {
                        (C::NonNullable, _) => C::NonNullable,
                        _ => C::PotentiallyNullable,
                    };
                Some((class, None))
            }
            // `Never <: Object`; any other non-nullable named or
            // signature-less function bound is a subtype of `Object` too.
            B::Never | B::Name(_) | B::Function => Some((C::NonNullable, None)),
        }
    }

    /// The type-name token's own identifier id: `handle_type` fires on the
    /// qualified chain's FIRST token (`io` of `io.Directory`, per the
    /// official `handleType(typeRefOrPrefix)`), so the type name is the
    /// chain's LAST identifier — the element the spelling resolves to.
    /// Walking `Period`-separated continuations recovers it; an
    /// unqualified name stays its own id, and a recovery tail (a `.`
    /// followed by a synthetic or non-identifier token) keeps the last
    /// real identifier seen.
    fn qualified_name(&mut self, head: Token) -> u32 {
        let mut index = head.id as usize;
        loop {
            let Some(dot) = self.tokens.next(index) else {
                break;
            };
            if self.tokens.get(dot).map(|t| t.kind) != Some(dartq_scanner::TokenKind::Period) {
                break;
            }
            let Some(tail) = self.tokens.next(dot) else {
                break;
            };
            if self.tokens.get(tail).map(|t| t.kind)
                != Some(dartq_scanner::TokenKind::Identifier)
                || self.tokens.get(tail).is_none_or(|t| t.is_synthetic())
            {
                break;
            }
            index = tail;
        }
        if index == head.id as usize {
            self.name(head)
        } else {
            self.intern_lexeme(self.tokens.lexeme(self.source, index))
        }
    }

    /// The lexeme of an interned name id — the recovery face turning
    /// `Base::Name(id)` (and event name ids) back into their source
    /// spelling. Ids are dense by construction; synthetic tokens read as
    /// their `#<handle>` placeholder.
    #[must_use]
    pub fn name_lexeme(&self, id: u32) -> &str {
        let index = id as usize;
        &self.names_by_id[index]
    }

    /// The written-argument tree interned at arena `id` — the cross-crate
    /// read face of the fact rows' `args` (#1010). [`crate::ty::EMPTY_ARGS`]
    /// (the reserved index 0) reads as the empty slice; a fact row only
    /// ever carries ids this recorder interned, so an out-of-range id is a
    /// program bug, not a recoverable condition.
    #[must_use]
    pub fn type_args_at(&self, id: u32) -> &[crate::ty::ArgTy] {
        &self.type_args[id as usize]
    }

    /// The non-promotion entries interned at arena `id` — the cross-crate
    /// read face of the why-not-promoted fact rows' `entries` (#1830).
    /// Index 0 (the reserved empty row) reads as the empty slice — the
    /// official empty-map path (`flow_analysis.dart:8858`), where a
    /// consumer renders nothing. A fact row only ever carries ids this
    /// recorder interned, so an out-of-range id is a program bug.
    #[must_use]
    pub fn non_promotion_at(&self, id: u32) -> &[crate::facts::NonPromotionEntry] {
        &self.non_promotions[id as usize]
    }

    /// The structured non-promotion reasons behind one fact row — the
    /// `computeWhyNotPromotedMessages` producer's entry point
    /// (`type_analyzer.dart:772`): match the row's own identity fields
    /// (read span, member name, receiver), then read its ordered
    /// `(type, args, cause)` entries here — no flow-graph re-walk.
    /// `None` for fact rows that carry no non-promotion face.
    #[must_use]
    pub fn non_promotion_context(
        &self,
        fact: &FlowFact,
    ) -> Option<&[crate::facts::NonPromotionEntry]> {
        match fact {
            FlowFact::WhyNotPromoted { entries, .. }
            | FlowFact::NonPromotedPropertyRead { entries, .. }
            | FlowFact::ThisNotPromoted { entries, .. } => Some(self.non_promotion_at(*entries)),
            _ => None,
        }
    }

    /// The structured non-promotion reasons behind one fact row, resolved
    /// to the five-variant shape ([`crate::facts::NonPromotionReason`])
    /// the context-message producer renders from
    /// (`computeWhyNotPromotedMessages`, `type_analyzer.dart:772`). The
    /// recorded write/suspension links pass through with their anchors;
    /// the property family resolves through `property` — the diagnoser's
    /// element-layer verdict for the row's member, the mirror of
    /// upstream's `whyPropertyIsNotPromotable` operation (upstream Dart
    /// SDK source,
    /// pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart:8769-8772):
    /// `Some(why)` is `PropertyNotPromotedForInherentReason`, `None` is
    /// `PropertyNotPromotedForNonInherentReason` (a library-level
    /// conflicting declaration, field promotion disabled, or both). A
    /// public member name answers
    /// [`crate::facts::PropertyNonPromotability::NotPrivate`] statically.
    /// Consulted only for rows carrying property evidence
    /// ([`FlowFact::NonPromotedPropertyRead`]); variable and `this` rows
    /// never read it.
    ///
    /// Rows without a non-promotion face answer empty, as does the
    /// reserved empty entries row — the official empty-map path
    /// (`flow_analysis.dart:8858`).
    #[must_use]
    pub fn non_promotion_reasons(
        &self,
        fact: &FlowFact,
        property: Option<crate::facts::PropertyNonPromotability>,
    ) -> Vec<crate::facts::NonPromotionReason> {
        let Some(entries) = self.non_promotion_context(fact) else {
            return Vec::new();
        };
        entries
            .iter()
            .map(|entry| crate::facts::NonPromotionReason::resolve(entry.cause, property))
            .collect()
    }

    /// Takes the pending label reference if one is set (break/continue
    /// with a target).
    fn take_label(&mut self, has_target: bool) -> Option<u32> {
        if has_target {
            self.pending_label_ref.take()
        } else {
            None
        }
    }

    /// Every fact of the unit, ordered by source offset.
    pub fn take_facts(&mut self) -> Vec<FlowFact> {
        let mut facts = std::mem::take(&mut self.facts);
        facts.sort_by_key(|fact| match fact {
            FlowFact::DeadCode { span }
            | FlowFact::UnassignedRead { span, .. }
            | FlowFact::PromotedRead { span, .. }
            | FlowFact::ReadType { span, .. }
            | FlowFact::ConditionType { span, .. }
            | FlowFact::MatchedType { span, .. }
            | FlowFact::AsyncContextUse { span, .. }
            | FlowFact::PromotedPropertyRead { span }
            | FlowFact::WhyNotPromoted { span, .. }
            | FlowFact::NonPromotedPropertyRead { span, .. }
            | FlowFact::ThisNotPromoted { span, .. }
            | FlowFact::BodyMayCompleteNormally { span } => span.range.offset,
            FlowFact::WriteSite { decl_name_span, .. } => decl_name_span.offset,
        });
        facts
    }

    /// Interns a name lexeme from outside the event stream — the
    /// staged-declaration face (#2140): an inferred local's flow-declared
    /// type spells a name the file may never write. Written occurrences
    /// share the id, so `Base::Name` equality stays lexeme equality.
    pub fn intern_name(&mut self, lexeme: &str) -> u32 {
        self.intern_lexeme(lexeme)
    }

    /// Mounts the resolved type-operations back-end (the official
    /// `FlowAnalysisTypeOperations` client seam) — every executable replay
    /// then consults it instead of the name-level default, through the
    /// `Ty`/`ArgTy` vocabulary the flow layer owns. Set once after
    /// construction, before the parse: a mid-parse swap would split one
    /// unit's verdicts across two worlds.
    pub fn set_ops(&mut self, ops: &'t dyn crate::ty::TypeOps) {
        self.ops = Some(ops);
    }

    /// Stages one inferred local's declared type (#2140): the semantic
    /// layer's initializer-derived static type for a `var`/`final`
    /// declarator, keyed on the declarator name token's offset. The fold
    /// adopts it at `InitializerEnd` when the written header left the
    /// slot unknown ([`crate::interp::VarMeta`] keeps the same anchor);
    /// the staged written-argument tree rides beside the head (#2506 —
    /// the `ArgListId::EMPTY` gate dropped every argument-bearing
    /// inference, and the Unknown read row downstream mis-fires the
    /// InvalidType-scrutinee posture). Shapes the flow lattice cannot
    /// spell never arrive (the stager drops them, the unknown-declared
    /// under-report stands).
    pub fn stage_inferred_local(
        &mut self,
        decl_offset: u32,
        ty: crate::ty::Ty,
        args: Vec<crate::ty::StagedArg>,
    ) {
        let args = self.intern_staged_args(&args);
        self.inferred_locals.insert(decl_offset, (ty, args));
    }

    /// Stages one assignment's right-hand-side type beside its written
    /// argument tree (#2174 plain `=`, #2360 `??=`, #2364 the tree): the
    /// semantic layer's resolved RHS static type for a spellable write,
    /// keyed on the write target token's offset. The fold adopts it at
    /// `Ev::Assign` when its own RHS read is the unknown face; shapes the
    /// flow lattice cannot spell never arrive (the stager drops them, the
    /// unknown-written demotion stands).
    pub fn stage_assignment_rhs(
        &mut self,
        target_offset: u32,
        ty: crate::ty::Ty,
        args: Vec<crate::ty::StagedArg>,
    ) {
        let args = self.intern_staged_args(&args);
        self.staged_rhs.insert(target_offset, (ty, args));
    }

    /// Stages one `this` read's static type (#2292): the semantic layer's
    /// resolved type for a `this` expression the event stream leaves
    /// unknown — an extension or extension-type instance member's extended
    /// type — keyed on the `this` token's offset. The fold adopts it at
    /// `Ev::This` when its own answer is the unknown face, so the
    /// `switch (this)` scrutinee (exhaustiveness) and `this`-chained
    /// promotion reads answer on the real type. Shapes the flow lattice
    /// cannot spell never arrive (the stager drops them, the unknown-read
    /// face stands).
    pub fn stage_this_type(&mut self, token_offset: u32, ty: crate::ty::Ty) {
        self.staged_this.insert(token_offset, ty);
    }

    /// Records the `handleExit` contract: a subexpression's static type is
    /// Never, so the flow state goes unreachable from the expression's end
    /// (official `FlowAnalysis.handleExit`, flow_analysis.dart:919-925).
    /// Fired by the engine once it resolves a subexpression's type to
    /// Never; the parser listener alone cannot know it.
    pub fn handle_exit(&mut self, end: TokenSpan) {
        self.record(Ev::Exit { end });
    }

    fn record(&mut self, ev: Ev) {
        if let Some(buffer) = self.buffer.as_mut() {
            buffer.push(ev);
        }
    }

    fn bin_op(&self, operator: Token) -> BinOp {
        match self.kind(operator) {
            Some(dartq_scanner::TokenKind::AmpersandAmpersand) => BinOp::And,
            Some(dartq_scanner::TokenKind::BarBar) => BinOp::Or,
            Some(dartq_scanner::TokenKind::QuestionQuestion) => BinOp::IfNull,
            Some(dartq_scanner::TokenKind::EqEq) => BinOp::Eq,
            Some(dartq_scanner::TokenKind::BangEq) => BinOp::NotEq,
            _ => BinOp::Other,
        }
    }

    /// `true` for `=`, `false` for every compound assignment operator
    /// (`+=`, `??=`, …). Synthetic operators read as compound.
    fn is_plain_assign(&self, operator: Token) -> bool {
        matches!(self.kind(operator), Some(dartq_scanner::TokenKind::Eq))
    }

    fn kind(&self, token: TokenSpan) -> Option<dartq_scanner::TokenKind> {
        let index = usize::try_from(token.id).ok()?;
        self.tokens.get(index).map(|token| token.kind)
    }

    /// Executable begin: starts a recording or nests inside the active one.
    fn exec_begin(&mut self, is_closure: bool) {
        match self.buffer.as_mut() {
            None => {
                self.buffer = Some(Vec::new());
                self.exec_depth = 1;
            }
            Some(buffer) => {
                buffer.push(Ev::ExecBegin { is_closure });
                self.exec_depth += 1;
            }
        }
    }

    /// Executable end: closes the outermost recording and runs the
    /// pipeline over it. `is_expr` marks function-expression ends, which
    /// net one value for the consumer (crate contract, see
    /// `SwitchExprBlockEnd`/`ListLitEnd`); `end` is the executable's end
    /// token — recorded on the nested `ExecEnd` event, and the
    /// completion fact's anchor for the outermost executable (which
    /// records no framing events of its own).
    fn exec_end(&mut self, is_expr: bool, end: Token) {
        self.exec_depth = self.exec_depth.saturating_sub(1);
        if self.exec_depth > 0 {
            self.record(Ev::ExecEnd { is_expr, end });
            return;
        }
        let Some(events) = self.buffer.take() else {
            return;
        };
        // The async-gap context face ids first (interning borrows the
        // recorder mutably; the interpreter below borrows its type tables).
        let (context_ty, state_ty, mounted, context_name) = (
            self.intern_lexeme("BuildContext"),
            self.intern_lexeme("State"),
            self.intern_lexeme("mounted"),
            self.intern_lexeme("context"),
        );
        // The exhaustiveness gate's core names (interned lazily, id-stable
        // across replays): `bool` splits over `true`/`false`,
        // `FutureOr<T>` recurses into `T`.
        self.closed.bool_name = Some(self.intern_lexeme("bool"));
        self.closed.future_or = Some(self.intern_lexeme("FutureOr"));
        let fallback = NameTypeOps;
        let ops = self.ops.unwrap_or(&fallback);
        let mut interp = Interp::new(ops, &self.type_args, &self.boundaries);
        // The unit's top-level read face (captured so far — declarations
        // parsed after this executable under-report there), and the closed
        // declarations the switch-end verdict reads.
        interp.top_level = &self.top_level;
        interp.closed = &self.closed;
        // The inferred locals' declared types the semantic layer staged
        // for this unit (#2140) — read-only here, adopted at
        // `InitializerEnd`.
        interp.inferred_locals = &self.inferred_locals;
        // The staged assignment RHS types (#2174) — read-only here,
        // adopted at `Ev::Assign` when the fold's own answer is unknown.
        interp.staged_rhs = &self.staged_rhs;
        // The staged `this` read types (#2292) — read-only here, adopted
        // at `Ev::This` when the fold's own answer is unknown.
        interp.staged_this = &self.staged_this;
        // The interned name lexemes: the send fold's class-head face reads
        // the callee's spelling (`x = C()` vs a lowercase function call).
        interp.names = &self.names_by_id;
        interp.run(&events);
        // The async-gap context face: one linear replay over the same
        // buffer (the ids are interned lazily — already-present names keep
        // their ids, so existing name-id assertions never shift).
        let gap = crate::async_gap::GapPass::new(context_ty, state_ty, mounted, context_name)
            .run(&events);
        interp.facts.extend(gap.facts);
        // The outermost body's fall-through completion: after `run` the
        // interpreter's current state IS the body's exit state (official
        // `checkForBodyMayCompleteNormally`'s reachability gate,
        // type_analyzer.dart:647-652; nested executables emit at their
        // own `ExecEnd` inside the replay).
        if interp.reachable() {
            interp.emit(FlowFact::BodyMayCompleteNormally { span: end });
        }
        // The non-promotion entry rows: append this executable's pool after
        // the unit's reserved empty row and rebase the facts' ids (the
        // interpreter's row 1 becomes the unit arena's next slot; its
        // reserved 0 stays 0).
        if interp.non_promotions.len() > 1 {
            let base = self.non_promotions.len();
            self.non_promotions.extend(interp.non_promotions.drain(1..));
            for fact in &mut interp.facts {
                match fact {
                    FlowFact::WhyNotPromoted { entries, .. }
                    | FlowFact::NonPromotedPropertyRead { entries, .. }
                    | FlowFact::ThisNotPromoted { entries, .. } => {
                        *entries = if *entries == 0 {
                            0
                        } else {
                            #[expect(
                                clippy::cast_possible_truncation,
                                reason = "arena index; a unit with 2^32 \
                                          demotion contexts cannot parse"
                            )]
                            let base = base as u32;
                            base + *entries - 1
                        };
                    }
                    _ => {}
                }
            }
        }
        self.facts.append(&mut interp.facts);
    }
}

impl ParserListener for Flow<'_> {
    // —— executable framing ————————————————————————————————————————————

    fn begin_top_level_method(
        &mut self,
        _last_consumed: Token,
        _augment: Option<Token>,
        _external: Option<Token>,
    ) {
        self.exec_begin(false);
    }

    fn end_top_level_method(&mut self, _begin: Token, _get_or_set: Option<Token>, end: Token) {
        self.exec_end(false, end);
    }

    fn begin_method(
        &mut self,
        _kind: DeclarationKind,
        _augment_kw: Option<Token>,
        _external_kw: Option<Token>,
        _static_kw: Option<Token>,
        _covariant_kw: Option<Token>,
        _var_final_or_const: Option<Token>,
        _get_or_set: Option<Token>,
        _name: Token,
        _enclosing: Option<&str>,
    ) {
        self.exec_begin(false);
    }

    fn end_method(
        &mut self,
        _kind: DeclarationKind,
        _get_or_set: Option<Token>,
        _begin: Token,
        _begin_param: Token,
        _begin_initializers: Option<Token>,
        end: Token,
    ) {
        self.exec_end(false, end);
    }

    fn begin_constructor(
        &mut self,
        _kind: DeclarationKind,
        _augment_kw: Option<Token>,
        _external_kw: Option<Token>,
        _static_kw: Option<Token>,
        _covariant_kw: Option<Token>,
        _var_final_or_const: Option<Token>,
        _get_or_set: Option<Token>,
        _new_kw: Option<Token>,
        _name: Token,
        _enclosing: Option<&str>,
    ) {
        self.exec_begin(false);
    }

    fn end_constructor(
        &mut self,
        _kind: DeclarationKind,
        _begin: Token,
        _new_kw: Option<Token>,
        _begin_param: Token,
        _begin_initializers: Option<Token>,
        end: Token,
    ) {
        self.exec_end(false, end);
    }

    fn begin_factory(
        &mut self,
        _kind: DeclarationKind,
        _last_consumed: Token,
        _augment_kw: Option<Token>,
        _external_kw: Option<Token>,
        _const_keyword: Option<Token>,
    ) {
        self.exec_begin(false);
    }

    fn end_factory(&mut self, _kind: DeclarationKind, _begin: Token, _factory: Token, end: Token) {
        self.exec_end(false, end);
    }

    fn begin_local_function_declaration(&mut self, _token: Token) {
        self.exec_begin(true);
        // The name latch (see [`Flow::in_local_fn`]): the name event fires
        // between this and the matching end, before the body's own nested
        // declarations.
        self.in_local_fn = true;
    }

    fn end_local_function_declaration(&mut self, end: Token) {
        self.in_local_fn = false;
        self.exec_end(false, end);
        self.record(Ev::StmtEnd {
            end: end.range.end(),
        });
    }

    fn end_function_name(&mut self, begin: Token, _end: Token, is_function_expression: bool) {
        // Only the local-function statement's name latches (a function
        // expression's name is not a callable binding).
        if self.in_local_fn && !is_function_expression {
            let name = self.name(begin);
            self.record(Ev::LocalFnName { name });
        }
    }

    fn begin_function_expression(&mut self, _token: Token) {
        self.exec_begin(true);
    }

    fn end_function_expression(&mut self, _begin: Token, end: Token) {
        self.exec_end(true, end);
    }

    fn begin_named_function_expression(&mut self, _token: Token) {
        self.exec_begin(true);
    }

    fn end_named_function_expression(&mut self, end: Token) {
        self.exec_end(true, end);
    }

    // —— class-like containers ————————————————————————————————————————

    // The container depth gates the top-level capture (see `top_level`):
    // a variables declaration under any class-like container is a member,
    // never a top-level variable. A named mixin application declares no
    // body and is not counted.
    fn begin_class_declaration(
        &mut self,
        _begin: Token,
        _abstract_kw: Option<Token>,
        _sealed_kw: Option<Token>,
        _base_kw: Option<Token>,
        _interface_kw: Option<Token>,
        _final_kw: Option<Token>,
        _augment_kw: Option<Token>,
        _mixin_kw: Option<Token>,
        _name: Token,
    ) {
        // The `isSealed` element face (type_system.dart:858): the switch
        // scrutinee's always-exhaustive verdict reads it.
        if _sealed_kw.is_some() {
            let name = self.name(_name);
            self.closed.names.insert(name);
        }
        self.container_depth += 1;
    }

    fn end_class_declaration(&mut self, _begin: Token, _end: Token) {
        self.container_depth = self.container_depth.saturating_sub(1);
    }

    fn begin_mixin_declaration(
        &mut self,
        _begin: Token,
        _augment_kw: Option<Token>,
        _base_kw: Option<Token>,
        _mixin: Token,
        _name: Token,
    ) {
        self.container_depth += 1;
    }

    fn end_mixin_declaration(&mut self, _begin: Token, _end: Token) {
        self.container_depth = self.container_depth.saturating_sub(1);
    }

    fn begin_enum_declaration(
        &mut self,
        _begin: Token,
        _augment_kw: Option<Token>,
        _enum_keyword: Token,
        _name: Token,
    ) {
        // Enums are sealed by construction (upstream
        // `element is EnumElement` arm, type_system.dart:855-856).
        let name = self.name(_name);
        self.closed.names.insert(name);
        self.container_depth += 1;
    }

    fn end_enum_declaration(
        &mut self,
        _begin: Token,
        _enum_keyword: Token,
        _left_brace: Token,
        _member_count: usize,
        _end: Token,
    ) {
        self.container_depth = self.container_depth.saturating_sub(1);
    }

    fn begin_extension_declaration(
        &mut self,
        _augment: Option<Token>,
        _extension_keyword: Token,
        _name: Option<Token>,
    ) {
        // The header's type annotations now arriving race for the on-type
        // latch (see [`Flow::extension_on_head`]): the primary
        // constructor's parameter types arrive first, the on-type's head
        // is the last depth-0 head before the body opens.
        self.in_extension_header = true;
        self.extension_on_head = None;
        self.container_depth += 1;
    }

    fn end_extension_declaration(
        &mut self,
        _begin: Token,
        _extension_keyword: Token,
        _on: Option<Token>,
        _end: Token,
    ) {
        self.container_depth = self.container_depth.saturating_sub(1);
        self.in_extension_header = false;
        self.extension_on_head = None;
    }

    fn begin_class_or_mixin_or_extension_body(&mut self, kind: DeclarationKind, _token: Token) {
        if kind == dartq_parser::DeclarationKind::Extension {
            self.in_extension_header = false;
        }
    }

    fn handle_no_extension_body(&mut self, _semicolon: Token) {
        self.in_extension_header = false;
    }

    fn begin_extension_type_declaration(
        &mut self,
        _augment_kw: Option<Token>,
        _extension_keyword: Token,
        _name: Token,
    ) {
        self.container_depth += 1;
    }

    fn end_extension_type_declaration(
        &mut self,
        _begin: Token,
        _augment_kw: Option<Token>,
        _extension_keyword: Token,
        _type_keyword: Token,
        _end: Token,
    ) {
        self.container_depth = self.container_depth.saturating_sub(1);
    }

    // —— bodies ————————————————————————————————————————————————————————

    fn begin_block_function_body(&mut self, _token: Token) {
        self.record(Ev::BlockBodyBegin);
    }

    fn end_block_function_body(&mut self, _count: usize, _begin: Token, _end: Token) {
        self.record(Ev::BlockBodyEnd);
    }

    fn handle_expression_function_body(&mut self, _arrow: Token, _end: Option<Token>) {
        self.record(Ev::ExprBody);
    }

    fn handle_empty_function_body(&mut self, _semicolon: Token) {
        self.record(Ev::EmptyBody);
    }

    fn handle_async_modifier(&mut self, async_kw: Option<Token>, star: Option<Token>) {
        self.record(Ev::AsyncModifier {
            is_async: async_kw.is_some(),
            is_generator: star.is_some(),
        });
    }

    // —— blocks and asserts ——————————————————————————————————————————————

    fn begin_block(&mut self, token: Token, kind: BlockKind) {
        // The finally marker rides the finally block's BEGIN (official
        // `tryFinallyStatement_finallyBegin`): the finally context must be
        // open before the body's events replay. The parser's own
        // `handle_finally_block` fires only after the block has parsed.
        if matches!(kind, BlockKind::FinallyClause) {
            self.record(Ev::FinallyBlock);
        }
        // Narrow to the recorded subset: only plain statement blocks carry
        // the dead-code check; catch blocks still scope their locals; try,
        // finally, and switch blocks (and function bodies, which frame
        // through the dedicated body events) push scope only.
        let kind = match kind {
            BlockKind::Statement => crate::ev::BlockKind::Statement,
            BlockKind::FunctionBody => crate::ev::BlockKind::FunctionBody,
            BlockKind::CatchClause => crate::ev::BlockKind::CatchClause,
            BlockKind::TryStatement | BlockKind::FinallyClause | BlockKind::SwitchStatement => {
                crate::ev::BlockKind::Other
            }
        };
        self.record(Ev::Block { kind, span: token });
        // The dead-code row for a dead block spans the full `{ ... }`
        // extent (official dead_code reports the whole statement); the
        // closing brace only arrives at `endBlock`, so remember the
        // event index and widen it there.
        if let Some(buffer) = &self.buffer {
            self.block_events.push(buffer.len() - 1);
        }
    }

    fn end_block(&mut self, _count: usize, begin: Token, end: Token, _kind: BlockKind) {
        if let (Some(buffer), Some(index)) = (&mut self.buffer, self.block_events.pop())
            && let Some(Ev::Block { span, .. }) = buffer.get_mut(index)
        {
            span.range = dartq_source::SourceRange {
                offset: begin.range.offset,
                length: end.range.offset + end.range.length - begin.range.offset,
            };
        }
        // Inside a do-while block body this is the body's own close (the
        // last-wins order makes inner blocks yield to it).
        if self.do_body_is_block {
            self.do_body_end = Some(end);
        }
        self.record(Ev::EndBlock {
            end: end.range.end(),
        });
    }

    fn begin_assert(&mut self, token: Token, _kind: AssertKind) {
        // The kind carries no flow weight: every assertion's condition is
        // evaluated and its exit joins the pre-assert state, whatever the
        // parse position.
        self.record(Ev::AssertBegin { span: token });
    }

    fn assert_after_condition(&mut self) {
        self.record(Ev::AssertAfterCondition);
    }

    fn end_assert(
        &mut self,
        _begin: Token,
        kind: AssertKind,
        _left_paren: Token,
        _comma: Option<Token>,
        end: Token,
    ) {
        self.record(Ev::AssertEnd);
        // The hook's `end` is the closing `)` (official `endAssert`
        // delivers the endGroup paren, parser_impl.dart:11137); the
        // statement's `;` rides no hook, so the completion event reads it
        // from the token table when the statement is well-formed (the
        // official dead-assert row spans through the `;` —
        // dead_code_test.dart `test_assert`, 14 carets over
        // `assert (true);`).
        let stmt_end = if matches!(kind, AssertKind::Statement)
            && let Some(next) = self.tokens.get(end.id as usize + 1)
            && next.kind == dartq_scanner::TokenKind::Semicolon
        {
            next.utf16_offset + 1
        } else {
            end.range.end()
        };
        self.record(Ev::StmtEnd { end: stmt_end });
    }

    // —— formal parameters ——————————————————————————————————————————————

    fn end_formal_parameter(
        &mut self,
        _var_or_final: Option<Token>,
        _covariant_kw: Option<Token>,
        this_keyword: Option<Token>,
        _super_keyword: Option<Token>,
        _period: Option<Token>,
        name_token: Token,
        _initializer_start: Option<Token>,
        _initializer_end: Option<Token>,
        _kind: FormalParameterKind,
        _member_kind: MemberKind,
    ) {
        let name = self.name(name_token);
        self.record(Ev::Formal {
            name,
            this_or_super: this_keyword.is_some(),
            token: name_token,
        });
    }

    fn end_function_typed_formal_parameter(&mut self, token: Token, _question: Option<Token>) {
        // The formal's own `end_formal_parameter` follows this event and
        // records `Ev::Formal` — recording one here too would double the
        // declaration (the second read finds an empty `pending_ty` and
        // redeclares the variable `Unknown`, #1758: `f()?.m()`'s receiver
        // classification then misses the written return type).
        let _ = self.name(token);
    }

    // —— local declarations —————————————————————————————————————————————

    fn begin_variables_declaration(
        &mut self,
        token: Token,
        late_kw: Option<Token>,
        var_final_or_const: Option<Token>,
    ) {
        let is_final = var_final_or_const.is_some_and(|token| {
            matches!(
                self.kind(token),
                Some(dartq_scanner::TokenKind::Keyword(
                    dartq_scanner::Keyword::Final | dartq_scanner::Keyword::Const
                ))
            )
        });
        // The statement-head anchor: the official's first-dead node for a
        // dead declaration list is the statement itself, whose offset is
        // the earliest header token — `late`, the `var`/`final`/`const`
        // keyword, or the annotation head (the grammar's
        // `finalConstVarOrType` always fires `handle_type` or
        // `handle_no_type` immediately before this hook, so the latched
        // head is fresh here). The event's span feeds only the dead-code
        // window's anchor; anchoring at the first declared name instead
        // landed the row at the name (oracle: the statement head).
        let head = [late_kw, var_final_or_const, self.type_head, Some(token)]
            .into_iter()
            .flatten()
            .min_by_key(|token| token.range.offset)
            .unwrap_or(token);
        self.record(Ev::VarDeclBegin {
            late: late_kw.is_some(),
            is_final,
            span: head,
        });
        // Top-level capture: outside any executable and outside every
        // class-like container the list declares top-level variables — its
        // header face joins the fold's top-level read table (the first
        // intern of a name wins; a redeclaration is the
        // duplicate-definition error's territory, not the read face's).
        let header = self.top_last_type.take();
        if self.buffer.is_none() && self.container_depth == 0 {
            self.top_decl = header.map(|(ty, args)| TopVar { ty, args });
        }
    }

    fn end_variables_declaration(&mut self, _count: usize, end: Option<Token>) {
        self.record(Ev::VarDeclEnd);
        self.top_decl = None;
        if let Some(end) = end {
            self.record(Ev::StmtEnd {
                end: end.range.end(),
            });
        }
    }

    fn begin_initialized_identifier(&mut self, token: Token) {
        let name = self.name(token);
        // The top-level read table gains the declarator (the capture gate
        // is `begin_variables_declaration`'s).
        if let Some(header) = self.top_decl {
            self.top_level.entry(name).or_insert(header);
        }
        self.record(Ev::InitializedIdent { name, token });
    }

    fn end_initialized_identifier(&mut self, _name: Token) {
        // The declare/initialize point is `InitializedIdent`; the paired
        // end event carries no payload the replay needs.
    }

    fn handle_no_variable_initializer(&mut self, _token: Token) {
        self.record(Ev::NoInitializer);
    }

    fn begin_variable_initializer(&mut self, _token: Token) {
        // A declaration outside any executable folds its initializer as
        // its own executable (the official analyzes field and top-level
        // initializers): the buffer opens here, and reads of earlier
        // top-level declarations inside it publish their rows through the
        // top-level read table. Initializers nested inside the open world
        // (a closure body's locals) count as brackets, never worlds.
        if self.buffer.is_none() {
            self.exec_begin(false);
            self.initializer_world = 1;
        } else if self.initializer_world > 0 {
            self.initializer_world += 1;
        }
        self.record(Ev::InitializerBegin);
    }

    fn end_variable_initializer(&mut self, token: Token) {
        self.record(Ev::InitializerEnd);
        // Only the bracket that opened the world closes it: a nested
        // bracket's end must not fire `exec_end` mid-body and desync the
        // executable framing (see [`Flow::initializer_world`]).
        if self.initializer_world > 0 {
            self.initializer_world -= 1;
            if self.initializer_world == 0 {
                // The `=` token anchors the initializer's body-completion
                // fact; the semantic layer stages completions by exact
                // executable end span, so the initializer face's row
                // (never staged) drops silently there.
                self.exec_end(false, token);
            }
        }
    }

    fn handle_pattern_variable_declaration_statement(
        &mut self,
        keyword: Token,
        _equals: Token,
        semicolon: Token,
    ) {
        self.record(Ev::PatternVarDeclStmt { span: keyword });
        self.record(Ev::StmtEnd {
            end: semicolon.range.end(),
        });
    }

    // —— types ——————————————————————————————————————————————————————————

    fn handle_type(&mut self, name: Token, question: Option<Token>) {
        // The type name is the qualified chain's last identifier — the
        // element the spelling resolves to. `name` is the chain's first
        // token (`io` of `io.Directory`); keying on it collapsed every
        // `io.*` type onto `Name(io)`, which deaded `x is io.File` after
        // `x is io.Directory` (`factor` bottomed on the same id).
        let id = self.qualified_name(name);
        // Classify on the resolved name's lexeme: `name_lexeme` of the id
        // is the last identifier's spelling — the prefix can never spell
        // a special base (`void`, `dynamic`), the tail can.
        let base = match self.name_lexeme(id) {
            "dynamic" => crate::ty::Base::Dynamic,
            "void" => crate::ty::Base::Void,
            "Never" => crate::ty::Base::Never,
            // The written `Null` type name IS the core null type (upstream
            // Dart SDK flow analysis `_isNullType`'s `isDartCoreNull`): the
            // null-literal-equivalent classification that drives
            // disjointness (`_typesAreDisjointDueToNullability`) and
            // promotion faces.
            "Null" => crate::ty::Base::Null,
            _ => crate::ty::Base::Name(id),
        };
        // Consumed per row, so the pending list can never leak across
        // types; an argument row nested in an open group joins it.
        let args = std::mem::take(&mut self.pending_type_args);
        let nullable = question.is_some();
        if let Some(group) = self.type_arg_groups.last_mut() {
            group.push(crate::ty::ArgTy {
                base,
                nullable,
                args,
            });
        }
        self.record(Ev::Type {
            base,
            nullable,
            args,
        });
        // The statement-head latch (see [`Flow::type_head`]): every
        // declaration's `finalConstVarOrType` fires its type row (or
        // `handle_no_type`) immediately before `begin_variables_declaration`,
        // so the most recent head is the annotation of the declaration now
        // starting — a qualified chain's outermost head fires last and keys
        // its first token.
        self.type_head = Some(name);
        // The top-level header latch (see `top_last_type`): a declaration
        // outside any executable reads the row that immediately precedes
        // it.
        if self.buffer.is_none() {
            self.top_last_type = Some((crate::ty::Ty { base, nullable }, args));
        }
        // A head row (no open argument group) inside a type-variable
        // completion is that variable's bound — the definitive face for
        // every non-function shape (it fires last: after the bound's own
        // argument groups, and after any nested function-type capture).
        if self.type_arg_groups.is_empty() && self.tv_capturing() {
            self.capture_bound_type(base, nullable, args);
        }
        // The extension header's on-type latch (see
        // [`Flow::extension_on_head`]): only depth-0 heads race — the
        // on-type's own type arguments nest inside their group — and only
        // a plain named head feeds the `this` staging (a `dynamic`/`void`/
        // `Never`/`Null` head answers nothing the verdict reads).
        if self.in_extension_header
            && self.type_arg_groups.is_empty()
            && let crate::ty::Base::Name(head) = base
        {
            self.extension_on_head = Some((head, nullable));
        }
    }

    fn begin_type_arguments(&mut self, _token: Token) {
        self.type_arg_groups.push(Vec::new());
    }

    fn handle_no_type_arguments(&mut self, _token: Token) {
        self.pending_type_args = crate::ty::EMPTY_ARGS;
    }

    fn end_type_arguments(&mut self, _count: usize, _begin: Token, _end: Token) {
        // The group is complete (every argument row fired before the
        // close, nested groups folded bottom-up); the head row follows.
        let entries = self.type_arg_groups.pop().unwrap_or_default();
        self.pending_type_args = self.intern_args(entries);
    }

    fn handle_no_type(&mut self, _last_consumed: Token) {
        self.record(Ev::NoType);
        // The statement-head latch (see [`Flow::type_head`]) clears with the
        // header's own absence — an untyped declaration anchors at its
        // modifier or name.
        self.type_head = None;
        // An untyped header answers unknown (the latch clears).
        if self.buffer.is_none() {
            self.top_last_type = None;
        }
    }

    fn end_function_type(&mut self, _function: Token, question: Option<Token>) {
        self.record(Ev::FunctionType {
            nullable: question.is_some(),
        });
        // A function-typed header has no name-level row; its own
        // return/parameter rows must not leak into the latch.
        if self.buffer.is_none() {
            self.top_last_type = None;
        }
        // A function-type bound's definitive face: fires after its return
        // keyword and any nested argument captures, so it wins. (A `void`
        // return's earlier capture is overwritten here.)
        if self.tv_capturing() {
            self.tv_bound = Some(crate::ty::Boundary {
                class: if question.is_some() {
                    crate::ty::Classification::PotentiallyNullable
                } else {
                    crate::ty::Classification::NonNullable
                },
                target: None,
                // The signature-less function head is all this level
                // spells; the official `isAlwaysExhaustive` answers `false`
                // for every function type, so the head is the whole
                // answer the verdict needs.
                bound: (
                    crate::ty::Ty {
                        base: crate::ty::Base::Function,
                        nullable: question.is_some(),
                    },
                    crate::ty::EMPTY_ARGS,
                ),
            });
        }
    }

    fn handle_void_keyword(&mut self, _token: Token) {
        self.record(Ev::VoidType);
        // A void header never types a variable; the latch clears so the
        // next declaration's face decides.
        if self.buffer.is_none() {
            self.top_last_type = None;
        }
        // A bare `void` bound's provisional face (a top type); a
        // function-type bound overwrites at its own end event.
        if self.tv_capturing() {
            self.tv_bound = Some(crate::ty::Boundary {
                class: crate::ty::Classification::PotentiallyNullable,
                target: None,
                bound: (
                    crate::ty::Ty {
                        base: crate::ty::Base::Void,
                        nullable: false,
                    },
                    crate::ty::EMPTY_ARGS,
                ),
            });
        }
    }

    // —— type variables (declared-bound nullability, #1077) ————————————

    fn begin_type_variables(&mut self, _token: Token) {}

    fn end_type_variables(&mut self, _begin: Token, _end: Token) {
        // The group is complete: drop any recovery-desynced state (the
        // committed boundaries survive — they belong to the declaration).
        self.tv_open = 0;
        self.tv_names.clear();
        self.tv_bound = None;
        self.tv_saved.clear();
    }

    fn begin_type_variable(&mut self, token: Token) {
        let id = self.name(token);
        self.tv_open += 1;
        self.tv_names.push(id);
        // A generic-function-type bound opens its own group inside the
        // enclosing completion; the outer bound's partial capture is none
        // of the inner variable's business.
        self.tv_saved.push(self.tv_bound.take());
    }

    fn end_type_variable(
        &mut self,
        _token: Token,
        _index: usize,
        extends_or_super: Option<Token>,
        _variance: Option<Token>,
    ) {
        let Some(id) = self.tv_names.pop() else {
            return;
        };
        let boundary = if extends_or_super.is_some() {
            self.tv_bound.take()
        } else {
            // No extends clause: the implicit bound is `Object?` (the
            // subtype rule's default, subtype.dart:313) — potentially
            // nullable, promoting to `T & Object` on a null check.
            let object = self.intern_lexeme("Object");
            Some(crate::ty::Boundary {
                class: crate::ty::Classification::PotentiallyNullable,
                target: Some((
                    crate::ty::Ty {
                        base: crate::ty::Base::Name(object),
                        nullable: false,
                    },
                    crate::ty::EMPTY_ARGS,
                )),
                // The implicit bound as written: `Object?`
                // (type_system.dart:868-877 reads the element's bound, and
                // the analyzer's default is the nullable top).
                bound: (
                    crate::ty::Ty {
                        base: crate::ty::Base::Name(object),
                        nullable: true,
                    },
                    crate::ty::EMPTY_ARGS,
                ),
            })
        };
        if let Some(boundary) = boundary {
            let index = id as usize;
            if self.boundaries.len() <= index {
                self.boundaries.resize(index + 1, None);
            }
            self.boundaries[index] = Some(boundary);
        }
        self.tv_bound = self.tv_saved.pop().flatten();
        self.tv_open -= 1;
    }

    fn begin_record_type(&mut self, _left_paren: Token) {
        self.tv_in_record = true;
    }

    fn end_record_type(
        &mut self,
        _left_paren: Token,
        question: Option<Token>,
        _count: usize,
        _has_named_fields: bool,
    ) {
        self.tv_in_record = false;
        // The record spelling owns no head in this lattice (a record is
        // neither a named nor a function base), so the type slot it feeds
        // takes the unknown face — the written `?` still rides it, the way
        // every unknown face keeps its nullability. Without this row the
        // slot keeps the LAST FIELD's `Type` row (the fields sink through
        // the same `Ev::Type` channel as any nested annotation), typing the
        // declared variable as that field's type: `({String? alias}) r`
        // typed `r` `String?`, and its member reads then resolved against
        // `String`. The unknown face also keeps the test/cast sinks
        // conservative — a record tested type certifies no dead branch.
        self.record(Ev::Type {
            base: crate::ty::Base::Unknown,
            nullable: question.is_some(),
            args: crate::ty::EMPTY_ARGS,
        });
    }

    fn end_top_level_declaration(&mut self, _end: Token) {
        self.boundaries.clear();
    }

    fn handle_invalid_top_level_declaration(&mut self, _end: Token) {
        self.boundaries.clear();
    }

    // —— statements ——————————————————————————————————————————————————————

    fn handle_expression_statement(&mut self, begin: Token, end: Token) {
        self.record(Ev::ExprStmt { span: begin });
        self.record(Ev::StmtEnd {
            end: end.range.end(),
        });
    }

    fn begin_if_statement(&mut self, token: Token) {
        self.record(Ev::IfBegin { span: token });
    }

    /// `handle_parenthesized_condition(begin, case, when)` — delivered at
    /// the condition's end. The `case` keyword exists only on the if-case
    /// form, so it is the exact signal the standalone pattern context
    /// opened inside this condition belongs to this if (an ordinary if
    /// must never adopt one: see `ThenBegin`).
    fn handle_parenthesized_condition(
        &mut self,
        _begin: Token,
        case_kw: Option<Token>,
        _when: Option<Token>,
    ) {
        if case_kw.is_some() {
            self.record(Ev::IfCaseCondition);
        }
    }

    fn begin_then_statement(&mut self, _token: Token) {
        self.record(Ev::ThenBegin);
    }

    fn begin_else_statement(&mut self, _token: Token) {
        self.record(Ev::ElseBegin);
    }

    fn end_if_statement(&mut self, _token: Token, else_kw: Option<Token>, end: Token) {
        self.record(Ev::IfEnd {
            has_else: else_kw.is_some(),
        });
        self.record(Ev::StmtEnd {
            end: end.range.end(),
        });
    }

    fn begin_while_statement(&mut self, token: Token) {
        self.record(Ev::WhileBegin { span: token });
    }

    fn begin_while_statement_body(&mut self, _token: Token) {
        self.record(Ev::WhileBodyBegin);
    }

    fn end_while_statement_body(&mut self, _end: Token) {
        self.record(Ev::WhileBodyEnd);
    }

    fn end_while_statement(&mut self, _token: Token, end: Token) {
        self.record(Ev::WhileEnd);
        self.record(Ev::StmtEnd {
            end: end.range.end(),
        });
    }

    fn begin_do_while_statement(&mut self, token: Token) {
        self.record(Ev::DoBegin { span: token });
    }

    fn begin_do_while_statement_body(&mut self, token: Token) {
        self.record(Ev::DoBodyBegin);
        // The official `while` part starts at a block body's closing brace
        // (dead_code_verifier.dart:292-308); a non-block body starts at
        // the `while` keyword itself. The hook's own token is the body's
        // first token — a `{` means the body is the block whose `}` (the
        // last `end_block` inside the body) carries the part's start.
        self.do_body_is_block = self.kind(token) == Some(dartq_scanner::TokenKind::OpenCurly);
        self.do_body_end = None;
    }

    fn end_do_while_statement_body(&mut self, _end: Token) {
        // The hook's token is the one past the body (`while`), so the
        // brace comes from `end_block` above.
        self.record(Ev::DoBodyEnd);
    }

    fn end_do_while_statement(&mut self, _token: Token, while_kw: Token, end: Token) {
        let while_start = match self.do_body_end.take() {
            Some(body_end) if self.kind(body_end) == Some(dartq_scanner::TokenKind::CloseCurly) => {
                body_end.range.offset
            }
            _ => while_kw.range.offset,
        };
        self.record(Ev::DoEnd {
            while_range: dartq_source::SourceRange {
                offset: while_start,
                length: end.range.end() - while_start,
            },
        });
        self.record(Ev::StmtEnd {
            end: end.range.end(),
        });
    }

    fn begin_for_statement(&mut self, token: Token) {
        self.record(Ev::ForBegin { span: token });
    }

    fn handle_for_loop_parts(
        &mut self,
        _for_kw: Token,
        _left_paren: Token,
        left_separator: Token,
        right_separator: Token,
        _expression_count: usize,
    ) {
        self.record(Ev::ForParts {
            left_sep: left_separator,
            right_sep: right_separator,
        });
    }

    fn begin_for_statement_body(&mut self, _token: Token) {
        self.record(Ev::ForBodyBegin);
    }

    fn end_for_statement_body(&mut self, _end: Token) {
        self.record(Ev::ForBodyEnd);
    }

    fn end_for_statement(&mut self, end: Token) {
        self.record(Ev::ForEnd);
        self.record(Ev::StmtEnd {
            end: end.range.end(),
        });
    }

    fn handle_for_in_loop_parts(
        &mut self,
        _await_kw: Option<Token>,
        _for_kw: Token,
        _left_paren: Token,
        _pattern_keyword: Option<Token>,
        _in_kw: Token,
    ) {
        self.record(Ev::ForInParts);
    }

    fn begin_for_in_expression(&mut self, _token: Token) {
        self.record(Ev::ForInExprBegin);
    }

    fn end_for_in_expression(&mut self, _end: Token) {
        self.record(Ev::ForInExprEnd);
    }

    fn begin_for_in_body(&mut self, _token: Token) {
        self.record(Ev::ForInBodyBegin);
    }

    fn end_for_in_body(&mut self, _end: Token) {
        self.record(Ev::ForInBodyEnd);
    }

    fn end_for_in(&mut self, end: Token) {
        self.record(Ev::ForInEnd);
        self.record(Ev::StmtEnd {
            end: end.range.end(),
        });
    }

    fn end_return_statement(&mut self, has_expression: bool, begin: Token, end: Token) {
        self.record(Ev::ReturnBegin { span: begin });
        self.record(Ev::ReturnEnd { has_expression });
        self.record(Ev::StmtEnd {
            end: end.range.end(),
        });
    }

    fn end_yield_statement(&mut self, begin: Token, _star: Option<Token>, end: Token) {
        self.record(Ev::YieldBegin { span: begin });
        self.record(Ev::YieldEnd {
            span: dartq_source::SourceRange {
                offset: begin.range.offset,
                length: end.range.end().saturating_sub(begin.range.offset),
            },
        });
        self.record(Ev::StmtEnd {
            end: end.range.end(),
        });
    }

    fn begin_await_expression(&mut self, token: Token) {
        self.record(Ev::AwaitBegin { token });
    }

    fn end_await_expression(&mut self, begin: Token, end: Token) {
        self.record(Ev::AwaitEnd {
            span: dartq_source::SourceRange {
                offset: begin.range.offset,
                length: end.range.end().saturating_sub(begin.range.offset),
            },
        });
    }

    fn handle_break_statement(&mut self, has_target: bool, break_kw: Token, end: Token) {
        let label = self.take_label(has_target);
        self.record(Ev::Break {
            label,
            span: break_kw,
        });
        self.record(Ev::StmtEnd {
            end: end.range.end(),
        });
    }

    fn handle_continue_statement(&mut self, has_target: bool, continue_kw: Token, end: Token) {
        let label = self.take_label(has_target);
        self.record(Ev::Continue {
            label,
            span: continue_kw,
        });
        self.record(Ev::StmtEnd {
            end: end.range.end(),
        });
    }

    fn handle_label(&mut self, _colon: Token) {
        // The label name arrives through `handle_identifier`
        // (`LabelDeclaration`); the colon carries no id.
    }

    fn begin_labeled_statement(&mut self, token: Token, _label_count: usize) {
        // The first label rides `LabeledBegin`, the rest follow as
        // `LabelDecl` — one frame tracks every label of the statement, so
        // `a: b: s` resolves a jump to either name (official
        // `getLabelTarget` scans every label of the labeled statement).
        let labels = std::mem::take(&mut self.pending_labels);
        let first = labels.first().copied();
        // The event token is the statement PAST the labels (official fasta
        // shape); the dead row anchors at the labeled statement's own
        // `firstToken` — the first label (dead_code_verifier.dart:253) —
        // so the latched label token wins when one was seen. No latch
        // (recovered label without a name) falls back to the event token.
        let anchor = self.pending_label_token.take().unwrap_or(token);
        self.record(Ev::LabeledBegin {
            label: first,
            span: anchor,
        });
        for id in labels.into_iter().skip(1) {
            self.record(Ev::LabelDecl { id });
        }
    }

    fn end_labeled_statement(&mut self, _label_count: usize) {
        self.record(Ev::LabeledEnd);
    }

    fn begin_try_statement(&mut self, token: Token) {
        self.record(Ev::TryBegin { span: token });
    }

    fn handle_catch_block(
        &mut self,
        _on_kw: Option<Token>,
        _catch_kw: Option<Token>,
        _comma: Option<Token>,
    ) {
        self.record(Ev::CatchClauseEnd);
    }

    fn begin_catch_clause(&mut self, _token: Token) {
        self.record(Ev::CatchClauseBegin);
    }

    fn end_catch_clause(&mut self, _token: Token) {
        // The clause HEADER closes here; the clause BODY ends at
        // `handle_catch_block`, which records the state join.
    }

    fn handle_finally_block(&mut self, _finally_kw: Token) {
        // Already recorded at the finally block's begin (official
        // `tryFinallyStatement_finallyBegin` position — before the body).
    }

    fn end_try_statement(
        &mut self,
        _count: usize,
        _begin: Token,
        finally_kw: Option<Token>,
        end: Token,
    ) {
        self.record(Ev::TryEnd {
            has_finally: finally_kw.is_some(),
        });
        self.record(Ev::StmtEnd {
            end: end.range.end(),
        });
    }

    fn begin_switch_statement(&mut self, token: Token) {
        self.record(Ev::SwitchBegin { span: token });
    }

    fn begin_switch_block(&mut self, _token: Token) {
        self.record(Ev::SwitchBlockBegin);
    }

    fn end_switch_block(&mut self, _count: usize, _begin: Token, _end: Token) {
        self.record(Ev::SwitchBlockEnd);
    }

    fn begin_switch_case(&mut self, label_count: usize, expression_count: usize, token: Token) {
        // Case labels are counted on `CaseBegin`; drop their name
        // captures so they can never attach to a later labeled statement.
        self.pending_labels.clear();
        self.pending_label_token = None;
        self.record(Ev::CaseBegin {
            label_count,
            expression_count,
            // A default-headed group's member begin token is the `default`
            // keyword (or the first label of a labeled default) — the
            // head-time anchor face.
            kw: (expression_count == 0).then_some(token),
        });
    }

    fn begin_case_expression(&mut self, case_kw: Token) {
        self.record(Ev::CaseExprBegin { span: case_kw });
    }

    fn handle_switch_case_no_when_clause(&mut self, _token: Token) {
        self.record(Ev::CaseNoWhen);
    }

    fn end_case_expression(&mut self, case_kw: Token, when: Option<Token>, _colon: Token) {
        self.record(Ev::CaseExprEnd {
            has_guard: when.is_some(),
            kw: case_kw,
        });
    }

    fn end_switch_case(
        &mut self,
        _label_count: usize,
        _expression_count: usize,
        default_kw: Option<Token>,
        _colon_after_default: Option<Token>,
        statement_count: usize,
        _begin: Token,
        _end: Token,
    ) {
        self.record(Ev::CaseEnd {
            statement_count,
            default_kw,
        });
    }

    fn end_switch_statement(&mut self, _token: Token, end: Token) {
        self.record(Ev::SwitchEnd);
        self.record(Ev::StmtEnd {
            end: end.range.end(),
        });
    }

    fn begin_switch_expression(&mut self, token: Token) {
        self.record(Ev::SwitchExprBegin { span: token });
    }

    fn begin_switch_expression_block(&mut self, _token: Token) {
        self.record(Ev::SwitchExprBlockBegin);
    }

    fn end_switch_expression_block(&mut self, _count: usize, _begin: Token, _end: Token) {
        self.record(Ev::SwitchExprBlockEnd);
    }

    fn begin_switch_expression_case(&mut self) {
        self.record(Ev::SwitchExprCaseBegin);
    }

    fn handle_switch_expression_case_pattern(&mut self, _token: Token) {
        self.record(Ev::SwitchExprCasePattern);
    }

    fn end_switch_expression_case(
        &mut self,
        begin: Token,
        when: Option<Token>,
        _arrow: Token,
        _end: Token,
    ) {
        self.record(Ev::SwitchExprCaseEnd {
            has_guard: when.is_some(),
            // The alternative's first token: the matched-type row's clause
            // anchor (#1349).
            begin,
        });
    }

    // —— expressions —————————————————————————————————————————————————————

    fn handle_identifier(&mut self, token: Token, context: IdentifierContext) {
        match context {
            IdentifierContext::Expression
            | IdentifierContext::ExpressionContinuation
            | IdentifierContext::NamedArgumentReference
            | IdentifierContext::CatchParameter
            | IdentifierContext::LocalVariableDeclaration => {
                let name = self.name(token);
                // The implicit-`this` transport: a bare read inside an
                // extension member reads the written `on` type
                // (`member` == `this.member`), so the flow's
                // unresolved-read face needs the on-type at the read's
                // own offset. Staged into the `this` map (offset-keyed;
                // bare-read offsets never collide with `this` tokens)
                // exactly like `handle_this_expression` — the consumer
                // gates on the invalid face, a resolved on-type keeps
                // the unknown-read posture (the member's own type is an
                // element question this layer cannot answer).
                if context == IdentifierContext::Expression
                    && let Some((head, false)) = self.extension_on_head
                {
                    self.staged_this.entry(token.range.offset).or_insert(
                        crate::ty::Ty {
                            base: crate::ty::Base::Name(head),
                            nullable: false,
                        },
                    );
                }
                self.record(Ev::Ident {
                    name,
                    token,
                    context,
                });
            }
            // A constructor-reference head (`new C()`, `const C()`,
            // `C<T>.named(...)`): the creation's value carries the head's
            // named type — without it the invocation nets no stack entry
            // and a following `x = C()` write consumes the wrong operand.
            IdentifierContext::ConstructorReference => {
                // Same qualified-head contract as `handle_type`: `io` of
                // `io.File()` fires here, the type is the tail `File` —
                // the implicit-creation write must promote to `File`, not
                // the shared `io` prefix. (Expression identifiers keep the
                // head token: `context.read()`'s `context` resolves to the
                // receiver, not the selector.)
                let name = self.qualified_name(token);
                self.record(Ev::Ident {
                    name,
                    token,
                    context,
                });
            }
            IdentifierContext::LabelReference => {
                // A break/continue target name (or case label): held for
                // the next break/continue event, which carries only
                // `has_target`.
                self.pending_label_ref = Some(self.name(token));
            }
            IdentifierContext::LabelDeclaration => {
                // A label name (`label:` on a statement or a switch case),
                // consumed by the enclosing construct's begin event.
                let id = self.name(token);
                self.pending_labels.push(id);
                if self.pending_label_token.is_none() {
                    self.pending_label_token = Some(token);
                }
            }
            _ => {}
        }
    }

    fn handle_literal_bool(&mut self, token: Token) {
        let value = self.kind(token)
            == Some(dartq_scanner::TokenKind::Keyword(
                dartq_scanner::Keyword::True,
            ));
        self.record(Ev::Bool { value });
    }

    fn handle_literal_null(&mut self, _token: Token) {
        self.record(Ev::Null);
    }

    fn handle_literal_int(&mut self, token: Token) {
        self.record(Ev::OtherExpr { span: token });
    }

    fn handle_literal_int_with_separators(&mut self, token: Token) {
        self.record(Ev::OtherExpr { span: token });
    }

    fn handle_literal_double(&mut self, token: Token) {
        self.record(Ev::OtherExpr { span: token });
    }

    fn handle_literal_double_with_separators(&mut self, token: Token) {
        self.record(Ev::OtherExpr { span: token });
    }

    fn end_literal_string(&mut self, interpolation_count: usize, _end: Token) {
        self.record(Ev::EndLiteralString {
            interpolation_count,
        });
    }

    fn begin_literal_symbol(&mut self, _token: Token) {
        // The symbol body contributes no expression events.
    }

    fn handle_literal_list(
        &mut self,
        count: usize,
        _left: Token,
        _const_keyword: Option<Token>,
        _right: Token,
    ) {
        self.record(Ev::ListLitEnd { count });
    }

    fn handle_literal_set_or_map(
        &mut self,
        count: usize,
        _left: Token,
        _const_keyword: Option<Token>,
        _right: Token,
        _has_set_entry: bool,
    ) {
        self.record(Ev::ListLitEnd { count });
    }

    fn handle_literal_map_entry(
        &mut self,
        _colon: Token,
        _end: Token,
        _null_aware_key: Option<Token>,
        _null_aware_value: Option<Token>,
    ) {
        self.record(Ev::MapEntryEnd);
    }

    fn handle_this_expression(&mut self, token: Token, _context: IdentifierContext) {
        self.record(Ev::This { span: token });
        // The extension on-type face (see [`Flow::extension_on_head`]):
        // `this` inside an extension member reads the written `on` type
        // (upstream Dart SDK source,
        // pkg/analyzer/lib/src/dart/element/extension_member.dart,
        // `ExtensionImpl.extendedType`). The mounted consumer stages the
        // same answer through [`Flow::stage_this_type`] when the type is
        // name-level spellable — but an argument-bearing `on` spelling
        // (`extension E<T> on A<T>`) drops there, and the flow lattice
        // still needs the HEAD: the switch-end exhaustiveness verdict is
        // an element question (`TypeSystem.isAlwaysExhaustive`'s sealed
        // arm reads the declaration, not its arguments — the nullability
        // half is the space model's). Fill only the gap: a first-writer
        // map keeps the consumer's staged entry, and only a non-nullable
        // head stages (a nullable `on` type keeps the unknown-read
        // posture — the nullable face would arm null-dereference rows
        // this capture cannot answer for).
        if let Some((head, false)) = self.extension_on_head {
            self.staged_this
                .entry(token.range.offset)
                .or_insert(crate::ty::Ty {
                    base: crate::ty::Base::Name(head),
                    nullable: false,
                });
        }
    }

    fn handle_super_expression(&mut self, token: Token, _context: IdentifierContext) {
        self.record(Ev::OtherExpr { span: token });
    }

    fn handle_unary_prefix_expression(&mut self, operator: Token) {
        self.record(Ev::UnaryPrefix {
            is_not: self.kind(operator) == Some(dartq_scanner::TokenKind::Bang),
        });
    }

    fn handle_non_null_assert_expression(&mut self, _bang: Token) {
        self.record(Ev::NonNullAssert);
    }

    fn begin_binary_expression(&mut self, operator: Token) {
        self.record(Ev::BinaryBegin {
            op: self.bin_op(operator),
            span: operator,
        });
    }

    fn handle_binary_rhs_end(&mut self, _operator: Token, rhs_last: Token) {
        self.binary_rhs_end = Some(rhs_last.range.end());
    }

    fn end_binary_expression(&mut self, operator: Token, end: Token) {
        // The row's extent is `rightOperand.end` (the doc contract on
        // `Ev::BinaryEnd::end`): the parser's `end` param tiles a bare-send
        // operand through its follower, so prefer the true last token the
        // pre-close hook just delivered.
        let end = self.binary_rhs_end.take().unwrap_or(end.range.end());
        self.record(Ev::BinaryEnd {
            op: self.bin_op(operator),
            end,
        });
    }

    fn begin_is_operator_type(&mut self, _operator: Token) {
        self.record(Ev::IsBegin);
    }

    // The polarity only arrives with the follow-up `handle_is_operator`
    // call (official `parseIsOperatorRest` fires `endIsOperatorType`
    // before `handleIsOperator`), so the flow's completion pair is
    // emitted there: the interp applies the branch at `IsEnd` from the
    // `(is_not, tested)` state `IsOperator` establishes.
    fn end_is_operator_type(&mut self, _operator: Token) {}

    fn handle_is_operator(&mut self, _is_operator: Token, not: Option<Token>) {
        self.record(Ev::IsOperator {
            is_not: not.is_some(),
        });
        self.record(Ev::IsEnd);
    }

    fn begin_as_operator_type(&mut self, operator: Token) {
        self.record(Ev::AsBegin { span: operator });
    }

    fn end_as_operator_type(&mut self, _operator: Token) {
        self.record(Ev::AsEnd);
    }

    fn handle_as_operator(&mut self, _operator: Token) {
        self.record(Ev::AsOperator);
    }

    fn handle_assignment_expression(&mut self, operator: Token, end: Token) {
        // The end token is the updater expression's end — the anchor the
        // dead-updater row needs (header parts carry no statement wrapper,
        // dead_code_verifier.dart:361-375's `updaters.endToken`).
        self.record(Ev::Assign {
            plain: self.is_plain_assign(operator),
            if_null: matches!(
                self.kind(operator),
                Some(dartq_scanner::TokenKind::QuestionQuestionEq)
            ),
            end: end.range.end(),
            field_init: false,
        });
    }

    fn begin_if_null_assignment_rhs(&mut self, operator: Token) {
        self.record(Ev::IfNullAssignBegin { op: operator });
    }

    fn begin_initializer(&mut self, _token: Token) {
        if let Some(buffer) = &self.buffer {
            self.init_starts.push(buffer.len());
        }
    }

    fn end_initializer(&mut self, end: Token) {
        let Some(start) = self.init_starts.pop() else {
            return;
        };
        // The field write `f = e` / `this.f = e` parses as the entry's own
        // expression: its `Assign` is the last one in this window and ends
        // exactly at the entry's end. `assert(...)` entries bracket their
        // inner assigns inside `AssertBegin/End` with different ends, so
        // only the field form matches. A redirect (`super(...)`/`this(...)`)
        // fires no `end_initializer` — its window never reaches here.
        if let Some(buffer) = &mut self.buffer {
            let end = end.range.end();
            if let Some(ev) = buffer[start..]
                .iter_mut()
                .rev()
                .find(|ev| matches!(ev, Ev::Assign { end: e, .. } if *e == end))
            {
                let Ev::Assign { field_init, .. } = ev else {
                    return;
                };
                *field_init = true;
            }
        }
    }

    fn handle_unary_prefix_assignment_expression(&mut self, operator: Token) {
        // The operator's span begins the dead-updater row; its end folds
        // from the operand's events (the parser reports the prefix
        // assignment after the operand parse).
        self.record(Ev::UnaryPrefixAssign { span: operator });
    }

    fn handle_unary_postfix_assignment_expression(&mut self, operator: Token) {
        // The operator's end is the updater expression's end — the anchor
        // the dead-updater row needs (header parts carry no statement
        // wrapper, dead_code_verifier.dart:361-365's `updaters.endToken`).
        self.record(Ev::UnaryPostfixAssign {
            end: operator.range.end(),
        });
    }

    fn handle_for_initializer_local_variable_declaration(&mut self, _end: Token, _for_in: bool) {
        // The init/cond index boundary: the condition's expression events
        // precede their `ExprStmt` wrapper (the wrapper fires at the
        // statement's end), so a literal condition's value would fall into
        // the init region of the offset-based header partition.
        self.record(Ev::ForInit);
    }

    fn handle_for_initializer_expression_statement(&mut self, _end: Token, _for_in: bool) {
        self.record(Ev::ForInit);
    }

    fn handle_for_initializer_empty_statement(&mut self, _at: Token) {
        self.record(Ev::ForInit);
    }

    fn handle_pattern_assignment(&mut self, _equals: Token) {
        self.record(Ev::PatternAssign);
    }

    fn handle_for_initializer_pattern_variable_assignment(
        &mut self,
        _pattern_keyword: Token,
        _equals: Token,
    ) {
        self.record(Ev::PatternAssign);
    }

    fn begin_conditional_expression(&mut self, _question: Token) {
        self.record(Ev::CondBegin);
    }

    fn end_conditional_expression_then_arm(
        &mut self,
        _question: Token,
        then_begin: Token,
        _colon: Token,
        then_end: Token,
    ) {
        // Latched for the argless colon call that follows immediately.
        self.cond_then = Some((then_begin, then_end.range.end()));
    }

    fn handle_conditional_expression_colon(&mut self) {
        let (then_begin, then_end) = self.cond_then.take().unwrap_or((
            TokenSpan {
                id: 0,
                range: dartq_source::SourceRange {
                    offset: 0,
                    length: 0,
                },
            },
            0,
        ));
        self.record(Ev::CondColon {
            then_begin,
            then_end,
        });
    }

    fn end_conditional_expression(&mut self, _question: Token, colon: Token, end: Token) {
        // The else arm starts at the token after the colon (official
        // `flowEnd(elseExpression)` rows span the arm's own range).
        let else_begin = self
            .tokens
            .next(colon.id as usize)
            .and_then(|index| self.span_at(index))
            .unwrap_or(colon);
        self.record(Ev::CondEnd {
            else_begin,
            end: end.range.end(),
        });
    }

    fn handle_dot_access(&mut self, _token: Token, member: Token, is_null_aware: bool) {
        self.record(Ev::DotAccess {
            null_aware: is_null_aware,
            member,
        });
    }

    fn handle_null_aware_index_operand_begin(&mut self, _question: Token) {
        self.record(Ev::NullAwareIndexOperandBegin);
    }

    fn handle_null_aware_member_begin(&mut self, _question: Token) {
        self.record(Ev::NullAwareMemberBegin);
    }

    fn handle_indexed_expression(&mut self, question: Option<Token>, _open: Token, _close: Token) {
        self.record(Ev::IndexAccess {
            null_aware: question.is_some(),
        });
    }

    fn handle_send(&mut self, _begin: Token, _end: Token) {
        // The invocation's own method type arguments (`C<int>()`) complete
        // with no head `handle_type` to consume them — the head is the
        // callee identifier — so they ride the send (the implicit-creation
        // face's written tree, #2243). Taking them here also clears a
        // group no consumer claimed, which would otherwise leak into the
        // next construct's type row (the same posture
        // `handle_object_pattern` takes for its head-less group).
        let args = std::mem::take(&mut self.pending_type_args);
        self.record(Ev::Send { args });
    }

    fn handle_send_without_arguments(&mut self, _begin: Token, _end: Token, _next: Token) {
        self.record(Ev::SendNoArgs);
    }

    fn begin_arguments(&mut self, _token: Token) {
        self.record(Ev::ArgumentsBegin);
    }

    fn end_arguments(&mut self, count: usize, _begin: Token, _end: Token) {
        self.record(Ev::ArgumentsEnd { count });
    }

    fn handle_named_argument(&mut self, _colon: Token) {
        // The named-argument marker rides the identifier event; the value
        // expression just completed is the argument boundary.
        self.record(Ev::ArgumentEnd);
    }

    fn handle_positional_argument(&mut self, _token: Token) {
        self.record(Ev::ArgumentEnd);
    }

    fn handle_throw_expression(&mut self, _throw: Token, _end: Token) {
        self.record(Ev::Throw);
    }

    fn begin_parenthesized_expression_or_record_literal(&mut self, _token: Token) {
        self.record(Ev::ParenBegin);
    }

    fn end_parenthesized_expression(&mut self, _token: Token) {
        self.record(Ev::ParenEnd);
    }

    fn end_record_literal(&mut self, _token: Token, count: usize, _const_keyword: Option<Token>) {
        self.record(Ev::RecordLitEnd { count });
    }

    fn begin_cascade(&mut self, operator: Token) {
        self.record(Ev::CascadeBegin {
            null_aware: matches!(
                self.kind(operator),
                Some(dartq_scanner::TokenKind::QuestionPeriodPeriod)
            ),
        });
    }

    fn handle_cascade_access(&mut self, _operator: Token, _end: Token, is_null_aware: bool) {
        self.record(Ev::CascadeAccess {
            null_aware: is_null_aware,
        });
    }

    fn end_cascade(&mut self) {
        self.record(Ev::CascadeEnd);
    }

    fn handle_spread_expression(&mut self, _operator: Token) {
        self.record(Ev::Spread);
    }

    fn handle_null_aware_element(&mut self, _token: Token) {
        self.record(Ev::NullAwareElement);
    }

    fn begin_if_control_flow(&mut self, _token: Token) {
        self.record(Ev::IfControlFlowBegin);
    }

    fn handle_then_control_flow(&mut self, token: Token) {
        self.record(Ev::ThenControlFlow {
            arm_start: token.range.offset,
        });
    }

    fn handle_else_control_flow(&mut self, else_kw: Token, else_start: Token) {
        self.record(Ev::ElseControlFlow {
            then_end: else_kw.range.offset,
            else_start: else_start.range.offset,
        });
    }

    fn end_if_control_flow(&mut self, token: Token) {
        self.record(Ev::EndIfControlFlow {
            has_else: false,
            arm_end: token.range.offset,
        });
    }

    fn end_if_else_control_flow(&mut self, token: Token) {
        self.record(Ev::EndIfControlFlow {
            has_else: true,
            arm_end: token.range.offset,
        });
    }

    fn begin_for_control_flow(&mut self, _await_kw: Option<Token>, _for_kw: Token) {
        self.record(Ev::ForControlFlowBegin);
    }

    fn end_for_control_flow(&mut self, _token: Token) {
        self.record(Ev::ForControlFlowEnd { for_in: false });
    }

    fn end_for_in_control_flow(&mut self, _token: Token) {
        self.record(Ev::ForControlFlowEnd { for_in: true });
    }

    // —— patterns ————————————————————————————————————————————————————————

    fn begin_pattern(&mut self, _token: Token) {
        self.record(Ev::PatternBegin);
    }

    fn end_pattern(&mut self, _token: Token) {
        self.record(Ev::PatternEnd);
    }

    fn begin_constant_pattern(&mut self, _const_keyword: Option<Token>) {
        self.record(Ev::ConstantPatternBegin);
    }

    fn end_constant_pattern(&mut self, _const_keyword: Option<Token>) {
        self.record(Ev::ConstantPatternEnd);
    }

    fn handle_declared_variable_pattern(
        &mut self,
        keyword: Option<Token>,
        variable: Token,
        _in_assignment_pattern: bool,
    ) {
        let name = self.name(variable);
        self.record(Ev::DeclaredVarPattern {
            name,
            is_final: keyword.is_some(),
            token: variable,
        });
    }

    fn handle_assigned_variable_pattern(&mut self, variable: Token) {
        let name = self.name(variable);
        self.record(Ev::AssignedVarPattern { name });
    }

    fn handle_wildcard_pattern(&mut self, _keyword: Option<Token>, wildcard: Token) {
        self.record(Ev::Wildcard { token: wildcard });
    }

    fn begin_binary_pattern(&mut self, operator: Token) {
        if self.token_text(operator) == Some("||") {
            self.record(Ev::BinaryOrPatternBegin);
        }
    }

    fn end_binary_pattern(&mut self, operator: Token) {
        if self.token_text(operator) == Some("||") {
            self.record(Ev::BinaryOrPatternEnd);
        }
    }

    fn begin_pattern_guard(&mut self, _when: Token) {
        self.record(Ev::GuardBegin);
    }

    fn end_pattern_guard(&mut self, _token: Token) {
        self.record(Ev::GuardEnd);
    }

    fn begin_switch_case_when_clause(&mut self, _token: Token) {
        // A switch-statement case guard. The guard's value is mandatory
        // flow input: official `switch_endAlternative` receives it as
        // `guardInfo` (flow_analysis.dart:1725-1740).
        self.record(Ev::GuardBegin);
    }

    fn end_switch_case_when_clause(&mut self, _token: Token) {
        self.record(Ev::GuardEnd);
    }

    fn handle_object_pattern(
        &mut self,
        first_identifier: Token,
        dot: Option<Token>,
        _second_identifier: Option<Token>,
    ) {
        // The bare `Type(...)` head is the pattern's type; a `Type.field`
        // shorthand head names a member and constrains nothing here. The
        // written arguments are taken, not read: the group completes with
        // no head `handle_type` to consume them (the head is this event),
        // so leaving them would leak into the next construct's type row.
        // A typed field sub-pattern resets the pending list first, so a
        // `Type<...>(field: Typed x)` head degrades to the bare spelling —
        // under-precision, never a wrong non-empty tree.
        let args = std::mem::take(&mut self.pending_type_args);
        if dot.is_some() {
            return;
        }
        let name = self.name(first_identifier);
        self.record(Ev::ObjectPattern { name, args });
    }

    /// A record pattern's close: the shape test can fail — the refutable
    /// context's unmatched chain needs the failure arm.
    fn handle_record_pattern(&mut self, _left_paren: Token, _count: usize) {
        self.record(Ev::ShapePattern);
    }

    fn handle_list_pattern(&mut self, _count: usize, _left: Token, _right: Token) {
        self.record(Ev::ShapePattern);
    }

    fn handle_map_pattern(&mut self, _count: usize, _left: Token, _right: Token) {
        self.record(Ev::ShapePattern);
    }

    fn handle_relational_pattern(&mut self, _operator: Token) {
        self.record(Ev::ShapePattern);
    }

    fn handle_null_check_pattern(&mut self, _question: Token) {
        self.record(Ev::NullCheckPattern);
    }

    // —— deliberately unrecorded faces ———————————————————————————————————

    fn handle_recoverable_error(&mut self, _message: Message, _start: Token, _end: Token) {}
    fn handle_experiment_not_enabled(
        &mut self,
        _flag: ExperimentalFlag,
        _begin: Token,
        _end: Token,
    ) {
    }
    fn end_invalid_await_expression(&mut self, _begin: Token, _end: Token) {}
    fn end_invalid_yield_statement(&mut self, _begin: Token, _star: Option<Token>, _end: Token) {}
    fn handle_invalid_function_body(&mut self, _token: Token) {}
    fn handle_invalid_statement(&mut self, _begin: Token) {}
    fn begin_rethrow_statement(&mut self, _token: Token) {}
    fn end_rethrow_statement(&mut self, _rethrow_kw: Token, end: Token) {
        self.record(Ev::Rethrow);
        // The statement-completion contract every other statement kind
        // keeps: `StmtEnd` extends an open dead interval across the
        // rethrow and refreshes the began-dead snapshot, so the statement
        // AFTER a flow-killing `rethrow;` anchors its own row (the
        // `throw 0;` path gets this from `handle_expression_statement`).
        self.record(Ev::StmtEnd {
            end: end.range.end(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ev::Ev;
    use dartq_parser::{IdentifierContext, ParserListener};
    use dartq_scanner::ScanConfig;

    /// Label ids ride the recorded events: the first label on
    /// `LabeledBegin`, additional labels on `LabelDecl`, and the
    /// break/continue target on `Break`/`Continue` — interned by lexeme,
    /// so the same name at different tokens resolves to the same id.
    #[test]
    fn label_ids_are_recorded_and_stable() {
        let source = "void f() { a: b: break a; a: break a; }";
        let result = dartq_scanner::scan(
            source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            dartq_source::FileTable::new().intern(std::path::Path::new("test.dart")),
        );
        assert!(result.diagnostics.is_empty());
        let tokens: &'static TokenTable = Box::leak(Box::new(result.tokens));
        // The Nth token with the given lexeme (labels recur in the source).
        let id_of = |lexeme: &str, occurrence: usize| -> u32 {
            let mut seen = 0;
            for index in 0.. {
                match tokens.get(index) {
                    Some(_) if tokens.lexeme(source, index) == lexeme => {
                        if seen == occurrence {
                            return u32::try_from(index).expect("token index fits u32");
                        }
                        seen += 1;
                    }
                    Some(_) => {}
                    None => panic!("token {lexeme}#{occurrence} not found"),
                }
            }
            unreachable!()
        };
        let sp = |id: u32| TokenSpan {
            id,
            range: dartq_source::SourceRange {
                offset: id * 2,
                length: 1,
            },
        };
        let (a1, a2, b) = (id_of("a", 0), id_of("a", 1), id_of("b", 0));

        let mut flow = Flow::new(tokens, source);
        flow.begin_top_level_method(sp(0), None, None);
        // `a: b: break a;`
        flow.handle_identifier(sp(a1), IdentifierContext::LabelDeclaration);
        flow.handle_identifier(sp(b), IdentifierContext::LabelDeclaration);
        flow.begin_labeled_statement(sp(a1), 2);
        flow.handle_identifier(sp(a1), IdentifierContext::LabelReference);
        flow.handle_break_statement(true, sp(a1), sp(a1));
        flow.end_labeled_statement(2);
        // `a: break a;` — a different `a` token, same lexeme.
        flow.handle_identifier(sp(a2), IdentifierContext::LabelDeclaration);
        flow.begin_labeled_statement(sp(a2), 1);
        flow.handle_identifier(sp(a2), IdentifierContext::LabelReference);
        flow.handle_break_statement(true, sp(a2), sp(a2));
        flow.end_labeled_statement(1);

        let events = flow.buffer.as_ref().unwrap();
        // 5 framed events + two breaks, each of which also records its
        // statement-completion `StmtEnd`.
        assert_eq!(events.len(), 9);
        let Ev::LabeledBegin { label: first_a, .. } = events[0] else {
            panic!("expected LabeledBegin, got {:?}", events[0]);
        };
        let Ev::LabelDecl { id: b_id } = events[1] else {
            panic!("expected LabelDecl, got {:?}", events[1]);
        };
        assert_ne!(Some(b_id), first_a, "distinct labels intern distinctly");
        let Ev::Break { label, .. } = events[2] else {
            panic!("expected Break, got {:?}", events[2]);
        };
        assert_eq!(label, first_a, "the break target carries its label id");
        let Ev::LabeledBegin {
            label: second_a, ..
        } = events[5]
        else {
            panic!("expected LabeledBegin, got {:?}", events[5]);
        };
        assert_eq!(
            second_a, first_a,
            "the same lexeme interns to the same id across tokens"
        );
    }

    /// Type-base classification reads the type-name TOKEN's lexeme: the
    /// old code indexed the token table by the interned name id, so
    /// `dynamic`, `Never`, and every `void` past the file's head
    /// classified as a plain named type (`void` only worked because every
    /// fixture began `void f(`, colliding id 0 with token 0).
    #[test]
    fn type_bases_classify_on_the_type_token_not_the_interned_id() {
        let source = "int x; void f(dynamic d) { if (d is Never) {} }";
        let result = dartq_scanner::scan(
            source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            dartq_source::FileTable::new().intern(std::path::Path::new("test.dart")),
        );
        assert!(result.diagnostics.is_empty());
        let tokens: &'static TokenTable = Box::leak(Box::new(result.tokens));
        // The Nth token with the given lexeme.
        let index_of = |lexeme: &str| -> u32 {
            let index = (0..)
                .map_while(|i| tokens.get(i).map(|_| i))
                .find(|&i| tokens.lexeme(source, i) == lexeme)
                .unwrap_or_else(|| panic!("token {lexeme} not found"));
            u32::try_from(index).expect("token index fits u32")
        };
        let sp = |id: u32| TokenSpan {
            id,
            range: dartq_source::SourceRange {
                offset: id * 2,
                length: 1,
            },
        };

        let mut flow = Flow::new(tokens, source);
        flow.begin_top_level_method(sp(0), None, None);
        // Interned in this order: int=0, void=1, dynamic=2, Never=3 — the
        // old code read tokens[0..=3] ("int", "x", ";", "void") instead of
        // each type name's own token.
        flow.handle_type(sp(index_of("int")), None);
        flow.handle_type(sp(index_of("void")), None);
        flow.handle_type(sp(index_of("dynamic")), None);
        flow.handle_type(sp(index_of("Never")), None);

        let events = flow.buffer.as_ref().unwrap();
        let bases: Vec<crate::ty::Base> = events
            .iter()
            .filter_map(|ev| match ev {
                Ev::Type { base, .. } => Some(*base),
                _ => None,
            })
            .collect();
        assert!(matches!(bases.first(), Some(crate::ty::Base::Name(0))));
        assert_eq!(bases.get(1), Some(&crate::ty::Base::Void));
        assert_eq!(bases.get(2), Some(&crate::ty::Base::Dynamic));
        assert_eq!(bases.get(3), Some(&crate::ty::Base::Never));
    }

    /// Drives the real parser over `source` with `self` as the listener
    /// and returns the collected flow facts — the `Flow`-as-listener
    /// wiring every type-variable event reaches (the semantic pipeline's
    /// `DiagnoseListener` forwards only a subset, so these cells live at
    /// the level where the recorder's own boundary table is populated).
    fn run_facts(source: &'static str) -> Vec<FlowFact> {
        let result = dartq_scanner::scan(
            source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            dartq_source::FileTable::new().intern(std::path::Path::new("test.dart")),
        );
        assert!(result.diagnostics.is_empty());
        let tokens: &'static TokenTable = Box::leak(Box::new(result.tokens));
        let mut flow = Flow::new(tokens, source);
        {
            let feats = dartq_language::FeatureSet::for_language_version(
                dartq_language::LanguageVersion::new(3, 15),
            );
            let mut parser = dartq_parser::Parser::new(
                source,
                tokens,
                dartq_parser::ParserFeatures::from_feature_set(feats),
                &mut flow,
            );
            parser.parse_unit();
        }
        flow.facts
    }

    /// #2297 `T extends St` (enum bound): the `TypeParameterTypeImpl` arm
    /// of `isAlwaysExhaustive` recurses into the declared bound — a fully
    /// covered switch leaves the body end unreachable, so no
    /// `BodyMayCompleteNormally` and the trailing statement stays live
    /// (there is none here). Oracle tp1: silent.
    #[test]
    fn type_param_enum_bound_fully_covered_reaches_no_body_end() {
        let facts = run_facts(
            "enum St { a, b }\nint f<T extends St>(T v) {\n  switch (v) {\n    case St.a: return 1;\n    case St.b: return 2;\n  }\n}\n",
        );
        assert!(
            !facts
                .iter()
                .any(|f| matches!(f, FlowFact::BodyMayCompleteNormally { .. })),
            "{facts:?}"
        );
        // Coverage is complete: no dead row either.
        assert!(
            !facts.iter().any(|f| matches!(f, FlowFact::DeadCode { .. })),
            "{facts:?}"
        );
    }

    /// The same bound, an all-returning switch, and a trailing
    /// statement: the exhaustive verdict deads what follows it (oracle
    /// tp4: `dead_code` at the trailing statement).
    #[test]
    fn type_param_enum_bound_deads_the_trailing_statement() {
        let facts = run_facts(
            "enum St { a, b }\nint f<T extends St>(T v) {\n  switch (v) {\n    case St.a: return 1;\n    case St.b: return 2;\n  }\n  return 3;\n}\n",
        );
        assert!(
            facts.iter().any(|f| matches!(f, FlowFact::DeadCode { .. })),
            "{facts:?}"
        );
    }

    /// A chained bound `T extends S, S extends St` recurses through the
    /// boundary table to the enum — same silent outcome (oracle
    /// nested_bound).
    #[test]
    fn type_param_chained_bound_reaches_no_body_end() {
        let facts = run_facts(
            "enum St { a, b }\nint f<T extends S, S extends St>(T v) {\n  switch (v) {\n    case St.a: return 1;\n    case St.b: return 2;\n  }\n}\n",
        );
        assert!(
            !facts
                .iter()
                .any(|f| matches!(f, FlowFact::BodyMayCompleteNormally { .. })),
            "{facts:?}"
        );
    }

    /// The conservative arm: an unbounded `T` carries the implicit
    /// `Object?` bound (subtype.dart:313), never always-exhaustive — a
    /// fully covered switch still leaves the end reachable (oracle
    /// implicit_bound keeps the unmatched path live).
    #[test]
    fn unbounded_type_param_keeps_the_body_end_reachable() {
        let facts = run_facts(
            "int f<T>(T v) {\n  switch (v) {\n    case 1: return 1;\n  }\n}\n",
        );
        assert!(
            facts
                .iter()
                .any(|f| matches!(f, FlowFact::BodyMayCompleteNormally { .. })),
            "{facts:?}"
        );
    }
}
