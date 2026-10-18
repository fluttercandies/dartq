//! The observable output of flow analysis: per-executable facts.

use dartq_parser::TokenSpan;
use dartq_source::SourceRange;

use crate::ty::Ty;

/// Which mounted getter validates a context usage (upstream
/// `associatedMountedGetter`): a `State.context`-shaped value is guarded
/// by `State.mounted`, a `BuildContext`-typed value by
/// `BuildContext.mounted`. A check guards a usage only when the origins
/// match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AsyncOrigin {
    /// A `BuildContext`-typed local/parameter.
    Context,
    /// The `State.context` getter shape (the unresolved bare `context`
    /// inside a class, or `.context` on `this`).
    State,
}

/// One flow-analysis fact about an executable body, in source order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlowFact {
    /// A statement (or statement-position block) is entered while the
    /// current control-flow path is unreachable — the `dead_code` family.
    /// `range` delimits the statement's first token.
    DeadCode {
        /// Span of the dead statement's first token.
        span: TokenSpan,
    },
    /// A read of a local variable (or parameter) that is not definitely
    /// assigned on this control-flow path. The consumer decides the
    /// diagnostic: `late` locals read while `definitely_unassigned` report
    /// `definitely_unassigned_late_local_variable`; non-late final locals
    /// report `read_potentially_unassigned_final`; non-late,
    /// non-nullable locals report
    /// `not_assigned_potentially_non_nullable_local_variable`
    /// (type_analyzer.dart `_checkReadOfNotAssignedLocalVariable`,
    /// :4990-5030).
    UnassignedRead {
        /// Span of the read's identifier token.
        span: TokenSpan,
        /// Whether the variable was declared `late`.
        is_late: bool,
        /// Whether the variable was declared `final` (or `const`).
        is_final: bool,
        /// The model's `unassigned` bit — upstream's
        /// `isDefinitelyUnassigned`/`isUnassigned` (flow_analysis.dart
        /// `:7221`): the variable is unassigned on EVERY incoming path.
        /// `false` marks the potentially-unassigned join face (assigned on
        /// some path, not others), which only the non-late read gates
        /// (`!isAssigned`, type_analyzer.dart:5010) consume. A
        /// `definitely_unassigned` read is also `!assigned` — the bits are
        /// exclusive (model.rs `PromotionModel` invariant).
        definitely_unassigned: bool,
        /// The variable's declared type, when the declaration carried one.
        declared_type: Option<dartq_source::SourceRange>,
    },
    /// A read of a local variable (or parameter) whose flow-promoted type
    /// differs from its declared type — the promotion facts a type-check
    /// consumer needs to see the promoted type at the read. Emitted only
    /// for real promotions (unpromoted and demoted reads carry no row).
    /// The row carries the promotion-chain head's written argument tree
    /// (`args`, an index into the recorder's arena read through
    /// [`crate::Flow::type_args_at`]; [`crate::ty::EMPTY_ARGS`] when
    /// argument-less) — the same pairing `ty` answers on the type-name
    /// side, so a consumer stages the promoted spelling exactly as
    /// written (#611 step 4; #1010 transport).
    PromotedRead {
        /// Span of the read's identifier token.
        span: TokenSpan,
        /// The promoted type on this control-flow path (chain head).
        ty: Ty,
        /// The chain head's written argument tree (arena index;
        /// [`crate::ty::EMPTY_ARGS`] when argument-less).
        args: u32,
    },
    /// The end of an executable body is reachable: the body can complete
    /// normally by falling off its end. The pure reachability face of the
    /// official `body_might_complete_normally` gate — the official first
    /// decision is `if (!flow.isReachable)` (`type_analyzer.dart:647-652`),
    /// before any return-type face. Every executable kind emits (method,
    /// top-level function, constructor, factory, local function,
    /// closure); the consumer owns the remaining gates (block-body shape,
    /// void/`dynamic`/`null` return faces, nullability) and the
    /// diagnostic's anchor remap (the official row reports at a block
    /// body's left bracket, not the end token).
    BodyMayCompleteNormally {
        /// Span of the executable's end token: the closing `}` of a block
        /// body, the end of an `=>` body's expression, or the `;` of an
        /// empty body.
        span: TokenSpan,
    },
    /// The flow type at one identifier read: the promoted type when a
    /// promotion stands, the declared type otherwise. Unlike
    /// [`FlowFact::PromotedRead`] every reachable read emits a row (a
    /// few hundred to a few thousand per file), so a consumer can ask
    /// for the static type at any read without re-running the fold.
    /// Payloads mirror [`FlowFact::PromotedRead`]; dead reads carry no
    /// row (their region is already the dead-code face), and a plain
    /// assignment's target identifier is a write, not a read — its row
    /// retracts with the target's read facts.
    ReadType {
        /// Span of the read's identifier token.
        span: TokenSpan,
        /// The flow type on this control-flow path (promoted or declared).
        ty: Ty,
        /// The type's written argument tree (arena index;
        /// [`crate::ty::EMPTY_ARGS`] when argument-less).
        args: u32,
    },
    /// The flow type at one condition operand's evaluation point: the
    /// promoted type when a promotion stands on the read variable, the
    /// declared type otherwise — the flow half of the official
    /// `checkForNonBoolCondition` query, whose nullable verdict reads the
    /// operand's flow-aware static type (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/error/bool_expression_verifier.dart:45-73,
    /// fired per condition operand at every `&&`/`||` node and at the
    /// if/if-element/while/do-while/C-`for`/assert/conditional condition
    /// heads). One row per boolean operator operand (the left at its own
    /// evaluation point, the right under the left's true branch) and one
    /// per statement-head condition that is a promotable reference read;
    /// other operand shapes (calls, property accesses, `is`-tests,
    /// assignments) carry no flow-visible type and emit no row — their
    /// static type is the type layer's question. A parenthesized operand
    /// keeps its read's anchor.
    ///
    /// Consumption seam (#1349): the diagnoser stages `(span.range, Ty)`
    /// rows exactly like [`FlowFact::ReadType`] (the payloads are the
    /// same shape), then answers a condition operand's flow type by the
    /// operand read's offset; `args` re-interns through
    /// [`crate::Flow::type_args_at`]. No row at an offset means the
    /// operand was not a promotable reference — the diagnoser falls back
    /// to its own resolved static type.
    ConditionType {
        /// Span of the operand's identifier read token.
        span: TokenSpan,
        /// The flow type at the operand's evaluation point.
        ty: Ty,
        /// The type's written argument tree (arena index;
        /// [`crate::ty::EMPTY_ARGS`] when argument-less).
        args: u32,
    },
    /// The switch scrutinee's flow type at one case clause's pattern
    /// match: the promoted type the pattern matched against (official
    /// `getMatchedValueType`, upstream Dart SDK source,
    /// pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart:8753-8760
    /// — the current promotion model's head for the matched value's
    /// promotion key, falling back to the matched value's declared type),
    /// queried by every pattern-matching callback the constant-pattern
    /// check rides (`constant_pattern_never_matches_value_type`). The
    /// clause's own pattern and guard have not run yet, but earlier
    /// alternatives' guard-false chains and every in-scope check before
    /// the switch are in. One row per case clause; a `default` clause
    /// matches no pattern and emits none. Requires a bare variable-read
    /// scrutinee (its promotion key); any other scrutinee shape emits no
    /// row and the diagnoser falls back to its own resolved type.
    ///
    /// Consumption seam (#1349): stage `(span.range, Ty)` rows like
    /// [`FlowFact::ConditionType`]; a statement clause anchors at its
    /// `case` keyword, a switch-expression alternative at its first
    /// token (the parser delivers no keyword face there).
    MatchedType {
        /// The clause anchor: a statement clause's `case` keyword, a
        /// switch-expression alternative's first token.
        span: TokenSpan,
        /// The scrutinee's flow type at the match.
        ty: Ty,
        /// The type's written argument tree (arena index;
        /// [`crate::ty::EMPTY_ARGS`] when argument-less).
        args: u32,
    },
    /// A property read whose flow state carries a non-null promotion
    /// (field promotion, #1409): the row stands at the read's expression
    /// start (the receiver's read token — for `c._b`, the `c` token; for
    /// `this._b`, the `this` token), emitted only while a promotion
    /// actually holds and the read is reachable — the property mirror of
    /// [`FlowFact::PromotedRead`]'s real-promotions bound.
    ///
    /// The flow layer tracks every promotable-shaped read (a plain
    /// identifier member on a bare local or `this` receiver); whether the
    /// member is a promotable FIELD (private, final, not external — the
    /// official `isPropertyPromotable` verdict, upstream Dart SDK source,
    /// pkg/_fe_analyzer_shared/lib/src/field_promotability.dart:70-76)
    /// under the consumer's field-promotion grant (the accessing
    /// library's settled language version ≥ 3.2 — `fieldPromotionEnabled`,
    /// `type_analyzer_options.dart:17`) is the consumer's eligibility
    /// gate: the row substitutes the non-null face of the consumer's own
    /// resolved static type.
    PromotedPropertyRead {
        /// The read's expression-start token (the receiver's read token).
        span: TokenSpan,
    },
    /// One write-classification row of a local variable (or parameter),
    /// aggregated from the official `AssignedVariables` anywhere sets:
    /// at most one row per (declaration, in-closure) pair per body —
    /// `in_closure: false` for a declaration with writes outside nested
    /// closures, `in_closure: true` for one written inside a nested
    /// closure. The write positions themselves are not tracked (the
    /// sets carry variable ids only); a declaration with no row is
    /// never written in its body.
    WriteSite {
        /// Span of the declaration's name token.
        decl_name_span: dartq_source::SourceRange,
        /// The writes sit inside a nested closure.
        in_closure: bool,
    },
    /// One BuildContext-typed usage evaluated behind at least one await
    /// crossing — the per-executable analysis core of
    /// `use_build_context_synchronously` (upstream Dart SDK source,
    /// `pkg/linter/lib/src/rules/use_build_context_synchronously.dart`),
    /// exposed for the lint's thin mount lane. Emit only this contract:
    ///
    /// * **Gap segments** — the body's await expressions segment the walk
    ///   into `gap + 1` segments; `gap` is the 0-based ordinal of the await
    ///   crossing the usage sits behind and `gap_at` is that `await`
    ///   keyword. A gap opens when the awaited operand's evaluation
    ///   completes (`AwaitEnd`); the operand itself is pre-gap.
    /// * **Guard-validity rule** — a mounted check guards a usage when
    ///   control can only reach the usage through a branch where the
    ///   check held. Check forms: `x.mounted` (positive; guards then
    ///   branches and for-statement conditions), `!x.mounted` (negative;
    ///   guards else branches), `== true/false` / `!=` bool-literal
    ///   flips, `&&`/`||` polarity tables, and the exit forms
    ///   (`if (!x.mounted) return;` — a terminating branch under a
    ///   negative check guards the code after the if). Guards are
    ///   side-matched: a check on a BuildContext-typed receiver only
    ///   guards context-origin usages; one on `this`/`State`-shaped
    ///   receivers only guards State-origin usages. A guard valid at the
    ///   usage sets `guarded`; a new await always re-opens the gap.
    /// * **Exemption shapes captured** — `context.mounted` itself is a
    ///   check, never a usage; uses with no await behind them emit no
    ///   row; closures are opaque (their internal awaits never gap the
    ///   enclosing body and their internal usages emit no rows — the
    ///   protected-callback argument face is NOT modeled here); a mounted
    ///   check in a `while` condition guards nothing (parity with the
    ///   official rule's documented while-condition limitation); a
    ///   mounted check in a C-style `for` condition guards that loop's
    ///   body.
    /// * **`unrelated_mounted`** — a mounted-name check on a receiver
    ///   whose origin does not match the usage's (or on an unresolvable
    ///   receiver) was seen between the verdict-forming event and the
    ///   usage: upstream `hasUnrelatedMountedCheck`, which selects the
    ///   `wrong_mounted` diagnostic code over `async_use` when
    ///   `guarded` is false.
    /// * **Resolution rule (spelling gate)** — a value is
    ///   BuildContext-typed when its declared type's interned name is
    ///   `BuildContext` (targets/accesses accept `BuildContext?`,
    ///   arguments require the non-nullable declared form, matching the
    ///   upstream `skipNullable` split), or when it is the unresolved
    ///   bare name `context` or a `.context` access on `this` (the
    ///   `State.context` getter shape; origin [`AsyncOrigin::State`]).
    ///   A same-file declaration shadowing `BuildContext` must be
    ///   excluded by the consumer (the official element-identity
    ///   posture; the lints crate's same-file-declaration face covers
    ///   it — a shadowing declaration silences the whole unit's rows).
    AsyncContextUse {
        /// The usage expression's first token: an invocation target, an
        /// argument expression, or the receiver read of a getter access.
        span: TokenSpan,
        /// The guard origin the usage was resolved under — which
        /// mounted getter validates it (upstream
        /// `associatedMountedGetter`).
        origin: AsyncOrigin,
        /// 0-based ordinal of the await crossing the usage sits behind.
        gap: u32,
        /// The `await` keyword of that crossing.
        gap_at: TokenSpan,
        /// A valid mounted guard for this usage's origin intervened
        /// since the gap crossing; `false` is the lint-firing shape.
        guarded: bool,
        /// A mismatched-origin mounted check was seen between the
        /// verdict-forming event and the usage (the `wrong_mounted`
        /// face). Only meaningful when `guarded` is false.
        unrelated_mounted: bool,
    },
    /// A reachable read of a local variable (or parameter) whose
    /// non-promotion history is non-empty — upstream `whyNotPromoted`'s
    /// variable arm (`flow_analysis.dart:8816-8854`): at least one
    /// recorded demotion stands between the variable's past promotion and
    /// this read. Emitted beside [`FlowFact::ReadType`] at every reachable
    /// read of a variable carrying history, so the consumer answers "why
    /// didn't this read promote" without re-walking the flow.
    ///
    /// `entries` is an arena index read through
    /// [`crate::Flow::non_promotion_at`]: ordered `(ty, args, cause)` rows
    /// — the official map's `type → reason` pairs, most recent demotion
    /// first, first-wins per type, filtered by the official
    /// `!isSubtypeOf(currentType, nonPromotedType)` rule at name level
    /// (`flow_analysis.dart:8844-8850`). `current` is the flow type at the
    /// read (the official `currentType`) — a consumer with a resolved type
    /// back-end re-filters the same rows against it.
    WhyNotPromoted {
        /// Span of the read's identifier token (the [`FlowFact::ReadType`]
        /// key for the same read).
        span: TokenSpan,
        /// The flow type at the read (promoted head or declared).
        current: Ty,
        /// `current`'s written-argument tree (arena index;
        /// [`crate::ty::EMPTY_ARGS`] when argument-less).
        current_args: u32,
        /// Arena index into [`crate::Flow::non_promotion_at`] (0 when the
        /// filtered map came out empty — a consumer renders nothing,
        /// matching upstream's empty-map path).
        entries: u32,
    },
    /// A reachable promotable-shaped property read that did NOT promote
    /// (the property mirror of [`FlowFact::WhyNotPromoted`]) — upstream's
    /// `_PropertyReference` arm of `_getNonPromotionReasons`
    /// (`flow_analysis.dart:8766-8815`): the slot's would-be promoted
    /// types stand as evidence, so a consumer that resolves the member's
    /// non-promotability can name what failed. Emitted when the read is
    /// reachable, the promotion does not stand, and the slot carries
    /// recorded evidence — the official `allPreviouslyPromotedTypes !=
    /// null` gate: a member never tested or promoted emits no row.
    ///
    /// Whether the member is inherently non-promotable (a getter, a public
    /// or external or non-final field) or fails for a non-inherent reason
    /// (a library-level name conflict, or field promotion disabled) is the
    /// consumer's element-layer verdict — upstream itself computes that
    /// answer at query time from `whyPropertyIsNotPromotable`
    /// (`flow_analysis.dart:8769-8772`), which is element data the flow
    /// layer deliberately does not carry. `entries` indexes the same
    /// arena: the would-be `(ty, args)` rows, each tagged
    /// [`NonPromotionCause::PropertyEvidence`].
    NonPromotedPropertyRead {
        /// The member read token (the [`FlowFact::PromotedPropertyRead`]
        /// anchor for the same read).
        span: TokenSpan,
        /// The member's interned name id — recovered through
        /// [`crate::Flow::name_lexeme`], the `'<prop>'` of the official
        /// context text.
        name: u32,
        /// The receiver the property was read on: `this` (explicit or
        /// implicit — the official `ThisPropertyTarget`) or a local
        /// variable (its declaration anchor, the consumer's identity key).
        receiver: PropReceiver,
        /// Arena index into [`crate::Flow::non_promotion_at`] (the
        /// would-be promoted types — the official map's keys).
        entries: u32,
    },
    /// A reachable `this` expression read whose type test stands
    /// unrepresentable — upstream `ThisNotPromoted`
    /// (`flow_analysis.dart:5962`): `this` passed a type test
    /// (`this is T`, `this as T`, `this != null`) but promotion of `this`
    /// is unsupported, so the read reports why it could not take the
    /// promoted type. `entries` indexes the same arena: one row per
    /// standing promoted type, each [`NonPromotionCause::ThisNotPromoted`].
    ///
    /// Taxonomy-complete, unemitted in this layer: the flow engine tracks
    /// no `this` type-test face (this-promotion is upstream's
    /// experimental-off flag, so no oracle diff hangs on it) — the variant
    /// exists so consumers can match the full upstream reason set without
    /// a layer change when that face lands.
    ThisNotPromoted {
        /// The `this` token.
        span: TokenSpan,
        /// Arena index into [`crate::Flow::non_promotion_at`].
        entries: u32,
    },
}

