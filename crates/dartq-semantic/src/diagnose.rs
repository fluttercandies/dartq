//! The M1 semantic diagnosis engine: the frozen `undefined_*` /
//! `duplicate_definition` family (batch 1, #120) plus the
//! `wrong_number_of_*` family and the static-nullability four (batch 2,
//! #121) — a pass over the parser's event stream that resolves every
//! reference a unit makes against the #117 lookup surface and the #119
//! element model, and emits the implemented frozen rows as [`Diagnostic`]s.
//!
//! # Architecture
//!
//! One parse with a reference-collecting listener (declaration capture stays
//! in [`crate::collector::collect`] — the model and the diagnostic surface
//! share the parser event stream, not the listener). The listener keeps an
//! expression row stack (the same shape the AST builder reconstructs from
//! these events): bare identifiers, member accesses merged at the dot event,
//! typed receivers (`this`/`super`/container names/declaration-typed names),
//! and per-row access intent (read / write / invoke / operator) finalized by
//! the send/assignment/operator events. Rows snapshot their resolution
//! verdict and declared type at push time (the frames tear down before
//! pass 2) and resolve in source order. Declaration-surface diagnostics —
//! `duplicate_definition` and the arity log — fold into the report from the
//! collector's capture-time logs.
//!
//! # Batch 2: arity and static nullability (#121)
//!
//! The type-argument family keys off `begin/end_type_arguments`, with the
//! code chosen by the use-site shape (the pinned oracle registry,
//! `.third/dart-sdk` `diagnostic.g.dart`):
//! a pending *type head* (a `TypeReference` identifier that has not seen
//! its `handle_type` yet) makes the group an annotation or a
//! creation type reference → the base code; a head consumed inside an
//! enum constant → `_enum`; an expression head under `new`/`const`/
//! implicit-creation position → base for a resolved container, `_constructor`
//! for a named constructor segment; any other expression head defers to the
//! send or to the type-argument application event, both of which report
//! `_element` for a resolved executable — the message names the
//! executable's oracle kind word, `function` or `method`
//! (`type_instantiation_target.dart`,
//! `InvocationTargetExecutableElement`) — or `_extension` for extension
//! overrides. The `_function` row (a function-typed *expression*
//! instantiation) displays the function type, a shape M1 cannot render, so
//! it has no emit site. Only resolvable *functions and methods* produce
//! counts — external (`dart:`) and unresolved heads suppress, and
//! getter-shaped callees (whose oracle kind word M1 does not model)
//! suppress too: the under-report direction.
//!
//! The static-nullability checks decide on declared types and literal
//! types (types-nullability-spec §2–§3): the engine tracks the declared
//! type of locals, formals, fields, top-level variables, and return types
//! (armed by `handle_type`/`handle_void_keyword`, disarmed by
//! `handle_no_type` — every declaration head runs one `parse_type`, so the
//! arm cannot leak), and types expressions minimally: the `null` literal is
//! `Null`, the string/int/double/bool literals carry their final primitives,
//! a name with a captured declared type is that type, everything else is
//! `Unknown` — the oracle's `InvalidType`, which suppresses every question
//! it touches. The decidable set is the nullability decisions
//! (`null`/`T?` into a non-nullable target) plus the final-primitive pairs
//! ([`FINAL_PRIMITIVES`]), and `?.`/`?[`/`?..` on a strictly non-nullable
//! receiver (`invalid_null_aware_operator`, a `STATIC_WARNING`).
//! Assignment/initializer/argument/return checks apply only to single-row
//! expressions (one pushed row) so compound operands never lend their type
//! to the whole expression.
//!
//! # The inheritance-override family (#453)
//!
//! Name-level `invalid_override` (`inheritance_override.dart:498-546`
//! `_checkDeclaredMember`): each instance-member declaration of a class,
//! mixin, or enum is compared against the nearest same-named non-static
//! member of every direct super-face (the supertype, the `with` mixins in
//! application order, the `implements` interfaces, and a mixin's `on`
//! constraints). The verdict chain is kind equality, positional arity,
//! named-parameter set, then type comparability — a kind mismatch skips
//! (other checks own it), an unresolvable or cyclic face chain suppresses,
//! and [`Ty::assignable_from`]'s `None` (distinct named types, unknowns)
//! suppresses: two types a subclass relation could still relate never
//! report. Emissions run from the parse listener off the completed element
//! model, anchored at the member's name span, once per failing face.
//! Signature texts in the message render the name-level shape we have
//! (`void Function(int)`-form); the oracle's instantiated function types
//! are a documented text divergence. Generic-method type-parameter
//! compatibility and getter/setter cross-shape conflicts under-report.
//!
//! # The abstract-completeness and interface-conflict families (#498)
//!
//! Both ride the #453 face machinery at container granularity, mounted at
//! the container's name event (`inheritance_override.dart:412-486` and
//! `:160-207`). *Completeness* (the five-tier
//! `non_abstract_class_inherits_abstract_member`): a concrete class or enum
//! reports every interface name without a concrete implementation, the
//! implemented sources being own members, the `with` mixins, and the super
//! chain — `implements` and `on` faces contribute interface members only
//! (`_addImplemented`). The exemptions mirror the official walk: `Object`
//! members, an enum's `values`/`values=`, own-declared names (the
//! `concrete_class_with_abstract_member` code owns those — under-report
//! here), names a concrete direct superclass carries, cross-library private
//! names (`isAccessibleFor`), and a concrete `noSuchMethod` anywhere on the
//! implemented sources (the forwarder gate). A name the candidate merge
//! dropped (a conflict) is absent from the interface — never reported
//! missing. Abstract members are captured at the declaration
//! (body-less/`abstract`); an abstract field's non-final shape misses both
//! its getter and its setter. *Conflicts* (`inconsistent_inheritance`
//! and `_getter_and_method`): candidates accumulate per name in the
//! official face order with a `with` mixin's own member replacing the
//! accumulated list; a getter/method kind mix is unresolvable and reports
//! even under a class-local declaration (oracle-verified), a same-kind
//! merge without a valid override reports unless the class declares the
//! name. A mixin declaration participates in the conflicts only. The
//! conflict rows anchor at the class name (the oracle moves the
//! getter/method row onto a local declaration when one exists — a span
//! divergence, metric-neutral on the (code, file) gate). Every
//! unresolvable face chain suppresses its container's whole check.
//!
//! # The flow-analysis mount (#449)
//!
//! Every flow-relevant listener event double-dispatches to a per-unit
//! [`dartq_flow::Flow`] recorder alongside the reference work — no second
//! pass, no engine state: the recorder is constructed in [`diagnose`],
//! lives only for the parse, and its [`dartq_flow::FlowFact`]s fold into
//! the report at teardown. The first consumed family is
//! [`SemanticCode::DeadCode`]: one static warning per statement entered
//! while unreachable, at the statement's anchor. The fact layer reports
//! per statement while the oracle folds each dead region into one
//! diagnostic spanning to the region's end (`dead_code_verifier.dart`
//! `_firstDead`) — a documented divergence, as are the declaration
//! anchors (variable names, not the declaration head). The
//! `UnassignedRead` facts are produced but not yet consumed (the
//! `not_assigned_potentially_non_nullable_local_variable` milestone).

//! # The frozen-family emit matrix (batch 1 #120, batch 2 #197)
//!
//! The batch-1 core (bare names, type positions, prefixed invocations,
//! `duplicate_definition`) plus the batch-2 member family cover the frozen
//! set's live triggers. Every member emission keys off the access intent and
//! the receiver domain, mirroring the oracle's single-point dispatch
//! (`.third/dart-sdk` `lookup_failure_reporter.dart`, domains × read/write ×
//! read-syntax):
//!
//! * instance receivers (`c.m` on a `C c`, `this.m`) report
//!   `undefined_method` / `undefined_getter` / `undefined_setter`
//!   (`InstanceLookupDomain`); a write survives when any same-named
//!   declaration exists (the oracle's assignment-to-* family) or a setter
//!   spelling (`name=`) defines it;
//! * the static face (`C.m`, `E.x`, `Ext.g`) reports the same interface
//!   trio — the type-literal invocation shape shares the `undefined_method`
//!   registry name and message — plus `undefined_enum_constant` on enum
//!   reads, and the extension trio on the extension face
//!   (`StaticLookupDomain`);
//! * `super.member` reports `undefined_super_member` with the per-shape
//!   message (`SuperLookupDomain`; one analyzerCode, four texts);
//! * prefixed reads and writes report `undefined_prefixed_name`
//!   (`PrefixedLookupDomain`);
//! * a bare invocation miss inside a class or enum instance member reports
//!   `undefined_method` naming `this`'s type (`UnqualifiedLookupDomain`);
//! * a named argument label on a resolved callee reports
//!   `undefined_named_parameter` (`type_analyzer.dart:5820` — an unresolved
//!   callee stays silent, its own invocation code reports);
//! * `await` in a sync body reports `undefined_identifier_await`;
//! * user-definable operators report `undefined_operator` at the operator
//!   token (index accesses at the bracket pair) when the receiver's
//!   declared type — direct (`c + d`, `c[i]`) or a member result
//!   (`c.j + 1`, `c.j[i] = v`) — genuinely lacks the read/write spelling.
//!
//! Under-reported by adjudication (each emits no code): operator intents on
//! static faces and prefix receivers, index chains through unknown
//! receivers (`f()[i]`), and `this[i]`-style accesses inside extension and
//! mixin bodies, whose `on` faces the model cannot see.
//!
//! # The names table seam (index-design-spec §8)
//!
//! Unresolved names are a **byproduct of lookup**, written into
//! [`FileReport::names`] — the same rows the §8 index names table wants
//! (`(name, kind, range, qualified)`), kept here until #122 mounts
//! `dartq-index` and the table moves behind it. Resolved references are not
//! recorded: they are the reference index's rows, not the recovery surface's.
//!
//! # The undefined guard (analyzer `shouldIgnoreUndefined`)
//!
//! `element.dart:8217-8250`: a name referenced through an import whose target
//! did not resolve is suppressed — but the filter face is show-list-only. The
//! mirror reads the graph's import edges (`target: None`) and suppresses a
//! name listed in a broken import's `show` clause, or a prefixed name whose
//! broken import has no `show` clause; a bare miss under an unfiltered or
//! hide-only broken import still reports ([`UndefinedGuard`]). The
//! `_$`-generated-part clause needs the part tree; the parser has no part
//! events yet, so it is deferred with the directive milestone (the clause
//! only ever suppresses). Harnesses model core-library names through
//! concrete declarations or shown broken edges.
//!
//! # False-positive defenses (each mirrors a verified analyzer behavior)
//!
//! * Locals, parameters, catch/pattern variables, and type parameters are
//!   scope-tracked in this pass (the model does not capture them) — a body
//!   reference to a local never reaches the library lookup.
//! * `dynamic` is a language builtin, not an import — never `undefined_class`.
//! * `await` reports `undefined_identifier_await` instead of the family's
//!   own code at both oracle sites: a bare read inside an executable
//!   (`property_element_resolver.dart:2143`) and the unresolved simple
//!   type name of a declaration parse (`named_type_resolver.dart:814-818`
//!   — `await x;` in a sync body keeps the declaration parse).
//! * `is`/`as` type operands and type arguments carry other oracle codes
//!   (`type_test_with_undefined_name`, `cast_to_non_type`,
//!   `non_type_as_type_argument`) — none frozen, so this pass under-reports
//!   them rather than emitting a wrong code.
//! * Member walks distinguish a cleanly walked chain (member genuinely
//!   absent) from a broken one (an unresolvable superclass — external type or
//!   mixin `on` constraint) and record only the former; enum synthesized
//!   members (`values`/`index`/`name`) and `Object` members are defined on
//!   every receiver shape the walk can reach.
//! * Member checks need a *declaration* receiver type (`this`, `super`, an
//!   extension or class name, or a name whose declared type is an indexed
//!   container — M1 deliberately under-reports inferred receivers,
//!   index-design-spec §12.2). A named constructor is invisible to the model,
//!   so `@Class.member` second-part checks are suppressed for classes
//!   (extensions have no constructors and stay checked).
//! * Constructor invocations are outside the model: a class-name callee is
//!   defined without inspection, and `new`/`const`-position callees resolve
//!   as type references (the oracle's `undefined_class`), never invocations —
//!   their constructor-family codes stay deferred. Implicit creations
//!   (`C<T>(...)` without `new`) bracket the same suppression window.
//! * Locals leak only where the parser offers no frame pairing (switch-case
//!   bodies share the switch block's frame) — a leak can only mask a miss
//!   (under-report), never fabricate one.
//!
//! # The #350 gate closure (the remaining frozen rows)
//!
//! * Assignability trio: literals carry their final-primitive types and
//!   [`Engine::assignable`] extends [`Ty::assignable_from`] with the
//!   final-primitive table (a value of one final primitive never fits
//!   another; `int`/`double` fit `num`; `int`→`double` is the oracle's
//!   literal conversion — `analyzeIntLiteral`'s `convertToDouble`, decided
//!   on the literal row, never on the value).
//! * `undefined_annotation`: a fully missing `MetadataReference` name,
//!   anchored over the `@` (the 3.13.1 caliber — removed upstream in 3.14).
//! * `undefined_constructor_in_initializer`: `super.named()` inside a
//!   constructor-initializer list checks the superclass's constructor face
//!   (captured per container: named selectors + the unnamed flag) and folds
//!   the row before pass 2's super-member dispatch can mis-report it.
//! * `undefined_enum_constructor`: an enum constant's constructor selector
//!   checks the enum's constructor face.
//! * The extension-override face: a container-callee send (`E('x')`,
//!   `C()`) folds into a [`Receiver::Created`] row, so member and operator
//!   accesses resolve against the instance-shaped face — extensions report
//!   their dedicated trio and operator row, class creations walk the
//!   superclass chain with the `Object` members defined.
//! * The external-primitive member face: a receiver type that resolves to
//!   no container but names a dart:core primitive checks
//!   [`CORE_TYPE_MEMBERS`] instead of suppressing.
//! * The `_function` tear-off row renders the function type from the
//!   captured member signature (type-parameter names joined the model);
//!   `C<...>.named`-shaped constructor references fall back to the base
//!   type-argument code. Both fire from the type-argument application
//!   event — the current parser consumes `id.id<...>` as a type head
//!   (#351's gap), so the emission activates when that parse lands.
//!
//! # Still deferred
//!
//! `undefined_referenced_parameter` (`@UseResult(unless:)` check,
//! `annotation_verifier.dart:519-541`), `undefined_hidden_name` /
//! `undefined_shown_name` (directives are consumed diet-style and the
//! combinator model carries clause names without source spans — both arrive
//! with the directive milestone) and the unnamed-selector message variants of
//! the two constructor rows. `wrong_number_of_parameters_for_setter` is emitted here although
//! the oracle fires it from the parser: dartq's parser does not emit the code
//! yet, and when it does, the collector-side emission must be removed or the
//! code double-reports. Every deferral under-reports.

use std::cell::RefCell;
use std::rc::Rc;

use dartq_diagnostics::{Diagnostic, Severity};
use dartq_flow::{Flow, FlowFact};
use dartq_parser::{
    AssertKind, BlockKind, ConstructorReferenceContext, DeclarationKind, ExperimentalFlag,
    FormalParameterKind, IdentifierContext, Message, Parser, ParserFeatures, ParserListener,
    TokenSpan as Token,
};
use dartq_resolution::{ImportEdge, LibraryGraph, LibraryId, NamespaceEntry, Namespaces};
use dartq_scanner::TokenTable;
use dartq_source::{FileId, SourceRange, StringId, StringTable};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::codes::{ExecKind, SemanticCode, SuperShape};
use crate::const_eval::{
    BinOp, ConstCapture, ConstFault, ConstLookup, ConstantValue, IdentVerdict, UnOp, Verdict, eval,
};
use crate::library::{ElementIndex, ScopedHit, lookup_scoped, owner_library, resolve_container};
use crate::model::{
    ArityViolation, Container, ContainerKind, ExtensionFace, FileElements, Formals, MemberEntry,
    MemberKind, MicroKind, TopDecl, Ty,
};
use dartq_types::{
    Case, Exhaustiveness, ExhaustivenessWorld, Key, Single, Space, Witness as EngineWitness,
    compute,
};

/// Members of `dart:core` `Object`: defined on every class-shaped receiver
/// the M1 walk can reach (the superclass chain ends at an unindexed root).
const OBJECT_MEMBERS: [&str; 5] = ["==", "hashCode", "toString", "noSuchMethod", "runtimeType"];

/// Synthesized enum members absent from the model; suppressed on enum
/// receivers so the member walk cannot false-positive on them.
const ENUM_SYNTH_MEMBERS: [&str; 3] = ["values", "index", "name"];

/// Builtin type names: always defined, independent of any import.
const BUILTIN_TYPES: [&str; 1] = ["dynamic"];

/// The dart:core top-level annotation names (`sdk/lib/core/annotations.dart`:
/// `Deprecated` :63, `deprecated` :212, `override` :262, `pragma` :297) —
/// dart:core is implicitly imported into every library, so an annotation of
/// one of them is defined even where the SDK summary faces carry no
/// top-level constants. The oracle resolves `@override` inside its own
/// hosted sources (meta 1.19.0 `meta_meta.dart:202`) and reports nothing.
const CORE_TOP_LEVEL_ANNOTATIONS: [&str; 4] = ["Deprecated", "deprecated", "override", "pragma"];

/// The dart:core primitive types whose member face the engine answers
/// directly: their declarations are external (the library graph carries
/// their top-level names only), so a member walk chain-breaks — but the
/// oracle still reports a genuinely absent member on them. The listed
/// members (instance getters/methods; setters none of them has) keep the
/// check on the oracle's side: a name on the list is defined, everything
/// else reports. Derived from the reference SDK (`sdk/lib/core/*.dart`).
const CORE_TYPE_MEMBERS: [(&str, &[&str]); 5] = [
    (
        "int",
        &[
            "bitLength",
            "isEven",
            "isOdd",
            "sign",
            "isFinite",
            "isInfinite",
            "isNaN",
            "isNegative",
            "abs",
            "gcd",
            "modInverse",
            "modPow",
            "remainder",
            "round",
            "roundToDouble",
            "floor",
            "floorToDouble",
            "ceil",
            "ceilToDouble",
            "truncate",
            "truncateToDouble",
            "toDouble",
            "toInt",
            "toStringAsFixed",
            "toStringAsExponential",
            "toStringAsPrecision",
            "toRadixString",
            "toSigned",
            "toUnsigned",
            "clamp",
            "compareTo",
            "toString",
            "hashCode",
            "runtimeType",
            "noSuchMethod",
        ],
    ),
    (
        "double",
        &[
            "isFinite",
            "isInfinite",
            "isNaN",
            "isNegative",
            "sign",
            "abs",
            "round",
            "roundToDouble",
            "floor",
            "floorToDouble",
            "ceil",
            "ceilToDouble",
            "truncate",
            "truncateToDouble",
            "toInt",
            "toDouble",
            "toStringAsFixed",
            "toStringAsExponential",
            "toStringAsPrecision",
            "clamp",
            "compareTo",
            "toString",
            "hashCode",
            "runtimeType",
            "noSuchMethod",
        ],
    ),
    (
        "num",
        &[
            "isFinite",
            "isInfinite",
            "isNaN",
            "isNegative",
            "sign",
            "abs",
            "remainder",
            "round",
            "roundToDouble",
            "floor",
            "floorToDouble",
            "ceil",
            "ceilToDouble",
            "truncate",
            "truncateToDouble",
            "toInt",
            "toDouble",
            "toStringAsFixed",
            "toStringAsExponential",
            "toStringAsPrecision",
            "clamp",
            "compareTo",
            "toString",
            "hashCode",
            "runtimeType",
            "noSuchMethod",
        ],
    ),
    (
        "String",
        &[
            "codeUnits",
            "isEmpty",
            "isNotEmpty",
            "length",
            "runes",
            "allMatches",
            "codeUnitAt",
            "compareTo",
            "contains",
            "endsWith",
            "indexOf",
            "lastIndexOf",
            "matchAsPrefix",
            "padLeft",
            "padRight",
            "replaceAll",
            "replaceFirst",
            "replaceFirstMapped",
            "replaceAllMapped",
            "split",
            "splitMapJoin",
            "startsWith",
            "substring",
            "toLowerCase",
            "toUpperCase",
            "trim",
            "trimLeft",
            "trimRight",
            "toString",
            "hashCode",
            "runtimeType",
            "noSuchMethod",
        ],
    ),
    (
        "bool",
        &["toString", "hashCode", "runtimeType", "noSuchMethod"],
    ),
];

/// The dart:core primitives whose assignability the name level decides: a
/// value typed by one of these names never fits another (the types are
/// final — no user subclass or implementation can close the gap). `num` is
/// the one supertype edge (`int`/`double` fit it).
const FINAL_PRIMITIVES: [&str; 5] = ["int", "double", "num", "bool", "String"];

/// User-definable binary operators (operator-name spellings the model keeps).
const USER_OPERATORS: [&str; 16] = [
    "+", "-", "*", "/", "%", "~/", "|", "&", "^", "<<", ">>", ">>>", "<", ">", "<=", ">=",
];

/// The reference shape a recorded name stands for (the §8 names-table `kind`
/// column).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameUse {
    /// Value read.
    Read,
    /// Assignment target.
    Write,
    /// Invocation callee.
    Invoke,
    /// Type annotation position.
    Type,
    /// Metadata annotation position.
    Annotation,
    /// Named argument label.
    NamedArgument,
}

/// One unresolved reference: the byproduct row the index names table consumes
/// (index-design-spec §8). Recorded exactly when a lookup ran and missed —
/// diagnosed or suppressed — never for resolved references.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NameRow {
    /// The referenced name (member/prefixed uses keep the member spelling).
    pub name: StringId,
    /// The reference shape.
    pub use_kind: NameUse,
    /// Source span of the reference.
    pub range: SourceRange,
    /// Whether the reference was qualified (member access or prefix).
    pub qualified: bool,
}

/// The diagnosis of one compilation unit: emitted diagnostics plus the
/// unresolved-name byproduct table.
#[derive(Debug, Default)]
pub struct FileReport {
    /// Frozen-family diagnostics, duplicates first, then in source order.
    pub diagnostics: Vec<Diagnostic>,
    /// Unresolved-name rows (the §8 recovery surface), in source order.
    pub names: Vec<NameRow>,
    /// The usage-cluster emissions staged while the parse runs (scope
    /// frames close out of source order); merged into `diagnostics` by
    /// offset at the end of the pass.
    usage: Vec<Diagnostic>,
}

/// The `shouldIgnoreUndefined` mirror (`element.dart:8217-8250`) over the
/// library's import edges: a name referenced through an import whose target
/// did not resolve is suppressed, because the missing target is the reported
/// root cause. The filter face is show-list-only — only `show` clauses
/// qualify a name, so a bare miss under an unfiltered (or hide-only) broken
/// import still reports.
#[derive(Clone, Copy, Debug)]
pub struct UndefinedGuard<'a> {
    edges: &'a [ImportEdge],
}

impl<'a> UndefinedGuard<'a> {
    /// Builds the guard over `lib`'s import edges.
    #[must_use]
    pub fn build(graph: &'a LibraryGraph, lib: LibraryId) -> Self {
        Self {
            edges: graph.import_edges(lib),
        }
    }

    /// Whether a reference to `name` (optionally through `prefix`) must not
    /// be reported as undefined.
    ///
    /// Mirrors the oracle loop (`element.dart:8217-8250`) over the broken
    /// imports matching the prefix: the name is suppressed when it sits in a
    /// `show` list (`shownNames.contains`, hide clauses never consulted), or
    /// when it is prefixed and the broken import has no `show` clause at
    /// all. Everything else — a bare miss under an unfiltered or hide-only
    /// broken import, a prefixed name absent from the show list — reports.
    /// The `_$`-generated-part clause is deferred with the directive
    /// milestone (it only ever suppresses; see the module docs).
    #[must_use]
    pub fn ignore(&self, prefix: Option<StringId>, name: StringId) -> bool {
        for edge in self.edges {
            if edge.target.is_some() || edge.prefix != prefix {
                continue;
            }
            if edge.combinators.shows(name) {
                return true;
            }
            if prefix.is_some() && !edge.combinators.has_show() {
                return true;
            }
        }
        false
    }
}

/// What a bare-name hit looked like — the receiver classification a later
/// member access consumes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BareInfo {
    /// Defined, and a named container (static/extension/enum access face).
    Container { name: StringId, kind: ContainerKind },
    /// Defined as an import prefix.
    Prefix { name: StringId },
    /// Defined, but not a receiver face; `ty` is the declared type when the
    /// model captured one (the static-nullability checks read it).
    Plain { ty: Ty },
    /// Not defined (the row's intent decides the emitted code). The scope
    /// context is snapshotted at push time — pass 2 runs after the frames
    /// tear down:
    Missing {
        /// `this`'s type when the miss sits inside a class or enum instance
        /// member — the `undefined_method` host of a bare invocation miss.
        this_type: Option<StringId>,
        /// The miss sits inside some executable body (the `await` trigger).
        in_exec: bool,
        /// The enclosing container's face cannot answer `on`-constraint
        /// members (mixin/extension bodies): a bare miss may be defined.
        on_opaque: bool,
    },
}

/// Per-row access intent, finalized by the send/assignment/operator events.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Intent {
    /// Tentative read; an assignment upgrade turns it into [`Intent::Write`]
    /// or [`Intent::ReadWrite`].
    Read,
    Write,
    ReadWrite,
    Invoke,
    /// Operator-shaped access (`a + b`, `a[i]`): the operator spelling and
    /// the report span (the binary operator token, or the bracket pair for
    /// index accesses — the oracle's `undefined_operator` anchors).
    Operator(StringId, SourceRange),
}

/// The write form an assignment applied to an operator row (`a[i] = v`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OpWrite {
    /// Plain `=`: the write-side operator (`[]=` for index rows).
    Assign,
    /// Compound (`+=`): a read of the operator plus its write side.
    Compound,
}

/// A typed receiver recorded at member-merge time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Receiver {
    /// `this.m` inside a container member.
    This {
        container: StringId,
        kind: ContainerKind,
    },
    /// `super.m` inside a container member.
    Super {
        container: StringId,
        kind: ContainerKind,
    },
    /// A container name used as a receiver (`C.m`, `E.x`, `Ext.g`).
    Container { name: StringId, kind: ContainerKind },
    /// An import prefix (`p.m`).
    Prefix { name: StringId },
    /// A declaration-typed name (`C c; c.m()`); `ty` is the declared type
    /// the static-nullability checks read.
    Typed { type_name: StringId, ty: Ty },
    /// A folded creation/extension-override result (`C()`, `E('x')`): the
    /// container the send resolves against, with the instance-shaped face —
    /// superclass chains and `Object` members apply, the static-face
    /// dispatch does not.
    Created { name: StringId, kind: ContainerKind },
    /// Unknown type — checks are skipped (under-report).
    Unknown,
}

/// One recorded expression reference.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    name: StringId,
    range: SourceRange,
    qualified: bool,
    shape: RowShape,
    intent: Intent,
    /// The write form an assignment applied to an operator row (`a[i] = v`).
    write_op: Option<OpWrite>,
    /// The row's expression type as far as M1 knows it (push-time snapshot:
    /// `Null` for the `null` literal, a captured declared type for names,
    /// `Unknown` — the suppress-everything sentinel — for everything else).
    ty: Ty,
}

/// The structural shape of a row.
#[derive(Clone, Debug, PartialEq, Eq)]
enum RowShape {
    /// A bare identifier, resolved at push time.
    Bare(BareInfo),
    /// A `receiver.member` access, merged at the dot event.
    Member { recv: Receiver },
    /// A typed receiver awaiting its member (`this`, `super`).
    Receiver(Receiver),
    /// A type-annotation position, verdict snapshotted at push time, plus
    /// the `is`/`as` suppression flag.
    TypeUse(TypeVerdict, bool),
    /// A resolved non-receiver value (call results, literals).
    Value,
}

/// Scope stack frame.
struct Frame {
    kind: FrameKind,
    /// Names declared directly in this frame's scope.
    locals: FxHashSet<StringId>,
    /// Declared-type bindings: name → declared type.
    var_types: FxHashMap<StringId, Ty>,
    /// `type_params` length to truncate back to when this frame closes.
    type_mark: usize,
    /// The row-stack length when this frame opened: type parameters that
    /// join mid-header (a method's own `<T>`, parsed after its return type)
    /// rewrite their not-yet-scoped `undefined_class` rows only above this
    /// mark, never an outer declaration's stale row.
    row_mark: usize,
}

enum FrameKind {
    /// A class/mixin/enum/extension body: fields and the member face.
    Container { name: StringId, kind: ContainerKind },
    /// An executable (method, constructor, function, function literal);
    /// `ret` carries the declared return type, the oracle host word, and the
    /// host name when the declaration shape exposes them (constructors and
    /// local functions stay `None` — their return checks under-report).
    Executable {
        instance: bool,
        ret: Option<ExecReturn>,
    },
    /// A statement block, catch clause, or typedef body.
    Block,
}

/// The return-check host of an executable frame (`return_of_invalid_type`'s
/// per-host message picks the word and the name).
struct ExecReturn {
    ty: Ty,
    host: ExecKind,
    name: StringId,
}

/// One open argument list: the arguments collected so far (name, type,
/// range — the range of the argument's expression row) plus the argument
/// currently being parsed. `mark` is the row-stack index where the current
/// argument's rows begin — the parser fires the positional-argument event
/// *after* the expression (rows already pushed) and the named-argument
/// events *before* the value, so the window from `mark` to the row count is
/// the event-order-agnostic shape of one argument.
struct ArgLevel {
    /// `(name, type, primitive-literal flag, range)` per argument — the
    /// flag feeds the int→double literal conversion
    /// ([`Engine::assignable`]).
    args: Vec<(Option<StringId>, Ty, bool, SourceRange)>,
    /// An argument is open between its `open_argument` and the next flush.
    arg_open: bool,
    /// The open argument's name (`None` = positional).
    pending_name: Option<StringId>,
    mark: usize,
}

/// The member-walk outcome: found, genuinely absent after a clean walk, or a
/// broken chain (an unresolvable container hop — suppression territory).
enum Walk<'a> {
    Found(&'a crate::model::MemberEntry),
    Missing,
    ChainBroken,
}

/// One non-static member hit of a face-chain walk
/// ([`Engine::face_entries`]).
#[derive(Clone, Copy)]
struct FaceEntry<'a> {
    /// The member's lookup spelling (setters under `name=`).
    name: StringId,
    /// The member entry.
    entry: &'a MemberEntry,
    /// The declaring container's name (the message's enclosing class).
    owner_name: StringId,
    /// The file that owns the declaring container (the private-name
    /// accessibility test).
    owner_file: FileId,
    /// The hop contributes to the official `implemented` map — own members,
    /// `with` mixins, superclass hops; never an `implements` hop
    /// (`inheritance_manager3.dart:333` `_addImplemented`).
    concrete_source: bool,
}

/// The abstract-completeness check's interface view
/// ([`Engine::collect_interface`]): the interface names in face order with a
/// representative hit each, plus the names a concrete source implements.
struct InterfaceView<'a> {
    names: Vec<StringId>,
    representative: FxHashMap<StringId, FaceEntry<'a>>,
    implemented: FxHashSet<StringId>,
}

/// The implicit extension-application outcome for one member access
/// ([`Engine::extension_application`]).
enum ExtensionApp {
    /// No applicable extension declares the member: the `undefined_*` path
    /// proceeds.
    None,
    /// Exactly one applicable extension declares it: the access is defined.
    Single,
    /// Two or more applicable extensions declare it and none is more
    /// specific: `ambiguous_extension_member_access`, with the candidates'
    /// display pair (element-shape, inline-shape) in face declaration order.
    Ambiguous(Vec<(String, String)>),
}

/// A qualified-type prefix awaiting its name (`p` of `p.C`); dead when the
/// prefix itself already reported and the name must not.
struct TypePrefix {
    prefix: StringId,
    live: bool,
}

/// The push-time verdict of a type-annotation position. Rows resolve after
/// the scope frames tear down, so the verdict snapshots what the scope stack
/// said while the type was in context. Defined types push no row at all (a
/// non-type used as a type carries an unfrozen oracle code — no row, no
/// report).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TypeVerdict {
    /// The head `p` of `p.C`, not defined: `undefined_identifier` at the
    /// head (guarded); the continuation contributes nothing.
    PrefixHead,
    /// `p.C` with `p` defined and `C` absent from the prefix: the guarded
    /// miss is `undefined_class` at `C`.
    Prefixed { prefix: StringId },
    /// Not defined anywhere: `undefined_class` unless the position is
    /// suppressed (`is`/`as` operands carry other oracle codes).
    Missing,
}

/// Which part of a type annotation an identifier occupies. The prefix head
/// takes the dedicated [`Engine::arm_type_prefix`] path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TypeHead {
    /// A bare type name (`Foo`).
    Bare,
    /// The name after the dot of a qualified type (`C` of `p.C`).
    Continuation,
}

/// One local declaration of the open frame stack (the usage cluster's
/// declaration side). `uses` counts the finalized reads that resolved here;
/// the routing at frame close follows the official
/// `_visitLocalVariableElement` split (unused_local_elements_verifier.dart:
/// 1259-1280).
struct LocalDecl {
    name: StringId,
    /// The name token's span (the diagnostic anchor).
    range: SourceRange,
    kind: LocalKind,
    uses: u32,
    /// The pattern-variable-declaration group (`None` = independent).
    group: Option<u32>,
}

/// The local declaration kinds the usage cluster routes on.
#[derive(Clone, Copy)]
enum LocalKind {
    /// A plain local variable.
    Variable,
    /// A pattern-declared variable (`var (a, b) = …`, `case var x`).
    PatternVariable,
    /// A formal parameter; the declaration site it belongs to feeds the
    /// `unused_element_parameter` body-reference rule.
    Parameter { site: Option<usize> },
    /// A catch clause's exception variable; `auto` records the official
    /// visitCatchClause auto-use (a plain `catch` without an `on` type, or
    /// a stack-trace sibling present) — patched in at `handle_catch_block`
    /// once both facts are known.
    CatchException {
        auto: bool,
    },
    /// A catch clause's stack trace variable.
    CatchStack,
    /// A named local function (reports as `unused_element`).
    LocalFunction,
}

/// One library-level declaration site of this file (the usage cluster's
/// library-level side), captured at the declaration events with the name
/// token's span.
#[derive(Clone, Copy)]
struct DeclSite {
    /// Lookup spelling — setters under `name=` (the model's convention;
    /// use checks compare against the bare name).
    name: StringId,
    range: SourceRange,
    kind: DeclKind,
    /// Any metadata annotation was present (the pragma/JS exemption gates'
    /// under-report superset: an annotated declaration is never reported).
    annotated: bool,
}

/// The library-level declaration kinds the usage cluster reports on.
#[derive(Clone, Copy)]
enum DeclKind {
    /// A top-level function, getter, or setter.
    TopLevelFn,
    /// A top-level variable.
    TopLevelVar,
    /// A typedef (either style).
    TypeAlias,
    /// A named class/mixin/enum/extension-type declaration. Unnamed and
    /// named extensions never join: the official verifier has no
    /// extension-declaration visit (unused_local_elements_verifier.dart
    /// visits classes, enums, mixins, and extension types only).
    Container,
    /// A class member; `container` names the declaring container.
    Member {
        kind: MemberKind,
        is_static: bool,
        container: StringId,
    },
    /// A named constructor of `container`.
    Constructor { container: StringId },
}

/// One import directive of this file (the `unused_import` side): the
/// written URI, its prefix, and the source-matched graph edge.
struct ImportSite {
    /// The URI as written (quotes stripped; the message's `{0}`).
    uri_text: String,
    /// The URI literal's span (the diagnostic anchor).
    uri_range: SourceRange,
    prefix: Option<StringId>,
}

/// One label scope (`_LabelTracker`): the labels declared in it and their
/// used flags.
struct LabelScope {
    labels: Vec<(StringId, SourceRange, bool)>,
}

/// One completed catch clause of a try (the
/// `_CatchClausesVerifier.nextCatchClause` inputs).
struct CatchClauseFact {
    /// The `on T` type's name (`None` = plain `catch`, and `Object` plays
    /// the same catch-all role).
    on_type: Option<StringId>,
    /// The clause's first token (`on`/`catch`).
    start: SourceRange,
    /// The clause block's closing brace.
    end: SourceRange,
}

/// The engine: one parse, then the resolution pass.
struct Engine<'a> {
    graph: &'a LibraryGraph,
    namespaces: &'a Namespaces,
    index: &'a ElementIndex,
    strings: &'a StringTable,
    source: &'a str,
    tokens: &'a TokenTable,
    file: FileId,
    lib: LibraryId,
    guard: UndefinedGuard<'a>,
    report: FileReport,
    rows: Vec<Row>,
    frames: Vec<Frame>,
    /// In-scope type parameters, outermost first (truncated per frame).
    type_params: Vec<StringId>,
    /// Type variables declared since the last frame opened. A declaration's
    /// header events fire before its frame (`begin_class_declaration` opens
    /// after the type variables, official `parseClassOrNamedMixinApplication`),
    /// so they park here and drain into the frame that owns them.
    pending_type_params: Vec<StringId>,
    /// The armed declared type: `handle_type`/`handle_void_keyword` arm,
    /// `handle_no_type` disarms. Consumed by declaration bindings and by the
    /// executable return-type latches.
    armed_ty: Option<Ty>,
    /// The open variable-declaration group's declared type (one
    /// `parse_type` per group), for the initializer checks.
    pending_decl_ty: Option<Ty>,
    /// Per open type-argument group: the type head it belongs to, when the
    /// group follows a type-annotation identifier (`C<int> x`, `const
    /// C<int>()`, `p.C<int>`), plus the row-stack height at `<` — the rows
    /// pushed inside the group are its own type arguments (unresolved ones
    /// report as `TypeUse` rows), so the head the group applies to sits at
    /// `base - 1`. A headless group belongs to an expression (`g<int>()`,
    /// `g<int>`).
    group_heads: Vec<(Option<(StringId, SourceRange)>, usize)>,
    /// The type head awaiting its group (`C` of `C<int>`), armed at the
    /// identifier and moved into `group_heads` at `begin_type_arguments`.
    pending_type_head: Option<(StringId, SourceRange)>,
    /// Expression type-argument groups awaiting their use: the callee row
    /// index, the given count, and the group span.
    deferred_args: Vec<(usize, u16, SourceRange)>,
    /// Per open argument list, the argument types collected so far.
    arg_levels: Vec<ArgLevel>,
    /// The enclosing enum constant's declared type-parameter count (the
    /// `_enum` code's declared side).
    in_enum_element: Option<u16>,
    /// The row-stack mark where the current function body's expression
    /// began (the arrow-body return check reads it).
    body_mark: Option<usize>,
    /// Open switch contexts (statements and expressions): the scrutinee's
    /// push-time type, the cases completed so far, and the anchors the
    /// diagnostics attach to.
    switches: Vec<SwitchCtx>,
    /// The open pattern-nesting stack (`begin_pattern`/`end_pattern`
    /// brackets); each frame collects the leaf restrictions it spans.
    pattern_stack: Vec<PatternFrame>,
    /// The number of currently open patterns.
    pattern_depth: usize,
    /// Completed root restrictions per open case (switch cases only).
    pattern_roots: Vec<Restriction>,
    /// A switch-case header's pattern window is open
    /// (`begin_case_expression`/`begin_switch_expression_case` through the
    /// pattern's completion): the only window whose root pattern feeds
    /// exhaustiveness. Roots outside it — if-case, pattern assignments —
    /// are not coverage questions and never enter the set.
    case_sink_open: bool,
    /// The in-flight constant pattern's classification, armed by the
    /// literal/identifier events inside it.
    pending_const: Option<Restriction>,
    /// The `switch` keyword of the open switch statement/expression (the
    /// non-exhaustive anchor; the block events only carry the `{`).
    switch_kw_latch: Option<SourceRange>,
    /// The row-stack mark where the current variable initializer's
    /// expression began (the initializer check reads it).
    init_mark: Option<usize>,
    /// The row-stack mark where the current return statement's expression
    /// began (the return check reads it).
    return_mark: Option<usize>,
    /// Inside a cascade (`a?..b` picks the `?..` message shape).
    in_cascade: bool,
    /// Per open argument list: the callee row index, when identifiable
    /// (`None` while inside a `new`/`const`/implicit-creation reference —
    /// its class name is a type use, never an invocation callee).
    invokes: Vec<Option<usize>>,
    /// The argument group that just closed, awaiting the send event that
    /// consumes it (an argless send never opens a group, so a bare read
    /// stays a read).
    pending_send: Option<usize>,
    type_prefix: Option<TypePrefix>,
    /// Suppression depth inside `is`/`as` type operands.
    non_type_pos: u32,
    /// Suppression depth inside `is` type operands only (the usage
    /// cluster's `_insideIsExpression` gate — `as` operands still count as
    /// type uses).
    is_type_pos: u32,
    /// Suppression depth inside `new`/`const`/implicit-creation expressions.
    ctor_pos: u32,
    /// The open constructor reference's segments `(class, ctor-selector)`:
    /// the wave-3 parser emits reference heads as `ConstructorReference`
    /// type uses (resolved names push no row), so the reference's
    /// type-argument group is matched to its head through this state
    /// instead of the row stack. Cleared at `end_constructor_reference`.
    creation_ref: Option<(StringId, Option<StringId>)>,
    /// Interned reserved-member spellings (populated once in [`Engine::new`]:
    /// the hot `resolve_bare` path compares ids, never re-interns).
    object_members: [StringId; OBJECT_MEMBERS.len()],
    enum_synth_members: [StringId; ENUM_SYNTH_MEMBERS.len()],
    builtin_types: [StringId; BUILTIN_TYPES.len()],
    /// `name` → `name=` lookup spellings already interned (write targets
    /// repeat across a unit; the cache is bounded by their distinct names).
    setter_ids: FxHashMap<StringId, StringId>,
    /// The five final primitives' interned names (the assignability table
    /// and the literal types compare ids, never text).
    primitive_ids: [StringId; 5],
    /// The open constructor-initializer list depth (`super.named()` in an
    /// initializer is the `undefined_constructor_in_initializer` shape).
    initializer_depth: u32,
    /// The `@` token of the open metadata entry (the
    /// `undefined_annotation` anchor spans `@` through the name).
    annotation_at: Option<SourceRange>,
    /// The representation checks gate on the primary-constructors feature
    /// arm (`error_verifier.dart:5425` branches on
    /// `Feature.primary_constructors`; the expected-representation-type row
    /// lives in the disabled arm only).
    primary_constructors_enabled: bool,
    /// The hop-namespace memo (`(file, probed name)` → the owning library):
    /// #491 S2a's reverse lookup, cached per probe.
    owner_libs: RefCell<FxHashMap<(FileId, StringId), Option<LibraryId>>>,
    /// Extension types whose representation checks already ran this pass
    /// (one emission per container: an augmentation re-fires the event).
    reported_extension_types: FxHashSet<StringId>,
    /// Containers whose face-inheritance checks (#498) already ran this
    /// pass (one emission per container: an augmentation re-fires the
    /// event).
    reported_face_checks: FxHashSet<StringId>,
    /// The #279 constant-evaluation capture: expression folding while a
    /// const context is open ([`crate::const_eval`]).
    const_capture: ConstCapture,
    /// Const-value scopes, innermost last; scope 0 is the file scope and
    /// never pops. Const locals/fields bind their evaluated value here as
    /// they close (file order).
    const_scopes: Vec<FxHashMap<StringId, ConstBinding>>,
    /// The open declaration group is `const` (locals or fields) — its
    /// initializer expressions are const contexts.
    const_group: bool,
    /// The const group's declared names, in initializer order.
    const_group_names: Vec<StringId>,
    /// The next const-group name an initializer window consumes.
    const_group_cursor: usize,
    /// Top-level variables this pass saw declared *without* `const` (the
    /// provable side of the identifier classification; use-before-decl
    /// stays suppressed).
    nonconst_top_level: FxHashSet<StringId>,
    /// Static const field values captured this pass, keyed
    /// `(container, member)` (file order: uses after the declaration).
    const_field_values: FxHashMap<(StringId, StringId), ConstantValue>,
    /// Containers of this file with a declared `const` constructor (the
    /// arg-check gate for `const C(...)` / enum constants).
    const_ctors: FxHashSet<StringId>,
    /// The interned `identical` spelling (the one invocable const call).
    identical_id: StringId,
    /// Names of the const initializer windows currently open (innermost
    /// last) — the binding key at close.
    pending_init_names: Vec<StringId>,
    /// The arg-check gate per open const-creation window (enum element /
    /// explicit const ctor), innermost last.
    arg_gates: Vec<bool>,
    /// An enum-constant window is open (`handle_enum_element` doubles as
    /// the previous element's close).
    enum_element_open: bool,
    /// Begin spans of open string literals (the `end_literal_string`
    /// event carries only the end token).
    const_string_begins: Vec<SourceRange>,
    // —— #504: the usage cluster ———————————————————————————————————————————
    /// Flat stack of the open frames' local declarations; frame `i` owns
    /// `[frame_local_marks[i], len)` (the #504 usage cluster's declaration
    /// side, `unused_local_elements_verifier.dart`).
    locals: Vec<LocalDecl>,
    /// Per open frame, the locals-stack length at frame open.
    frame_local_marks: Vec<usize>,
    /// Local-referencing rows: the row's range offset → the local
    /// declaration the bare read resolved to. Consumed by the write-only /
    /// useless-read cancellations and the receiver merge (the row stack
    /// shifts, so rows carry no back-pointer).
    local_hits: FxHashMap<u32, u32>,
    /// The open pattern-variable-declaration group window (a locals-stack
    /// base): every pattern-declared variable since the base joins the
    /// group, and one used member spares the whole group (official
    /// `visitPatternVariableDeclaration`, unused_local_elements_verifier.
    /// dart:856-884). The window opens at the first declared variable after
    /// the previous statement boundary and closes at
    /// `handle_pattern_variable_declaration_statement` — case-pattern
    /// variables keep `group = None` and report independently.
    pattern_group_mark: Option<usize>,
    /// The pattern-group sequence (group ids).
    pattern_group_seq: u32,
    /// The parameters of the executable declaration in flight attach to
    /// this declaration site (set at the site-creating begin events,
    /// read by the formal-parameter identifiers).
    open_exec_site: Option<usize>,
    /// Declaration sites of this file, parse order (= source order): the
    /// library-level side of the usage cluster.
    decl_sites: Vec<DeclSite>,
    /// The open field group's static bit (`begin_fields` arms it, the
    /// group's `FieldDeclaration` identifiers read it).
    /// Metadata names of the declaration in flight (`MetadataReference`
    /// collects; a declaration site consumes them into its `annotated`
    /// exemption bit — any annotation exempts, the official pragma/JS
    /// gates' under-report superset).
    pending_annotations: Vec<StringId>,
    /// Library-level reference index: every name some reference resolved
    /// to in this file (locals included — shadowing only ever suppresses).
    used_names: FxHashSet<StringId>,
    /// Names a READ-shaped member access resolved to (the `unused_field`
    /// read-only rule: writes never spare a field).
    read_names: FxHashSet<StringId>,
    /// `(callee spelling, named argument)` pairs some invocation gave (the
    /// `_addParametersForArguments` face of the official use index).
    named_given: FxHashSet<(StringId, StringId)>,
    /// Callee spelling → the largest positional argument count any
    /// invocation gave (optional-positional parameters past it stay
    /// ungiven).
    positional_given: FxHashMap<StringId, u16>,
    /// Declaration site → the parameter names some body use resolved to.
    site_param_uses: FxHashMap<usize, FxHashSet<StringId>>,
    // —— #505: the import/flow cluster —————————————————————————————————————
    /// The import directives of this file, parse order: the written URI
    /// and the prefix (`ImportPrefixDeclaration` identifier).
    imports: Vec<ImportSite>,
    /// Open label scopes (`_LabelTracker`, dead_code_verifier.dart:
    /// 618-661): labeled statements and switch member label groups.
    label_scopes: Vec<LabelScope>,
    /// The `on`-type name of the catch clause in flight (the first type
    /// reference before the catch parameters).
    catch_on_type: Option<StringId>,
    /// The catch clause's first token (`on`/`catch`) — the clause fact's
    /// start anchor.
    catch_clause_start: Option<SourceRange>,
    /// Catch parameters seen in the clause in flight (0 = none, 1 =
    /// exception, 2 = exception + stack trace).
    catch_params: u8,
    /// Per open try, the completed catch-clause facts (the
    /// `_CatchClausesVerifier` state).
    try_clauses: Vec<Vec<CatchClauseFact>>,
    /// Open `??` binary groups: rows-stack mark at `begin_binary_expression`,
    /// for the strictly-non-nullable left operand check at close.
    if_null_groups: Vec<(usize, SourceRange)>,
    /// Formal-parameter name spans per declaration site (the
    /// `unused_element_parameter` anchors).
    site_params: FxHashMap<usize, Vec<(StringId, SourceRange)>>,
    /// Rows pushed while the named scope referred to itself (the official
    /// `_enclosingClass`/`_enclosingExec` self-reference exclusions): the
    /// used-name marking at resolve time skips exactly these rows.
    self_scope_rows: FxHashSet<u32>,
    /// Rows pushed while the bare name matched a field of the enclosing
    /// container: a non-write resolution is the field's implicit-getter
    /// read (the official `readMembers` face the `unused_field` rule
    /// consults).
    field_read_rows: FxHashSet<u32>,
    /// The container of the constructor/factory being parsed (`None`
    /// outside one).
    ctor_pending: Option<StringId>,
    /// The top-level function declaration site awaiting its setter
    /// spelling fix-up at `end_top_level_method`.
    open_toplevel_site: Option<usize>,
    /// The member declaration's `set` head (`begin_method` latch; the
    /// member site's lookup spelling).
    open_member_setter: bool,
    /// The member/field declaration's `static` head (`begin_method` /
    /// `begin_fields` latch; the member site's static bit).
    open_member_static: bool,
    /// The label name captured at `LabelDeclaration`, consumed by the
    /// `handle_label` colon.
    pending_label: Option<(StringId, SourceRange)>,
    /// The last block's closing-brace span (the catch body anchor).
    last_block_end: Option<SourceRange>,
    /// The local function whose name has fired but whose declaration has
    /// not closed: the tracked `LocalFunction` entry materializes into the
    /// ENCLOSING frame's window at the end event (the name resolves in the
    /// function's own frame, but its use-counting scope is the enclosing
    /// one).
    pending_local_fn: Option<(StringId, SourceRange)>,
    /// Open local-function frames as `(frame depth, name)`: the official
    /// `_enclosingExec` spelling for bodies inside a local function (a
    /// self-reference never marks the function used).
    local_fn_stack: Vec<(usize, StringId)>,
}

/// A const-environment binding.
enum ConstBinding {
    /// The evaluated value of a completed const variable.
    Value(ConstantValue),
    /// The initializer currently being evaluated — a self-reference is a
    /// `recursive_compile_time_constant` cycle.
    InProgress,
}

/// The callee classification a closed bare-identifier call resolves to
/// (`evaluation.dart` `_invalidConstantForMethodInvocation`).
enum CallClass {
    /// An in-file container with a declared const constructor — the
    /// implicit creation form checks its arguments.
    ConstCtor,
    /// A resolvable invocable name (top-level function/variable, local
    /// callable): the invocation itself faults.
    Invocable,
    /// Unresolvable, or a container without a const constructor (the
    /// `const_with_non_const*` family) — out of subset, defer.
    Unknown,
}

impl<'a> Engine<'a> {
    // —— plumbing ————————————————————————————————————————————————————

    #[allow(clippy::too_many_arguments)] // the analysis context, one field each
    fn new(
        graph: &'a LibraryGraph,
        namespaces: &'a Namespaces,
        index: &'a ElementIndex,
        strings: &'a StringTable,
        source: &'a str,
        tokens: &'a TokenTable,
        file: FileId,
        lib: LibraryId,
        primary_constructors_enabled: bool,
    ) -> Self {
        let guard = UndefinedGuard::build(graph, lib);
        let object_members = OBJECT_MEMBERS.map(|m| strings.intern(m));
        let enum_synth_members = ENUM_SYNTH_MEMBERS.map(|m| strings.intern(m));
        let builtin_types = BUILTIN_TYPES.map(|m| strings.intern(m));
        let primitive_ids = FINAL_PRIMITIVES.map(|m| strings.intern(m));
        let identical_id = strings.intern("identical");
        Self {
            graph,
            namespaces,
            index,
            strings,
            source,
            tokens,
            file,
            lib,
            guard,
            report: FileReport::default(),
            rows: Vec::new(),
            frames: Vec::new(),
            type_params: Vec::new(),
            pending_type_params: Vec::new(),
            armed_ty: None,
            pending_decl_ty: None,
            group_heads: Vec::new(),
            pending_type_head: None,
            deferred_args: Vec::new(),
            arg_levels: Vec::new(),
            in_enum_element: None,
            body_mark: None,
            init_mark: None,
            return_mark: None,
            switches: Vec::new(),
            pattern_stack: Vec::new(),
            pattern_depth: 0,
            pattern_roots: Vec::new(),
            case_sink_open: false,
            pending_const: None,
            switch_kw_latch: None,
            in_cascade: false,
            invokes: Vec::new(),
            pending_send: None,
            type_prefix: None,
            non_type_pos: 0,
            is_type_pos: 0,
            ctor_pos: 0,
            creation_ref: None,
            object_members,
            enum_synth_members,
            builtin_types,
            setter_ids: FxHashMap::default(),
            primitive_ids,
            initializer_depth: 0,
            annotation_at: None,
            const_capture: ConstCapture::new(identical_id),
            const_scopes: vec![FxHashMap::default()],
            const_group: false,
            const_group_names: Vec::new(),
            const_group_cursor: 0,
            nonconst_top_level: FxHashSet::default(),
            const_field_values: FxHashMap::default(),
            const_ctors: FxHashSet::default(),
            identical_id,
            pending_init_names: Vec::new(),
            arg_gates: Vec::new(),
            enum_element_open: false,
            const_string_begins: Vec::new(),
            primary_constructors_enabled,
            owner_libs: RefCell::new(FxHashMap::default()),
            reported_extension_types: FxHashSet::default(),
            reported_face_checks: FxHashSet::default(),
            locals: Vec::new(),
            frame_local_marks: Vec::new(),
            local_hits: FxHashMap::default(),
            pattern_group_mark: None,
            pattern_group_seq: 0,
            open_exec_site: None,
            decl_sites: Vec::new(),
            pending_annotations: Vec::new(),
            used_names: FxHashSet::default(),
            read_names: FxHashSet::default(),
            named_given: FxHashSet::default(),
            positional_given: FxHashMap::default(),
            site_param_uses: FxHashMap::default(),
            imports: Vec::new(),
            label_scopes: Vec::new(),
            catch_on_type: None,
            catch_clause_start: None,
            catch_params: 0,
            try_clauses: Vec::new(),
            if_null_groups: Vec::new(),
            site_params: FxHashMap::default(),
            self_scope_rows: FxHashSet::default(),
            field_read_rows: FxHashSet::default(),
            ctor_pending: None,
            open_toplevel_site: None,
            open_member_setter: false,
            open_member_static: false,
            pending_label: None,
            last_block_end: None,
            pending_local_fn: None,
            local_fn_stack: Vec::new(),
        }
    }

    /// The library whose scope resolves `name` as written in `owner_file`
    /// (#491 S2a): hop 2+ of a super-face climb resolves through the
    /// declaring container's library, not the diagnosing one. Memoized per
    /// `(file, name)` probe — the BFS over the graph edges repeats only for
    /// distinct probes.
    fn owner_library_of(&self, owner_file: FileId, name: StringId) -> Option<LibraryId> {
        if owner_file == self.file {
            return Some(self.lib);
        }
        if let Some(&cached) = self.owner_libs.borrow().get(&(owner_file, name)) {
            return cached;
        }
        let found = owner_library(self.graph, self.lib, owner_file, name);
        self.owner_libs
            .borrow_mut()
            .insert((owner_file, name), found);
        found
    }

    fn intern(&self, name: &str) -> StringId {
        self.strings.intern(name)
    }

    /// Token text, `None` for parser-inserted synthetics (not in the table).
    fn text_of(&self, token: Token) -> Option<&'a str> {
        self.tokens
            .get(token.id as usize)
            .map(|t| &self.source[t.byte_range()])
    }

    fn name_of(&self, token: Token) -> Option<StringId> {
        self.text_of(token).map(|text| self.strings.intern(text))
    }

    fn emit(&mut self, code: SemanticCode, range: SourceRange, a0: &str, a1: &str) {
        self.report.diagnostics.push(Diagnostic::new(
            code.severity(),
            code.code_info(),
            code.message(a0, a1),
            self.file,
            range,
            None,
        ));
    }

    /// The three-placeholder emit (`message3`).
    fn emit3(&mut self, code: SemanticCode, range: SourceRange, a0: &str, a1: &str, a2: &str) {
        self.report.diagnostics.push(Diagnostic::new(
            code.severity(),
            code.code_info(),
            code.message3(a0, a1, a2),
            self.file,
            range,
            None,
        ));
    }

    /// The four-placeholder emit (`message4`).
    fn emit4(
        &mut self,
        code: SemanticCode,
        range: SourceRange,
        a0: &str,
        a1: &str,
        a2: &str,
        a3: &str,
    ) {
        self.report.diagnostics.push(Diagnostic::new(
            code.severity(),
            code.code_info(),
            code.message4(a0, a1, a2, a3),
            self.file,
            range,
            None,
        ));
    }

    /// The five-placeholder emit (`message5`).
    #[allow(clippy::too_many_arguments)] // the template's five slots, one each
    fn emit5(
        &mut self,
        code: SemanticCode,
        range: SourceRange,
        a0: &str,
        a1: &str,
        a2: &str,
        a3: &str,
        a4: &str,
    ) {
        self.report.diagnostics.push(Diagnostic::new(
            code.severity(),
            code.code_info(),
            code.message5(a0, a1, a2, a3, a4),
            self.file,
            range,
            None,
        ));
    }

    fn record_miss(&mut self, name: StringId, range: SourceRange, qualified: bool, kind: NameUse) {
        self.report.names.push(NameRow {
            name,
            range,
            qualified,
            use_kind: kind,
        });
    }

    // —— scope frames ————————————————————————————————————————————————

    fn push_frame(&mut self, kind: FrameKind) {
        let type_mark = self.type_params.len();
        let row_mark = self.rows.len();
        let pending = std::mem::take(&mut self.pending_type_params);
        self.type_params.extend(pending);
        self.frames.push(Frame {
            kind,
            locals: FxHashSet::default(),
            var_types: FxHashMap::default(),
            type_mark,
            row_mark,
        });
        self.frame_local_marks.push(self.locals.len());
        self.const_scopes.push(FxHashMap::default());
    }

    fn pop_frame(&mut self) {
        if let Some(frame) = self.frames.pop() {
            self.type_params.truncate(frame.type_mark);
            self.close_frame_locals(frame);
        }
        while self
            .local_fn_stack
            .last()
            .is_some_and(|(depth, _)| *depth >= self.frames.len())
        {
            self.local_fn_stack.pop();
        }
        // Scope 0 is the file scope; it never pops.
        if self.const_scopes.len() > 1 {
            self.const_scopes.pop();
        }
    }

    /// Closes a frame's local declarations: the usage cluster's per-scope
    /// decision point. Every un-referenced declaration routes per its kind
    /// (the official `_visitLocalVariableElement` split,
    /// unused_local_elements_verifier.dart:1259-1280); the emissions stage
    /// for the end-of-pass source-order merge.
    fn close_frame_locals(&mut self, _frame: Frame) {
        let mark = self.frame_local_marks.pop().unwrap_or(self.locals.len());
        if mark >= self.locals.len() {
            self.locals.truncate(mark);
            return;
        }
        // Pattern-declaration groups report all-or-nothing: one used member
        // spares the group (official visitPatternVariableDeclaration).
        let mut group_used: FxHashMap<u32, bool> = FxHashMap::default();
        for decl in &self.locals[mark..] {
            if let Some(group) = decl.group {
                *group_used.entry(group).or_default() |= decl.uses > 0;
            }
        }
        for index in mark..self.locals.len() {
            let (name, range, kind, uses, group) = {
                let decl = &self.locals[index];
                (decl.name, decl.range, decl.kind, decl.uses, decl.group)
            };
            // Parameters feed the declaration site's body-reference rule
            // (`unused_element_parameter`); they never emit per se.
            if let LocalKind::Parameter { site: Some(site) } = kind {
                if uses > 0 {
                    self.site_param_uses
                        .entry(site)
                        .or_default()
                        .insert(name);
                }
                continue;
            }
            if uses > 0 {
                continue;
            }
            let spared = match group {
                Some(group) => group_used.get(&group).copied().unwrap_or(false),
                None => false,
            };
            if spared {
                continue;
            }
            let wildcard = self.is_wildcard(name);
            let name_text = self.strings.resolve(name).to_string();
            let (code, name) = match kind {
                LocalKind::Variable | LocalKind::PatternVariable => {
                    // The wildcard-variables feature: an all-underscores
                    // name never reports (unused_local_elements_verifier.dart
                    // :985-994).
                    if wildcard {
                        continue;
                    }
                    (SemanticCode::UnusedLocalVariable, name_text)
                }
                LocalKind::Parameter { .. } => {
                    // Parameters route through `unused_element_parameter` at
                    // the declaration site, never the local-variable row.
                    continue;
                }
                LocalKind::LocalFunction => {
                    if wildcard {
                        continue;
                    }
                    (SemanticCode::UnusedElement, name_text)
                }
                LocalKind::CatchException { auto } => {
                    // A plain `catch (e)` (no `on` type) or a clause that
                    // also declares a stack trace parameter auto-uses its
                    // exception variable (official visitCatchClause,
                    // unused_local_elements_verifier.dart:67-82); the flag
                    // was patched in at `handle_catch_block`.
                    if auto || wildcard {
                        continue;
                    }
                    (SemanticCode::UnusedCatchClause, name_text)
                }
                // The stack trace row carries no wildcard exemption — the
                // official catch-stack branch has none either.
                LocalKind::CatchStack => (SemanticCode::UnusedCatchStack, name_text),
            };
            self.stage_usage(code, range, &name);
        }
        self.locals.truncate(mark);
    }

    /// Stages one usage-cluster diagnostic (merged into the report at the
    /// end of the pass, source order restored there).
    fn stage_usage(&mut self, code: SemanticCode, range: SourceRange, a0: &str) {
        let correction = code.correction_message(a0);
        self.report.usage.push(Diagnostic::new(
            code.severity(),
            code.code_info(),
            code.message(a0, ""),
            self.file,
            range,
            correction,
        ));
    }

    /// The bare lookup spelling of a site name (setters live under `name=`
    /// in the sites; the use index carries plain names).
    fn bare_name(&self, spelling: StringId) -> StringId {
        let text = self.strings.resolve(spelling);
        match text.strip_suffix('=') {
            Some(bare) => self.intern(bare),
            None => spelling,
        }
    }

    /// The `_`-private test (library privacy, the usage cluster's gate).
    fn is_private_name(&self, name: StringId) -> bool {
        self.strings.resolve(name).starts_with('_')
    }

    /// The usage cluster's library-level pass, after every use is indexed:
    /// the declaration sites of this file walk against the use index (the
    /// official `UnusedLocalElementsVerifier`), then the import directives
    /// against the contributed-element index (`imports_verifier.dart`).
    /// Each arm's gate mirrors the official `_isUsed*` split; every
    /// unmodelable corner suppresses (the under-report direction).
    fn finalize_usage(&mut self) {
        for site_index in 0..self.decl_sites.len() {
            let site = self.decl_sites[site_index];
            let bare = self.bare_name(site.name);
            let name_text = self.strings.resolve(site.name).to_string();
            match site.kind {
                DeclKind::TopLevelFn | DeclKind::TopLevelVar | DeclKind::TypeAlias => {
                    // `_isUsedElement`'s else arm: a public name is used;
                    // otherwise any indexed use spares the declaration.
                    if !site.annotated
                        && self.is_private_name(site.name)
                        && !self.used_names.contains(&bare)
                    {
                        self.stage_usage(SemanticCode::UnusedElement, site.range, &name_text);
                    }
                }
                DeclKind::Container => {
                    // `_visitClassElement` (classes, enums, mixins, and
                    // extension types; extensions are never visited): the
                    // same public/used gate.
                    if !site.annotated
                        && self.is_private_name(site.name)
                        && !self.used_names.contains(&bare)
                    {
                        self.stage_usage(SemanticCode::UnusedElement, site.range, &name_text);
                    }
                }
                DeclKind::Member {
                    kind: MemberKind::Field,
                    is_static,
                    container,
                } => {
                    // `_isReadMember`: a public field of a public container
                    // is used; a public static field of a private container
                    // is checked like a private one. Used only when READ
                    // (the implicit getter enters the read set; writes never
                    // spare it), or when a super-name it overrides is read.
                    let eligible = !site.annotated
                        && (self.is_private_name(site.name)
                            || (self.is_private_name(container) && is_static));
                    if !eligible || self.read_names.contains(&bare) {
                        continue;
                    }
                    self.stage_usage(SemanticCode::UnusedField, site.range, &name_text);
                }
                DeclKind::Member { container, .. } => {
                    // `_isUsedMember` for methods/getters/setters: the
                    // accessibility gate first, then indexed uses, then the
                    // override face (a member overriding a super-name is
                    // spared — the walk cannot see whether the overridden
                    // member is itself used, so this only under-reports).
                    // The parameter pass below runs regardless — the
                    // official `visitFormalParameterList` is independent of
                    // the member's own unused-ness.
                    let spared = site.annotated
                        || self.is_publicly_accessible(site.name, &site.kind)
                        || self.used_names.contains(&bare)
                        || matches!(self.walk_super(container, site.name), Walk::Found(_));
                    if !spared {
                        self.stage_usage(SemanticCode::UnusedElement, site.range, &name_text);
                    }
                }
                DeclKind::Constructor { container } => {
                    // `_visitConstructorElement`: named constructors only
                    // (the capture side enforces that), and only when the
                    // class declares more than one — a single constructor
                    // may serve to prevent extension. The parameter pass
                    // below runs regardless.
                    let multi = self
                        .index
                        .get(self.file)
                        .and_then(|f| f.container(container))
                        .is_some_and(|c| {
                            c.constructors.len() + usize::from(c.has_unnamed_constructor) > 1
                        });
                    let spared = !multi
                        || site.annotated
                        || self.is_publicly_accessible(site.name, &site.kind)
                        || self.used_names.contains(&bare)
                        || matches!(self.walk_super(container, site.name), Walk::Found(_));
                    if !spared {
                        self.stage_usage(SemanticCode::UnusedElement, site.range, &name_text);
                    }
                }
            }
            self.report_optional_parameters(site_index, site);
        }
        self.report_unused_imports();
    }

    /// `_isPubliclyAccessible` (unused_local_elements_verifier.dart:
    /// 1002-1028) over the site's facts: a private name is never
    /// accessible; an instance member of a private interface stays
    /// reachable through exported instances; a static member or
    /// constructor of a private interface is not; a member of an extension
    /// is accessible exactly when the extension is; a constructor of an
    /// enum never is.
    fn is_publicly_accessible(&self, name: StringId, kind: &DeclKind) -> bool {
        if self.is_private_name(name) {
            return false;
        }
        let (container, is_static, is_ctor) = match kind {
            DeclKind::Member {
                is_static,
                container,
                ..
            } => (*container, *is_static, false),
            DeclKind::Constructor { container } => (*container, false, true),
            // A top-level executable's accessibility is just its name.
            _ => return !self.is_private_name(name),
        };
        let Some(c) = self
            .index
            .get(self.file)
            .and_then(|f| f.container(container))
        else {
            return true;
        };
        match c.kind {
            ContainerKind::Enum => !(is_ctor || (self.is_private_name(container) && is_static)),
            ContainerKind::Extension => !self.is_private_name(container),
            _ => !(self.is_private_name(container) && (is_static || is_ctor)),
        }
    }

    /// The `unused_element_parameter` pass over one executable's
    /// declaration site (`visitFormalParameterList` +
    /// `_isUsedElement`'s parameter arm,
    /// unused_local_elements_verifier.dart:1064-1115): optional parameters
    /// of constructors, methods, and top-level functions that no body use
    /// resolves to and no invocation supplies. The official exemptions in
    /// order: a required parameter, a constructor of a generic class, a
    /// generic executable, a publicly accessible executable, an executable
    /// overriding a super-name, and the super-parameter face (unmodelable —
    /// suppressed).
    fn report_optional_parameters(&mut self, site_index: usize, site: DeclSite) {
        let (member_kind, container) = match site.kind {
            DeclKind::Member { kind, container, .. } => (Some(kind), Some(container)),
            DeclKind::Constructor { container } => (None, Some(container)),
            _ => (None, None),
        };
        if matches!(member_kind, Some(MemberKind::Field) | Some(MemberKind::Setter)) {
            return;
        }
        let formals = match container {
            Some(c) => self
                .index
                .get(self.file)
                .and_then(|f| f.container(c))
                .and_then(|x| x.members.get(&site.name))
                .and_then(|m| m.formals.as_deref()),
            None => self
                .index
                .get(self.file)
                .and_then(|f| f.decl(site.name))
                .and_then(|d| d.formals.as_deref()),
        };
        let Some(formals) = formals else {
            return;
        };
        // The 47839 exemptions: a constructor of a generic class, or an
        // executable with its own type parameters, is never reported.
        let executable = self.index.get(self.file).and_then(|f| {
            container.and_then(|c| f.container(c)).map(|x| {
                (
                    x.type_params > 0,
                    x.members.get(&site.name).map_or(false, |m| m.type_params > 0),
                )
            })
        });
        let (class_generic, member_generic) = match (site.kind, executable) {
            (DeclKind::Constructor { .. }, Some((class_generic, _))) => (class_generic, false),
            (_, Some((class_generic, member_generic))) => (class_generic, member_generic),
            _ => (false, false),
        };
        if member_generic || (class_generic && matches!(site.kind, DeclKind::Constructor { .. })) {
            return;
        }
        // The accessibility and override gates.
        if self.is_publicly_accessible(site.name, &site.kind) {
            return;
        }
        if let Some(c) = container
            && matches!(self.walk_super(c, site.name), Walk::Found(_))
        {
            return;
        }
        let bare = self.bare_name(site.name);
        // Cloned: the emit below mutates the report while the set is read.
        let body_uses = self.site_param_uses.get(&site_index).cloned();
        let given_positional = self.positional_given.get(&bare).copied().unwrap_or(0);
        let name_span = |engine: &Self, formal: StringId| -> SourceRange {
            engine
                .site_params
                .get(&site_index)
                .and_then(|ps| ps.iter().find(|(n, _)| *n == formal).map(|(_, r)| *r))
                .unwrap_or(site.range)
        };
        for (index, formal) in formals.positional.iter().enumerate() {
            if formal.required {
                continue;
            }
            let referenced = body_uses.as_ref().is_some_and(|u| u.contains(&formal.name));
            if referenced || given_positional > index as u16 {
                continue;
            }
            let text = self.strings.resolve(formal.name).to_string();
            let range = name_span(self, formal.name);
            self.stage_usage(SemanticCode::UnusedElementParameter, range, &text);
        }
        for formal in formals.named.iter() {
            if formal.required {
                continue;
            }
            let referenced = body_uses.as_ref().is_some_and(|u| u.contains(&formal.name));
            if referenced || self.named_given.contains(&(bare, formal.name)) {
                continue;
            }
            let text = self.strings.resolve(formal.name).to_string();
            let range = name_span(self, formal.name);
            self.stage_usage(SemanticCode::UnusedElementParameter, range, &text);
        }
    }

    /// The `unused_import` pass: an import with a resolved, existing,
    /// non-`dart:core` target reports when it contributed no used element
    /// (`generateUnusedImportWarnings`, imports_verifier.dart:228-268).
    /// Edge matching is positional and only when the directive count agrees
    /// with the library's edge count — a mismatch suppresses (under-report).
    fn report_unused_imports(&mut self) {
        if self.imports.is_empty() {
            return;
        }
        let edges = self.graph.import_edges(self.lib);
        if edges.len() != self.imports.len() {
            return;
        }
        let used: Vec<StringId> = self.used_names.iter().copied().collect();
        for (index, edge) in edges.iter().enumerate() {
            // Copied first: the emit below mutates the report.
            let (uri_text, uri_range, prefix) = {
                let site = &self.imports[index];
                (site.uri_text.clone(), site.uri_range, site.prefix)
            };
            let Some(target) = edge.target else {
                continue;
            };
            if self.graph.uri(target) == "dart:core" {
                continue;
            }
            let is_used = match prefix {
                Some(prefix) => self.used_names.contains(&prefix),
                None => used
                    .iter()
                    .any(|&n| edge.combinators.allows(n) && self.graph.exported(target, n).is_some()),
            };
            if !is_used {
                self.stage_usage(SemanticCode::UnusedImport, uri_range, &uri_text);
            }
        }
    }

    /// The `_`-wildcard test: the name consists entirely of underscores
    /// (the official fallback rule, unused_local_elements_verifier.dart:
    /// 985-994 — the wildcard-variables feature's `isWildcardVariable`
    /// answers the same for `_`).
    fn is_wildcard(&self, name: StringId) -> bool {
        let text = self.strings.resolve(name);
        !text.is_empty() && text.bytes().all(|b| b == b'_')
    }

    /// The enclosing executable's lookup spelling once its name has fired
    /// (the official `_enclosingExec`); constructors, factories stay
    /// `None` (their `ret` latch is unset by design). A local function's
    /// own body is covered by the open local-function stack.
    fn exec_self_name(&self) -> Option<StringId> {
        if let Some((depth, name)) = self.local_fn_stack.last()
            && *depth == self.frames.len().saturating_sub(1)
        {
            return Some(*name);
        }
        self.frames.last().and_then(|f| match &f.kind {
            FrameKind::Executable {
                ret: Some(ret), ..
            } => Some(ret.name),
            _ => None,
        })
    }

    /// The official self-reference test: `name` names the enclosing
    /// executable or the enclosing container (`_useIdentifierElement`'s
    /// identical checks).
    fn in_own_named_scope(&self, name: StringId) -> bool {
        self.exec_self_name() == Some(name)
            || self.container_context().is_some_and(|(c, _)| c == name)
    }

    /// Consumes the pending annotation latch: any metadata annotation
    /// since the last declaration site marks it exempt (the pragma/JS
    /// entry-point gates' under-report superset).
    fn take_annotated(&mut self) -> bool {
        let annotated = !self.pending_annotations.is_empty();
        self.pending_annotations.clear();
        annotated
    }

    /// Pushes one library-level declaration site (the usage cluster's
    /// library-level side) and returns its index.
    fn note_decl_site(&mut self, name: StringId, range: SourceRange, kind: DeclKind) -> usize {
        let annotated = self.take_annotated();
        self.decl_sites.push(DeclSite {
            name,
            range,
            kind,
            annotated,
        });
        self.decl_sites.len() - 1
    }

    /// A local variable declaration (`LocalVariableDeclaration`): the
    /// tracked local of the open frame.
    fn note_local(&mut self, name: StringId, range: SourceRange) {
        self.add_tracked_local(name, range, LocalKind::Variable);
    }

    /// A formal parameter of the executable whose name has fired: the
    /// tracked local joins its declaration site for the
    /// `unused_element_parameter` body-reference rule.
    fn note_parameter(&mut self, name: StringId, range: SourceRange) {
        self.add_tracked_local(name, range, LocalKind::Parameter {
            site: self.open_exec_site,
        });
        if let Some(site) = self.open_exec_site {
            self.site_params.entry(site).or_default().push((name, range));
        }
    }

    /// A catch clause parameter: first is the exception variable, second
    /// the stack trace. The exception's auto-use bit patches in at
    /// `handle_catch_block` when both facts are known.
    fn note_catch_parameter(&mut self, name: StringId, range: SourceRange) {
        let kind = if self.catch_params == 0 {
            LocalKind::CatchException { auto: false }
        } else {
            LocalKind::CatchStack
        };
        self.catch_params += 1;
        self.add_tracked_local(name, range, kind);
    }

    /// A named local function (reports as `unused_element`). The name
    /// event fires inside the function's own frame: the name joins the
    /// ENCLOSING frame's resolution set (its official scope — in-body
    /// recursion resolves through it, and outer invocations find it),
    /// while the tracked entry defers to the end event — its use-counting
    /// window is the enclosing frame's, so an outer invocation spares it
    /// (the official `_enclosingExec` keeps only in-body self-references
    /// from counting: the entry is absent from the tracked stack while
    /// the body parses, so a self-reference finds no declaration).
    fn note_local_function(&mut self, name: StringId, range: SourceRange) {
        if self.frames.len() >= 2
            && let Some(enclosing) = self.frames.len().checked_sub(2)
            && let Some(frame) = self.frames.get_mut(enclosing)
        {
            frame.locals.insert(name);
        }
        self.pending_local_fn = Some((name, range));
        if let Some(depth) = self.frames.len().checked_sub(1) {
            self.local_fn_stack.push((depth, name));
        }
    }

    /// `end_local_function_declaration` / `end_named_function_expression`:
    /// the tracked local-function entry materializes into the enclosing
    /// frame's window just before the frame pops.
    fn finish_local_fn_site(&mut self) {
        if let Some((name, range)) = self.pending_local_fn.take() {
            self.locals.push(LocalDecl {
                name,
                range,
                kind: LocalKind::LocalFunction,
                uses: 0,
                group: None,
            });
        }
    }

    /// `begin_import`: the URI literal is the token after the keyword.
    /// Only an honestly quoted literal joins (recovery shapes stay
    /// silent — the under-report direction).
    fn begin_import_site(&mut self, import_kw: Token) {
        let Some(uri) = self.tokens.get(import_kw.id as usize + 1) else {
            return;
        };
        let raw = &self.source[uri.byte_range()];
        let bytes = raw.as_bytes();
        if bytes.len() < 2 {
            return;
        }
        let quote = bytes[0];
        if (quote != b'\'' && quote != b'"') || bytes[bytes.len() - 1] != quote {
            return;
        }
        self.imports.push(ImportSite {
            uri_text: raw[1..raw.len() - 1].to_string(),
            uri_range: SourceRange {
                offset: uri.utf16_offset,
                length: raw.chars().map(char::len_utf16).sum::<usize>() as u32,
            },
            prefix: None,
        });
    }

    /// The statement-window read rule (the official `_isUsefulRead`,
    /// unused_local_elements_verifier.dart:603-619): a single unqualified
    /// read in an expression statement whose parent shape is a
    /// null-assertion, a logical-not, an increment/decrement, or a
    /// unary-operator invocation never counts (`!v;`, `v!;`, `v++;`,
    /// `++v;`, `-v;`); a compound assignment's read does not count either
    /// (`visitCompoundAssignment`'s `readCountsAsUse: parent is!
    /// ExpressionStatement`). Plain `v;`, `v + 1;`, and `v ??= 1;` are
    /// useful reads.
    fn check_statement_use(&mut self, begin: Token, end: Token) {
        // The statement boundary closes the pattern-declaration window: a
        // later group is its own all-or-nothing set.
        self.pattern_group_mark = None;
        let w0 = begin.range.offset;
        let w1 = end.range.end();
        // The window's single non-value row (value rows are literals,
        // groups, closures — their shapes are never the discarded read).
        let mut found: Option<(SourceRange, bool, bool)> = None; // (range, read, read-write)
        for row in self.rows.iter().rev() {
            if row.range.end() <= w0 {
                break;
            }
            if row.range.offset >= w1 || matches!(row.shape, RowShape::Value) {
                continue;
            }
            let read = row.intent == Intent::Read;
            let read_write = row.intent == Intent::ReadWrite;
            if !read && !read_write {
                return; // invoke/write/operator rows are always uses
            }
            if found.is_some() {
                return; // two named rows: the reads are useful
            }
            found = Some((row.range, read, read_write));
        }
        let Some((row_range, read, read_write)) = found else {
            return;
        };
        // The window's token texts (bounded by the statement itself); the
        // offset side is the token's UTF-16 offset, comparable with the
        // rows' ranges.
        let window_ops: Vec<(u32, &str)> = (begin.id..=end.id)
            .filter_map(|id| {
                self.tokens
                    .get(id as usize)
                    .map(|t| (t.utf16_offset, &self.source[t.byte_range()]))
            })
            .collect();
        let discarded = if read {
            let head_op = self.text_of(begin).is_some_and(|text| {
                matches!(text, "!" | "-" | "~" | "++" | "--")
            });
            let tail_op = window_ops
                .iter()
                .any(|(offset, text)| *offset == row_range.end() && matches!(*text, "!" | "++" | "--"));
            head_op || tail_op
        } else if read_write {
            // A compound assignment discards; `??=` never does
            // (`visitIfNullAssignment` reads count as use unconditionally).
            window_ops.iter().any(|(_, text)| {
                matches!(
                    *text,
                    "+=" | "-="
                        | "*=" | "/="
                        | "%="
                        | "~/="
                        | "&="
                        | "|="
                        | "^="
                        | "<<="
                        | ">>="
                        | ">>>="
                )
            })
        } else {
            false
        };
        if discarded && let Some(hit) = self.local_hits.remove(&row_range.offset) {
            self.locals[hit as usize].uses -= 1;
        }
    }

    /// `handle_catch_block`: the clause's facts are complete (on-type,
    /// parameters, body block). The official auto-use decision patches the
    /// exception variable (plain `catch`, or a stack-trace sibling, uses
    /// it), the clause joins the try's clause list, and the catch frame
    /// closes — the parameters leave scope only after the body.
    fn finish_catch_clause(&mut self) {
        let auto = self.catch_on_type.is_none() || self.catch_params > 1;
        if let Some(mark) = self.frame_local_marks.last().copied() {
            for decl in self.locals[mark..].iter_mut().rev() {
                if let LocalKind::CatchException { auto: slot } = &mut decl.kind {
                    *slot = auto;
                    break;
                }
            }
        }
        self.catch_params = 0;
        let Some(start) = self.catch_clause_start.take() else {
            self.catch_on_type = None;
            return;
        };
        let on_type = self.catch_on_type.take();
        if let Some(clauses) = self.try_clauses.last_mut() {
            clauses.push(CatchClauseFact {
                on_type,
                start,
                end: self.last_block_end.unwrap_or(start),
            });
        }
        self.pop_frame();
    }

    /// `end_try_statement`: the official `_CatchClausesVerifier` — a
    /// catch-all clause (no `on` type, or `on Object`) followed by more
    /// clauses makes the tail dead; the span covers the next clause's
    /// first token through the last clause's closing brace
    /// (`dead_code_verifier.dart`, `nextCatchClause`).
    fn finish_try_statement(&mut self) {
        let Some(clauses) = self.try_clauses.pop() else {
            return;
        };
        let object = self.intern("Object");
        for (index, clause) in clauses.iter().enumerate() {
            let catches_all = clause.on_type.map_or(true, |t| t == object);
            if catches_all && index + 1 < clauses.len() {
                let start = clauses[index + 1].start.offset;
                let end = clauses[clauses.len() - 1].end.end();
                self.stage_usage(
                    SemanticCode::DeadCodeCatchFollowingCatch,
                    SourceRange {
                        offset: start,
                        length: end.saturating_sub(start),
                    },
                    "",
                );
                break;
            }
        }
    }

    /// `handle_label` (the colon fired): the pending label opens its own
    /// scope; `end_labeled_statement` pops one scope per label.
    fn note_label_decl(&mut self) {
        if let Some((name, range)) = self.pending_label.take() {
            self.label_scopes.push(LabelScope {
                labels: vec![(name, range, false)],
            });
        }
    }

    /// A `break`/`continue` target: the innermost scope declaring the name
    /// marks its label used.
    fn note_label_use(&mut self, name: StringId) {
        for scope in self.label_scopes.iter_mut().rev() {
            if let Some(slot) = scope.labels.iter_mut().find(|(n, _, _)| *n == name) {
                slot.2 = true;
                return;
            }
        }
    }

    /// `end_labeled_statement`: one scope per label pops; the statement's
    /// labels merge (nested `a: b: stmt` declares both) and the unused
    /// ones report (the official `_LabelTracker` pop,
    /// dead_code_verifier.dart).
    fn finish_labeled_statement(&mut self, label_count: usize) {
        let count = label_count.min(self.label_scopes.len());
        if count == 0 {
            return;
        }
        let split = self.label_scopes.len() - count;
        let mut merged: Vec<(StringId, SourceRange, bool)> = Vec::new();
        for scope in self.label_scopes.drain(split..) {
            merged.extend(scope.labels);
        }
        for (name, range, used) in merged {
            if !used {
                let text = self.strings.resolve(name).to_string();
                self.stage_usage(SemanticCode::UnusedLabel, range, &text);
            }
        }
    }

    /// `begin_binary_expression` for `??`: the left operand's row parks
    /// with the operator span.
    fn begin_if_null(&mut self, op_range: SourceRange) {
        self.if_null_groups.push((self.rows.len(), op_range));
    }

    /// `end_binary_expression` for `??`: the right operand's row anchors
    /// the report when the left operand's type is strictly non-nullable —
    /// the right operand is never executed (the official
    /// `_checkForDeadNullCoalesce`, error_verifier.dart:4733-4739).
    fn end_if_null(&mut self) {
        let Some((mark, _op_range)) = self.if_null_groups.pop() else {
            return;
        };
        let non_null = mark
            .checked_sub(1)
            .and_then(|index| self.rows.get(index))
            .is_some_and(|lhs| lhs.ty.is_strictly_non_nullable());
        if !non_null {
            return;
        }
        if let Some(rhs) = self.rows.last() {
            let range = rhs.range;
            self.stage_usage(SemanticCode::DeadNullAwareExpression, range, "");
        }
    }

    /// `handle_assignment_expression` for `??=`: the target's read type
    /// strictly non-nullable anchors the report at the value (the official
    /// `_checkForDeadNullCoalesce`'s `??=` arm).
    fn check_dead_null_assignment(&mut self, operator: Token) {
        if self.text_of(operator) != Some("??=") {
            return;
        }
        let limit = operator.range.offset;
        let non_null = self
            .rows
            .iter()
            .rev()
            .find(|row| row.range.end() <= limit && !matches!(row.shape, RowShape::Value))
            .is_some_and(|target| target.ty.is_strictly_non_nullable());
        if non_null && let Some(value) = self.rows.last() {
            let range = value.range;
            self.stage_usage(SemanticCode::DeadNullAwareExpression, range, "");
        }
    }

    /// The local declaration `name` resolves to, innermost frame first
    /// (the same walk [`Engine::resolve_bare`]'s locals arm runs).
    fn resolve_local(&self, name: StringId) -> Option<u32> {
        for depth in (0..self.frames.len()).rev() {
            if self.frames[depth].locals.contains(&name) {
                let lo = self.frame_local_marks[depth];
                let hi = self
                    .frame_local_marks
                    .get(depth + 1)
                    .copied()
                    .unwrap_or(self.locals.len());
                // Within one frame, a use follows its declaration
                // textually: the last entry under the name is it.
                return self.locals[lo..hi]
                    .iter()
                    .rposition(|d| d.name == name)
                    .map(|p| (lo + p) as u32);
            }
        }
        None
    }

    fn container_context(&self) -> Option<(StringId, ContainerKind)> {
        self.frames.iter().rev().find_map(|f| match &f.kind {
            FrameKind::Container { name, kind } => Some((*name, *kind)),
            _ => None,
        })
    }

    /// Whether the innermost executable is an instance member.
    fn instance_context(&self) -> bool {
        self.frames
            .iter()
            .rev()
            .find_map(|f| match &f.kind {
                FrameKind::Executable { instance, ret: _ } => Some(*instance),
                _ => None,
            })
            .unwrap_or(false)
    }

    fn in_executable(&self) -> bool {
        self.frames
            .iter()
            .any(|f| matches!(f.kind, FrameKind::Executable { .. }))
    }

    /// Binds a local's declared type in the innermost frame (the
    /// declared-variable-pattern binding face).
    fn bind_var_type(&mut self, name: StringId, ty: Ty) {
        if let Some(frame) = self.frames.last_mut() {
            frame.var_types.insert(name, ty);
        }
    }

    /// Classifies a constant pattern whose events left no literal: a
    /// qualified `E.c` tail (enum container + member row) becomes the
    /// enum-value leaf; everything else is unrepresentable.
    fn classify_constant_tail(&self) -> Restriction {
        // The member push replaces the qualifier row, so the `E` of `E.c`
        // survives only as the member row's container receiver.
        let Some(last) = self.rows.last() else {
            return Restriction::Unknown;
        };
        let RowShape::Member { recv } = &last.shape else {
            return Restriction::Unknown;
        };
        match recv {
            Receiver::Container {
                name,
                kind: ContainerKind::Enum,
            } => Restriction::EnumValue(*name, last.name),
            _ => Restriction::Unknown,
        }
    }

    fn add_local(&mut self, name: StringId) {
        if let Some(frame) = self.frames.last_mut() {
            frame.locals.insert(name);
        }
    }

    /// Records a local declaration of the usage cluster: the frame's name
    /// set (resolution) plus the tracked entry (kind, anchor, group).
    fn add_tracked_local(&mut self, name: StringId, range: SourceRange, kind: LocalKind) {
        self.add_local(name);
        // A pattern-variable-declaration window claims the pattern-declared
        // variables since its last statement boundary: the first opens it,
        // the rest join its group (one used member spares all).
        let group = if matches!(kind, LocalKind::PatternVariable) {
            match self.pattern_group_mark {
                Some(base) => self.locals[base].group,
                None => {
                    self.pattern_group_seq += 1;
                    Some(self.pattern_group_seq)
                }
            }
        } else {
            None
        };
        if group.is_some() && self.pattern_group_mark.is_none() {
            self.pattern_group_mark = Some(self.locals.len());
        }
        self.locals.push(LocalDecl {
            name,
            range,
            kind,
            uses: 0,
            group,
        });
    }

    /// Binds the open declaration group's type to `name` in the innermost
    /// frame (one `parse_type` arms the group; every name in it binds).
    fn bind_declared_type(&mut self, name: StringId) {
        let ty = self.pending_decl_ty.unwrap_or(Ty::UNKNOWN);
        if let Some(frame) = self.frames.last_mut() {
            frame.var_types.insert(name, ty);
        }
    }

    /// Binds a formal or catch parameter's own declared type (one arm per
    /// parameter, not shared with a group).
    fn bind_parameter_type(&mut self, name: StringId) {
        let ty = self.armed_ty.take().unwrap_or(Ty::UNKNOWN);
        if let Some(frame) = self.frames.last_mut() {
            frame.var_types.insert(name, ty);
        }
    }

    /// A container head: the member face and the `this`/`super` receiver for
    /// its body. Unnamed extensions get a placeholder face — their container
    /// name resolves to nothing, so member walks through them suppress.
    fn push_container_frame(&mut self, kind: ContainerKind, name: Option<StringId>) {
        self.push_frame(FrameKind::Container {
            name: name.unwrap_or_else(|| self.intern("")),
            kind,
        });
    }

    /// An executable head: formals and its own type parameters live inside.
    fn push_executable_frame(&mut self, instance: bool) {
        self.push_frame(FrameKind::Executable {
            instance,
            ret: None,
        });
    }

    /// Latches the armed declared type as the innermost executable's return
    /// type (`begin_method`/`begin_top_level_method` fire before the return
    /// type's `parse_type`; the name identifier fires after it).
    fn latch_return_type(&mut self, host: ExecKind, name: StringId) {
        let ty = self.armed_ty.take().unwrap_or(Ty::UNKNOWN);
        if let Some(frame) = self.frames.last_mut()
            && let FrameKind::Executable { ret, .. } = &mut frame.kind
            && ret.is_none()
        {
            *ret = Some(ExecReturn { ty, host, name });
        }
    }

    /// A statement block, catch clause, for body, or typedef scope.
    fn push_block_frame(&mut self) {
        self.push_frame(FrameKind::Block);
    }

    /// Enters a type parameter's scope. A declaration whose frame is already
    /// open (a method's own `<T>` parses after its return type; function
    /// expressions open before theirs) joins the frame's `type_params`
    /// directly — the frame's `type_mark` truncates it on pop — and
    /// re-scopes the signature type rows that resolved `Missing` before the
    /// parameter existed. Container headers parse their variables before
    /// `begin_*_declaration`, so those park for the frame that owns them.
    fn push_type_parameter(&mut self, name: StringId) {
        let Some(mark) = self.frames.last().map(|f| f.row_mark) else {
            self.pending_type_params.push(name);
            return;
        };
        self.type_params.push(name);
        // The signature rows pushed since the frame opened (`T m<T>(...)`'s
        // return type) never saw this parameter: a still-missing type use of
        // the same name was the parameter all along, not an undefined class.
        for row in &mut self.rows[mark..] {
            if row.name == name
                && matches!(row.shape, RowShape::TypeUse(TypeVerdict::Missing, false))
            {
                row.shape = RowShape::Value;
            }
        }
    }

    /// The name after the dot of a qualified type arrived: the prefix slot
    /// is consumed.
    fn consume_type_prefix(&mut self) {
        self.type_prefix = None;
    }

    fn enter_non_type_pos(&mut self) {
        self.non_type_pos += 1;
    }

    fn leave_non_type_pos(&mut self) {
        self.non_type_pos -= 1;
    }

    fn enter_ctor_pos(&mut self) {
        self.ctor_pos += 1;
    }

    fn leave_ctor_pos(&mut self) {
        self.ctor_pos -= 1;
    }

    /// A bare name's declared type, innermost frame first.
    fn typed_local(&self, name: StringId) -> Option<Ty> {
        self.frames
            .iter()
            .rev()
            .find_map(|f| f.var_types.get(&name).copied())
    }

    /// A container member's declared field type from a clean walk
    /// (`Unknown` for executables and broken chains).
    fn member_declared_ty(&self, container: StringId, member: StringId) -> Ty {
        match self.walk_members(container, member) {
            Walk::Found(entry) if entry.kind == MemberKind::Field => entry.var_ty,
            _ => Ty::UNKNOWN,
        }
    }

    /// Whether the formal table declares a named formal under `name`.
    fn has_named_formal(formals: Option<&Formals>, name: StringId) -> bool {
        formals.is_some_and(|f| f.named.iter().any(|formal| formal.name == name))
    }

    /// The resolved top-level declaration under `name` (`None` = no check).
    fn top_level_decl(&self, name: StringId) -> Option<&TopDecl> {
        match lookup_scoped(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        ) {
            ScopedHit::Local(decl) => Some(decl),
            ScopedHit::Found(NamespaceEntry::Decl(file)) => {
                self.index.get(file).and_then(|f| f.decl(name))
            }
            _ => None,
        }
    }

    /// The full formal table of a resolved member callee, walked from `recv`.
    fn member_formals_full(&self, recv: &Receiver, name: StringId) -> Option<&Formals> {
        let (container, super_walk) = match *recv {
            Receiver::This { container, .. } => (container, false),
            Receiver::Super { container, .. } => (container, true),
            Receiver::Typed { type_name, .. } => (type_name, false),
            // A folded creation (`C().m(...)`): the instance face — the
            // constructor-argument membership check walks the superclass
            // chain like a declaration-typed receiver.
            Receiver::Created { name, .. } => (name, false),
            Receiver::Container {
                name: container, ..
            } => {
                let entry = resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    container,
                )
                .and_then(|(c, _)| c.members.get(&name));
                return match entry {
                    Some(entry) if entry.kind == MemberKind::Method => entry.formals.as_deref(),
                    _ => None,
                };
            }
            Receiver::Prefix { .. } | Receiver::Unknown => return None,
        };
        let walk = if super_walk {
            self.walk_super(container, name)
        } else {
            self.walk_members(container, name)
        };
        match walk {
            Walk::Found(entry) if entry.kind == MemberKind::Method => entry.formals.as_deref(),
            _ => None,
        }
    }

    // —— resolution ——————————————————————————————————————————————————

    /// Resolves a bare name in value position: locals, type parameters, the
    /// member face (own-file container chain), the extension pool, enum
    /// synthesized members, then the scoped library lookup.
    fn resolve_bare(&self, name: StringId) -> BareInfo {
        if self.frames.iter().rev().any(|f| f.locals.contains(&name)) {
            return BareInfo::Plain {
                ty: self.typed_local(name).unwrap_or(Ty::UNKNOWN),
            };
        }
        if self.type_params.contains(&name) {
            return BareInfo::Plain {
                ty: Ty::named(name),
            };
        }
        if let Some((container, kind)) = self.container_context() {
            if !matches!(self.walk_members(container, name), Walk::Missing) {
                // Found, or a broken chain — both suppress.
                return BareInfo::Plain {
                    ty: self.member_declared_ty(container, name),
                };
            }
            if kind == ContainerKind::Enum && self.enum_synth_members.contains(&name) {
                return BareInfo::Plain { ty: Ty::UNKNOWN };
            }
            if self.instance_context() && self.object_members.contains(&name) {
                return BareInfo::Plain { ty: Ty::UNKNOWN };
            }
        }
        if self
            .index
            .get(self.file)
            .is_some_and(|f| f.has_extension_member(name))
        {
            return BareInfo::Plain { ty: Ty::UNKNOWN };
        }
        match lookup_scoped(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        ) {
            ScopedHit::Local(decl) => self.bare_info_of(name, decl),
            ScopedHit::Found(entry) => self.bare_info_of_entry(name, entry),
            ScopedHit::Ambiguous(_) => BareInfo::Plain { ty: Ty::UNKNOWN },
            ScopedHit::NotFound => self.missing_info(),
        }
    }

    /// The push-time context snapshot of a bare miss (pass 2 runs after the
    /// frames tear down — see [`BareInfo::Missing`]).
    fn missing_info(&self) -> BareInfo {
        let context = self
            .instance_context()
            .then(|| self.container_context())
            .flatten();
        BareInfo::Missing {
            this_type: context
                .filter(|(_, kind)| matches!(kind, ContainerKind::Class | ContainerKind::Enum))
                .map(|(name, _)| name),
            in_exec: self.in_executable(),
            on_opaque: context.is_some_and(|(_, kind)| {
                matches!(kind, ContainerKind::Mixin | ContainerKind::Extension)
            }),
        }
    }

    fn bare_info_of_entry(&self, name: StringId, entry: NamespaceEntry) -> BareInfo {
        match entry {
            NamespaceEntry::Prefix => BareInfo::Prefix { name },
            NamespaceEntry::Decl(file) => {
                let decl = self.index.get(file).and_then(|f| f.decl(name));
                self.bare_info_of(
                    name,
                    decl.map_or(
                        &TopDecl::bare(MicroKind::TopLevelVar),
                        std::convert::identity,
                    ),
                )
            }
        }
    }

    /// The [`BareInfo`] of a resolved top-level declaration.
    fn bare_info_of(&self, name: StringId, decl: &TopDecl) -> BareInfo {
        match decl.kind {
            // Type parameters are declaration-scoped: the per-unit tag is
            // stale outside its declaration, and in-scope uses were answered
            // by the engine's type-parameter stack. The stale reference is a
            // plain miss whose code name follows the live invocation context
            // (`undefined_method`'s host inside a class instance member).
            MicroKind::TypeParam => self.missing_info(),
            MicroKind::Class => BareInfo::Container {
                name,
                kind: ContainerKind::Class,
            },
            MicroKind::Enum => BareInfo::Container {
                name,
                kind: ContainerKind::Enum,
            },
            MicroKind::Mixin => BareInfo::Container {
                name,
                kind: ContainerKind::Mixin,
            },
            MicroKind::Extension => BareInfo::Container {
                name,
                kind: ContainerKind::Extension,
            },
            _ => BareInfo::Plain { ty: decl.var_ty },
        }
    }

    /// Walks `container`'s member face through the linearized supertype
    /// chain: the container's own members, then its `with` mixins in reverse
    /// application order (the official application sequence
    /// `inheritance_manager3.dart:571-579` — `class X extends S with M1, M2`
    /// is `S&M1 extends S`, `S&M2 extends S&M1`, `X extends S&M2`, so each
    /// later mixin shadows the earlier ones and the superclass), then the
    /// superclass name edges. Any unresolvable hop breaks the chain
    /// (external type, unresolvable superclass name): the caller suppresses
    /// rather than reports.
    fn walk_members(&self, container: StringId, member: StringId) -> Walk<'a> {
        self.walk_from(container, member, false)
    }

    /// The climb behind [`Engine::walk_members`]: `skip_own` starts at the
    /// container but excludes its own member map (the `super.member` shape —
    /// the application classes `S&M2` are still the chain).
    fn walk_from(&self, container: StringId, member: StringId, mut skip_own: bool) -> Walk<'a> {
        let mut name = container;
        let mut owner = self.file;
        let mut scope_lib = self.lib;
        let mut seen: FxHashSet<StringId> = FxHashSet::default();
        loop {
            if !seen.insert(name) {
                return Walk::ChainBroken;
            }
            let Some((found, found_owner)) = resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                scope_lib,
                owner,
                name,
            ) else {
                return Walk::ChainBroken;
            };
            if !skip_own && let Some(entry) = found.members.get(&member) {
                return Walk::Found(entry);
            }
            skip_own = false;
            // The stacked applications, later first: each shadows the members
            // of the earlier ones and of the superclass. The mixin names are
            // written in `found_owner` and resolve through that file's
            // library (probed by the container name it declares, #491 S2a).
            let owner_lib = self
                .owner_library_of(found_owner, name)
                .unwrap_or(scope_lib);
            for mixin in found.with_clause.iter().rev() {
                if !seen.insert(*mixin) {
                    return Walk::ChainBroken;
                }
                let mixin_lib = owner_lib;
                if let Some(entry) = resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    mixin_lib,
                    found_owner,
                    *mixin,
                )
                .and_then(|(mixin_container, _)| mixin_container.members.get(&member))
                {
                    return Walk::Found(entry);
                }
            }
            let Some(superclass) = found.superclass else {
                return Walk::Missing;
            };
            // The superclass name is written in the declaring container's
            // file and resolves through that container's library (#491 S2a).
            scope_lib = owner_lib;
            name = superclass;
            owner = found_owner;
        }
    }

    /// `super.member`: the walk starts at the enclosing container but skips
    /// its own members — the official `S&M2` superclass chain still applies
    /// (`super.m` in `class C extends S with M1, M2` resolves M2 over M1
    /// over S). A container without a captured superclass edge and without
    /// mixins (mixins: `on` constraints are not edges) breaks the chain.
    fn walk_super(&self, container: StringId, member: StringId) -> Walk<'a> {
        let Some(container_decl) = self
            .index
            .get(self.file)
            .and_then(|f| f.container(container))
        else {
            return Walk::ChainBroken;
        };
        if container_decl.superclass.is_none() && container_decl.with_clause.is_empty() {
            return Walk::ChainBroken;
        }
        self.walk_from(container, member, true)
    }

    // —— invalid_override (#453: the inheritance-override family) ——————

    /// Checks one instance-member declaration against the container's direct
    /// super-faces (`.third/dart-sdk`
    /// `pkg/analyzer/lib/src/error/inheritance_override.dart:498-546`
    /// `_checkDeclaredMember`): for each face, the nearest same-named
    /// non-static member; kind mismatches skip (other checks own them), and
    /// a decidable name-level signature conflict emits `invalid_override` at
    /// the member's name span. Every unresolvable hop, cycle, or
    /// incomparable type suppresses — the under-report direction, never a
    /// false positive.
    ///
    /// Runs from the parse listener, so the element model (built by the
    /// earlier `collect`) is complete and the declaration's name token
    /// supplies the anchor span. Emissions repeat per failing face, matching
    /// the oracle's per-superinterface loop.
    fn check_override(&mut self, spelling: StringId, range: SourceRange) {
        // Member declarations parse inside a container frame; extensions and
        // extension types have no inheritance faces.
        let Some((container, kind)) = self.container_context() else {
            return;
        };
        if matches!(
            kind,
            ContainerKind::Extension | ContainerKind::ExtensionType
        ) {
            return;
        }
        let Some(elements) = self.index.get(self.file) else {
            return;
        };
        let Some(decl) = elements.container(container) else {
            return;
        };
        let Some(entry) = decl.members.get(&spelling) else {
            return;
        };
        if entry.is_static {
            return;
        }
        // A duplicate (second) declaration keeps the first entry in the
        // model — this span's signature is not the one recorded, and the
        // duplicate_definition owns that report.
        if elements
            .duplicates()
            .iter()
            .any(|d| d.name == spelling && d.range == range)
        {
            return;
        }
        // The direct super-faces, in the oracle's order: the supertype, the
        // `with` mixins in application order, the `implements` interfaces,
        // and the `on` constraints (mixins only — the model leaves the clause
        // empty for every other kind).
        let mut faces = Vec::with_capacity(
            usize::from(decl.superclass.is_some())
                + decl.with_clause.len()
                + decl.implements_clause.len()
                + decl.on_clause.len(),
        );
        faces.extend(decl.superclass);
        faces.extend(decl.with_clause.iter().copied());
        faces.extend(decl.implements_clause.iter().copied());
        faces.extend(decl.on_clause.iter().copied());
        let mut broken_seen: Vec<&MemberEntry> = Vec::new();
        for face in faces {
            let Some((inherited, face_name)) = self.face_member(face, spelling) else {
                // Unresolvable or cyclic face chain: suppression territory
                // (recovery residue in a clause lands here too).
                continue;
            };
            if inherited.kind != entry.kind {
                continue;
            }
            // The stacked climb can surface one inherited member through
            // several direct faces (the applied `S&M2` superclass face and
            // the `M2` face both answer M2's member); the oracle's verdict
            // per inherited member is one diagnostic.
            if broken_seen
                .iter()
                .any(|prior: &&MemberEntry| std::ptr::eq(*prior, inherited))
            {
                continue;
            }
            if Self::override_broken(entry, inherited) {
                broken_seen.push(inherited);
                let display = self.strings.resolve(spelling);
                let display = display.strip_suffix('=').unwrap_or(display);
                let container_text = self.strings.resolve(container);
                let declared_text = self.override_signature_text(entry);
                let face_text = self.strings.resolve(face_name);
                let inherited_text = self.override_signature_text(inherited);
                let code = if entry.kind == MemberKind::Setter {
                    SemanticCode::InvalidOverrideSetter
                } else {
                    SemanticCode::InvalidOverride
                };
                self.emit5(
                    code,
                    range,
                    display,
                    container_text,
                    &declared_text,
                    face_text,
                    &inherited_text,
                );
            }
        }
    }

    // —— abstract completeness + interface conflicts (#498) ———————————

    /// Mounts both container-face families at the container's name event:
    /// the same-name super-interface conflicts (`inconsistent_inheritance*`)
    /// and the concrete class/enum abstract completeness
    /// (`non_abstract_class_inherits_abstract_member`, five tiers). The
    /// element model is complete (`collect` parsed before this pass) and the
    /// name token anchors the emissions. Extensions and extension types have
    /// no inheritance faces; a mixin declaration participates in the
    /// conflicts only (the official completeness loop is
    /// `ClassElementImpl && !isAbstract || EnumElementImpl`,
    /// `inheritance_override.dart:412`). Both checks suppress on any
    /// unresolvable face chain — the under-report direction.
    fn check_face_inheritance(
        &mut self,
        container: StringId,
        kind: ContainerKind,
        name_range: SourceRange,
    ) {
        if matches!(
            kind,
            ContainerKind::Extension | ContainerKind::ExtensionType
        ) {
            return;
        }
        // One emission pass per container (an augmentation re-fires the
        // event for the same container).
        if !self.reported_face_checks.insert(container) {
            return;
        }
        let Some(elements) = self.index.get(self.file) else {
            return;
        };
        let Some(decl) = elements.container(container) else {
            return;
        };
        let Some(conflicted) = self.inheritance_conflicts(decl, name_range) else {
            return;
        };
        if kind != ContainerKind::Mixin {
            self.abstract_completeness(kind, decl, &conflicted, name_range);
        }
    }

    /// The face's linearized chain, every non-static member: the face
    /// container itself, its `with` mixins in reverse application order, its
    /// `implements` interfaces, then the superclass hop, repeating per hop
    /// ([`Engine::face_member`]'s climb, enumerating). `top_concrete` says
    /// the face contributes implementations at all (a superclass or `with`
    /// face); an `implements`/`on` face contributes interface members only,
    /// so every hit is forced `concrete_source: false`
    /// ([`FaceEntry::concrete_source`] — `_addImplemented`'s sources are own
    /// members, mixins, and the super chain only,
    /// `inheritance_manager3.dart:333`). `None` = an unresolvable or cyclic
    /// hop — the caller suppresses rather than reports.
    fn face_entries(&self, face: StringId, top_concrete: bool) -> Option<Vec<FaceEntry<'a>>> {
        let mut name = face;
        let mut owner = self.file;
        let mut scope_lib = self.lib;
        let mut seen = FxHashSet::default();
        let mut hits: Vec<FaceEntry<'a>> = Vec::new();
        loop {
            if !seen.insert(name) {
                return None;
            }
            let (found, found_owner) = resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                scope_lib,
                owner,
                name,
            )?;
            for (member, entry) in &found.members {
                if !entry.is_static {
                    hits.push(FaceEntry {
                        name: *member,
                        entry,
                        owner_name: name,
                        owner_file: found_owner,
                        concrete_source: top_concrete,
                    });
                }
            }
            // The mixin/superclass names are written in `found_owner` and
            // resolve through that file's library (#491 S2a).
            let owner_lib = self
                .owner_library_of(found_owner, name)
                .unwrap_or(scope_lib);
            for mixin in found.with_clause.iter().rev() {
                if !seen.insert(*mixin) {
                    return None;
                }
                if let Some((mixin_container, mixin_owner)) = resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    owner_lib,
                    found_owner,
                    *mixin,
                ) {
                    for (member, entry) in &mixin_container.members {
                        if !entry.is_static {
                            hits.push(FaceEntry {
                                name: *member,
                                entry,
                                owner_name: *mixin,
                                owner_file: mixin_owner,
                                concrete_source: true,
                            });
                        }
                    }
                }
            }
            for interface in &found.implements_clause {
                if let Some((interface_container, interface_owner)) = resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    owner_lib,
                    found_owner,
                    *interface,
                ) {
                    for (member, entry) in &interface_container.members {
                        if !entry.is_static {
                            hits.push(FaceEntry {
                                name: *member,
                                entry,
                                owner_name: *interface,
                                owner_file: interface_owner,
                                concrete_source: false,
                            });
                        }
                    }
                }
            }
            // The chain ends here (Object is unindexed) — enumeration
            // completes cleanly; only a broken named hop suppresses.
            let Some(superclass) = found.superclass else {
                return Some(hits);
            };
            scope_lib = owner_lib;
            name = superclass;
            owner = found_owner;
        }
    }

    /// The same-name super-interface candidate conflicts
    /// (`inheritance_override.dart:160-207` `_reportInterfaceConflicts`).
    /// Candidates accumulate per name in the oracle's face order — the
    /// superclass face (the applied chain), the `with` mixins in
    /// application order, the `implements` interfaces, the `on` constraints
    /// — with a `with` mixin's *own* member replacing the accumulated list
    /// (the official application sequence). A name conflicts when its
    /// candidates mix getter and method kinds — unresolvable, reported even
    /// when the class declares the name (verified against the oracle) — or
    /// the candidates share a kind, no candidate is a valid override of all
    /// the others, and no class-local member resolves the name (the
    /// official `map.containsKey` gate). Returns the conflicted names — a
    /// conflict drops the name from the class's interface (the official
    /// combine returns no signature), so the completeness check must not
    /// report it missing. `None` = a broken face chain — the caller
    /// suppresses.
    fn inheritance_conflicts(
        &mut self,
        decl: &Container,
        name_range: SourceRange,
    ) -> Option<FxHashSet<StringId>> {
        let mut faces: Vec<(StringId, bool, bool)> = Vec::new();
        if let Some(superclass) = decl.superclass {
            faces.push((superclass, false, true));
        }
        // `with`: candidates in application order, own-member replacement.
        faces.extend(decl.with_clause.iter().map(|m| (*m, true, true)));
        // `implements`/`on`: candidates only — never implementations.
        faces.extend(decl.implements_clause.iter().map(|i| (*i, false, false)));
        faces.extend(decl.on_clause.iter().map(|o| (*o, false, false)));
        let mut order: Vec<StringId> = Vec::new();
        let mut candidate_map: FxHashMap<StringId, Vec<FaceEntry<'a>>> = FxHashMap::default();
        for (face, is_with, contributes) in &faces {
            let entries = self.face_entries(*face, *contributes)?;
            // Keep-first per name (the nearest member wins within a chain),
            // name-sorted for a deterministic emission order.
            let mut per_name: Vec<FaceEntry<'a>> = Vec::new();
            let mut present = FxHashSet::default();
            for hit in entries {
                if present.insert(hit.name) {
                    per_name.push(hit);
                }
            }
            per_name.sort_unstable_by_key(|h| h.name);
            for hit in per_name {
                if let Some(list) = candidate_map.get_mut(&hit.name) {
                    if *is_with && hit.owner_name == *face {
                        // The mixin's own member replaces; a kind mismatch in
                        // the replacement is itself the getter/method
                        // conflict (`inheritance_manager3.dart:594-610`).
                        if Self::kind_class(list[0].entry) != Self::kind_class(hit.entry) {
                            self.emit_kind_conflict(hit.name, &list[0], &hit, name_range);
                        }
                        *list = vec![hit];
                    } else {
                        list.push(hit);
                    }
                } else {
                    order.push(hit.name);
                    candidate_map.insert(hit.name, vec![hit]);
                }
            }
        }
        // A class-local instance member resolves the same-kind conflict for
        // its name (fields cover their setter half).
        let mut declared: FxHashSet<StringId> = FxHashSet::default();
        for (name, entry) in &decl.members {
            if entry.is_static {
                continue;
            }
            declared.insert(*name);
            if entry.kind == MemberKind::Field && !entry.is_final {
                let setter = self.setter_spelling(*name);
                declared.insert(setter);
            }
        }
        let mut conflicted: FxHashSet<StringId> = FxHashSet::default();
        for name in order {
            let list = &candidate_map[&name];
            if list.len() >= 2 && self.emit_name_conflict(name, list, &declared, name_range) {
                conflicted.insert(name);
            }
        }
        Some(conflicted)
    }

    /// The candidate kind class: getter-shaped (getters and fields — the
    /// interface sees a field as its getter), method, setter.
    fn kind_class(entry: &MemberEntry) -> u8 {
        match entry.kind {
            MemberKind::Method => 0,
            MemberKind::Setter => 1,
            MemberKind::Getter | MemberKind::Field => 2,
        }
    }

    /// One multi-candidate name's conflict verdict, emitted in place.
    /// Returns whether the name is conflicted — dropped from the class's
    /// interface, so the completeness check must not report it missing.
    ///
    /// The getter/method mix has no resolution
    /// (`_checkForGetterMethodConflict`): the first getter-shaped and first
    /// method candidate in face order name the interfaces. A class-local
    /// instance member resolves the same-kind conflict for its name. The
    /// name-level valid-override search then decides: a candidate not broken
    /// against any other is a valid override. An incomparable pair reads as
    /// compatible (`override_broken`'s suppression direction) — the check
    /// never false-positives on types a subclass edge could still relate.
    fn emit_name_conflict(
        &mut self,
        name: StringId,
        list: &[FaceEntry<'a>],
        declared: &FxHashSet<StringId>,
        name_range: SourceRange,
    ) -> bool {
        let getter = list.iter().find(|c| Self::kind_class(c.entry) == 2);
        let method = list.iter().find(|c| c.entry.kind == MemberKind::Method);
        if let (Some(getter), Some(method)) = (getter, method) {
            self.emit_kind_conflict(name, getter, method, name_range);
            return true;
        }
        if declared.contains(&name) {
            return false;
        }
        let has_valid = list.iter().enumerate().any(|(i, a)| {
            list.iter()
                .enumerate()
                .all(|(j, b)| i == j || !Self::override_broken(a.entry, b.entry))
        });
        if has_valid {
            return false;
        }
        let display = self.strings.resolve(name);
        let signatures = list
            .iter()
            .map(|c| {
                format!(
                    "{}.{} ({})",
                    self.strings.resolve(c.owner_name),
                    display,
                    self.override_signature_text(c.entry)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        self.emit(
            SemanticCode::InconsistentInheritance,
            name_range,
            display,
            &signatures,
        );
        true
    }

    /// The getter/method kind conflict's emission: the member name, the
    /// getter candidate's interface, the method candidate's interface.
    fn emit_kind_conflict(
        &mut self,
        name: StringId,
        getter: &FaceEntry<'a>,
        method: &FaceEntry<'a>,
        name_range: SourceRange,
    ) {
        self.emit3(
            SemanticCode::InconsistentInheritanceGetterAndMethod,
            name_range,
            self.strings.resolve(name),
            self.strings.resolve(getter.owner_name),
            self.strings.resolve(method.owner_name),
        );
    }

    /// The concrete class/enum abstract-completeness check
    /// (`inheritance_override.dart:412-486`): every interface name without
    /// a concrete implementation is a missing row, the five-tier message
    /// rendering the sorted `Enclosing.member` descriptions. The exemptions
    /// mirror the official walk: `Object` members (always implemented), an
    /// enum's `values`/`values=`, names the class itself declares (the
    /// concrete-class-with-abstract-member code owns those — not
    /// implemented here, so they suppress), names a *concrete* direct
    /// superclass's interface carries (`_isNotImplementedInConcreteSuper
    /// Class` — the issue belongs there), cross-library private names
    /// (`isAccessibleFor`), and a concrete `noSuchMethod` on the
    /// implemented sources (the forwarder gate). A name the conflict pass
    /// dropped (`conflicted`) is absent from the class's interface — never
    /// reported missing here. An abstract field's non-final shape misses
    /// both halves (getter and setter); a final one misses the getter only.
    fn abstract_completeness(
        &mut self,
        kind: ContainerKind,
        decl: &Container,
        conflicted: &FxHashSet<StringId>,
        name_range: SourceRange,
    ) {
        if kind == ContainerKind::Class && decl.is_abstract {
            return;
        }
        // The concrete-superclass exemption's face: the direct superclass
        // resolved, concrete, and carrying the name in its interface. A
        // broken chain suppresses the whole check.
        let concrete_super_names: Option<FxHashSet<StringId>> = match decl.superclass {
            Some(superclass) => {
                let Some((super_container, _)) = resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    superclass,
                ) else {
                    return;
                };
                if super_container.kind == ContainerKind::Class && super_container.is_abstract {
                    None
                } else {
                    let Some(hits) = self.face_entries(superclass, true) else {
                        return;
                    };
                    Some(hits.into_iter().map(|h| h.name).collect())
                }
            }
            None => None,
        };
        // Own instance members block the report for their names: a concrete
        // one implements them, an abstract one is the
        // `concrete_class_with_abstract_member` report's business (not
        // implemented here, so it suppresses — never a false positive).
        let mut implemented: FxHashSet<StringId> = FxHashSet::default();
        let mut blocked: FxHashSet<StringId> = FxHashSet::default();
        for (name, entry) in &decl.members {
            if entry.is_static {
                continue;
            }
            blocked.insert(*name);
            if entry.kind == MemberKind::Field && !entry.is_final {
                let setter = self.setter_spelling(*name);
                blocked.insert(setter);
                if !entry.is_abstract {
                    implemented.insert(setter);
                }
            }
            if !entry.is_abstract {
                implemented.insert(*name);
            }
        }
        let Some(InterfaceView {
            names,
            representative,
            implemented: face_implemented,
        }) = self.collect_interface(decl)
        else {
            return;
        };
        implemented.extend(face_implemented);
        // An enum's synthesized `values` pair is always implemented.
        let values = self.enum_synth_members[0];
        let values_setter = self.setter_spelling(values);
        let descriptions: Vec<String> = names
            .into_iter()
            .filter(|name| !blocked.contains(name))
            .filter(|name| !self.object_members.contains(name))
            .filter(|name| {
                kind != ContainerKind::Enum || (*name != values && *name != values_setter)
            })
            .filter(|name| {
                concrete_super_names
                    .as_ref()
                    .is_none_or(|names| !names.contains(name))
            })
            .filter(|name| !conflicted.contains(name))
            .filter(|name| !implemented.contains(name))
            .filter(|name| {
                // `isAccessibleFor`: a *private* name inherited from another
                // library can never be implemented — the oracle skips it.
                // Public names are accessible from any importing library.
                if !self.strings.resolve(*name).starts_with('_') {
                    return true;
                }
                let hit = &representative[name];
                hit.owner_file == self.file
                    || self.owner_library_of(hit.owner_file, *name) == Some(self.lib)
            })
            .map(|name| self.member_description(&representative[&name], name))
            .collect();
        self.emit_missing_members(descriptions, name_range);
    }

    /// The interface view for the completeness check: every resolved face's
    /// names (a non-final field carries its setter half), the first face in
    /// order supplying the representative hit, and the names a concrete
    /// source implements. `None` suppresses the whole check — a broken face
    /// chain, or the noSuchMethod forwarder gate: a concrete noSuchMethod on
    /// the implemented sources (own members, mixins, the super chain — never
    /// an `implements` face) implements every interface name.
    fn collect_interface(&mut self, decl: &Container) -> Option<InterfaceView<'a>> {
        let mut faces: Vec<(StringId, bool)> = Vec::new();
        faces.extend(decl.superclass.iter().map(|s| (*s, true)));
        faces.extend(decl.with_clause.iter().map(|m| (*m, true)));
        faces.extend(decl.implements_clause.iter().map(|i| (*i, false)));
        faces.extend(decl.on_clause.iter().map(|o| (*o, false)));
        let mut face_hits: Vec<Vec<FaceEntry<'a>>> = Vec::with_capacity(faces.len());
        for (face, contributes) in faces {
            face_hits.push(self.face_entries(face, contributes)?);
        }
        let no_such_method = self.object_members[3];
        let forwarder_gate = decl
            .members
            .get(&no_such_method)
            .is_some_and(|e| !e.is_static && !e.is_abstract)
            || face_hits
                .iter()
                .flatten()
                .any(|h| h.name == no_such_method && h.concrete_source && !h.entry.is_abstract);
        if forwarder_gate {
            return None;
        }
        let mut representative: FxHashMap<StringId, FaceEntry<'a>> = FxHashMap::default();
        let mut names: Vec<StringId> = Vec::new();
        let mut implemented: FxHashSet<StringId> = FxHashSet::default();
        for hits in &face_hits {
            let mut sorted = hits.clone();
            sorted.sort_unstable_by_key(|h| h.name);
            for hit in sorted {
                let setter_half = match hit.entry.kind {
                    MemberKind::Field if !hit.entry.is_final => {
                        Some(self.setter_spelling(hit.name))
                    }
                    _ => None,
                };
                if let std::collections::hash_map::Entry::Vacant(slot) =
                    representative.entry(hit.name)
                {
                    slot.insert(hit);
                    names.push(hit.name);
                }
                if let Some(setter) = setter_half
                    && let std::collections::hash_map::Entry::Vacant(slot) =
                        representative.entry(setter)
                {
                    slot.insert(hit);
                    names.push(setter);
                }
                if hit.concrete_source && !hit.entry.is_abstract {
                    implemented.insert(hit.name);
                    if let Some(setter) = setter_half {
                        implemented.insert(setter);
                    }
                }
            }
        }
        Some(InterfaceView {
            names,
            representative,
            implemented,
        })
    }

    /// The description text of one missing member: `Enclosing.member`, the
    /// official `setter ` prefix for setter spellings and none for methods.
    fn member_description(&self, hit: &FaceEntry<'a>, name: StringId) -> String {
        let owner = self.strings.resolve(hit.owner_name);
        let text = self.strings.resolve(name);
        if let Some(base) = text.strip_suffix('=') {
            format!("setter {owner}.{base}")
        } else if hit.entry.kind == MemberKind::Method {
            format!("{owner}.{text}")
        } else {
            format!("getter {owner}.{text}")
        }
    }

    /// The five-tier missing-implementation row: sorted descriptions select
    /// the official message shape.
    fn emit_missing_members(&mut self, mut descriptions: Vec<String>, name_range: SourceRange) {
        descriptions.sort();
        let code = match descriptions.len() {
            0 => return,
            1 => SemanticCode::InheritsAbstractMemberOne,
            2 => SemanticCode::InheritsAbstractMemberTwo,
            3 => SemanticCode::InheritsAbstractMemberThree,
            4 => SemanticCode::InheritsAbstractMemberFour,
            _ => SemanticCode::InheritsAbstractMemberFivePlus,
        };
        let message = match descriptions.len() {
            1 => code.message(&descriptions[0], ""),
            2 => code.message3(&descriptions[0], &descriptions[1], ""),
            3 => code.message3(&descriptions[0], &descriptions[1], &descriptions[2]),
            4 => code.message4(
                &descriptions[0],
                &descriptions[1],
                &descriptions[2],
                &descriptions[3],
            ),
            n => code.message5(
                &descriptions[0],
                &descriptions[1],
                &descriptions[2],
                &descriptions[3],
                &(n - 4).to_string(),
            ),
        };
        self.report.diagnostics.push(Diagnostic::new(
            code.severity(),
            code.code_info(),
            message,
            self.file,
            name_range,
            None,
        ));
    }

    // —— extension-type representation (#476) ————————————————————————————

    /// The semantic representation checks for one extension type, mounted at
    /// the declaration's name event (the element model is complete —
    /// `collect` parsed before this pass — and the name token anchors the
    /// type-level emissions; the official dispatch order is
    /// depends-on-itself first, the name checks last,
    /// `error_verifier.dart:1184-1193`).
    ///
    /// The syntactic representation shapes (missing formals, `this.x`/`super.x`,
    /// delimiters, function-typed suffixes, modifiers, multiple formals) are
    /// the parser's (`expected_representation_field` /
    /// `multiple_representation_fields` / `representation_field_modifier`);
    /// this side reads the surviving single representation formal.
    fn check_extension_type(&mut self, container: StringId, name_range: SourceRange) {
        // One emission per container per pass: an augmentation declaration
        // re-fires the event for the same container.
        if !self.reported_extension_types.insert(container) {
            return;
        }
        let Some(elements) = self.index.get(self.file) else {
            return;
        };
        let Some(decl) = elements.container(container) else {
            return;
        };
        if decl.kind != ContainerKind::ExtensionType {
            return;
        }
        let Some(rep) = decl.primary_formals.first() else {
            return;
        };
        // The representation can't depend on itself
        // (`error_verifier.dart:5343-5354`, anchored at the type name). M1
        // name-level: a direct `E` reference; a generic `List<E>`-shaped
        // dependency stays an under-report.
        if rep.formal.ty == Ty::named(container) {
            self.emit(
                SemanticCode::ExtensionTypeRepresentationDependsOnItself,
                name_range,
                "",
                "",
            );
        }
        // `_checkForExtensionTypeRepresentationErrorCodes`'s name checks
        // (:5411-5423, both non-returning): the field may not share the
        // type's name, nor an `Object` member's.
        if rep.formal.name == container {
            self.emit(SemanticCode::MemberWithClassName, rep.name_range, "", "");
        } else if self.object_members.contains(&rep.formal.name) {
            self.emit(
                SemanticCode::ExtensionTypeDeclaresMemberOfObject,
                rep.name_range,
                "",
                "",
            );
        }
        // The disabled-arm representation-type requirement
        // (:5449-5452): a typeless representation formal — alongside, not
        // instead of, the modifier check the parser owns.
        if rep.formal.ty == Ty::UNKNOWN && !self.primary_constructors_enabled {
            self.emit(
                SemanticCode::ExpectedRepresentationType,
                rep.name_range,
                "",
                "",
            );
        }
    }

    /// The nearest non-static `spelling` member on `face`'s linearized
    /// chain: the face container itself, then its `with` mixins in reverse
    /// application order (the official superclass face of
    /// `class C extends S with M1, M2` is the applied `S&M2` interface —
    /// `inheritance_override.dart:344-351` — so the climb sees M2 over M1
    /// over S), then direct-superclass name edges (each hop resolved
    /// file-locally first, then through the declaring container's library,
    /// #491 S2a; the visited set guards cycles). `None` = an unresolvable or
    /// cyclic hop — the caller suppresses rather than reports.
    fn face_member(
        &self,
        face: StringId,
        spelling: StringId,
    ) -> Option<(&'a MemberEntry, StringId)> {
        let mut name = face;
        let mut owner = self.file;
        let mut scope_lib = self.lib;
        let mut seen = FxHashSet::default();
        loop {
            if !seen.insert(name) {
                return None;
            }
            let (found, found_owner) = resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                scope_lib,
                owner,
                name,
            )?;
            if let Some(entry) = found.members.get(&spelling)
                && !entry.is_static
            {
                return Some((entry, name));
            }
            // The mixin/superclass names are written in `found_owner` and
            // resolve through that file's library (probed by the container
            // name it declares, #491 S2a).
            let owner_lib = self
                .owner_library_of(found_owner, name)
                .unwrap_or(scope_lib);
            for mixin in found.with_clause.iter().rev() {
                if !seen.insert(*mixin) {
                    return None;
                }
                let mixin_lib = owner_lib;
                if let Some((mixin_container, _)) = resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    mixin_lib,
                    found_owner,
                    *mixin,
                ) && let Some(entry) = mixin_container.members.get(&spelling)
                    && !entry.is_static
                {
                    return Some((entry, *mixin));
                }
            }
            let superclass = found.superclass?;
            scope_lib = owner_lib;
            name = superclass;
            owner = found_owner;
        }
    }

    /// The name-level signature verdict: `true` only when the equal-kind
    /// members are decuably incompatible — a positional-arity mismatch, a
    /// named-parameter set mismatch, or a nullability-decidable type
    /// conflict. An incomparable type pair ([`Ty::assignable_from`]'s
    /// `None`) suppresses: two distinct named types could still relate
    /// through an unseen subclass edge, and an untyped shape declares
    /// nothing to compare.
    fn override_broken(declared: &MemberEntry, inherited: &MemberEntry) -> bool {
        // A field's face is its declared type; every executable's is its
        // return type.
        let member_ty = |e: &MemberEntry| {
            if e.kind == MemberKind::Field {
                e.var_ty
            } else {
                e.return_ty
            }
        };
        match (declared.formals.as_deref(), inherited.formals.as_deref()) {
            (Some(a), Some(b)) => {
                // Positional arity first: both directions break it.
                if a.positional.len() != b.positional.len() {
                    return true;
                }
                // The named set: every inherited name must survive (a
                // dropped named parameter rejects calls the super accepts),
                // and `required` may only tighten onto a parameter that is
                // required in the super — a newly required parameter (added
                // or strengthened) rejects calls without it. An added
                // optional named parameter is the valid-override widening
                // (oracle-verified: `void f(int x, {int? y})` over
                // `void f(int x)` is clean).
                for f in &b.named {
                    match a.named_formal(f.name) {
                        None => return true,
                        Some(d) if d.required && !f.required => return true,
                        Some(_) => {}
                    }
                }
                if a.named
                    .iter()
                    .any(|d| d.required && b.named_formal(d.name).is_none())
                {
                    return true;
                }
                // Parameter types are contravariant: the declared parameter
                // must accept the inherited parameter's values.
                for (d, i) in a.positional.iter().zip(&b.positional) {
                    match d.ty.assignable_from(i.ty) {
                        Some(false) => return true,
                        Some(true) => {}
                        None => return false,
                    }
                }
                for d in &a.named {
                    let Some(i) = b.named_formal(d.name) else {
                        return false;
                    };
                    match d.ty.assignable_from(i.ty) {
                        Some(false) => return true,
                        Some(true) => {}
                        None => return false,
                    }
                }
            }
            (None, None) => {}
            // A parameter-list mix on equal kinds is recovery residue.
            _ => return false,
        }
        // The result type is covariant: the declared result must fit the
        // inherited one.
        matches!(
            member_ty(inherited).assignable_from(member_ty(declared)),
            Some(false)
        )
    }

    /// The member's name-level signature text (the message's type slots, in
    /// the oracle's function-type shape): nullary faces render
    /// `{ty} Function()`, methods render their parameter list, setters
    /// `void Function({ty})`. Absent declared types render `dynamic` — the
    /// implicit declared type the oracle would display.
    fn override_signature_text(&self, entry: &MemberEntry) -> String {
        match entry.kind {
            MemberKind::Method => {
                let params = match entry.formals.as_deref() {
                    Some(formals) => {
                        let mut parts: Vec<String> = formals
                            .positional
                            .iter()
                            .map(|f| self.declared_ty_text(f.ty))
                            .collect();
                        if !formals.named.is_empty() {
                            let named = formals
                                .named
                                .iter()
                                .map(|f| {
                                    let text = format!(
                                        "{} {}",
                                        self.declared_ty_text(f.ty),
                                        self.strings.resolve(f.name)
                                    );
                                    if f.required {
                                        format!("required {text}")
                                    } else {
                                        text
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join(", ");
                            parts.push(format!("{{{named}}}"));
                        }
                        parts.join(", ")
                    }
                    None => String::new(),
                };
                format!(
                    "{} Function({params})",
                    self.declared_ty_text(entry.return_ty)
                )
            }
            MemberKind::Getter | MemberKind::Field => {
                let ty = if entry.kind == MemberKind::Field {
                    entry.var_ty
                } else {
                    entry.return_ty
                };
                format!("{} Function()", self.declared_ty_text(ty))
            }
            MemberKind::Setter => {
                let ty = entry
                    .formals
                    .as_deref()
                    .and_then(|f| f.positional.first())
                    .map_or(Ty::UNKNOWN, |p| p.ty);
                format!("void Function({})", self.declared_ty_text(ty))
            }
        }
    }

    /// The declared-type text as the oracle renders it: `dynamic` for an
    /// absent type (the implicit declared type), `Null`/`void` keep their
    /// spellings, a named type gains the syntactic `?`. Unlike
    /// [`Engine::ty_display`] (whose unknowns stay empty for its
    /// suppression-shaped callers), an override signature always names its
    /// types.
    fn declared_ty_text(&self, ty: Ty) -> String {
        match ty {
            Ty::Unknown | Ty::Dynamic => "dynamic".to_string(),
            Ty::Null => "Null".to_string(),
            Ty::Void => "void".to_string(),
            Ty::Named { name, nullable } => {
                let text = self.strings.resolve(name);
                if nullable {
                    format!("{text}?")
                } else {
                    text.to_string()
                }
            }
        }
    }

    // —— row stack ———————————————————————————————————————————————————

    fn push_row(&mut self, row: Row) {
        self.rows.push(row);
    }

    /// A bare identifier in value position: resolved immediately (the
    /// verdict is the snapshot pass 2 emits from). A miss under `new`/`const`
    /// is a creation head: it names a class, so the guarded miss is the
    /// oracle's `undefined_class` (`new Missing()`), never an invocation
    /// read — the row reclassifies as a type use.
    fn push_bare(&mut self, name: StringId, range: SourceRange) {
        let info = self.resolve_bare(name);
        // The usage cluster's local-use capture: a bare read that resolves
        // into the frame stack counts for the declaration it lands on.
        // (A local function's own name is absent from the tracked stack
        // while its body parses, so in-body self-references never count —
        // the official `_enclosingExec` exclusion.) The final intent may
        // still cancel it (write-only, statement-discarded reads).
        if let Some(hit) = self.resolve_local(name) {
            let hit = hit as usize;
            self.locals[hit].uses += 1;
            self.local_hits.insert(range.offset, hit as u32);
        }
        // The official self-reference exclusions (`_useIdentifierElement`):
        // a reference to the enclosing container or the enclosing executable
        // itself never marks that declaration used. The row offset joins the
        // skip set so the resolve-time marking obeys the same rule.
        if self.exec_self_name() == Some(name) || self.container_context().map_or(false, |(c, _)| c == name)
        {
            self.self_scope_rows.insert(range.offset);
        }
        // A bare mention of a field name inside its own container: the
        // final intent decides read vs write at resolve time.
        if let Some((container, _)) = self.container_context()
            && self
                .index
                .get(self.file)
                .and_then(|f| f.container(container))
                .is_some_and(|c| {
                    c.members
                        .get(&name)
                        .is_some_and(|m| m.kind == MemberKind::Field)
                })
        {
            self.field_read_rows.insert(range.offset);
        }
        if self.ctor_pos > 0 && matches!(info, BareInfo::Missing { .. }) {
            self.push_row(Row {
                name,
                range,
                qualified: false,
                shape: RowShape::TypeUse(TypeVerdict::Missing, false),
                intent: Intent::Read,
                write_op: None,
                ty: Ty::UNKNOWN,
            });
            return;
        }
        let ty = match info {
            BareInfo::Plain { ty } if ty != Ty::UNKNOWN => ty,
            // #279 type folding: an in-file const top-level variable's
            // declared type folds to its evaluated constant's type (the
            // oracle infers the variable's type from the initializer). A
            // shadowing local keeps its own (already-folded) type.
            BareInfo::Plain { .. } => {
                let from_local = self.frames.iter().any(|f| f.locals.contains(&name));
                let is_top_var = self
                    .top_level_decl(name)
                    .is_some_and(|d| d.kind == MicroKind::TopLevelVar);
                if !from_local && is_top_var {
                    self.const_top_level_ty(name).unwrap_or(Ty::UNKNOWN)
                } else {
                    Ty::UNKNOWN
                }
            }
            _ => Ty::UNKNOWN,
        };
        self.push_row(Row {
            name,
            range,
            qualified: false,
            shape: RowShape::Bare(info),
            intent: Intent::Read,
            write_op: None,
            ty,
        });
    }

    /// An identifier after a dot, awaiting the merge at the dot event.
    fn push_member(&mut self, name: StringId, range: SourceRange) {
        self.push_row(Row {
            name,
            range,
            qualified: true,
            shape: RowShape::Bare(BareInfo::Plain { ty: Ty::UNKNOWN }),
            intent: Intent::Read,
            write_op: None,
            ty: Ty::UNKNOWN,
        });
    }

    /// A resolved non-receiver value (literal, parenthesized group, closure,
    /// collection): member merges past it see an unknown receiver. The row
    /// carries the construct's span so the assignment/operator scans can
    /// tell "before the operator" (candidate) from "after" (skip).
    fn push_value_row(&mut self, token: Token) {
        self.push_row(Row {
            name: self.intern(""),
            range: token.range,
            qualified: false,
            shape: RowShape::Value,
            intent: Intent::Read,
            write_op: None,
            ty: Ty::UNKNOWN,
        });
    }

    /// The `null` literal: the only literal whose type M1 knows exactly.
    fn push_null_row(&mut self, token: Token) {
        self.push_row(Row {
            name: self.intern(""),
            range: token.range,
            qualified: false,
            shape: RowShape::Value,
            intent: Intent::Read,
            write_op: None,
            ty: Ty::Null,
        });
    }

    /// `this`/`super` in expression position: a typed receiver when a
    /// container is in scope, unknown otherwise (top-level recovery).
    fn push_keyword_receiver(&mut self, token: Token, is_super: bool) {
        let recv = match self.container_context() {
            Some((container, kind)) => {
                if is_super {
                    Receiver::Super { container, kind }
                } else {
                    Receiver::This { container, kind }
                }
            }
            None => Receiver::Unknown,
        };
        self.push_row(Row {
            name: self.intern(""),
            range: token.range,
            qualified: false,
            shape: RowShape::Receiver(recv),
            intent: Intent::Read,
            write_op: None,
            ty: Ty::UNKNOWN,
        });
    }

    /// A type-annotation position. Defined types push no row; the §8 names
    /// table carries misses only. The declaration binding arms at
    /// `handle_type` now — this only classifies the reference.
    fn push_type_use(&mut self, name: StringId, range: SourceRange, head: TypeHead) {
        let suppressed = self.non_type_pos > 0;
        match head {
            TypeHead::Bare => {
                if self.builtin_types.contains(&name) {
                    return;
                }
                if !matches!(self.resolve_bare(name), BareInfo::Missing { .. }) {
                    // A defined type reference uses the type (the official
                    // `visitNamedType` → `_useIdentifierElement` face) —
                    // except inside an `is` type operand, where the oracle
                    // deliberately does not count it
                    // (`_useIdentifierElement`'s `_insideIsExpression`
                    // early return), and except a self-reference from
                    // inside the declaration itself (`_enclosingClass` /
                    // `_enclosingExec`).
                    if self.is_type_pos == 0 && !self.in_own_named_scope(name) {
                        self.used_names.insert(name);
                    }
                    return;
                }
                self.push_row(Row {
                    name,
                    range,
                    qualified: false,
                    shape: RowShape::TypeUse(TypeVerdict::Missing, suppressed),
                    intent: Intent::Read,
                    write_op: None,
                    ty: Ty::UNKNOWN,
                });
            }
            TypeHead::Continuation => {
                let Some(tp) = self.type_prefix.as_ref() else {
                    return;
                };
                if !tp.live {
                    return; // the head already reports; the name is dead
                }
                let prefix = tp.prefix;
                let hit = self.prefixed_hit(prefix, name);
                if !hit {
                    self.push_row(Row {
                        name,
                        range,
                        qualified: true,
                        shape: RowShape::TypeUse(TypeVerdict::Prefixed { prefix }, false),
                        intent: Intent::Read,
                        write_op: None,
                        ty: Ty::UNKNOWN,
                    });
                } else if self.is_type_pos == 0 && !self.in_own_named_scope(name) {
                    self.used_names.insert(name);
                }
            }
        }
    }

    /// Arms the type-prefix slot for a `PrefixedTypeReference` head; a miss
    /// also pushes the head's own report row. A live prefix head counts as
    /// a prefix use (the `unused_import` prefix face).
    fn arm_type_prefix(&mut self, name: StringId, range: SourceRange) {
        let live = !matches!(self.resolve_bare(name), BareInfo::Missing { .. });
        if live {
            self.used_names.insert(name);
        }
        self.type_prefix = Some(TypePrefix { prefix: name, live });
        if !live {
            self.push_row(Row {
                name,
                range,
                qualified: false,
                shape: RowShape::TypeUse(TypeVerdict::PrefixHead, false),
                intent: Intent::Read,
                write_op: None,
                ty: Ty::UNKNOWN,
            });
        }
    }

    /// Merges `receiver.member` at the dot/cascade event: pops the member row,
    /// classifies the receiver below it, and pushes the merged row. A
    /// null-aware access on a strictly non-nullable receiver emits
    /// `invalid_null_aware_operator` (a `STATIC_WARNING` — the receiver can
    /// never be null, `type_system.dart:1921-`).
    fn merge_member(&mut self, is_null_aware: bool) {
        let Some(member) = self.rows.pop() else {
            return;
        };
        let receiver_offset = self.rows.last().map(|r| r.range.offset);
        let recv = match self.rows.pop() {
            Some(popped) => {
                // The receiver's read counts for the local it resolved to
                // (`v.m` reads `v`): the member row inherits the hit.
                if let Some(hit) = self.local_hits.remove(&popped.range.offset) {
                    self.local_hits.insert(member.range.offset, hit);
                }
                // The receiver's own reference marks its name used (the
                // official `members` face, and `unused_import`'s
                // hasPrefixUsedWithoutName face for a prefix receiver):
                // the row folds into the member row, so pass 2 never
                // resolves it.
                if matches!(popped.shape, RowShape::Bare(_))
                    && !self.self_scope_rows.contains(&popped.range.offset)
                {
                    self.used_names.insert(popped.name);
                }
                match popped.shape {
                    RowShape::Bare(info) => Engine::receiver_of_bare(info),
                    RowShape::Receiver(recv) => recv,
                    _ => Receiver::Unknown,
                }
            }
            None => Receiver::Unknown,
        };
        if is_null_aware {
            let receiver_ty = match recv {
                Receiver::This { container, .. } | Receiver::Super { container, .. } => {
                    Ty::named(container)
                }
                Receiver::Typed { ty, .. } => ty,
                _ => Ty::UNKNOWN,
            };
            if receiver_ty.is_strictly_non_nullable() {
                let (op, replacement): (&'static str, &'static str) = if self.in_cascade {
                    ("?..", "..")
                } else {
                    ("?.", ".")
                };
                self.emit3(
                    SemanticCode::InvalidNullAwareOperator,
                    member.range,
                    op,
                    replacement,
                    "",
                );
            }
        }
        // `super.named()` in a constructor-initializer list is a super
        // *constructor* reference, never a member access: the named shape
        // checks the superclass's constructor face
        // (`undefined_constructor_in_initializer`), and the row folds —
        // pass 2's `super.member` dispatch must never see it. The selector
        // name is the report's second slot; the anchor spans the
        // `super.named()` shape the oracle reports.
        if self.initializer_depth > 0
            && let Receiver::Super { container, .. } = recv
        {
            let span = SourceRange {
                offset: receiver_offset.unwrap_or(member.range.offset),
                length: member.range.end() - receiver_offset.unwrap_or(member.range.offset),
            };
            self.check_super_constructor(container, &member, span);
            self.push_row(Row {
                name: member.name,
                range: member.range,
                qualified: true,
                shape: RowShape::Value,
                intent: Intent::Read,
                write_op: None,
                ty: Ty::UNKNOWN,
            });
            return;
        }
        self.push_row(Row {
            name: member.name,
            range: member.range,
            qualified: true,
            shape: RowShape::Member { recv },
            intent: member.intent,
            write_op: member.write_op,
            ty: Ty::UNKNOWN,
        });
    }

    /// The super-constructor selector in an initializer list
    /// (`error_verifier.dart`, the `SuperConstructorInvocation` resolution):
    /// the direct superclass's named constructor set decides. An
    /// unresolvable superclass (external, `on`-constraint chain) suppresses.
    fn check_super_constructor(&mut self, container: StringId, member: &Row, span: SourceRange) {
        let Some(superclass) = self
            .index
            .get(self.file)
            .and_then(|f| f.container(container))
            .and_then(|c| c.superclass)
        else {
            return;
        };
        let Some((super_container, _)) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            superclass,
        ) else {
            return;
        };
        if super_container.constructors.contains(&member.name) {
            return;
        }
        let class_text = self.strings.resolve(superclass).to_string();
        let name_text = self.strings.resolve(member.name).to_string();
        self.emit(
            SemanticCode::UndefinedConstructorInInitializer,
            span,
            &class_text,
            &name_text,
        );
    }

    /// Opens an argument group: the top row is its callee — unless the group
    /// belongs to a `new`/`const`/implicit-creation reference, whose class
    /// name resolves as a type use, never an invocation.
    fn open_arguments(&mut self) {
        // Inside a creation window the head row tells the two lists apart:
        // the creation's own list heads with the container row (no formals
        // to mark), while the trailing member invocation's list
        // (`C().m(1, 2)`) heads with the member row and must join the
        // given-count face.
        let callee = if self.ctor_pos > 0 {
            self.callee_index().filter(|&i| {
                matches!(
                    self.rows.get(i).map(|r| &r.shape),
                    Some(RowShape::Member { .. })
                )
            })
        } else {
            self.callee_index()
        };
        self.invokes.push(callee);
        let mark = self.rows.len();
        self.arg_levels.push(ArgLevel {
            args: Vec::new(),
            arg_open: false,
            pending_name: None,
            mark,
        });
    }

    /// Closes an argument group; its callee awaits the send event. The
    /// collected argument types check against the callee's formals first
    /// (`argument_type_not_assignable`, decided on declared types only).
    /// `positional` is the parser's total argument count feeding the
    /// given-at-call index.
    fn close_arguments(&mut self, positional: u16) {
        self.flush_unclaimed();
        self.flush_argument();
        let levels = self.arg_levels.pop();
        self.check_arguments(levels, positional);
        self.pending_send = self.invokes.pop().flatten();
    }

    /// Starts a new argument in the open argument list (the row window the
    /// argument owns closes at the next [`Engine::flush_argument`]).
    fn open_argument(&mut self, name: Option<StringId>) {
        self.flush_unclaimed();
        self.flush_argument();
        if let Some(level) = self.arg_levels.last_mut() {
            level.arg_open = true;
            level.pending_name = name;
        }
    }

    /// Recovers the unclaimed window `[mark, rows.len())` as a positional
    /// argument. The normal expression path fires no per-argument events
    /// (only the const path emits `positional_argument`), so a group whose
    /// window holds exactly one row since `mark` reads that row as the
    /// single positional argument; wider windows stay unattributed
    /// (compound operands must not lend their type — under-report).
    fn flush_unclaimed(&mut self) {
        let Some(level) = self.arg_levels.last_mut() else {
            return;
        };
        if level.arg_open {
            return;
        }
        let hi = self.rows.len();
        if hi == level.mark {
            return;
        }
        if let Some(row) = self.rows.get(level.mark).filter(|_| hi == level.mark + 1) {
            let literal = Self::primitive_literal(row, &self.primitive_ids);
            level.args.push((None, row.ty, literal, row.range));
        }
        level.mark = hi;
    }

    /// Closes the pending argument: exactly one pushed row since the last
    /// argument boundary is a terminal expression whose type stands for the
    /// whole argument; anything else stays `Unknown` (compound operands must
    /// not lend their type). An empty window pushes nothing.
    fn flush_argument(&mut self) {
        let Some(level) = self.arg_levels.last_mut() else {
            return;
        };
        if !level.arg_open {
            return;
        }
        level.arg_open = false;
        let name = level.pending_name.take();
        let hi = self.rows.len();
        if hi == level.mark {
            return;
        }
        let (ty, literal, range) = match self.rows.get(level.mark) {
            Some(row) if hi == level.mark + 1 => {
                let literal = Self::primitive_literal(row, &self.primitive_ids);
                (row.ty, literal, row.range)
            }
            _ => (
                Ty::UNKNOWN,
                false,
                SourceRange {
                    offset: 0,
                    length: 0,
                },
            ),
        };
        level.mark = hi;
        level.args.push((name, ty, literal, range));
    }

    /// Pairs the closed argument list against the callee's declared formals:
    /// positional by index, named by spelling. Extra positionals are the
    /// `extra_positional_arguments` family's business (unfrozen — skipped);
    /// unresolved callees suppress.
    fn check_arguments(&mut self, level: Option<ArgLevel>, positional: u16) {
        let Some(level) = level else { return };
        let Some(callee) = self.invokes.last().and_then(|slot| *slot) else {
            return;
        };
        let Some(row) = self.rows.get(callee) else {
            return;
        };
        // The usage cluster's given-at-call index (the
        // `_addParametersForArguments` face) keys by callee spelling alone
        // and must not wait on the formals: a receiver this face cannot
        // type (a creation-folded `C().m(...)` head) still supplies its
        // arguments. The parser's count is exact where the row recovery is
        // not: wide positional windows (compound operands) stay
        // unattributed above, so the positional supply is the total count
        // less the attributed named entries. Callee-spelling keys — a
        // same-named member elsewhere suppresses, never false-positives.
        let callee_spelling = row.name;
        let named = level
            .args
            .iter()
            .filter(|(name, ..)| name.is_some())
            .count() as u16;
        if positional > named {
            let given = self.positional_given.entry(callee_spelling).or_default();
            *given = (*given).max(positional - named);
        }
        // Cloned: the assignability path below reads the table while the
        // named-argument marking mutates it.
        let formals: Option<Formals> = match &row.shape {
            // A top-level function's declared formals.
            RowShape::Bare(BareInfo::Plain { .. }) => self
                .top_level_decl(row.name)
                .and_then(|d| d.formals.as_deref().cloned()),
            RowShape::Member { recv } => self.member_formals_full(recv, row.name).cloned(),
            _ => None,
        };
        let Some(formals) = formals else {
            return;
        };
        let mut positional_index = 0usize;
        let mut mismatches: Vec<(Ty, Ty, SourceRange)> = Vec::new();
        for (name, ty, literal, range) in &level.args {
            let formal = if let Some(n) = name {
                formals.named_formal(*n)
            } else {
                let formal = formals.positional.get(positional_index);
                positional_index += 1;
                formal
            };
            if let Some(n) = name {
                self.named_given.insert((callee_spelling, *n));
            }
            let Some(formal) = formal else {
                continue;
            };
            if self.assignable(*ty, formal.ty, *literal) == Some(false) {
                mismatches.push((*ty, formal.ty, *range));
            }
        }
        for (from, to, range) in mismatches {
            let (a0, a1) = (self.ty_display(from), self.ty_display(to));
            self.emit3(SemanticCode::ArgumentTypeNotAssignable, range, &a0, &a1, "");
        }
    }

    /// Consumes a completed send: the closed group's callee row becomes an
    /// invocation. A send without arguments never opened a group, so a bare
    /// read stays a read (`foo;` vs `foo()`). An explicit type-argument group
    /// on the callee checks its arity (`_element`, or `_extension` for an
    /// extension override).
    ///
    /// A container-name callee (`E('x')` the explicit extension application,
    /// `C()` the implicit creation) folds into a receiver row carrying the
    /// container: the argument rows belong to the send (truncated away), and
    /// a following `.member` / operator merge must resolve against the
    /// container's face, not the last argument's row. Member callees
    /// (`C.named()`, methods) are not folds — their own rows stand.
    fn consume_send(&mut self) {
        let Some(callee) = self.pending_send.take() else {
            return;
        };
        if let Some(row) = self.rows.get_mut(callee)
            && row.intent == Intent::Read
        {
            row.intent = Intent::Invoke;
        }
        if let Some(pos) = self
            .deferred_args
            .iter()
            .rposition(|(row, _, _)| *row == callee)
        {
            let (_, count, range) = self.deferred_args.remove(pos);
            self.check_invocation_arity(callee, count, range);
        }
        if matches!(
            self.rows.get(callee).map(|r| r.shape.clone()),
            Some(RowShape::Bare(BareInfo::Container { .. }))
        ) {
            self.rows.truncate(callee + 1);
            if let Some(row) = self.rows.get_mut(callee)
                && let RowShape::Bare(BareInfo::Container { name, kind }) = row.shape
            {
                // The creation's head reference uses the class (the official
                // `visitNamedType`/identifier face): the row folds into a
                // receiver in place, so pass 2 never resolves it.
                if !self.self_scope_rows.contains(&row.range.offset) {
                    self.used_names.insert(name);
                }
                *row = Row {
                    name,
                    range: row.range,
                    qualified: false,
                    shape: RowShape::Receiver(Receiver::Created { name, kind }),
                    intent: Intent::Read,
                    write_op: None,
                    ty: Ty::UNKNOWN,
                };
            }
        }
    }

    /// An implicit creation ended (`g<int>(...)`): the oracle rewrites the
    /// shape to a plain invocation when the element resolves to a function
    /// (`ast_rewrite.dart`), so a deferred bare-function head checks its
    /// arity here (`_element`). Class/extension heads already checked at
    /// their group.
    fn end_implicit_creation(&mut self) {
        if self.ctor_pos != 1 {
            return;
        }
        let Some((row, count, range)) = self.deferred_args.pop() else {
            return;
        };
        let Some(head) = self.rows.get(row) else {
            return;
        };
        // A plain function head (`g<int>(...)`) — the oracle rewrites the
        // shape to a plain invocation (`ast_rewrite.dart`) — and a member
        // head reached through the creation syntax (`C.sm<int, String>(...)`,
        // the `id.id<...>(` form) both check as the `_element` invocation.
        // Class and extension heads checked at their type-argument group.
        if matches!(
            head.shape,
            RowShape::Bare(BareInfo::Plain { .. }) | RowShape::Member { .. }
        ) {
            self.check_invocation_arity(row, count, range);
        }
    }

    /// The invoked-callee arity check (`invocation_inferrer.dart` reports
    /// through the oracle's target dispatch, `type_instantiation_target.
    /// dart`): a resolved executable invoked with explicit type arguments
    /// reports `_element`, naming its kind word — a top-level function is
    /// `function`, a member method is `method`. Function-typed variables and
    /// getters resolve to type displays M1 cannot render (the oracle's
    /// `_function` / kind-`getter` rows) — no declared count, suppress.
    fn check_invocation_arity(&mut self, callee: usize, count: u16, range: SourceRange) {
        let Some(row) = self.rows.get(callee) else {
            return;
        };
        let name = row.name;
        let qualified = row.qualified;
        let shape = row.shape.clone();
        if let RowShape::Bare(BareInfo::Container {
            name,
            kind: ContainerKind::Extension,
        }) = &shape
        {
            // An explicit extension override reached through the send
            // (`Ex<int>(list)`) — the group's deferred check dispatches by
            // shape here (`extension_member_resolver.dart:380`).
            return self.check_extension_arity(*name, count, range);
        }
        let declared = match &shape {
            // The wave-3 member shape: the send closes before the
            // dot-access event merges `receiver.name`, so the callee is an
            // unmerged member placeholder (qualified, unresolved) with the
            // receiver row directly below — classify the receiver and walk
            // its members (`c.m<D>()` reports `m`).
            RowShape::Bare(BareInfo::Plain { .. }) if qualified => {
                let recv = callee.checked_sub(1).and_then(|i| self.rows.get(i)).map_or(
                    Receiver::Unknown,
                    |below| match &below.shape {
                        RowShape::Bare(info) => Engine::receiver_of_bare(*info),
                        RowShape::Receiver(recv) => *recv,
                        _ => Receiver::Unknown,
                    },
                );
                self.member_exec_arity(&recv, name)
            }
            // A top-level function (`g<int>()`); only real functions carry a
            // parameter list — getters stay silent.
            RowShape::Bare(BareInfo::Plain { .. }) => self
                .top_level_decl(name)
                .filter(|d| d.kind == MicroKind::TopLevelFn && d.formals.is_some())
                .map(|d| (d.type_params, ExecKind::Function)),
            RowShape::Member { recv } => self.member_exec_arity(recv, name),
            _ => None,
        };
        if let Some((declared, kind)) = declared
            && declared != count
        {
            let name_text = self.strings.resolve(name).to_string();
            self.emit4(
                SemanticCode::WrongNumberOfTypeArgumentsElement(kind),
                range,
                kind.word(),
                &name_text,
                &declared.to_string(),
                &count.to_string(),
            );
        }
    }

    /// A resolved container's declared type-parameter count (`None` when the
    /// name does not resolve — external types suppress).
    fn declared_type_params_of_container(&self, name: StringId) -> Option<u16> {
        resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        )
        .map(|(c, _)| c.type_params)
    }

    /// A member callee's declared type-parameter count with its oracle kind
    /// word: methods only — getters, setters, and fields resolve to oracle
    /// shapes M1 does not model, so they suppress (`None`).
    fn member_exec_arity(&self, recv: &Receiver, name: StringId) -> Option<(u16, ExecKind)> {
        let (container, super_walk) = match *recv {
            Receiver::This { container, .. } => (container, false),
            Receiver::Super { container, .. } => (container, true),
            Receiver::Typed {
                type_name: container,
                ..
            }
            | Receiver::Created {
                name: container, ..
            }
            | Receiver::Container {
                name: container, ..
            } => (container, false),
            _ => return None,
        };
        let walk = if super_walk {
            self.walk_super(container, name)
        } else {
            self.walk_members(container, name)
        };
        match walk {
            Walk::Found(entry) if entry.kind == MemberKind::Method => {
                Some((entry.type_params, ExecKind::Method))
            }
            _ => None,
        }
    }

    /// The type-argument application event (`g<int>;`, `c<int>;`): a
    /// resolved executable tear-off reports `_element` naming its kind word
    /// (`function_reference_resolver.dart` → the
    /// `InvocationTargetExecutableElement` dispatch); a type literal reports
    /// the base code; an implicit call reference reports `_element` naming
    /// `call`. Getters and function-typed variables resolve to oracle shapes
    /// M1 cannot name (the getter kind word, the `_function` type display) —
    /// suppress.
    fn consume_type_argument_application(&mut self) {
        // The application applies to the most recent expression group —
        // its head row was resolved at `end_type_arguments` through the
        // group's base mark, so the group's own `TypeUse` rows above it
        // never hide the entry (LIFO, as in [`Engine::end_implicit_creation`]).
        let Some((row_index, count, range)) = self.deferred_args.pop() else {
            return;
        };
        let Some(row) = self.rows.get(row_index) else {
            return;
        };
        match &row.shape {
            RowShape::Bare(BareInfo::Container { name, .. }) => {
                // Type-literal instantiation `C<int>`: the base code.
                self.check_annotation_arity(*name, count, range);
            }
            RowShape::Bare(BareInfo::Plain { ty }) => {
                // A named top-level function tear-off, or an implicit call
                // reference through a declaration-typed receiver (`c<int>`
                // checks `call`).
                let named_fn = self
                    .top_level_decl(row.name)
                    .filter(|d| d.kind == MicroKind::TopLevelFn && d.formals.is_some())
                    .map(|d| (d.type_params, ExecKind::Function));
                if let Some((declared, kind)) = named_fn {
                    self.emit_element_arity(row.name, declared, kind, count, range);
                } else if let Ty::Named { name, .. } = *ty
                    && let Some(entry) = self.resolved_member(name, self.intern("call"))
                    && entry.kind == MemberKind::Method
                {
                    self.emit_element_arity(
                        self.intern("call"),
                        entry.type_params,
                        ExecKind::Method,
                        count,
                        range,
                    );
                }
            }
            RowShape::Member { recv } => {
                // A property-access tear-off (`C.sm<int, String>;`): the
                // oracle's `InvocationTargetFunctionTypedExpression` reports
                // `_function` leading with the function type — rendered from
                // the captured signature. A method tear-off is the only
                // function-typed shape the model can render: getters, fields,
                // and unknown receivers suppress. A name that is no member at
                // all is the constructor-reference shape `C<...>.named` — the
                // base type-argument code against the container.
                let container = match *recv {
                    Receiver::This { container, .. }
                    | Receiver::Super { container, .. }
                    | Receiver::Typed {
                        type_name: container,
                        ..
                    }
                    | Receiver::Container {
                        name: container, ..
                    } => Some(container),
                    _ => None,
                };
                let Some(container) = container else {
                    return;
                };
                match self.walk_members(container, row.name) {
                    Walk::Found(entry) if entry.kind == MemberKind::Method => {
                        if entry.type_params != count {
                            let display = self.function_type_display(entry);
                            self.emit3(
                                SemanticCode::WrongNumberOfTypeArgumentsFunction,
                                range,
                                &display,
                                &entry.type_params.to_string(),
                                &count.to_string(),
                            );
                        }
                    }
                    // Non-method finds (getter/field tear-offs) render to
                    // oracle shapes the model cannot name; a broken chain is
                    // opaque — both suppress.
                    Walk::Found(_) | Walk::ChainBroken => {}
                    Walk::Missing => self.check_annotation_arity(container, count, range),
                }
            }
            _ => {}
        }
    }

    /// The `_function` message's function-type display from a captured
    /// method signature: `{return} Function<{type params}>({formal types})`
    /// — `T m<T>(T x)` renders `T Function<T>(T)`, the oracle's rendering of
    /// the generic instantiation's uninstantiated type.
    fn function_type_display(&self, entry: &crate::model::MemberEntry) -> String {
        let ret = self.ty_display(entry.return_ty);
        let params = entry
            .type_param_names
            .iter()
            .map(|p| self.strings.resolve(*p))
            .collect::<Vec<_>>()
            .join(", ");
        let formals = entry
            .formals
            .as_ref()
            .map(|f| {
                f.positional
                    .iter()
                    .map(|formal| self.ty_display(formal.ty))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let group = if params.is_empty() {
            String::new()
        } else {
            format!("<{params}>")
        };
        format!("{ret} Function{group}({formals})")
    }

    /// The `_element` emit: kind word, executable name, declared vs given.
    fn emit_element_arity(
        &mut self,
        name: StringId,
        declared: u16,
        kind: ExecKind,
        count: u16,
        range: SourceRange,
    ) {
        if declared != count {
            let name_text = self.strings.resolve(name).to_string();
            self.emit4(
                SemanticCode::WrongNumberOfTypeArgumentsElement(kind),
                range,
                kind.word(),
                &name_text,
                &declared.to_string(),
                &count.to_string(),
            );
        }
    }

    /// The resolved member entry of `container.member` from a clean walk.
    fn resolved_member(
        &self,
        container: StringId,
        member: StringId,
    ) -> Option<&crate::model::MemberEntry> {
        match self.walk_members(container, member) {
            Walk::Found(entry) => Some(entry),
            _ => None,
        }
    }

    /// The annotation/creation arity check: the head's declared
    /// type-parameter count against the given count (`dynamic` and
    /// in-scope type parameters declare none; unresolved and external heads
    /// suppress).
    fn check_annotation_arity(&mut self, name: StringId, count: u16, range: SourceRange) {
        let Some(declared) = self.declared_type_params(name) else {
            return;
        };
        if declared != count {
            let name_text = self.strings.resolve(name).to_string();
            self.emit3(
                SemanticCode::WrongNumberOfTypeArguments,
                range,
                &name_text,
                &declared.to_string(),
                &count.to_string(),
            );
        }
    }

    /// A type head's declared type-parameter count: `dynamic` and type
    /// parameters declare none, a container keeps its own count, a
    /// typedef/function keeps its declaration's; anything unresolved is
    /// `None` (suppress — `undefined_class` already reports).
    fn declared_type_params(&self, name: StringId) -> Option<u16> {
        if self.builtin_types.contains(&name) || self.type_params.contains(&name) {
            return Some(0);
        }
        match self.resolve_bare(name) {
            BareInfo::Container { name, .. } => resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                name,
            )
            .map(|(c, _)| c.type_params),
            BareInfo::Plain { .. } => self.top_level_decl(name).map(|d| d.type_params),
            _ => None,
        }
    }

    /// The `end_type_arguments` dispatch — the code choice by use-site shape.
    fn end_type_arguments(&mut self, count: usize, begin: Token, end: Token) {
        let count = u16::try_from(count).unwrap_or(u16::MAX);
        let range = SourceRange {
            offset: begin.range.offset,
            length: end.range.end() - begin.range.offset,
        };
        // A type head makes the group an annotation or a creation type
        // reference.
        let (head, base) = self.group_heads.pop().unwrap_or((None, 0));
        if let Some((head, _head_range)) = head {
            self.check_annotation_arity(head, count, range);
            return;
        }
        // An enum constant's constructor arguments (`a<int>()`).
        if let Some(declared) = self.in_enum_element
            && declared != count
        {
            self.emit3(
                SemanticCode::WrongNumberOfTypeArgumentsEnum,
                range,
                &declared.to_string(),
                &count.to_string(),
                "",
            );
            return;
        }
        // The wave-3 constructor-reference shape, checked before the row
        // stack: reference heads fire as `ConstructorReference` type uses
        // (a resolved name pushes no row, so the stack may be empty) — the
        // reference's own group matches through `creation_ref`. `C<int>`
        // checks the class face (base code); `C.named<int>` reports
        // `_constructor` for any count (constructors never declare type
        // parameters; the class face must resolve — a missing class already
        // reports `undefined_class`).
        if self.ctor_pos > 0
            && let Some((class, ctor)) = self.creation_ref
        {
            match ctor {
                Some(ctor) => {
                    if !matches!(self.resolve_bare(class), BareInfo::Missing { .. }) {
                        let class = self.strings.resolve(class).to_string();
                        let ctor = self.strings.resolve(ctor).to_string();
                        self.emit3(
                            SemanticCode::WrongNumberOfTypeArgumentsConstructor,
                            range,
                            &class,
                            &ctor,
                            "",
                        );
                    }
                }
                None => self.check_annotation_arity(class, count, range),
            }
            return;
        }
        let Some(callee) = self.rows.len().checked_sub(1) else {
            return;
        };
        if self.ctor_pos > 0 {
            // A creation reference head. `parse_constructor_reference` keeps
            // its segments unmerged on the row stack: `C<int>` is a bare
            // container row (base code), `C.named<int>` is two bare rows —
            // the class face then the constructor segment (any count
            // reports: constructors never declare type parameters).
            let named_ctor = (callee >= 1)
                .then(|| {
                    let ctor = &self.rows[callee];
                    let class = &self.rows[callee - 1];
                    match (&class.shape, &ctor.shape) {
                        (
                            RowShape::Bare(BareInfo::Container {
                                name: class_name,
                                kind: ContainerKind::Class,
                            }),
                            RowShape::Bare(_),
                        ) => Some((*class_name, ctor.name)),
                        _ => None,
                    }
                })
                .flatten();
            if let Some((class, ctor)) = named_ctor {
                let class = self.strings.resolve(class).to_string();
                let ctor = self.strings.resolve(ctor).to_string();
                self.emit3(
                    SemanticCode::WrongNumberOfTypeArgumentsConstructor,
                    range,
                    &class,
                    &ctor,
                    "",
                );
                return;
            }
            let row = &self.rows[callee];
            match &row.shape {
                RowShape::Bare(BareInfo::Container {
                    name,
                    kind: ContainerKind::Extension,
                }) => {
                    // Explicit extension override `Ext<int>(list)` — any
                    // arity mismatch reports at the group
                    // (`extension_member_resolver.dart:380`).
                    self.check_extension_arity(*name, count, range);
                }
                RowShape::Bare(BareInfo::Container { name, .. }) => {
                    let name = *name;
                    self.check_annotation_arity(name, count, range);
                }
                _ => {
                    // A function invoked through the implicit-creation
                    // syntax resolves at the send; defer.
                    self.deferred_args.push((callee, count, range));
                }
            }
            return;
        }
        // The head row the group applies to is the last row pushed before
        // `<` (`base - 1`); rows pushed inside the group are its own type
        // arguments — unresolved ones are `TypeUse` rows. An unresolved head
        // pushes its own `TypeUse` row, which the deferred checks then
        // ignore; a row from an older expression never matches.
        let Some(target) = base.checked_sub(1).and_then(|i| self.rows.get(i)) else {
            return;
        };
        if matches!(target.shape, RowShape::Bare(_) | RowShape::Member { .. }) {
            self.deferred_args.push((base - 1, count, range));
        }
    }

    /// The explicit extension override's type-argument check
    /// (`extension_member_resolver.dart:380`): any arity mismatch reports
    /// `_extension` at the group.
    fn check_extension_arity(&mut self, name: StringId, count: u16, range: SourceRange) {
        if let Some(declared) = self.declared_type_params_of_container(name)
            && declared != count
        {
            let name_text = self.strings.resolve(name).to_string();
            self.emit3(
                SemanticCode::WrongNumberOfTypeArgumentsExtension,
                range,
                &name_text,
                &declared.to_string(),
                &count.to_string(),
            );
        }
    }

    /// The receiver classification of a bare row's info.
    fn receiver_of_bare(info: BareInfo) -> Receiver {
        match info {
            BareInfo::Container { name, kind } => Receiver::Container { name, kind },
            BareInfo::Prefix { name } => Receiver::Prefix { name },
            BareInfo::Plain { ty } => match ty {
                Ty::Named {
                    name: type_name, ..
                } => Receiver::Typed { type_name, ty },
                _ => Receiver::Unknown,
            },
            // The receiver itself is undefined; its own row reports, so the
            // member access must not.
            BareInfo::Missing { .. } => Receiver::Unknown,
        }
    }

    /// Arms the pending type head: the identifier just parsed a type
    /// annotation and may be followed by a type-argument group.
    fn arm_type_head(&mut self, name: StringId, range: SourceRange) {
        self.pending_type_head = Some((name, range));
    }

    /// The expression type of an already-pushed row shaped as a receiver or
    /// value (`a?[i]`'s receiver check).
    fn receiver_row_ty(row: &Row) -> Ty {
        match &row.shape {
            RowShape::Bare(info) => match info {
                BareInfo::Plain { ty } => *ty,
                BareInfo::Container { name, .. } => Ty::named(*name),
                _ => Ty::UNKNOWN,
            },
            RowShape::Receiver(recv) => match recv {
                Receiver::This { container, .. } | Receiver::Super { container, .. } => {
                    Ty::named(*container)
                }
                Receiver::Typed { ty, .. } => *ty,
                _ => Ty::UNKNOWN,
            },
            _ => Ty::UNKNOWN,
        }
    }

    /// Marks the assignment target: the closest row wholly before the
    /// operator, skipping folded value rows (index results, groups). Bare and
    /// member rows upgrade their intent; index-operator rows record the write
    /// form (`c[i] = v` writes through `[]=`, never the index argument).
    fn mark_assignment(&mut self, operator: Token) {
        let Some(op_text) = self.text_of(operator) else {
            return;
        };
        let limit = operator.range.offset;
        let compound = op_text != "=";
        let index_op = self.intern("[]");
        for row in self.rows.iter_mut().rev() {
            if row.range.end() > limit {
                continue;
            }
            if matches!(row.shape, RowShape::Value) {
                continue;
            }
            match &row.intent {
                Intent::Read => {
                    // A write alone never uses a local variable (the
                    // official `_useAssignmentTargetElement` early return);
                    // the plain-write hit cancels.
                    if !compound
                        && let Some(hit) = self.local_hits.remove(&row.range.offset)
                    {
                        self.locals[hit as usize].uses -= 1;
                    }
                    row.intent = if compound {
                        Intent::ReadWrite
                    } else {
                        Intent::Write
                    };
                }
                Intent::Operator(op, _) if *op == index_op => {
                    row.write_op = Some(if compound {
                        OpWrite::Compound
                    } else {
                        OpWrite::Assign
                    });
                }
                _ => {}
            }
            return;
        }
    }

    /// Marks an operator operand: the closest row wholly before `op`, when it
    /// is a bare identifier, a typed receiver, or a member access (`a + b`,
    /// `this + b`, `a.b + c` — the operator applies to the member result).
    fn mark_operator(&mut self, op: StringId, range: SourceRange) {
        for row in self.rows.iter_mut().rev() {
            if row.range.end() > range.offset {
                continue;
            }
            if matches!(
                row.shape,
                RowShape::Bare(_) | RowShape::Receiver(_) | RowShape::Member { .. }
            ) && row.intent == Intent::Read
            {
                row.intent = Intent::Operator(op, range);
            }
            return;
        }
    }

    /// The top row index when it can be an invocation callee.
    fn callee_index(&self) -> Option<usize> {
        let index = self.rows.len().checked_sub(1)?;
        matches!(
            self.rows[index].shape,
            RowShape::Bare(_) | RowShape::Member { .. }
        )
        .then_some(index)
    }

    /// The declared type of the assignment target row (`x = e`, `obj.f = e`):
    /// a bare name reads its snapshot, a member walks to the field's declared
    /// type; executables and unknown receivers stay `Unknown`.
    fn assignment_target_ty(&self, row: &Row) -> Ty {
        match &row.shape {
            RowShape::Bare(BareInfo::Plain { ty }) => *ty,
            RowShape::Bare(BareInfo::Container { name, kind }) => {
                // A container name is never a valid assignment target — the
                // oracle reports assignment-to-`this`-prefix shapes with
                // other codes; suppress here.
                let _ = (name, kind);
                Ty::UNKNOWN
            }
            RowShape::Member { recv } => match recv {
                Receiver::This { container, .. } | Receiver::Super { container, .. } => {
                    self.member_declared_ty(*container, row.name)
                }
                Receiver::Typed { type_name, .. } => self.member_declared_ty(*type_name, row.name),
                Receiver::Container {
                    name: container, ..
                } => resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    *container,
                )
                .and_then(|(c, _)| c.members.get(&row.name))
                .filter(|e| e.kind == MemberKind::Field)
                .map_or(Ty::UNKNOWN, |e| e.var_ty),
                _ => Ty::UNKNOWN,
            },
            _ => Ty::UNKNOWN,
        }
    }

    /// The assignment check (`invalid_assignment`,
    /// `assignment_expression_resolver.dart:795`): a single-row right-hand
    /// side whose type the M1 facts know (`Null` or a captured declared
    /// type) against the target's declared type. Plain `=` only — compound
    /// operators carry their own read semantics (under-report).
    fn check_assignment(&mut self, operator: Token) {
        let Some(op_text) = self.text_of(operator) else {
            return;
        };
        if op_text != "=" {
            return;
        }
        let limit = operator.range.offset;
        // The target: the closest row wholly before the operator, skipping
        // folded value rows (the same scan `mark_assignment` runs).
        let Some(target) = self
            .rows
            .iter()
            .enumerate()
            .rev()
            .find(|(_, row)| row.range.end() <= limit && !matches!(row.shape, RowShape::Value))
            .map(|(index, _)| index)
        else {
            return;
        };
        // `c[i] = e` writes through the `[]=` operator: the target's own
        // declared type is not the assignment's destination type.
        if matches!(self.rows[target].intent, Intent::Operator(..)) {
            return;
        }
        // The right-hand side: exactly one row strictly after the operator
        // (rows are pushed in source order, so the first post-operator row
        // and the absence of a second decide without collecting).
        let mut rhs = self.rows[target + 1..]
            .iter()
            .filter(|row| row.range.offset > limit);
        let Some(rhs_row) = rhs.next() else {
            return;
        };
        if rhs.next().is_some() {
            return;
        }
        let (from, range) = (rhs_row.ty, rhs_row.range);
        let literal = Self::primitive_literal(rhs_row, &self.primitive_ids);
        let to = self.assignment_target_ty(&self.rows[target]);
        if self.assignable(from, to, literal) == Some(false) {
            let (a0, a1) = (self.ty_display(from), self.ty_display(to));
            self.emit3(SemanticCode::InvalidAssignment, range, &a0, &a1, "");
        }
    }

    /// The variable-declaration initializer check
    /// (`variable_declaration_resolver.dart:140`): the same decision as the
    /// assignment check against the declaration group's declared type.
    fn check_decl_initializer(&mut self) {
        let Some(to) = self.pending_decl_ty else {
            return;
        };
        let Some(mark) = self.init_mark else {
            return;
        };
        if self.rows.len() != mark + 1 {
            return;
        }
        let row = &self.rows[mark];
        let literal = Self::primitive_literal(row, &self.primitive_ids);
        if self.assignable(row.ty, to, literal) == Some(false) {
            let (a0, a1) = (self.ty_display(row.ty), self.ty_display(to));
            self.emit3(SemanticCode::InvalidAssignment, row.range, &a0, &a1, "");
        }
        self.init_mark = None;
    }

    /// The return check (`rtv.dart:209-249` sync matrix): `void` hosts
    /// accept `void`/`dynamic`/`Null`; `void`/`dynamic` expressions only
    /// fit `void`/`dynamic` hosts; everything else defers to the
    /// assignability decision (the return expression is the literal's
    /// context, so `from_literal` feeds the conversion — `double f() => 1;`
    /// is clean). Bare `return;` is a `void` expression.
    fn check_return(&mut self, from: Ty, range: SourceRange, from_literal: bool) {
        let Some(ret) = self.frames.iter().rev().find_map(|f| match &f.kind {
            FrameKind::Executable { ret: Some(ret), .. } => Some((ret.ty, ret.host, ret.name)),
            _ => None,
        }) else {
            return;
        };
        let (to, host, name) = ret;
        let verdict = match to {
            Ty::Void => match from {
                Ty::Void | Ty::Dynamic | Ty::Null => Some(true),
                Ty::Unknown => None,
                _ => Some(false),
            },
            Ty::Unknown => None,
            _ => match from {
                Ty::Void => Some(!matches!(to, Ty::Void | Ty::Dynamic)),
                Ty::Dynamic => Some(true),
                _ => self.assignable(from, to, from_literal),
            },
        };
        if verdict == Some(false) {
            let (a0, a1, a2) = (
                self.ty_display(from),
                self.ty_display(to),
                self.strings.resolve(name).to_string(),
            );
            self.emit3(
                SemanticCode::ReturnOfInvalidType(host),
                range,
                &a0,
                &a1,
                &a2,
            );
        }
    }

    /// A single-row expression's type since `mark`, for the body/return
    /// checks, with the row's primitive-literal flag (the conversion's
    /// source-side fact); `None` when the expression is compound.
    fn single_row_since(&self, mark: usize) -> Option<(Ty, bool, SourceRange)> {
        let row = self.rows.get(mark)?;
        (self.rows.len() == mark + 1).then_some((
            row.ty,
            Self::primitive_literal(row, &self.primitive_ids),
            row.range,
        ))
    }

    /// Enters an enum constant: its constructor type arguments check against
    /// the enclosing enum's declared type-parameter count.
    fn enter_enum_element(&mut self) {
        self.in_enum_element = self
            .container_context()
            .filter(|(_, kind)| *kind == ContainerKind::Enum)
            .and_then(|(name, _)| self.declared_type_params_of_container(name));
    }

    fn leave_enum_element(&mut self) {
        self.in_enum_element = None;
    }

    // —— pattern/switch machinery (M2 face) ————————————————————————

    /// Appends a leaf to the innermost open pattern frame; outside any
    /// pattern (pattern-assignment RHS, if-case roots) the leaf is dropped
    /// — only switch exhaustiveness consumes case spaces.
    fn pattern_leaf(&mut self, leaf: Restriction) {
        if let Some(frame) = self.pattern_stack.last_mut() {
            frame.leaves.push(leaf);
        }
    }

    /// Collapses one frame's leaves into the restriction it covers
    /// (`||` unions, `&&` intersects, a bare leaf passes through, a
    /// type-free pattern covers its context).
    fn collapse(leaves: Vec<Restriction>, or_ops: usize) -> Restriction {
        if leaves.is_empty() {
            // A type-free variable/wildcard: the context full space. The
            // context is substituted when the case closes.
            return Restriction::NamedContext;
        }
        if or_ops > 0 {
            return Restriction::Union(leaves);
        }
        // `&&` joins (or a bare single leaf) intersect at name level.
        leaves
            .into_iter()
            .reduce(Restriction::and)
            .unwrap_or(Restriction::Unknown)
    }

    /// Closes the case-header pattern window and drains its root: the
    /// topmost restriction is the header pattern's coverage (pushed between
    /// the window's open and here), everything below is debris from earlier
    /// bodies. `None` — a legacy or recovered header with no pattern —
    /// falls back to the unknown space.
    fn close_case_sink(&mut self) -> Option<Restriction> {
        self.case_sink_open = false;
        let root = self.pattern_roots.pop();
        self.pattern_roots.clear();
        root
    }

    /// Substitutes a captured root's type-free coverage with the
    /// scrutinee's context type.
    fn substitute_root(&self, root: Option<Restriction>) -> Restriction {
        let space = root.unwrap_or(Restriction::Unknown);
        if space == Restriction::NamedContext {
            return match NameTy::from_semantic(
                self.switches.last().map_or(Ty::Unknown, |c| c.scrutinee),
            ) {
                NameTy::Named(name, nullable) => Restriction::Named(name, nullable),
                _ => Restriction::Unknown,
            };
        }
        space
    }

    /// The name-level exhaustiveness world over this unit's element model.
    /// `'a` (not `'_`): the world borrows only the engine's long-lived
    /// index/string-table references, so it can outlive `&mut self` uses.
    fn name_world(&self) -> NameWorld<'a> {
        NameWorld {
            index: self.index,
            file: self.file,
            strings: self.strings,
            bool_id: self.intern("bool"),
            unknowns: std::cell::Cell::new(0),
        }
    }

    /// Closes an object pattern (the parser fires `handle_object_pattern`
    /// *after* the rest — fields and boundaries included): the row lowers
    /// to the bare head type exactly when every field's drained restriction
    /// covers its member's whole column ([`NameWorld::property_type`], the
    /// member-table walk, superclass chain included). Any residual narrows
    /// the row below the name level's expressible spaces, so the row is
    /// the unknown space (the established under-report). `head` is `None`
    /// for the unmodeled `Type.field` shorthand shapes.
    fn close_object_fields(&mut self, head: Option<StringId>, fields: Vec<ObjectField>) {
        let Some(ty) = head else {
            self.pattern_leaf(Restriction::Unknown);
            return;
        };
        let world = self.name_world();
        let covered = fields.iter().all(|field| match field.leaves.as_slice() {
            // A type-free sub-pattern (`var x`, `_`, the `:x` shorthand)
            // covers the whole column.
            [] => true,
            // A conjunctive residual is not one name-level row.
            [leaf] => match field.name {
                Some(name) => {
                    let Restriction::Named(lt, ln) = leaf else {
                        return false;
                    };
                    world.member_column(ty, name).is_some_and(|column| {
                        world.is_subtype_of(&column, &NameTy::Named(*lt, *ln))
                    })
                }
                // Positional fields: the member table keeps no declaration
                // order, so a constraining leaf is unattributable.
                None => false,
            },
            _ => false,
        });
        let leaf = if covered {
            Restriction::Named(ty, false)
        } else {
            Restriction::Unknown
        };
        self.pattern_leaf(leaf);
    }

    /// Opens a switch context; the scrutinee's push-time row is the last
    /// one on the row stack when the block opens. No row — the shape a
    /// complex scrutinee leaves — latches `Ty::Unknown`, and
    /// [`Self::report_switch`] skips the entire check for that (the
    /// oracle's `InvalidType` gate), so the latch gap suppresses instead
    /// of mis-answering.
    fn open_switch(&mut self, is_expression: bool, kw: SourceRange) {
        let scrutinee = self.rows.last().map_or(Ty::Unknown, |row| row.ty);
        // Any roots surviving from before this switch are debris (a broken
        // header's un-drained capture): they must never feed this switch's
        // cases.
        self.pattern_roots.clear();
        self.case_sink_open = false;
        self.switches.push(SwitchCtx {
            is_expression,
            switch_kw: kw,
            scrutinee,
            cases: Vec::new(),
            default_kw: None,
            pending_root: None,
        });
    }

    /// Computes the exhaustiveness verdict and emits the switch
    /// diagnostics (per-case reachability, then the covering gap).
    fn report_switch(&mut self) {
        let Some(ctx) = self.switches.pop() else {
            return;
        };
        // An unresolvable scrutinee is the oracle's `InvalidType`
        // (constant_verifier.dart:1035): the entire exhaustiveness pass —
        // reachability and covering alike — gates off behind
        // `hasInvalidType` (:1095). Mapping it to the nullable top would
        // drop every case row and false-positive the covering gap. (The
        // push-time latch leaves `Unknown` exactly here: no row, or a row
        // whose declared type never resolved — the "complex scrutinee"
        // blind spot, now a clean whole-check suppression.)
        if ctx.scrutinee == Ty::Unknown {
            return;
        }
        let world = self.name_world();
        let value = NameTy::from_semantic(ctx.scrutinee);
        let cases: Vec<Case<NameTy>> = ctx
            .cases
            .iter()
            .map(|c| Case {
                space: case_space(&world, &c.space, value),
                guarded: c.guarded,
            })
            .collect();
        let report = compute(&world, &value, &cases);
        let unreachable: Vec<SourceRange> = report
            .unreachable
            .iter()
            .filter_map(|index| ctx.cases.get(*index).map(|c| c.anchor))
            .collect();
        let (ty_text, unmatched, suggested) = match &report.exhaustiveness {
            Exhaustiveness::NonExhaustive(witnesses) => {
                let first = witnesses.first();
                (
                    self.ty_display(ctx.scrutinee),
                    first.map_or(String::new(), |w: &EngineWitness<NameTy>| {
                        w.as_witness(&world)
                    }),
                    first.map_or(String::new(), |w| w.as_correction(&world)),
                )
            }
            Exhaustiveness::Exhaustive => (String::new(), String::new(), String::new()),
        };
        // The oracle reports reachability per case first
        // (constant_verifier.dart:1080-1088), then the covering gap
        // (:1122-1140) at the `switch` keyword.
        for anchor in unreachable {
            self.emit(SemanticCode::UnreachableSwitchCase, anchor, "", "");
        }
        // A cross-file scrutinee name is opaque to the file-scoped
        // world ([`NameWorld::name_resolvable`]): its sealedness and
        // subclass set are unknowable in-file, so the covering verdict
        // may be falsely negative and the expression face — always
        // must-be-exhaustive — suppresses (under-report direction).
        // The statement face is already gated on in-file sealed facts.
        let expression_face_decides = match value {
            NameTy::Named(name, _) => world.name_resolvable(name),
            _ => true,
        };
        let must_be_exhaustive = (ctx.is_expression && expression_face_decides)
            || Self::is_always_exhaustive(&world, value);
        if let Exhaustiveness::NonExhaustive(_) = report.exhaustiveness {
            if must_be_exhaustive && ctx.default_kw.is_none() {
                let code = if ctx.is_expression {
                    SemanticCode::NonExhaustiveSwitchExpression
                } else {
                    SemanticCode::NonExhaustiveSwitchStatement
                };
                // The oracle row carries a correctionMessage whose `{2}` is
                // the first witness's correction text (diagnostic.g.dart
                // :13018/:13079, constant_verifier.dart:1104-1120); the
                // diagnostic carries it in the correction slot.
                self.report.diagnostics.push(Diagnostic::new(
                    code.severity(),
                    code.code_info(),
                    code.message3(&ty_text, &unmatched, &suggested),
                    self.file,
                    ctx.switch_kw,
                    code.correction_message(&suggested),
                ));
            }
        } else if must_be_exhaustive && let Some(default_at) = ctx.default_kw {
            // The cases already cover the scrutinee, so the `default`
            // clause can never match (`nonExhaustiveness == null &&
            // defaultNode != null && mustBeExhaustive`,
            // constant_verifier.dart:1141-1148).
            self.emit(SemanticCode::UnreachableSwitchDefault, default_at, "", "");
        }
    }

    /// `isAlwaysExhaustive` (type_system.dart:846-887) at name level:
    /// `bool`, `Null`, enums, and sealed classes split completely; nothing
    /// else does. The official check reads the scrutinee type's *element*
    /// face (`isDartCoreBool`/enum/`isSealed`) and is nullability-agnostic
    /// — `S?` of a sealed `S` stays always-exhaustive, the null half being
    /// the space model's nullable split rather than this gate (`dynamic`
    /// has no element and stays `false`).
    fn is_always_exhaustive(world: &NameWorld<'_>, ty: NameTy) -> bool {
        match ty {
            NameTy::Null => true,
            NameTy::Named(name, _) => world.is_sealed_name(name),
            _ => false,
        }
    }

    fn ty_display(&self, ty: Ty) -> String {
        match ty {
            Ty::Unknown => String::new(),
            Ty::Null => "Null".to_string(),
            Ty::Dynamic => "dynamic".to_string(),
            Ty::Void => "void".to_string(),
            Ty::Named { name, nullable } => {
                let text = self.strings.resolve(name);
                if nullable {
                    format!("{text}?")
                } else {
                    text.to_string()
                }
            }
        }
    }

    /// The M1 assignability decision ([`Ty::assignable_from`] plus the
    /// final-primitive table): a value typed by one final primitive never
    /// fits another — the types cannot be extended, implemented, or mixed
    /// in, so no subclass relation can close the gap, whatever declaration
    /// the names resolve to. `int`/`double` fit `num` (the one supertype
    /// edge). `int` → `double` is the literal conversion
    /// (`_fe_analyzer_shared/lib/src/type_inference/type_analyzer.dart`,
    /// `analyzeIntLiteral` → `convertToDouble`: an integer literal whose
    /// context schema `int` does not satisfy but `double` does is typed
    /// `double`), so `from_literal` — whether the source row is a
    /// primitive literal — decides: `double d = 1;` is clean, `double d =
    /// intVar;` reports (a non-literal expression keeps type `int`; the
    /// conversion keys on the literal, never the value). `None` suppresses
    /// (under-report).
    fn assignable(&self, from: Ty, to: Ty, from_literal: bool) -> Option<bool> {
        if let Some(verdict) = to.assignable_from(from) {
            return Some(verdict);
        }
        let (
            Ty::Named {
                name: from_name,
                nullable: false,
            },
            Ty::Named {
                name: to_name,
                nullable: false,
            },
        ) = (from, to)
        else {
            return None;
        };
        if from_name == to_name
            || !self.primitive_ids.contains(&from_name)
            || !self.primitive_ids.contains(&to_name)
        {
            return None;
        }
        let (int, double, num) = (
            self.primitive_ids[0],
            self.primitive_ids[1],
            self.primitive_ids[2],
        );
        if (from_name == int || from_name == double) && to_name == num {
            return Some(true);
        }
        if from_name == int && to_name == double {
            // `analyzeIntLiteral`'s `convertToDouble`: the literal converts,
            // the value does not.
            return Some(from_literal);
        }
        Some(false)
    }

    /// A primitive-literal row: [`Engine::push_primitive_row`] is the only
    /// `RowShape::Value` creator that carries a known final-primitive type
    /// (every other `Value` push holds `Unknown` or `Null`), so the shape
    /// plus a non-nullable final-primitive type identifies the int/double/
    /// bool/String literal rows the conversion decides on.
    fn primitive_literal(row: &Row, primitive_ids: &[StringId; 5]) -> bool {
        let Ty::Named {
            name,
            nullable: false,
        } = row.ty
        else {
            return false;
        };
        matches!(row.shape, RowShape::Value) && primitive_ids.contains(&name)
    }

    /// The enum-constant constructor selector (`c.other()` in the
    /// element's own position): the enclosing enum's constructor face
    /// decides (`parseEnumElement`, the `ConstructorReferenceContinuation`
    /// resolution). Unresolvable enum containers suppress.
    fn check_enum_constructor_selector(&mut self, name: StringId, range: SourceRange) {
        let Some((enum_name, _)) = self.container_context() else {
            return;
        };
        let Some((container, _)) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            enum_name,
        ) else {
            return;
        };
        if container.constructors.contains(&name) {
            return;
        }
        let name_text = self.strings.resolve(name).to_string();
        self.emit(
            SemanticCode::UndefinedEnumConstructor,
            range,
            &name_text,
            "",
        );
    }

    /// A simple annotation name (`@foo`): the oracle's annotation resolution
    /// needs a class (or a `foo.name` constant face); a fully missing name is
    /// `undefined_annotation` anchored over the `@` (3.13.1 caliber — the
    /// diagnostic was removed upstream in 3.14, `m1_codes.txt` freezes the
    /// measured form). A [`CORE_TOP_LEVEL_ANNOTATIONS`] name is defined by
    /// the implicit dart:core import, never missing. Defined names stay
    /// silent: the class/constructor second-part checks sit outside the M1
    /// model (under-report).
    fn check_annotation(&mut self, name: StringId, range: SourceRange) {
        let missing = matches!(self.resolve_bare(name), BareInfo::Missing { .. })
            && !CORE_TOP_LEVEL_ANNOTATIONS.contains(&self.strings.resolve(name));
        let at = self.annotation_at.take();
        if missing && !self.guard.ignore(None, name) {
            let span = SourceRange {
                offset: at.map_or(range.offset, |at| at.offset),
                length: range.end() - at.map_or(range.offset, |at| at.offset),
            };
            let name_text = self.strings.resolve(name).to_string();
            self.emit(SemanticCode::UndefinedAnnotation, span, &name_text, "");
        }
        if missing {
            self.record_miss(name, range, false, NameUse::Annotation);
        }
    }

    /// A literal whose type is a final primitive (`push_value_row` plus the
    /// type the static-nullability checks decide on).
    fn push_primitive_row(&mut self, token: Token, primitive: usize) {
        self.push_row(Row {
            name: self.intern(""),
            range: token.range,
            qualified: false,
            shape: RowShape::Value,
            intent: Intent::Read,
            write_op: None,
            ty: Ty::named(self.primitive_ids[primitive]),
        });
    }

    /// The named argument membership test against the active callee
    /// (`type_analyzer.dart:5820`): a resolved callee whose formal table
    /// lacks the label reports `undefined_named_parameter` at the label; an
    /// unresolved callee stays silent (its own invocation code reports) and
    /// only feeds the §8 table.
    fn check_named_argument(&mut self, name: StringId, range: SourceRange) {
        let Some(callee) = self.invokes.last().and_then(|slot| *slot) else {
            return;
        };
        let (shape, callee_name) = match self.rows.get(callee) {
            Some(row) => (row.shape.clone(), row.name),
            None => return,
        };
        let (resolved, defined) = match &shape {
            // The membership test exists only where the oracle's does: a real
            // top-level function's formal table. A class callee is a
            // constructor invocation (a constructor accepting the label is
            // silent — constructors sit outside the M1 model) and a getter
            // callee is a property access; both stay unresolved, mirroring
            // the arity path's filter.
            RowShape::Bare(_) => {
                let decl = self
                    .top_level_decl(callee_name)
                    .filter(|d| d.kind == MicroKind::TopLevelFn && d.formals.is_some());
                let defined =
                    decl.is_some_and(|d| Self::has_named_formal(d.formals.as_deref(), name));
                (decl.is_some(), defined)
            }
            RowShape::Member { recv } => {
                let formals = self.member_formals_full(recv, callee_name);
                (
                    formals.is_some(),
                    formals.is_some_and(|f| f.named.iter().any(|entry| entry.name == name)),
                )
            }
            _ => return,
        };
        if resolved && !defined {
            let text = self.strings.resolve(name).to_string();
            self.emit(SemanticCode::UndefinedNamedParameter, range, &text, "");
        }
        if !defined {
            // The §8 table carries every real miss (the membership test
            // itself is live).
            self.record_miss(name, range, false, NameUse::NamedArgument);
        }
    }

    // —— pass 2 ———————————————————————————————————————————————————————

    /// Resolves the recorded rows in source order and emits.
    fn resolve_rows(&mut self) {
        let rows = std::mem::take(&mut self.rows);
        for row in &rows {
            match &row.shape {
                RowShape::Bare(info) => self.resolve_bare_row(row, *info),
                RowShape::Member { recv } => self.resolve_member_row(row, *recv),
                RowShape::Receiver(recv) => self.resolve_receiver_row(row, *recv),
                RowShape::TypeUse(..) => self.resolve_type_row(row),
                RowShape::Value => {}
            }
        }
    }

    /// A surviving typed-keyword receiver (`this + b`, `super[i]`): only
    /// operator intents carry checks — member merges consumed every other
    /// shape at the dot event.
    fn resolve_receiver_row(&mut self, row: &Row, recv: Receiver) {
        if !matches!(row.intent, Intent::Operator(..)) {
            return;
        }
        let (read, write) = self.operator_spellings(row);
        match recv {
            Receiver::This { container, .. } => {
                let mut missing: Vec<String> = Vec::new();
                for spelling in [read, write].into_iter().flatten() {
                    if matches!(self.walk_members(container, spelling), Walk::Missing) {
                        missing.push(self.strings.resolve(spelling).to_string());
                    }
                }
                if missing.is_empty() {
                    return;
                }
                let type_text = self.strings.resolve(container).to_string();
                for text in missing {
                    self.emit(
                        SemanticCode::UndefinedOperator,
                        row.range,
                        &text,
                        &type_text,
                    );
                }
            }
            Receiver::Super { container, .. } => {
                let mut missing: Vec<String> = Vec::new();
                for spelling in [read, write].into_iter().flatten() {
                    if matches!(self.walk_super(container, spelling), Walk::Missing) {
                        missing.push(self.strings.resolve(spelling).to_string());
                    }
                }
                if missing.is_empty() {
                    return;
                }
                let type_text = self.strings.resolve(container).to_string();
                for text in missing {
                    self.emit(
                        SemanticCode::UndefinedSuperMember(SuperShape::Operator),
                        row.range,
                        &text,
                        &type_text,
                    );
                }
            }
            // A folded creation/extension-override receiver (`C() + 1`,
            // `E(1) * 2`): the operator resolves against the container's
            // member face — extensions report their dedicated operator row.
            Receiver::Created { name, kind } => {
                let Intent::Operator(_, op_range) = row.intent else {
                    return;
                };
                let mut missing: Vec<String> = Vec::new();
                for spelling in [read, write].into_iter().flatten() {
                    if matches!(self.walk_members(name, spelling), Walk::Missing) {
                        missing.push(self.strings.resolve(spelling).to_string());
                    }
                }
                if missing.is_empty() {
                    return;
                }
                let host_text = self.strings.resolve(name).to_string();
                for text in missing {
                    let code = if kind == ContainerKind::Extension {
                        SemanticCode::UndefinedExtensionOperator
                    } else {
                        SemanticCode::UndefinedOperator
                    };
                    self.emit(code, op_range, &text, &host_text);
                }
                self.record_miss(name, op_range, true, NameUse::Read);
            }
            _ => {}
        }
    }

    fn resolve_bare_row(&mut self, row: &Row, info: BareInfo) {
        let name_text = self.strings.resolve(row.name);
        match info {
            // A prefix used bare: invoking a prefix is an error the oracle
            // names with an unfrozen code — record only. The bare prefix
            // reference still counts as a use of the prefix (`unused_import`
            // hasPrefixUsedWithoutName face).
            BareInfo::Container { .. } | BareInfo::Prefix { .. } => {
                if !self.self_scope_rows.contains(&row.range.offset) {
                    self.used_names.insert(row.name);
                }
            }
            BareInfo::Plain { ty } => {
                // Any resolved reference uses the name it resolved to (the
                // official `_useIdentifierElement` / `members` faces; writes
                // included — only the local-variable rows distinguish). The
                // parse-time self-reference exclusions skip their rows.
                if !self.self_scope_rows.contains(&row.range.offset) {
                    self.used_names.insert(row.name);
                }
                // A non-write resolution of a field name is the implicit
                // getter's read (writes never spare a field).
                if !matches!(row.intent, Intent::Write)
                    && self.field_read_rows.contains(&row.range.offset)
                {
                    self.read_names.insert(row.name);
                }
                // A direct operator application on a declared-typed receiver
                // (`c + d`, `c[i] = v`).
                if matches!(row.intent, Intent::Operator(..)) {
                    self.check_operator_expr(ty, row);
                }
            }
            BareInfo::Missing {
                this_type,
                in_exec,
                on_opaque,
            } => {
                // Mixin `on` constraints and extension `on` types are
                // invisible to the model: inside their instance members a
                // bare miss may be a constraint member — record only.
                if on_opaque {
                    self.record_miss(row.name, row.range, false, use_kind_of(&row.intent));
                    return;
                }
                match row.intent {
                    Intent::Invoke => {
                        // UnqualifiedLookupDomain: inside a class or enum
                        // instance member the invocation miss names `this`'s
                        // type (`undefined_method`); everywhere else it is
                        // `undefined_function`.
                        if let Some(container) = this_type {
                            let type_text = self.strings.resolve(container).to_string();
                            if !self.guard.ignore(None, row.name) {
                                self.emit(
                                    SemanticCode::UndefinedMethod,
                                    row.range,
                                    name_text,
                                    &type_text,
                                );
                            }
                        } else if !self.guard.ignore(None, row.name) {
                            self.emit(SemanticCode::UndefinedFunction, row.range, name_text, "");
                        }
                        self.record_miss(row.name, row.range, false, NameUse::Invoke);
                    }
                    Intent::Read if row.name == self.intern("await") && in_exec => {
                        // `await` in a sync body is never a lookup miss: the
                        // frozen code names the missing async marker.
                        if !self.guard.ignore(None, row.name) {
                            self.emit(SemanticCode::UndefinedIdentifierAwait, row.range, "", "");
                        }
                        self.record_miss(row.name, row.range, false, NameUse::Read);
                    }
                    Intent::Read | Intent::Write | Intent::ReadWrite => {
                        // A write to a defined getter-only name is under-reported
                        // (the oracle's setter code needs a receiver type); a
                        // fully missing name reports.
                        if !self.guard.ignore(None, row.name) {
                            self.emit(SemanticCode::UndefinedIdentifier, row.range, name_text, "");
                        }
                        let use_kind = match row.intent {
                            Intent::Write | Intent::ReadWrite => NameUse::Write,
                            _ => NameUse::Read,
                        };
                        self.record_miss(row.name, row.range, false, use_kind);
                    }
                    Intent::Operator(..) => {
                        // The undefined receiver itself reports above all; the
                        // operator on an invalid type never cascades.
                        self.record_miss(row.name, row.range, false, NameUse::Read);
                    }
                }
            }
        }
    }

    /// Emits a type-position row from its push-time verdict snapshot.
    fn resolve_type_row(&mut self, row: &Row) {
        let RowShape::TypeUse(verdict, suppressed) = row.shape else {
            return;
        };
        let name_text = self.strings.resolve(row.name);
        match verdict {
            TypeVerdict::PrefixHead => {
                if !self.guard.ignore(None, row.name) {
                    self.emit(SemanticCode::UndefinedIdentifier, row.range, name_text, "");
                }
                self.record_miss(row.name, row.range, false, NameUse::Read);
            }
            TypeVerdict::Prefixed { prefix } => {
                if !self.guard.ignore(Some(prefix), row.name) {
                    self.emit(SemanticCode::UndefinedClass, row.range, name_text, "");
                }
                self.record_miss(row.name, row.range, true, NameUse::Type);
            }
            TypeVerdict::Missing => {
                if !suppressed && !self.guard.ignore(None, row.name) {
                    // A declaration parse keeps `await` in the type position:
                    // the unresolved simple name carries the await code, not
                    // `undefined_class` (named_type_resolver.dart:814-818 —
                    // `importPrefix == null && lexeme == 'await'` fires
                    // `undefinedIdentifierAwait` at the NamedType and
                    // returns before the `undefinedClass` fallthrough; the
                    // message names `await` itself, no arguments).
                    if row.name == self.intern("await") {
                        self.emit(SemanticCode::UndefinedIdentifierAwait, row.range, "", "");
                    } else {
                        self.emit(SemanticCode::UndefinedClass, row.range, name_text, "");
                    }
                }
                self.record_miss(row.name, row.range, false, NameUse::Type);
            }
        }
    }

    fn resolve_member_row(&mut self, row: &Row, recv: Receiver) {
        // Any member access uses the accessed name (the official `members`
        // set; unresolved reads land in `unresolvedReadMembers`, which the
        // field rule credits the same). Read-shaped accesses also join the
        // field-read index — writes never spare a field. A row that named
        // the enclosing executable itself never marks (the official
        // `_enclosingExec` exclusion).
        let self_named = self.self_scope_rows.contains(&row.range.offset);
        if !self_named {
            self.used_names.insert(row.name);
        }
        if !self_named && matches!(row.intent, Intent::Read | Intent::ReadWrite) {
            self.read_names.insert(row.name);
        }
        let name_text = self.strings.resolve(row.name);
        match recv {
            Receiver::Unknown => {}
            Receiver::Prefix { name: prefix } => {
                if self.prefixed_hit(prefix, row.name) || self.guard.ignore(Some(prefix), row.name)
                {
                    return;
                }
                match row.intent {
                    // Verified: a prefixed *invocation* miss is
                    // `undefined_function`; the property path reports
                    // `undefined_prefixed_name` (PrefixedLookupDomain).
                    Intent::Invoke => {
                        self.emit(SemanticCode::UndefinedFunction, row.range, name_text, "");
                        self.record_miss(row.name, row.range, true, NameUse::Invoke);
                    }
                    Intent::Read | Intent::Write | Intent::ReadWrite => {
                        let type_text = self.strings.resolve(prefix).to_string();
                        self.emit(
                            SemanticCode::UndefinedPrefixedName,
                            row.range,
                            name_text,
                            &type_text,
                        );
                        let use_kind = match row.intent {
                            Intent::Write | Intent::ReadWrite => NameUse::Write,
                            _ => NameUse::Read,
                        };
                        self.record_miss(row.name, row.range, true, use_kind);
                    }
                    Intent::Operator(..) => {
                        self.record_miss(row.name, row.range, true, NameUse::Read);
                    }
                }
            }
            Receiver::Container { name, kind } => {
                self.check_container_member(row, name, kind);
            }
            // A folded creation: class-shaped results resolve on the
            // instance face (superclass walk, `Object` members); an
            // extension override resolves against the extension's members —
            // its `on` type's `Object` face is the under-report.
            Receiver::Created { name, kind } => {
                if kind == ContainerKind::Extension {
                    if self.object_members.contains(&row.name) || row.name == self.intern("==") {
                        return;
                    }
                    self.check_container_member(row, name, kind);
                } else {
                    self.check_instance_member(
                        row,
                        Receiver::Typed {
                            type_name: name,
                            ty: Ty::UNKNOWN,
                        },
                    );
                }
            }
            Receiver::This { .. } | Receiver::Typed { .. } => {
                self.check_instance_member(row, recv);
            }
            Receiver::Super { container, .. } => {
                self.check_super_member(row, container);
            }
        }
    }

    fn prefixed_hit(&self, prefix: StringId, name: StringId) -> bool {
        !matches!(
            self.namespaces.lookup_prefixed(self.lib, prefix, name),
            dartq_resolution::Lookup::NotFound
        )
    }

    /// Static/enum/extension access on a container name (`C.m`, `E.x`,
    /// `Ext.g`) — the oracle's `StaticLookupDomain`
    /// (`lookup_failure_reporter.dart`): the interface face reports
    /// `undefined_method` (the type-literal shape shares the registry name
    /// and message), `undefined_getter`, `undefined_enum_constant` on enum
    /// reads, and `undefined_setter` on writes; the extension face reports
    /// its dedicated trio. Operator intents have no static-face shape the
    /// M1 model can name — record only. External containers suppress.
    fn check_container_member(&mut self, row: &Row, container: StringId, kind: ContainerKind) {
        let Some((resolved, _)) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        ) else {
            return;
        };
        let write = matches!(row.intent, Intent::Write | Intent::ReadWrite);
        let mut defined = resolved.members.contains_key(&row.name);
        if !defined && write {
            defined = resolved
                .members
                .contains_key(&self.setter_spelling(row.name));
        }
        if !defined && resolved.constructors.contains(&row.name) {
            // A named-constructor selector (`C.named`) resolves through the
            // creation face — the constructor is the class's own
            // declaration, never an undefined member.
            return;
        }
        if defined {
            return;
        }
        let name_text = self.strings.resolve(row.name);
        let type_text = self.strings.resolve(container).to_string();
        // A compound write checks both faces (the oracle resolves the read
        // first): the read-face text, then the setter text.
        let read_face_missing = !resolved.members.contains_key(&row.name);
        let (invoke_code, read_code, write_code) = match kind {
            ContainerKind::Extension => (
                SemanticCode::UndefinedExtensionMethod,
                SemanticCode::UndefinedExtensionGetter,
                SemanticCode::UndefinedExtensionSetter,
            ),
            _ => (
                SemanticCode::UndefinedMethod,
                if kind == ContainerKind::Enum {
                    SemanticCode::UndefinedEnumConstant
                } else {
                    SemanticCode::UndefinedGetter
                },
                SemanticCode::UndefinedSetter,
            ),
        };
        match row.intent {
            Intent::Invoke => self.emit(invoke_code, row.range, name_text, &type_text),
            Intent::Read => self.emit(read_code, row.range, name_text, &type_text),
            Intent::Write => self.emit(write_code, row.range, name_text, &type_text),
            Intent::ReadWrite => {
                if read_face_missing {
                    self.emit(read_code, row.range, name_text, &type_text);
                }
                self.emit(write_code, row.range, name_text, &type_text);
            }
            Intent::Operator(..) => {}
        }
        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
    }

    /// Instance access through a declaration-typed receiver (`this.m`, `c.m`
    /// on a `C c`). A clean walk that misses reports the member family
    /// (`undefined_method`/`_getter`/`_setter` by access intent); a broken
    /// chain, the extension pool, enum synthesized members, and `Object`
    /// members suppress. Operator intents are the member-result shape
    /// (`c.j + 1`, `c.j[i]`): the member itself is the operator's receiver —
    /// a found member hands the operator check to its result type, a clean
    /// miss reports the getter read (the operator's invalid type never
    /// cascades). Mixin `on` constraints and extension `on` types are
    /// invisible to the model, so bodies of those containers record only.
    fn check_instance_member(&mut self, row: &Row, recv: Receiver) {
        let (Receiver::This { container, .. }
        | Receiver::Typed {
            type_name: container,
            ..
        }) = recv
        else {
            return;
        };
        if let Intent::Operator(..) = row.intent {
            match self.walk_members(container, row.name) {
                Walk::Found(entry) => {
                    let ty = Self::member_result_ty(entry);
                    self.check_operator_expr(ty, row);
                    return;
                }
                Walk::ChainBroken => return,
                Walk::Missing => {
                    let name_text = self.strings.resolve(row.name);
                    let type_text = self.strings.resolve(container).to_string();
                    self.emit(
                        SemanticCode::UndefinedGetter,
                        row.range,
                        name_text,
                        &type_text,
                    );
                    self.record_miss(row.name, row.range, true, NameUse::Read);
                    return;
                }
            }
        }
        if self.member_defined(container, row, false) {
            return;
        }
        if matches!(self.walk_members(container, row.name), Walk::ChainBroken) {
            // A chain broken at the root type (`int x; x.noSuchGetter`) is
            // the external-primitive face: the core member table decides.
            // A broken superclass hop suppresses as before.
            self.check_core_type_member(container, row);
            return;
        }
        let Some(kind) = self.container_kind_of(container) else {
            return; // external receiver type: suppress
        };
        if kind == ContainerKind::Enum && self.enum_synth_members.contains(&row.name) {
            return;
        }
        if matches!(kind, ContainerKind::Mixin | ContainerKind::Extension) {
            // The `on` face is invisible: a miss may be a constraint member.
            self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
            return;
        }
        let name_text = self.strings.resolve(row.name);
        let type_text = self.strings.resolve(container).to_string();
        match row.intent {
            Intent::Invoke => {
                self.emit(
                    SemanticCode::UndefinedMethod,
                    row.range,
                    name_text,
                    &type_text,
                );
            }
            Intent::Write | Intent::ReadWrite => {
                // A compound write checks the read face first (the oracle
                // resolves the getter before the setter).
                if row.intent == Intent::ReadWrite
                    && matches!(self.walk_members(container, row.name), Walk::Missing)
                {
                    self.emit(
                        SemanticCode::UndefinedGetter,
                        row.range,
                        name_text,
                        &type_text,
                    );
                }
                self.emit(
                    SemanticCode::UndefinedSetter,
                    row.range,
                    name_text,
                    &type_text,
                );
            }
            Intent::Read => {
                self.emit(
                    SemanticCode::UndefinedGetter,
                    row.range,
                    name_text,
                    &type_text,
                );
            }
            Intent::Operator(..) => unreachable!("operator intents returned above"),
        }
        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
    }

    /// `super.member` — the oracle's `SuperLookupDomain`: one analyzerCode
    /// (`undefined_super_member`) with a per-access-shape message, reported
    /// against the enclosing container's name. Operator intents follow the
    /// member-result shape (the direct `super + b` form lives on the
    /// receiver rows in [`Engine::resolve_receiver_row`], which reports the
    /// operator shape). A broken super chain (mixins: `on` constraints are
    /// not edges) suppresses.
    fn check_super_member(&mut self, row: &Row, container: StringId) {
        if let Intent::Operator(..) = row.intent {
            match self.walk_super(container, row.name) {
                Walk::Found(entry) => {
                    let ty = Self::member_result_ty(entry);
                    self.check_operator_expr(ty, row);
                }
                Walk::ChainBroken => {}
                Walk::Missing => {
                    let name_text = self.strings.resolve(row.name);
                    let type_text = self.strings.resolve(container).to_string();
                    self.emit(
                        SemanticCode::UndefinedSuperMember(SuperShape::Getter),
                        row.range,
                        name_text,
                        &type_text,
                    );
                    self.record_miss(row.name, row.range, true, NameUse::Read);
                }
            }
            return;
        }
        if self.member_defined(container, row, true) {
            return;
        }
        if matches!(self.walk_super(container, row.name), Walk::ChainBroken) {
            return;
        }
        let shape = match row.intent {
            Intent::Invoke => SuperShape::Method,
            Intent::Write | Intent::ReadWrite => SuperShape::Setter,
            Intent::Read => SuperShape::Getter,
            Intent::Operator(..) => unreachable!("operator intents returned above"),
        };
        let name_text = self.strings.resolve(row.name);
        let type_text = self.strings.resolve(container).to_string();
        // A compound write checks the read face first (the oracle resolves
        // the getter before the setter).
        if row.intent == Intent::ReadWrite
            && matches!(self.walk_super(container, row.name), Walk::Missing)
        {
            self.emit(
                SemanticCode::UndefinedSuperMember(SuperShape::Getter),
                row.range,
                name_text,
                &type_text,
            );
        }
        self.emit(
            SemanticCode::UndefinedSuperMember(shape),
            row.range,
            name_text,
            &type_text,
        );
        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
    }

    /// The external-primitive member face (`int x; x.noSuchGetter`): the
    /// receiver type is a dart:core primitive when the name is one of the
    /// [`CORE_TYPE_MEMBERS`] keys and resolves to no indexed container.
    /// Extension members still apply implicitly, so the pool gates first; a
    /// table hit is defined, a miss reports the access-intent trio.
    fn check_core_type_member(&mut self, container: StringId, row: &Row) {
        let container_text = self.strings.resolve(container);
        let Some((_, members)) = CORE_TYPE_MEMBERS
            .iter()
            .find(|(name, _)| container_text == *name)
        else {
            return; // not a known primitive: suppress as before
        };
        if resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        )
        .is_some()
        {
            return; // locally declared: the regular walk already decided
        }
        match self.extension_application(container, row.name) {
            ExtensionApp::None => {}
            ExtensionApp::Single => return,
            ExtensionApp::Ambiguous(displays) => {
                self.emit_extension_ambiguity(row.name, row.range, displays);
                return;
            }
        }
        let write = matches!(row.intent, Intent::Write | Intent::ReadWrite);
        let name_text = self.strings.resolve(row.name);
        if members.contains(&name_text)
            || (write && members.contains(&self.strings.resolve(self.setter_spelling(row.name))))
        {
            return;
        }
        if row.name == self.intern("==") || self.object_members.contains(&row.name) {
            return;
        }
        let type_text = container_text.to_string();
        match row.intent {
            Intent::Invoke => self.emit(
                SemanticCode::UndefinedMethod,
                row.range,
                name_text,
                &type_text,
            ),
            Intent::Write | Intent::ReadWrite => {
                if row.intent == Intent::ReadWrite {
                    self.emit(
                        SemanticCode::UndefinedGetter,
                        row.range,
                        name_text,
                        &type_text,
                    );
                }
                self.emit(
                    SemanticCode::UndefinedSetter,
                    row.range,
                    name_text,
                    &type_text,
                );
            }
            Intent::Read => self.emit(
                SemanticCode::UndefinedGetter,
                row.range,
                name_text,
                &type_text,
            ),
            Intent::Operator(..) => {}
        }
        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
    }

    /// The read and write operator spellings an access needs (`[]` / `[]=`
    /// for index rows by their write form; the plain spelling for binary
    /// operators); `(None, None)` for non-operator rows.
    fn operator_spellings(&self, row: &Row) -> (Option<StringId>, Option<StringId>) {
        let Intent::Operator(op, _) = &row.intent else {
            return (None, None);
        };
        if *op == self.intern("[]") {
            match row.write_op {
                Some(OpWrite::Assign) => (None, Some(self.intern("[]="))),
                Some(OpWrite::Compound) => (Some(*op), Some(self.intern("[]="))),
                None => (Some(*op), None),
            }
        } else {
            (Some(*op), None)
        }
    }

    /// The `name=` lookup spelling of a write target. Cached: the `format!`
    /// plus intern allocation repeats for the same write targets across a
    /// unit, and pass-time member checks ask on every write.
    fn setter_spelling(&mut self, name: StringId) -> StringId {
        if let Some(&spelling) = self.setter_ids.get(&name) {
            return spelling;
        }
        let text = self.strings.resolve(name);
        let spelling = self.strings.intern(&format!("{text}="));
        self.setter_ids.insert(name, spelling);
        spelling
    }

    /// A member's result type as far as M1 knows it: the declared field type
    /// or getter return type; methods (function-typed tear-offs) and setters
    /// stay [`Ty::Unknown`] — every check on them suppresses.
    fn member_result_ty(entry: &MemberEntry) -> Ty {
        match entry.kind {
            MemberKind::Field => entry.var_ty,
            MemberKind::Getter => entry.return_ty,
            MemberKind::Method | MemberKind::Setter => Ty::UNKNOWN,
        }
    }

    /// The operator application on a known receiver type (`c + d`, `c[i]`,
    /// `c.j + 1` against the member result): each read/write spelling that
    /// the type's container genuinely lacks reports `undefined_operator` at
    /// the operator's anchor span. Unresolved or external types (`Unknown`,
    /// broken chains) suppress — an invalid receiver type never cascades.
    fn check_operator_expr(&mut self, ty: Ty, row: &Row) {
        let Intent::Operator(_, op_range) = row.intent else {
            return;
        };
        let (read, write) = self.operator_spellings(row);
        let Ty::Named { name, .. } = ty else {
            return;
        };
        if resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        )
        .is_none()
        {
            return;
        }
        let mut missing: Vec<String> = Vec::new();
        for spelling in [read, write].into_iter().flatten() {
            if matches!(self.walk_members(name, spelling), Walk::Missing) {
                missing.push(self.strings.resolve(spelling).to_string());
            }
        }
        if missing.is_empty() {
            return;
        }
        let type_text = self.strings.resolve(name).to_string();
        for text in missing {
            self.emit(SemanticCode::UndefinedOperator, op_range, &text, &type_text);
        }
    }

    /// Runs the walk(s) an access needs and applies the shared suppressions
    /// (broken chain, extension application, `Object` members). A write also
    /// accepts the setter spelling (`name=`): a same-named getter, field, or
    /// method suppresses with the oracle's assignment-to-* codes, a setter
    /// defines the write.
    fn member_defined(&mut self, container: StringId, row: &Row, super_walk: bool) -> bool {
        let write = matches!(row.intent, Intent::Write | Intent::ReadWrite);
        let mut found = self.member_found(container, row.name, super_walk);
        if !found && write {
            let spelling = self.setter_spelling(row.name);
            found = self.member_found(container, spelling, super_walk);
        }
        if found {
            return true;
        }
        if row.name == self.intern("==") || self.object_members.contains(&row.name) {
            return true;
        }
        if super_walk {
            // Extensions do not apply to `super` accesses; the conservative
            // any-face gate keeps the under-report direction for the
            // unresolvable-receiver shapes.
            return self
                .index
                .get(self.file)
                .is_some_and(|f| f.has_extension_member(row.name));
        }
        match self.extension_application(container, row.name) {
            ExtensionApp::None => false,
            ExtensionApp::Single => true,
            ExtensionApp::Ambiguous(displays) => {
                self.emit_extension_ambiguity(row.name, row.range, displays);
                true
            }
        }
    }

    /// The implicit-application outcome for `member` against a receiver
    /// whose resolved container is `receiver`
    /// (`extension_member_resolver.dart:85-128` `findExtension` +
    /// `applicable_extensions.dart:157-171` `applicableTo`): the same-file
    /// faces whose `on` type spells the receiver's name and that declare the
    /// member under its base or setter spelling. A face with an absent or
    /// unresolvable `on` type never matches a resolved receiver — its
    /// members are suppressed from application (the oracle's `extendedType`
    /// fails to resolve; an inapplicable extension never gates an
    /// `undefined_*`).
    ///
    /// Two or more applicable candidates are never separable at the M1 name
    /// level: `_isMoreSpecific` (`extension_member_resolver.dart:469-513`)
    /// decides on SDK-ness and subtype relations between the instantiated
    /// `on` types, and same-named `on` types answer equal — so the access is
    /// ambiguous by the oracle's definition. The `Ambiguous` payload carries
    /// the candidates' display pairs (the error path only; `None`/`Single`
    /// allocate nothing).
    fn extension_application(&mut self, receiver: StringId, member: StringId) -> ExtensionApp {
        let setter = self.setter_spelling(member);
        let mut candidates: Vec<(String, String)> = Vec::new();
        let empty: Vec<ExtensionFace> = Vec::new();
        let faces = self
            .index
            .get(self.file)
            .map(|f| f.extension_faces())
            .unwrap_or(&empty);
        for face in faces {
            if face.on_type != Some(receiver) {
                continue;
            }
            if face.members.contains_key(&member) || face.members.contains_key(&setter) {
                if candidates.len() == 1 {
                    return ExtensionApp::Ambiguous(self.ambiguity_displays(receiver, member));
                }
                candidates.push(self.extension_display(face));
            }
        }
        match candidates.len() {
            0 => ExtensionApp::None,
            1 => ExtensionApp::Single,
            _ => ExtensionApp::Ambiguous(candidates),
        }
    }

    /// The display pair of one applicable face: the element shape
    /// (`extension E on C`, the ExtensionElement toString — the official
    /// two-variant argument, `ambiguous_extension_member_access_test.dart:54`)
    /// and the resolver's inline shape (`extension 'E'` / `unnamed extension
    /// on 'C'`, `extension_member_resolver.dart:119-124`).
    fn extension_display(&self, face: &ExtensionFace) -> (String, String) {
        let name = face.name.map(|n| self.strings.resolve(n).to_string());
        let on_type = self
            .strings
            .resolve(face.on_type.expect("an applicable face has an on type"))
            .to_string();
        let element = match &name {
            Some(name) => format!("extension {name} on {on_type}"),
            None => format!("extension on {on_type}"),
        };
        let inline = match name {
            Some(name) => format!("extension '{name}'"),
            None => format!("unnamed extension on '{on_type}'"),
        };
        (element, inline)
    }

    /// Every applicable candidate's display pair, re-walked in face
    /// declaration order (the three-or-more error path).
    fn ambiguity_displays(
        &mut self,
        receiver: StringId,
        member: StringId,
    ) -> Vec<(String, String)> {
        let setter = self.setter_spelling(member);
        let empty: Vec<ExtensionFace> = Vec::new();
        self.index
            .get(self.file)
            .map(|f| f.extension_faces())
            .unwrap_or(&empty)
            .iter()
            .filter(|face| {
                face.on_type == Some(receiver)
                    && (face.members.contains_key(&member) || face.members.contains_key(&setter))
            })
            .map(|face| self.extension_display(face))
            .collect()
    }

    /// Emits the `ambiguous_extension_member_access` verdict for the
    /// applicable candidates: the two-variant message for exactly two, the
    /// joined-list variant for more (`diagnostic.g.dart:338-348` for
    /// three-or-more, :363-374 for two), anchored at the access. The list
    /// renders per `commaSeparatedWithAnd`
    /// (`utilities/extensions/string.dart:41-104`): `a and b`, then
    /// `a, b, and c` from three on.
    fn emit_extension_ambiguity(
        &mut self,
        member: StringId,
        range: SourceRange,
        candidates: Vec<(String, String)>,
    ) {
        let name_text = self.strings.resolve(member).to_string();
        let (code, a1, a2) = match candidates.as_slice() {
            [first, second] => (
                SemanticCode::AmbiguousExtensionMemberTwo,
                first.0.clone(),
                second.0.clone(),
            ),
            _ => {
                let inlines: Vec<String> =
                    candidates.into_iter().map(|(_, inline)| inline).collect();
                let list = match inlines.len() {
                    0 | 1 => inlines.into_iter().next().unwrap_or_default(),
                    2 => {
                        let mut it = inlines.into_iter();
                        format!(
                            "{} and {}",
                            it.next().unwrap_or_default(),
                            it.next().unwrap_or_default()
                        )
                    }
                    _ => {
                        let last = inlines.last().expect("non-empty above").clone();
                        let head = &inlines[..inlines.len() - 1];
                        format!("{}, and {}", head.join(", "), last)
                    }
                };
                (
                    SemanticCode::AmbiguousExtensionMemberThreeOrMore,
                    list,
                    String::new(),
                )
            }
        };
        self.emit3(code, range, &name_text, &a1, &a2);
    }

    /// Whether the plain member walk finds `member` on `container`.
    fn member_found(&self, container: StringId, member: StringId, super_walk: bool) -> bool {
        let walk = if super_walk {
            self.walk_super(container, member)
        } else {
            self.walk_members(container, member)
        };
        matches!(walk, Walk::Found(_))
    }

    fn container_kind_of(&self, container: StringId) -> Option<ContainerKind> {
        resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        )
        .map(|(c, _)| c.kind)
    }
}

// —— #279: the constant-evaluation face ——————————————————————————————————

impl ConstLookup for Engine<'_> {
    /// Bare-name classification for the const evaluator: const-environment
    /// bindings first (a self-reference is the cycle fault), then
    /// in-scope locals (provably non-const), then the model (containers
    /// fold to type literals, top-level functions are constant tear-offs;
    /// everything else defers — the rows report genuine misses).
    fn ident(&self, name: StringId) -> IdentVerdict {
        for scope in self.const_scopes.iter().rev() {
            match scope.get(&name) {
                Some(ConstBinding::Value(v)) => return IdentVerdict::Value(v.clone()),
                Some(ConstBinding::InProgress) => return IdentVerdict::InProgress,
                None => {}
            }
        }
        if self.frames.iter().any(|f| f.locals.contains(&name)) {
            return IdentVerdict::NotConstant;
        }
        match self.resolve_bare(name) {
            BareInfo::Container { name, .. } => IdentVerdict::Value(ConstantValue::Type(name)),
            BareInfo::Plain { .. } => match self.top_level_decl(name).map(|d| d.kind) {
                Some(MicroKind::TopLevelFn) => IdentVerdict::Constant,
                Some(MicroKind::TopLevelVar) => {
                    if self.nonconst_top_level.contains(&name) {
                        IdentVerdict::NotConstant
                    } else {
                        IdentVerdict::Constant
                    }
                }
                _ => IdentVerdict::Constant,
            },
            _ => IdentVerdict::Constant,
        }
    }

    /// `recv.name` where `recv` is a type literal: the in-file enum's
    /// constants fold to enum values; this pass's captured static const
    /// field values fold; everything else (synthesized enum members,
    /// foreign/static faces) defers.
    fn member(&self, recv: &ConstantValue, name: StringId) -> Verdict {
        let ConstantValue::Type(container) = recv else {
            return Verdict::Constant;
        };
        if let Some(c) = self
            .index
            .get(self.file)
            .and_then(|f| f.container(*container))
            && c.kind == ContainerKind::Enum
            && c.enum_constants.contains(&name)
        {
            return Verdict::Value(ConstantValue::Enum {
                container: *container,
                name,
            });
        }
        if let Some(v) = self.const_field_values.get(&(*container, name)) {
            return Verdict::Value(v.clone());
        }
        Verdict::Constant
    }

    fn is_identical(&self, name: StringId) -> bool {
        name == self.identical_id
    }
}

impl<'a> Engine<'a> {
    /// Arms a declaration group as a const context (the `const` keyword
    /// seen on a local-variable or field group).
    pub fn arm_const_group(&mut self) {
        self.const_group = true;
        self.const_group_names.clear();
        self.const_group_cursor = 0;
    }

    /// A declaration-group name: const groups queue it for their
    /// initializer windows; a non-const *top-level* variable joins the
    /// provable non-const set (in-file, declaration order).
    pub fn const_decl_name(&mut self, name: StringId, field_context: bool) {
        if self.const_group {
            self.const_group_names.push(name);
        } else if field_context && self.frames.is_empty() {
            self.nonconst_top_level.insert(name);
        }
    }

    pub fn disarm_const_group(&mut self) {
        self.const_group = false;
        self.const_group_names.clear();
        self.const_group_cursor = 0;
    }

    /// A const initializer window opens: the group's next name marks
    /// in-progress (a self-reference inside is the cycle fault).
    pub fn open_const_initializer(&mut self) {
        if !self.const_group {
            return;
        }
        let Some(name) = self.const_group_names.get(self.const_group_cursor).copied() else {
            return;
        };
        self.const_group_cursor += 1;
        if let Some(scope) = self.const_scopes.last_mut() {
            scope.insert(name, ConstBinding::InProgress);
        }
        self.pending_init_names.push(name);
        self.const_capture.open_initializer();
    }

    /// A const local-variable initializer closes: evaluate, report per
    /// the official site mapping, bind the folded value and type.
    pub fn close_const_initializer(&mut self) {
        self.finish_const_initializer(true);
    }

    /// A const field initializer closes (class static consts and
    /// top-level consts — const fields are implicitly static).
    pub fn close_const_field_initializer(&mut self) {
        self.finish_const_initializer(false);
    }

    fn finish_const_initializer(&mut self, local: bool) {
        let Some(name) = self.pending_init_names.pop() else {
            return;
        };
        if !self.const_capture.open() {
            return;
        }
        let root = self.const_capture.close_initializer();
        let verdict = root
            .as_ref()
            .map_or(Verdict::Constant, |node| eval(node, self));
        self.resolve_const_verdict(&verdict);
        if let Verdict::Value(value) = &verdict {
            let folded = self.folded_ty(value);
            if local {
                self.bind_var_type(name, folded);
            } else if let Some((container, _)) = self.container_context() {
                self.const_field_values
                    .insert((container, name), value.clone());
            }
            // The value joins the const environment: locals in their
            // frame scope, top-level consts in the file scope (later
            // in-file initializers fold from it).
            let scope = if local || !self.frames.is_empty() {
                self.const_scopes.last_mut()
            } else {
                self.const_scopes.first_mut()
            };
            if let Some(scope) = scope {
                scope.insert(name, ConstBinding::Value(value.clone()));
            }
        } else if let Some(scope) = self.const_scopes.last_mut() {
            scope.remove(&name);
        }
    }

    /// Reports one const verdict at its site mapping: a fault carries its
    /// own `constEval*` code (the oracle's allow-list override), a
    /// generic non-constancy takes the site default
    /// (`const_initialized_with_non_constant_value`).
    fn resolve_const_verdict(&mut self, verdict: &Verdict) {
        match verdict {
            Verdict::Value(_) | Verdict::Constant => {}
            Verdict::NotConstant(span) => {
                self.emit(
                    SemanticCode::ConstInitializedWithNonConstantValue,
                    *span,
                    "",
                    "",
                );
            }
            Verdict::Fault(fault, span) => self.emit_const_fault(*fault, *span),
        }
    }

    fn emit_const_fault(&mut self, fault: ConstFault, span: SourceRange) {
        let code = match fault {
            ConstFault::TypeBool => SemanticCode::ConstEvalTypeBool,
            ConstFault::TypeBoolInt => SemanticCode::ConstEvalTypeBoolInt,
            ConstFault::TypeBoolNumString => SemanticCode::ConstEvalTypeBoolNumString,
            ConstFault::TypeInt => SemanticCode::ConstEvalTypeInt,
            ConstFault::TypeNum => SemanticCode::ConstEvalTypeNum,
            ConstFault::TypeNumString => SemanticCode::ConstEvalTypeNumString,
            ConstFault::TypeString => SemanticCode::ConstEvalTypeString,
            ConstFault::ThrowsException => SemanticCode::ConstEvalThrowsException,
            ConstFault::ThrowsIdbze => SemanticCode::ConstEvalThrowsIdbze,
            ConstFault::PrimitiveEquality => SemanticCode::ConstEvalPrimitiveEquality,
            ConstFault::Recursive => SemanticCode::RecursiveCompileTimeConstant,
        };
        self.emit(code, span, "", "");
    }

    /// The declared type a constant value folds to (the oracle infers a
    /// const variable's type from its constant initializer; only the
    /// shapes [`Ty`] can carry fold).
    fn folded_ty(&self, value: &ConstantValue) -> Ty {
        match value {
            ConstantValue::Int(_) => Ty::named(self.primitive_ids[0]),
            ConstantValue::Double(_) => Ty::named(self.primitive_ids[1]),
            ConstantValue::Bool(_) => Ty::named(self.primitive_ids[3]),
            ConstantValue::Str(_) => Ty::named(self.primitive_ids[4]),
            ConstantValue::Null => Ty::Null,
            ConstantValue::Enum { container, .. } => Ty::named(*container),
            // `const x = C();` has static type C (the const-creation
            // container); collection shapes have no Ty face here.
            ConstantValue::Created(container) => Ty::named(*container),
            ConstantValue::Type(_) | ConstantValue::Instance(_) => Ty::UNKNOWN,
        }
    }

    /// The folded declared type of an in-file const top-level variable
    /// (the type-folding face of #279: `const x = 1;` folds `x` to
    /// `int`).
    pub fn const_top_level_ty(&self, name: StringId) -> Option<Ty> {
        match self.const_scopes.first()?.get(&name)? {
            ConstBinding::Value(v) => Some(self.folded_ty(v)),
            ConstBinding::InProgress => None,
        }
    }

    /// An enum constant's window opens (`handle_enum_element` doubles as
    /// the previous element's close); its constructor arguments are the
    /// const context. The arg gate needs the enum to resolve in this file
    /// and carry a declared const constructor.
    pub fn open_enum_const_element(&mut self) {
        if self.enum_element_open {
            self.close_enum_const_element();
        }
        // Every enum value creation is a const creation (the spec requires
        // an enum's generative constructors to be const), so the gate arms
        // on the in-file enum alone — enum constants precede their
        // constructor declarations in source order.
        let gate = self
            .container_context()
            .filter(|(_, kind)| *kind == ContainerKind::Enum)
            .is_some_and(|(name, _)| {
                self.index
                    .get(self.file)
                    .is_some_and(|f| f.container(name).is_some())
            });
        self.const_capture.open_enum_element();
        self.arg_gates.push(gate);
        self.enum_element_open = true;
    }

    pub fn close_enum_const_element(&mut self) {
        if !self.enum_element_open {
            return;
        }
        self.enum_element_open = false;
        self.arg_gates.pop();
        if self.const_capture.open() {
            self.const_capture.close_enum_element();
        }
    }

    /// An explicit `const C(...)` opened (`begin_const_expression`); the
    /// gate arms once the target reference resolves.
    pub fn open_const_ctor_invocation(&mut self) {
        self.const_capture.open_ctor_invocation();
        self.arg_gates.push(false);
    }

    /// `end_constructor_reference(..., Const)` inside a const-creation
    /// window: the head token names the target; a const constructor this
    /// file declared arms the argument check (foreign and non-const
    /// targets defer — under-report).
    pub fn arm_const_ctor_gate(&mut self, head: Token) {
        let gate = self
            .name_of(head)
            .is_some_and(|name| self.const_ctor_target(name));
        if let Some(slot) = self.arg_gates.last_mut() {
            // OR-in: an enum element's window pre-arms its gate (the
            // constant's own ctor reference names the constant, never the
            // enum's constructor) — a genuine `const C(...)` target only
            // ever widens it.
            *slot |= gate;
        }
    }

    pub fn close_const_ctor_invocation(&mut self) {
        self.arg_gates.pop();
        if self.const_capture.open() {
            self.const_capture.close_ctor_invocation();
        }
    }

    /// A declared constructor or factory with the `const` modifier: its
    /// container joins the provable const-constructor set.
    pub fn note_const_ctor(&mut self) {
        if let Some((name, _)) = self.container_context() {
            self.const_ctors.insert(name);
        }
    }

    fn const_ctor_target(&self, container: StringId) -> bool {
        self.const_ctors.contains(&container)
            && self
                .index
                .get(self.file)
                .is_some_and(|f| f.container(container).is_some())
    }

    /// The callee classification a closed bare-identifier call resolves
    /// to (`evaluation.dart` `_invalidConstantForMethodInvocation`).
    fn classify_call(&self, name: StringId) -> CallClass {
        if self.const_ctor_target(name) {
            return CallClass::ConstCtor;
        }
        if matches!(self.resolve_bare(name), BareInfo::Missing { .. }) {
            return CallClass::Unknown;
        }
        if self.frames.iter().any(|f| f.locals.contains(&name))
            || self.const_scopes.iter().any(|s| s.contains_key(&name))
        {
            return CallClass::Invocable;
        }
        match self.top_level_decl(name).map(|d| d.kind) {
            Some(MicroKind::TopLevelFn) | Some(MicroKind::TopLevelVar) => CallClass::Invocable,
            _ => CallClass::Unknown,
        }
    }

    /// Arguments of a closed const creation: each evaluates against the
    /// `const_with_non_constant_argument` default
    /// (`constant_verifier.dart` `_validateConstantArguments`).
    pub fn check_const_args(&mut self, gate: bool, args: &[crate::const_eval::Node]) {
        if !gate {
            return;
        }
        for arg in args {
            let verdict = eval(arg, self);
            match verdict {
                Verdict::Value(_) | Verdict::Constant => {}
                Verdict::NotConstant(span) => {
                    self.emit(SemanticCode::ConstWithNonConstantArgument, span, "", "");
                }
                Verdict::Fault(fault, span) => self.emit_const_fault(fault, span),
            }
        }
    }

    /// Elements of a closed explicit const literal: each evaluates
    /// against its `non_constant_*_element` default (the
    /// `_ConstLiteralVerifier` site).
    pub fn check_const_elements(&mut self, code: SemanticCode, parts: &[crate::const_eval::Node]) {
        for part in parts {
            match eval(part, self) {
                Verdict::Value(_) | Verdict::Constant => {}
                Verdict::NotConstant(span) => self.emit(code, span, "", ""),
                Verdict::Fault(fault, span) => self.emit_const_fault(fault, span),
            }
        }
    }

    // —— capture event forwards (cheap no-ops with no window) ———

    pub fn const_int_literal(&mut self, token: Token) {
        let Some(text) = self.text_of(token) else {
            return;
        };
        match parse_int_literal(text) {
            Some(v) => self
                .const_capture
                .literal(token.range, ConstantValue::Int(v)),
            // Beyond i64: a valid constant of unobservable value — defer.
            None => self.const_capture.opaque_leaf(token.range),
        }
    }

    pub fn const_double_literal(&mut self, token: Token) {
        let Some(text) = self.text_of(token) else {
            return;
        };
        let cleaned: String = text.chars().filter(|c| *c != '_').collect();
        match cleaned.parse::<f64>() {
            Ok(v) => self
                .const_capture
                .literal(token.range, ConstantValue::Double(v)),
            Err(_) => self.const_capture.opaque_leaf(token.range),
        }
    }

    pub fn const_bool_literal(&mut self, token: Token) {
        let value = self.text_of(token).is_some_and(|t| t == "true");
        self.const_capture
            .literal(token.range, ConstantValue::Bool(value));
    }

    pub fn const_null_literal(&mut self, token: Token) {
        self.const_capture.literal(token.range, ConstantValue::Null);
    }

    /// The raw interior of an interpolation-free string literal (the
    /// begin token spans the whole literal, quotes included; escapes are
    /// not decoded — no diagnostic observes the contents).
    pub fn const_string_leaf(&mut self, begin: SourceRange, end: SourceRange) {
        let text = self
            .source
            .get(begin.offset as usize + 1..begin.end().saturating_sub(1) as usize)
            .unwrap_or("");
        self.const_capture
            .end_literal_string(0, begin, end, ConstantValue::Str(text.into()));
    }

    pub fn const_string_end(&mut self, count: usize, begin: SourceRange, end: SourceRange) {
        self.const_capture
            .end_literal_string(count, begin, end, ConstantValue::Str("".into()));
    }
}

/// Parses an int literal body: decimal or `0x` hex, `_` separators
/// stripped; `None` when the value exceeds `i64` (an unobservable edge —
/// the literal defers to an opaque constant).
fn parse_int_literal(text: &str) -> Option<i64> {
    let cleaned: String = text.chars().filter(|c| *c != '_').collect();
    let (radix, digits) = if let Some(rest) = cleaned
        .strip_prefix("0x")
        .or_else(|| cleaned.strip_prefix("0X"))
    {
        (16, rest)
    } else {
        (10, cleaned.as_str())
    };
    i64::from_str_radix(digits, radix).ok()
}

/// The capture operator for a binary operator spelling (`??=` and
/// friends never reach `begin_binary_expression` as plain binaries).
fn bin_op_of(text: &str) -> BinOp {
    match text {
        "+" => BinOp::Add,
        "-" => BinOp::Sub,
        "*" => BinOp::Mul,
        "/" => BinOp::Div,
        "~/" => BinOp::IntDiv,
        "%" => BinOp::Mod,
        "&" => BinOp::BitAnd,
        "|" => BinOp::BitOr,
        "^" => BinOp::BitXor,
        "<<" => BinOp::Shl,
        ">>" => BinOp::Shr,
        ">>>" => BinOp::Ushr,
        "<" => BinOp::Lt,
        ">" => BinOp::Gt,
        "<=" => BinOp::Le,
        ">=" => BinOp::Ge,
        "==" => BinOp::Eq,
        "!=" => BinOp::Ne,
        "&&" => BinOp::And,
        "||" => BinOp::Or,
        "??" => BinOp::IfNull,
        _ => BinOp::Add, // unreachable: the expression grammar's binary set
    }
}

/// The reference-collecting [`ParserListener`] feeding a shared [`Engine`]:
/// one parse over the same token table `collect` used, then pass 2 resolves
/// the rows. Every flow-relevant event double-dispatches to the per-unit
/// [`Flow`] recorder (the #449 fan-out), whose facts fold into the report
/// after the parse.
struct DiagnoseListener<'a> {
    engine: Rc<RefCell<Engine<'a>>>,
    flow: Rc<RefCell<Flow<'a>>>,
}

/// Stably merges two source-ordered diagnostic lists by range offset
/// (ties keep the base list's rows first).
fn merge_source_ordered(base: Vec<Diagnostic>, extra: Vec<Diagnostic>) -> Vec<Diagnostic> {
    let mut merged = Vec::with_capacity(base.len() + extra.len());
    let mut base = base.into_iter().peekable();
    let mut extra = extra.into_iter().peekable();
    loop {
        let take_base = match (base.peek(), extra.peek()) {
            (Some(b), Some(e)) => b.range.offset <= e.range.offset,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => break,
        };
        if take_base {
            merged.push(base.next().expect("peeked"));
        } else {
            merged.push(extra.next().expect("peeked"));
        }
    }
    merged
}

/// Diagnoses one compilation unit against its library scope (#120, batch 1).
///
/// The unit must already be collected and indexed: `index` carries the
/// file's element model (its duplicate log folds into the front of the
/// report as `duplicate_definition`s) and its non-type-parameter
/// declarations are already declared into the library graph. The parse is a
/// second pass over the same token table `collect` used.
///
/// # Panics
///
/// Panics if the parser still holds the listener at teardown — impossible
/// by construction (`parser` is dropped above), asserted with `expect`.
#[allow(clippy::too_many_arguments)] // the analysis context, one field each
pub fn diagnose(
    source: &str,
    tokens: &TokenTable,
    features: ParserFeatures,
    strings: &StringTable,
    file: FileId,
    lib: LibraryId,
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    index: &ElementIndex,
) -> FileReport {
    let engine = Rc::new(RefCell::new(Engine::new(
        graph,
        namespaces,
        index,
        strings,
        source,
        tokens,
        file,
        lib,
        features.primary_constructors_enabled,
    )));
    let flow = Rc::new(RefCell::new(Flow::new(tokens, source)));
    let mut parser = Parser::new(
        source,
        tokens,
        features,
        DiagnoseListener {
            engine: Rc::clone(&engine),
            flow: Rc::clone(&flow),
        },
    );
    parser.parse_unit();
    drop(parser);
    // The parser held the only other strong reference and is gone, so the
    // unwrap cannot fail.
    let mut engine = Rc::into_inner(engine)
        .expect("the parser released its listener")
        .into_inner();
    engine.resolve_rows();
    // The usage cluster's library-level pass runs after every use indexed:
    // declaration sites and import directives walk the index, staging into
    // the report; the staged emissions merge source-ordered into the tail.
    engine.finalize_usage();
    let mut report = std::mem::take(&mut engine.report);
    // The flow layer's dead-code facts join the source-ordered tail of the
    // report (both sides are offset-sorted; the merge keeps the invariant).
    // The flow merge runs FIRST: dead_code is a resolution-phase diagnostic
    // while the usage cluster is the post-resolution verifier, so at a
    // shared anchor (dead `var x = 1;` and its unread `x`) the official
    // collection order is dead_code before unused_local_variable — the
    // `<=` tie-break below then keeps that order into the final list.
    let mut flow_diagnostics = Vec::new();
    for fact in flow.borrow_mut().take_facts() {
        if let FlowFact::DeadCode { span } = fact {
            let code = SemanticCode::DeadCode;
            flow_diagnostics.push(Diagnostic::new(
                code.severity(),
                code.code_info(),
                code.message("", ""),
                file,
                span.range,
                code.correction_message(""),
            ));
        }
    }
    report.diagnostics = merge_source_ordered(report.diagnostics, flow_diagnostics);
    let mut staged = std::mem::take(&mut report.usage);
    staged.sort_by_key(|d| d.range.offset);
    report.diagnostics = merge_source_ordered(report.diagnostics, staged);
    // `duplicate_definition`s lead the report, each pointing at the SECOND
    // declaration's name token (the analyzer's diagnose-at-second rule).
    // Setters live under `name=` in the model; the oracle's message names
    // the bare identifier.
    let mut diagnostics = Vec::with_capacity(
        index
            .get(file)
            .map_or(0, |f| f.duplicates().len() + f.arity_violations().len()),
    );
    if let Some(elements) = index.get(file) {
        for dup in elements.duplicates() {
            let text = strings.resolve(dup.name);
            let display = text.strip_suffix('=').unwrap_or(text);
            diagnostics.push(Diagnostic::new(
                Severity::Error,
                SemanticCode::DuplicateDefinition.code_info(),
                SemanticCode::DuplicateDefinition.message(display, ""),
                file,
                dup.range,
                None,
            ));
        }
        // The declaration-arity log folds next (capture-time order =
        // source order): operator parameter counts and the setter shape
        // rule.
        for arity in elements.arity_violations() {
            let (code, message) = match arity.violation {
                ArityViolation::Operator {
                    name,
                    expected,
                    found,
                } => {
                    let code = SemanticCode::WrongNumberOfParametersForOperator;
                    (
                        code,
                        code.message3(
                            strings.resolve(name),
                            &expected.to_string(),
                            &found.to_string(),
                        ),
                    )
                }
                ArityViolation::OperatorMinus { found } => {
                    let code = SemanticCode::WrongNumberOfParametersForOperatorMinus;
                    (code, code.message3(&found.to_string(), "", ""))
                }
                ArityViolation::Setter => (
                    SemanticCode::WrongNumberOfParametersForSetter,
                    SemanticCode::WrongNumberOfParametersForSetter.message("", ""),
                ),
            };
            diagnostics.push(Diagnostic::new(
                code.severity(),
                code.code_info(),
                message,
                file,
                arity.range,
                None,
            ));
        }
    }
    diagnostics.append(&mut report.diagnostics);
    report.diagnostics = diagnostics;
    report
}

impl ParserListener for DiagnoseListener<'_> {
    // —— frames: containers ——————————————————————————————————————————

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
        name: Token,
    ) {
        let mut engine = self.engine.borrow_mut();
        let name_range = name.range;
        let name = engine.name_of(name);
        engine.push_container_frame(ContainerKind::Class, name);
        if let Some(name) = name {
            engine.note_decl_site(name, name_range, DeclKind::Container);
            engine.check_face_inheritance(name, ContainerKind::Class, name_range);
        }
    }

    fn end_class_declaration(&mut self, _begin: Token, _end: Token) {
        self.engine.borrow_mut().pop_frame();
    }

    fn begin_named_mixin_application(
        &mut self,
        _begin: Token,
        _abstract_kw: Option<Token>,
        _sealed_kw: Option<Token>,
        _base_kw: Option<Token>,
        _interface_kw: Option<Token>,
        _final_kw: Option<Token>,
        _augment_kw: Option<Token>,
        _mixin_kw: Option<Token>,
        name: Token,
    ) {
        let mut engine = self.engine.borrow_mut();
        let name_range = name.range;
        let name = engine.name_of(name);
        engine.push_container_frame(ContainerKind::Class, name);
        if let Some(name) = name {
            engine.note_decl_site(name, name_range, DeclKind::Container);
            engine.check_face_inheritance(name, ContainerKind::Class, name_range);
        }
    }

    fn end_named_mixin_application(
        &mut self,
        _begin: Token,
        _class: Token,
        _equals: Token,
        _implements: Option<Token>,
        _end: Token,
    ) {
        self.engine.borrow_mut().pop_frame();
    }

    fn begin_mixin_declaration(
        &mut self,
        _begin: Token,
        _augment_kw: Option<Token>,
        _base_kw: Option<Token>,
        _mixin: Token,
        name: Token,
    ) {
        let mut engine = self.engine.borrow_mut();
        let name_range = name.range;
        let name = engine.name_of(name);
        engine.push_container_frame(ContainerKind::Mixin, name);
        if let Some(name) = name {
            engine.note_decl_site(name, name_range, DeclKind::Container);
            engine.check_face_inheritance(name, ContainerKind::Mixin, name_range);
        }
    }

    fn end_mixin_declaration(&mut self, _begin: Token, _end: Token) {
        self.engine.borrow_mut().pop_frame();
    }

    fn begin_enum_declaration(
        &mut self,
        _begin: Token,
        _augment_kw: Option<Token>,
        _enum_keyword: Token,
        name: Token,
    ) {
        let mut engine = self.engine.borrow_mut();
        let name_range = name.range;
        let name = engine.name_of(name);
        engine.push_container_frame(ContainerKind::Enum, name);
        if let Some(name) = name {
            engine.note_decl_site(name, name_range, DeclKind::Container);
            engine.check_face_inheritance(name, ContainerKind::Enum, name_range);
        }
    }

    fn end_enum_declaration(
        &mut self,
        _begin: Token,
        _enum_keyword: Token,
        _left_brace: Token,
        _member_count: usize,
        _end: Token,
    ) {
        self.engine.borrow_mut().pop_frame();
    }

    fn begin_extension_declaration(
        &mut self,
        _augment: Option<Token>,
        _extension_keyword: Token,
        name: Option<Token>,
    ) {
        let mut engine = self.engine.borrow_mut();
        let name = name.and_then(|token| engine.name_of(token));
        engine.push_container_frame(ContainerKind::Extension, name);
    }

    fn end_extension_declaration(
        &mut self,
        _begin: Token,
        _extension_keyword: Token,
        _on: Option<Token>,
        _end: Token,
    ) {
        self.engine.borrow_mut().pop_frame();
    }

    fn begin_extension_type_declaration(
        &mut self,
        _augment_kw: Option<Token>,
        _extension_keyword: Token,
        name: Token,
    ) {
        let mut engine = self.engine.borrow_mut();
        let name_range = name.range;
        let name = engine.name_of(name);
        // The representation checks read the element model (complete since
        // `collect`) and anchor at this name token.
        if let Some(name) = name {
            engine.note_decl_site(
                name,
                name_range,
                DeclKind::Container,
            );
            engine.check_extension_type(name, name_range);
        }
        engine.push_container_frame(ContainerKind::Extension, name);
    }

    fn end_extension_type_declaration(
        &mut self,
        _begin: Token,
        _augment_kw: Option<Token>,
        _extension_keyword: Token,
        _type_keyword: Token,
        _end: Token,
    ) {
        self.engine.borrow_mut().pop_frame();
    }

    // —— frames: executables ——————————————————————————————————————————
    // The Executable frame opens at the declaration head so its type
    // parameters and formals live inside it; the body block nests within.

    fn begin_method(
        &mut self,
        _kind: DeclarationKind,
        _augment_kw: Option<Token>,
        _external_kw: Option<Token>,
        static_kw: Option<Token>,
        _covariant_kw: Option<Token>,
        _var_final_or_const: Option<Token>,
        get_or_set: Option<Token>,
        name: Token,
        _enclosing_declaration_name: Option<&str>,
    ) {
        self.flow.borrow_mut().begin_method(
            _kind,
            _augment_kw,
            _external_kw,
            static_kw,
            _covariant_kw,
            _var_final_or_const,
            get_or_set,
            name,
            _enclosing_declaration_name,
        );

        let mut engine = self.engine.borrow_mut();
        let is_setter = get_or_set.and_then(|t| engine.text_of(t)) == Some("set");
        engine.open_member_setter = is_setter;
        engine.open_member_static = static_kw.is_some();
        engine.ctor_pending = None;
        engine.open_toplevel_site = None;
        engine.open_exec_site = None;
        engine.push_executable_frame(static_kw.is_none());
        // The `invalid_override` check runs at the declaration head: the
        // element model is already complete (`collect` parsed before this
        // pass), and this event carries the name token whose span anchors
        // the emission. Setters live under `name=` in the model.
        let name_range = name.range;
        let Some(name) = engine.name_of(name) else {
            return;
        };
        let is_setter = engine.open_member_setter;
        let spelling = if is_setter {
            engine.setter_spelling(name)
        } else {
            name
        };
        engine.check_override(spelling, name_range);
    }

    fn end_method(
        &mut self,
        _kind: DeclarationKind,
        _get_or_set: Option<Token>,
        _begin: Token,
        _begin_param: Token,
        _begin_initializers: Option<Token>,
        _end: Token,
    ) {
        self.flow.borrow_mut().end_method(
            _kind,
            _get_or_set,
            _begin,
            _begin_param,
            _begin_initializers,
            _end,
        );

        self.engine.borrow_mut().pop_frame();
    }

    fn begin_top_level_method(
        &mut self,
        _last_consumed: Token,
        _augment: Option<Token>,
        _external: Option<Token>,
    ) {
        self.flow
            .borrow_mut()
            .begin_top_level_method(_last_consumed, _augment, _external);

        let mut engine = self.engine.borrow_mut();
        engine.open_member_setter = false;
        engine.open_member_static = false;
        engine.ctor_pending = None;
        engine.open_exec_site = None;
        engine.push_executable_frame(false);
    }

    fn end_top_level_method(&mut self, _begin: Token, _get_or_set: Option<Token>, _end: Token) {
        self.flow
            .borrow_mut()
            .end_top_level_method(_begin, _get_or_set, _end);

        let mut engine = self.engine.borrow_mut();
        // A top-level setter's site spelling fixes up here (`set` parses
        // before the name event): setters live under `name=`.
        let site = engine.open_toplevel_site.take();
        let is_setter = _get_or_set.and_then(|t| engine.text_of(t)) == Some("set");
        if let (Some(site), true) = (site, is_setter) {
            let bare = engine.decl_sites[site].name;
            engine.decl_sites[site].name = engine.setter_spelling(bare);
        }
        engine.pop_frame();
    }

    fn begin_constructor(
        &mut self,
        _kind: DeclarationKind,
        _augment_kw: Option<Token>,
        _external_kw: Option<Token>,
        _static_kw: Option<Token>,
        _covariant_kw: Option<Token>,
        var_final_or_const: Option<Token>,
        _get_or_set: Option<Token>,
        _new_kw: Option<Token>,
        _name: Token,
        _enclosing_declaration_name: Option<&str>,
    ) {
        self.flow.borrow_mut().begin_constructor(
            _kind,
            _augment_kw,
            _external_kw,
            _static_kw,
            _covariant_kw,
            var_final_or_const,
            _get_or_set,
            _new_kw,
            _name,
            _enclosing_declaration_name,
        );

        let mut engine = self.engine.borrow_mut();
        // A `const` constructor makes its container a provable const-ctor
        // target (the #279 arg-check gate).
        if var_final_or_const.and_then(|t| engine.text_of(t)) == Some("const") {
            engine.note_const_ctor();
        }
        engine.open_member_setter = false;
        engine.open_member_static = false;
        engine.open_toplevel_site = None;
        engine.open_exec_site = None;
        engine.ctor_pending = engine.container_context().map(|(c, _)| c);
        engine.push_executable_frame(true);
    }

    fn end_constructor(
        &mut self,
        _kind: DeclarationKind,
        _begin: Token,
        _new_kw: Option<Token>,
        _begin_param: Token,
        _begin_initializers: Option<Token>,
        _end: Token,
    ) {
        self.flow.borrow_mut().end_constructor(
            _kind,
            _begin,
            _new_kw,
            _begin_param,
            _begin_initializers,
            _end,
        );

        let mut engine = self.engine.borrow_mut();
        engine.ctor_pending = None;
        engine.pop_frame();
    }

    fn begin_factory(
        &mut self,
        _kind: DeclarationKind,
        _last_consumed: Token,
        _augment_kw: Option<Token>,
        _external_kw: Option<Token>,
        const_keyword: Option<Token>,
    ) {
        self.flow.borrow_mut().begin_factory(
            _kind,
            _last_consumed,
            _augment_kw,
            _external_kw,
            const_keyword,
        );

        let mut engine = self.engine.borrow_mut();
        // A `const` factory is a const-constructor target.
        if const_keyword.is_some() {
            engine.note_const_ctor();
        }
        // Factories have no `this`.
        engine.open_member_setter = false;
        engine.open_member_static = false;
        engine.open_toplevel_site = None;
        engine.open_exec_site = None;
        engine.ctor_pending = engine.container_context().map(|(c, _)| c);
        engine.push_executable_frame(false);
    }

    fn end_factory(&mut self, _kind: DeclarationKind, _begin: Token, _factory: Token, _end: Token) {
        self.flow
            .borrow_mut()
            .end_factory(_kind, _begin, _factory, _end);

        let mut engine = self.engine.borrow_mut();
        engine.ctor_pending = None;
        engine.pop_frame();
    }

    fn begin_local_function_declaration(&mut self, _token: Token) {
        self.flow
            .borrow_mut()
            .begin_local_function_declaration(_token);

        // The closure/local-function face leaves the enclosing site: its
        // parameters are never `unused_element_parameter` candidates (the
        // official `_isUsedElement` enclosing-element gate), and its locals
        // close with their own frame.
        let instance = self.engine.borrow().instance_context();
        self.engine.borrow_mut().open_exec_site = None;
        self.engine.borrow_mut().pending_local_fn = None;
        self.engine.borrow_mut().push_executable_frame(instance);
    }

    fn end_local_function_declaration(&mut self, _end: Token) {
        self.flow.borrow_mut().end_local_function_declaration(_end);

        let mut engine = self.engine.borrow_mut();
        // Materialize after the pop: the function's own frame mark covers the
        // decl index otherwise, so `close_frame_locals` would judge the decl
        // at the function's close (before any later use) and truncate it.
        // After the pop it joins the enclosing frame's window — the official
        // verifier judges local functions at their scope's close.
        engine.pop_frame();
        engine.finish_local_fn_site();
    }

    fn begin_function_expression(&mut self, _token: Token) {
        self.flow.borrow_mut().begin_function_expression(_token);

        self.engine.borrow_mut().const_capture.begin_opaque_group();
        let instance = self.engine.borrow().instance_context();
        self.engine.borrow_mut().open_exec_site = None;
        self.engine.borrow_mut().pending_local_fn = None;
        self.engine.borrow_mut().push_executable_frame(instance);
    }

    fn end_function_expression(&mut self, _begin: Token, end: Token) {
        self.flow.borrow_mut().end_function_expression(_begin, end);

        let mut engine = self.engine.borrow_mut();
        engine.const_capture.end_opaque_group(end.range);
        engine.pop_frame();
        engine.push_value_row(end);
    }

    fn begin_named_function_expression(&mut self, _token: Token) {
        self.flow
            .borrow_mut()
            .begin_named_function_expression(_token);

        let instance = self.engine.borrow().instance_context();
        self.engine.borrow_mut().open_exec_site = None;
        self.engine.borrow_mut().pending_local_fn = None;
        self.engine.borrow_mut().push_executable_frame(instance);
    }

    fn end_named_function_expression(&mut self, end: Token) {
        self.flow.borrow_mut().end_named_function_expression(end);

        let mut engine = self.engine.borrow_mut();
        // Same pop-before-materialize discipline as the declaration form.
        engine.pop_frame();
        engine.finish_local_fn_site();
        engine.push_value_row(end);
    }

    // —— frames: scopes ———————————————————————————————————————————————

    fn begin_typedef(&mut self, _token: Token) {
        self.engine.borrow_mut().push_block_frame();
    }

    fn end_typedef(
        &mut self,
        _augment: Option<Token>,
        _typedef: Token,
        _equals: Option<Token>,
        _end: Token,
    ) {
        self.engine.borrow_mut().pop_frame();
    }

    fn begin_catch_clause(&mut self, _token: Token) {
        self.flow.borrow_mut().begin_catch_clause(_token);

        // The catch frame owns the catch parameters; the body block's own
        // events are not overridable from outside dartq-parser (BlockKind
        // is crate-private), so block scoping comes from the executable and
        // catch frames only. The frame closes at `handle_catch_block` —
        // after the body — so the parameters stay visible in it and their
        // locals close with the clause.
        let mut engine = self.engine.borrow_mut();
        engine.catch_clause_start = Some(_token.range);
        engine.catch_on_type = None;
        engine.catch_params = 0;
        engine.push_block_frame();
    }

    fn end_catch_clause(&mut self, _token: Token) {
        self.flow.borrow_mut().end_catch_clause(_token);

        // The frame pop moved to `handle_catch_block` (the clause's body
        // block parses between the two events).
    }

    fn begin_for_statement(&mut self, _token: Token) {
        self.flow.borrow_mut().begin_for_statement(_token);

        self.engine.borrow_mut().push_block_frame();
    }

    fn end_for_statement(&mut self, _end: Token) {
        self.flow.borrow_mut().end_for_statement(_end);

        self.engine.borrow_mut().pop_frame();
    }

    fn handle_for_in_loop_parts(
        &mut self,
        _await_kw: Option<Token>,
        _for_kw: Token,
        _left_paren: Token,
        _pattern_keyword: Option<Token>,
        _in_kw: Token,
    ) {
        self.flow.borrow_mut().handle_for_in_loop_parts(
            _await_kw,
            _for_kw,
            _left_paren,
            _pattern_keyword,
            _in_kw,
        );

        self.engine.borrow_mut().push_block_frame();
    }

    fn end_for_in(&mut self, _end: Token) {
        self.flow.borrow_mut().end_for_in(_end);

        self.engine.borrow_mut().pop_frame();
    }

    // —— scopes: locals and type parameters ———————————————————————————

    fn begin_variables_declaration(
        &mut self,
        _token: Token,
        _late_kw: Option<Token>,
        var_final_or_const: Option<Token>,
    ) {
        self.flow
            .borrow_mut()
            .begin_variables_declaration(_token, _late_kw, var_final_or_const);

        let mut engine = self.engine.borrow_mut();
        // The group's declared type arms here (one `parse_type` per group,
        // `handle_no_type` disarmed it for untyped groups); every name in
        // the group binds it.
        engine.pending_decl_ty = engine.armed_ty.take();
        if var_final_or_const.and_then(|t| engine.text_of(t)) == Some("const") {
            engine.arm_const_group();
        }
    }

    fn end_variables_declaration(&mut self, _count: usize, _end: Option<Token>) {
        self.flow
            .borrow_mut()
            .end_variables_declaration(_count, _end);

        let mut engine = self.engine.borrow_mut();
        engine.pending_decl_ty = None;
        engine.armed_ty = None;
        engine.disarm_const_group();
    }

    fn begin_variable_initializer(&mut self, _token: Token) {
        self.flow.borrow_mut().begin_variable_initializer(_token);

        let mut engine = self.engine.borrow_mut();
        engine.init_mark = Some(engine.rows.len());
        engine.open_const_initializer();
    }

    fn end_variable_initializer(&mut self, _token: Token) {
        self.flow.borrow_mut().end_variable_initializer(_token);

        let mut engine = self.engine.borrow_mut();
        engine.close_const_initializer();
        engine.check_decl_initializer();
    }

    fn begin_type_variable(&mut self, token: Token) {
        let mut engine = self.engine.borrow_mut();
        if let Some(name) = engine.name_of(token) {
            engine.push_type_parameter(name);
        }
    }

    fn begin_type_variables(&mut self, _token: Token) {
        // A new header started: a previous declaration's parked variables
        // never met their frame (a typedef's, say) — drop them.
        self.engine.borrow_mut().pending_type_params.clear();
    }

    // —— identifiers ——————————————————————————————————————————————————

    fn handle_identifier(&mut self, token: Token, context: IdentifierContext) {
        self.flow.borrow_mut().handle_identifier(token, context);

        use IdentifierContext as C;
        let mut engine = self.engine.borrow_mut();
        let name = engine.name_of(token);
        match context {
            C::Expression => {
                if let Some(name) = name {
                    engine.push_bare(name, token.range);
                    engine.const_capture.identifier(name, token.range);
                }
            }
            C::ExpressionContinuation => {
                if let Some(name) = name {
                    engine.push_member(name, token.range);
                    engine.const_capture.identifier(name, token.range);
                }
            }
            C::TypeReference | C::TypeReferenceContinuation => {
                if let Some(name) = name {
                    // The catch clause's `on` type: the first type reference
                    // after `begin_catch_clause` and before any parameter.
                    if engine.catch_clause_start.is_some()
                        && engine.catch_params == 0
                        && engine.catch_on_type.is_none()
                    {
                        engine.catch_on_type = Some(name);
                    }
                    let head = if context == C::TypeReference {
                        TypeHead::Bare
                    } else {
                        TypeHead::Continuation
                    };
                    engine.push_type_use(name, token.range, head);
                    // The head awaits its type-argument group (or its
                    // completing `handle_type`).
                    engine.arm_type_head(name, token.range);
                }
            }
            C::PrefixedTypeReference => {
                if let Some(name) = name {
                    engine.arm_type_prefix(name, token.range);
                }
            }
            // A constructor reference names a class: the guarded miss is the
            // oracle's `undefined_class` (`new Missing()`), never an
            // invocation.
            C::ConstructorReference => {
                if let Some(name) = name {
                    engine.push_type_use(name, token.range, TypeHead::Bare);
                    engine.creation_ref = Some((name, None));
                }
            }
            C::ConstructorReferenceContinuation => {
                if let Some(name) = name
                    && let Some(r) = engine.creation_ref.as_mut()
                {
                    r.1 = Some(name);
                }
            }
            C::LocalVariableDeclaration => {
                if let Some(name) = name {
                    engine.note_local(name, token.range);
                    engine.const_decl_name(name, false);
                    engine.bind_declared_type(name);
                }
            }
            C::FormalParameterDeclaration => {
                if let Some(name) = name {
                    engine.note_parameter(name, token.range);
                    engine.bind_parameter_type(name);
                }
            }
            C::CatchParameter => {
                if let Some(name) = name {
                    engine.note_catch_parameter(name, token.range);
                    engine.bind_parameter_type(name);
                }
            }
            C::LocalFunctionDeclaration => {
                if let Some(name) = name {
                    engine.note_local_function(name, token.range);
                }
            }
            C::LabelDeclaration => {
                if let Some(name) = name {
                    engine.pending_label = Some((name, token.range));
                }
            }
            C::LabelReference => {
                if let Some(name) = name {
                    engine.note_label_use(name);
                }
            }
            C::ImportPrefixDeclaration => {
                if let Some(name) = name
                    && let Some(site) = engine.imports.last_mut()
                {
                    site.prefix = Some(name);
                }
            }
            C::NamedArgumentReference => {
                if let Some(name) = name {
                    engine.check_named_argument(name, token.range);
                    engine.open_argument(Some(name));
                }
            }
            C::EnumValueDeclaration => {
                // The enum constant's window opens here: its type arguments
                // check against the enclosing enum's declared count (the
                // `_enum` code), and its constructor arguments capture as
                // a const context (`handle_enum_element` closes it). The
                // constant is a static field site — the `unused_field`
                // read rule owns it.
                engine.enter_enum_element();
                engine.open_enum_const_element();
                if let Some(name) = name
                    && let Some((container, _)) = engine.container_context()
                {
                    engine.note_decl_site(
                        name,
                        token.range,
                        DeclKind::Member {
                            kind: MemberKind::Field,
                            is_static: true,
                            container,
                        },
                    );
                }
            }
            C::ConstructorReferenceContinuationAfterTypeArguments
                if engine.in_enum_element.is_some() && engine.ctor_pos == 0 =>
            {
                // The enum constant's constructor selector (`c.other()`):
                // outside argument expressions (ctor_pos 0), the name must
                // be a constructor the enclosing enum declares
                // (`undefined_enum_constructor`).
                if let Some(name) = name {
                    engine.check_enum_constructor_selector(name, token.range);
                }
            }
            C::MetadataReference => {
                if let Some(name) = name {
                    // The annotated-declaration exemption latch (the
                    // pragma/JS entry-point gates' superset) plus the
                    // annotation-arity check.
                    engine.pending_annotations.push(name);
                    engine.check_annotation(name, token.range);
                }
            }
            C::TopLevelFunctionDeclaration => {
                if let Some(name) = name {
                    engine.latch_return_type(ExecKind::Function, name);
                    let site = engine.note_decl_site(name, token.range, DeclKind::TopLevelFn);
                    engine.open_toplevel_site = Some(site);
                    engine.open_exec_site = Some(site);
                }
            }
            C::MethodDeclaration => {
                if let Some(name) = name {
                    engine.latch_return_type(ExecKind::Method, name);
                    let is_setter = engine.open_member_setter;
                    let spelling = if is_setter {
                        engine.setter_spelling(name)
                    } else {
                        name
                    };
                    let is_static = engine.open_member_static;
                    let container = engine.container_context().map_or(name, |(c, _)| c);
                    let site = engine.note_decl_site(
                        spelling,
                        token.range,
                        DeclKind::Member {
                            kind: MemberKind::Method,
                            is_static,
                            container,
                        },
                    );
                    engine.open_exec_site = Some(site);
                }
            }
            C::OperatorName => {
                if let Some(name) = name {
                    let container = engine.container_context().map_or(name, |(c, _)| c);
                    engine.note_decl_site(
                        name,
                        token.range,
                        DeclKind::Member {
                            kind: MemberKind::Method,
                            is_static: false,
                            container,
                        },
                    );
                }
            }
            C::MethodDeclarationContinuation => {
                // The named constructor selector (`A.named`): the site joins
                // only under an open constructor/factory (the same context
                // also fires for qualified method heads, which never declare
                // a constructor).
                if let (Some(name), Some(container)) = (name, engine.ctor_pending) {
                    let site = engine.note_decl_site(
                        name,
                        token.range,
                        DeclKind::Constructor { container },
                    );
                    engine.open_exec_site = Some(site);
                }
            }
            C::TypedefDeclaration => {
                if let Some(name) = name {
                    engine.note_decl_site(name, token.range, DeclKind::TypeAlias);
                }
            }
            // Field groups (class fields, enum body fields, top-level
            // variables): const groups queue the names for their
            // initializer windows; non-const top-level variables join the
            // provable non-const set. Class fields also enter the
            // invalid_override check (their face is the declared type).
            C::FieldDeclaration | C::TopLevelVariableDeclaration => {
                if let Some(name) = name {
                    engine.const_decl_name(name, true);
                    if context == C::FieldDeclaration {
                        engine.check_override(name, token.range);
                        if let Some((container, _)) = engine.container_context() {
                            let is_static = engine.open_member_static;
                            engine.note_decl_site(
                                name,
                                token.range,
                                DeclKind::Member {
                                    kind: MemberKind::Field,
                                    is_static,
                                    container,
                                },
                            );
                        }
                    } else {
                        engine.note_decl_site(name, token.range, DeclKind::TopLevelVar);
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_qualified(&mut self, _period: Token) {
        self.engine.borrow_mut().consume_type_prefix();
    }

    // —— declared types ——————————————————————————————————————————————

    fn handle_type(&mut self, name: Token, question: Option<Token>) {
        self.flow.borrow_mut().handle_type(name, question);

        let mut engine = self.engine.borrow_mut();
        // The type completed without a group; its head slot is consumed.
        engine.pending_type_head = None;
        let Some(text) = engine.text_of(name) else {
            return;
        };
        // `dynamic` is a top type and `Null` is the null-type singleton —
        // both have dedicated [`Ty`] shapes.
        let ty = match text {
            "dynamic" => Ty::Dynamic,
            "Null" => Ty::Null,
            _ => Ty::Named {
                name: engine.strings.intern(text),
                nullable: question.is_some(),
            },
        };
        engine.armed_ty = Some(ty);
    }

    fn handle_no_type(&mut self, _last_consumed: Token) {
        self.flow.borrow_mut().handle_no_type(_last_consumed);

        self.engine.borrow_mut().armed_ty = None;
    }

    fn handle_void_keyword(&mut self, _token: Token) {
        self.flow.borrow_mut().handle_void_keyword(_token);

        self.engine.borrow_mut().armed_ty = Some(Ty::Void);
    }

    fn handle_void_keyword_with_type_arguments(&mut self, _token: Token) {
        self.engine.borrow_mut().armed_ty = Some(Ty::Void);
    }

    fn begin_initializers(&mut self, _token: Token) {
        self.engine.borrow_mut().initializer_depth += 1;
    }

    fn end_initializers(&mut self, _count: usize, _begin: Token, _end: Token) {
        self.engine.borrow_mut().initializer_depth -= 1;
    }

    fn begin_metadata(&mut self, token: Token) {
        self.engine.borrow_mut().annotation_at = Some(token.range);
    }

    fn end_metadata(&mut self, _begin: Token, _period_before_name: Option<Token>, _end: Token) {
        self.engine.borrow_mut().annotation_at = None;
    }

    // —— receivers and selectors ——————————————————————————————————————

    fn handle_this_expression(&mut self, token: Token, _context: IdentifierContext) {
        self.flow
            .borrow_mut()
            .handle_this_expression(token, _context);

        self.engine.borrow_mut().push_keyword_receiver(token, false);
        self.engine
            .borrow_mut()
            .const_capture
            .this_or_super(token.range);
    }

    fn handle_super_expression(&mut self, token: Token, _context: IdentifierContext) {
        self.flow
            .borrow_mut()
            .handle_super_expression(token, _context);

        self.engine.borrow_mut().push_keyword_receiver(token, true);
        self.engine
            .borrow_mut()
            .const_capture
            .this_or_super(token.range);
    }

    fn handle_dot_access(&mut self, token: Token, end: Token, is_null_aware: bool) {
        self.flow
            .borrow_mut()
            .handle_dot_access(token, end, is_null_aware);

        self.engine.borrow_mut().merge_member(is_null_aware);
        self.engine
            .borrow_mut()
            .const_capture
            .dot_access(is_null_aware, end.range);
        let _ = token;
    }

    fn handle_cascade_access(&mut self, _operator: Token, _end: Token, is_null_aware: bool) {
        self.flow
            .borrow_mut()
            .handle_cascade_access(_operator, _end, is_null_aware);

        self.engine.borrow_mut().merge_member(is_null_aware);
    }

    fn handle_send(&mut self, _begin: Token, _end: Token) {
        self.flow.borrow_mut().handle_send(_begin, _end);

        self.engine.borrow_mut().consume_send();
    }

    fn handle_assignment_expression(&mut self, operator: Token, _end: Token) {
        self.flow
            .borrow_mut()
            .handle_assignment_expression(operator, _end);

        let mut engine = self.engine.borrow_mut();
        engine.const_capture.assignment(operator.range);
        engine.mark_assignment(operator);
        engine.check_assignment(operator);
        engine.check_dead_null_assignment(operator);
    }

    fn begin_binary_expression(&mut self, operator: Token) {
        self.flow.borrow_mut().begin_binary_expression(operator);

        let mut engine = self.engine.borrow_mut();
        let Some(text) = engine.text_of(operator) else {
            return;
        };
        engine
            .const_capture
            .begin_binary(bin_op_of(text), operator.range);
        if text == "??" {
            engine.begin_if_null(operator.range);
        }
        if USER_OPERATORS.contains(&text) {
            let op = engine.intern(text);
            engine.mark_operator(op, operator.range);
        }
    }

    fn end_binary_expression(&mut self, _operator: Token, end: Token) {
        self.flow.borrow_mut().end_binary_expression(_operator, end);

        let mut engine = self.engine.borrow_mut();
        engine.const_capture.end_binary(end.range);
        if engine.text_of(_operator) == Some("??") {
            engine.end_if_null();
        }
    }

    fn handle_unary_prefix_expression(&mut self, operator: Token) {
        self.flow
            .borrow_mut()
            .handle_unary_prefix_expression(operator);

        let mut engine = self.engine.borrow_mut();
        let op = match engine.text_of(operator) {
            Some("-") => UnOp::Neg,
            Some("!") => UnOp::Not,
            Some("~") => UnOp::BitNot,
            _ => UnOp::Inc, // `++`/`--` prefixes
        };
        engine.const_capture.unary_prefix(op);
    }

    fn handle_unary_postfix_assignment_expression(&mut self, operator: Token) {
        self.engine
            .borrow_mut()
            .const_capture
            .unary_postfix(operator.range);
    }

    fn begin_conditional_expression(&mut self, _question: Token) {
        self.flow
            .borrow_mut()
            .begin_conditional_expression(_question);

        self.engine.borrow_mut().const_capture.begin_conditional();
    }

    fn handle_conditional_expression_colon(&mut self) {
        self.flow.borrow_mut().handle_conditional_expression_colon();

        self.engine.borrow_mut().const_capture.conditional_colon();
    }

    fn end_conditional_expression(&mut self, _question: Token, _colon: Token, end: Token) {
        self.flow
            .borrow_mut()
            .end_conditional_expression(_question, _colon, end);

        self.engine
            .borrow_mut()
            .const_capture
            .end_conditional(end.range);
    }

    fn begin_parenthesized_expression_or_record_literal(&mut self, _token: Token) {
        self.flow
            .borrow_mut()
            .begin_parenthesized_expression_or_record_literal(_token);

        self.engine.borrow_mut().const_capture.parenthesized_begin();
    }

    fn handle_is_operator(&mut self, is_operator: Token, _not: Option<Token>) {
        self.flow.borrow_mut().handle_is_operator(is_operator, _not);

        self.engine
            .borrow_mut()
            .const_capture
            .not_constant(is_operator.range);
    }

    fn handle_as_operator(&mut self, operator: Token) {
        self.flow.borrow_mut().handle_as_operator(operator);

        self.engine
            .borrow_mut()
            .const_capture
            .opaque_leaf(operator.range);
    }

    // —— suppression brackets —————————————————————————————————————————

    fn begin_is_operator_type(&mut self, _operator: Token) {
        self.flow.borrow_mut().begin_is_operator_type(_operator);

        let mut engine = self.engine.borrow_mut();
        engine.enter_non_type_pos();
        // The `is` operand's type never counts as a type use (the official
        // `_insideIsExpression` face).
        engine.is_type_pos += 1;
    }

    fn end_is_operator_type(&mut self, _operator: Token) {
        self.flow.borrow_mut().end_is_operator_type(_operator);

        let mut engine = self.engine.borrow_mut();
        engine.leave_non_type_pos();
        engine.is_type_pos -= 1;
    }

    fn begin_as_operator_type(&mut self, _operator: Token) {
        self.flow.borrow_mut().begin_as_operator_type(_operator);

        self.engine.borrow_mut().enter_non_type_pos();
    }

    fn end_as_operator_type(&mut self, _operator: Token) {
        self.flow.borrow_mut().end_as_operator_type(_operator);

        self.engine.borrow_mut().leave_non_type_pos();
    }

    fn begin_new_expression(&mut self, token: Token) {
        self.engine.borrow_mut().enter_ctor_pos();
        self.engine.borrow_mut().const_capture.begin_opaque_group();
        let _ = token;
    }

    fn end_new_expression(&mut self, token: Token) {
        self.engine.borrow_mut().leave_ctor_pos();
        self.engine
            .borrow_mut()
            .const_capture
            .end_opaque_group(token.range);
    }

    fn begin_const_expression(&mut self, const_keyword: Token) {
        let mut engine = self.engine.borrow_mut();
        engine.enter_ctor_pos();
        engine.open_const_ctor_invocation();
        let _ = const_keyword;
    }

    fn end_const_expression(&mut self, _token: Token) {
        let mut engine = self.engine.borrow_mut();
        engine.close_const_ctor_invocation();
        engine.leave_ctor_pos();
    }
    fn begin_implicit_creation_expression(&mut self, _token: Token) {
        self.engine.borrow_mut().enter_ctor_pos();
    }

    fn end_implicit_creation_expression(&mut self, _begin: Token, _open_angle: Token) {
        let mut engine = self.engine.borrow_mut();
        engine.end_implicit_creation();
        engine.leave_ctor_pos();
    }

    fn begin_arguments(&mut self, _token: Token) {
        self.flow.borrow_mut().begin_arguments(_token);

        self.engine.borrow_mut().const_capture.begin_arguments();
        self.engine.borrow_mut().open_arguments();
    }

    fn end_arguments(&mut self, count: usize, _begin: Token, end: Token) {
        self.flow.borrow_mut().end_arguments(count, _begin, end);

        let closed = self
            .engine
            .borrow_mut()
            .const_capture
            .end_arguments(end.range);
        let mut engine = self.engine.borrow_mut();
        if let Some(closed) = closed {
            if closed.creation_owned {
                // The creation's own list: each argument checks against
                // `const_with_non_constant_argument`; the gate lives until
                // the window closes (nested calls arm their own gates).
                let gate = engine.arg_gates.last().copied().unwrap_or(false);
                engine.check_const_args(gate, &closed.args);
                if closed.ctor_invocation {
                    // The creation's value joins the enclosing window —
                    // typed by its container when this file declared the
                    // const constructor (type folding), opaque otherwise.
                    let ctor = closed.callee.is_some_and(|n| engine.const_ctor_target(n));
                    let value = if ctor {
                        ConstantValue::Created(closed.callee.expect("checked above"))
                    } else {
                        ConstantValue::Instance(crate::const_eval::InstanceKind::Object)
                    };
                    engine.const_capture.push_value(closed.span, value);
                }
            } else if let Some(name) = closed.callee {
                // `name(...)` in a const context: the implicit const
                // creation form checks per argument; any other resolvable
                // callee is a method-invocation fault; unknown names defer
                // (their own row reports).
                match engine.classify_call(name) {
                    CallClass::ConstCtor => {
                        engine.check_const_args(true, &closed.args);
                        engine
                            .const_capture
                            .push_value(closed.span, ConstantValue::Created(name));
                    }
                    CallClass::Invocable => {
                        engine.emit(SemanticCode::ConstEvalMethodInvocation, closed.span, "", "");
                        engine.const_capture.opaque_leaf(closed.span);
                    }
                    CallClass::Unknown => engine.const_capture.opaque_leaf(closed.span),
                }
            } else {
                engine.const_capture.opaque_leaf(closed.span);
            }
        }
        engine.close_arguments(u16::try_from(count).unwrap_or(u16::MAX));
    }

    fn handle_positional_argument(&mut self, _token: Token) {
        self.engine.borrow_mut().const_capture.positional_argument();
        self.engine.borrow_mut().open_argument(None);
    }

    fn handle_named_argument(&mut self, _colon: Token) {
        self.flow.borrow_mut().handle_named_argument(_colon);

        // The capture opens the named argument here (fires before the
        // value); the row engine opened it at the label's identifier event.
        self.engine.borrow_mut().const_capture.named_argument();
    }

    fn begin_return_statement(&mut self, _token: Token) {
        let mut engine = self.engine.borrow_mut();
        engine.return_mark = Some(engine.rows.len());
    }

    fn end_return_statement(&mut self, has_expression: bool, _begin: Token, end: Token) {
        self.flow
            .borrow_mut()
            .end_return_statement(has_expression, _begin, end);

        let mut engine = self.engine.borrow_mut();
        let mark = engine.return_mark.take();
        if !has_expression {
            // Bare `return;` is a `void` expression in the oracle's matrix.
            engine.check_return(Ty::Void, end.range, false);
            return;
        }
        if let Some(mark) = mark
            && let Some((ty, literal, range)) = engine.single_row_since(mark)
        {
            engine.check_return(ty, range, literal);
        }
    }

    fn handle_expression_function_body(&mut self, _arrow: Token, _end: Option<Token>) {
        self.flow
            .borrow_mut()
            .handle_expression_function_body(_arrow, _end);

        let mut engine = self.engine.borrow_mut();
        let Some(mark) = engine.body_mark.take() else {
            return;
        };
        if let Some((ty, literal, range)) = engine.single_row_since(mark) {
            engine.check_return(ty, range, literal);
        }
    }

    fn handle_async_modifier(&mut self, _async_kw: Option<Token>, _star: Option<Token>) {
        self.flow
            .borrow_mut()
            .handle_async_modifier(_async_kw, _star);

        let mut engine = self.engine.borrow_mut();
        engine.body_mark = Some(engine.rows.len());
    }

    fn begin_cascade(&mut self, operator: Token) {
        self.flow.borrow_mut().begin_cascade(operator);

        let mut engine = self.engine.borrow_mut();
        engine.in_cascade = true;
        engine.const_capture.begin_opaque_group();
        let _ = operator;
    }

    fn end_cascade(&mut self) {
        self.flow.borrow_mut().end_cascade();

        let mut engine = self.engine.borrow_mut();
        engine.in_cascade = false;
        engine.const_capture.end_opaque_group(SourceRange {
            offset: 0,
            length: 0,
        });
    }

    fn handle_indexed_expression(&mut self, question: Option<Token>, open: Token, close: Token) {
        self.flow
            .borrow_mut()
            .handle_indexed_expression(question, open, close);

        let mut engine = self.engine.borrow_mut();
        // The oracle anchors index-access reports on the bracket pair
        // (`property_element_resolver.dart`, `_reportUnresolvedIndex`).
        let bracket = SourceRange {
            offset: open.range.offset,
            length: close.range.end() - open.range.offset,
        };
        // `#279: e1[e2]` is a spec constant the subset cannot value —
        // both operands defer through an opaque leaf.
        engine.const_capture.indexed(bracket);
        // The receiver: the closest row wholly before the `[`.
        let Some(pos) = engine
            .rows
            .iter()
            .rposition(|row| row.range.end() <= open.range.offset)
        else {
            return;
        };
        if question.is_some() {
            let ty = Engine::receiver_row_ty(&engine.rows[pos]);
            if ty.is_strictly_non_nullable() {
                let range = engine.rows[pos].range;
                engine.emit3(SemanticCode::InvalidNullAwareOperator, range, "?[", "[", "");
            }
        }
        // Fold (`a[i]`, `c.j[i] = v`): the index arguments leave the stack —
        // the receiver row carries the `[]` access with the bracket span, and
        // a value row stands for the result so compound-expression detection
        // (argument types, initializer checks) still sees one opaque row. A
        // receiver whose own resolution left no name row (call results,
        // groups) stays unfolded — its operator checks suppress anyway.
        if matches!(
            engine.rows[pos].shape,
            RowShape::Bare(_) | RowShape::Member { .. } | RowShape::Receiver(_)
        ) && engine.rows[pos].intent == Intent::Read
        {
            let index_op = engine.intern("[]");
            engine.rows.truncate(pos + 1);
            engine.rows[pos].intent = Intent::Operator(index_op, bracket);
            engine.push_value_row(close);
        }
    }

    // —— patterns & switch exhaustiveness (M2 face) ——————————————————————

    fn begin_pattern(&mut self, _token: Token) {
        self.flow.borrow_mut().begin_pattern(_token);

        let mut engine = self.engine.borrow_mut();
        engine.pattern_depth += 1;
        engine.pattern_stack.push(PatternFrame {
            leaves: Vec::new(),
            or_ops: 0,
            fields: Vec::new(),
        });
    }

    fn end_pattern(&mut self, _token: Token) {
        self.flow.borrow_mut().end_pattern(_token);

        let mut engine = self.engine.borrow_mut();
        engine.pattern_depth = engine.pattern_depth.saturating_sub(1);
        let Some(frame) = engine.pattern_stack.pop() else {
            return;
        };
        let merged = Engine::collapse(frame.leaves, frame.or_ops);
        if engine.pattern_stack.is_empty() {
            // A root pattern: only an open case header's coverage feeds the
            // exhaustiveness walk; other roots (if-case, pattern
            // assignments) are dropped here, so debris can neither leak
            // into a later case nor grow the set.
            if engine.case_sink_open {
                engine.pattern_roots.push(merged);
            }
        } else if let Some(parent) = engine.pattern_stack.last_mut() {
            parent.leaves.push(merged);
        }
    }

    fn begin_binary_pattern(&mut self, operator: Token) {
        let mut engine = self.engine.borrow_mut();
        let is_or = engine.text_of(operator) == Some("||");
        if is_or && let Some(frame) = engine.pattern_stack.last_mut() {
            frame.or_ops += 1;
        }
    }

    fn end_binary_pattern(&mut self, _operator: Token) {}

    fn handle_cast_pattern(&mut self, _operator: Token) {
        let mut engine = self.engine.borrow_mut();
        // `p as X` covers X's values (official `castPatternToSpace`).
        let leaf = match engine.armed_ty.take() {
            Some(Ty::Named { name, nullable }) => Restriction::Named(name, nullable),
            _ => Restriction::Unknown,
        };
        engine.pattern_leaf(leaf);
    }

    fn handle_null_assert_pattern(&mut self, _bang: Token) {
        // `p!` covers the non-null side of the context.
        let mut engine = self.engine.borrow_mut();
        let leaf = match NameTy::from_semantic(
            engine.switches.last().map_or(Ty::Unknown, |c| c.scrutinee),
        ) {
            NameTy::Named(name, _) => Restriction::Named(name, false),
            _ => Restriction::Unknown,
        };
        engine.pattern_leaf(leaf);
    }

    fn handle_null_check_pattern(&mut self, _question: Token) {
        // `p?` covers the context values plus `null` (`nullCheckToSpace`):
        // a nullable context already covers null; a non-null context
        // unions with it.
        let mut engine = self.engine.borrow_mut();
        let leaf = match NameTy::from_semantic(
            engine.switches.last().map_or(Ty::Unknown, |c| c.scrutinee),
        ) {
            NameTy::Named(name, true) => Restriction::Named(name, true),
            NameTy::Named(name, false) => {
                Restriction::Union(vec![Restriction::Named(name, false), Restriction::Null])
            }
            _ => Restriction::Unknown,
        };
        engine.pattern_leaf(leaf);
    }

    fn handle_relational_pattern(&mut self, _operator: Token) {
        // Value comparisons are not representable at name level.
        self.engine.borrow_mut().pattern_leaf(Restriction::Unknown);
    }

    fn handle_list_pattern(&mut self, _count: usize, _left: Token, _right: Token) {
        // The list face arrives through keysOfInterest/additional properties
        // — not modeled at name level (documented under-report).
        self.engine.borrow_mut().pattern_leaf(Restriction::Unknown);
    }

    fn handle_map_pattern(&mut self, _count: usize, _left: Token, _right: Token) {
        self.engine.borrow_mut().pattern_leaf(Restriction::Unknown);
    }

    fn handle_record_pattern(&mut self, _left_paren: Token, _count: usize) {
        // Record shapes live in the interned lattice (`dartq-types`); the
        // name level has no record type, so the row is unknown-shaped and
        // the engine under-reports.
        self.engine.borrow_mut().pattern_leaf(Restriction::Unknown);
    }

    fn begin_constant_pattern(&mut self, _const_keyword: Option<Token>) {
        self.flow
            .borrow_mut()
            .begin_constant_pattern(_const_keyword);

        self.engine.borrow_mut().pending_const = None;
    }

    fn end_constant_pattern(&mut self, _const_keyword: Option<Token>) {
        self.flow.borrow_mut().end_constant_pattern(_const_keyword);

        let mut engine = self.engine.borrow_mut();
        let leaf = engine
            .pending_const
            .take()
            .unwrap_or_else(|| engine.classify_constant_tail());
        engine.pattern_leaf(leaf);
    }

    /// Closes an object pattern. The parser fires this event *after* the
    /// rest — the fields parsed and their `handle_pattern_field` boundaries
    /// drained this pattern's own bracket frame — so the head name and the
    /// drained buckets meet here: the row is the bare head type exactly
    /// when every field's restriction covers its member's whole column.
    fn handle_object_pattern(
        &mut self,
        first_identifier: Token,
        dot: Option<Token>,
        _second_identifier: Option<Token>,
    ) {
        let mut engine = self.engine.borrow_mut();
        // `Type(...)` shapes: the first identifier is the type when no dot
        // follows; the `Type.field`/`:field` shorthands stay unmodeled.
        let head = if dot.is_none() {
            engine.name_of(first_identifier)
        } else {
            None
        };
        let fields = engine
            .pattern_stack
            .last_mut()
            .map_or(Vec::new(), |frame| std::mem::take(&mut frame.fields));
        engine.close_object_fields(head, fields);
    }

    fn handle_pattern_field(&mut self, colon: Option<Token>) {
        let mut engine = self.engine.borrow_mut();
        // The field's sub-pattern just completed: its bracket merged into
        // this pattern's own frame, so everything buffered since the last
        // boundary is exactly this field's sub-restrictions. A type-free
        // sub-pattern (`var x`, `_`) merges to the context space — it
        // constrains no column, so it pools as no leaf. Record patterns
        // fire the same boundary on their own frames; their buckets have
        // no consumer and die with the bracket.
        let name = colon.and_then(|token| engine.name_of(token));
        let mut leaves = engine
            .pattern_stack
            .last_mut()
            .map_or(Vec::new(), |frame| std::mem::take(&mut frame.leaves));
        leaves.retain(|leaf| *leaf != Restriction::NamedContext);
        if let Some(frame) = engine.pattern_stack.last_mut() {
            frame.fields.push(ObjectField { name, leaves });
        }
    }

    fn handle_declared_variable_pattern(
        &mut self,
        _keyword: Option<Token>,
        variable: Token,
        _in_assignment_pattern: bool,
    ) {
        self.flow.borrow_mut().handle_declared_variable_pattern(
            _keyword,
            variable,
            _in_assignment_pattern,
        );

        let mut engine = self.engine.borrow_mut();
        // The bound variable joins the enclosing scope (the flat M1 frame;
        // the oracle's pattern scoping is finer, and a wider binding can
        // only under-report undefined-name, never false-positive). The
        // usage cluster tracks it as a pattern variable: the enclosing
        // `PatternVariableDeclaration` group reports all-or-nothing.
        if let Some(name) = engine.name_of(variable) {
            engine.add_tracked_local(name, variable.range, LocalKind::PatternVariable);
            match engine.armed_ty.take() {
                Some(Ty::Named {
                    name: ty_name,
                    nullable,
                }) => {
                    engine.bind_var_type(
                        name,
                        Ty::Named {
                            name: ty_name,
                            nullable,
                        },
                    );
                    engine.pattern_leaf(Restriction::Named(ty_name, nullable));
                }
                ty => {
                    if let Some(ty) = ty {
                        engine.bind_var_type(name, ty);
                    }
                    // A type-free variable covers its context: no leaf.
                }
            }
        } else {
            engine.armed_ty = None;
        }
    }

    fn handle_wildcard_pattern(&mut self, _keyword: Option<Token>, _wildcard: Token) {
        self.flow
            .borrow_mut()
            .handle_wildcard_pattern(_keyword, _wildcard);

        // A typed wildcard (`M _`) covers the declared type's space (the
        // official wildcard converts through its type); a bare `_` (or
        // `var _`) covers the context: no leaf.
        let mut engine = self.engine.borrow_mut();
        let Some(Ty::Named { name, nullable }) = engine.armed_ty.take() else {
            return;
        };
        engine.pattern_leaf(Restriction::Named(name, nullable));
    }

    // —— switch statements ————————————————————————————————————————————

    fn begin_switch_block(&mut self, token: Token) {
        self.flow.borrow_mut().begin_switch_block(token);

        let kw = self
            .engine
            .borrow_mut()
            .switch_kw_latch
            .take()
            .unwrap_or(token.range);
        self.engine.borrow_mut().open_switch(false, kw);
    }

    /// Latches the `switch` keyword: the non-exhaustive anchor (the block
    /// event that follows only carries the `{`).
    fn begin_switch_statement(&mut self, token: Token) {
        self.flow.borrow_mut().begin_switch_statement(token);

        self.engine.borrow_mut().switch_kw_latch = Some(token.range);
    }

    fn end_switch_block(&mut self, _count: usize, _begin: Token, _end: Token) {
        self.flow
            .borrow_mut()
            .end_switch_block(_count, _begin, _end);

        self.engine.borrow_mut().report_switch();
    }

    fn begin_switch_case_when_clause(&mut self, _token: Token) {}

    /// The case header's pattern window opens before its pattern parses;
    /// the root it produces is captured at [`Self::end_case_expression`].
    fn begin_case_expression(&mut self, _case_kw: Token) {
        self.flow.borrow_mut().begin_case_expression(_case_kw);

        self.engine.borrow_mut().case_sink_open = true;
    }

    /// One statement-case header completed: its pattern's coverage latches
    /// here with the guard flag and the `case`-keyword anchor (the oracle's
    /// `caseNode.keyword`, constant_verifier.dart:1076-1085). Shared-body
    /// groups (`case A: case B:`) capture one case per header.
    fn end_case_expression(&mut self, case_kw: Token, when: Option<Token>, _colon: Token) {
        self.flow
            .borrow_mut()
            .end_case_expression(case_kw, when, _colon);

        let mut engine = self.engine.borrow_mut();
        let root = engine.close_case_sink();
        let space = engine.substitute_root(root);
        if let Some(ctx) = engine.switches.last_mut() {
            ctx.cases.push(NameCase {
                space,
                guarded: when.is_some(),
                anchor: case_kw.range,
            });
        }
    }

    fn end_switch_case(
        &mut self,
        _label_count: usize,
        _expression_count: usize,
        default_kw: Option<Token>,
        _colon_after_default: Option<Token>,
        _statement_count: usize,
        _begin: Token,
        _end: Token,
    ) {
        self.flow.borrow_mut().end_switch_case(
            _label_count,
            _expression_count,
            default_kw,
            _colon_after_default,
            _statement_count,
            _begin,
            _end,
        );

        // The case headers already captured their spaces
        // ([`Self::end_case_expression`]); the body-level event only
        // latches `default` and closes any window a broken header left
        // open.
        let mut engine = self.engine.borrow_mut();
        engine.case_sink_open = false;
        if let Some(ctx) = engine.switches.last_mut()
            && let Some(token) = default_kw
        {
            ctx.default_kw = Some(token.range);
        }
    }

    // —— switch expressions ————————————————————————————————————————————

    fn begin_switch_expression_block(&mut self, token: Token) {
        self.flow.borrow_mut().begin_switch_expression_block(token);

        let kw = self
            .engine
            .borrow_mut()
            .switch_kw_latch
            .take()
            .unwrap_or(token.range);
        self.engine.borrow_mut().open_switch(true, kw);
    }

    /// Latches the `switch` keyword of a switch expression.
    fn begin_switch_expression(&mut self, token: Token) {
        self.flow.borrow_mut().begin_switch_expression(token);

        self.engine.borrow_mut().switch_kw_latch = Some(token.range);
    }

    /// The expression-case header's pattern window opens before its
    /// pattern parses.
    fn begin_switch_expression_case(&mut self) {
        self.flow.borrow_mut().begin_switch_expression_case();

        self.engine.borrow_mut().case_sink_open = true;
    }

    /// The expression case's pattern completed: its coverage latches now
    /// (the guard and the `=>` anchor only exist at the case's end).
    fn handle_switch_expression_case_pattern(&mut self, _token: Token) {
        self.flow
            .borrow_mut()
            .handle_switch_expression_case_pattern(_token);

        let mut engine = self.engine.borrow_mut();
        let root = engine.close_case_sink();
        let space = engine.substitute_root(root);
        if let Some(ctx) = engine.switches.last_mut() {
            ctx.pending_root = Some(space);
        }
    }

    fn end_switch_expression_case(
        &mut self,
        _begin: Token,
        when: Option<Token>,
        arrow: Token,
        _end: Token,
    ) {
        self.flow
            .borrow_mut()
            .end_switch_expression_case(_begin, when, arrow, _end);

        let mut engine = self.engine.borrow_mut();
        let space = engine
            .switches
            .last_mut()
            .and_then(|ctx| ctx.pending_root.take())
            .unwrap_or(Restriction::Unknown);
        if let Some(ctx) = engine.switches.last_mut() {
            ctx.cases.push(NameCase {
                space,
                guarded: when.is_some(),
                anchor: arrow.range,
            });
        }
    }

    fn end_switch_expression_block(&mut self, _count: usize, _begin: Token, _end: Token) {
        self.flow
            .borrow_mut()
            .end_switch_expression_block(_count, _begin, _end);

        self.engine.borrow_mut().report_switch();
    }

    // —— if-case ————————————————————————————————————————————————————————

    fn handle_parenthesized_condition(
        &mut self,
        _begin: Token,
        case_kw: Option<Token>,
        _when: Option<Token>,
    ) {
        // An if-case's pattern events follow; the binding face works
        // through `handle_declared_variable_pattern`, and the condition
        // itself carries no exhaustiveness question. Its root pattern is
        // never captured: the case-sink window is only open across a
        // switch-case header.
        let _ = case_kw;
    }

    fn handle_enum_element(&mut self, _begin: Token, _augment: Option<Token>) {
        // The parser fires this after the constant's type arguments and
        // constructor arguments: the constant's window closed (it opened at
        // the EnumValueDeclaration identifier).
        let mut engine = self.engine.borrow_mut();
        engine.close_enum_const_element();
        engine.leave_enum_element();
    }

    fn handle_enum_elements(&mut self, _elements_end: Token, _count: usize) {
        let mut engine = self.engine.borrow_mut();
        engine.close_enum_const_element();
        engine.leave_enum_element();
    }

    fn begin_type_arguments(&mut self, _token: Token) {
        let mut engine = self.engine.borrow_mut();
        let head = engine.pending_type_head.take();
        let base = engine.rows.len();
        engine.group_heads.push((head, base));
    }

    fn end_type_arguments(&mut self, count: usize, begin: Token, end: Token) {
        self.engine
            .borrow_mut()
            .end_type_arguments(count, begin, end);
    }

    fn handle_no_type_arguments(&mut self, _token: Token) {
        // The type completed without a group; the head slot is consumed.
        self.engine.borrow_mut().pending_type_head = None;
    }

    fn handle_type_argument_application(&mut self, _open_angle: Token) {
        self.engine.borrow_mut().consume_type_argument_application();
    }

    // —— value rows: literals and groups ———————————————————————————————
    // Adjacent string literals push no extra row: each constituent string
    // already did.

    fn handle_literal_int(&mut self, token: Token) {
        self.flow.borrow_mut().handle_literal_int(token);

        self.engine.borrow_mut().push_primitive_row(token, 0);
        self.engine.borrow_mut().const_int_literal(token);
    }

    fn handle_literal_int_with_separators(&mut self, token: Token) {
        self.flow
            .borrow_mut()
            .handle_literal_int_with_separators(token);

        self.engine.borrow_mut().push_primitive_row(token, 0);
        self.engine.borrow_mut().const_int_literal(token);
    }

    fn handle_literal_double(&mut self, token: Token) {
        self.flow.borrow_mut().handle_literal_double(token);

        self.engine.borrow_mut().push_primitive_row(token, 1);
        self.engine.borrow_mut().const_double_literal(token);
    }

    fn handle_literal_double_with_separators(&mut self, token: Token) {
        self.flow
            .borrow_mut()
            .handle_literal_double_with_separators(token);

        self.engine.borrow_mut().push_primitive_row(token, 1);
        self.engine.borrow_mut().const_double_literal(token);
    }

    fn handle_literal_bool(&mut self, token: Token) {
        self.flow.borrow_mut().handle_literal_bool(token);

        let mut engine = self.engine.borrow_mut();
        if engine.pattern_depth > 0 {
            let is_true = engine.text_of(token) == Some("true");
            engine.pending_const = Some(Restriction::BoolValue(is_true));
        }
        engine.push_primitive_row(token, 3);
        engine.const_bool_literal(token);
    }

    fn handle_literal_null(&mut self, token: Token) {
        self.flow.borrow_mut().handle_literal_null(token);

        let mut engine = self.engine.borrow_mut();
        if engine.pattern_depth > 0 {
            engine.pending_const = Some(Restriction::Null);
        }
        engine.push_null_row(token);
        engine.const_null_literal(token);
    }

    fn begin_literal_string(&mut self, begin: Token) {
        let mut engine = self.engine.borrow_mut();
        engine.const_string_begins.push(begin.range);
        engine.const_capture.begin_literal_string();
    }

    fn handle_interpolation_expression(&mut self, left: Token, _right: Option<Token>) {
        self.engine
            .borrow_mut()
            .const_capture
            .interpolation_expression(left.range);
    }

    fn end_literal_string(&mut self, interpolation_count: usize, end: Token) {
        self.flow
            .borrow_mut()
            .end_literal_string(interpolation_count, end);

        let mut engine = self.engine.borrow_mut();
        // The begin span arrives via the capture's own string marks (the
        // engine latched it at `begin_literal_string` — carried on the
        // event's `end` counterpart below).
        let begin = engine.const_string_begins.pop().unwrap_or(end.range);
        if interpolation_count == 0 {
            engine.const_string_leaf(begin, end.range);
        } else {
            engine.const_string_end(interpolation_count, begin, end.range);
        }
        engine.push_primitive_row(end, 4);
    }

    fn end_literal_symbol(&mut self, hash: Token, _identifier_count: usize) {
        self.engine.borrow_mut().push_value_row(hash);
        self.engine.borrow_mut().const_capture.symbol(hash.range);
    }

    fn handle_adjacent_string_literals(&mut self, start: Token, literal_count: usize) {
        self.engine
            .borrow_mut()
            .const_capture
            .adjacent_strings(literal_count, start.range);
    }

    fn handle_literal_list(
        &mut self,
        count: usize,
        left: Token,
        _const_keyword: Option<Token>,
        right: Token,
    ) {
        self.flow
            .borrow_mut()
            .handle_literal_list(count, left, _const_keyword, right);

        let mut engine = self.engine.borrow_mut();
        if let Some(elements) = engine.const_capture.literal_list(count, left.range) {
            // Implicit const literals in a const context check per element
            // with the element code too (the oracle evaluates the literal
            // at the enclosing site).
            engine.check_const_elements(SemanticCode::NonConstantListElement, &elements);
            engine.const_capture.push_value(
                right.range,
                ConstantValue::Instance(crate::const_eval::InstanceKind::List),
            );
        }
        engine.push_value_row(right);
    }

    fn handle_literal_set_or_map(
        &mut self,
        count: usize,
        left: Token,
        _const_keyword: Option<Token>,
        right: Token,
        has_set_entry: bool,
    ) {
        self.flow.borrow_mut().handle_literal_set_or_map(
            count,
            left,
            _const_keyword,
            right,
            has_set_entry,
        );

        use crate::const_eval::{InstanceKind, SetOrMap};
        let mut engine = self.engine.borrow_mut();
        if let Some(shape) =
            engine
                .const_capture
                .literal_set_or_map(count, left.range, has_set_entry)
        {
            match shape {
                SetOrMap::Map(entries) => {
                    let flat: Vec<_> = entries.into_iter().flat_map(|(k, v)| [k, v]).collect();
                    engine.check_const_elements(SemanticCode::NonConstantMapElement, &flat);
                    engine
                        .const_capture
                        .push_value(right.range, ConstantValue::Instance(InstanceKind::Map));
                }
                SetOrMap::Set(elements) => {
                    engine.check_const_elements(SemanticCode::NonConstantSetElement, &elements);
                    engine
                        .const_capture
                        .push_value(right.range, ConstantValue::Instance(InstanceKind::Set));
                }
            }
        }
        engine.push_value_row(right);
    }

    fn handle_literal_map_entry(
        &mut self,
        colon: Token,
        _end: Token,
        _nk: Option<Token>,
        _nv: Option<Token>,
    ) {
        self.flow
            .borrow_mut()
            .handle_literal_map_entry(colon, _end, _nk, _nv);

        self.engine
            .borrow_mut()
            .const_capture
            .literal_map_entry(colon.range);
    }

    fn end_parenthesized_expression(&mut self, token: Token) {
        self.flow.borrow_mut().end_parenthesized_expression(token);

        self.engine.borrow_mut().push_value_row(token);
        self.engine.borrow_mut().const_capture.parenthesized_end();
    }

    fn end_record_literal(&mut self, token: Token, count: usize, _const_keyword: Option<Token>) {
        self.flow
            .borrow_mut()
            .end_record_literal(token, count, _const_keyword);

        let mut engine = self.engine.borrow_mut();
        if let Some(fields) = engine.const_capture.record_literal(count, token.range) {
            engine.check_const_elements(SemanticCode::NonConstantRecordField, &fields);
            engine.const_capture.push_value(
                token.range,
                ConstantValue::Instance(crate::const_eval::InstanceKind::Record),
            );
        }
        engine.push_value_row(token);
    }

    fn end_const_literal(&mut self, end: Token) {
        let mut engine = self.engine.borrow_mut();
        engine.const_capture.close_explicit_literal();
        engine.push_value_row(end);
    }

    // —— #505: directives ———————————————————————————————————————————————

    fn begin_import(&mut self, import_keyword: Token) {
        self.engine.borrow_mut().begin_import_site(import_keyword);
    }

    // —— #279: const contexts ————————————————————————————————————————

    fn begin_fields(
        &mut self,
        _kind: DeclarationKind,
        _augment_kw: Option<Token>,
        _abstract_kw: Option<Token>,
        _external_kw: Option<Token>,
        static_kw: Option<Token>,
        _covariant_kw: Option<Token>,
        _late_kw: Option<Token>,
        var_final_or_const: Option<Token>,
        _last_consumed: Token,
    ) {
        let mut engine = self.engine.borrow_mut();
        if var_final_or_const.and_then(|t| engine.text_of(t)) == Some("const") {
            engine.arm_const_group();
        }
        engine.open_member_static = static_kw.is_some();
    }

    fn end_fields(
        &mut self,
        _kind: DeclarationKind,
        _abstract_kw: Option<Token>,
        _augment_kw: Option<Token>,
        _external_kw: Option<Token>,
        _static_kw: Option<Token>,
        _covariant_kw: Option<Token>,
        _late_kw: Option<Token>,
        _var_final_or_const: Option<Token>,
        _count: usize,
        _begin: Token,
        _end: Token,
    ) {
        self.engine.borrow_mut().disarm_const_group();
    }

    fn end_top_level_fields(
        &mut self,
        _augment_kw: Option<Token>,
        _abstract_kw: Option<Token>,
        _external_kw: Option<Token>,
        _static_kw: Option<Token>,
        _covariant_kw: Option<Token>,
        _late_kw: Option<Token>,
        _var_final_or_const: Option<Token>,
        _count: usize,
        _begin: Token,
        _end: Token,
    ) {
        self.engine.borrow_mut().disarm_const_group();
    }

    fn begin_field_initializer(&mut self, _token: Token) {
        self.engine.borrow_mut().open_const_initializer();
    }

    fn end_field_initializer(&mut self, _assignment: Token, _end: Token) {
        self.engine.borrow_mut().close_const_field_initializer();
    }

    fn begin_const_literal(&mut self, _token: Token) {
        self.engine
            .borrow_mut()
            .const_capture
            .open_explicit_literal();
    }

    fn end_constructor_reference(
        &mut self,
        start: Token,
        _period_before_name: Option<Token>,
        _end: Token,
        context: ConstructorReferenceContext,
    ) {
        let mut engine = self.engine.borrow_mut();
        if context == ConstructorReferenceContext::Const {
            engine.arm_const_ctor_gate(start);
        }
        // The reference closed — its segments stop matching type-argument
        // groups (a plain invocation's group inside `new C(...)`'s
        // arguments must not).
        engine.creation_ref = None;
    }

    fn begin_await_expression(&mut self, token: Token) {
        self.flow.borrow_mut().begin_await_expression(token);

        self.engine.borrow_mut().const_capture.begin_opaque_group();
        let _ = token;
    }

    fn end_await_expression(&mut self, begin: Token, _end: Token) {
        self.flow.borrow_mut().end_await_expression(begin, _end);

        self.engine
            .borrow_mut()
            .const_capture
            .end_opaque_group(begin.range);
    }

    fn handle_throw_expression(&mut self, throw: Token, _end: Token) {
        self.flow.borrow_mut().handle_throw_expression(throw, _end);

        self.engine
            .borrow_mut()
            .const_capture
            .throw_expression(throw.range);
    }

    fn handle_non_null_assert_expression(&mut self, bang: Token) {
        self.flow
            .borrow_mut()
            .handle_non_null_assert_expression(bang);

        self.engine
            .borrow_mut()
            .const_capture
            .non_null_assert(bang.range);
    }

    // —— flow fan-out: events the diagnoser does not consume itself ———
    // Forwarded verbatim to the per-unit flow recorder (see the module
    // docs' flow section).

    fn begin_assert(&mut self, token: Token, kind: AssertKind) {
        self.flow.borrow_mut().begin_assert(token, kind);
    }

    fn begin_block(&mut self, token: Token, kind: BlockKind) {
        self.flow.borrow_mut().begin_block(token, kind);
    }

    fn begin_block_function_body(&mut self, token: Token) {
        self.flow.borrow_mut().begin_block_function_body(token);
    }

    fn begin_do_while_statement(&mut self, token: Token) {
        self.flow.borrow_mut().begin_do_while_statement(token);
    }

    fn begin_do_while_statement_body(&mut self, token: Token) {
        self.flow.borrow_mut().begin_do_while_statement_body(token);
    }

    fn begin_else_statement(&mut self, token: Token) {
        self.flow.borrow_mut().begin_else_statement(token);
    }

    fn begin_for_control_flow(&mut self, await_kw: Option<Token>, for_kw: Token) {
        self.flow
            .borrow_mut()
            .begin_for_control_flow(await_kw, for_kw);
    }

    fn begin_for_in_body(&mut self, token: Token) {
        self.flow.borrow_mut().begin_for_in_body(token);
    }

    fn begin_for_in_expression(&mut self, token: Token) {
        self.flow.borrow_mut().begin_for_in_expression(token);
    }

    fn begin_for_statement_body(&mut self, token: Token) {
        self.flow.borrow_mut().begin_for_statement_body(token);
    }

    fn begin_if_control_flow(&mut self, token: Token) {
        self.flow.borrow_mut().begin_if_control_flow(token);
    }

    fn begin_if_statement(&mut self, token: Token) {
        self.flow.borrow_mut().begin_if_statement(token);
    }

    fn begin_initialized_identifier(&mut self, token: Token) {
        self.flow.borrow_mut().begin_initialized_identifier(token);
    }

    fn begin_labeled_statement(&mut self, token: Token, label_count: usize) {
        self.flow
            .borrow_mut()
            .begin_labeled_statement(token, label_count);
    }

    fn begin_literal_symbol(&mut self, token: Token) {
        self.flow.borrow_mut().begin_literal_symbol(token);
    }

    fn begin_pattern_guard(&mut self, when: Token) {
        self.flow.borrow_mut().begin_pattern_guard(when);
    }

    fn begin_rethrow_statement(&mut self, token: Token) {
        self.flow.borrow_mut().begin_rethrow_statement(token);
    }

    fn begin_switch_case(&mut self, label_count: usize, expression_count: usize, token: Token) {
        self.flow
            .borrow_mut()
            .begin_switch_case(label_count, expression_count, token);

        // A case's pattern variables form their own group window.
        self.engine.borrow_mut().pattern_group_mark = None;
    }

    fn begin_then_statement(&mut self, token: Token) {
        self.flow.borrow_mut().begin_then_statement(token);
    }

    fn begin_try_statement(&mut self, token: Token) {
        self.flow.borrow_mut().begin_try_statement(token);

        let mut engine = self.engine.borrow_mut();
        engine.try_clauses.push(Vec::new());
    }

    fn begin_while_statement(&mut self, token: Token) {
        self.flow.borrow_mut().begin_while_statement(token);
    }

    fn begin_while_statement_body(&mut self, token: Token) {
        self.flow.borrow_mut().begin_while_statement_body(token);
    }

    fn end_assert(
        &mut self,
        begin: Token,
        kind: AssertKind,
        left_paren: Token,
        comma: Option<Token>,
        end: Token,
    ) {
        self.flow
            .borrow_mut()
            .end_assert(begin, kind, left_paren, comma, end);
    }

    fn end_block(&mut self, count: usize, begin: Token, end: Token, kind: BlockKind) {
        self.flow.borrow_mut().end_block(count, begin, end, kind);

        // The catch clause fact's body anchor: `handle_catch_block` fires
        // right after the clause body's block ends, so the last block end
        // is its closing brace.
        self.engine.borrow_mut().last_block_end = Some(end.range);
    }

    fn end_block_function_body(&mut self, count: usize, begin: Token, end: Token) {
        self.flow
            .borrow_mut()
            .end_block_function_body(count, begin, end);
    }

    fn end_do_while_statement(&mut self, token: Token, while_kw: Token, end: Token) {
        self.flow
            .borrow_mut()
            .end_do_while_statement(token, while_kw, end);
    }

    fn end_do_while_statement_body(&mut self, end: Token) {
        self.flow.borrow_mut().end_do_while_statement_body(end);
    }

    fn end_for_control_flow(&mut self, token: Token) {
        self.flow.borrow_mut().end_for_control_flow(token);
    }

    fn end_for_in_body(&mut self, end: Token) {
        self.flow.borrow_mut().end_for_in_body(end);
    }

    fn end_for_in_control_flow(&mut self, token: Token) {
        self.flow.borrow_mut().end_for_in_control_flow(token);
    }

    fn end_for_in_expression(&mut self, end: Token) {
        self.flow.borrow_mut().end_for_in_expression(end);
    }

    fn end_for_statement_body(&mut self, end: Token) {
        self.flow.borrow_mut().end_for_statement_body(end);
    }

    #[allow(clippy::too_many_arguments)]
    fn end_formal_parameter(
        &mut self,
        var_or_final: Option<Token>,
        this_keyword: Option<Token>,
        super_keyword: Option<Token>,
        period_after_this_or_super: Option<Token>,
        name: Token,
        initializer_start: Option<Token>,
        initializer_end: Option<Token>,
        kind: FormalParameterKind,
        member_kind: dartq_parser::MemberKind,
    ) {
        self.flow.borrow_mut().end_formal_parameter(
            var_or_final,
            this_keyword,
            super_keyword,
            period_after_this_or_super,
            name,
            initializer_start,
            initializer_end,
            kind,
            member_kind,
        );
    }

    fn end_function_type(&mut self, function: Token, question: Option<Token>) {
        self.flow.borrow_mut().end_function_type(function, question);
    }

    fn end_function_typed_formal_parameter(&mut self, token: Token, question: Option<Token>) {
        self.flow
            .borrow_mut()
            .end_function_typed_formal_parameter(token, question);
    }

    fn end_if_control_flow(&mut self, token: Token) {
        self.flow.borrow_mut().end_if_control_flow(token);
    }

    fn end_if_else_control_flow(&mut self, token: Token) {
        self.flow.borrow_mut().end_if_else_control_flow(token);
    }

    fn end_if_statement(&mut self, token: Token, else_kw: Option<Token>, end: Token) {
        self.flow.borrow_mut().end_if_statement(token, else_kw, end);
    }

    fn end_initialized_identifier(&mut self, name: Token) {
        self.flow.borrow_mut().end_initialized_identifier(name);
    }

    fn end_invalid_await_expression(&mut self, begin: Token, end: Token) {
        self.flow
            .borrow_mut()
            .end_invalid_await_expression(begin, end);
    }

    fn end_invalid_yield_statement(&mut self, begin: Token, star: Option<Token>, end: Token) {
        self.flow
            .borrow_mut()
            .end_invalid_yield_statement(begin, star, end);
    }

    fn end_labeled_statement(&mut self, label_count: usize) {
        self.flow.borrow_mut().end_labeled_statement(label_count);

        self.engine.borrow_mut().finish_labeled_statement(label_count);
    }

    fn end_pattern_guard(&mut self, token: Token) {
        self.flow.borrow_mut().end_pattern_guard(token);
    }

    fn end_rethrow_statement(&mut self, rethrow_kw: Token, end: Token) {
        self.flow
            .borrow_mut()
            .end_rethrow_statement(rethrow_kw, end);
    }

    fn end_switch_statement(&mut self, token: Token, end: Token) {
        self.flow.borrow_mut().end_switch_statement(token, end);
    }

    fn end_try_statement(
        &mut self,
        count: usize,
        begin: Token,
        finally_kw: Option<Token>,
        end: Token,
    ) {
        self.flow
            .borrow_mut()
            .end_try_statement(count, begin, finally_kw, end);

        self.engine.borrow_mut().finish_try_statement();
    }

    fn end_while_statement(&mut self, token: Token, end: Token) {
        self.flow.borrow_mut().end_while_statement(token, end);
    }

    fn end_while_statement_body(&mut self, end: Token) {
        self.flow.borrow_mut().end_while_statement_body(end);
    }

    fn end_yield_statement(&mut self, begin: Token, star: Option<Token>, end: Token) {
        self.flow.borrow_mut().end_yield_statement(begin, star, end);
    }

    fn handle_assigned_variable_pattern(&mut self, variable: Token) {
        self.flow
            .borrow_mut()
            .handle_assigned_variable_pattern(variable);
    }

    fn handle_break_statement(&mut self, has_target: bool, break_kw: Token, end: Token) {
        self.flow
            .borrow_mut()
            .handle_break_statement(has_target, break_kw, end);
    }

    fn handle_catch_block(
        &mut self,
        on_kw: Option<Token>,
        catch_kw: Option<Token>,
        comma: Option<Token>,
    ) {
        self.flow
            .borrow_mut()
            .handle_catch_block(on_kw, catch_kw, comma);

        self.engine.borrow_mut().finish_catch_clause();
    }

    fn handle_continue_statement(&mut self, has_target: bool, continue_kw: Token, end: Token) {
        self.flow
            .borrow_mut()
            .handle_continue_statement(has_target, continue_kw, end);
    }

    fn handle_else_control_flow(&mut self, token: Token) {
        self.flow.borrow_mut().handle_else_control_flow(token);
    }

    fn handle_empty_function_body(&mut self, semicolon: Token) {
        self.flow.borrow_mut().handle_empty_function_body(semicolon);
    }

    fn handle_experiment_not_enabled(&mut self, flag: ExperimentalFlag, begin: Token, end: Token) {
        self.flow
            .borrow_mut()
            .handle_experiment_not_enabled(flag, begin, end);
    }

    fn handle_expression_statement(&mut self, begin: Token, end: Token) {
        self.flow
            .borrow_mut()
            .handle_expression_statement(begin, end);

        self.engine.borrow_mut().check_statement_use(begin, end);
    }

    fn handle_finally_block(&mut self, finally_kw: Token) {
        self.flow.borrow_mut().handle_finally_block(finally_kw);
    }

    fn handle_for_initializer_pattern_variable_assignment(
        &mut self,
        pattern_keyword: Token,
        equals: Token,
    ) {
        self.flow
            .borrow_mut()
            .handle_for_initializer_pattern_variable_assignment(pattern_keyword, equals);
    }

    fn handle_for_loop_parts(
        &mut self,
        for_kw: Token,
        left_paren: Token,
        left_separator: Token,
        right_separator: Token,
        expression_count: usize,
    ) {
        self.flow.borrow_mut().handle_for_loop_parts(
            for_kw,
            left_paren,
            left_separator,
            right_separator,
            expression_count,
        );
    }

    fn handle_invalid_function_body(&mut self, token: Token) {
        self.flow.borrow_mut().handle_invalid_function_body(token);
    }

    fn handle_invalid_statement(&mut self, begin: Token) {
        self.flow.borrow_mut().handle_invalid_statement(begin);
    }

    fn handle_label(&mut self, token: Token) {
        self.flow.borrow_mut().handle_label(token);

        self.engine.borrow_mut().note_label_decl();
    }

    fn handle_no_variable_initializer(&mut self, token: Token) {
        self.flow.borrow_mut().handle_no_variable_initializer(token);
    }

    fn handle_null_aware_element(&mut self, token: Token) {
        self.flow.borrow_mut().handle_null_aware_element(token);
    }

    fn handle_pattern_assignment(&mut self, equals: Token) {
        self.flow.borrow_mut().handle_pattern_assignment(equals);
    }

    fn handle_pattern_variable_declaration_statement(
        &mut self,
        keyword: Token,
        equals: Token,
        semicolon: Token,
    ) {
        self.flow
            .borrow_mut()
            .handle_pattern_variable_declaration_statement(keyword, equals, semicolon);

        // The statement boundary closes the group window (the next
        // declaration is its own all-or-nothing set).
        self.engine.borrow_mut().pattern_group_mark = None;
    }

    fn handle_recoverable_error(&mut self, _p0: Message, _p1: Token, _p2: Token) {
        self.flow
            .borrow_mut()
            .handle_recoverable_error(_p0, _p1, _p2);
    }

    fn handle_send_without_arguments(&mut self, begin: Token, end: Token, next: Token) {
        self.flow
            .borrow_mut()
            .handle_send_without_arguments(begin, end, next);
    }

    fn handle_spread_expression(&mut self, operator: Token) {
        self.flow.borrow_mut().handle_spread_expression(operator);
    }

    fn handle_switch_case_no_when_clause(&mut self, token: Token) {
        self.flow
            .borrow_mut()
            .handle_switch_case_no_when_clause(token);
    }

    fn handle_then_control_flow(&mut self, token: Token) {
        self.flow.borrow_mut().handle_then_control_flow(token);
    }
}

/// The §8 names-table use kind of a row's finalized intent.
fn use_kind_of(intent: &Intent) -> NameUse {
    match intent {
        Intent::Invoke => NameUse::Invoke,
        Intent::Write | Intent::ReadWrite => NameUse::Write,
        _ => NameUse::Read,
    }
}

// —— records/patterns/exhaustiveness (M2 face) ————————————————————————————

/// The static type the semantic-level exhaustiveness walk uses: the M1
/// [`Ty`] lattice plus the restriction leaves the case patterns produce
/// (enum constants, `bool` values). Names resolve through the unit's
/// element model, which is file-scoped — the cross-unit-sealed blind spot
/// is no longer a silent under-report: an unresolvable scrutinee name
/// suppresses the expression-face covering emit outright
/// ([`NameWorld::name_resolvable`]), and a cyclic in-file sealed
/// hierarchy terminates through the walk's visited set.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum NameTy {
    Never,
    Null,
    /// `dynamic` — the nullable top.
    TopNullable,
    TopNonNullable,
    /// A named type as written, with the syntactic `?`. The nullable
    /// projection doubles as the engine's `Nullable` variant.
    Named(StringId, bool),
    /// An enum constant: the enum's name and the constant's.
    EnumValue(StringId, StringId),
    BoolValue(bool),
    /// The nullable projection of an enum constant
    /// (`NullableStaticType(EnumElementStaticType)`, static_type.dart:350):
    /// the constant plus `null`, never its siblings.
    EnumValueOrNull(StringId, StringId),
    /// The nullable projection of a `true`/`false` value
    /// (`NullableStaticType(_BoolValueStaticType)`, static_type.dart:350):
    /// the value plus `null`, never the other boolean.
    BoolOrNull(bool),
    /// A unique unknown (relational patterns, unrepresentable constants):
    /// matched by nothing.
    Unknown(u64),
}

/// One completed case: its coverage restriction, the guard flag, and the
/// token the diagnostics anchor to (`case` for statement cases, `=>` for
/// expression cases).
struct NameCase {
    space: Restriction,
    guarded: bool,
    anchor: SourceRange,
}

/// The open switch (statement or expression): the scrutinee's push-time
/// type, its keyword anchor, and the cases completed so far.
struct SwitchCtx {
    is_expression: bool,
    switch_kw: SourceRange,
    scrutinee: Ty,
    cases: Vec<NameCase>,
    /// The `default` clause's keyword anchor (the unreachable-default row's
    /// token, `constant_verifier.dart:1141-1148`); `None` when the switch
    /// has no default.
    default_kw: Option<SourceRange>,
    /// The captured coverage of an in-flight switch-expression case (the
    /// space latches at the pattern's completion, the guard and anchor at
    /// the case's end). Statement cases capture in one step and leave this
    /// `None`.
    pending_root: Option<Restriction>,
}

/// One open `begin_pattern`..`end_pattern` bracket: the leaf restrictions
/// collected inside it, whether `||` joined any of them (an `||` union
/// keeps its alternatives apart; everything else intersects), and the
/// object-pattern field buckets the `handle_pattern_field` boundaries
/// drained into it (the property columns of the official object-pattern
/// space — frame-scoped, so a record pattern's unused buckets die with its
/// own bracket).
struct PatternFrame {
    leaves: Vec<Restriction>,
    or_ops: usize,
    fields: Vec<ObjectField>,
}

/// One object-pattern field's drained sub-restrictions (empty for a
/// type-free `var`/`_` sub-pattern, which covers the whole column).
struct ObjectField {
    /// The field's member name (`x` of `x: pattern`). `None` for
    /// positional fields — the member table keeps no declaration order, so
    /// their columns are unresolvable at name level.
    name: Option<StringId>,
    leaves: Vec<Restriction>,
}

/// The name-level coverage a pattern leaf contributes (the space-building
/// face of `PatternConverter`, constant_verifier.dart:1031-1050).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Restriction {
    /// A type test as written (`X`, `X?`): cast patterns, typed variable
    /// patterns, object patterns.
    Named(StringId, bool),
    /// The `null` constant (the only value-precise constant at name level).
    Null,
    /// `true`/`false`.
    BoolValue(bool),
    /// A qualified enum constant `E.c`.
    EnumValue(StringId, StringId),
    /// Alternatives (`a || b`).
    Union(Vec<Restriction>),
    /// The type-free coverage (`var x`, `_`): substituted with the
    /// scrutinee's context type when the case closes.
    NamedContext,
    /// The unknown space: relational patterns, list/map patterns, and
    /// constants the name level cannot represent. Never must-matches, so
    /// the engine under-reports instead of mis-answering.
    Unknown,
}

impl Restriction {
    /// `&&`-conjunction at name level: the single side when both spell the
    /// same test, unknown otherwise (an incomparable conjunction's
    /// coverage is not one row type). By value because
    /// `reduce(Restriction::and)` consumes the frame's leaves.
    #[allow(clippy::needless_pass_by_value)]
    fn and(self, other: Restriction) -> Restriction {
        if self == Restriction::Unknown || other == Restriction::Unknown {
            return Restriction::Unknown;
        }
        if self == other {
            return self;
        }
        Restriction::Unknown
    }
}

/// Builds one case's engine space from its restriction over `world`.
fn case_space(world: &NameWorld<'_>, space: &Restriction, context: NameTy) -> Space<NameTy> {
    let mut singles = Vec::new();
    collect_singles(world, space, context, &mut singles);
    Space::from_singles(world, Vec::new(), singles)
}

/// Flattens a restriction into the union's single spaces.
fn collect_singles(
    world: &NameWorld<'_>,
    space: &Restriction,
    context: NameTy,
    out: &mut Vec<Single<NameTy>>,
) {
    match space {
        Restriction::Union(parts) => {
            for part in parts {
                collect_singles(world, part, context, out);
            }
        }
        Restriction::Unknown => {
            out.push(Single::full(world.unknown()));
        }
        Restriction::Null => out.push(Single::full(NameTy::Null)),
        // Typed leaves (`X`, `X?`, `E.c`, `true`/`false`): one type-test
        // single (official `convertTypePattern`/`convertConstantPattern`).
        Restriction::Named(name, nullable) => {
            out.push(Single::full(NameTy::Named(*name, *nullable)));
        }
        Restriction::BoolValue(value) => out.push(Single::full(NameTy::BoolValue(*value))),
        Restriction::EnumValue(enum_name, constant) => {
            out.push(Single::full(NameTy::EnumValue(*enum_name, *constant)));
        }
        // Substituted before the case is recorded; unreachable here.
        Restriction::NamedContext => out.push(Single::full(context)),
    }
}

/// The name-level [`ExhaustivenessWorld`]: sealed/enum facts come from the
/// unit's element model, subtype edges from the superclass and
/// `with`/`implements` names of the unit's containers, plus the fixed core
/// chain (`int`/`double` <: `num` <: `Object`) the nullability codes
/// already assume.
struct NameWorld<'a> {
    index: &'a ElementIndex,
    file: FileId,
    strings: &'a StringTable,
    /// The interned `bool` spelling (`BoolValue` subtyping compares ids,
    /// never re-interns per query).
    bool_id: StringId,
    unknowns: std::cell::Cell<u64>,
}

/// The two core edges every unit sees without a declaration (`num` has no
/// container; the nullability face already treats these names specially).
const CORE_SUPERTYPES: [(&str, &str); 3] = [("int", "num"), ("double", "num"), ("num", "Object")];

impl NameTy {
    fn from_semantic(ty: Ty) -> Self {
        match ty {
            Ty::Unknown | Ty::Dynamic | Ty::Void => NameTy::TopNullable,
            Ty::Null => NameTy::Null,
            Ty::Named { name, nullable } => NameTy::Named(name, nullable),
        }
    }
}

impl ExhaustivenessWorld for NameWorld<'_> {
    type Ty = NameTy;

    fn object_nullable(&self) -> NameTy {
        NameTy::TopNullable
    }

    fn never(&self) -> NameTy {
        NameTy::Never
    }

    fn null(&self) -> NameTy {
        NameTy::Null
    }

    fn unknown(&self) -> NameTy {
        let next = self.unknowns.get();
        self.unknowns.set(next.wrapping_add(1));
        NameTy::Unknown(next)
    }

    fn nullable(&self, ty: &NameTy) -> NameTy {
        match ty {
            // The nullable projection of a name.
            NameTy::Named(name, _) => NameTy::Named(*name, true),
            // A restricted value's projection keeps the restriction
            // (`NullableStaticType` over the value static type,
            // static_type.dart:350-383, `NonNullableStaticType.nullable`):
            // it covers the value and `null`, never the siblings.
            NameTy::EnumValue(enum_name, constant) => {
                NameTy::EnumValueOrNull(*enum_name, *constant)
            }
            NameTy::BoolValue(value) => NameTy::BoolOrNull(*value),
            NameTy::TopNonNullable => NameTy::TopNullable,
            other => *other,
        }
    }

    // A type lattice: several arms legitimately answer plain `true`.
    #[allow(clippy::match_same_arms)]
    fn is_subtype_of(&self, sub: &NameTy, sup: &NameTy) -> bool {
        if sub == sup {
            return true;
        }
        match (sub, sup) {
            // Everything subtypes the nullable top (static_type.dart:166);
            // `Never` subtypes everything; `Null <: T?` — through the
            // restricted-value nullable projections too
            // (`NullableStaticType.isSubtypeOf`, the underlying chain ends
            // in `Never`).
            (_, NameTy::TopNullable)
            | (NameTy::Never, _)
            | (
                NameTy::Null,
                NameTy::Named(_, true) | NameTy::EnumValueOrNull(..) | NameTy::BoolOrNull(_),
            ) => true,
            // `S <: T?` iff `S <: T` (static_type.dart:365-372); `S? <: T?`
            // likewise; `S? <: T` never (null matches no plain T).
            (NameTy::Named(sn, s), NameTy::Named(tn, t)) => (!*s || *t) && self.named_sub(*sn, *tn),
            // The bare value fits its own nullable projection (the
            // must-match rule: `true <: true?` through the identical
            // restriction, types.dart:52-56).
            (NameTy::EnumValue(en, c), NameTy::EnumValueOrNull(fn_, d)) => en == fn_ && c == d,
            (NameTy::BoolValue(v), NameTy::BoolOrNull(w)) => v == w,
            // All non-nullable types are subtypes of Object
            // (`NonNullableStaticType.isSubtypeOf`, static_type.dart
            // :415-421).
            (
                NameTy::Named(_, false) | NameTy::EnumValue(..) | NameTy::BoolValue(_),
                NameTy::TopNonNullable,
            ) => true,
            // A constant's type is its enum, non-null.
            (NameTy::EnumValue(en, _), sup) => self.is_subtype_of(&NameTy::Named(*en, false), sup),
            // The restricted nullable inherits the value's type edges, but
            // only toward nullable targets (its `null` half never fits a
            // non-nullable type).
            (NameTy::EnumValueOrNull(en, _), NameTy::Named(tn, true)) => {
                self.is_subtype_of(&NameTy::Named(*en, false), &NameTy::Named(*tn, true))
            }
            (NameTy::BoolOrNull(_), NameTy::Named(tn, true)) => self.is_subtype_of(
                &NameTy::Named(self.bool_id, false),
                &NameTy::Named(*tn, true),
            ),
            (NameTy::BoolValue(_), sup) => {
                self.is_subtype_of(&NameTy::Named(self.bool_id, false), sup)
            }
            (NameTy::TopNonNullable, NameTy::TopNonNullable) => true,
            _ => false,
        }
    }

    fn is_sealed(&self, ty: &NameTy) -> bool {
        match ty {
            // Nullable projections split (static_type.dart:356).
            NameTy::Named(_, true)
            | NameTy::TopNullable
            | NameTy::EnumValueOrNull(..)
            | NameTy::BoolOrNull(_) => true,
            NameTy::Named(name, false) => self.is_sealed_name(*name),
            _ => false,
        }
    }

    fn subtypes(&self, ty: &NameTy) -> Vec<NameTy> {
        match ty {
            NameTy::Named(name, true) => vec![NameTy::Named(*name, false), NameTy::Null],
            NameTy::TopNullable => vec![NameTy::TopNonNullable, NameTy::Null],
            // NullableStaticType.getSubtypes: the value plus `null`
            // (static_type.dart:359-362).
            NameTy::EnumValueOrNull(enum_name, constant) => {
                vec![NameTy::EnumValue(*enum_name, *constant), NameTy::Null]
            }
            NameTy::BoolOrNull(value) => vec![NameTy::BoolValue(*value), NameTy::Null],
            NameTy::Named(name, false) => self.name_subtypes(*name),
            _ => Vec::new(),
        }
    }

    /// `getPropertyType` (static_type.dart:36-46): the declared type of
    /// the instance member `key` names, or `None` when the world does not
    /// know it (the checker's columns then fall back to the nullable
    /// object, exhaustive.dart:396-406).
    fn property_type(&self, ty: &NameTy, key: &Key) -> Option<NameTy> {
        let NameTy::Named(name, _) = ty else {
            return None;
        };
        let Key::Name(member) = key else {
            // Record keys are not modeled at name level.
            return None;
        };
        self.member_column(*name, self.strings.intern(member))
    }

    fn is_bottom(&self, ty: &NameTy) -> bool {
        matches!(ty, NameTy::Never)
    }

    fn is_null(&self, ty: &NameTy) -> bool {
        matches!(ty, NameTy::Null)
    }

    fn witness_text(&self, ty: &NameTy, fields: &[(Key, String)]) -> String {
        if fields.is_empty() {
            return self.ty_text(ty);
        }
        let inner = fields
            .iter()
            .map(|(key, text)| match key {
                Key::RecordIndex(index) => format!("${}:{text}", index + 1),
                Key::RecordName(name) | Key::Name(name) => format!("{name}: {text}"),
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("{}({inner})", self.ty_text(ty))
    }

    fn display(&self, ty: &NameTy) -> String {
        self.ty_text(ty)
    }
}

impl NameWorld<'_> {
    /// The unit's container record for `name`.
    fn container(&self, name: StringId) -> Option<&Container> {
        self.index.get(self.file).and_then(|f| f.container(name))
    }

    /// The declared type of the instance member `member` on `head`,
    /// climbing the superclass chain (the official property lookup
    /// includes inherited members): fields through
    /// [`MemberEntry::var_ty`], getters through their return type. A name
    /// taken by a method or setter is no property; a cyclic hierarchy
    /// terminates on the visited set.
    fn member_column(&self, head: StringId, member: StringId) -> Option<NameTy> {
        let mut current = head;
        let mut seen = rustc_hash::FxHashSet::default();
        while seen.insert(current) {
            let container = self.container(current)?;
            if let Some(entry) = container.members.get(&member)
                && !entry.is_static
            {
                return match entry.kind {
                    MemberKind::Field => Some(NameTy::from_semantic(entry.var_ty)),
                    MemberKind::Getter => Some(NameTy::from_semantic(entry.return_ty)),
                    _ => None,
                };
            }
            current = container.superclass?;
        }
        None
    }

    /// The unit's container names in declaration order.
    fn container_order(&self) -> &[StringId] {
        self.index
            .get(self.file)
            .map_or(&[], FileElements::container_order)
    }

    /// The witness/diagnostic text for a static type.
    fn ty_text(&self, ty: &NameTy) -> String {
        match ty {
            NameTy::Never => "Never".to_string(),
            NameTy::Null => "null".to_string(),
            NameTy::TopNullable => "_".to_string(),
            NameTy::TopNonNullable => "Object".to_string(),
            NameTy::Named(name, nullable) => {
                let text = self.strings.resolve(*name);
                if *nullable {
                    format!("{text}?")
                } else {
                    text.to_string()
                }
            }
            NameTy::EnumValue(enum_name, constant) => {
                format!(
                    "{}.{}",
                    self.strings.resolve(*enum_name),
                    self.strings.resolve(*constant)
                )
            }
            // NullableStaticType.name: the underlying head plus `?`
            // (static_type.dart:375-378).
            NameTy::EnumValueOrNull(enum_name, constant) => {
                format!(
                    "{}.{}?",
                    self.strings.resolve(*enum_name),
                    self.strings.resolve(*constant)
                )
            }
            NameTy::BoolValue(value) => value.to_string(),
            NameTy::BoolOrNull(value) => format!("{value}?"),
            NameTy::Unknown(_) => "?".to_string(),
        }
    }

    /// Whether `sub`'s supertype chain reaches `sup`: the unit's declared
    /// superclass and `with`/`implements` edges plus the fixed core chain.
    fn named_sub(&self, sub: StringId, sup: StringId) -> bool {
        if sub == sup {
            return true;
        }
        let mut queue = std::collections::VecDeque::new();
        let mut seen = rustc_hash::FxHashSet::default();
        queue.push_back(sub);
        seen.insert(sub);
        while let Some(name) = queue.pop_front() {
            for edge in self.direct_edges(name) {
                if edge == sup {
                    return true;
                }
                if seen.insert(edge) {
                    queue.push_back(edge);
                }
            }
            for (base, parent) in CORE_SUPERTYPES {
                if self.strings.resolve(name) == base {
                    let parent_id = self.strings.intern(parent);
                    if parent_id == sup {
                        return true;
                    }
                    if seen.insert(parent_id) {
                        queue.push_back(parent_id);
                    }
                }
            }
        }
        false
    }

    /// The declared direct edges out of `name` (superclass + interfaces).
    fn direct_edges(&self, name: StringId) -> Vec<StringId> {
        let Some(container) = self.container(name) else {
            return Vec::new();
        };
        let mut edges = Vec::new();
        if let Some(superclass) = container.superclass {
            edges.push(superclass);
        }
        edges.extend(container.with_clause.iter().copied());
        edges.extend(container.implements_clause.iter().copied());
        edges.extend(container.on_clause.iter().copied());
        edges
    }

    fn is_sealed_name(&self, name: StringId) -> bool {
        if name == self.bool_id {
            return true;
        }
        self.container(name)
            .is_some_and(|c| c.sealed || c.kind == ContainerKind::Enum)
    }

    /// Whether the unit's own facts decide `name`'s sealedness and
    /// subclass set: the declaration lives in this file, or the name is a
    /// core sealed type (`bool`). A cross-file name is opaque to the
    /// file-scoped index — its sealed classes and enum constants are
    /// invisible here, so a non-exhaustive verdict over it may be false.
    ///
    /// Interim under-report seam: the expression-face covering emit
    /// suppresses while this answers false. The oracle resolves
    /// cross-library facts through the whole library graph, down to the
    /// private-enum variant rows (`diagnostic.g.dart`
    /// `non_exhaustive_switch_expression_private`); feeding sealed and
    /// subclass facts from the library graph is future work, deferred
    /// together with those variants.
    fn name_resolvable(&self, name: StringId) -> bool {
        self.container(name).is_some() || self.is_sealed_name(name)
    }

    /// The closed subtype set of a sealed class or enum, in declaration
    /// order (types/sealed.dart, types/enum.dart).
    fn name_subtypes(&self, name: StringId) -> Vec<NameTy> {
        if name == self.bool_id {
            return vec![NameTy::BoolValue(true), NameTy::BoolValue(false)];
        }
        let Some(container) = self.container(name) else {
            return Vec::new();
        };
        if container.kind == ContainerKind::Enum {
            return container
                .enum_constants
                .iter()
                .map(|constant| NameTy::EnumValue(name, *constant))
                .collect();
        }
        if !container.sealed {
            return Vec::new();
        }
        let mut subtypes = Vec::new();
        for &decl in self.container_order() {
            // A well-founded closed set never contains its root: a
            // self-edge (`sealed S extends S`) drops here, before the
            // walk's cycle guard would discard it.
            if decl == name {
                continue;
            }
            let Some(declared) = self.container(decl) else {
                continue;
            };
            let edge = declared.superclass == Some(name)
                || declared.with_clause.contains(&name)
                || declared.implements_clause.contains(&name)
                || declared.on_clause.contains(&name);
            if edge {
                subtypes.push(NameTy::Named(decl, false));
            }
        }
        subtypes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::collect;
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_resolution::{CombinatorClause, Combinators, LibraryGraphBuilder};
    use dartq_scanner::ScanConfig;
    use dartq_source::FileTable;
    use std::path::Path;

    /// Diagnoses `source` as the sole file of a library with no imports.
    /// Core-library names are deliberately absent: an M1 harness must model
    /// them through concrete declarations or shown broken edges — see the
    /// undefined-guard module docs.
    fn report(source: &str) -> FileReport {
        report_with_edge(source, |_| None)
    }

    /// [`report`], plus one extra import edge built with the live string
    /// table (combinator names need interning).
    fn report_with_edge(
        source: &str,
        make_edge: impl FnOnce(&StringTable) -> Option<ImportEdge>,
    ) -> FileReport {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let file = files.intern(Path::new("main.dart"));
        let scanned = dartq_scanner::scan(
            source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file,
        );
        assert!(
            scanned.diagnostics.is_empty(),
            "scanner diagnostics: {source:?}"
        );
        let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
            LanguageVersion::new(3, 15),
        ));
        let elements = collect(source, &scanned.tokens, features, &strings);
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib = builder.intern("package:t/main.dart");
        let mut index = ElementIndex::default();
        index.insert(file, elements.clone());
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib, name, file);
        }
        if let Some(edge) = make_edge(&strings) {
            builder.import(lib, edge);
        }
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        diagnose(
            source,
            &scanned.tokens,
            features,
            &strings,
            file,
            lib,
            &graph,
            &namespaces,
            &index,
        )
    }

    fn code_names(report: &FileReport) -> Vec<&'static str> {
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    }

    /// A bare undefined read reports `undefined_identifier` with the oracle's
    /// verbatim text; the miss also lands in the §8 names table.
    ///
    /// The static-nullability trio with primitive literal types (#350): a
    /// String literal into an int parameter / variable / return host reports
    /// each verbatim text; the same-name and int-literal-in-double-context
    /// shapes stay silent (the oracle's literal conversion).
    ///
    /// Hand-trace: `f('s')` — the argument flush attributes the single
    /// String-typed row; the formal's declared `int` answers
    /// [`Ty::assignable_from`] `None` (distinct named types), the final-
    /// primitive table decides `false`. The declaration initializer and the
    /// return check read the same table.
    #[test]
    fn primitive_literals_drive_the_assignability_trio() {
        let report = report(
            "class int {} class double {} \
             void f(int x) {} int g() { return 's'; } \
             void main() { f('s'); int y = 's'; double d = 1; int ok = 1; }",
        );
        assert_eq!(
            code_names(&report),
            [
                "return_of_invalid_type",
                "argument_type_not_assignable",
                // `y`/`d`/`ok` are never read: the official
                // `_isUsefulRead`/`_isUsedElement` split counts reads only,
                // so each declaration also reports `unused_local_variable`
                // at its name (source order interleaves with the assignability rows).
                "unused_local_variable",
                "invalid_assignment",
                "unused_local_variable",
                "unused_local_variable",
            ],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "A value of type 'String' can't be returned from the function 'g' because it has a return type of 'int'."
        );
        assert_eq!(
            report.diagnostics[1].message,
            "The argument type 'String' can't be assigned to the parameter type 'int'. "
        );
        assert_eq!(
            report.diagnostics[3].message,
            "A value of type 'String' can't be assigned to a variable of type 'int'."
        );
    }

    /// The int→double literal conversion (`analyzeIntLiteral`'s
    /// `convertToDouble`): an int *literal* in a double context is clean on
    /// every context site the M1 facts check (declaration initializer,
    /// argument, return expression, assignment right-hand side), while an
    /// int *value* (a variable read) reports on the same sites — the
    /// conversion keys on the literal, never on the value.
    ///
    /// Hand-trace: `1` pushes a [`Engine::push_primitive_row`] row (a
    /// `Value` row typed `int`, [`Engine::primitive_literal`] true); `i`'s
    /// bare row carries the declared `int` in a non-literal shape. The
    /// final-primitive table answers the literal flag for the int→double
    /// pair, so the literal sites return `true` (silent) and the variable
    /// sites `false` (the trio's codes, in source order).
    #[test]
    fn int_literal_converts_to_double_but_the_value_does_not() {
        let report = report(
            "class int {} class double {} \
             void f(double x) {} double g() { return 1; } \
             void main() { int i = 1; double a = 1; double b = i; f(1); f(i); double c; c = 1; c = i; }",
        );
        assert_eq!(
            code_names(&report),
            [
                // `a`/`b`/`c` are never read (a pure write is not a use —
                // the official read-only use rule), so each reports
                // `unused_local_variable` interleaved in source order.
                "unused_local_variable",
                "unused_local_variable",
                "invalid_assignment",
                "argument_type_not_assignable",
                "unused_local_variable",
                "invalid_assignment",
            ],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[2].message,
            "A value of type 'int' can't be assigned to a variable of type 'double'."
        );
    }

    /// A dart:core top-level annotation name (`@override` — meta 1.19.0
    /// `meta_meta.dart:202`, the M1 r2 gate's only non-#351 differential
    /// row) is defined by the implicit dart:core import: no
    /// `undefined_annotation` and no §8 miss. A genuinely missing name
    /// still reports.
    ///
    /// Hand-trace: the `MetadataReference` identifier resolves `Missing` for
    /// all three names; the [`CORE_TOP_LEVEL_ANNOTATIONS`] membership
    /// clears `override`/`pragma`, leaving `gone` to report over the `@`.
    #[test]
    fn core_top_level_annotation_names_stay_silent() {
        let report = report("@override @pragma('x') @gone void f() {}");
        assert_eq!(code_names(&report), ["undefined_annotation"]);
        assert_eq!(
            report.diagnostics[0].message,
            "Undefined name 'gone' used as an annotation."
        );
    }

    /// A fully missing annotation name reports `undefined_annotation` over
    /// the `@` (#350; the 3.13.1 caliber — the diagnostic was removed
    /// upstream in 3.14). A defined class annotation stays silent.
    ///
    /// Hand-trace: `@gone` — `begin_metadata` latches the `@`; the
    /// MetadataReference identifier resolves `Missing` (the annotation name
    /// is no declaration) -> the emit spans `@gone`.
    #[test]
    fn missing_annotation_reports_over_the_at_sign() {
        let report = report("class A {} @gone void f() {} @A void g() {}");
        assert_eq!(code_names(&report), ["undefined_annotation"]);
        let d = &report.diagnostics[0];
        assert_eq!(d.message, "Undefined name 'gone' used as an annotation.");
        assert_eq!(u32::try_from("class A {}@".len()).unwrap(), d.range.offset);
        assert_eq!(u32::try_from("@gone".len()).unwrap(), d.range.length);
    }

    /// `super.named()` in a constructor-initializer list is a super
    /// *constructor* reference: the named shape reports
    /// `undefined_constructor_in_initializer` against the superclass, and
    /// the member pass never mis-reports `undefined_super_member` (#350).
    /// A declared selector stays silent.
    ///
    /// Hand-trace: the initializer bracket raises the depth; `super.named()`
    /// merges at the dot with a `Super` receiver — the merge emits from the
    /// superclass constructor face and folds the row, so pass 2 sees only a
    /// value.
    #[test]
    fn super_initializer_selector_reports_the_class_constructor() {
        let report = report(
            "class A { A.named(); } \
             class B extends A { B.bad() : super.missing(); B.good() : super.named(); }",
        );
        assert_eq!(
            code_names(&report),
            ["undefined_constructor_in_initializer"]
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The class 'A' doesn't have a constructor named 'missing'."
        );
    }

    /// An enum constant's constructor selector names a constructor the enum
    /// does not declare: `undefined_enum_constructor` (#350). A declared
    /// selector stays silent.
    ///
    /// Hand-trace: `c.bad()` — the selector identifier fires inside the
    /// enum-element window; the enum's constructor face is `{other}` (the
    /// named selector) with no unnamed constructor; `bad` misses -> the
    /// emit at the selector.
    #[test]
    fn enum_constant_selector_reports_the_enum_constructor() {
        let report = report("enum E { c.bad(); const E.other(); }");
        assert_eq!(code_names(&report), ["undefined_enum_constructor"]);
        assert_eq!(
            report.diagnostics[0].message,
            "The enum doesn't have a constructor named 'bad'."
        );
    }

    /// The explicit extension application resolves its members against the
    /// extension's face — `E('x').h` reports the extension trio (#350),
    /// including the operator shape at the operator token — and `Object`
    /// members (the `on` type's face) stay silent.
    ///
    /// Hand-trace: `E(1)` folds at the send into a `Created` receiver; the
    /// merged member resolves through the extension kind's
    /// `check_container_member`; `toString` suppresses (the `on` type
    /// carries it).
    #[test]
    fn extension_application_reports_the_extension_trio() {
        let report = report(
            "class int {} \
             extension E on int { int get g => 1; void m() {} set s(int v) {} int operator +(int o) => 1; } \
             void f() { E(1).h; E(1).n(); E(1).t = 1; E(1) * 2; E(1).toString; }",
        );
        assert_eq!(
            code_names(&report),
            [
                "undefined_extension_getter",
                "undefined_extension_method",
                "undefined_extension_setter",
                "undefined_extension_operator",
            ],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The getter 'h' isn't defined for the extension 'E'."
        );
        assert_eq!(
            report.diagnostics[3].message,
            "The operator '*' isn't defined for the extension 'E'."
        );
    }

    /// The external-primitive member face (#350): a member access on a
    /// dart:core primitive receiver reports the interface trio when the
    /// primitive genuinely lacks the name, and stays silent on its real
    /// surface (`isEven`, `toString`).
    ///
    /// Hand-trace: `int x` types the receiver `int` — unresolvable as a
    /// container, the walk chain-breaks at the root and
    /// [`CORE_TYPE_MEMBERS`] decides. Locally declared faces keep the
    /// regular walk.
    #[test]
    fn primitive_receivers_report_the_member_trio() {
        let report = report(
            "void f() { int x = 1; x.noSuchGetter; x.noSuchMethodHere(); x.noSuchSetter = 2; x.isEven; x.toString; }",
        );
        // The receiver type `int` is undeclared in the harness: its type use
        // carries the known `undefined_class` row ahead of the member trio.
        assert_eq!(
            code_names(&report),
            [
                "undefined_class",
                "undefined_getter",
                "undefined_method",
                "undefined_setter",
            ],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[1].message,
            "The getter 'noSuchGetter' isn't defined for the type 'int'."
        );
        assert_eq!(
            report.diagnostics[2].message,
            "The method 'noSuchMethodHere' isn't defined for the type 'int'."
        );
        assert_eq!(
            report.diagnostics[3].message,
            "The setter 'noSuchSetter' isn't defined for the type 'int'."
        );
    }

    /// A creation result used as an operator receiver reports
    /// `undefined_operator` at the operator token (#350); the creation's own
    /// instance face still resolves (`Object` members, superclass chains).
    ///
    /// Hand-trace: `B() + 1` — the send folds the class callee into a
    /// `Created` receiver; the operator intent walks `B`'s member face and
    /// misses `+`. `B().m()` resolves through the superclass edge;
    /// `B().toString` suppresses (the Object member every class carries).
    #[test]
    fn creation_operator_reports_undefined_operator() {
        let report = report(
            "class A { void m() {} } class B extends A {} \
             void f() { B() + 1; B().m(); B().toString; C() + 1; } class C { void nosuch() {} }",
        );
        assert_eq!(
            code_names(&report),
            ["undefined_operator", "undefined_operator"],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The operator '+' isn't defined for the type 'B'."
        );
        assert_eq!(
            report.diagnostics[1].message,
            "The operator '+' isn't defined for the type 'C'."
        );
    }

    /// A member's own type parameters scope over its whole signature
    /// (#350): the return-type row pushed before `<T>` parsed re-scopes, so
    /// no `undefined_class` fires — and the invocation arity checks still
    /// answer.
    ///
    /// Hand-trace: `T m<T>(T x)` — the Executable frame opens before the
    /// return type; `begin_type_variable` joins the frame's `type_params`
    /// and rewrites the still-missing `T` rows above the frame's row mark.
    #[test]
    fn member_type_parameters_scope_over_the_signature() {
        let report = report(
            "class D {} class C { T m<T>(T x) => x; static T sm<T>(T x) => x; } \
             void g<T>(D x) {} \
             void f() { var d = D(); g<D, D>(d); }",
        );
        assert_eq!(
            code_names(&report),
            ["wrong_number_of_type_arguments_element"],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The function 'g' is declared with 1 type parameters, but 2 type arguments are given."
        );
    }

    /// Hand-trace: `f` opens the Executable frame; `nose` resolves through
    /// locals → type parameters → container face (none) → extension pool →
    /// `lookup_scoped` → `NotFound` (`Missing`); the argless send never opens
    /// an argument group, so the row stays a read and emits.
    #[test]
    fn bare_miss_reports_undefined_identifier() {
        let report = report("void f() { nose; }");
        assert_eq!(code_names(&report), ["undefined_identifier"]);
        let d = &report.diagnostics[0];
        assert_eq!(d.message, "Undefined name 'nose'.");
        assert_eq!(d.severity, Severity::Error);
        assert_eq!(d.range.offset, u32::try_from("void f() { ".len()).unwrap());
        assert_eq!(report.names.len(), 1);
        assert_eq!(report.names[0].use_kind, NameUse::Read);
        assert!(!report.names[0].qualified);
    }

    /// Locals shadow the library scope while in scope; `var` drops any stale
    /// declaration type so the bound name stays an unknown receiver.
    ///
    /// Hand-trace: `var x = 0;` clears `last_type`, adds `x` to the Executable
    /// frame; `x;` resolves `Plain` (no row); `nose;` reports.
    #[test]
    fn locals_shadow_and_stay_scoped() {
        let report = report("void f() { var x = 0; x; nose; }");
        assert_eq!(code_names(&report), ["undefined_identifier"]);
        assert_eq!(report.diagnostics[0].message, "Undefined name 'nose'.");
    }

    /// An invocation-callee miss is `undefined_function`; a bare read stays
    /// `undefined_identifier` (`foo;` never opens an argument group).
    ///
    /// Hand-trace: `foo(` opens a group registering the callee row; at `)`
    /// it becomes `pending_send` and `handle_send` consumes it (invoke). The
    /// inner identifier of `f(g)` would not: its `handle_send` finds no
    /// pending group and leaves the read intact.
    #[test]
    fn invoke_versus_read() {
        let report = report("void f() { foo(); bar; }");
        assert_eq!(
            code_names(&report),
            ["undefined_function", "undefined_identifier"]
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The function 'foo' isn't defined."
        );
        assert_eq!(report.names[0].use_kind, NameUse::Invoke);
    }

    /// An assignment target reports `undefined_identifier` (the oracle's
    /// code for a fully missing write target); the literal row to the right
    /// of the operator is skipped by the scan.
    #[test]
    fn assignment_target_reports_identifier() {
        let report = report("void f() { x = 1; }");
        assert_eq!(code_names(&report), ["undefined_identifier"]);
        assert_eq!(report.names[0].use_kind, NameUse::Write);
    }

    /// Type positions report `undefined_class`; `dynamic` is a builtin;
    /// in-scope type parameters resolve; file-local classes never reach the
    /// type path.
    ///
    /// Hand-trace: `T` sits in `type_params` while the Container frame is
    /// open → `Plain`; `Missing` walks `C`'s member face (absent), then the
    /// library scope → `NotFound` → `TypeUse(Missing)` → emit; `dynamic`
    /// short-circuits on the builtin table.
    #[test]
    fn type_positions_report_undefined_class() {
        let report = report("class C<T> { T? t; Missing m; dynamic d; }");
        assert_eq!(code_names(&report), ["undefined_class"]);
        assert_eq!(report.diagnostics[0].message, "Undefined class 'Missing'.");
        assert_eq!(report.names[0].use_kind, NameUse::Type);
    }

    /// `new C()` on a defined class is silent; `new Missing()` is the
    /// oracle's `undefined_class` (a constructor reference names a class,
    /// never an invocation).
    ///
    /// Hand-trace: `begin_new_expression` raises `ctor_pos`, so the argument
    /// group registers no callee and no send consumption can misfire; the
    /// `ConstructorReference` identifier resolves as a type use.
    #[test]
    fn constructor_reference_resolves_as_type() {
        let report = report("class C {} void f() { new C(); new Missing(); }");
        assert_eq!(code_names(&report), ["undefined_class"]);
        assert_eq!(report.diagnostics[0].message, "Undefined class 'Missing'.");
    }

    /// `duplicate_definition` folds from the collector's log, pointing at the
    /// SECOND declaration's name token, ahead of the row diagnostics; the
    /// model keeps the first entry.
    #[test]
    fn duplicates_fold_first() {
        let report = report("var x = 0; void f() { nose; } var x = 1;");
        assert_eq!(
            code_names(&report),
            ["duplicate_definition", "undefined_identifier"]
        );
        let dup = &report.diagnostics[0];
        assert_eq!(dup.message, "The name 'x' is already defined.");
        // The second `x`'s token: after `var ` of the third declaration.
        assert_eq!(
            dup.range.offset,
            u32::try_from("var x = 0; void f() { nose; } var ".len()).unwrap()
        );
    }

    /// Getter/setter pairs coexist (distinct model spellings); two members
    /// of the same static/instance bit are duplicates.
    ///
    /// Hand-trace: `g` and `g=` are distinct keys; both `m` commits hit the
    /// same key with the same instance bit — the second logs a duplicate.
    #[test]
    fn member_duplicates_respect_the_static_bit() {
        let report = report("class N { N get g => N(); set g(N v) {} void m() {} void m() {} }");
        assert_eq!(code_names(&report), ["duplicate_definition"]);
        assert_eq!(
            report.diagnostics[0].message,
            "The name 'm' is already defined."
        );
    }

    /// A name shown by a broken import is suppressed
    /// (`shouldIgnoreUndefined`) but still recorded in the §8 names table.
    ///
    /// Hand-trace: the edge matches (target `None`, prefix `None`) and
    /// `shows(gone)` holds — the guard fires before the emit.
    #[test]
    fn broken_import_shown_name_is_suppressed() {
        let report = report_with_edge("void f() { gone; }", |strings| {
            Some(ImportEdge {
                target: None,
                prefix: None,
                deferred: false,
                combinators: Combinators::new(
                    [CombinatorClause::Show(
                        vec![strings.intern("gone")].into_boxed_slice(),
                    )],
                    strings,
                ),
            })
        });
        assert!(report.diagnostics.is_empty());
        assert_eq!(report.names.len(), 1);
        assert_eq!(report.names[0].use_kind, NameUse::Read);
    }

    /// A bare miss under an *unfiltered* broken import still reports — the
    /// oracle ignores only show-listed names (`shouldIgnoreUndefined`,
    /// element.dart:8217-8250; the C7 shape).
    ///
    /// Hand-trace: the edge matches (target `None`, prefix `None`) but has
    /// no `show` clause, so neither guard arm fires and the emit happens.
    #[test]
    fn unfiltered_broken_import_keeps_bare_miss() {
        let report = report_with_edge("void f() { gone; }", |_| {
            Some(ImportEdge {
                target: None,
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            })
        });
        assert_eq!(code_names(&report), ["undefined_identifier"]);
    }

    /// A defined prefixed function resolves silently; a prefixed invocation
    /// miss through a resolved prefix reports `undefined_function` (the
    /// qualified shape of the family's trigger).
    ///
    /// Hand-trace: `p.real` merges with the `Prefix` receiver and
    /// `lookup_prefixed` finds `real` — no report; `gone` misses the prefix
    /// table and the guard skips resolved imports — the emit fires.
    #[test]
    fn prefixed_invoke_miss_reports_undefined_function() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let file_a = files.intern(Path::new("a.dart"));
        let file_m = files.intern(Path::new("main.dart"));
        let source_a = "void real() {}";
        let source_m = "import 'a.dart' as p; void f() { p.real(); p.gone(); }";
        let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
            LanguageVersion::new(3, 15),
        ));
        let scanned_a = dartq_scanner::scan(
            source_a,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file_a,
        );
        let scanned_m = dartq_scanner::scan(
            source_m,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file_m,
        );
        let elements_a = collect(source_a, &scanned_a.tokens, features, &strings);
        let elements_m = collect(source_m, &scanned_m.tokens, features, &strings);
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib_a = builder.intern("package:t/a.dart");
        let lib_m = builder.intern("package:t/main.dart");
        let mut index = ElementIndex::default();
        index.insert(file_a, elements_a.clone());
        index.insert(file_m, elements_m.clone());
        for (name, _) in elements_a.declarations() {
            builder.declare(lib_a, name, file_a);
        }
        for (name, decl) in elements_m.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib_m, name, file_m);
        }
        builder.import(
            lib_m,
            ImportEdge {
                target: Some(lib_a),
                prefix: Some(strings.intern("p")),
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let report = diagnose(
            source_m,
            &scanned_m.tokens,
            features,
            &strings,
            file_m,
            lib_m,
            &graph,
            &namespaces,
            &index,
        );
        assert_eq!(code_names(&report), ["undefined_function"]);
        assert_eq!(
            report.diagnostics[0].message,
            "The function 'gone' isn't defined."
        );
        assert!(report.names[0].qualified);
    }

    /// Named arguments are membership-tested against the callee's formals:
    /// a resolved callee with a missing label reports
    /// `undefined_named_parameter` at the label; an unresolved callee stays
    /// silent for the label (its own invocation code reports).
    ///
    /// Hand-trace: `g`'s group is open while the labels fire, so
    /// `top_level_decl(g)` resolves and its named formals are `["n"]`; `n`
    /// is contained (silent), `bad` misses a concrete formal table → emit +
    /// record. `nos` misses `top_level_decl` (unresolved) → `bad2` records
    /// only, and the invocation itself is `undefined_function`.
    #[test]
    fn named_argument_miss_reports_on_resolved_callee() {
        let report = report("void g({var n}) {} void f() { g(n: 1, bad: 2); nos(bad2: 3); }");
        assert_eq!(
            code_names(&report),
            ["undefined_named_parameter", "undefined_function"],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The named parameter 'bad' isn't defined."
        );
        let named: Vec<_> = report
            .names
            .iter()
            .filter(|row| row.use_kind == NameUse::NamedArgument)
            .collect();
        assert_eq!(named.len(), 2, "both labels feed the §8 table");
    }

    /// The named-argument membership test runs only against a real
    /// top-level function's formals (`type_analyzer.dart:5811-5824`: the
    /// code fires when the invoked target's named-parameter table exists
    /// and lacks the name). A class callee is a constructor invocation — a
    /// constructor accepting the label is silent — and a getter callee is a
    /// property access; both previously false-positived on their
    /// `formals: None` declarations.
    ///
    /// Hand-trace: `C(x: 1)`'s bare head resolves `Container`, and the
    /// filtered `top_level_decl` leaves it unresolved (silent); `g(x: 1)`'s
    /// getter declaration carries no formals (silent); `bad` on the real
    /// function `g` still reports.
    #[test]
    fn named_argument_gates_on_a_real_function_formals_table() {
        let class_callee = report("class C { C({var x}); } void f() { C(x: 1); }");
        assert!(
            class_callee.diagnostics.is_empty(),
            "{:?}",
            code_names(&class_callee)
        );
        let getter_callee = report("class C {} C get g => C(); void f() { g(x: 1); }");
        assert!(
            getter_callee.diagnostics.is_empty(),
            "{:?}",
            code_names(&getter_callee)
        );
        let real_miss = report("void g({var n}) {} void f() { g(bad: 1); }");
        assert_eq!(code_names(&real_miss), ["undefined_named_parameter"]);
        assert_eq!(
            real_miss.diagnostics[0].message,
            "The named parameter 'bad' isn't defined."
        );
    }

    /// `await x;` in a sync body keeps the official declaration parse
    /// (`gate_351.rs::await_statement_shapes_follow_official_dispatch`):
    /// the engine sees `await` as an unresolved type-use row and reports
    /// `undefined_identifier_await` at the type name's span (no arguments);
    /// `x` is the declared variable — no `undefined_identifier`. In `g`'s
    /// async body the parser reads `await` as a real await-expression
    /// reading the defined `f` — nothing fires, the vector stays a single
    /// row (named_type_resolver.dart:814-818, property_element_resolver
    /// .dart:2143; `COMPILE_TIME_ERROR`, severity `Error`).
    #[test]
    fn await_in_sync_body_reports_await_code() {
        let report = report("void f() { await x; } void g() async { await f; }");
        // In a sync body `await x` recovers as the type `await` declaring
        // the local `x`, which is never read — the official read-only use
        // rule reports `unused_local_variable` at its name.
        assert_eq!(
            code_names(&report),
            ["undefined_identifier_await", "unused_local_variable"],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "Undefined name 'await' in function body not marked with 'async'."
        );
        // The NamedType node's span: the `await` name token (offset 11,
        // length 5).
        assert_eq!(report.diagnostics[0].range.offset, 11);
        assert_eq!(report.diagnostics[0].range.length, 5);
    }

    /// Defined names never report: a top-level function as a value, and a
    /// static member through its class face.
    ///
    /// Hand-trace: `fn` resolves `Plain`; `C` resolves `Container` and the
    /// merged member row hits `C`'s member map (`s`) — both silent, no §8
    /// rows (defined references are the index's rows, not the recovery
    /// surface's).
    #[test]
    fn defined_names_never_report() {
        let report = report("class C { static var s = 0; } void fn() {} void f() { fn; C.s; }");
        assert!(report.diagnostics.is_empty(), "{:?}", code_names(&report));
        assert!(report.names.is_empty());
    }

    // —— batch 2 (#121): wrong_number_of_type_arguments ——————————————————

    /// A type annotation's arity is the declaration's type-parameter count.
    ///
    /// Hand-trace: `C` fires `TypeReference` (resolves — no row) and arms the
    /// pending head; `begin_type_arguments` moves the head into the group
    /// slot; `end_type_arguments(1)` pops it and checks `C`'s declared count
    /// (2, from the model) against 1 → base code at the group. `A` resolves
    /// (silent); `handle_type(C)` then binds `x`'s declared type.
    #[test]
    fn annotation_arity_reports_base_code() {
        let report = report("class A {} class C<T, U> {} void f() { C<A> x; }");
        // `x` is declared, never read — the official read-only use rule
        // reports `unused_local_variable` after the arity row.
        assert_eq!(
            code_names(&report),
            [
                "wrong_number_of_type_arguments",
                "unused_local_variable",
            ],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The type 'C' is declared with 2 type parameters, but 1 type arguments were given."
        );
        assert!(report.names.is_empty());
    }

    /// A creation type reference reports the base code too
    /// (oracle `test_const_nonGeneric`).
    ///
    /// Wave-3 event shape: `C` fires `ConstructorReference` inside the
    /// `const` context (a resolved head pushes no row), so the group checks
    /// the class face through `creation_ref` — declared count (0) against 1
    /// → base code at the group. `new C()` stays silent.
    ///
    /// Oracle (single-file `dart analyze --format=machine` run): the same
    /// fixture also reports `const_with_non_const` on the `const` keyword —
    /// outside the modeled subset (the `const_with_non_const*` family is
    /// deferred), so the modeled report is the base code alone.
    #[test]
    fn creation_arity_reports_base_code() {
        let report = report("class A {} class C {} void f() { new C(); const C<A>(); }");
        assert_eq!(code_names(&report), ["wrong_number_of_type_arguments"]);
        assert_eq!(
            report.diagnostics[0].message,
            "The type 'C' is declared with 0 type parameters, but 1 type arguments were given."
        );
    }

    /// Type arguments on a named constructor segment report `_constructor`:
    /// constructors never declare type parameters (`ast_rewrite.dart:549`).
    ///
    /// Wave-3 event shape: `C` fires `ConstructorReference` and `named`
    /// fires `ConstructorReferenceContinuation` — both land in
    /// `creation_ref` (resolved heads push no rows), so the group reports
    /// the `(class, ctor)` pair for any count while the class face
    /// resolves. `D` resolves, so no other code fires.
    ///
    /// Oracle (single-file `dart analyze --format=machine` run):
    /// `WRONG_NUMBER_OF_TYPE_ARGUMENTS_CONSTRUCTOR` on `<D>` —
    /// "The constructor 'C.named' doesn't have type parameters." — the only
    /// diagnostic; matched exactly.
    #[test]
    fn named_constructor_type_args_report_constructor_code() {
        let report = report("class D {} class C { C.named(); } void f() { new C.named<D>(); }");
        assert_eq!(
            code_names(&report),
            ["wrong_number_of_type_arguments_constructor"]
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The constructor 'C.named' doesn't have type parameters."
        );
    }

    /// An enum constant's constructor type arguments report `_enum`
    /// (`type_arguments_verifier.dart:163`).
    ///
    /// Hand-trace: `handle_enum_element` snapshots the enclosing enum's
    /// declared count (2, from the collector's depth-1 group); the constant's
    /// group closes with 1 → counts differ → `_enum` at the group. `a`
    /// itself fires `EnumValueDeclaration` (no row), so the trailing `()`
    /// opens a group with no callee — silent.
    #[test]
    fn enum_constant_type_args_report_enum_code() {
        let report = report("class D {} enum E<T, U> { a<D>(); }");
        assert_eq!(code_names(&report), ["wrong_number_of_type_arguments_enum"]);
        assert_eq!(
            report.diagnostics[0].message,
            "The enum is declared with 2 type parameters, but 1 type arguments were given."
        );
    }

    /// An invoked generic function with explicit type arguments reports
    /// `_element` with the `function` kind word (the oracle's target
    /// dispatch, `type_instantiation_target.dart`: the invocation syntax
    /// parses as an implicit creation and resolution reports through
    /// `InvocationTargetExecutableElement`).
    ///
    /// Hand-trace: `g<A, A>()` parses as an implicit creation (`ctor_pos`
    /// raised), so `g` stays a read and the group defers;
    /// `end_implicit_creation` pops the deferred entry, sees a bare
    /// top-level function head, and checks its declared count (1) against 2.
    /// The `A` arguments resolve — no other code.
    #[test]
    fn generic_invocation_arity_reports_element_function() {
        let report = report("class A {} void g<T>() {} void f() { g<A, A>(); }");
        assert_eq!(
            code_names(&report),
            ["wrong_number_of_type_arguments_element"]
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The function 'g' is declared with 1 type parameters, but 2 type arguments are given."
        );
    }

    /// An invoked generic member method reports `_element` with the `method`
    /// kind word through the send path.
    ///
    /// Wave-3 event shape: the send closes with the group deferred against
    /// `m`'s unmerged member placeholder row, and only then does the
    /// dot-access event merge `c.m` — the deferred check classifies the
    /// receiver from the row below the placeholder and walks `C`'s members,
    /// finding `m` with 2 type parameters against the given 1. `D`
    /// resolves, so no other code fires.
    ///
    /// Oracle (single-file `dart analyze --format=machine` run):
    /// `WRONG_NUMBER_OF_TYPE_ARGUMENTS_ELEMENT` on `<D>` — "The method 'm'
    /// is declared with 2 type parameters, but 1 type arguments are
    /// given." — the only diagnostic; matched exactly.
    #[test]
    fn member_invocation_arity_reports_element_method() {
        let report = report("class D {} class C { void m<T, U>() {} } void f(C c) { c.m<D>(); }");
        assert_eq!(
            code_names(&report),
            ["wrong_number_of_type_arguments_element"]
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The method 'm' is declared with 2 type parameters, but 1 type arguments are given."
        );
    }

    /// A getter invoked or applied with type arguments stays silent: the
    /// oracle names the getter kind word (or the function type display),
    /// neither of which M1 models — under-report, never a wrong code.
    ///
    /// Hand-trace: `g`'s top-level declaration carries no formals (no
    /// parameter list), so the `formals.is_some()` filter suppresses both
    /// the send path and the application path.
    #[test]
    fn getter_type_arguments_suppress() {
        let report = report("class A {} class C {} C get g => C(); void f() { g<A, A>(); g<A>; }");
        assert!(report.diagnostics.is_empty(), "{:?}", code_names(&report));
    }

    /// An applied (non-invoked) top-level function tear-off reports
    /// `_element` with the `function` kind word
    /// (`function_reference_resolver.dart` →
    /// `InvocationTargetExecutableElement`).
    ///
    /// Hand-trace: `g<A, A>;` never opens an argument group; the application
    /// event consumes the deferred entry against `g`'s declared count.
    ///
    /// RED (parser gap, #339): in expression-statement context the
    /// declaration probe consumes `g<A, A>` as a parameterized type (the
    /// `;` follower reads as a declaration end), so the statement parses as
    /// a nameless declaration head and the engine receives no identifier or
    /// application events (observed output: `[]`). Un-reds when the
    /// `<...>`-group scan in statement position falls back to the expression
    /// grammar once no name follows (official computeType posture).
    #[test]
    fn function_reference_arity_reports_element_function() {
        let report = report("class A {} void g<T>() {} void f() { g<A, A>; }");
        assert_eq!(
            code_names(&report),
            ["wrong_number_of_type_arguments_element"]
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The function 'g' is declared with 1 type parameters, but 2 type arguments are given."
        );
    }

    /// An implicit call reference (`c<int>;` on a callable class) reports
    /// `_element` naming `call` with the `method` kind word (the oracle's
    /// `_getCallMethod` resolves the `call` method element).
    ///
    /// Wave-3 event shape: the statement parses as a send closed at `<`
    /// followed by a real type-argument group and its application event —
    /// the group defers against the row at the group's base mark (`c`'s,
    /// below its own `TypeUse` argument rows), and the application walks
    /// `C.call`, finding a method with 1 type parameter against the given
    /// 2.
    ///
    /// Oracle (single-file `dart analyze --format=machine` run): the same
    /// fixture reports `WRONG_NUMBER_OF_TYPE_ARGUMENTS_ELEMENT` on
    /// `<A, A>` followed by `NON_TYPE_AS_TYPE_ARGUMENT` ×2 on the `A`
    /// arguments (this fixture defines no `A`). The type-argument miss code
    /// is outside the modeled subset — the rows report the modeled
    /// stand-in `undefined_class` — so the modeled report is the element
    /// code then the two stand-ins, in the oracle's order.
    #[test]
    fn implicit_call_reference_reports_element_call() {
        let report = report("class C { T call<T>(T x) => x; } void f(C c) { c<A, A>; }");
        assert_eq!(
            code_names(&report),
            [
                "wrong_number_of_type_arguments_element",
                "undefined_class",
                "undefined_class",
            ]
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The method 'call' is declared with 1 type parameters, but 2 type arguments are given."
        );
        // The group span `<A, A>`, then the two argument names.
        assert_eq!(report.diagnostics[0].range.offset, 48);
        assert_eq!(report.diagnostics[0].range.length, 6);
        assert_eq!(report.diagnostics[1].range.offset, 49);
        assert_eq!(report.diagnostics[1].range.length, 1);
        assert_eq!(report.diagnostics[2].range.offset, 52);
        assert_eq!(report.diagnostics[2].range.length, 1);
    }

    /// An explicit extension override with mismatched type arguments reports
    /// `_extension` (`extension_member_resolver.dart:380`).
    ///
    /// Wave-3 event shape: `Ex<A>(a);` parses as a plain invocation — no
    /// implicit-creation events — so the group defers against `Ex`'s
    /// container row and the send's deferred check dispatches the extension
    /// face: declared count (0) against 1, reported at the group. The
    /// trailing `(a)` stays silent.
    ///
    /// Oracle (single-file `dart analyze --format=machine` run): the same
    /// fixture also reports `extension_override_without_access` on
    /// `Ex<A>(a)` — outside the modeled subset, so the modeled report is
    /// `_extension` alone.
    #[test]
    fn extension_override_arity_reports_extension_code() {
        let report = report("class A {} extension Ex on A {} void f(A a) { Ex<A>(a); }");
        assert_eq!(
            code_names(&report),
            ["wrong_number_of_type_arguments_extension"]
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The extension 'Ex' is declared with 0 type parameters, but 1 type arguments were given."
        );
    }

    // —— batch 2 (#121): static-nullability four ——————————————————————————

    /// `null` into a non-nullable target reports `invalid_assignment`
    /// (`assignment_expression_resolver.dart:795`).
    ///
    /// Hand-trace: `int x` arms the group type and binds `x` =
    /// `int` (non-nullable); `x = null` finds the target row before `=`,
    /// exactly one right-hand row (`null`, typed `Null`), and
    /// `Ty::assignable_from(Null → int)` = `Some(false)` (Left Null,
    /// subtype.dart:349) → the code. The `int` class models the core type in
    /// the no-core harness.
    #[test]
    fn null_assignment_reports_invalid_assignment() {
        let report = report("class int {} void f() { int x; x = null; }");
        // `x` is written but never read: the official read-only use rule
        // reports `unused_local_variable` at its name, before the
        // assignment row.
        assert_eq!(
            code_names(&report),
            ["unused_local_variable", "invalid_assignment"]
        );
        assert_eq!(
            report.diagnostics[1].message,
            "A value of type 'Null' can't be assigned to a variable of type 'int'."
        );
        assert_eq!(report.diagnostics[1].severity, Severity::Error);
    }

    /// A declaration initializer runs the same decision
    /// (`variable_declaration_resolver.dart:140`); a nullable target accepts
    /// `null` silently.
    ///
    /// Hand-trace: `int x = null` checks the single-row initializer against
    /// the group type → `invalid_assignment`; `int? y = null` is Right Top
    /// (subtype.dart:263) → silent.
    ///
    /// RED (parser gap, #339): a nullable declared type followed by an
    /// initializer (`int? y = null;`) loses the declaration: the `?`-type
    /// plus `=`-initializer statement shape parses as the expression
    /// `int ? y …`, `y` never binds, and its name surfaces as
    /// `undefined_identifier` (probe: `int? y = 1;` alone reports it too;
    /// `int? y;` without an initializer parses fine). Un-reds when the
    /// declaration probe accepts `Type? name =` as a variable declaration.
    #[test]
    fn declaration_initializer_null_reports_invalid_assignment() {
        let report = report("class int {} void f() { int x = null; int? y = null; }");
        // `x` and `y` are never read (the official read-only use rule);
        // only `x`'s null initializer reports the assignment.
        assert_eq!(
            code_names(&report),
            [
                "unused_local_variable",
                "invalid_assignment",
                "unused_local_variable",
            ]
        );
        assert_eq!(
            report.diagnostics[1].message,
            "A value of type 'Null' can't be assigned to a variable of type 'int'."
        );
    }

    /// A nullable right-hand side into a non-nullable target is decidable
    /// from nullability alone (Left Nullable, subtype.dart:389-392).
    ///
    /// Hand-trace: `y`'s row snapshots its declared type `int?`; the check
    /// needs no name lattice → `Some(false)`; the display texts are `int?`
    /// and `int`.
    #[test]
    fn nullable_assignment_reports_invalid_assignment() {
        let report = report("class int {} void f() { int? y; int x; x = y; }");
        // `x` is written only (never read — the official read-only use
        // rule); `y` is read by the assignment and stays silent.
        assert_eq!(
            code_names(&report),
            ["unused_local_variable", "invalid_assignment"]
        );
        assert_eq!(
            report.diagnostics[1].message,
            "A value of type 'int?' can't be assigned to a variable of type 'int'."
        );
    }

    /// `null` (and nullable values) into a known non-nullable parameter
    /// reports `argument_type_not_assignable` (helpers.dart:96-142). The
    /// message keeps the template's trailing record appendix (empty at M1).
    ///
    /// Hand-trace: `g(null)` — the argument list collects one terminal row
    /// (typed `Null`); the callee's declared positional formal is `int`
    /// (non-nullable) → `Some(false)`. `g(y)` with `int? y` reads `y`'s
    /// snapshot (`int?`).
    #[test]
    fn null_argument_reports_argument_type_not_assignable() {
        let report = report("class int {} void g(int p) {} void f() { int? y; g(null); g(y); }");
        assert_eq!(
            code_names(&report),
            [
                "argument_type_not_assignable",
                "argument_type_not_assignable"
            ]
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The argument type 'Null' can't be assigned to the parameter type 'int'. "
        );
        assert_eq!(
            report.diagnostics[1].message,
            "The argument type 'int?' can't be assigned to the parameter type 'int'. "
        );
    }

    /// `return null` in a non-nullable function reports
    /// `return_of_invalid_type` with the per-host message
    /// (`rtv.dart:209-249` sync matrix).
    ///
    /// Hand-trace: `f`'s return type latches at the name identifier (the
    /// return type's `handle_type` precedes it); `return null`'s single row
    /// fails Left Null → the function-host message. `m`'s method shape picks
    /// the method-host word.
    #[test]
    fn null_return_reports_return_of_invalid_type() {
        let report =
            report("class int {} class C { int m() { return null; } } int f() { return null; }");
        assert_eq!(
            code_names(&report),
            ["return_of_invalid_type", "return_of_invalid_type"]
        );
        assert_eq!(
            report.diagnostics[0].message,
            "A value of type 'Null' can't be returned from the method 'm' because it has a return type of 'int'."
        );
        assert_eq!(
            report.diagnostics[1].message,
            "A value of type 'Null' can't be returned from the function 'f' because it has a return type of 'int'."
        );
    }

    /// The void-host arm of the return matrix: `return <non-void>` in a
    /// `void` function reports; `return null` in a void function is
    /// oracle-legal.
    ///
    /// Hand-trace: host `void` accepts `void`/`dynamic`/`Null` — `return x`
    /// with `x: int` is none of them → report; `return null` matches the
    /// `Null` accept set → silent.
    #[test]
    fn void_return_matrix() {
        let report = report("class int {} void f(int x) { return x; } void g() { return null; }");
        assert_eq!(code_names(&report), ["return_of_invalid_type"]);
        assert_eq!(
            report.diagnostics[0].message,
            "A value of type 'int' can't be returned from the function 'f' because it has a return type of 'void'."
        );
    }

    /// `?.` on a strictly non-nullable receiver reports the
    /// `invalid_null_aware_operator` `STATIC_WARNING`; a nullable receiver
    /// stays silent (`type_system.dart:1921-` `isStrictlyNonNullable`).
    ///
    /// Hand-trace: `c`'s declared type is `C` (no `?`) → strictly
    /// non-nullable → the warning at the merge; `c2`'s type is `C?` →
    /// suppress. `m` is declared, so the member walk resolves both accesses.
    #[test]
    fn null_aware_on_non_nullable_reports_static_warning() {
        let report = report("class C { void m() {} } void f(C c, C? c2) { c?.m(); c2?.m(); }");
        assert_eq!(code_names(&report), ["invalid_null_aware_operator"]);
        let d = &report.diagnostics[0];
        assert_eq!(
            d.message,
            "The receiver can't be null, so the null-aware operator '?.' is unnecessary."
        );
        assert_eq!(d.severity, Severity::Warning);
        assert_eq!(
            d.code.dtype,
            dartq_diagnostics::DiagnosticType::StaticWarning
        );
    }

    /// Declaration-arity violations fold from the collector's log ahead of
    /// the row diagnostics, in capture order: the operator count table
    /// (`error_verifier.dart:5688-5740`) and the setter shape rule
    /// (`ast_builder.dart:6283-6291`).
    ///
    /// Hand-trace: `operator +(a, b)` → expected 1, found 2; `set s(a, b)` →
    /// not one required-positional → the syntactic-tier setter text; the
    /// correct `operator [](i)` and `set ok(v)` stay silent. The `int`
    /// annotations model `dart:core` (the harness has no core library —
    /// `report()`'s contract — so core names join as concrete declarations,
    /// as in `null_assignment`).
    #[test]
    fn declaration_arity_folds_operator_and_setter() {
        let report = report(
            "class int {} class C { int operator +(int a, int b) => 1; set s(int x, int y) {} int operator [](int i) => 1; set ok(int v) {} }",
        );
        assert_eq!(
            code_names(&report),
            [
                "wrong_number_of_parameters_for_operator",
                "wrong_number_of_parameters_for_setter",
            ]
        );
        assert_eq!(
            report.diagnostics[0].message,
            "Operator '+' should declare exactly 1 parameters, but 2 found."
        );
        assert_eq!(
            report.diagnostics[1].message,
            "Setters must declare exactly one required positional parameter."
        );
        assert_eq!(report.diagnostics[1].severity, Severity::Error);
    }

    /// The minus operator keeps its own template under the same analyzerCode
    /// (0 or 1 parameters — unary and binary minus coexist).
    ///
    /// Hand-trace: `operator -(a, b)` declares 2 parameters; the table's
    /// fall-through arm (`error_verifier.dart:5731-5740`) fires the minus
    /// variant with the found count only. `int` models `dart:core` (the
    /// `report()` harness contract).
    #[test]
    fn minus_operator_arity_uses_the_minus_template() {
        let report = report("class int {} class C { C operator -(int a, int b) => C(); }");
        assert_eq!(
            code_names(&report),
            ["wrong_number_of_parameters_for_operator"]
        );
        assert_eq!(
            report.diagnostics[0].message,
            "Operator '-' should declare 0 or 1 parameter, but 2 found."
        );
    }

    /// The arity checks suppress on unresolved heads: a second code at the
    /// type-argument group would contaminate the diff (rule 0c — invalid
    /// types never cascade).
    ///
    /// Hand-trace: `Missing<A>()` parses as a bare invocation, not a
    /// creation — no `new`/`const` prefix, so the official parser runs it
    /// through the send path and the resolver reports the identifier miss
    /// (`undefined_function`, the static-context invocation miss). The
    /// group's arity check finds no resolvable declaration and defers past
    /// it → exactly one code.
    #[test]
    fn unresolved_creation_head_suppresses_arity() {
        let report = report("class A {} void f() { Missing<A>(); }");
        assert_eq!(code_names(&report), ["undefined_function"]);
    }

    // —— batch 2 (#197): the member family ————————————————————————————

    /// Instance member misses report `undefined_method` /
    /// `undefined_getter` / `undefined_setter` by access intent
    /// (`InstanceLookupDomain`); a write survives on any same-named
    /// declaration (getter-only: the oracle's assignment-to-final family)
    /// or on a setter spelling; a setter-only name still reads as a
    /// getter miss.
    ///
    /// Hand-trace: the harness has no core library, so `int` — undeclared
    /// here — is a missing type reference at each member declaration
    /// (`undefined_class` ×2, the oracle's annotation miss); `c.h` walks
    /// `C` (no `h`, no superclass) → clean miss → `undefined_getter`;
    /// `c.g` finds the getter (silent); `c.s = 1` misses `s` and `s=` →
    /// `undefined_setter`; `c.g = 1` finds the getter `g` (write survives,
    /// unfrozen assignment-to-final); `c.p` on the setter-only `p=` misses
    /// the read → `undefined_getter`.
    #[test]
    fn instance_member_family_reports_by_intent() {
        let report = report(
            "class C { int get g => 1; set p(int v) {} } \
             void f(C c) { c.h; c.s = 1; c.g = 2; c.p; }",
        );
        assert_eq!(
            code_names(&report),
            [
                "undefined_class",
                "undefined_class",
                "undefined_getter",
                "undefined_setter",
                "undefined_getter",
            ],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[2].message,
            "The getter 'h' isn't defined for the type 'C'."
        );
        assert_eq!(
            report.diagnostics[3].message,
            "The setter 's' isn't defined for the type 'C'."
        );
        assert_eq!(
            report.diagnostics[2].range.offset,
            u32::try_from("class C { int get g => 1; set p(int v) {} } void f(C c) { c.".len())
                .unwrap()
        );
    }

    /// A declared method invoked through an instance receiver reports
    /// `undefined_method` (`ReadSyntax.invocation`).
    ///
    /// Hand-trace: `c.m()`'s send marks the member row invoke; the walk on
    /// `C` clean-misses `m` → the method code, not the getter code.
    #[test]
    fn instance_invocation_miss_reports_undefined_method() {
        let report = report("class C {} void f(C c) { c.m(); }");
        assert_eq!(code_names(&report), ["undefined_method"]);
        assert_eq!(
            report.diagnostics[0].message,
            "The method 'm' isn't defined for the type 'C'."
        );
    }

    /// The static face (`StaticLookupDomain`): interface shapes report the
    /// `undefined_method`/`_getter`/`_setter` trio (the type-literal
    /// invocation row shares the registry name and message), enum reads
    /// report `undefined_enum_constant`, and enum writes report
    /// `undefined_setter`.
    ///
    /// Hand-trace: every `X.member` merges to a `Container` receiver whose
    /// face is the declaration's own member map; `E.x` reads fall to the
    /// enum-constant text, `E.z = 1` writes to the uniform setter arm.
    #[test]
    fn static_face_reports_interface_trio_and_enum_constant() {
        let report = report(
            "class C {} enum E { a } \
             void f() { C.missing(); C.g; C.s = 1; E.a; E.x; E.y(); E.z = 1; }",
        );
        assert_eq!(
            code_names(&report),
            [
                "undefined_method",
                "undefined_getter",
                "undefined_setter",
                "undefined_enum_constant",
                "undefined_method",
                "undefined_setter",
            ],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The method 'missing' isn't defined for the type 'C'."
        );
        assert_eq!(
            report.diagnostics[3].message,
            "There's no constant named 'x' in 'E'."
        );
    }

    /// The extension face reports its dedicated trio
    /// (`StaticLookupDomain._extensionReadFailure` / the setter arm).
    ///
    /// Hand-trace: `X` resolves as an `Extension` container; the member map
    /// misses each name → the extension texts with the extension name.
    #[test]
    fn extension_static_face_reports_extension_trio() {
        let report = report(
            "class int {} extension X on int {} \
             void f() { X.g; X.m(); X.s = 1; }",
        );
        assert_eq!(
            code_names(&report),
            [
                "undefined_extension_getter",
                "undefined_extension_method",
                "undefined_extension_setter",
            ],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The getter 'g' isn't defined for the extension 'X'."
        );
    }

    /// `super.member` reports `undefined_super_member` with the
    /// per-shape message against the enclosing container (`SuperLookupDomain`);
    /// members found through the superclass edge stay silent.
    ///
    /// Hand-trace: the harness has no core library, so B's `int`-typed
    /// members carry two `undefined_class` rows; `super.g/m/s` resolve in
    /// `B` (the superclass edge reaches it) — silent; the `q*` misses walk
    /// B cleanly → getter/method/setter texts.
    #[test]
    fn super_member_family_reports_by_shape() {
        let report = report(
            "class B { int get g => 1; void m() {} set s(int v) {} } \
             class C extends B { void f() { super.g; super.m(); super.s = 1; super.q; super.q2(); super.q3 = 2; } }",
        );
        assert_eq!(
            code_names(&report),
            [
                "undefined_class",
                "undefined_class",
                "undefined_super_member",
                "undefined_super_member",
                "undefined_super_member",
            ],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[2].message,
            "The getter 'q' isn't defined in a superclass of 'C'."
        );
        assert_eq!(
            report.diagnostics[3].message,
            "The method 'q2' isn't defined in a superclass of 'C'."
        );
        assert_eq!(
            report.diagnostics[4].message,
            "The setter 'q3' isn't defined in a superclass of 'C'."
        );
    }

    /// Prefixed property reads and writes report `undefined_prefixed_name`
    /// (`PrefixedLookupDomain`); the resolved name stays silent.
    ///
    /// Hand-trace: `p.real` hits the prefix table (silent); `p.gone` and the
    /// `p.gone2 = 1` write miss it and pass the guard → the two-part text.
    #[test]
    fn prefixed_property_miss_reports_undefined_prefixed_name() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let file_a = files.intern(Path::new("a.dart"));
        let file_m = files.intern(Path::new("main.dart"));
        let source_a = "void real() {}";
        let source_m = "import 'a.dart' as p; void f() { p.real; p.gone; p.gone2 = 1; }";
        let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
            LanguageVersion::new(3, 15),
        ));
        let scanned_a = dartq_scanner::scan(
            source_a,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file_a,
        );
        let scanned_m = dartq_scanner::scan(
            source_m,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file_m,
        );
        let elements_a = collect(source_a, &scanned_a.tokens, features, &strings);
        let elements_m = collect(source_m, &scanned_m.tokens, features, &strings);
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib_a = builder.intern("package:t/a.dart");
        let lib_m = builder.intern("package:t/main.dart");
        let mut index = ElementIndex::default();
        index.insert(file_a, elements_a.clone());
        index.insert(file_m, elements_m.clone());
        for (name, _) in elements_a.declarations() {
            builder.declare(lib_a, name, file_a);
        }
        for (name, decl) in elements_m.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib_m, name, file_m);
        }
        builder.import(
            lib_m,
            ImportEdge {
                target: Some(lib_a),
                prefix: Some(strings.intern("p")),
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let report = diagnose(
            source_m,
            &scanned_m.tokens,
            features,
            &strings,
            file_m,
            lib_m,
            &graph,
            &namespaces,
            &index,
        );
        assert_eq!(
            code_names(&report),
            ["undefined_prefixed_name", "undefined_prefixed_name"],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The name 'gone' is being referenced through the prefix 'p', but it isn't defined \
             in any of the libraries imported using that prefix."
        );
    }

    /// A bare invocation miss inside a class or enum instance member names
    /// `this`'s type (`UnqualifiedLookupDomain` invocation arm); static and
    /// top-level bodies keep `undefined_function`.
    ///
    /// Hand-trace: `missing()` in `C.f` — the container walk clean-misses
    /// (no `this` member, no extension pool, no library scope) and the
    /// instance context maps the row to `undefined_method(C)`; `C.g`'s
    /// static body and the enum's instance body pick their own domains.
    #[test]
    fn bare_invoke_in_instance_reports_undefined_method() {
        let report = report(
            "class C { void f() { missing(); } static void g() { missing2(); } } \
             enum E { a; void f() { missing3(); } }",
        );
        assert_eq!(
            code_names(&report),
            ["undefined_method", "undefined_function", "undefined_method"],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The method 'missing' isn't defined for the type 'C'."
        );
        assert_eq!(
            report.diagnostics[1].message,
            "The function 'missing2' isn't defined."
        );
        assert_eq!(
            report.diagnostics[2].message,
            "The method 'missing3' isn't defined for the type 'E'."
        );
    }

    /// A stale type-parameter reference follows the live invocation context:
    /// invoked inside a class instance member it reports `undefined_method`
    /// naming the enclosing type (the `UnqualifiedLookupDomain` arm), not
    /// the context-free `undefined_function`.
    ///
    /// Hand-trace: `T` is out of scope in `D.m` (the engine's type-parameter
    /// stack truncated when `C`'s frame closed), but the scoped lookup still
    /// resolves the per-unit tag to `C`'s `TypeParam` declaration;
    /// `bare_info_of` snapshots the live frames (`this` of `D`) instead of a
    /// context-free missing row → `undefined_method(D)`.
    #[test]
    fn stale_type_param_invocation_follows_live_frame() {
        let report = report("class C<T> {} class D { void m() { T(); } }");
        assert_eq!(
            code_names(&report),
            ["undefined_method"],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The method 'T' isn't defined for the type 'D'."
        );
    }

    /// Mixin `on` constraints and extension `on` types are invisible to the
    /// model: bare names and `this.member` accesses inside their instance
    /// bodies record only — the miss may be a constraint member.
    ///
    /// Hand-trace: `real`/`real2` resolve against `C` in the oracle (the
    /// `on` face), so emitting would false-positive; the walk on `M`/`X`
    /// clean-misses but the container-kind gate suppresses.
    #[test]
    fn mixin_and_extension_bodies_suppress_member_misses() {
        let report = report(
            "class C { void real() {} } \
             mixin M on C { void f() { real(); this.real2(); } } \
             extension X on C { void g() { real3(); this.real4(); } }",
        );
        assert!(report.diagnostics.is_empty(), "{:?}", code_names(&report));
    }

    // —— batch 2 (#197): undefined_operator (#120 residual 2) ————————————
    // (index tests below double as residual 4's subscript-intent evidence)

    /// A binary operator on a declared-typed receiver reports
    /// `undefined_operator` at the operator token
    /// (`binary_expression_resolver.dart`); a declared operator and
    /// `this`-receiver hits stay silent; `super.operator` reports the super
    /// operator shape.
    ///
    /// Hand-trace: `c + d` — `c`'s row carries `Operator('+', token)`;
    /// `C` declares `+` (silent). `d + d2` — `D` lacks `+` → the operator
    /// token anchors `undefined_operator('+', D)`. `this - o` walks `E`'s
    /// face; `super * this` walks `B` and reports the super text.
    #[test]
    fn binary_operator_misses_report_undefined_operator() {
        let report = report(
            "class C { C operator +(C o) => this; } class D {} \
             void f(C c, D d, D d2) { c + d; d + d2; } \
             class E { void g(E o) { this - o; } } \
             class B {} class S extends B { void h() { super * this; } }",
        );
        assert_eq!(
            code_names(&report),
            [
                "undefined_operator",
                "undefined_operator",
                "undefined_super_member",
            ],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The operator '+' isn't defined for the type 'D'."
        );
        assert_eq!(
            report.diagnostics[1].message,
            "The operator '-' isn't defined for the type 'E'."
        );
        assert_eq!(
            report.diagnostics[2].message,
            "The operator '*' isn't defined in a superclass of 'S'."
        );
        assert_eq!(
            report.diagnostics[0].range.offset,
            u32::try_from(
                "class C { C operator +(C o) => this; } class D {} void f(C c, D d, D d2) { c + d; d "
                    .len()
            )
            .unwrap()
        );
    }

    /// A binary operator on a member result applies to the member's
    /// declared type (`c.j + r` checks `+` on the field's type); a missing
    /// member reports the getter read instead — the operator's invalid
    /// type never cascades.
    ///
    /// Hand-trace: `j`'s row carries `Operator('+')`; the walk finds the
    /// field `j: R` → the operator check runs on `R` → `undefined_operator`.
    /// `c.k` has no declaration in `C` → clean miss → `undefined_getter`
    /// only (a declared field would read cleanly and lend its own type to
    /// the operator, so `k` must stay undeclared for the no-cascade shape).
    #[test]
    fn member_result_operator_checks_the_result_type() {
        let report = report(
            "class R {} class C { R j; } class R2 {} \
             void f(C c, R r, R2 r2) { c.j + r; c.k + r2; }",
        );
        assert_eq!(
            code_names(&report),
            ["undefined_operator", "undefined_getter"],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The operator '+' isn't defined for the type 'R'."
        );
    }

    // —— batch 2 (#197): index intents (#120 residual 4) —————————————————

    /// Index reads and writes check `[]` / `[]=` independently, anchored on
    /// the bracket pair (`property_element_resolver.dart`); compound writes
    /// check both; declared operators stay silent — and the index argument
    /// never takes the assignment's write intent.
    ///
    /// Hand-trace: `d[0]` folds `d`'s row to `Operator('[]', brackets)` →
    /// the read spelling misses on `D`. `d[0] = 1` records the write form →
    /// only `[]=` is checked. `d[0] += 1` checks both. The declared pair on
    /// `C` resolves both forms of `c[0] = c[1]` silently, and `c[0] = null`
    /// does NOT read `c`'s own type as the destination.
    #[test]
    fn index_read_write_and_compound_check_spellings() {
        let index_report = report(
            "class int {} class D {} \
             void f(D d, int i) { d[i]; d[i] = 1; d[i] += 1; }",
        );
        assert_eq!(
            code_names(&index_report),
            [
                "undefined_operator",
                "undefined_operator",
                "undefined_operator",
                "undefined_operator",
            ],
            "{:?}",
            code_names(&index_report)
        );
        assert_eq!(
            index_report.diagnostics[0].message,
            "The operator '[]' isn't defined for the type 'D'."
        );
        assert_eq!(
            index_report.diagnostics[1].message,
            "The operator '[]=' isn't defined for the type 'D'."
        );
        assert_eq!(
            index_report.diagnostics[2].message,
            "The operator '[]' isn't defined for the type 'D'."
        );
        assert_eq!(
            index_report.diagnostics[3].message,
            "The operator '[]=' isn't defined for the type 'D'."
        );
        let defined_report = report(
            "class int {} class C { C operator [](int i) => this; C operator []=(int i, C v) {} } \
             void f(C c, int i) { c[i] = c[i]; c[i] = null; }",
        );
        assert!(
            defined_report.diagnostics.is_empty(),
            "{:?}",
            code_names(&defined_report)
        );
    }

    /// The `c.j[i] = v` subscript intent (#120 residual 4): the write goes
    /// through `[]=` on the member's result type; the index argument keeps
    /// its read; no setter code touches `j`; a missing `j` reports the
    /// getter read and suppresses the operator.
    ///
    /// Hand-trace: the fold at `[` marks `j`'s member row
    /// `Operator('[]', brackets)` and consumes `i`'s row; the assignment
    /// records the `[]=` write form; the walk finds getter `j: V` → the
    /// operator check runs on `V` → one `undefined_operator('[]=', V)`. In
    /// the second statement `k` clean-misses → `undefined_getter(k)`, no
    /// cascade.
    #[test]
    fn subscript_write_targets_the_member_result_type() {
        let report = report(
            "class int {} class V {} class C { V get j => V(); } \
             void f(C c, int i, V v) { c.j[i] = v; c.k[i] = v; }",
        );
        assert_eq!(
            code_names(&report),
            ["undefined_operator", "undefined_getter"],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The operator '[]=' isn't defined for the type 'V'."
        );
        assert_eq!(
            report.diagnostics[0].range.offset,
            u32::try_from(
                "class int {} class V {} class C { V get j => V(); } void f(C c, int i, V v) { c.j"
                    .len()
            )
            .unwrap()
        );
    }

    /// A compound member write checks both faces (`c.g += 1` needs the
    /// getter read and the setter write) — the getter error first, matching
    /// the oracle's resolution order.
    ///
    /// Hand-trace: `g`'s row upgrades to `ReadWrite`; the getter walk
    /// clean-misses → `undefined_getter`; the write walk misses `g` and
    /// `g=` → `undefined_setter`.
    #[test]
    fn compound_member_write_reports_both_faces() {
        let report = report("class C {} void f(C c) { c.g += 1; }");
        assert_eq!(
            code_names(&report),
            ["undefined_getter", "undefined_setter"],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The getter 'g' isn't defined for the type 'C'."
        );
        assert_eq!(
            report.diagnostics[1].message,
            "The setter 'g' isn't defined for the type 'C'."
        );
    }

    // —— records/patterns/exhaustiveness (M2 face) ————————————————————————

    /// A statement switch over an enum with every constant covered is
    /// exhaustive: no diagnostics.
    ///
    /// Hand-trace: the scrutinee row carries `Color` (formal type); each
    /// `case Color.red:` classifies through the constant-tail walk (the
    /// `.red` member row over the bare `Color` row of an enum container)
    /// into `EnumValue`; the engine's covering search finds no gap.
    #[test]
    fn enum_switch_all_constants_is_exhaustive() {
        let report = report(
            "enum Color { red, green, blue } void f(Color c) { switch (c) { case Color.red: break; case Color.green: break; case Color.blue: break; } }",
        );
        assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
    }

    /// A missing enum constant reports `non_exhaustive_switch_statement`
    /// at the `switch` keyword, with the oracle's verbatim text and the
    /// first missing constant as the suggested correction pattern.
    ///
    /// Hand-trace: `blue` never covers → `NonExhaustive([Color.blue])`;
    /// the statement is mustBeExhaustive (enums are always exhaustive,
    /// type_system.dart:846-887) and no `default` is present.
    #[test]
    fn enum_switch_missing_constant_reports_correction() {
        let report = report(
            "enum Color { red, green, blue } void f(Color c) { switch (c) { case Color.red: break; case Color.green: break; } }",
        );
        assert_eq!(
            code_names(&report),
            ["non_exhaustive_switch_statement"],
            "{:?}",
            code_names(&report)
        );
        let d = &report.diagnostics[0];
        assert_eq!(
            d.message,
            "The type 'Color' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'Color.blue'."
        );
        // The oracle row's correctionMessage carries the suggested pattern
        // as `{2}` (diagnostic.g.dart:13079).
        assert_eq!(
            d.correction.as_deref(),
            Some("Try adding a default case or cases that match 'Color.blue'.")
        );
        assert_eq!(d.severity, Severity::Error);
        let switch_at =
            u32::try_from("enum Color { red, green, blue } void f(Color c) { ".len()).unwrap();
        assert_eq!(d.range.offset, switch_at);
    }

    /// A `default` case suppresses the non-exhaustive report even when
    /// constants are missing (the oracle's `reportNonExhaustive` gate,
    /// constant_verifier.dart:1122-1140).
    #[test]
    fn default_case_suppresses_non_exhaustive() {
        let report = report(
            "enum Color { red, green, blue } void f(Color c) { switch (c) { case Color.red: break; default: break; } }",
        );
        assert_eq!(report.diagnostics.len(), 0);
    }

    /// A duplicate constant after total coverage is
    /// `unreachable_switch_case` (`STATIC_WARNING`) at the repeated case's
    /// keyword — reported per case before the covering check, matching the
    /// oracle's decision order (constant_verifier.dart:1080-1088).
    #[test]
    fn duplicate_constant_case_is_unreachable() {
        let report = report(
            "enum Color { red, green, blue } void f(Color c) { switch (c) { case Color.red: break; case Color.green: break; case Color.blue: break; case Color.red: break; } }",
        );
        assert_eq!(
            code_names(&report),
            ["unreachable_switch_case"],
            "{:?}",
            code_names(&report)
        );
        let d = &report.diagnostics[0];
        assert_eq!(d.message, "This case is covered by the previous cases.");
        assert_eq!(d.severity, Severity::Warning);
        let repeat_at = u32::try_from(
            "enum Color { red, green, blue } void f(Color c) { switch (c) { case Color.red: break; case Color.green: break; case Color.blue: break; ".len(),
        )
        .unwrap();
        assert_eq!(d.range.offset, repeat_at);
    }

    /// A `when`-guarded case never covers: the guarded case is excluded
    /// from the covering search (`constant_verifier.dart` guarded rows) and
    /// the missing constant still reports.
    #[test]
    fn guarded_case_does_not_cover() {
        let report = report(
            "enum Color { red, green, blue } void f(Color c) { switch (c) { case Color.red: break; case Color.green: break; case Color.blue when true: break; } }",
        );
        assert_eq!(code_names(&report), ["non_exhaustive_switch_statement"]);
        assert_eq!(
            report.diagnostics[0].message,
            "The type 'Color' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'Color.blue'."
        );
    }

    /// Switch expressions are always mustBeExhaustive: a missing constant
    /// reports `non_exhaustive_switch_expression` even though a statement
    /// over the same cases would stay silent only without the seal — and
    /// the same shape over a *statement* is the T2 report, not this code.
    #[test]
    fn switch_expression_requires_all_constants() {
        let report = report(
            "enum Color { red, green, blue } void f(Color c) { var x = switch (c) { Color.red => 1, Color.green => 2 }; }",
        );
        // `x` binds the switch expression's value and is never read (the
        // official read-only use rule), so it reports before the
        // exhaustiveness row.
        assert_eq!(
            code_names(&report),
            ["unused_local_variable", "non_exhaustive_switch_expression"],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[1].message,
            "The type 'Color' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'Color.blue'."
        );
        // The expression-face correctionMessage (diagnostic.g.dart:13018).
        assert_eq!(
            report.diagnostics[1].correction.as_deref(),
            Some("Try adding a wildcard pattern or cases that match 'Color.blue'.")
        );
    }

    /// A switch expression over a sealed class missing one subclass reports
    /// the subclass name as the witness; covering every declared subclass
    /// is exhaustive.
    ///
    /// Hand-trace: `S` is sealed; `subtypes` walks the unit's containers in
    /// declaration order over superclass edges → `[A, B]`; `case A a:`
    /// leaves `B` uncovered.
    #[test]
    fn sealed_class_switch_reports_missing_subclass() {
        let missing = report(
            "sealed class S {} class A extends S {} class B extends S {} void f(S s) { var x = switch (s) { A a => 1 }; }",
        );
        // `x` is unread before the switch; the case variable `a` is unread
        // after it — the covering row anchors at the switch expression
        // (constant_verifier.dart:1080-1088), so source order interleaves
        // the two unused rows around it.
        assert_eq!(
            code_names(&missing),
            [
                "unused_local_variable",
                "non_exhaustive_switch_expression",
                "unused_local_variable",
            ],
            "{:?}",
            code_names(&missing)
        );
        assert_eq!(
            missing.diagnostics[1].message,
            "The type 'S' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'B'."
        );

        let covered = report(
            "sealed class S {} class A extends S {} class B extends S {} void f(S s) { var x = switch (s) { A a => 1, B b => 2 }; }",
        );
        // `x`, `a`, and `b` are all unread.
        assert_eq!(
            code_names(&covered),
            [
                "unused_local_variable",
                "unused_local_variable",
                "unused_local_variable",
            ]
        );
    }

    /// A switch expression over a nullable enum needs the `null` case: the
    /// nullable projection splits into `[Color, null]` and the constants
    /// alone leave the null half uncovered.
    #[test]
    fn nullable_enum_switch_needs_null_case() {
        let missing = report(
            "enum Color { red, green, blue } void f(Color? c) { var x = switch (c) { Color.red => 1, Color.green => 2, Color.blue => 3 }; }",
        );
        // `x` is unread (the official read-only use rule).
        assert_eq!(
            code_names(&missing),
            ["unused_local_variable", "non_exhaustive_switch_expression"]
        );
        assert_eq!(
            missing.diagnostics[1].message,
            "The type 'Color?' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'null'."
        );

        let with_null = report(
            "enum Color { red, green, blue } void f(Color? c) { var x = switch (c) { Color.red => 1, Color.green => 2, Color.blue => 3, null => 4 }; }",
        );
        // `x` is unread.
        assert_eq!(code_names(&with_null), ["unused_local_variable"]);
    }

    /// A single non-sealed type case covers the whole (non-nullable)
    /// scrutinee: exhaustiveness does not require a seal.
    ///
    /// Hand-trace: `case C v:` is a type-free-variable frame whose context
    /// substitutes to `Named(C, false)` — the full space of the scrutinee.
    #[test]
    fn single_type_case_covers_non_sealed_scrutinee() {
        // `x` and the case variable `v` are never read (the official
        // read-only use rule).
        let report = report("class C {} void f(C c) { var x = switch (c) { C v => 1 }; }");
        assert_eq!(
            code_names(&report),
            ["unused_local_variable", "unused_local_variable"]
        );
    }

    /// A cyclic sealed hierarchy terminates: the covering search walks the
    /// subclass closure through the file-scoped facts, which cannot reject
    /// `sealed S extends S` (self-edge dropped at the source) or a mutual
    /// `A`/`B` cycle (dropped by the walk's visited set) — without the
    /// guard the FIFO expansion re-enqueues the same type forever.
    ///
    /// Hand-trace: the `1` constant is the unknown space (never a
    /// must-match), so the sealed walk must expand its subtypes — the
    /// exact shape the cycle breaks. Both verdicts are exhaustive (no
    /// witness over a sealed closure), so nothing emits.
    #[test]
    fn cyclic_sealed_hierarchy_terminates_without_false_positive() {
        let self_cycle =
            report("sealed class S extends S {} void f(S s) { var x = switch (s) { 1 => 2 }; }");
        // `x` is unread (the official read-only use rule).
        assert_eq!(code_names(&self_cycle), ["unused_local_variable"]);
        let mutual_cycle = report(
            "sealed class A extends B {} sealed class B extends A {} void f(A a) { var x = switch (a) { 1 => 2 }; }",
        );
        assert_eq!(code_names(&mutual_cycle), ["unused_local_variable"]);
    }

    /// An unresolvable scrutinee skips the whole exhaustiveness check (the
    /// oracle's `InvalidType` gate at `constant_verifier.dart:1035` and
    /// `:1095`): a switch expression over a call whose type never resolves
    /// emits nothing — mapping it to the nullable top would drop every row
    /// and false-positive the covering gap.
    #[test]
    fn unknown_scrutinee_switch_expression_emits_nothing() {
        // `x` is unread (the official read-only use rule); the `_`
        // wildcard never reports.
        let report = report("void g() {} void f() { var x = switch (g()) { _ => 0 }; }");
        assert_eq!(code_names(&report), ["unused_local_variable"]);
    }

    /// A switch expression over a name declared outside the file emits no
    /// covering gap: the file-scoped world cannot decide the name's
    /// sealedness, so the expression face suppresses (the cross-file-sealed
    /// interim seam) instead of false-positiving off an opaque non-sealed
    /// reading.
    ///
    /// Hand-trace: `S`/`A`/`B` have no in-file containers, so pre-fix the
    /// scrutinee read as non-sealed with rows that never must-match it —
    /// a witness over any `S` value, emitted as
    /// `non_exhaustive_switch_expression`. The one remaining `undefined_class`
    /// is the unresolved parameter type itself (oracle-matching, see
    /// `type_positions_report_undefined_class`); the object patterns
    /// `A()`/`B()` stay silent.
    #[test]
    fn cross_file_sealed_scrutinee_expression_emits_no_covering_gap() {
        let report = report("void f(S s) { var x = switch (s) { A() => 1, B() => 2 }; }");
        // `x` is unread (the official read-only use rule), after the
        // unresolved parameter type.
        assert_eq!(
            code_names(&report),
            ["undefined_class", "unused_local_variable"],
            "{:?}",
            code_names(&report)
        );
    }

    /// A switch expression's duplicate case is `unreachable_switch_case`
    /// anchored at the repeated case's `=>` (the expression-case anchor,
    /// constant_verifier.dart:1080-1088).
    #[test]
    fn switch_expression_duplicate_case_is_unreachable_at_arrow() {
        let report = report(
            "enum Color { red, green, blue } void f(Color c) { var x = switch (c) { Color.red => 1, Color.red => 2, Color.green => 3, Color.blue => 4 }; }",
        );
        // `x` is unread (the official read-only use rule), before the
        // duplicate-case row.
        assert_eq!(
            code_names(&report),
            ["unused_local_variable", "unreachable_switch_case"]
        );
        // The repeated case's `=>` (the expression-case anchor).
        let arrow_at = u32::try_from(
            "enum Color { red, green, blue } void f(Color c) { var x = switch (c) { Color.red => 1, Color.red ".len(),
        )
        .unwrap();
        assert_eq!(report.diagnostics[1].range.offset, arrow_at);
    }

    /// A typed variable pattern binds its local for the body: the bound
    /// name reads cleanly, and the case narrows to the pattern's type.
    ///
    /// Hand-trace: `handle_declared_variable_pattern` adds `other` to the
    /// flat local frame with the armed `Color` type — `other;` resolves
    /// through locals, so no `undefined_identifier`.
    #[test]
    fn typed_pattern_binds_local() {
        let report = report(
            "enum Color { red } void f(Color c) { switch (c) { case Color other: other; break; } }",
        );
        assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
    }

    /// An if-case binds the same way: `if (o case Type x)` declares `x`
    /// for the then-block.
    #[test]
    fn if_case_binds_local() {
        let report =
            report("enum Color { red } void f(Color c) { if (c case Color other) { other; } }");
        assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
    }

    /// A type-free `var` pattern binds without a type and covers its
    /// context (substituted with the scrutinee's type at case close).
    #[test]
    fn untyped_pattern_binds_and_covers() {
        let report = report("class C {} void f(C c) { switch (c) { case var y: y; break; } }");
        assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
    }

    /// Record patterns destructuring by shape: the fields bind as typed
    /// locals and the name level emits no shape diagnostic (record shape
    /// checking is the types-crate face — the documented M2 name-level
    /// blind spot).
    ///
    /// Hand-trace: the record pattern bracket pushes the unknown-space leaf
    /// (never a false must-match); the field patterns bind `a`/`b` through
    /// their declared-variable events, so both reads resolve.
    #[test]
    fn record_pattern_binds_fields_without_shape_diagnostic() {
        let report = report("class C {} void f(C c) { if (c case (C a, C b)) { a; b; } }");
        assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
    }

    // —— case-root capture (the case-sink window) ———————————————————————

    /// An if-case root before a switch never feeds its cases: roots are
    /// captured only across a case header's pattern window, so the stale
    /// unknown from `if (y case 42)` can neither cover a case nor trip the
    /// empty fallback.
    ///
    /// Hand-trace (pre-fix): the stale root sat below the case roots; the
    /// second case's `end_switch_case` drained it and reported a false
    /// `non_exhaustive_switch_statement` over `bool`.
    #[test]
    fn stale_if_case_root_never_feeds_a_case() {
        let report = report(
            "class bool {} void g(bool y, bool x) { if (y case 42) {} switch (x) { case false: break; case true: break; } }",
        );
        assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
    }

    /// An if-case inside one case's body never feeds the next case: each
    /// header captures at its own `end_case_expression`, so the stale root
    /// cannot shift the case/space pairing.
    ///
    /// Hand-trace (pre-fix): `case E.a`'s body if-case pushed above `E.a`'s
    /// root and the LIFO pop handed the *next* case the wrong spaces —
    /// a false `non_exhaustive_switch_statement` (missing `E.b`).
    #[test]
    fn stale_case_body_root_never_feeds_the_next_case() {
        let report = report(
            "class bool {} enum E { a, b } void f(bool y, E e) { switch (e) { case E.a: if (y case 42) {} case E.b: break; } }",
        );
        assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
    }

    /// A nested switch opens after the enclosing case's root was captured:
    /// the `open_switch` debris-clear must not destroy live case spaces.
    #[test]
    fn nested_switch_preserves_the_outer_case_root() {
        let report = report(
            "enum E { a, b } void f(E a, E b) { switch (a) { case E.a: switch (b) { case E.a: break; case E.b: break; } case E.b: break; } }",
        );
        assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
    }

    /// A shared-body case group (`case A: case B:`) captures one case per
    /// header, matching the oracle's per-`case` nodes.
    ///
    /// Hand-trace (pre-fix): one body event drained a single root, so the
    /// group collapsed to its *last* header's space and `E.a` went
    /// uncovered — a false `non_exhaustive_switch_statement`.
    #[test]
    fn shared_body_case_group_captures_each_header() {
        let report =
            report("enum E { a, b } void f(E e) { switch (e) { case E.a: case E.b: break; } }");
        assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
    }

    /// A labeled duplicate case anchors `unreachable_switch_case` at the
    /// `case` keyword itself, not the label (the oracle's
    /// `caseNode.keyword`, constant_verifier.dart:1076-1085).
    #[test]
    fn labeled_case_anchors_unreachable_at_case_keyword() {
        let report = report(
            "enum Color { red, green } void f(Color c) { switch (c) { case Color.red: break; case Color.green: break; l: case Color.red: break; } }",
        );
        assert_eq!(code_names(&report), ["unreachable_switch_case"]);
        let case_at = u32::try_from(
            "enum Color { red, green } void f(Color c) { switch (c) { case Color.red: break; case Color.green: break; l: ".len(),
        )
        .unwrap();
        assert_eq!(report.diagnostics[0].range.offset, case_at);
    }

    /// A `default` after total coverage is `unreachable_switch_default`
    /// (`STATIC_WARNING`) at the `default` keyword — the case spaces
    /// already cover the scrutinee and the enum face is always
    /// mustBeExhaustive (`nonExhaustiveness == null && defaultNode != null
    /// && mustBeExhaustive`, constant_verifier.dart:1141-1148).
    #[test]
    fn default_after_total_coverage_is_unreachable() {
        let report = report(
            "enum Color { red, green, blue } void f(Color c) { switch (c) { case Color.red: break; case Color.green: break; case Color.blue: break; default: break; } }",
        );
        assert_eq!(
            code_names(&report),
            ["unreachable_switch_default"],
            "{:?}",
            code_names(&report)
        );
        let d = &report.diagnostics[0];
        assert_eq!(
            d.message,
            "This default clause is covered by the previous cases."
        );
        assert_eq!(d.severity, Severity::Warning);
        let default_at = u32::try_from(
            "enum Color { red, green, blue } void f(Color c) { switch (c) { case Color.red: break; case Color.green: break; case Color.blue: break; ".len(),
        )
        .unwrap();
        assert_eq!(d.range.offset, default_at);
    }

    /// A `default` covering a real gap stays reachable: the
    /// unreachable-default report lives only in the covering search's
    /// exhaustive branch (the `nonExhaustiveness != null` gate silences
    /// the whole else arm), and a switch without a default that covers
    /// stays silent.
    #[test]
    fn reachable_default_and_no_default_stay_silent() {
        let covering_gap = report(
            "enum Color { red, green, blue } void f(Color c) { switch (c) { case Color.red: break; default: break; } }",
        );
        assert_eq!(
            covering_gap.diagnostics.len(),
            0,
            "{:?}",
            code_names(&covering_gap)
        );
        let no_default = report(
            "enum Color { red, green, blue } void f(Color c) { switch (c) { case Color.red: break; case Color.green: break; case Color.blue: break; } }",
        );
        assert_eq!(
            no_default.diagnostics.len(),
            0,
            "{:?}",
            code_names(&no_default)
        );
    }

    /// A statement switch over a *nullable* sealed scrutinee is
    /// mustBeExhaustive — the official check reads the scrutinee's element
    /// face and is nullability-agnostic (type_system.dart:846-866,
    /// `isAlwaysExhaustive(S?)` over a sealed `S`) — so the missing `null`
    /// half reports; covering it silences.
    #[test]
    fn nullable_sealed_statement_switch_needs_null() {
        let missing = report(
            "sealed class S {} class A extends S {} class B extends S {} void f(S? s) { switch (s) { case A a: break; case B b: break; } }",
        );
        // The case variables `a`/`b` are unread (the official read-only
        // use rule), after the exhaustiveness row.
        assert_eq!(
            code_names(&missing),
            [
                "non_exhaustive_switch_statement",
                "unused_local_variable",
                "unused_local_variable",
            ],
            "{:?}",
            code_names(&missing)
        );
        assert_eq!(
            missing.diagnostics[0].message,
            "The type 'S?' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'null'."
        );

        let with_null = report(
            "sealed class S {} class A extends S {} class B extends S {} void f(S? s) { switch (s) { case A a: break; case B b: break; case null: break; } }",
        );
        // The case variables `a`/`b` are unread.
        assert_eq!(
            code_names(&with_null),
            ["unused_local_variable", "unused_local_variable"],
            "{:?}",
            code_names(&with_null)
        );
    }

    /// A nullable *non-sealed* scrutinee stays off the mustBeExhaustive
    /// gate (the element is not sealed, type_system.dart:846-866): a
    /// statement switch without a default emits nothing.
    #[test]
    fn nullable_non_sealed_statement_switch_stays_unconditional() {
        // The case variable `c` is unread (the official read-only use
        // rule); the exhaustiveness gate stays off for the non-sealed
        // nullable scrutinee.
        let report = report("class C {} void f(C? c) { switch (c) { case C c: break; } }");
        assert_eq!(code_names(&report), ["unused_local_variable"]);
    }

    /// `NonNullableStaticType.isSubtypeOf` (static_type.dart:415-421):
    /// every non-nullable static type — named, enum constant, or bool
    /// value — subtypes the non-nullable top; `Null` and the nullable
    /// projections do not.
    #[test]
    fn non_nullable_types_subtype_the_non_nullable_top() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let file = files.intern(Path::new("main.dart"));
        let index = ElementIndex::default();
        let world = NameWorld {
            index: &index,
            file,
            strings: &strings,
            bool_id: strings.intern("bool"),
            unknowns: std::cell::Cell::new(0),
        };
        let c = strings.intern("C");
        let e = strings.intern("E");
        assert!(world.is_subtype_of(&NameTy::Named(c, false), &NameTy::TopNonNullable));
        assert!(world.is_subtype_of(&NameTy::EnumValue(e, c), &NameTy::TopNonNullable));
        assert!(world.is_subtype_of(&NameTy::BoolValue(true), &NameTy::TopNonNullable));
        assert!(!world.is_subtype_of(&NameTy::Null, &NameTy::TopNonNullable));
        assert!(!world.is_subtype_of(&NameTy::Named(c, true), &NameTy::TopNonNullable));
    }

    /// An object pattern's typed field covers its column when the member's
    /// declared type subtypes the field's type test
    /// ([`NameWorld::property_type`], the member-table walk, climbed from
    /// the subclass here): the row lowers to the bare head, so `A()` plus
    /// `B(x: int _)` is exhaustive over the sealed pair. Pre-fix the field
    /// leaf `and`-folded against the head into the unknown space and the
    /// covering gap mis-reported.
    ///
    /// The genuinely non-exhaustive direction stays reported: a narrower
    /// field test (`int` against `num`) does not cover the column, the row
    /// stays unknown-shaped, and the gap names `B`.
    #[test]
    fn object_pattern_typed_field_covers_when_the_member_type_subtypes_it() {
        let covered = report(
            "class int {} sealed class S {} class A extends S {} sealed class Base extends S { int x = 0; } class B extends Base {} \
             void f(S s) { var v = switch (s) { A() => 1, B(x: int _) => 2 }; }",
        );
        // `v` is unread (the official read-only use rule); the `int _`
        // wildcard never reports.
        assert_eq!(code_names(&covered), ["unused_local_variable"]);

        let narrower = report(
            "class int {} class num {} sealed class S {} class A extends S {} sealed class Base extends S { num x = 0; } class B extends Base {} \
             void f(S s) { var v = switch (s) { A() => 1, B(x: int _) => 2 }; }",
        );
        // `v` is unread, before the exhaustiveness row.
        assert_eq!(
            code_names(&narrower),
            ["unused_local_variable", "non_exhaustive_switch_expression"],
            "{:?}",
            code_names(&narrower)
        );
    }

    /// A type-free object field (`B(:var x)`) covers its column without
    /// consulting the member table, and an unresolvable head
    /// (`undefined_class`, the cross-file shape) keeps the whole row in
    /// the unknown space — no new report beyond the name resolution one.
    #[test]
    fn object_pattern_type_free_field_and_opaque_head_stay_quiet() {
        let type_free = report(
            "sealed class S {} class A extends S {} class B extends S {} \
             void f(S s) { var v = switch (s) { A() => 1, B(:var x) => 2 }; }",
        );
        // `v` and the bound field variable `x` are unread (the official
        // read-only use rule).
        assert_eq!(
            code_names(&type_free),
            ["unused_local_variable", "unused_local_variable"]
        );
        let opaque = report("void f(S s) { var v = switch (s) { A() => 1, B(:var x) => 2 }; }");
        // `v` and `x` are unread, after the unresolved parameter type.
        assert_eq!(
            code_names(&opaque),
            [
                "undefined_class",
                "unused_local_variable",
                "unused_local_variable",
            ]
        );
    }

    // —— restricted nullable projections (the Null|value fold) ———————————

    /// `nullable` over a restricted value keeps the restriction: the
    /// `Null|BoolValue(true)` fold lowers to `true?`, so `case null` after
    /// `case true || null` is unreachable while `case false` stays
    /// reachable and the `false` gap still reports.
    ///
    /// Hand-trace: the fold space covers `{true, null}` only
    /// (`NullableStaticType(_BoolValueStaticType)`, static_type.dart:350);
    /// lowering it to `bool?` would swallow `false` on both questions.
    #[test]
    fn bool_value_nullable_keeps_the_restriction() {
        let report = report(
            "class bool {} void f(bool b) { switch (b) { case true || null: break; case null: break; } }",
        );
        assert_eq!(
            code_names(&report),
            ["unreachable_switch_case", "non_exhaustive_switch_statement"],
            "{:?}",
            code_names(&report)
        );
        // The `false` gap, not a swallowed one.
        assert_eq!(
            report.diagnostics[1].message,
            "The type 'bool' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'false'."
        );
        // `case null` anchors at its own `case` keyword.
        let null_case_at = u32::try_from(
            "class bool {} void f(bool b) { switch (b) { case true || null: break; ".len(),
        )
        .unwrap();
        assert_eq!(report.diagnostics[0].range.offset, null_case_at);
    }

    /// The same precision for enum constants: `case E.a || null` covers
    /// `{E.a, null}` only — the sibling `E.b` stays reachable, and the
    /// nullable-scrutinee statement face emits nothing else.
    ///
    /// Hand-trace (pre-fix): the fold lowered to `E?`, whose split covers
    /// every constant — a false `unreachable_switch_case` on `E.b`.
    #[test]
    fn enum_value_nullable_keeps_the_restriction() {
        let report = report(
            "enum E { a, b } void f(E? e) { switch (e) { case E.a || null: break; case E.b: break; } }",
        );
        assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
    }

    // —— enum/mixin supertype edges toward sealed classes —————————————————

    /// An enum's `implements` and a mixin's `on` edge reach the sealed
    /// splitter: both join the sealed class's closed subtype set, so
    /// covering the enum's constants and the mixin covers everything.
    ///
    /// Hand-trace: `subtypes(S)` walks the unit's containers over the
    /// superclass and interface edges → `[E, M]`; `E` splits over its
    /// constants and `M` is covered by the typed case.
    #[test]
    fn enum_and_mixin_edges_split_the_sealed_class() {
        let covered = report(
            "sealed class S {} enum E implements S { a, b } mixin M on S {} void f(S s) { switch (s) { case E.a: break; case E.b: break; case M m: break; } }",
        );
        // The case variable `m` is unread (the official read-only use rule).
        assert_eq!(code_names(&covered), ["unused_local_variable"]);

        let missing = report(
            "sealed class S {} enum E implements S { a, b } mixin M on S {} void f(S s) { switch (s) { case E.a: break; case E.b: break; } }",
        );
        assert_eq!(
            code_names(&missing),
            ["non_exhaustive_switch_statement"],
            "{:?}",
            code_names(&missing)
        );
        assert_eq!(
            missing.diagnostics[0].message,
            "The type 'S' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'M'."
        );
    }

    /// A typed wildcard (`M _`) covers its declared type's space: the
    /// canonical sealed-coverage style turns exhaustive.
    ///
    /// Hand-trace (pre-fix): the wildcard dropped its type, each case
    /// collapsed to the context space, and the second case reported a false
    /// `unreachable_switch_case`.
    #[test]
    fn typed_wildcard_covers_its_type() {
        let report = report(
            "sealed class S {} class A extends S {} class B extends S {} void f(S s) { switch (s) { case A _: break; case B _: break; } }",
        );
        assert_eq!(report.diagnostics.len(), 0, "{:?}", code_names(&report));
    }

    /// A bare `_` still covers the whole context (no leaf of its own).
    #[test]
    fn bare_wildcard_covers_the_context() {
        // `x` is unread (the official read-only use rule); the `_`
        // wildcard never reports.
        let report = report("class C {} void f(C c) { var x = switch (c) { _ => 1 }; }");
        assert_eq!(code_names(&report), ["unused_local_variable"]);
    }

    // —— #279: the constant-evaluation family ———————————————————————————
    //
    // Anchors: the reporting sites in
    // `.third/dart-sdk/pkg/analyzer/lib/src/dart/constant/
    // constant_verifier.dart` (`visitVariableDeclaration` :552, the
    // `_reportError` allow-list :717+), the operator matrix in
    // `.../constant/value.dart`, the invocation faults in
    // `.../constant/evaluation.dart` (`_invalidConstantForMethodInvocation`
    // :2197), the verbatim texts in
    // `.../analyzer/lib/src/diagnostic/diagnostic.g.dart`, and the corpus
    // `pkg/analyzer/test/src/dart/constant/evaluation_test.dart` /
    // `tests/language/operator/integer_division_by_zero_test.dart`.

    /// Clean const initializers over every subset shape: arithmetic,
    /// adjacent strings, collections, records, const-ctor creations
    /// (explicit and implicit), forward const references — no rows at all.
    #[test]
    fn const_initializer_subset_shapes_stay_clean() {
        let rep = report(
            "class int {} class C { const C(); } \
             const a = 1 + 2; \
             const b = 'x' 'y'; \
             const c = [1, 2]; \
             const d = {'k': 1}; \
             const e = {}; \
             const g = C(); \
             const h = const C(); \
             const fwd = base; const base = 1; \
             const neg = -a; \
             const shift = 1 << 2;",
        );
        assert_eq!(rep.diagnostics.len(), 0, "{:?}", code_names(&rep));
    }

    /// A provably non-const operand reports the site default
    /// `const_initialized_with_non_constant_value` anchored at the
    /// offending subexpression (constant_verifier.dart:552; the
    /// `InvalidConstant` error entity).
    #[test]
    fn non_const_initializer_reports_the_site_default() {
        let rep = report("class int {} int nc = 0; const x = 1 + nc;");
        assert_eq!(
            code_names(&rep),
            ["const_initialized_with_non_constant_value"],
            "{:?}",
            code_names(&rep)
        );
        // The local/formal shape proves non-constancy too; the const
        // local `y` is never read (the official read-only use rule), and
        // the const verdict anchors at the offending subexpression `l`.
        let rep = report("void m() { var l = 1; const y = l; }");
        assert_eq!(
            code_names(&rep),
            [
                "unused_local_variable",
                "const_initialized_with_non_constant_value",
            ],
            "{:?}",
            code_names(&rep)
        );
    }

    /// Unknown names defer: the M1 row reports, no const verdict fires
    /// (under-report discipline — use-before-decl stays suppressed).
    #[test]
    fn unknown_names_defer_without_a_const_verdict() {
        let rep = report("const x = gone;");
        assert_eq!(
            code_names(&rep),
            ["undefined_identifier"],
            "{:?}",
            code_names(&rep)
        );
    }

    /// A self-reference inside its own initializer is the cycle fault
    /// (the allow-list's `recursive_compile_time_constant` override,
    /// constant_verifier.dart:717+).
    #[test]
    fn self_referential_constant_reports_the_cycle() {
        let rep = report("const a = a + 1;");
        assert_eq!(
            code_names(&rep),
            ["recursive_compile_time_constant"],
            "{:?}",
            code_names(&rep)
        );
    }

    /// Const-creation arguments check per argument with
    /// `const_with_non_constant_argument`
    /// (`_validateConstantArguments`, constant_verifier.dart:843): the
    /// explicit `const C(l)` and the implicit `C(l)` forms, and enum
    /// constants with a declared const constructor.
    #[test]
    fn const_creation_arguments_check_per_argument() {
        let rep = report(
            "class int {} class C { const C(int v); } \
             void m() { var l = 1; const a = C(l); const b = const C(l); }",
        );
        // The const locals `a`/`b` are never read (the official read-only
        // use rule); the argument verdicts anchor at their `l` arguments.
        assert_eq!(
            code_names(&rep),
            [
                "unused_local_variable",
                "const_with_non_constant_argument",
                "unused_local_variable",
                "const_with_non_constant_argument",
            ],
            "{:?}",
            code_names(&rep)
        );

        let rep = report(
            "class int {} int l = 0; enum E { a.named(l); const E.named(int v); final int v; }",
        );
        assert_eq!(
            code_names(&rep),
            ["const_with_non_constant_argument"],
            "{:?}",
            code_names(&rep)
        );
    }

    /// A const-creation target this file declared with a const
    /// constructor keeps clean arguments silent, and the creation binds
    /// its folded value (`const x = C();` has static type `C`). The
    /// negative face (a foreign host type) defers: user-class pairs sit
    /// outside the M1's final-primitive table, which is the engine's
    /// documented under-report direction.
    #[test]
    fn const_creation_type_folds_to_its_container() {
        // `ok` is never read (the official read-only use rule); the const
        // chain itself stays clean.
        let rep = report(
            "class C { const C(); } \
             const x = C(); \
             const w = x; \
             void m() { C ok = x; }",
        );
        assert_eq!(code_names(&rep), ["unused_local_variable"]);
    }

    /// A container without a const constructor and an unresolvable name
    /// both defer (`const_with_non_const*` is outside the family) — no
    /// const verdict either way.
    #[test]
    fn non_const_and_unknown_creation_targets_defer() {
        let rep = report("class C { C(); } const x = C();");
        assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));

        let rep = report("const y = Gone();");
        assert!(
            code_names(&rep)
                .iter()
                .all(|c| !c.starts_with("const_") && !c.starts_with("non_constant_")),
            "{:?}",
            code_names(&rep)
        );
    }

    /// An invocable non-constructor callee faults with
    /// `const_eval_method_invocation` at the invocation
    /// (evaluation.dart:2197, diagnostic.g.dart:2846).
    #[test]
    fn invocable_callee_reports_method_invocation() {
        let rep = report("void f() {} const x = f();");
        assert_eq!(
            code_names(&rep),
            ["const_eval_method_invocation"],
            "{:?}",
            code_names(&rep)
        );
        assert_eq!(
            rep.diagnostics[0].message,
            "Methods can't be invoked in constant expressions."
        );
        // A local callable faults the same way; the const local `y` is
        // never read (the official read-only use rule), before the
        // invocation anchor.
        let rep = report("void m() { var g = 1; const y = g(); }");
        assert_eq!(
            code_names(&rep),
            ["unused_local_variable", "const_eval_method_invocation"],
            "{:?}",
            code_names(&rep)
        );
    }

    /// Explicit const literals check per element with their
    /// `non_constant_*_element` codes (the `_ConstLiteralVerifier` sites).
    #[test]
    fn const_literal_elements_check_per_element() {
        let rep = report(
            "void m() { var l = 1; \
             const a = [l]; \
             const b = {l}; \
             const c = {l: 1}; \
             const d = (l,); }",
        );
        // The const locals `a`/`b`/`c`/`d` are never read (the official
        // read-only use rule); each element verdict anchors at its `l`
        // element, interleaving in source order.
        assert_eq!(
            code_names(&rep),
            [
                "unused_local_variable",
                "non_constant_list_element",
                "unused_local_variable",
                "non_constant_set_element",
                "unused_local_variable",
                "non_constant_map_element",
                "unused_local_variable",
                "non_constant_record_field",
            ],
            "{:?}",
            code_names(&rep)
        );
        // Clean elements stay silent, including the fault-deciding lazy
        // shapes (`false && l` never evaluates its right operand); the
        // const locals `e`/`f`/`g` are unread.
        let rep = report(
            "void m() { var l = 1; const e = [0, 1]; const f = {0: 1}; const g = false && l == 1; }",
        );
        assert_eq!(
            code_names(&rep),
            [
                "unused_local_variable",
                "unused_local_variable",
                "unused_local_variable",
            ],
            "{:?}",
            code_names(&rep)
        );
    }

    /// The `constEval*` fault family overrides the site default, one code
    /// per subfamily (the `_reportError` allow-list): division throws,
    /// operand-type asserts, interpolation operand gate.
    #[test]
    fn const_eval_faults_match_the_official_codes() {
        // `~/ 0` throws IntegerDivisionByZeroException
        // (tests/language/operator/integer_division_by_zero_test.dart);
        // `% 0` throws the generic exception
        // (evaluation_test.dart:1765).
        let rep = report("const a = 1 ~/ 0; const b = 1 % 0;");
        assert_eq!(
            code_names(&rep),
            ["const_eval_throws_idbze", "const_eval_throws_exception"],
            "{:?}",
            code_names(&rep)
        );
        // Operand-type asserts, left operand first
        // (evaluation_test.dart:1895 anchors the bool case).
        let rep = report("const c = !1; const d = 1 + 'a'; const e = 1 & true; const f = 'a' + 1;");
        assert_eq!(
            code_names(&rep),
            [
                "const_eval_type_bool",
                "const_eval_type_num",
                "const_eval_type_bool_int",
                "const_eval_throws_exception",
            ],
            "{:?}",
            code_names(&rep)
        );
        // Interpolation operand gate (`isBoolNumStringOrNull`,
        // value.dart:545): a constant operand of a non-convertible type
        // faults; a NON-CONSTANT operand keeps the site default (the
        // oracle fails its evaluation first).
        let rep = report("const g = 'a${[1]}';");
        assert_eq!(
            code_names(&rep),
            ["const_eval_type_bool_num_string"],
            "{:?}",
            code_names(&rep)
        );
        // The const local `h` is never read (the official read-only use
        // rule); the verdict anchors at the interpolated `l`.
        let rep = report("void m() { var l = 1; const h = 'a${l}'; }");
        assert_eq!(
            code_names(&rep),
            [
                "unused_local_variable",
                "const_initialized_with_non_constant_value",
            ],
            "{:?}",
            code_names(&rep)
        );
    }

    /// `is` is not on the spec's constant-expression list (the site
    /// default reports); `as` is, and its unknown result defers —
    /// `(1 as int) + 1` stays clean.
    #[test]
    fn is_reports_and_as_defers() {
        let rep = report("class int {} const x = 1 is int;");
        assert_eq!(
            code_names(&rep),
            ["const_initialized_with_non_constant_value"],
            "{:?}",
            code_names(&rep)
        );
        let rep = report("class int {} const y = 1 as int; const z = (1 as int) + 1;");
        assert_eq!(rep.diagnostics.len(), 0, "{:?}", code_names(&rep));
    }

    /// A clean const variable's type folds from its initializer: `const
    /// z = 1;` gives `z` static type `int` (the official inference), so a
    /// String host rejects the read while an int host stays silent.
    #[test]
    fn const_variable_type_folds_per_the_official_rules() {
        let rep = report(
            "class int {} class String {} const z = 1; \
             void m() { int ok = z; String bad = z; }",
        );
        // `ok`/`bad` are never read (the official read-only use rule);
        // `bad`'s initializer reports the assignment.
        assert_eq!(
            code_names(&rep),
            [
                "unused_local_variable",
                "unused_local_variable",
                "invalid_assignment",
            ],
            "{:?}",
            code_names(&rep)
        );
        assert_eq!(
            rep.diagnostics[2].message,
            "A value of type 'int' can't be assigned to a variable of type 'String'."
        );
        // Enum constants fold to their container too (the negative face
        // against a user class defers — outside the final-primitive
        // table); `ok` is never read.
        let rep = report("enum E { a, b } void m() { E ok = E.a; }");
        assert_eq!(code_names(&rep), ["unused_local_variable"]);
    }

    /// Deferral shapes never fabricate a verdict: interpolation text,
    /// string repetition, index expressions, member invocations — all
    /// silent here (documented under-report, see `const_eval` module
    /// docs).
    #[test]
    fn deferred_shapes_stay_silent() {
        let rep = report(
            "class int {} const n = 3; const a = 'x${n}'; const b = 'ab' * 2; \
             class C { const C(); } const c = C(); \
             const d = 'a$b'; const w = c;",
        );
        // `d` interpolates an unknown constant (`b` folds deferred) —
        // clean; `w` reads a deferred creation — clean.
        assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
    }

    // —— invalid_override (#453) ——————————————————————————————————————

    /// An equal-kind method whose positional arity differs from the
    /// inherited member emits `invalid_override` at the declaring member's
    /// name span.
    ///
    /// Hand-trace: `B.f` (1 positional) vs the nearest `f` on the `A` face
    /// (0 positional) — the arity arm decides before any type question.
    #[test]
    fn arity_mismatch_emits_invalid_override() {
        let rep =
            report("class int {} class A { void f() {} } class B extends A { void f(int x) {} }");
        assert_eq!(
            code_names(&rep),
            ["invalid_override"],
            "{:?}",
            code_names(&rep)
        );
        assert_eq!(
            rep.diagnostics[0].message,
            "'B.f' ('void Function(int)') isn't a valid override of 'A.f' ('void Function()')."
        );
    }

    /// An equal-arity method whose named-parameter set narrows the
    /// inherited face emits: dropping an inherited named parameter, or
    /// adding one as `required`, rejects calls the super accepts. An added
    /// optional named parameter is the valid widening and stays silent.
    #[test]
    fn named_set_mismatch_emits_invalid_override() {
        let rep = report(
            "class int {} class A { void f(int x) {} } \
             class B extends A { void f(int x, {required int y}) {} }",
        );
        assert_eq!(
            code_names(&rep),
            ["invalid_override"],
            "{:?}",
            code_names(&rep)
        );
        assert_eq!(
            rep.diagnostics[0].message,
            "'B.f' ('void Function(int, {required int y})') isn't a valid override of 'A.f' ('void Function(int)')."
        );
        // The widening arm: `{int? y}` added accepts every call the super
        // accepts — no report (oracle-verified).
        let rep = report(
            "class int {} class A { void f(int x) {} } \
             class B extends A { void f(int x, {int? y}) {} }",
        );
        assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
    }

    /// The type arm emits only when nullability decides: a nullable getter
    /// result over a non-nullable one is a decidable violation; two
    /// distinct non-nullable named types are incomparable at the name
    /// level (`assignable_from` answers `None`) — suppress, the
    /// documented under-report direction.
    #[test]
    fn type_verdict_follows_assignable_from() {
        let rep = report(
            "class int {} class A { int get f => 1; } class B extends A { int? get f => 2; }",
        );
        assert_eq!(
            code_names(&rep),
            ["invalid_override"],
            "{:?}",
            code_names(&rep)
        );
        assert_eq!(
            rep.diagnostics[0].message,
            "'B.f' ('int? Function()') isn't a valid override of 'A.f' ('int Function()')."
        );
        let rep = report(
            "class int {} class String {} class A { String f() => 'x'; } \
             class B extends A { int f() => 1; }",
        );
        assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
    }

    /// Static members take no part in overriding: a static inherited
    /// member is invisible to the face walk, and a static declaration is
    /// skipped before any comparison.
    #[test]
    fn static_members_are_skipped() {
        let rep = report(
            "class int {} class A { static void f() {} } class B extends A { void f(int x) {} }",
        );
        assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
        let rep = report(
            "class int {} class A { void f() {} } class B extends A { static void f(int x) {} }",
        );
        assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
    }

    /// An unresolvable superclass suppresses the whole check for its
    /// dependents — the chain break is suppression territory, never a
    /// report. (The broken superclass edge also makes every member walk
    /// from `B` chain-break, so the header's own unresolved name stays
    /// silent under the same false-positive defense.)
    #[test]
    fn unresolved_superclass_suppresses() {
        let rep = report("class B extends Missing { void f(int x) {} } class int {}");
        assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
    }

    /// A mixin's `on` constraints are super-faces for its own members: a
    /// mixin member clashing with an `on`-type member emits.
    #[test]
    fn mixin_on_constraint_is_a_super_face() {
        let rep = report("class int {} class A { void f(int x) {} } mixin M on A { void f() {} }");
        assert_eq!(
            code_names(&rep),
            ["invalid_override"],
            "{:?}",
            code_names(&rep)
        );
        assert_eq!(
            rep.diagnostics[0].message,
            "'M.f' ('void Function()') isn't a valid override of 'A.f' ('void Function(int)')."
        );
    }

    /// A field's face is its declared type: a nullable field type over a
    /// non-nullable inherited field emits (the FieldDeclaration hook).
    #[test]
    fn field_over_field_decidable_nullability_emits() {
        let rep = report("class int {} class A { int x = 1; } class B extends A { int? x = 2; }");
        assert_eq!(
            code_names(&rep),
            ["invalid_override"],
            "{:?}",
            code_names(&rep)
        );
        assert_eq!(
            rep.diagnostics[0].message,
            "'B.x' ('int? Function()') isn't a valid override of 'A.x' ('int Function()')."
        );
    }

    /// The setter variant keeps the `invalid_override` code name and
    /// swaps the message text; a decidable contravariant parameter
    /// conflict (non-nullable declared setter parameter over a nullable
    /// inherited one) emits.
    #[test]
    fn setter_variant_keeps_the_code_name() {
        let rep = report(
            "class int {} class A { set f(int? x) {} } class B extends A { set f(int y) {} }",
        );
        assert_eq!(
            code_names(&rep),
            ["invalid_override"],
            "{:?}",
            code_names(&rep)
        );
        assert_eq!(
            rep.diagnostics[0].message,
            "The setter 'B.f' ('void Function(int)') isn't a valid override of 'A.f' ('void Function(int?)')."
        );
    }

    /// The residency widening pays off: a superclass in another resident
    /// library (same package, two files) resolves through the namespace
    /// hop, so the override verdict fires across the file seam.
    #[test]
    fn cross_file_superclass_emits() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let file_a = files.intern(Path::new("a.dart"));
        let file_m = files.intern(Path::new("main.dart"));
        let source_a = "class A { void f() {} }";
        let source_m = "import 'a.dart'; class int {} class B extends A { void f(int x) {} }";
        let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
            LanguageVersion::new(3, 15),
        ));
        let scanned_a = dartq_scanner::scan(
            source_a,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file_a,
        );
        let scanned_m = dartq_scanner::scan(
            source_m,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file_m,
        );
        let elements_a = collect(source_a, &scanned_a.tokens, features, &strings);
        let elements_m = collect(source_m, &scanned_m.tokens, features, &strings);
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib_a = builder.intern("package:t/a.dart");
        let lib_m = builder.intern("package:t/main.dart");
        let mut index = ElementIndex::default();
        index.insert(file_a, elements_a.clone());
        index.insert(file_m, elements_m.clone());
        for (name, _) in elements_a.declarations() {
            builder.declare(lib_a, name, file_a);
        }
        for (name, decl) in elements_m.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib_m, name, file_m);
        }
        builder.import(
            lib_m,
            ImportEdge {
                target: Some(lib_a),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let rep = diagnose(
            source_m,
            &scanned_m.tokens,
            features,
            &strings,
            file_m,
            lib_m,
            &graph,
            &namespaces,
            &index,
        );
        assert_eq!(
            code_names(&rep),
            ["invalid_override"],
            "{:?}",
            code_names(&rep)
        );
        assert_eq!(
            rep.diagnostics[0].message,
            "'B.f' ('void Function(int)') isn't a valid override of 'A.f' ('void Function()')."
        );
    }

    // —— abstract completeness + interface conflicts (#498) ————————————

    /// The five message tiers, with the oracle-verified texts: the list is
    /// the sorted `Enclosing.member` descriptions (`getter ` prefixed for
    /// getter shapes, plain for methods — the oracle's sort places the
    /// unprefixed method first).
    #[test]
    fn missing_member_tiers_render_the_five_shapes() {
        let rep = report("abstract class A { void m(); } class B extends A {}");
        assert_eq!(
            code_names(&rep),
            ["non_abstract_class_inherits_abstract_member"],
            "{:?}",
            code_names(&rep)
        );
        assert_eq!(
            rep.diagnostics[0].message,
            "Missing concrete implementation of 'A.m'."
        );
        for (count, expected) in [
            (2, "Missing concrete implementations of 'A.a' and 'A.b'."),
            (
                3,
                "Missing concrete implementations of 'A.a', 'A.b', and 'A.c'.",
            ),
            (
                4,
                "Missing concrete implementations of 'A.a', 'A.b', 'A.c', and 'A.d'.",
            ),
            (
                5,
                "Missing concrete implementations of 'A.a', 'A.b', 'A.c', 'A.d', and 1 more.",
            ),
        ] {
            let members = "void a(); void b(); void c(); void d(); void e();"
                .split("; ")
                .take(count)
                .collect::<Vec<_>>()
                .join(" ");
            let rep = report(&format!(
                "abstract class A {{ {members} }} class B extends A {{}}"
            ));
            assert_eq!(
                code_names(&rep),
                ["non_abstract_class_inherits_abstract_member"],
                "{count}"
            );
            assert_eq!(rep.diagnostics[0].message, expected, "{count}");
        }
        // The description sort is the oracle's: the unprefixed method sorts
        // before the prefixed getter.
        let rep = report("abstract class A { void z(); int get a; } class B extends A {}");
        assert_eq!(
            rep.diagnostics[0].message,
            "Missing concrete implementations of 'A.z' and 'getter A.a'."
        );
    }

    /// `implements` contributes interface members but never
    /// implementations (`_addImplemented`'s sources are the super chain,
    /// the mixins, and own members only): a concrete member on an
    /// `implements` face still misses; the same member through `extends`
    /// resolves.
    #[test]
    fn implements_never_implements_but_extends_resolves() {
        let rep = report(
            "class A { void m() {} } abstract class B { void m(); } class C implements A, B {}",
        );
        assert_eq!(
            code_names(&rep),
            ["non_abstract_class_inherits_abstract_member"],
            "{:?}",
            code_names(&rep)
        );
        // The interface representative is the most specific candidate — the
        // super-side one wins the description.
        let rep = report(
            "class A { void m() {} } abstract class B { void m(); } class C extends B implements A {}",
        );
        assert_eq!(
            rep.diagnostics[0].message,
            "Missing concrete implementation of 'B.m'."
        );
        let rep = report("class A { void m() {} } class C extends A {}");
        assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
    }

    /// The concrete-superclass exemption (`_isNotImplementedInConcreteSuper
    /// Class`): the issue belongs to the concrete class that already lacks
    /// the member. An abstract class exempts itself from the check; its own
    /// abstract members are the `concrete_class_with_abstract_member`
    /// report's business — not implemented here, so they suppress.
    #[test]
    fn abstract_and_concrete_super_exemptions() {
        // An abstract class never reports.
        let rep = report("abstract class A { void m(); } abstract class B extends A {}");
        assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
        // A concrete class's own abstract member is the other code's row.
        let rep = report("class C { void m(); }");
        assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
        // A concrete superclass whose interface has the name blocks the
        // report for its subclasses.
        let rep = report("class C { void m(); } class D extends C {}");
        assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
    }

    /// A concrete `noSuchMethod` on the implemented sources (the class
    /// itself, the super chain, a `with` mixin) gates the whole check — the
    /// oracle's forwarder machinery implements every interface name.
    #[test]
    fn no_such_method_gates_completeness() {
        let rep = report(
            "class Invocation {} abstract class A { void m(); } \
             class B extends A { noSuchMethod(Invocation i) => null; }",
        );
        assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
        let rep = report(
            "class Invocation {} abstract class A { void m(); } \
             class S { noSuchMethod(Invocation i) => super.noSuchMethod(i); } \
             class C extends S implements A {}",
        );
        assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
    }

    /// The getter/method kind collision is unresolvable — the class-local
    /// declaration does not silence it (oracle-verified; the row anchors at
    /// the class name here, the span divergence the module docs carry).
    #[test]
    fn getter_method_kind_collision_reports_despite_local_member() {
        let rep = report(
            "class int {} abstract class A { int get m; } abstract class B { void m(); } \
             class C implements A, B {} \
             class D implements A, B { int get m => 1; } \
             class E implements A, B { void m() {} }",
        );
        assert_eq!(
            code_names(&rep),
            [
                "inconsistent_inheritance_getter_and_method",
                "inconsistent_inheritance_getter_and_method",
                "inconsistent_inheritance_getter_and_method",
            ],
            "{:?}",
            code_names(&rep)
        );
        assert_eq!(
            rep.diagnostics[0].message,
            "'m' is inherited as a getter (from 'A') and also a method (from 'B')."
        );
    }

    /// The same-kind signature conflict reports the candidates in face
    /// order with the name-level signatures; a class-local member whose
    /// signature validly overrides every candidate resolves it (the
    /// official `map.containsKey` gate).
    #[test]
    fn candidates_conflict_and_its_local_resolution() {
        let rep = report(
            "class int {} class A { void m({int? a}) {} } class B { void m({int? b}) {} } \
             class C extends A implements B {}",
        );
        assert_eq!(
            code_names(&rep),
            ["inconsistent_inheritance"],
            "{:?}",
            code_names(&rep)
        );
        assert_eq!(
            rep.diagnostics[0].message,
            "Superinterfaces don't have a valid override for 'm': \
             A.m (void Function({int? a})), B.m (void Function({int? b}))."
        );
        // The resolutions split (oracle-verified): the widened signature
        // validly overrides both candidates, while `E.m` still rejects
        // `B`'s `b:` calls — the per-member `invalid_override` reports
        // (independent of the conflict row `C` carries above).
        let rep = report(
            "class int {} class A { void m({int? a}) {} } class B { void m({int? b}) {} } \
             class D extends A implements B { void m({int? a, int? b}) {} } \
             class E extends A implements B { void m({int? a}) {} }",
        );
        assert_eq!(
            code_names(&rep),
            ["invalid_override"],
            "{:?} {:#?}",
            code_names(&rep),
            rep.diagnostics
        );
        assert_eq!(
            rep.diagnostics[0].message,
            "'E.m' ('void Function({int? a})') isn't a valid override of 'B.m' ('void Function({int? b})')."
        );
    }

    /// A `with` mixin's own member replaces the accumulated candidates (the
    /// official application sequence): a kind mismatch in the replacement
    /// is itself the getter/method conflict, a same-kind replacement is
    /// clean, and a stacked application whose member validly overrides
    /// every side resolves the whole name.
    #[test]
    fn mixin_replacement_resolves_and_conflicts() {
        // Kind mismatch through the replacement: the getter survived on the
        // superclass face, the mixin's own method replaces it.
        let rep = report(
            "class int {} abstract class A { int get m; } mixin M { void m() {} } class C extends A with M {}",
        );
        assert_eq!(
            code_names(&rep),
            ["inconsistent_inheritance_getter_and_method"],
            "{:?}",
            code_names(&rep)
        );
        assert_eq!(
            rep.diagnostics[0].message,
            "'m' is inherited as a getter (from 'A') and also a method (from 'M')."
        );
        // Same-kind replacement: clean.
        let rep = report(
            "abstract class B { void m(); } mixin M { void m() {} } class C extends B with M {}",
        );
        assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
        // The stacked resolution: the mixin's widened member validly
        // overrides both the replaced super candidate and the interface
        // candidate (oracle-verified — no row).
        let rep = report(
            "class int {} class A { void m({int? a}) {} } class B { void m({int? b}) {} } \
             mixin M { void m({int? a, int? b}) {} } class C extends A with M implements B {}",
        );
        assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
        // Without the mixin's member the conflict surfaces.
        let rep = report(
            "class int {} class A { void m({int? a}) {} } class B { void m({int? b}) {} } \
             mixin N { void other() {} } class C extends A with N implements B {}",
        );
        assert_eq!(code_names(&rep), ["inconsistent_inheritance"]);
    }

    /// A mixin declaration participates in the conflicts through its `on`
    /// constraints (the completeness loop skips mixins entirely).
    #[test]
    fn mixin_on_clause_conflicts_but_mixins_never_miss() {
        let rep = report(
            "class int {} mixin A { void m({int? a}) {} } mixin B { void m({int? b}) {} } mixin C on A, B {}",
        );
        assert_eq!(
            code_names(&rep),
            ["inconsistent_inheritance"],
            "{:?}",
            code_names(&rep)
        );
        // Mixins take no part in the abstract-completeness loop.
        let rep = report("class int {} mixin M { void m(); }");
        assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
    }

    /// Enums participate in the completeness check; an enum's synthesized
    /// `values`/`values=` stay exempt (the official exemption arm), while
    /// foreign abstract members report.
    #[test]
    fn enum_participates_with_values_exempt() {
        let rep =
            report("abstract class A { void a(); void b(); } enum E implements A { v1, v2; }");
        assert_eq!(
            code_names(&rep),
            ["non_abstract_class_inherits_abstract_member"],
            "{:?}",
            code_names(&rep)
        );
        assert_eq!(
            rep.diagnostics[0].message,
            "Missing concrete implementations of 'A.a' and 'A.b'."
        );
        // The synthesized `values` getter satisfies the interface name.
        let rep =
            report("class int {} abstract class A { int get values; } enum E implements A { v }");
        assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
    }

    /// An abstract field's non-final shape misses both halves (getter and
    /// setter, the setter displayed without the `=`); a final field misses
    /// the getter only. A concrete field implements both.
    #[test]
    fn abstract_field_halves_split_by_final() {
        let rep = report(
            "class int {} abstract class A { abstract int x; abstract final int y; } class B extends A {}",
        );
        assert_eq!(
            code_names(&rep),
            ["non_abstract_class_inherits_abstract_member"],
            "{:?}",
            code_names(&rep)
        );
        assert_eq!(
            rep.diagnostics[0].message,
            "Missing concrete implementations of 'getter A.x', 'getter A.y', and 'setter A.x'."
        );
        let rep =
            report("class int {} class A { abstract int x; } class B extends A { int x = 0; }");
        assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
    }

    /// A private name inherited from another library can never be
    /// implemented — the oracle skips it (`isAccessibleFor`); the public
    /// sibling still reports.
    #[test]
    fn cross_library_private_names_are_inaccessible() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let file_a = files.intern(Path::new("a.dart"));
        let file_m = files.intern(Path::new("main.dart"));
        let source_a = "abstract class A { void _priv(); void pub(); }";
        let source_m = "import 'a.dart'; class B extends A {}";
        let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
            LanguageVersion::new(3, 15),
        ));
        let scanned_a = dartq_scanner::scan(
            source_a,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file_a,
        );
        let scanned_m = dartq_scanner::scan(
            source_m,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file_m,
        );
        let elements_a = collect(source_a, &scanned_a.tokens, features, &strings);
        let elements_m = collect(source_m, &scanned_m.tokens, features, &strings);
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib_a = builder.intern("package:t/a.dart");
        let lib_m = builder.intern("package:t/main.dart");
        let mut index = ElementIndex::default();
        index.insert(file_a, elements_a.clone());
        index.insert(file_m, elements_m.clone());
        for (name, _) in elements_a.declarations() {
            builder.declare(lib_a, name, file_a);
        }
        for (name, decl) in elements_m.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib_m, name, file_m);
        }
        builder.import(
            lib_m,
            ImportEdge {
                target: Some(lib_a),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let rep = diagnose(
            source_m,
            &scanned_m.tokens,
            features,
            &strings,
            file_m,
            lib_m,
            &graph,
            &namespaces,
            &index,
        );
        assert_eq!(
            code_names(&rep),
            ["non_abstract_class_inherits_abstract_member"],
            "{:?}",
            code_names(&rep)
        );
        assert_eq!(
            rep.diagnostics[0].message,
            "Missing concrete implementation of 'A.pub'."
        );
    }

    /// Empty clause lists and faceless declarations are silent; a named
    /// mixin application participates like any concrete class.
    #[test]
    fn empty_clauses_are_silent_and_mixin_applications_participate() {
        let rep = report("class C {} mixin M {} enum E { v }");
        assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
        // The application resolves the member through the mixin.
        let rep =
            report("abstract class A { void m(); } mixin M { void m() {} } class C = A with M;");
        assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
        // Without the mixin's member the application misses.
        let rep = report(
            "abstract class A { void m(); } mixin N { void other() {} } class C = A with N;",
        );
        assert_eq!(
            code_names(&rep),
            ["non_abstract_class_inherits_abstract_member"],
            "{:?}",
            code_names(&rep)
        );
        assert_eq!(
            rep.diagnostics[0].message,
            "Missing concrete implementation of 'A.m'."
        );
        // An abstract named mixin application exempts itself.
        let rep = report(
            "abstract class A { void m(); } mixin N { void other() {} } abstract class C = A with N;",
        );
        assert!(code_names(&rep).is_empty(), "{:?}", code_names(&rep));
    }

    // —— #449: the flow-analysis mount (dead_code first milestone) ————————

    /// UTF-16 offsets of the report's `dead_code` rows.
    fn dead_code_offsets(report: &FileReport) -> Vec<u32> {
        report
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "dead_code")
            .map(|d| d.range.offset)
            .collect()
    }

    fn offset_of(source: &str, needle: &str) -> u32 {
        source.find(needle).expect("needle present") as u32
    }

    /// A statement after `return` reports `dead_code` (a static warning) at
    /// the dead statement's anchor, with the verbatim oracle texts.
    #[test]
    fn dead_code_after_return_emits() {
        let source = "void f() { return; 1; }";
        let report = report(source);
        assert_eq!(code_names(&report), ["dead_code"]);
        let d = &report.diagnostics[0];
        assert_eq!(d.severity, Severity::Warning);
        assert_eq!(d.message, "Dead code.");
        assert_eq!(
            d.correction.as_deref(),
            Some("Try removing the code, or fixing the code before it so that it can be reached.")
        );
        assert_eq!(dead_code_offsets(&report), [offset_of(source, "1;")]);
    }

    /// Each statement entered while unreachable reports its own anchor: the
    /// fact layer's per-statement contract (the oracle folds each dead
    /// region into one diagnostic — see the module docs).
    #[test]
    fn dead_code_reports_per_statement() {
        let source = "void f() { return; 1; 2; }";
        let report = report(source);
        assert_eq!(code_names(&report), ["dead_code", "dead_code"]);
        assert_eq!(
            dead_code_offsets(&report),
            [offset_of(source, "1;"), offset_of(source, "2;")]
        );
    }

    /// A dead local declaration reports at the declared name (the fact
    /// layer's declaration anchor), not the declaration's head token.
    #[test]
    fn dead_local_declaration_emits_at_its_name() {
        let source = "void f() { return; var x = 1; }";
        let report = report(source);
        // The dead declaration's `x` is also never read (the official
        // read-only use rule); both rows anchor at `x`, dead_code first.
        assert_eq!(code_names(&report), ["dead_code", "unused_local_variable"]);
        assert_eq!(dead_code_offsets(&report), [offset_of(source, "x")]);
    }

    /// A closure body's own dead code reports inside the closure; the
    /// enclosing function's tail stays reachable — the facts are
    /// per-executable, so the closure's `return` cannot kill the outer
    /// statements after it.
    #[test]
    fn closure_return_does_not_kill_the_enclosing_tail() {
        let source = "void f() { var h = () { return; 1; }; 2; }";
        let report = report(source);
        // `h` is never read (the official read-only use rule), before the
        // dead-statement row inside the closure.
        assert_eq!(
            code_names(&report),
            ["unused_local_variable", "dead_code"]
        );
        assert_eq!(dead_code_offsets(&report), [offset_of(source, "1;")]);
    }

    /// Code after a `return` inside a branch stays live on the other path:
    /// no over-report on partially dead control flow.
    #[test]
    fn partial_dead_branch_does_not_report() {
        let report = report("void f(c) { if (c) { return; } 1; }");
        assert!(code_names(&report).is_empty(), "{:?}", code_names(&report));
    }

    // —— #491 S1: header-window attribution —————————————————————————————

    /// The audit #489 pin: `extends A, B` reports only the parser's
    /// `multiple_extends` (not in this pass's report); `B` is residue, so
    /// `C.f` never checks against it and no dartq-only `invalid_override`
    /// fires (`ast_builder.dart:3972-3982` throws the overflow away).
    ///
    /// Hand-trace: `B` reaches the buffer after `A` latched the superclass;
    /// `handle_implements(None, 0)` claims nothing, so `B` lands in
    /// `header_residue` — `check_override`'s face list never sees it.
    #[test]
    fn extends_overflow_is_never_a_super_face() {
        let report = report(
            "class int {} class A {} class B { void f(int x) {} } \
             class C extends A, B { void f() {} }",
        );
        assert!(code_names(&report).is_empty(), "{:?}", code_names(&report));
    }

    /// A named mixin application's `implements` names wait for the parser's
    /// real `handle_implements(Some, count)` emission (ticket #460, parser
    /// lane): until it lands they are residue and the interface edge
    /// under-reports. Kept as the cross-lane pin — expected red until #460
    /// lands, never worked around here.
    #[test]
    fn named_mixin_application_implements_capture() {
        let strings = StringTable::new();
        let source = "class S {} mixin M {} class I {} class C = S with M implements I;";
        let file = FileTable::new().intern(Path::new("nma.dart"));
        let scanned = dartq_scanner::scan(
            source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file,
        );
        assert!(scanned.diagnostics.is_empty());
        let elements = collect(
            source,
            &scanned.tokens,
            ParserFeatures::from_feature_set(FeatureSet::for_language_version(
                LanguageVersion::new(3, 15),
            )),
            &strings,
        );
        let intern = |text: &str| strings.intern(text);
        let c = elements.container(intern("C")).expect("container C");
        assert_eq!(
            c.implements_clause,
            vec![intern("I")],
            "the interface edge must attribute to the implements clause"
        );
    }

    /// A three-file hop resolves the superclass name in the declaring
    /// container's library (the oracle climbs the declaring library's scope
    /// chain, not the diagnosing one): `S` lives in n2 and extends n3's `T`;
    /// the diagnosing library imports n1's conflicting `T`. The climb must
    /// reach n3's `T.m(int)` — arity-broken against `C.m()` — not n1's
    /// compatible `T.m()`.
    ///
    /// Hand-trace: hop 1 `S` resolves through main's import face to n2's
    /// file; hop 2 probes the library whose locals declare `T` in n2's file —
    /// n3's — and resolves its `T` through n3's namespace.
    #[test]
    fn superclass_hop_resolves_in_the_declaring_library() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
            LanguageVersion::new(3, 15),
        ));
        let sources = [
            ("n1.dart", "class T { void m() {} }"),
            ("n3.dart", "class int {} class T { void m(int x) {} }"),
            ("n2.dart", "import 'n3.dart'; class S extends T {}"),
            (
                "main.dart",
                "import 'n1.dart'; import 'n2.dart'; class C extends S { void m() {} }",
            ),
        ];
        let mut index = ElementIndex::default();
        let mut tokens_by_file = Vec::new();
        for (path, source) in sources {
            let file = files.intern(Path::new(path));
            let scanned = dartq_scanner::scan(
                source,
                ScanConfig {
                    enable_triple_shift: true,
                    enable_augmentations: true,
                },
                file,
            );
            assert!(scanned.diagnostics.is_empty(), "{path}");
            let elements = collect(source, &scanned.tokens, features, &strings);
            index.insert(file, elements);
            tokens_by_file.push(scanned.tokens);
        }
        let mut builder = LibraryGraphBuilder::new(&strings);
        let n1 = builder.intern("package:t/n1.dart");
        let n3 = builder.intern("package:t/n3.dart");
        let n2 = builder.intern("package:t/n2.dart");
        let main = builder.intern("package:t/main.dart");
        for (at, (path, _source)) in sources.iter().enumerate() {
            let lib = match at {
                0 => n1,
                1 => n3,
                2 => n2,
                _ => main,
            };
            let file = files.intern(Path::new(path));
            let elements = collect(sources[at].1, &tokens_by_file[at], features, &strings);
            for (name, decl) in elements.declarations() {
                if decl.kind == MicroKind::TypeParam {
                    continue;
                }
                builder.declare(lib, name, file);
            }
        }
        builder.import(
            n2,
            ImportEdge {
                target: Some(n3),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        builder.import(
            main,
            ImportEdge {
                target: Some(n1),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        builder.import(
            main,
            ImportEdge {
                target: Some(n2),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let main_file = files.intern(Path::new("main.dart"));
        let report = diagnose(
            sources[3].1,
            &tokens_by_file[3],
            features,
            &strings,
            main_file,
            main,
            &graph,
            &namespaces,
            &index,
        );
        // The `n1` import contributes nothing `main` references (`T` is
        // only used inside `n2` through its own import): the official
        // `unused_import` fires at its URI, before the override row.
        assert_eq!(
            code_names(&report),
            ["unused_import", "invalid_override"],
            "{:?}",
            code_names(&report)
        );
    }

    /// One applicable extension defines the access; the same-named member of
    /// a *non-applicable* extension does not (the applicability filter —
    /// `applicable_extensions.dart:157-171` gates on the receiver type, so
    /// an extension `on` another type never suppresses an `undefined_*`).
    ///
    /// Hand-trace: `E.on_type == "int"` never matches the resolved `C`
    /// receiver; the flat-pool predecessor suppressed this report.
    #[test]
    fn inapplicable_extension_member_reports_undefined() {
        let report = report(
            "class int {} class C {} extension E on int { void m() {} } \
             void main() { C().m(); }",
        );
        assert_eq!(
            code_names(&report),
            ["undefined_method"],
            "{:?}",
            code_names(&report)
        );
    }

    /// A single applicable extension defines the access silently.
    #[test]
    fn single_applicable_extension_defines_the_access() {
        let report = report(
            "class C {} extension E on C { void m() {} } \
             void main() { C().m(); C().m2; }",
        );
        assert_eq!(
            code_names(&report),
            ["undefined_getter"],
            "{:?}",
            code_names(&report)
        );
    }

    /// Two applicable extensions both declaring the member report
    /// `ambiguous_extension_member_access` once (the two-candidate message
    /// text; `diagnostic.g.dart:363-374`) — and the access counts as
    /// resolved, so no `undefined_method` stacks on top.
    ///
    /// Hand-trace: both faces carry `on_type == "C"`; `_isMoreSpecific`
    /// cannot separate same-`on`-type candidates at the M1 name level.
    #[test]
    fn two_applicable_extensions_report_the_ambiguity() {
        let report = report(
            "class C {} extension E1 on C { void m() {} } \
             extension E2 on C { void m() {} } \
             void main() { C().m(); }",
        );
        assert_eq!(
            code_names(&report),
            ["ambiguous_extension_member_access"],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "A member named 'm' is defined in 'extension E1 on C' and 'extension E2 on C', and neither is more specific."
        );
    }

    /// Three or more candidates take the joined-list variant with the
    /// `commaSeparatedWithAnd` rendering
    /// (`utilities/extensions/string.dart:41-104`: Oxford 'and'), including
    /// the unnamed-extension display.
    #[test]
    fn three_applicable_extensions_use_the_list_variant() {
        let report = report(
            "class C {} extension E1 on C { void m() {} } \
             extension E2 on C { void m() {} } \
             extension on C { void m() {} } \
             void main() { C().m(); }",
        );
        assert_eq!(code_names(&report), ["ambiguous_extension_member_access"]);
        assert_eq!(
            report.diagnostics[0].message,
            "A member named 'm' is defined in extension 'E1', extension 'E2', and unnamed extension on 'C', and none are more specific."
        );
    }

    /// The on-type's own member wins: interface members precede extension
    /// application (`findExtension` runs only after the interface lookup
    /// misses), so no ambiguity fires against an extension on the same type.
    #[test]
    fn own_member_shadows_the_applicable_extensions() {
        let report = report(
            "class int {} class String {} class C { void m() {} } \
             extension E1 on C { void m(int x) {} } \
             extension E2 on C { void m(String x) {} } \
             void main() { C().m(); }",
        );
        assert!(code_names(&report).is_empty(), "{:?}", code_names(&report));
    }

    /// An unresolved `on` type suppresses the face from application
    /// entirely: `extension E on Undefined` never gates an `undefined_*`
    /// (the oracle's `extendedType` fails to resolve), while the type
    /// reference itself reports.
    #[test]
    fn unresolved_on_type_suppresses_the_face() {
        let report = report(
            "extension E on Undefined { void m() {} } \
             class C {} void main() { C().m(); }",
        );
        assert_eq!(
            code_names(&report),
            ["undefined_class", "undefined_method"],
            "{:?}",
            code_names(&report)
        );
    }

    /// A setter-only extension gates a write through the setter spelling
    /// (`havingMemberWithBaseName` couples the field's getter/setter pair;
    /// `applicable_extensions.dart:104-115`).
    #[test]
    fn setter_only_extension_defines_the_write() {
        let report = report(
            "class int {} class C {} extension E on C { set s(int v) {} } \
             void main() { C().s = 1; }",
        );
        assert!(code_names(&report).is_empty(), "{:?}", code_names(&report));
    }

    /// Extension-type members are not implicit-application candidates: the
    /// face set holds plain extensions only, so an unrelated class's access
    /// reports (the old merged pool suppressed it), while the extension
    /// type's own receiver still resolves through its container face.
    #[test]
    fn extension_type_members_stay_out_of_the_faces() {
        let miss = report(
            "class int {} extension type T(int x) { void m() {} } \
             class C {} void main() { C().m(); }",
        );
        assert_eq!(
            code_names(&miss),
            ["undefined_method"],
            "{:?}",
            code_names(&miss)
        );
        let hit = report(
            "class int {} extension type T(int x) { void m() {} } \
             void main() { T(1).m(); }",
        );
        assert!(code_names(&hit).is_empty(), "{:?}", code_names(&hit));
    }

    // —— #476 item 2: extension-type representation ———————————————————————

    /// The representation field sharing the extension type's name reports
    /// `member_with_class_name` at the field's name
    /// (`error_verifier.dart:5411-5413`).
    #[test]
    fn representation_named_like_the_type_reports() {
        let report = report("class int {} extension type E(int E) {}");
        assert_eq!(
            code_names(&report),
            ["member_with_class_name"],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "A class member can't have the same name as the enclosing class."
        );
    }

    /// The representation field named like an `Object` member reports
    /// `extension_type_declares_member_of_object`
    /// (`error_verifier.dart:5416-5423`).
    #[test]
    fn representation_named_like_an_object_member_reports() {
        let report = report("class int {} extension type E(int hashCode) {}");
        assert_eq!(
            code_names(&report),
            ["extension_type_declares_member_of_object"],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "Extension types can't declare members with the same name as a member declared by 'Object'."
        );
    }

    /// A representation field typed with the extension type itself reports
    /// `extension_type_representation_depends_on_itself` at the type's name
    /// (`error_verifier.dart:5343-5354`; the M1 name-level approximation
    /// covers direct references — `List<E>` shapes stay an under-report).
    #[test]
    fn self_referential_representation_reports() {
        let report = report("extension type E(E x) {}");
        assert_eq!(
            code_names(&report),
            ["extension_type_representation_depends_on_itself"],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The extension type representation can't depend on itself."
        );
    }

    /// A clean representation emits nothing from the semantic side, and the
    /// 3.15 language version runs the official primary-constructors-enabled
    /// arm (`enabled_in: 3.13`), where the typeless-representation row is
    /// deliberately absent (`error_verifier.dart:5425` feature branch).
    #[test]
    fn clean_representation_stays_silent() {
        let clean = report("class int {} extension type E(int x) {}");
        assert!(code_names(&clean).is_empty(), "{:?}", code_names(&clean));
        let typeless = report("class int {} extension type E(it) {}");
        assert!(
            code_names(&typeless).is_empty(),
            "{:?}",
            code_names(&typeless)
        );
    }

    // —— #476 item 3: the mixin application chain —————————————————————————

    /// A mixin-only member resolves through the stacked application:
    /// `C().m()` finds `M1.m` even though the superclass chain lacks it (the
    /// flat predecessor walked the superclass only and reported).
    #[test]
    fn mixin_application_member_resolves() {
        let report = report(
            "class B {} mixin M1 { void m() {} } \
             class C extends B with M1 {} \
             void main() { C().m(); }",
        );
        assert!(code_names(&report).is_empty(), "{:?}", code_names(&report));
    }

    /// Later applications win: with `M1.m: int` and `M2.m: String` fields,
    /// the `m` read resolves M2's `String` — proven by the return-type
    /// mismatch against an `int` host (`inheritance_manager3.dart:571-579`:
    /// each mixin replaces the members of the interface).
    #[test]
    fn later_mixin_application_wins() {
        let report = report(
            "class B {} class int {} class String {} \
             mixin M1 { int m = 1; } \
             mixin M2 { String m = ''; } \
             class C extends B with M1, M2 { int f() { return m; } }",
        );
        assert_eq!(
            code_names(&report),
            ["return_of_invalid_type"],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "A value of type 'String' can't be returned from the method 'f' because it has a return type of 'int'."
        );
    }

    /// With the later mixin lacking the member, the earlier one's surfaces
    /// through the application chain (`M1` over `S`) — the read resolves
    /// M1's `int` field, so the `int` host stays clean.
    #[test]
    fn earlier_mixin_member_surfaces_through_the_chain() {
        let report = report(
            "class B {} class int {} class String {} \
             mixin M1 { int m = 1; } \
             mixin M2 { void other() {} } \
             class C extends B with M1, M2 { int f() { return m; } }",
        );
        assert!(code_names(&report).is_empty(), "{:?}", code_names(&report));
    }

    /// `super.member` walks the applied chain too: `super.h()` in
    /// `class C extends B with M1` resolves M1's `h` (the official
    /// superclass of `C` is the application `B&M1`,
    /// `inheritance_override.dart:344-351`).
    #[test]
    fn super_walk_sees_the_stacked_chain() {
        let report = report(
            "class B {} mixin M1 { void h() {} } \
             class C extends B with M1 { void f() { super.h(); } }",
        );
        assert!(code_names(&report).is_empty(), "{:?}", code_names(&report));
    }

    /// The override check sees the stacked winner: `C.m()` is arity-broken
    /// against `M2.m(int)` (the interface winner per
    /// `inheritance_manager3.dart:571-579`) — reported once for the stacked
    /// superclass face (the same inherited entry, pointer-deduped). The
    /// separate `B.m(int)` and `M2.m(int)` face entries are distinct
    /// declarations and keep the oracle's per-superinterface loop shape
    /// (#453), so the total stays at the incumbent two; `M1`'s own `m()` is
    /// a valid override and never reports.
    #[test]
    fn override_check_reports_the_stacked_winner() {
        let report = report(
            "class int {} class B { void m(int x) {} } \
             mixin M1 { void m() {} } \
             mixin M2 { void m(int x) {} } \
             class C extends B with M1, M2 { void m() {} }",
        );
        assert_eq!(
            code_names(&report),
            ["invalid_override", "invalid_override"],
            "{:?}",
            code_names(&report)
        );
        // The stacked face's report names the interface winner.
        let stacked = report
            .diagnostics
            .iter()
            .find(|d| d.message.contains("M2.m"))
            .expect("the stacked winner's report");
        assert!(
            stacked
                .message
                .starts_with("'C.m' ('void Function()') isn't a valid override of 'M2.m'"),
            "{}",
            stacked.message
        );
    }
    // —— #504: the usage cluster ————————————————————————————————————————

    /// An unread local reports; a plain read spares; a write alone never
    /// uses; the useless-statement reads (`!v;`, `v++;`) never count.
    ///
    /// Hand-trace: `dead` has no hit; `live`'s argument read counts; `gone`
    /// takes only the write (the local hit cancels at the assignment);
    /// `discarded`'s two reads both land in statement windows whose parent
    /// shape is a unary form (`_isUsefulRead`).
    #[test]
    fn unused_local_variable_reports_and_reads_spare() {
        let r = report(
            "class int {} void take(int x) {} \
             void main() { int dead = 0; int live = 0; take(live); \
             int gone = 0; gone = 1; int discarded = 0; !discarded; discarded++; }",
        );
        assert_eq!(
            code_names(&r),
            [
                "unused_local_variable",
                "unused_local_variable",
                "unused_local_variable"
            ],
            "{:?}",
            code_names(&r)
        );
        assert_eq!(
            r.diagnostics[0].message,
            "The value of the local variable 'dead' isn't used."
        );
    }

    /// `v;` and `v + 1;` are useful reads (the official `_isUsefulRead`
    /// spares everything but the unary statement shapes).
    #[test]
    fn plain_and_operator_reads_spare_the_local() {
        // Both reads are useful per the official `_isUsefulRead` (a plain
        // statement expression and a binary-operator operand), so the
        // local never reports `unused_local_variable`; `class int {}`
        // declares no `+`, so the operator read reports the oracle's
        // `undefined_operator` at the site.
        let r = report("class int {} void main() { int v = 0; v; v + 1; }");
        assert_eq!(code_names(&r), ["undefined_operator"], "{:?}", code_names(&r));
    }

    /// The wildcard-variables face: an all-underscores name never reports;
    /// a mixed name still does.
    #[test]
    fn wildcard_locals_never_report() {
        let r = report("class int {} void main() { var _ = 1; int _z = 2; }");
        assert_eq!(code_names(&r), ["unused_local_variable"], "{:?}", code_names(&r));
        assert!(
            r.diagnostics[0].message.contains("'_z'"),
            "{}",
            r.diagnostics[0].message
        );
    }

    /// A pattern-variable-declaration group reports all-or-nothing: one
    /// used member spares the rest (official `visitPatternVariableDeclaration`).
    #[test]
    fn pattern_groups_report_all_or_nothing() {
        let r = report("void main() { var (a, b) = (1, 2); }");
        assert_eq!(
            code_names(&r),
            ["unused_local_variable", "unused_local_variable"],
            "{:?}",
            code_names(&r)
        );
        // The local `int` stands in for the core type (the harness has no
        // seeded dart:core); the read of `a` spares the whole pattern group
        // (official visitPatternVariableDeclaration all-or-nothing).
        let r2 = report("class int {} void take(int x) {} void main() { var (a, b) = (1, 2); take(a); }");
        assert!(code_names(&r2).is_empty(), "{:?}", code_names(&r2));
    }

    /// An unreferenced local function reports `unused_element`; its own
    /// self-reference never counts (`_enclosingExec`), but an outer
    /// invocation does.
    #[test]
    fn unused_local_function_reports_unused_element() {
        let r = report("void main() { void inner() {} }");
        assert_eq!(code_names(&r), ["unused_element"], "{:?}", code_names(&r));
        assert_eq!(
            r.diagnostics[0].message,
            "The declaration 'inner' isn't referenced."
        );
        let r2 = report("void main() { void inner() {} inner(); }");
        assert!(code_names(&r2).is_empty(), "{:?}", code_names(&r2));
    }

    /// Private top-level functions and classes report when nothing outside
    /// themselves references them; public names never do.
    #[test]
    fn unused_private_library_declarations_report() {
        let r = report("void _dead() {} class _Gone {} void main() {}");
        assert_eq!(
            code_names(&r),
            ["unused_element", "unused_element"],
            "{:?}",
            code_names(&r)
        );
        let r2 = report("void _live() {} class _Kept {} void main() { _live(); _Kept(); }");
        assert!(code_names(&r2).is_empty(), "{:?}", code_names(&r2));
    }

    /// The `unused_field` rule reads only: a write never spares, a read
    /// anywhere in the library does (the implicit-getter face).
    #[test]
    fn unused_field_reads_only() {
        let r = report(
            "class int {} class _C { int _read = 0; int _written = 0; int _dead = 0; int live = 0; \
             int f() => _read; int g() => live; void w() { _written = 1; } } \
             void main() { _C().f(); _C().g(); }",
        );
        assert_eq!(
            code_names(&r),
            ["unused_field", "unused_field"],
            "{:?}",
            code_names(&r)
        );
        assert_eq!(
            r.diagnostics[0].message,
            "The value of the field '_written' isn't used."
        );
        assert_eq!(
            r.diagnostics[1].message,
            "The value of the field '_dead' isn't used."
        );
    }

    /// Optional parameters of private executables report when no body use
    /// resolves to them and no invocation supplies them: positionally by
    /// given-count, named by name.
    #[test]
    fn unused_optional_parameters_report() {
        let r = report(
            "class int {} class _C { void _m(int a, [int b = 0]) {} void _n(int a, {int c = 0}) {} } \
             void main() { _C()._m(1); _C()._n(1); }",
        );
        assert_eq!(
            code_names(&r),
            ["unused_element_parameter", "unused_element_parameter"],
            "{:?}",
            code_names(&r)
        );
        assert_eq!(
            r.diagnostics[0].message,
            "A value for optional parameter 'b' isn't ever given."
        );
        assert_eq!(
            r.diagnostics[1].message,
            "A value for optional parameter 'c' isn't ever given."
        );
        let r2 = report(
            "class int {} class _C { void _m(int a, [int b = 0]) {} } void main() { _C()._m(1, 2); }",
        );
        assert!(code_names(&r2).is_empty(), "{:?}", code_names(&r2));
        let r3 = report(
            "class int {} class _C { void _m(int a, [int b = 0]) { b; } } void main() { _C()._m(1); }",
        );
        assert!(code_names(&r3).is_empty(), "{:?}", code_names(&r3));
    }

    /// A named constructor's optional parameter reports when never given
    /// (the class keeps more than one constructor); the invocation spares
    /// the constructor itself.
    #[test]
    fn unused_named_constructor_parameter_reports() {
        let r = report(
            "class int {} class _D { _D.named([int x = 0]); _D.other(); } \
             void main() { _D.named(); _D.other(); }",
        );
        assert_eq!(
            code_names(&r),
            ["unused_element_parameter"],
            "{:?}",
            code_names(&r)
        );
        assert_eq!(
            r.diagnostics[0].message,
            "A value for optional parameter 'x' isn't ever given."
        );
    }

    // —— #505: the import/flow cluster ———————————————————————————————————

    /// An import that contributed nothing reports at its URI; the same
    /// import with a used prefix stays silent.
    #[test]
    fn unused_import_reports_at_the_uri() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let file = files.intern(Path::new("main.dart"));
        let source = "import 'other.dart'; void main() {}";
        let scanned = dartq_scanner::scan(
            source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file,
        );
        assert!(scanned.diagnostics.is_empty());
        let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
            LanguageVersion::new(3, 15),
        ));
        let elements = collect(source, &scanned.tokens, features, &strings);
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib = builder.intern("package:t/main.dart");
        let other = builder.intern("package:t/other.dart");
        builder.declare(other, strings.intern("thing"), file);
        builder.import(
            lib,
            ImportEdge {
                target: Some(other),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let mut index = ElementIndex::default();
        index.insert(file, elements.clone());
        let r = diagnose(
            source,
            &scanned.tokens,
            features,
            &strings,
            file,
            lib,
            &graph,
            &namespaces,
            &index,
        );
        assert_eq!(code_names(&r), ["unused_import"], "{:?}", code_names(&r));
        assert_eq!(r.diagnostics[0].message, "Unused import: 'other.dart'.");
    }

    #[test]
    fn used_prefix_import_stays_silent() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let file = files.intern(Path::new("main.dart"));
        let source = "import 'other.dart' as p; void main() { p.thing; }";
        let scanned = dartq_scanner::scan(
            source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file,
        );
        assert!(scanned.diagnostics.is_empty());
        let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
            LanguageVersion::new(3, 15),
        ));
        let elements = collect(source, &scanned.tokens, features, &strings);
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib = builder.intern("package:t/main.dart");
        let other = builder.intern("package:t/other.dart");
        builder.declare(other, strings.intern("thing"), file);
        builder.import(
            lib,
            ImportEdge {
                target: Some(other),
                prefix: Some(strings.intern("p")),
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let mut index = ElementIndex::default();
        index.insert(file, elements.clone());
        let r = diagnose(
            source,
            &scanned.tokens,
            features,
            &strings,
            file,
            lib,
            &graph,
            &namespaces,
            &index,
        );
        assert!(
            !code_names(&r).contains(&"unused_import"),
            "{:?}",
            code_names(&r)
        );
    }

    /// The catch-variable routing: `on T catch (e)` without a stack trace
    /// reports the clause; a stack trace variable always reports when
    /// unread; a plain `catch (e)` auto-uses its exception.
    #[test]
    fn unused_catch_variables_route_by_kind() {
        let r = report(
            "class int {} void main() { try {} on int catch (e) {} try {} catch (e2, st) {} }",
        );
        assert_eq!(
            code_names(&r),
            ["unused_catch_clause", "unused_catch_stack"],
            "{:?}",
            code_names(&r)
        );
        assert_eq!(
            r.diagnostics[0].message,
            "The exception variable 'e' isn't used, so the 'catch' clause can be removed."
        );
        assert_eq!(
            r.diagnostics[1].message,
            "The stack trace variable 'st' isn't used and can be removed."
        );
    }

    /// A catch-all clause followed by more clauses makes the tail dead
    /// (`_CatchClausesVerifier.nextCatchClause`); the span covers the
    /// following clauses.
    #[test]
    fn dead_catch_tail_reports() {
        let r = report(
            "class int {} class Object {} void main() { try {} on Object catch (e) {} on int {} }",
        );
        assert_eq!(
            code_names(&r),
            ["unused_catch_clause", "dead_code_catch_following_catch"],
            "{:?}",
            code_names(&r)
        );
    }

    /// A `??` whose left operand is strictly non-nullable anchors the
    /// report at the right operand; `??=` anchors at the value. Both reads
    /// count (the assignment target never loses its use).
    #[test]
    fn dead_null_aware_right_operands_report() {
        let r = report("class int {} void main() { int x = 1; int y = 2; x ?? y; y ??= x; }");
        assert_eq!(
            code_names(&r),
            ["dead_null_aware_expression", "dead_null_aware_expression"],
            "{:?}",
            code_names(&r)
        );
    }

    /// Labels declare through the labeled-statement scope chain: a
    /// `break` into an outer scope spares that label, the unreferenced one
    /// reports at its own statement end.
    #[test]
    fn unused_labels_report() {
        let r = report(
            "void main() { used: for (;;) { break used; } dead: for (;;) { break used; } }",
        );
        assert_eq!(code_names(&r), ["unused_label"], "{:?}", code_names(&r));
        assert_eq!(r.diagnostics[0].message, "The label 'dead' isn't used.");
    }
}