/// What kind of receiver a [`FlowFact::NonPromotedPropertyRead`] row's
/// property was read on — the official `PropertyTarget` distinction
/// (`flow_analysis.dart:5524-5529`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropReceiver {
    /// `this` — explicit (`this._b`) or implicit (`_b`), the official
    /// `ThisPropertyTarget`. `this` never rebinds, so the identity is
    /// per-member-name, not per-value.
    This,
    /// A local variable or parameter: the declaration's name-token span —
    /// the consumer's stable identity key across fact rows (the same
    /// anchor [`FlowFact::WriteSite::decl_name_span`] and
    /// [`FlowFact::UnassignedRead::declared_type`] use).
    Local(SourceRange),
}

/// Why one promoted type failed to hold at a read — the cause half of a
/// [`crate::Flow::non_promotion_at`] row: the recorded half of upstream's
/// `NonPromotionReason` set (`flow_analysis.dart:4552`) — the write and
/// suspension links the engine constructs eagerly (`flow_analysis.dart:9541`,
/// `:7945`) plus the flat-slot property evidence and the `this` tag. The
/// resolved five-variant shape the context producer renders from is
/// [`NonPromotionReason`]. The property family stays undecided at this
/// layer by design: upstream itself distinguishes inherent from
/// non-inherent only by asking the element layer
/// (`whyPropertyIsNotPromotable`), so [`PropReceiver`]/`name` carry the
/// identity and the consumer resolves the verdict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NonPromotionCause {
    /// `DemoteViaExplicitWrite` — an explicit assignment dropped the
    /// promotion. `span` is the write expression's own range (upstream's
    /// `node` anchor for the `"…due to an assignment"` context row).
    ExplicitWrite { span: SourceRange },
    /// `DemoteViaSuspension` — an `await`/`yield` inside a local function
    /// dropped the promotion. `span` is the suspension node's range (the
    /// `"…due to an 'await' or 'yield'"` row's anchor).
    Suspension { span: SourceRange },
    /// A property slot's cleared promoted type (upstream's
    /// `allPreviouslyPromotedTypes` evidence): the promotion that WOULD
    /// have stood had the member been promotable. The `PropertyNotPromoted`
    /// reason — inherent vs non-inherent — is resolved at query time
    /// through [`crate::Flow::non_promotion_reasons`] (see
    /// [`FlowFact::NonPromotedPropertyRead`]).
    PropertyEvidence,
    /// `ThisNotPromoted` — `this` can't be promoted (no payload upstream;
    /// the context row anchors at the error entity itself).
    ThisNotPromoted,
}

/// One entry of a [`crate::Flow::non_promotion_at`] row: the
/// non-promoted type (the official map's key, `SharedTypeView`) beside
/// its cause — ordered most recent demotion first, deduplicated per type
/// by the official first-wins rule (`flow_analysis.dart:8844-8850`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NonPromotionEntry {
    /// The type the promotion failed to hold (the official map key; the
    /// analyzer skips potentially-nullable keys before taking the first
    /// entry — `type_analyzer.dart:783-791`).
    pub ty: Ty,
    /// `ty`'s written-argument tree (arena index;
    /// [`crate::ty::EMPTY_ARGS`] when argument-less).
    pub args: u32,
    /// Why the type failed to hold.
    pub cause: NonPromotionCause,
}

/// Why a property member is inherently non-promotable — the mirror of
/// upstream's `PropertyNonPromotabilityReason` (upstream Dart SDK source,
/// pkg/_fe_analyzer_shared/lib/src/flow_analysis/
/// flow_analysis_operations.dart:127-140). The diagnoser's element layer
/// answers this per member (the official `whyPropertyIsNotPromotable`
/// operation); a public member name answers [`Self::NotPrivate`]
/// statically (`flow_analysis.dart:8769-8772`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropertyNonPromotability {
    /// `isNotField` — the property is a getter or a method tear-off, not
    /// a field.
    NotField,
    /// `isNotPrivate` — the property's name is public.
    NotPrivate,
    /// `isExternal` — the property is an external field.
    External,
    /// `isNotFinal` — the property is a non-final field.
    NotFinal,
}

/// The five-variant reason a promotion failed to hold at a read — the
/// resolved mirror of upstream's `NonPromotionReason` hierarchy (upstream
/// Dart SDK source,
/// pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart:4552;
/// variants at `:57`, `:90`, `:5453`, `:5503`, `:5962`), shaped for the
/// diagnoser's `computeWhyNotPromotedMessages` producer
/// (`type_analyzer.dart:772`; the visitor mapping,
/// `type_analyzer.dart:5970`). Resolve a read's fact row through
/// [`crate::Flow::non_promotion_reasons`] and render each reason from the
/// table below. The recorded [`NonPromotionCause`] links are the
/// engine-side half; this type is the query-time construction — upstream
/// builds the property and `this` variants lazily in
/// `_getNonPromotionReasons` (`flow_analysis.dart:8762-8857`), so the
/// property family's inherent-vs-conflict split arrives as the caller's
/// element-verdict input rather than recorded state.
///
/// | Variant | Upstream | Payload | Context-row anchor |
/// | --- | --- | --- | --- |
/// | [`NonPromotionReason::ExplicitWrite`] | `DemoteViaExplicitWrite` | the write expression's span | the demoting write node — "Variable '<name>' could not be promoted due to an assignment" (`<name>` reads the fact row's own read span) |
/// | [`NonPromotionReason::Suspension`] | `DemoteViaSuspension` | the suspension node's span | the await/yield node — "…due to an 'await' or 'yield'" |
/// | [`NonPromotionReason::PropertyInherent`] | `PropertyNotPromotedForInherentReason` | the inherent `why`; the `'<prop>'` spelling and declaration anchor come from the fact row's `name`/`receiver` | the property's declaration `nameOffset`, cross-file |
/// | [`NonPromotionReason::PropertyConflict`] | `PropertyNotPromotedForNonInherentReason` | the fact row's `name`/`receiver` — the conflicting members are the consumer's element layer's answer | each conflicting element's `nameOffset`, cross-file |
/// | [`NonPromotionReason::This`] | `ThisNotPromoted` | none | the error entity itself (the diagnostic's own span) |
///
/// Upstream's `fieldPromotionEnabled` payload (the optional "field
/// promotion is only available in Dart 3.2 and above." row) is the
/// diagnoser's language-version gate — the flow layer carries no
/// language-version state — so neither property variant carries it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NonPromotionReason {
    /// `DemoteViaExplicitWrite`: an explicit assignment dropped the
    /// promotion.
    ExplicitWrite {
        /// The write expression's own range (the context row's anchor).
        span: SourceRange,
    },
    /// `DemoteViaSuspension`: an `await`/`yield` inside a local function
    /// dropped the promotion.
    Suspension {
        /// The suspension node's own range (the context row's anchor).
        span: SourceRange,
    },
    /// `PropertyNotPromotedForInherentReason`: the member is inherently
    /// non-promotable — `why` names the inherent face.
    PropertyInherent {
        /// Why the member is not promotable.
        why: PropertyNonPromotability,
    },
    /// `PropertyNotPromotedForNonInherentReason`: the member is promotable
    /// in itself but the promotion stands unavailable — a library-level
    /// conflicting declaration, field promotion disabled, or both (the
    /// client picks the reported reason; upstream's class doc,
    /// `flow_analysis.dart:5490-5500`).
    PropertyConflict,
    /// `ThisNotPromoted`: `this` can't be promoted (no payload upstream;
    /// the context row anchors at the error entity itself).
    This,
}

impl NonPromotionReason {
    /// The resolved reason for one recorded entry: the write, suspension,
    /// and `this` links pass through; a property-evidence link resolves
    /// through the caller's element verdict (`Some` = inherent, `None` =
    /// non-inherent conflict).
    pub(crate) fn resolve(
        cause: NonPromotionCause,
        property: Option<PropertyNonPromotability>,
    ) -> Self {
        match cause {
            NonPromotionCause::ExplicitWrite { span } => Self::ExplicitWrite { span },
            NonPromotionCause::Suspension { span } => Self::Suspension { span },
            NonPromotionCause::PropertyEvidence => match property {
                Some(why) => Self::PropertyInherent { why },
                None => Self::PropertyConflict,
            },
            NonPromotionCause::ThisNotPromoted => Self::This,
        }
    }
}
