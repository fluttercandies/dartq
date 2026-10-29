//! The run entry: one traversal over the arena dispatching enabled rules,
//! suppression applied at the reporting face.
//!
//! Traversal is an explicit-stack DFS (no structural recursion — the engine's
//! termination invariant holds on any input), parents before children, in
//! source order. Rules see the node through [`Check`], which carries the
//! ancestor path so parent questions (`!(x is T)`, else-branch shape) cost
//! one slice read, never the arena's forward-walk `parent`.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use dartq_ast::{NodeKind, PayloadField, SyntaxTable, byte_span, utf16_span};
use dartq_scanner::TokenTable;
use dartq_semantic::Analysis;
use dartq_source::LineIndex;
use rustc_hash::FxHashMap;

use crate::comments::Comments;
use crate::ignore::{IgnoreInfo, NameEntry};
use crate::registry::{EnableSet, Group, LATE_JOIN_RULES, RuleId, rule_by_name, rules};
use crate::rules::SupertypeClause;
use crate::rules::declarations::remove_deprecations_in_breaking_versions::check_annotation;
use crate::rules::expressions::same_file::Decls;

/// Narrows a source- or arena-derived count/offset to the crate's `u32`
/// span width. The parser's id/offset layout is `u32` by design and every
/// real input stays far below `u32::MAX`, so the narrowing never wraps.
#[inline]
#[expect(
    clippy::cast_possible_truncation,
    reason = "source/arena quantities are u32 by design (the parser's \
              id/offset layout); inputs are far below u32::MAX"
)]
pub(crate) fn narrow_u32(value: usize) -> u32 {
    value as u32
}

/// The language-feature bits a rule may query. Derived from the library's
/// [`dartq_language::FeatureSet`] ([`Features::from_feature_set`]) — one
/// keyed fold over the generated registry, no hand-listed yaml mirrors —
/// with the registration polarity carried by the gate table
/// ([`crate::registry::REGISTRATION_GATES`]) and the in-body exemption
/// reads below. Features the caller does not know are treated per their
/// shipped defaults; the caller pins the bits it wants to control (e.g.
/// from `// @dart=` / experiment config).
#[expect(
    clippy::struct_excessive_bools,
    reason = "one independent bit per shipped language feature, mirroring the \
              official feature table; a state-machine refactor would obscure \
              the 1:1 mapping"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Features {
    /// `wildcard_variables` (shipped in Dart 3.7): a lone `_` parameter name
    /// is exempt from `avoid_shadowing_type_parameters`
    /// (avoid_shadowing_type_parameters.dart:34-37) and from the library
    /// prefix rules (library_prefixes.dart:36-39,
    /// no_leading_underscores_for_library_prefixes.dart:46-50).
    pub wildcard_variables: bool,
    /// `enhanced_parts` (experimental, unshipped — `enhanced-parts` has no
    /// `enabledIn`, `tools/experimental_features.yaml:150-152`):
    /// `use_string_in_part_of_directives` registers only without it
    /// (use_string_in_part_of_directives.dart:32-34).
    pub enhanced_parts: bool,
    /// `unnamed_libraries` (shipped in 2.19, `experimental_features.yaml:397-400`):
    /// `unnecessary_library_name` registers only with it
    /// (unnecessary_library_name.dart:32-34).
    pub unnamed_libraries: bool,
    /// `patterns` (shipped in 3.0, `experimental_features.yaml:387-389`): the
    /// bit the official `AstBuilder` keys switch-statement member
    /// materialization on (`ast_builder.dart:1295`) — with it on, every `case`
    /// of a switch statement is a `SwitchPatternCase`, so
    /// `no_duplicate_case_values` (whose visitor processes only legacy
    /// `SwitchCase` members, `no_duplicate_case_values.dart:41`) is silent.
    pub patterns: bool,
    /// `primary_constructors` (shipped in 3.13, `experimental_features.yaml`
    /// `enabledIn: 3.13.0`): the registration face of the declaration-domain
    /// family — `empty_container_bodies`,
    /// `unnecessary_const_in_enum_constructor`,
    /// `unnecessary_type_name_in_constructor`, and `use_primary_constructors`
    /// register only with it, `var_with_no_type_annotation` only without it
    /// (each rule's upstream `isFeatureEnabled` registration bail). The
    /// grammar posture is independent: the parser's fixed 3.15 surface
    /// always accepts the syntax.
    pub primary_constructors: bool,
    /// `null_aware_elements` (shipped in 3.8, `experimental_features.yaml`
    /// `enabledIn: 3.8.0`): `use_null_aware_elements` registers only with
    /// it (`use_null_aware_elements.dart:35`).
    pub null_aware_elements: bool,
    /// `super_parameters` (shipped in 2.17, `experimental_features.yaml`
    /// `enabledIn: 2.17.0`): `use_super_parameters` registers only with it
    /// (`use_super_parameters.dart:43`).
    pub super_parameters: bool,
}

impl Features {
    /// The all-off fold every feature application starts from.
    const OFF: Self = Self {
        wildcard_variables: false,
        enhanced_parts: false,
        unnamed_libraries: false,
        patterns: false,
        primary_constructors: false,
        null_aware_elements: false,
        super_parameters: false,
    };

    /// Derives the bits from a [`dartq_language::FeatureSet`] — one pass
    /// over the generated registry (`Feature::ALL`), each enabled feature
    /// applied through [`Features::enable`], the single place a feature's
    /// linter-visible stance lives. The stance match is exhaustive over
    /// the generated enum with no wildcard arm, so a re-vendored yaml
    /// entry without a lints stance fails compilation until it gets one.
    /// Version-derived enablement comes from the registry's `enabledIn`
    /// (`FeatureSet::for_language_version`); the caller can additionally
    /// layer `--enable-experiment` opt-ins onto the set beforehand.
    #[must_use]
    pub fn from_feature_set(set: dartq_language::FeatureSet) -> Self {
        let mut folded = Self::OFF;
        for feature in dartq_language::Feature::ALL {
            if set.is_enabled(feature) {
                folded.enable(feature);
            }
        }
        folded
    }

    /// Applies one enabled feature to the fold — the linter-visible stance
    /// table, exhaustive over the generated registry enum on purpose (no
    /// wildcard arm).
    fn enable(&mut self, feature: dartq_language::Feature) {
        match feature {
            dartq_language::Feature::WildcardVariables => self.wildcard_variables = true,
            dartq_language::Feature::EnhancedParts => self.enhanced_parts = true,
            dartq_language::Feature::UnnamedLibraries => self.unnamed_libraries = true,
            dartq_language::Feature::Patterns => self.patterns = true,
            dartq_language::Feature::PrimaryConstructors => self.primary_constructors = true,
            dartq_language::Feature::NullAwareElements => self.null_aware_elements = true,
            dartq_language::Feature::SuperParameters => self.super_parameters = true,
            // No linter-visible stance: no rule registration or exemption
            // reads these entries; their parser/scanner faces live in
            // their own projections.
            dartq_language::Feature::Variance
            | dartq_language::Feature::ConstFunctions
            | dartq_language::Feature::Macros
            | dartq_language::Feature::RecordUse
            | dartq_language::Feature::DataAssets
            | dartq_language::Feature::UnquotedImports
            | dartq_language::Feature::Augmentations
            | dartq_language::Feature::InferenceUpdate4
            | dartq_language::Feature::ThisPromotion
            | dartq_language::Feature::StaticExtensions
            | dartq_language::Feature::AnonymousMethods
            | dartq_language::Feature::TestExperiment
            | dartq_language::Feature::PrivateNamedParameters
            | dartq_language::Feature::DotShorthands
            | dartq_language::Feature::NativeAssets
            | dartq_language::Feature::GetterSetterError
            | dartq_language::Feature::SoundFlowAnalysis
            | dartq_language::Feature::InferenceUsingBounds
            | dartq_language::Feature::DigitSeparators
            | dartq_language::Feature::InferenceUpdate3
            | dartq_language::Feature::InlineClass
            | dartq_language::Feature::InferenceUpdate2
            | dartq_language::Feature::SealedClass
            | dartq_language::Feature::ClassModifiers
            | dartq_language::Feature::Records
            | dartq_language::Feature::AlternativeInvalidationStrategy
            | dartq_language::Feature::InferenceUpdate1
            | dartq_language::Feature::EnhancedEnums
            | dartq_language::Feature::NamedArgumentsAnywhere
            | dartq_language::Feature::ConstructorTearoffs
            | dartq_language::Feature::GenericMetadata
            | dartq_language::Feature::TripleShift
            | dartq_language::Feature::NonfunctionTypeAliases
            | dartq_language::Feature::NonNullable
            | dartq_language::Feature::ExtensionMethods
            | dartq_language::Feature::ConstantUpdate2018
            | dartq_language::Feature::ControlFlowCollections
            | dartq_language::Feature::SetLiterals
            | dartq_language::Feature::SpreadCollections => {}
        }
    }

    /// Whether `feature` is enabled — the keyed lookup the registration
    /// gates evaluate ([`crate::registry`]'s gate table resolves every
    /// entry through this face). Exhaustive over the generated enum: a
    /// re-vendored yaml entry without a stance arm fails compilation.
    #[must_use]
    pub fn is_enabled(&self, feature: dartq_language::Feature) -> bool {
        match feature {
            dartq_language::Feature::WildcardVariables => self.wildcard_variables,
            dartq_language::Feature::EnhancedParts => self.enhanced_parts,
            dartq_language::Feature::UnnamedLibraries => self.unnamed_libraries,
            dartq_language::Feature::Patterns => self.patterns,
            dartq_language::Feature::PrimaryConstructors => self.primary_constructors,
            dartq_language::Feature::NullAwareElements => self.null_aware_elements,
            dartq_language::Feature::SuperParameters => self.super_parameters,
            // No linter-visible bit: the feature gates no rule and no
            // exemption, so its enabled answer is immaterial — `false`
            // keeps a stray `Gate::Feature` entry on it structurally
            // silent (the table's own startup arm names its rules).
            dartq_language::Feature::Variance
            | dartq_language::Feature::ConstFunctions
            | dartq_language::Feature::Macros
            | dartq_language::Feature::RecordUse
            | dartq_language::Feature::DataAssets
            | dartq_language::Feature::UnquotedImports
            | dartq_language::Feature::Augmentations
            | dartq_language::Feature::InferenceUpdate4
            | dartq_language::Feature::ThisPromotion
            | dartq_language::Feature::StaticExtensions
            | dartq_language::Feature::AnonymousMethods
            | dartq_language::Feature::TestExperiment
            | dartq_language::Feature::PrivateNamedParameters
            | dartq_language::Feature::DotShorthands
            | dartq_language::Feature::NativeAssets
            | dartq_language::Feature::GetterSetterError
            | dartq_language::Feature::SoundFlowAnalysis
            | dartq_language::Feature::InferenceUsingBounds
            | dartq_language::Feature::DigitSeparators
            | dartq_language::Feature::InferenceUpdate3
            | dartq_language::Feature::InlineClass
            | dartq_language::Feature::InferenceUpdate2
            | dartq_language::Feature::SealedClass
            | dartq_language::Feature::ClassModifiers
            | dartq_language::Feature::Records
            | dartq_language::Feature::AlternativeInvalidationStrategy
            | dartq_language::Feature::InferenceUpdate1
            | dartq_language::Feature::EnhancedEnums
            | dartq_language::Feature::NamedArgumentsAnywhere
            | dartq_language::Feature::ConstructorTearoffs
            | dartq_language::Feature::GenericMetadata
            | dartq_language::Feature::TripleShift
            | dartq_language::Feature::NonfunctionTypeAliases
            | dartq_language::Feature::NonNullable
            | dartq_language::Feature::ExtensionMethods
            | dartq_language::Feature::ConstantUpdate2018
            | dartq_language::Feature::ControlFlowCollections
            | dartq_language::Feature::SetLiterals
            | dartq_language::Feature::SpreadCollections => false,
        }
    }
}

impl Default for Features {
    /// The latest shipped posture — derived from
    /// [`dartq_language::FeatureSet::latest`], never hand-listed: every
    /// feature with `enabledIn` at or below the registry's current version
    /// folds on (the experimental entries stay off).
    fn default() -> Self {
        Self::from_feature_set(dartq_language::FeatureSet::latest())
    }
}

/// The substrate facts one run reads. All references must stay alive for the
/// whole run; the AST/token table must be the unified post-parse pair (the
/// `dartq_ast::byte_span` sequencing contract).
pub struct Unit<'a> {
    pub source: &'a str,
    pub tokens: &'a TokenTable,
    pub ast: &'a SyntaxTable,
    /// Byte↔line mapping used by suppression line attribution.
    pub lines: &'a LineIndex,
    /// The analyzed file path — borrowed, never copied. `file_names` reads
    /// the basename (official `source.shortName`,
    /// file_names.dart:31-41). `None` (a run without a path) skips the
    /// path-dependent rules, mirroring the official `element != null` guard.
    pub path: Option<&'a Path>,
    pub features: Features,
    /// The library's settled language version (`major.minor`): the owning
    /// package's version with the `// @dart` override folded in — the same
    /// settled face [`Features`] derives from, mounted by assignment at the
    /// caller's hand-off (`unit.version = …`, the [`Unit::workspace`]
    /// shape). The version-gated registrations
    /// (`crate::registry::REGISTRATION_GATES`) consult it once per run.
    /// `None` — a caller without a settled face (bare test fixtures) —
    /// applies no version gate: the official posture for a library with no
    /// version info is the latest default, which admits every current
    /// gate.
    pub version: Option<dartq_language::LanguageVersion>,
    /// The containing-package facts the pub-aware rules read
    /// ([`WorkspaceFacts`]). `None` — outside any known package, or where
    /// the containing pubspec is unparseable — is the official no-pubspec
    /// posture (`package is! PubPackage` / `pubspec == null`,
    /// depend_on_referenced_packages.dart:33-40): those rules stay silent.
    pub workspace: Option<&'a WorkspaceFacts>,
    /// The mounted reachability face ([`ReachabilityFace`], #1199) — the
    /// `unreachable_from_main` consumption seam. `None` — a run without the
    /// engine face — keeps the rule silent, the same posture as the
    /// semantic rules' unmounted [`Check::semantics`].
    pub reachability: Option<ReachabilityFace<'a>>,
    /// The mounted async-gap rows ([`AsyncContextUse`], #1198) — the
    /// `use_build_context_synchronously` consumption seam. `None` keeps the
    /// rule silent (the same unmounted-face posture).
    pub async_context_uses: Option<&'a [AsyncContextUse]>,
    /// The mounted library type-use counts (#1640) — the
    /// `avoid_private_typedef_functions` consumption seam: one name-lexeme →
    /// named-type-row count per owning library, folded across the
    /// registration-order units ([`crate::engine::type_use_counts`]). `None` — a run
    /// without the engine face — keeps the rule silent, the same
    /// unmounted-face posture; no same-file fallback count is fabricated.
    pub library_type_uses: Option<&'a rustc_hash::FxHashMap<&'a str, u32>>,
}

/// The pubspec-derived facts of the analyzed file's containing package,
/// built once per package by the mounting face (the package table's
/// attribution plus the containing pubspec's parsed dependency names).
pub struct WorkspaceFacts {
    /// The containing package's own name (official `pubspec.name`).
    pub own_package: String,
    /// The own name plus every `dependencies` entry — available to every
    /// import of the package (depend_on_referenced_packages.dart:44-48).
    pub dependencies: rustc_hash::FxHashSet<String>,
    /// Every `dev_dependencies` entry — available only outside the public
    /// dirs; the rule's check applies the `isInPublicDir` gate per unit
    /// (ast.dart:112-129: `lib/`, `bin/`, `hook/build.dart`,
    /// `hook/link.dart`).
    pub dev_dependencies: rustc_hash::FxHashSet<String>,
    /// The containing package's root (canonical) — the public-dir gates'
    /// base.
    pub root: PathBuf,
    /// The package's package-URI directory (`packageUri`, normally `lib/`):
    /// a file under it resolves to a `package:` URI —
    /// `implementation_imports`' registration gate
    /// (implementation_imports.dart:30-33).
    pub lib_dir: PathBuf,
}

/// One `BuildContext` usage verdict of the async-gap face (#1198): the
/// `use_build_context_synchronously` consumption seam, mounted as
/// [`Unit::async_context_uses`]. The face owns use identification
/// (BuildContext-typed values, the `State.context` getter shape, nullable
/// exemption), await-gap segmentation, and mounted-guard recognition; a row
/// is one judged use, emitted only when it is reportable or
/// wrong-guarded.
pub struct AsyncContextUse {
    /// UTF-16 half-open span of the use — the report anchor.
    pub range: core::ops::Range<u32>,
    /// A valid `mounted` check guards the use on this path: never reported.
    pub guarded: bool,
    /// A `mounted` check stood in guard position but resolved to the wrong
    /// association (`State.mounted` over a plain `BuildContext` use, or the
    /// reverse): the report upgrades to the `wrong_mounted` message
    /// variant.
    pub unrelated_mounted: bool,
}

/// The mounted reachability face of one unit (#1199 engine pass): the
/// `unreachable_from_main` consumption seam, mounted as
/// [`Unit::reachability`]. The face owns entry detection (`main`,
/// `@pragma('vm:entry-point')`, `@Preview`) and the reference propagation
/// over the library's captures; the shell reads verdicts and applies its
/// own reporting face (public, not `@visibleForTesting`, unreachable).
///
/// The engine side answers through `Reachability::is_reachable(file, row)`
/// keyed by capture rows; the adapter that fills this seam (the mounting
/// lane) joins those bits with the capture rows' name-token offsets and
/// derives `has_entry` by scanning the library's capture rows for the
/// entry kinds — the substrate deliberately exposes no separate entry API.
#[derive(Clone, Copy)]
pub struct ReachabilityFace<'a> {
    /// Whether the library (parts included) declares any entry point.
    /// `false` reports nothing at all — the official empty-entry-points
    /// bail (`unreachable_from_main.dart`: `if (entryPoints.isEmpty)
    /// return`).
    pub has_entry: bool,
    /// `(name-token UTF-16 offset, reachable)` pairs, ascending — one per
    /// capture declaration row of this unit. A name offset absent from the
    /// table is outside the face's universe: the conservative answer is no
    /// report (the official posture for a declaration the gatherer never
    /// sees).
    pub flags: &'a [(u32, bool)],
}

impl ReachabilityFace<'_> {
    /// The entry-reachability of the declaration whose name token starts at
    /// `name_offset`; `None` outside the face's universe.
    #[must_use]
    pub fn is_reachable(&self, name_offset: u32) -> Option<bool> {
        let index = self
            .flags
            .partition_point(|&(offset, _)| offset < name_offset);
        match self.flags.get(index) {
            Some(&(offset, reachable)) if offset == name_offset => Some(reachable),
            _ => None,
        }
    }
}

/// Reduces one parsed unit's named-type rows to a name-lexeme → count map —
/// the per-unit half of the [`Unit::library_type_uses`] face (#1640). The
/// row face is exactly the one the `avoid_private_typedef_functions` check
/// reads (upstream `_CountVisitor.visitNamedType` counting `NamedType` rows
/// by `node.name.lexeme`, upstream Dart SDK source,
/// pkg/linter/lib/src/rules/avoid_private_typedef_functions.dart:40-50):
/// every `Type` row contributes its head token's lexeme. The mounting
/// driver folds this over the owning library's registration-order units —
/// the defining unit first, then each part in part-directive order, the set
/// [`dartq_semantic::Analysis::library_units`] publishes — and mounts the
/// merged map. Keys borrow the unit's own source text, so a caller folding
/// several units keeps the map's keys anchored to texts that outlive it.
#[must_use]
pub fn type_use_counts<'a>(
    source: &'a str,
    ast: &SyntaxTable,
    tokens: &TokenTable,
) -> FxHashMap<&'a str, u32> {
    let mut uses: FxHashMap<&'a str, u32> = FxHashMap::default();
    for row in 0..narrow_u32(ast.len()) {
        if ast.kind(row) != NodeKind::Type {
            continue;
        }
        let head = ast.token_range(row).start;
        *uses
            .entry(tokens.lexeme(source, head as usize))
            .or_insert(0) += 1;
    }
    uses
}

/// One reported lint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lint {
    pub rule: RuleId,
    /// UTF-16 half-open range of the reported node/token.
    pub range: core::ops::Range<u32>,
    /// Rendered `problemMessage`.
    pub message: String,
    /// Rendered `correctionMessage`, when the official code provides one.
    pub correction: Option<String>,
}

/// Per-file output fuse (the parser's diagnostic cap, applied to lints):
/// past this many hits a run stops reporting. Structural bound on the sink.
const MAX_LINTS_PER_FILE: usize = 10_000;

/// A wildcard local/parameter declaration: the byte range of the `_`-name and
/// the byte range of the scope it binds over. `no_wildcard_variable_uses`
/// reads the table (with the feature off, a `_` local/parameter binds, and a
/// bare use of it reports — no_wildcard_variable_uses.dart:37-42 flags
/// Local/Formal elements only, so fields/getters never enter the table).
#[derive(Clone, Debug)]
pub struct WildcardScope {
    /// The declared name's byte range (all underscores).
    pub name: core::ops::Range<usize>,
    /// The byte range of the scope the declaration binds over.
    pub scope: core::ops::Range<usize>,
}

/// Kind -> rule dispatch. Built once from the registry; a node whose
/// kind has no registered rules costs one empty-Vec test.
#[expect(
    clippy::cast_possible_truncation,
    reason = "rule positions come from the static registry, bounded far below \
              u16::MAX; u16 is the RuleId payload width"
)]
static DISPATCH: LazyLock<[Vec<u16>; NodeKind::KIND_COUNT as usize]> = LazyLock::new(|| {
    let mut table: [Vec<u16>; NodeKind::KIND_COUNT as usize] = std::array::from_fn(|_| Vec::new());
    for (i, spec) in rules().iter().enumerate() {
        for kind in spec.kinds {
            table[*kind as usize].push(i as u16);
        }
    }
    table
});

/// The run-scoped, lazily-built caches for the whole-unit declaration faces
/// (the #703 F4 hoist): before this, every candidate node re-walked all
/// top-level declarations (`same_file::scan` + `declares_name`) and every
/// field query re-walked the class body (`type_face_of_field`) —
/// O(candidates x declarations) arena walks and allocations per run. Each
/// slot is built once on first query (units a rule family never touches pay
/// nothing) and dropped with the run; no global state. Contents are token
/// tables only — name queries compare lexemes over the cached rows (the
/// O(children) read the rules are documented against), never re-walk the
/// arena.
///
/// Slot contract (the [`Check::direct_children`] materialize-first rule,
/// applied to caches): a returned borrow is a read-only view shared freely,
/// but it must not stay alive across a build of the *same* slot — the build
/// takes the slot exclusively. The accessors keep guards function-local to
/// their read, so no call site can trip this; nested-borrow panics name the
/// violation.
/// Class row -> its instance fields: `(name token, written type row)` per
/// field, in body order (`None` type row = the field is untyped).
type FieldTypes = FxHashMap<u32, Vec<(u32, Option<u32>)>>;

pub(crate) struct RunCaches {
    /// The unit's same-file declaration index
    /// ([`Decls`]), read by the annotation/shadow/creation faces.
    pub(crate) decls: core::cell::RefCell<Option<Decls>>,
    /// Read by [`crate::rules::type_face_of_field`].
    pub(crate) field_types: core::cell::RefCell<FieldTypes>,
    /// Class-like row -> its written supertypes as `(name head token,
    /// clause)` pairs in source order. Read by [`crate::rules::supertypes`]
    /// (#758: the header scan ran per ancestor per ancestor-chain build —
    /// O(class subtree) each — and its `TypeArguments` skip never advanced
    /// the cursor, hanging on every generic declaration).
    pub(crate) supertype_lists: core::cell::RefCell<FxHashMap<u32, Vec<(u32, SupertypeClause)>>>,
    /// Supertype/class-like name -> the same-file class-like row, built once
    /// per run on first query (`None`-guarded like [`Self::decls`]). Read by
    /// [`crate::rules::same_file_declaration`] (#758: every lookup re-walked
    /// all top-level wrappers).
    pub(crate) class_rows: core::cell::RefCell<Option<FxHashMap<String, u32>>>,
    /// `(class-like row, concrete)` -> the transitive same-file ancestor
    /// chain in written order. Read by
    /// [`crate::rules::same_file_ancestors`] (#758: `annotate_overrides`
    /// rebuilt the identical chain per member).
    pub(crate) ancestor_chains: core::cell::RefCell<FxHashMap<(u32, bool), Vec<u32>>>,
    /// Class-like row -> its member rows ([`crate::rules::body_members`]).
    /// Read by the override-family rules, which re-derive the identical list
    /// per member (#758: each derivation filtered the whole arena by
    /// `parent`).
    pub(crate) body_member_rows: core::cell::RefCell<FxHashMap<u32, Vec<u32>>>,
    /// The same-file class-hierarchy face of `implicit_reopen` — built once
    /// per run on first query (`None`-guarded like [`Self::decls`]): the
    /// extends-able class rows by name plus each row's effective
    /// class-family flags (written + the sealed induction,
    /// `instance_member_inferrer.dart` `_setInducedModifier`).
    pub(crate) class_hierarchy:
        core::cell::RefCell<Option<crate::rules::declarations::ClassHierarchy>>,
    /// Typedef row -> its computed per-parameter variances
    /// (`variance_builder.dart`), computed on demand with the
    /// provisional-insert cycle guard. Read by the `unsafe_variance`
    /// argument mapping.
    pub(crate) alias_variances: core::cell::RefCell<crate::rules::types::AliasVariances>,
    /// Row owner -> its child rows, ascending — the whole-arena parent
    /// grouping [`crate::rules::rows_with_parent`] reads, built once per run
    /// on first query (`None`-guarded like [`Self::decls`]) so the per-owner
    /// queries cost one lookup instead of one arena pass each (#2467: every
    /// query filtered the whole arena).
    pub(crate) owner_rows: core::cell::RefCell<Option<rustc_hash::FxHashMap<u32, Vec<u32>>>>,
}

/// Runs the enabled rules over one unit. Output is in traversal order:
/// pre-order (a node before its children), registry order within a node —
/// the official visitor dispatch order. The AST-only posture: the semantic
/// query face is unmounted, so semantic rules (their `c.semantics()?`
/// guard) stay silent — mount one through [`Unit::run_with_semantics`].
///
/// Traversal visits every node exactly once. The arena's `children` range is
/// a completion window (descendants complete inside it, before their
/// ancestor), so the walk extracts *direct* children with the subtree-tiling
/// scan in `push_direct`: a child's whole subtree occupies
/// `[children(child).start, child]`, contiguous, ending at the child
/// itself — the next direct sibling starts right after. The one production
/// exception is the switch-member window (the bridge snapshots the segment
/// start before the previous member seals, so each member's window starts AT
/// that member's arena row); `tile_direct` canonicalizes it: the block
/// level pushes the jump-landed member and skips its subtree, the borrowing
/// member skips the snapped row, and every row still dispatches exactly
/// once.
#[must_use]
#[expect(
    clippy::implicit_hasher,
    reason = "FxHashSet is the workspace-mandated concrete hasher               (deterministic iteration, hot-path cost)"
)]
pub fn run(
    unit: &Unit<'_>,
    enabled: &EnableSet,
    unignorable: &rustc_hash::FxHashSet<String>,
) -> Vec<Lint> {
    run_semantics(unit, enabled, unignorable, None)
}

impl Unit<'_> {
    /// Runs the enabled rules with the unit's semantic query face mounted
    /// (the g1031 C.3 run-parameter mount, the sibling of the plain
    /// [`run`]): `semantics` is one unit's immutable fold projection
    /// ([`Analysis`], built by the driver per file) and travels to the rules
    /// through [`Check::semantics`]. `None` — the [`run`] posture — keeps
    /// every semantic rule silent; a semantic rule that is enabled while the
    /// face is absent is a driver-configuration error surfaced there, never
    /// inside a rule body.
    ///
    /// The face is the method's own borrow: it needs to outlive exactly this
    /// call, which is why it rides the call instead of the [`Unit`] (the
    /// bridge's transient substrate carries no context tables, and the
    /// AST-only constructors across the crate's test fixtures stay
    /// field-complete).
    #[must_use]
    pub fn run_with_semantics(
        &self,
        enabled: &EnableSet,
        unignorable: &rustc_hash::FxHashSet<String>,
        semantics: Option<&Analysis<'_>>,
    ) -> Vec<Lint> {
        run_semantics(self, enabled, unignorable, semantics)
    }
}

/// The breaking-release annotation face (`remove_deprecations_in_breaking_
/// versions`, the #1156 mount): the official rule registers its annotation
/// visitor only when the containing package's pubspec `version:` parses to
/// a breaking release (`remove_deprecations_in_breaking_version.dart
/// :36-48` — a missing or unparseable value never registers), so the spec
/// stays `kinds: &[]` — no node dispatch — and the mounting layer joins the
/// enable set with the pubspec fact, then runs this walk. Every `Metadata`
/// row goes through [`check_annotation`] (which re-checks the version gate
/// per row) and a hit reports through the standard emit path, so the
/// ignore suppression and `cannot-ignore` override ride along. An absent
/// fact (`""`) is the registration bail.
///
/// Returns the walk's rows and its suppression record — the `(line, name)`
/// pairs its ignore gate dropped. That record is the late-join tail's
/// facts (#1904): the run's own `unnecessary_ignore` epilogue can never
/// see these drops (the walk joins after the run returns), so the shell
/// feeds it to [`unnecessary_ignore_late_rows`], whose join half answers
/// the late-join citations exactly.
#[must_use]
#[expect(
    clippy::implicit_hasher,
    reason = "FxHashSet is the workspace-mandated concrete hasher               (deterministic iteration, hot-path cost)"
)]
pub fn run_breaking_version_annotations(
    unit: &Unit<'_>,
    enabled: &EnableSet,
    unignorable: &rustc_hash::FxHashSet<String>,
    pubspec_version: &str,
) -> (Vec<Lint>, Vec<(u32, &'static str)>) {
    crate::rules::flutter::reset_refutation_memo();
    let Some(rule) = rule_by_name("remove_deprecations_in_breaking_versions") else {
        return (Vec::new(), Vec::new());
    };
    if !enabled.is_enabled(rule) {
        return (Vec::new(), Vec::new());
    }
    let Some(root) = unit.ast.root() else {
        return (Vec::new(), Vec::new());
    };
    let comments = Comments::extract(unit.source, unit.tokens);
    let ignore = IgnoreInfo::parse(unit.source, &comments);
    let suppressed = core::cell::RefCell::new(Vec::new());
    let caches = RunCaches {
        decls: core::cell::RefCell::new(None),
        field_types: core::cell::RefCell::new(FxHashMap::default()),
        supertype_lists: core::cell::RefCell::new(FxHashMap::default()),
        class_rows: core::cell::RefCell::new(None),
        ancestor_chains: core::cell::RefCell::new(FxHashMap::default()),
        body_member_rows: core::cell::RefCell::new(FxHashMap::default()),
        class_hierarchy: core::cell::RefCell::new(None),
        alias_variances: core::cell::RefCell::new(crate::rules::types::AliasVariances::default()),
        owner_rows: core::cell::RefCell::new(None),
    };
    let direct = core::cell::RefCell::new(Vec::new());
    let path = [root];
    let mut out = Vec::new();
    // One linear arena pass (the `rows_with_parent` idiom): a `Metadata`
    // row is one annotation; the scan allocates nothing beyond the hits.
    for meta in 0..narrow_u32(unit.ast.len()) {
        if out.len() >= MAX_LINTS_PER_FILE {
            break;
        }
        if unit.ast.kind(meta) != NodeKind::Metadata {
            continue;
        }
        let mut check = Check {
            unit,
            comments: &comments,
            ignore: &ignore,
            unignorable,
            suppressed: &suppressed,
            path: &path,
            wildcards: &[],
            rule,
            direct: &direct,
            caches: &caches,
            semantics: None,
            out: &mut out,
        };
        if let Some(range) = check_annotation(&check, pubspec_version, meta) {
            check.report(range, &[]);
        }
    }
    (out, suppressed.into_inner())
}

/// The shared walk behind [`run`] and [`Unit::run_with_semantics`].
fn run_semantics(
    unit: &Unit<'_>,
    enabled: &EnableSet,
    unignorable: &rustc_hash::FxHashSet<String>,
    semantics: Option<&Analysis<'_>>,
) -> Vec<Lint> {
    // One run per memo: verdicts computed below answer for this walk only
    // (the pin is defense-in-depth; see `spells_framework`).
    crate::rules::flutter::reset_refutation_memo();
    let root = match unit.ast.root() {
        Some(root) if enabled != &EnableSet::none() => root,
        _ => return Vec::new(),
    };
    // The registration consult: feature- and version-gated rules join the
    // run only when the unit's faces admit them — every upstream
    // `isFeatureEnabled` registration bail plus the version gates live in
    // one table ([`crate::registry::REGISTRATION_GATES`], resolved
    // against `unit.features`/`unit.version` here). One filtered copy per
    // run — the per-node dispatch keeps testing bits.
    let enabled = crate::registry::registration_gated(enabled, &unit.features, unit.version);
    let comments = Comments::extract(unit.source, unit.tokens);
    let ignore = IgnoreInfo::parse(unit.source, &comments);
    let suppressed = core::cell::RefCell::new(Vec::new());
    // The wildcard table is collected inside the same DFS (one `Identifier`
    // kind test per node) and only when the one rule that reads it runs —
    // the rule registers only with the wildcard feature OFF
    // (no_wildcard_variable_uses.dart:36-38), a polarity the gate table
    // already carried into `enabled`.
    let no_wildcard =
        rule_by_name("no_wildcard_variable_uses").is_some_and(|id| enabled.is_enabled(id));
    let mut wildcards: Vec<WildcardScope> = Vec::new();

    let mut out = Vec::new();

    // Explicit-stack DFS (no structural recursion — the engine's termination
    // invariant holds on any input). The stack holds `(node, depth)` where
    // `depth` is the ancestor-path length the node belongs at; `path` is the
    // shared chain the rules read. A single scratch buffer collects direct
    // children per node (one allocation for the whole run).
    let mut stack: Vec<(u32, usize)> = Vec::new();
    let mut path: Vec<u32> = vec![root];
    let direct = core::cell::RefCell::new(Vec::new());
    let caches = RunCaches {
        decls: core::cell::RefCell::new(None),
        field_types: core::cell::RefCell::new(FxHashMap::default()),
        supertype_lists: core::cell::RefCell::new(FxHashMap::default()),
        class_rows: core::cell::RefCell::new(None),
        ancestor_chains: core::cell::RefCell::new(FxHashMap::default()),
        body_member_rows: core::cell::RefCell::new(FxHashMap::default()),
        class_hierarchy: core::cell::RefCell::new(None),
        alias_variances: core::cell::RefCell::new(crate::rules::types::AliasVariances::default()),
        owner_rows: core::cell::RefCell::new(None),
    };
    dispatch(
        unit,
        &comments,
        &ignore,
        unignorable,
        &suppressed,
        &path,
        &enabled,
        &direct,
        &caches,
        &wildcards,
        semantics,
        &mut out,
    );
    push_direct(unit.ast, root, &mut stack, &direct, path.len());

    while let Some((node, depth)) = stack.pop() {
        path.truncate(depth);
        path.push(node);
        if no_wildcard {
            collect_wildcard(unit, &path, &mut wildcards);
        }
        dispatch(
            unit,
            &comments,
            &ignore,
            unignorable,
            &suppressed,
            &path,
            &enabled,
            &direct,
            &caches,
            &wildcards,
            semantics,
            &mut out,
        );
        push_direct(unit.ast, node, &mut stack, &direct, path.len());
    }

    // The `unnecessary_ignore` epilogue (the #1345 mount): the emitted set
    // is complete and the run has not returned — the exact face the official
    // validator occupies (`library_analyzer.dart`:331-341, the validator
    // runs after all other diagnostics but before the list is filtered).
    if rule_by_name("unnecessary_ignore").is_some_and(|id| enabled.is_enabled(id)) {
        let suppressed = suppressed.into_inner();
        let entries = ignore.name_entries(unit.source);
        // The run half: the late-join and analyzer-code names belong to the
        // shell's re-joins (`unnecessary_ignore_late_rows` /
        // `unnecessary_ignore_analyzer_rows`) — their suppression facts are
        // records this run structurally cannot hold.
        unnecessary_ignore_rows(
            unit,
            &ignore,
            unignorable,
            &suppressed,
            JoinHalf::Run,
            &entries,
            |_| false,
            &mut out,
        );
    }

    collapse_duplicate_rows(&mut out);
    out
}

/// Collapses the rows the oracle collapses. The analyzer's diagnostic
/// listener collects into a set whose equality is (code, offset, length,
/// message) (upstream Dart SDK source, pkg/analyzer/lib/error/listener.dart
/// with the `Diagnostic ==` in diagnostic.dart), so two rule shapes firing
/// the same expression surface one row. Within one run the file and the
/// lint severity are constant, so the per-row identity is
/// (rule, span, message); `correction` stays outside the key, exactly as
/// upstream equality ignores it. First emission wins and emission order is
/// preserved (`retain` keeps kept rows in place), matching the
/// insertion-ordered set the oracle serves rows from.
///
/// One pass over the emitted rows, gated at the duplicate floor; the set
/// dies with the pass.
fn collapse_duplicate_rows(out: &mut Vec<Lint>) {
    if out.len() < 2 {
        return;
    }
    // The key owns its message text: `retain` hands each row a fresh shared
    // borrow that cannot escape into a set living across calls, so cloning
    // is the exact, allocation-bounded shape (≤ the row count of short
    // messages, dropped at return).
    let mut seen: rustc_hash::FxHashSet<(RuleId, u32, u32, String)> =
        rustc_hash::FxHashSet::default();
    out.retain(|lint| {
        seen.insert((
            lint.rule,
            lint.range.start,
            lint.range.end,
            lint.message.clone(),
        ))
    });
}

/// The `unnecessary_ignore` run epilogue (the #1345 mount): every ignore
/// comment citation that dropped no diagnostic of this run reports at the
/// cited name token — the [`crate::rules::docs::unnecessary_ignore`] spec's
/// consumption seam, the dartq shape of the analyzer's `IgnoreValidator`
/// (dart 3.15 main, `ignore_validator.dart:73-289`).
///
/// The join is the official walk's tail over this run's own facts. The
/// citations arrive through [`IgnoreInfo::name_entries`] (the dedup'd walk —
/// exact duplicates left it, as the official duplicate pass removes them).
/// "Used" is the run's suppression record: a citation fired exactly when it
/// dropped a hit — file-wide when the name dropped anywhere, line-scoped
/// when the drop sat on the entry's line (the official match pass removes
/// the name from the file list and the diagnostic's line list,
/// :137-144). Gates mirror the official unnecessary pass
/// (:199-249): names in the `cannot-ignore` set never report (the official
/// unignorable branch removes them from the lists first, :89/:113/:164-174);
/// removed rules never report here (the official removed-state face
/// reports `removed_lint_use` instead and continues, :219-235). Fourth, the
/// ownership seam: the join runs in three halves ([`JoinHalf`]) — the run's
/// epilogue owns the lint names whose rows this run produced; the
/// [`LATE_JOIN_RULES`] names belong to the shell's breaking-version re-join
/// ([`unnecessary_ignore_late_rows`]); analyzer diagnostic codes belong to
/// the shell's analyzer re-join ([`unnecessary_ignore_analyzer_rows`]).
/// Each half answers against the rows its producing walk carried; the union
/// is the official single pass over the unfiltered final list
/// (`library_analyzer.dart`:327-341). The whole validator is silent on
/// generated files (the official mount filters them before the validator
/// constructs, `library_analyzer.dart`:331-334 `whereNot(isGenerated)`).
/// The rows then pass the same suppression gate every lint passes — the
/// official validator's rows run before the pipeline's ignore filter
/// (`library_analyzer.dart`:135) — so a `// ignore: unnecessary_ignore`
/// still silences its own verdict.
///
/// Bounded: one pass over the citations plus one pass over the suppression
/// record into two hash sets — linear in citations + suppressed hits.
///
/// The file-scope face's problem template — `messages.yaml`
/// `unnecessaryIgnoreFile` (the second code of the `unnecessaryIgnore`
/// shared name; the generated meta table holds the line-scope code only).
const PROBLEM_IN_FILE: &str =
    "The diagnostic '{0}' isn't produced in this file so it doesn't need to be ignored.";

#[expect(
    clippy::implicit_hasher,
    reason = "FxHashSet is the workspace-mandated concrete hasher               (deterministic iteration, hot-path cost)"
)]
fn unnecessary_ignore_rows(
    unit: &Unit<'_>,
    ignore: &IgnoreInfo,
    unignorable: &rustc_hash::FxHashSet<String>,
    suppressed: &[(u32, &'static str)],
    half: JoinHalf,
    entries: &[NameEntry],
    known_code: impl Fn(&str) -> bool,
    out: &mut Vec<Lint>,
) {
    let Some(rule) = rule_by_name("unnecessary_ignore") else {
        return;
    };
    // The official mount's generated-file filter (`library_analyzer.dart`
    // :331-334): the whole validator — every half — skips generated files.
    if unit.path.is_some_and(path_is_generated) {
        return;
    }
    if entries.is_empty() {
        return;
    }
    let mut used_names: rustc_hash::FxHashSet<&str> = rustc_hash::FxHashSet::default();
    let mut used_scopes: rustc_hash::FxHashSet<(u32, &str)> = rustc_hash::FxHashSet::default();
    for &(line, name) in suppressed {
        used_names.insert(name);
        used_scopes.insert((line, name));
    }
    let spec = rules()[rule.0 as usize];
    for entry in entries {
        if out.len() >= MAX_LINTS_PER_FILE {
            return;
        }
        if unignorable.contains(entry.name.as_str()) {
            continue;
        }
        match rule_by_name(&entry.name) {
            Some(id) => {
                if rules()[id.0 as usize].group == Group::Removed {
                    continue;
                }
                // The ownership seam (#1353, the #1904/#2116 splits): a
                // late-join name has its rows joined by the mounting
                // shell's breaking-version walk, an analyzer code by the
                // shell's semantic fold — each name answers in its owning
                // half only, and a lint name never answers in the
                // analyzer half at all.
                let late = LATE_JOIN_RULES.contains(&rules()[id.0 as usize].name);
                if match half {
                    JoinHalf::Run => late,
                    JoinHalf::Late => !late,
                    JoinHalf::Analyzer => true,
                } {
                    continue;
                }
            }
            None => {
                // Not a lint name: the official validator admits a name the
                // lint registry does not know only when the diagnostic-code
                // registry knows it (`ignore_validator.dart`:208-213 — an
                // unknown name could come from a plugin and never reports);
                // a known code falls through to the unnecessary walk. Those
                // codes' rows are the analyzer layer's, so their facts live
                // at the shell's analyzer half — the run and late halves
                // skip them unconditionally.
                if half != JoinHalf::Analyzer
                    || !known_code(&entry.name)
                    || PARTIAL_ANALYZER_CODES.contains(&entry.name.as_str())
                {
                    continue;
                }
            }
        }
        let used = match entry.scope {
            None => used_names.contains(entry.name.as_str()),
            Some(line) => used_scopes.contains(&(line, entry.name.as_str())),
        };
        if used {
            continue;
        }
        let (line, _) = unit.lines.line_col_utf16(unit.source, entry.offset);
        if ignore.ignored(line, spec.name) && !unignorable.contains(spec.name) {
            continue;
        }
        out.push(Lint {
            rule,
            range: entry.offset..entry.offset + entry.length,
            // The two official templates of the shared name: the meta
            // table carries the line-scope one (`unnecessaryIgnore`,
            // messages.yaml:15138); a file-scope citation reports the
            // `unnecessaryIgnoreFile` wording (messages.yaml:15174).
            message: crate::registry::render(
                if entry.scope.is_none() {
                    PROBLEM_IN_FILE
                } else {
                    spec.problem
                },
                &[&entry.name],
            ),
            correction: (!spec.correction.is_empty())
                .then(|| crate::registry::render(spec.correction, &[&entry.name])),
        });
    }
}

/// Analyzer codes whose dartq production is known to under-report: the
/// receiver static type of a member read over an SDK type (`x?.abs()`,
/// `a.g` on a getter) is not modelled, so `invalid_null_aware_operator`
/// on a null-shorted chain (`x?.abs()?.isEven`) is missed. The absence of
/// a row is then no evidence the citation is unnecessary, so the analyzer
/// half stays silent on these names (an under-report, never a false
/// `unnecessary_ignore`).
const PARTIAL_ANALYZER_CODES: [&str; 1] = ["invalid_null_aware_operator"];

/// Which walk's facts the join answers over — the ownership split that
/// keeps each citation answered by the record that actually saw its drops.
#[derive(Clone, Copy, PartialEq, Eq)]
enum JoinHalf {
    /// The run epilogue: lint names this run dispatched (`run`'s own
    /// suppression record).
    Run,
    /// The shell's breaking-version re-join: the [`LATE_JOIN_RULES`] names.
    Late,
    /// The shell's analyzer re-join: known analyzer diagnostic codes, the
    /// #2116 face.
    Analyzer,
}

/// The upstream generated-file suffix table (`file_paths.dart:89-104`
/// `isGenerated`): the official validator mount skips generated files
/// entirely (`library_analyzer.dart`:331-334), so every `unnecessary_ignore`
/// half is silent on them.
const GENERATED_SUFFIXES: [&str; 6] = [
    ".g.dart",
    ".pb.dart",
    ".pbenum.dart",
    ".pbserver.dart",
    ".pbjson.dart",
    ".template.dart",
];

/// Whether `path` is a generated file per the official suffix table
/// ([`GENERATED_SUFFIXES`]).
fn path_is_generated(path: &Path) -> bool {
    GENERATED_SUFFIXES.iter().any(|suffix| {
        path.as_os_str()
            .as_encoded_bytes()
            .ends_with(suffix.as_bytes())
    })
}

/// The analyzer half of the `unnecessary_ignore` join (#2116): citations
/// naming analyzer diagnostic codes answer against the rows the semantic
/// layer produced for this file — the official match pass walks every
/// reported diagnostic, lint and analyzer row alike
/// (`ignore_validator.dart`:137-144), and a non-lint name the diagnostic
/// registry knows falls through to the unnecessary walk (:208-213). The
/// mounting shell derives the suppression record from the pre-filter
/// production rows (the fold's own ignore gate strips suppressed rows
/// before its report returns, so the shell neutralizes the ignore
/// keywords for that one fold — the pairs are what the unfiltered list
/// held). An empty record answers upstream-true: nothing consumed the
/// citation, it reports.
///
/// `known_code` is the diagnostic-code registry's membership test — the
/// REPORT filter leg (name-only, `ignore_validator.dart`:208-213);
/// `canonical_code` resolves a `uniqueName` spelling to its shared
/// registry `name` — the citation match's second leg
/// (`ignore_info.dart`:39-52). The admission probe joins both: a file
/// citing only a `uniqueName` still qualifies as an analyzer-code
/// citation (it can suppress a diagnostic), while the per-entry
/// unnecessary report stays name-only — a `uniqueName` entry is skipped
/// by `known_code` and stays silent, exactly as upstream. The shell
/// passes the engine's two faces; the dartq-lints crate keeps no
/// dependency on it. Plain runs — test harnesses, embedded consumers
/// without the shell — keep the #1353 silence on these names: the run
/// epilogue's record never holds analyzer drops, so the verdict belongs
/// here. Generated files stay silent ([`GENERATED_SUFFIXES`]).
#[must_use]
#[expect(
    clippy::implicit_hasher,
    reason = "FxHashSet is the workspace-mandated concrete hasher               (deterministic iteration, hot-path cost)"
)]
pub fn unnecessary_ignore_analyzer_rows(
    unit: &Unit<'_>,
    enabled: &EnableSet,
    unignorable: &rustc_hash::FxHashSet<String>,
    analyzer_suppressed: &[(u32, &'static str)],
    known_code: impl Fn(&str) -> bool,
    canonical_code: impl Fn(&str) -> Option<&'static str>,
) -> Vec<Lint> {
    if !rule_by_name("unnecessary_ignore").is_some_and(|id| enabled.is_enabled(id)) {
        return Vec::new();
    }
    let comments = Comments::extract(unit.source, unit.tokens);
    let ignore = IgnoreInfo::parse(unit.source, &comments);
    let entries = ignore.name_entries(unit.source);
    // Nothing to admit unless a citation names a code the registry knows —
    // in either spelling (name OR uniqueName, `ignore_info.dart`:39-52);
    // the same probe the shell pays before its fold, repeated here so the
    // face is self-contained for direct callers (#2136).
    if !entries
        .iter()
        .any(|entry| known_code(entry.name.as_str()) || canonical_code(&entry.name).is_some())
    {
        return Vec::new();
    }
    let mut out = Vec::new();
    unnecessary_ignore_rows(
        unit,
        &ignore,
        unignorable,
        analyzer_suppressed,
        JoinHalf::Analyzer,
        &entries,
        known_code,
        &mut out,
    );
    out
}

/// The late-join half of the `unnecessary_ignore` join (#1904): the
/// mounting shell re-runs the join over the [`LATE_JOIN_RULES`] names,
/// fed `late_suppressed` — the drops its own breaking-version walk's
/// ignore gate recorded ([`run_breaking_version_annotations`]'s second
/// element). A citation that dropped a late row stays silent, one that
/// dropped nothing reports: the official validator walks the unfiltered
/// final list where that row would already sit (`library_analyzer.dart`
/// :327-341). An empty record answers upstream-true as well — the rule
/// registers only on a breaking version
/// (`remove_deprecations_in_breaking_version.dart`:37-54), so no row
/// exists at all, `getRule` still finds the known rule and falls through
/// the state checks (`ignore_validator.dart`:222-248), the match pass
/// removes nothing, and the citation reports.
///
/// The plain-run face (no mounting shell) keeps the #1353 posture: a bare
/// [`run`] never reports the late-join names — the verdict belongs here.
#[must_use]
#[expect(
    clippy::implicit_hasher,
    reason = "FxHashSet is the workspace-mandated concrete hasher               (deterministic iteration, hot-path cost)"
)]
pub fn unnecessary_ignore_late_rows(
    unit: &Unit<'_>,
    enabled: &EnableSet,
    unignorable: &rustc_hash::FxHashSet<String>,
    late_suppressed: &[(u32, &'static str)],
) -> Vec<Lint> {
    if !rule_by_name("unnecessary_ignore").is_some_and(|id| enabled.is_enabled(id)) {
        return Vec::new();
    }
    // A citation requires the name's text in the source; the substring
    // probe skips the comment re-parse for the files that never cite a
    // late-join rule (the shell calls this per file). A false positive
    // (the name in a string literal) just takes the full path and finds
    // no entries — never a wrong verdict.
    if !LATE_JOIN_RULES
        .iter()
        .any(|name| unit.source.contains(*name))
    {
        return Vec::new();
    }
    let comments = Comments::extract(unit.source, unit.tokens);
    let ignore = IgnoreInfo::parse(unit.source, &comments);
    let entries = ignore.name_entries(unit.source);
    let mut out = Vec::new();
    unnecessary_ignore_rows(
        unit,
        &ignore,
        unignorable,
        late_suppressed,
        JoinHalf::Late,
        &entries,
        |_| false,
        &mut out,
    );
    out
}

/// Records one wildcard local/parameter/pattern-binding declaration. Only
/// the element faces the official rule flags enter the table
/// (`LocalVariableElement` / `FormalParameterElement` / catch parameter /
/// pattern variable — the official test's `test_field`/`test_getter` faces
/// stay out); the scope is the declaration's binding extent: the innermost
/// enclosing block/for-in/catch clause for locals, catch parameters, and
/// pattern bindings, the enclosing function for parameters. The production
/// block kinds the walk answers: `Block` is a nested-statement block,
/// `BlockBody` every function/literal body, and `SwitchCase`/
/// `SwitchExpressionCase` the member form — a case-bound name (or a member
/// local, the member's flat statement run carries no `Block` of its own)
/// binds over the member window. The bare-`_` pattern element is a
/// `WildcardPattern` (the parser routes it there, matching the official
/// parser) and the official scope walk binds no element for it, so it
/// stays out of the table — its uses are not wildcard-variable uses.
fn collect_wildcard(unit: &Unit<'_>, path: &[u32], out: &mut Vec<WildcardScope>) {
    // A local's binding extent is the innermost enclosing statement block:
    // `Block` is the nested-statement `{ ... }`, `BlockBody` the function
    // body form (the production bridge emits `BlockBody` for every body
    // context — a `var _` at function top level binds over it, not over a
    // `Block` that never exists). `SwitchCase`/`SwitchExpressionCase` cover
    // the member forms: case-bound pattern variables and locals declared in
    // a member's flat statement run bind over the member itself (its window
    // is the case head plus body — matching the official scope walk, which
    // ends a case-declared variable at the member's statement list).
    const LOCAL_SCOPES: [NodeKind; 6] = [
        NodeKind::Block,
        NodeKind::BlockBody,
        NodeKind::ForIn,
        NodeKind::CatchClause,
        NodeKind::SwitchCase,
        NodeKind::SwitchExpressionCase,
    ];
    const FUNCTION_SCOPES: [NodeKind; 7] = [
        NodeKind::Method,
        NodeKind::TopLevelMethod,
        NodeKind::Constructor,
        NodeKind::Factory,
        NodeKind::FunctionExpression,
        NodeKind::LocalFunctionDeclaration,
        NodeKind::PrimaryConstructorBody,
    ];

    let node = *path.last().expect("collect on a node");
    // The name span and scope set per declaration face: an `Identifier`
    // names through its own node span and reads the context discriminant
    // off its `Context` payload word; a `DeclaredVariablePattern` names
    // through its `Variable` payload word (the variable token — the `: T`
    // shorthand carries none) and scopes like a local.
    let (name_bytes, scopes) = match unit.ast.kind(node) {
        NodeKind::Identifier => {
            let Some(ctx) = unit.ast.field(node, PayloadField::Context) else {
                return;
            };
            let scopes: &[NodeKind] = match ctx {
                c if c == dartq_parser::IdentifierContext::LocalVariableDeclaration as u32 => {
                    &LOCAL_SCOPES[..]
                }
                c if c == dartq_parser::IdentifierContext::CatchParameter as u32 => {
                    &LOCAL_SCOPES[3..4]
                }
                c if c == dartq_parser::IdentifierContext::FormalParameterDeclaration as u32 => {
                    &FUNCTION_SCOPES[..]
                }
                _ => return,
            };
            (byte_span(unit.tokens, unit.ast, node), scopes)
        }
        NodeKind::DeclaredVariablePattern => {
            let Some(name_token) = unit.ast.field(node, PayloadField::Variable) else {
                return;
            };
            if name_token == dartq_ast::TOKEN_NONE {
                return;
            }
            let Some(token) = unit.tokens.get(name_token as usize) else {
                debug_assert!(false, "pattern name token outside the table");
                return;
            };
            (
                token.byte_offset as usize..(token.byte_offset + token.byte_len) as usize,
                &LOCAL_SCOPES[..],
            )
        }
        _ => return,
    };
    let name = &unit.source[name_bytes.clone()];
    if !name.bytes().all(|b| b == b'_') {
        return;
    }
    let scope = path
        .iter()
        .rev()
        .find(|&&ancestor| scopes.contains(&unit.ast.kind(ancestor)))
        .map(|&ancestor| byte_span(unit.tokens, unit.ast, ancestor));
    let Some(scope) = scope else {
        return;
    };
    out.push(WildcardScope {
        name: name_bytes,
        scope,
    });
}

/// Pushes the direct children of `id` onto the work stack, last child first
/// so pops yield source order. Direct children are read off the completion
/// window by the canonical subtree tiling (`tile_direct`). `scratch` is
/// the run's shared buffer (the rules borrow it through `Check` between
/// calls).
fn push_direct(
    ast: &SyntaxTable,
    id: u32,
    stack: &mut Vec<(u32, usize)>,
    scratch: &core::cell::RefCell<Vec<u32>>,
    depth: usize,
) {
    let mut scratch = scratch.borrow_mut();
    tile_direct(ast, id, &mut scratch);
    // Descending (last child first); pushed as-is, the last push is the
    // first child, so pops yield source order.
    stack.extend(scratch.drain(..).map(|child| (child, depth)));
}

/// Tiles `id`'s completion window into its direct children, collecting them
/// into `out` (cleared first) in descending arena order (top of the window
/// first).
///
/// The plain reverse walk pushes each subtree root and jumps to
/// `children(root).start`, relying on the tiling invariant: a child's whole
/// subtree occupies `[children(child).start, child]`, contiguous, ending at
/// the child itself. The switch-member windows break that invariant — the
/// bridge snapshots the material segment before the previous member seals,
/// so each member's window starts AT that member's arena row. Three repairs,
/// keyed on the window start:
///
/// - the jump from a pushed child lands ON the previous member's row (a
///   completed subtree root claimed as the child's own first entry). That
///   row is this level's member: push it, then skip its subtree by
///   descending its claimed left edge — pushing any further chained members
///   the same way (member n's window starts at member n-1's row).
/// - descending into a borrowing member, the snapped row sits at
///   `window.start` with its claimed subtree poking below it: not this
///   level's child (the block level pushed it) — skip it.
/// - a row at `window.start` whose parent is deeper than this level (the
///   block-head snapshot faces): not this level's child either — it
///   dispatches under its own parent chain within the member.
///
/// Rows at `window.start` are only ever *reached* by the decrement when one
/// of the two head shapes sits there (a clean tiling ends at the bottom
/// subtree's claimed start and the loop exits), so the head checks cost
/// nothing on clean windows. For every other shape the two jump rules
/// degenerate to the plain walk: the fire needs an unclean boundary (the
/// landed row carries a claimed subtree) claimed by the pushed child itself,
/// which the operand adoptions (`BinaryExpression`, `Assignment`,
/// `ObjectPattern`, patterns) never produce — their segments start on a
/// claimed-childless subtree bottom parented inside the operand.
fn tile_direct(ast: &SyntaxTable, id: u32, out: &mut Vec<u32>) {
    out.clear();
    let window = ast.children(id);
    let mut i = window.end;
    while i > window.start {
        i -= 1;
        if i == window.start && (ast.children(i).start < window.start || ast.parent(i) != Some(id))
        {
            // Head shape: dispatched through its own parent chain (or at
            // the block level), never as this level's child.
            i = ast.children(i).start;
            continue;
        }
        out.push(i);
        let mut s = ast.children(i).start;
        if s > window.start && ast.children(s).start < s && ast.parent(s) == Some(i) {
            // The jump-landed row is this level's member, not the pushed
            // child's: push it and continue below its subtree.
            out.push(s);
            let mut member = s;
            s = ast.children(s).start;
            while s > window.start && ast.children(s).start < s {
                if ast.parent(s) == Some(member) {
                    // Member n-1, borrowed by member n the same way.
                    out.push(s);
                    member = s;
                }
                s = ast.children(s).start;
            }
        }
        i = s;
    }
}

/// Dispatches the rules registered for the path's last node.
#[expect(
    clippy::too_many_arguments,
    reason = "dispatch threads the shared analysis context through the rule hot path; bundling \
              it into a struct adds indirection for no gain"
)]
fn dispatch(
    unit: &Unit<'_>,
    comments: &Comments,
    ignore: &IgnoreInfo,
    unignorable: &rustc_hash::FxHashSet<String>,
    suppressed: &core::cell::RefCell<Vec<(u32, &'static str)>>,
    path: &[u32],
    enabled: &EnableSet,
    direct: &core::cell::RefCell<Vec<u32>>,
    caches: &RunCaches,
    wildcards: &[WildcardScope],
    semantics: Option<&Analysis<'_>>,
    out: &mut Vec<Lint>,
) {
    let current = *path.last().expect("dispatch on a node");
    for &rule in DISPATCH[unit.ast.kind(current) as usize]
        .iter()
        .filter(|rule| enabled.is_enabled(RuleId(**rule)))
    {
        if out.len() >= MAX_LINTS_PER_FILE {
            return;
        }
        let mut check = Check {
            unit,
            comments,
            ignore,
            unignorable,
            suppressed,
            path,
            wildcards,
            rule: RuleId(rule),
            direct,
            caches,
            semantics,
            out: &mut *out,
        };
        (rules()[rule as usize].check)(&mut check);
    }
}

/// What a rule sees: the unit, its comments/suppression table, the ancestor
/// path (current node last), and the reporting face. One lifetime: every
/// field is re-borrowed to the dispatch-call scope, so each rule gets a
/// fresh check per node.
pub struct Check<'a> {
    pub unit: &'a Unit<'a>,
    pub comments: &'a Comments,
    pub ignore: &'a IgnoreInfo,
    /// The governing `analyzer.cannot-ignore` set (lowercased): names in it
    /// survive every ignore comment (`_filterIgnoredDiagnostics`,
    /// library_analyzer.dart:552-563) and report `unignorable_ignore`
    /// through the standalone validator face. Empty for the dominant
    /// workspace.
    pub unignorable: &'a rustc_hash::FxHashSet<String>,
    /// The run's suppression record: every hit the ignore gate dropped as
    /// its `(line, name)` — the pre-filter face the `unnecessary_ignore`
    /// epilogue joins the citations against (upstream runs the
    /// `IgnoreValidator` over the *unfiltered* diagnostic list,
    /// library_analyzer.dart:331-341, so a citation is "used" exactly when
    /// it dropped a hit). One scratch per run, shared by every `Check`.
    suppressed: &'a core::cell::RefCell<Vec<(u32, &'static str)>>,
    /// Ancestor path, root first, current node last.
    pub path: &'a [u32],
    /// The wildcard local/parameter declaration table (`no_wildcard_variable_
    /// uses`; empty unless that rule runs with the wildcard feature off).
    pub wildcards: &'a [WildcardScope],
    /// The rule being dispatched.
    pub rule: RuleId,
    /// Shared direct-children scratch (one buffer per run, refilled per
    /// call — no per-node allocation).
    direct: &'a core::cell::RefCell<Vec<u32>>,
    /// The run's lazily-built declaration caches ([`RunCaches`]); the
    /// whole-unit scans the declaration rules read, built once per run.
    pub(crate) caches: &'a RunCaches,
    /// The unit's semantic query face (`None` under the plain [`run`]).
    semantics: Option<&'a Analysis<'a>>,
    out: &'a mut Vec<Lint>,
}

/// Fills `buf` with the direct children of `id`, ascending, via the
/// canonical subtree tiling of the completion window (see [`run`] and
/// `tile_direct`).
fn fill_direct(ast: &SyntaxTable, id: u32, buf: &mut Vec<u32>) {
    tile_direct(ast, id, buf);
    buf.reverse();
}

impl<'a> Check<'a> {
    /// The current node.
    ///
    /// # Panics
    ///
    /// Panics when the path is empty — cannot happen for a live `Check`
    /// (every run starts at the root); the `expect` names that invariant.
    #[must_use]
    pub fn node(&self) -> u32 {
        *self.path.last().expect("check always has a current node")
    }

    /// The `i`-th ancestor from the current node (`0` = current, `1` =
    /// parent, ...), `None` past the root.
    #[must_use]
    pub fn ancestor(&self, depth: usize) -> Option<u32> {
        self.path.len().checked_sub(depth + 1).map(|i| self.path[i])
    }

    #[must_use]
    pub fn kind(&self, id: u32) -> NodeKind {
        self.unit.ast.kind(id)
    }

    /// The unit's semantic query face — one `?`-guard away for the semantic
    /// rules ([`Analysis`], mounted through [`Unit::run_with_semantics`]).
    /// `None` under the plain [`run`]: the rule stays silent, because a
    /// semantic rule that is enabled while the face is absent is a
    /// driver-configuration error surfaced at the driver, never a
    /// rule-side fallback shape.
    #[must_use]
    pub fn semantics(&self) -> Option<&'a Analysis<'a>> {
        self.semantics
    }

    /// The library type-use counts (#1640, [`Unit::library_type_uses`]) —
    /// one entry per name lexeme that heads a named-type row anywhere in
    /// the owning library's units, the `_CountVisitor` face the
    /// `avoid_private_typedef_functions` check reads. `None` — a run
    /// without the driver-mounted face — keeps the consuming rule silent.
    #[must_use]
    pub fn library_type_uses(&self) -> Option<&'a rustc_hash::FxHashMap<&'a str, u32>> {
        self.unit.library_type_uses
    }

    /// The completion window of `id` — node ids whose construction happened
    /// inside `id`'s begin..end. Contains descendants; use
    /// [`Check::direct_children`] when direct children are meant (or the
    /// window-O(1) forms: an empty window has no descendants, and the last
    /// window entry is always the last direct child).
    #[must_use]
    pub fn children(&self, id: u32) -> core::ops::Range<u32> {
        self.unit.ast.children(id)
    }

    /// The direct children of `id`, ascending, in the shared scratch.
    ///
    /// # The materialize-first contract
    ///
    /// The scratch is ONE buffer per run, so the returned borrow is the only
    /// legal view of it: it must not stay alive across any nested children
    /// read — the next refill would clobber the buffer under the guard. Code
    /// whose children must survive a nested read materializes them first
    /// through [`Check::collect_direct`] (an owned copy) — the rule the
    /// rule-side comments call "collect first". Debug builds assert the
    /// contract here and name the violation; release keeps the `RefCell`
    /// guard as the backstop (the refill panics on the live borrow).
    #[expect(
        clippy::must_use_candidate,
        reason = "adding `#[must_use]` would mint new warnings at every \
                  out-of-crate call site; outside this warning sweep"
    )]
    pub fn direct_children(&self, id: u32) -> core::cell::Ref<'_, Vec<u32>> {
        debug_assert!(
            self.direct.try_borrow().is_ok(),
            "the shared children scratch is already checked out: \
             materialize with `collect_direct` before a nested children read"
        );
        fill_direct(self.unit.ast, id, &mut self.direct.borrow_mut());
        self.direct.borrow()
    }

    /// [`Check::direct_children`] as an owned vec — the materialize-first
    /// contract's escape hatch: it never touches the shared scratch, so the
    /// result survives any nested children read (and any nested
    /// `direct_children` borrow). Use it whenever children are held across a
    /// call that may itself read children; prefer [`Check::direct_children`]
    /// otherwise (no allocation).
    #[expect(
        clippy::must_use_candidate,
        reason = "adding `#[must_use]` would mint new warnings at every \
                  out-of-crate call site; outside this warning sweep"
    )]
    pub fn collect_direct(&self, id: u32) -> Vec<u32> {
        let mut buf = Vec::new();
        fill_direct(self.unit.ast, id, &mut buf);
        buf
    }

    #[must_use]
    pub fn payload(&self, id: u32) -> &[u32] {
        self.unit.ast.payload(id)
    }

    /// The node's payload word named by `field` — the named read face over
    /// [`Check::payload`] (see [`SyntaxTable::field`]); the raw word, with
    /// [`NONE_TOKEN`] filtering left to the caller. `None` only on a
    /// kind/field mismatch, which the debug assert names.
    #[must_use]
    pub fn field(&self, id: u32, field: PayloadField) -> Option<u32> {
        self.unit.ast.field(id, field)
    }

    /// [`Check::field`] for token-valued words: `None` when the word is
    /// absent or carries the [`dartq_ast::TOKEN_NONE`] sentinel — the named
    /// face of [`Check::payload_word`].
    #[must_use]
    pub fn field_token(&self, id: u32, field: PayloadField) -> Option<u32> {
        let stored = self
            .field(id, field)
            .filter(|&stored| stored != dartq_ast::TOKEN_NONE);
        debug_assert!(
            stored.is_some(),
            "payload field {field:?} of {:?} row {id} is absent or NONE_TOKEN",
            self.unit.ast.kind(id),
        );
        stored
    }

    /// The token id at the head of `id`'s span.
    #[must_use]
    pub fn head_token(&self, id: u32) -> u32 {
        self.unit.ast.token_range(id).start
    }

    #[must_use]
    pub fn lexeme(&self, token: u32) -> &'a str {
        self.unit.tokens.lexeme(self.unit.source, token as usize)
    }

    #[must_use]
    pub fn token_is_synthetic(&self, token: u32) -> bool {
        self.unit
            .tokens
            .get(token as usize)
            .is_some_and(dartq_scanner::Token::is_synthetic)
    }

    /// The `token` table entry, `None` when `token` lies outside the table.
    ///
    /// The unified table resolves every id an AST row can carry after the
    /// parser's `append_inserted` fold (dartq-parser `lib.rs`, single choke
    /// point at parse end), so a `None` here is a stale-id parser bug (an id
    /// held across a cursor reset/detach), never a user-input face — the
    /// debug_assert keeps the contract loud in debug builds while release
    /// degrades to the caller's `let-else`/`?` (a skipped verdict or a
    /// dropped report anchor, never wrong output).
    #[must_use]
    pub fn token(&self, token: u32) -> Option<&'a dartq_scanner::Token> {
        let entry = self.unit.tokens.get(token as usize);
        debug_assert!(
            entry.is_some(),
            "AST-carried token id {token} outside the unit's folded token table"
        );
        entry
    }

    /// Payload word `word` of `id` as a token id, `None` when the payload
    /// is absent, short, or carries the `NONE_TOKEN` sentinel.
    ///
    /// `patch()` writes operator/name tokens into word 0 of the operator-
    /// shape rows (`BinaryExpression`/`Assignment`/`OperatorName`/…), so a
    /// `None` under a kind-dispatched read is a parser layout bug — the
    /// same debug-loud / release-degrade contract as [`Check::token`].
    /// Reads that legitimately store a non-token discriminant in word 0
    /// (the `Identifier` context stamps, `MemberKind`) keep reading
    /// [`Check::payload`] directly.
    #[must_use]
    pub fn payload_word(&self, id: u32, word: usize) -> Option<u32> {
        let stored = self
            .unit
            .ast
            .payload(id)
            .get(word)
            .copied()
            .filter(|&stored| stored != dartq_ast::TOKEN_NONE);
        debug_assert!(
            stored.is_some(),
            "payload word {word} of {:?} row {id} is absent or NONE_TOKEN",
            self.unit.ast.kind(id),
        );
        stored
    }

    /// The UTF-16 start offset of `token`, `None` on [`Check::token`] miss.
    /// The common shape of the token-anchored report sites.
    #[must_use]
    pub fn utf16_of_token(&self, token: u32) -> Option<u32> {
        self.token(token).map(|t| t.utf16_offset)
    }

    /// The exclusive UTF-16 end of `token`'s lexeme, `None` on
    /// [`Check::token`] miss.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "utf16 lengths fit u32 by the SourceRange convention"
    )]
    #[must_use]
    pub fn utf16_end_of_token(&self, token: u32) -> Option<u32> {
        self.token(token)
            .map(|t| t.utf16_offset + self.lexeme(token).encode_utf16().count() as u32)
    }

    /// Byte span of `id` (raw source coordinates, for slicing lexemes).
    #[must_use]
    pub fn byte_span(&self, id: u32) -> core::ops::Range<usize> {
        byte_span(self.unit.tokens, self.unit.ast, id)
    }

    /// UTF-16 span of `id` — the engine's external coordinate convention.
    #[must_use]
    pub fn utf16_span(&self, id: u32) -> core::ops::Range<u32> {
        utf16_span(self.unit.source, self.unit.tokens, self.unit.ast, id)
    }

    /// Reports the rule's official code at `range` (UTF-16), filling the
    /// `{n}` slots with `args`. Suppressed hits are dropped here
    /// (`IgnoreInfo.ignored`, line of the hit's start).
    pub fn report(&mut self, range: core::ops::Range<u32>, args: &[&str]) {
        let spec = rules()[self.rule.0 as usize];
        self.emit(range, spec.problem, spec.correction, args);
    }

    /// Reports with explicit texts — for the rules whose official code has
    /// two message variants under one name (field/local splits). Texts are
    /// passed verbatim from the official templates.
    pub fn report_texts(
        &mut self,
        range: core::ops::Range<u32>,
        problem: &str,
        correction: &str,
        args: &[&str],
    ) {
        self.emit(range, problem, correction, args);
    }

    fn emit(
        &mut self,
        range: core::ops::Range<u32>,
        problem: &str,
        correction: &str,
        args: &[&str],
    ) {
        let name = rules()[self.rule.0 as usize].name;
        let (line, _) = self
            .unit
            .lines
            .line_col_utf16(self.unit.source, range.start);
        // The suppression gate with the `cannot-ignore` override: a code in
        // the set survives every ignore comment (library_analyzer.dart
        // :552-563 — "don't allow un-ignorable codes to be ignored"). A
        // dropped hit marks its citation used for the `unnecessary_ignore`
        // epilogue — the official match pass removes the names of every
        // reported (pre-filter) diagnostic from the citation lists
        // (ignore_validator.dart:137-144).
        if self.ignore.ignored(line, name) && !self.unignorable.contains(name) {
            self.suppressed.borrow_mut().push((line, name));
            return;
        }
        self.out.push(Lint {
            rule: self.rule,
            range,
            message: crate::registry::render(problem, args),
            correction: (!correction.is_empty()).then(|| crate::registry::render(correction, args)),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::rule_by_name;
    use crate::testsupport::{CONFIG, Fixture, one_rule, tok, unit_of};
    use dartq_ast::{AstBuilder, NodeKind};
    use dartq_scanner::scan;
    use dartq_source::FileTable;
    use std::path::Path;

    #[test]
    fn traversal_is_pre_order_and_reports_suppress() {
        // Two hits in one unit; `// ignore:` above the second line drops
        // only that hit. Comments are not tokenized, so the token table is
        // if(0)((1)b(2))(3);(4)else(5);(6) | if(7)((8)b(9))(10);(11)else(12);(13).
        let source = "if (b) ; else ;\n// ignore: avoid_empty_else\nif (b) ; else ;\n";
        let mut files = FileTable::default();
        let scanned = scan(source, CONFIG, files.intern(Path::new("t.dart")));
        let tokens = scanned.tokens;
        // Trees: two if-statements, each with an empty else.
        let mut b = AstBuilder::new();
        b.begin(NodeKind::CompilationUnit, 0);
        // if #1: if(0) ((1) b(2) )(3) ;(4) else(5) ;(6)
        b.begin(NodeKind::IfStatement, 0);
        b.patch(0, 5);
        b.begin(NodeKind::ParenthesizedCondition, 1);
        b.patch(0, 1);
        b.patch(1, 3);
        b.end(NodeKind::ParenthesizedCondition, 4);
        b.leaf(NodeKind::EmptyStatement, 4);
        b.leaf(NodeKind::EmptyStatement, 6);
        b.end(NodeKind::IfStatement, 7);
        // if #2: if(7) ((8) b(9) )(10) ;(11) else(12) ;(13)
        b.begin(NodeKind::IfStatement, 7);
        b.patch(0, 12);
        b.begin(NodeKind::ParenthesizedCondition, 8);
        b.patch(0, 8);
        b.patch(1, 10);
        b.end(NodeKind::ParenthesizedCondition, 11);
        b.leaf(NodeKind::EmptyStatement, 11);
        b.leaf(NodeKind::EmptyStatement, 13);
        b.end(NodeKind::IfStatement, 14);
        b.end(NodeKind::CompilationUnit, 14);
        let ast = b.finish();
        let lines = LineIndex::new(source);

        let unit = Unit {
            source,
            tokens: &tokens,
            ast: &ast,
            lines: &lines,
            path: None,
            features: Features::default(),
            version: None,
            workspace: None,
            reachability: None,
            async_context_uses: None,
            library_type_uses: None,
        };
        let id = rule_by_name("avoid_empty_else").unwrap();
        let mut enabled = EnableSet::none();
        enabled.add(id);
        let hits = run(&unit, &enabled, &rustc_hash::FxHashSet::default());
        assert_eq!(hits.len(), 1, "the suppressed else must not report");
        assert_eq!(hits[0].rule, id);
        // The surviving hit is the FIRST if's else (`;` at token 6):
        // pre-order output.
        assert_eq!(hits[0].range.start, utf16_offset(&tokens, 6));
    }

    fn utf16_offset(tokens: &TokenTable, index: usize) -> u32 {
        tokens.get(index).expect("token").utf16_offset
    }

    #[test]
    fn unignorable_names_survive_the_ignore_gate() {
        // The same suppressed second hit, with the name in the unignorable
        // set: the row emits anyway (`_filterIgnoredDiagnostics`,
        // library_analyzer.dart:547-563 — unignorable codes survive every
        // ignore suppression).
        let source = "if (b) ; else ;\n// ignore: avoid_empty_else\nif (b) ; else ;\n";
        let f = Fixture::build(
            source,
            unit_of(|b, _| {
                // if #1: if(0)((1)b(2))(3);(4)else(5);(6)
                b.begin(NodeKind::IfStatement, 0);
                b.patch(0, 5);
                b.begin(NodeKind::ParenthesizedCondition, 1);
                b.patch(0, 1);
                b.patch(1, 3);
                b.end(NodeKind::ParenthesizedCondition, 4);
                b.leaf(NodeKind::EmptyStatement, 4);
                b.leaf(NodeKind::EmptyStatement, 6);
                b.end(NodeKind::IfStatement, 7);
                // if #2: if(7)((8)b(9))(10);(11)else(12);(13)
                b.begin(NodeKind::IfStatement, 7);
                b.patch(0, 12);
                b.begin(NodeKind::ParenthesizedCondition, 8);
                b.patch(0, 8);
                b.patch(1, 10);
                b.end(NodeKind::ParenthesizedCondition, 11);
                b.leaf(NodeKind::EmptyStatement, 11);
                b.leaf(NodeKind::EmptyStatement, 13);
                b.end(NodeKind::IfStatement, 14);
            }),
        );
        let unit = Unit {
            source: &f.source,
            tokens: &f.tokens,
            ast: &f.ast,
            lines: &f.lines,
            path: Some(&f.path),
            features: f.features,
            version: f.version,
            workspace: None,
            reachability: None,
            async_context_uses: None,
            library_type_uses: None,
        };
        let id = rule_by_name("avoid_empty_else").unwrap();
        let mut enabled = EnableSet::none();
        enabled.add(id);
        let unignorable = rustc_hash::FxHashSet::from_iter(["avoid_empty_else".to_owned()]);
        let lints = run(&unit, &enabled, &unignorable);
        assert_eq!(lints.len(), 2, "the ignore no longer suppresses: {lints:?}");
        assert!(lints.iter().all(|lint| lint.rule == id));
    }

    #[test]
    fn empty_enable_set_is_a_no_op() {
        let f = Fixture::build("if (b) ;", unit_of(|_, _| {}));
        assert!(f.run(&EnableSet::none()).is_empty());
    }

    /// The epilogue's unignorable face: a citation of a `cannot-ignore`
    /// name never reports unnecessary — the official unignorable branch
    /// removes the name from the citation lists before the unnecessary walk
    /// (ignore_validator.dart:89/:113/:164-174). The armed shape: the
    /// unignorable row survives suppression at the emit gate and the
    /// citation stays out of the epilogue.
    #[test]
    fn unignorable_citations_never_report_unnecessary() {
        let f = Fixture::parse("void f() {\n  if (b) ; else ; // ignore: avoid_empty_else\n}\n");
        let unit = Unit {
            source: &f.source,
            tokens: &f.tokens,
            ast: &f.ast,
            lines: &f.lines,
            path: Some(&f.path),
            features: f.features,
            version: f.version,
            workspace: None,
            reachability: None,
            async_context_uses: None,
            library_type_uses: None,
        };
        let mut enabled = EnableSet::none();
        enabled.add(rule_by_name("unnecessary_ignore").expect("registered"));
        enabled.add(rule_by_name("avoid_empty_else").expect("registered"));
        let unignorable = rustc_hash::FxHashSet::from_iter(["avoid_empty_else".to_owned()]);
        let lints = run(&unit, &enabled, &unignorable);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].rule,
            rule_by_name("avoid_empty_else").expect("registered")
        );
    }

    /// The late-join face (#1353): a `remove_deprecations_in_breaking_
    /// versions` citation above a breaking-release annotation stays silent
    /// from `unnecessary_ignore` — the rule's rows join the mounting
    /// shell's walk ([`run_breaking_version_annotations`]) after this run
    /// returns, so this run's suppression record can never hold them, yet
    /// the citation genuinely suppresses in the final list the official
    /// validator walks unfiltered (`library_analyzer.dart`:331-341). A
    /// normal rule's unused citation still reports.
    #[test]
    fn late_join_citations_stay_silent_from_unnecessary_ignore() {
        let f = Fixture::parse(
            "// ignore: remove_deprecations_in_breaking_versions\n\
             // ignore: avoid_empty_else\n\
             @Deprecated('cleanup')\n\
             var x = 0;\n",
        );
        let unit = Unit {
            source: &f.source,
            tokens: &f.tokens,
            ast: &f.ast,
            lines: &f.lines,
            path: Some(&f.path),
            features: f.features,
            version: f.version,
            workspace: None,
            reachability: None,
            async_context_uses: None,
            library_type_uses: None,
        };
        let mut enabled = EnableSet::none();
        enabled.add(rule_by_name("unnecessary_ignore").expect("registered"));
        enabled.add(rule_by_name("remove_deprecations_in_breaking_versions").expect("registered"));
        enabled.add(rule_by_name("avoid_empty_else").expect("registered"));
        let lints = run(&unit, &enabled, &rustc_hash::FxHashSet::default());
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].rule,
            rule_by_name("unnecessary_ignore").expect("registered")
        );
        assert!(
            lints[0].message.contains("avoid_empty_else"),
            "{:?}",
            lints[0].message
        );
    }

    /// Builds the fixture-plus-unit pair the analyzer-half tests share: one
    /// fixture bound to `$f` and the unit to `$u`, both in the caller's
    /// scope (the unit borrows the fixture; macro hygiene demands both
    /// idents passed).
    macro_rules! analyzer_unit {
        ($f:ident, $u:ident, $source:expr, $path:expr) => {
            let $f = Fixture::parse($source);
            let $u = Unit {
                source: &$f.source,
                tokens: &$f.tokens,
                ast: &$f.ast,
                lines: &$f.lines,
                path: $path,
                features: $f.features,
                version: $f.version,
                workspace: None,
                reachability: None,
                async_context_uses: None,
                library_type_uses: None,
            };
        };
    }

    /// The #2116 POSITIVE face: an ignore naming a known analyzer diagnostic
    /// code that produced nothing here reports at the name token — the
    /// official validator admits a non-lint name the diagnostic registry
    /// knows and falls through to the unnecessary walk
    /// (ignore_validator.dart:208-213, :284-288).
    #[test]
    fn analyzer_code_citation_reports_when_nothing_suppresses() {
        analyzer_unit!(
            f,
            unit,
            "// ignore: unused_local_variable\nvoid f() {}\n",
            Some(Path::new("lint_fixture.dart"))
        );
        let rows = unnecessary_ignore_analyzer_rows(
            &unit,
            &one_rule("unnecessary_ignore"),
            &rustc_hash::FxHashSet::default(),
            &[],
            |name| name == "unused_local_variable",
            |_| None,
        );
        assert_eq!(rows.len(), 1, "{rows:?}");
        // The name token: `// ignore: ` is 11 chars, the name 21.
        assert_eq!(rows[0].range, 11..32);
        assert!(rows[0].message.contains("unused_local_variable"));
    }

    /// The two official templates of the shared name: a line-scope
    /// citation reports the `unnecessaryIgnore` wording, a file-scope
    /// (`ignore_for_file:`) one the `unnecessaryIgnoreFile` wording
    /// (`messages.yaml`:15138/:15174) — the #2116 analyzer half's
    /// `UnnecessaryIgnoreTest/test_file` face.
    #[test]
    fn file_scope_citation_uses_the_in_file_template() {
        analyzer_unit!(
            f,
            unit,
            "// ignore_for_file: unused_local_variable\nvoid f() {}\n",
            Some(Path::new("lint_fixture.dart"))
        );
        let rows = unnecessary_ignore_analyzer_rows(
            &unit,
            &one_rule("unnecessary_ignore"),
            &rustc_hash::FxHashSet::default(),
            &[],
            |name| name == "unused_local_variable",
            |_| None,
        );
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(
            rows[0].message,
            "The diagnostic 'unused_local_variable' isn't produced in this file so it \
             doesn't need to be ignored."
        );
    }

    /// The #2116 NEGATIVE faces: a citation the analyzer layer consumed
    /// stays silent — line-scoped when a diagnostic with that code sat on
    /// the cited line (the standalone comment's target), file-scoped when
    /// one sat anywhere under an `ignore_for_file:` (the official match
    /// pass removes the name from the line's and the file's lists,
    /// ignore_validator.dart:137-144).
    #[test]
    fn analyzer_code_citation_silenced_when_produced() {
        analyzer_unit!(
            f1,
            unit,
            "// ignore: unused_local_variable\nvoid f() {}\n",
            Some(Path::new("lint_fixture.dart"))
        );
        // The standalone comment's entry scope is the next line (2).
        assert!(
            unnecessary_ignore_analyzer_rows(
                &unit,
                &one_rule("unnecessary_ignore"),
                &rustc_hash::FxHashSet::default(),
                &[(2, "unused_local_variable")],
                |name| name == "unused_local_variable",
                |_| None,
            )
            .is_empty()
        );
        drop(unit);
        analyzer_unit!(
            f2,
            unit,
            "// ignore_for_file: unused_local_variable\nvoid f() {}\n",
            Some(Path::new("lint_fixture.dart"))
        );
        assert!(
            unnecessary_ignore_analyzer_rows(
                &unit,
                &one_rule("unnecessary_ignore"),
                &rustc_hash::FxHashSet::default(),
                &[(2, "unused_local_variable")],
                |name| name == "unused_local_variable",
                |_| None,
            )
            .is_empty()
        );
    }

    /// Ownership: the plain run never reports an analyzer-code citation —
    /// its suppression record structurally cannot hold analyzer drops, so
    /// the verdict belongs to the shell's analyzer half (the #1353
    /// posture). A bare `run` stays silent.
    #[test]
    fn analyzer_codes_never_report_from_the_plain_run() {
        analyzer_unit!(
            f,
            unit,
            "// ignore: unused_local_variable\nvoid f() {}\n",
            Some(Path::new("lint_fixture.dart"))
        );
        assert!(
            run(
                &unit,
                &one_rule("unnecessary_ignore"),
                &rustc_hash::FxHashSet::default()
            )
            .is_empty()
        );
    }

    /// The plugin bail survives: a name neither registry knows never
    /// reports, whatever the record holds (ignore_validator.dart:209-213).
    #[test]
    fn unknown_analyzer_names_stay_silent() {
        analyzer_unit!(
            f,
            unit,
            "// ignore: undefined_diagnostic_code\nvoid f() {}\n",
            Some(Path::new("lint_fixture.dart"))
        );
        assert!(
            unnecessary_ignore_analyzer_rows(
                &unit,
                &one_rule("unnecessary_ignore"),
                &rustc_hash::FxHashSet::default(),
                &[],
                |name| name == "unused_local_variable",
                |_| None,
            )
            .is_empty()
        );
    }

    /// Routing: a lint-name citation never answers in the analyzer half —
    /// the lint registry admits it first and the ownership seam sends it to
    /// the run or late half.
    #[test]
    fn lint_name_citations_stay_out_of_the_analyzer_half() {
        analyzer_unit!(
            f,
            unit,
            "// ignore: avoid_empty_else\nvoid f() {}\n",
            Some(Path::new("lint_fixture.dart"))
        );
        assert!(
            unnecessary_ignore_analyzer_rows(
                &unit,
                &one_rule("unnecessary_ignore"),
                &rustc_hash::FxHashSet::default(),
                &[],
                |_| true,
                |_| None,
            )
            .is_empty()
        );
    }

    /// The official mount's generated-file filter
    /// (`library_analyzer.dart`:331-334 `whereNot(isGenerated)`,
    /// `file_paths.dart:89-104`): the whole validator is silent on
    /// generated files.
    #[test]
    fn generated_files_stay_silent() {
        analyzer_unit!(
            f,
            unit,
            "// ignore: unused_local_variable\nvoid f() {}\n",
            Some(Path::new("lib/test.g.dart"))
        );
        assert!(
            unnecessary_ignore_analyzer_rows(
                &unit,
                &one_rule("unnecessary_ignore"),
                &rustc_hash::FxHashSet::default(),
                &[],
                |name| name == "unused_local_variable",
                |_| None,
            )
            .is_empty()
        );
    }

    /// The #2136 citation-match leg (#2131's deferred consumer): a
    /// `uniqueName` citation IS an analyzer-code citation — the admission
    /// probe admits it (name OR uniqueName, `ignore_info.dart`:39-52), and
    /// when the canonical-named diagnostic was suppressed the record's
    /// `(line, canonical)` pair exists — yet the REPORT filter stays
    /// name-only (`ignore_validator.dart`:208-213), so the uniqueName
    /// entry never reports `unnecessary_ignore` — suppressed or not —
    /// exactly as upstream.
    #[test]
    fn unique_name_citation_admitted_by_probe_but_never_reports() {
        analyzer_unit!(
            f,
            unit,
            "// ignore: invalid_null_aware_operator_after_short_circuit\n\
             void f() {}\n",
            Some(Path::new("lint_fixture.dart"))
        );
        let known = |name: &str| name == "invalid_null_aware_operator";
        let canonical = |name: &str| {
            (name == "invalid_null_aware_operator_after_short_circuit")
                .then_some("invalid_null_aware_operator" as &'static str)
        };
        // The joined probe admits the file (only a uniqueName is cited):
        // the face does NOT early-return — it runs the walk and finds the
        // entry skipped by the name-only report filter.
        assert!(
            unnecessary_ignore_analyzer_rows(
                &unit,
                &one_rule("unnecessary_ignore"),
                &rustc_hash::FxHashSet::default(),
                &[(2, "invalid_null_aware_operator")],
                known,
                canonical,
            )
            .is_empty()
        );
        // Same verdict with an empty record: the upstream
        // valid-names bail keeps an unused uniqueName citation silent too.
        assert!(
            unnecessary_ignore_analyzer_rows(
                &unit,
                &one_rule("unnecessary_ignore"),
                &rustc_hash::FxHashSet::default(),
                &[],
                known,
                canonical,
            )
            .is_empty()
        );
    }

    /// Asserts the canonical dispatch set over the whole arena: walked from
    /// the root through the tiling, every row is reached exactly once.
    fn assert_exactly_once(ast: &SyntaxTable) {
        let root = ast.root().expect("non-empty unit");
        let mut kids = Vec::new();
        let mut dispatched = vec![false; ast.len()];
        let mut stack = vec![root];
        dispatched[root as usize] = true;
        let mut count = 1usize;
        while let Some(node) = stack.pop() {
            tile_direct(ast, node, &mut kids);
            for &child in &kids {
                assert!(
                    !dispatched[child as usize],
                    "row {child} ({:?}) dispatched twice",
                    ast.kind(child)
                );
                dispatched[child as usize] = true;
                count += 1;
                stack.push(child);
            }
        }
        assert_eq!(count, ast.len(), "a row was never dispatched");
    }

    /// The switch-member windows dispatch canonically. `end_switch_case`
    /// snapshots the next member's segment start on the previous member's
    /// own arena row, so the plain tiling walk spills the first member's
    /// body as block-level children and loses the member row. Pinned here:
    /// the block level sees every member exactly once, the first member's
    /// children are its case expression and body, the borrowing member's
    /// children exclude the snapped row, and the whole arena tiles
    /// exactly-once.
    #[test]
    fn switch_member_windows_tile_exactly_once() {
        let f = Fixture::parse(
            r"
void f(bool c) {
  switch (c) {
    case true:
      continue L;
    L:
    case false:
      print(1);
  }
}
",
        );
        let ast = &f.ast;
        let block = (0..narrow_u32(ast.len()))
            .find(|&row| ast.kind(row) == NodeKind::SwitchBlock)
            .expect("the fixture has a switch block");
        let mut members = Vec::new();
        fill_direct(ast, block, &mut members);
        let kinds: Vec<NodeKind> = members.iter().map(|&row| ast.kind(row)).collect();
        assert_eq!(kinds, [NodeKind::SwitchCase, NodeKind::SwitchCase]);
        let [first, second] = members[..] else {
            panic!("two members asserted above")
        };
        // The first member's children: its case expression and body — the
        // rows the spill used to dispatch at block level.
        let mut kid_rows = Vec::new();
        fill_direct(ast, first, &mut kid_rows);
        let kid_kinds: Vec<NodeKind> = kid_rows.iter().map(|&row| ast.kind(row)).collect();
        assert_eq!(
            kid_kinds,
            [
                NodeKind::CaseExpression,
                NodeKind::Identifier,
                NodeKind::Continue
            ]
        );
        // The second member's window starts at the first member's row: the
        // tiling excludes that row and covers only the second member's own
        // material (the `L:` label pair, the `false` case expression) and
        // body.
        let mut second_kids = Vec::new();
        fill_direct(ast, second, &mut second_kids);
        assert!(!second_kids.contains(&first), "{second_kids:?}");
        assert_eq!(ast.kind(second_kids[0]), NodeKind::Identifier);
        assert_eq!(
            ast.kind(*second_kids.last().expect("non-empty")),
            NodeKind::ExpressionStatement
        );
        assert_exactly_once(ast);
    }

    /// Three members chain the borrow (each member's window starts at the
    /// one before it): the block level still sees all three in source order,
    /// and the tiling stays exactly-once across the arena.
    #[test]
    fn three_member_switch_tiles_every_member() {
        let f = Fixture::parse(
            r"
void f(bool c) {
  switch (c) {
    case true:
      print(1);
    case false:
      print(2);
    default:
      print(3);
  }
}
",
        );
        let ast = &f.ast;
        let block = (0..narrow_u32(ast.len()))
            .find(|&row| ast.kind(row) == NodeKind::SwitchBlock)
            .expect("the fixture has a switch block");
        let mut members = Vec::new();
        fill_direct(ast, block, &mut members);
        let kinds: Vec<NodeKind> = members.iter().map(|&row| ast.kind(row)).collect();
        assert_eq!(
            kinds,
            [
                NodeKind::SwitchCase,
                NodeKind::SwitchCase,
                NodeKind::SwitchCase
            ]
        );
        assert_exactly_once(ast);
    }

    /// A default-first member: no row completes between the block head and
    /// the case event (a bare `default:` consumes only tokens), so the head
    /// snapshot IS the member's first material row. Pinned through the real
    /// bridge: the statement is the case's own child, not a block-level
    /// phantom (`parent` walked to the SwitchBlock under the off-by-one
    /// snapshot). The statement's read face is the production flat shape —
    /// `ExpressionStatement` is a span leaf whose expression rows tile at the
    /// member level beside it (the same sibling run a `Block` yields).
    #[test]
    fn default_first_member_claims_its_statement() {
        let f = Fixture::parse(
            r"
void f(int x) {
  switch (x) {
    default:
      a;
  }
}
",
        );
        let ast = &f.ast;
        let block = (0..narrow_u32(ast.len()))
            .find(|&row| ast.kind(row) == NodeKind::SwitchBlock)
            .expect("the fixture has a switch block");
        let mut members = Vec::new();
        fill_direct(ast, block, &mut members);
        let [member] = members[..] else {
            panic!("one member asserted below")
        };
        assert_eq!(ast.kind(member), NodeKind::SwitchCase);
        let mut kids = Vec::new();
        fill_direct(ast, member, &mut kids);
        let kinds: Vec<NodeKind> = kids.iter().map(|&row| ast.kind(row)).collect();
        assert_eq!(
            kinds,
            [
                NodeKind::Identifier,
                NodeKind::NoTypeArguments,
                NodeKind::NoArguments,
                NodeKind::Send,
                NodeKind::ExpressionStatement
            ]
        );
        assert_eq!(ast.parent(kids[0]), Some(member));
        assert_eq!(ast.parent(*kids.last().expect("non-empty")), Some(member));
        assert_exactly_once(ast);
    }

    /// A labeled-first member: the label's `Identifier` completes before the
    /// case event too, so the head snapshot must keep it inside the member's
    /// window — the label-name read face (`declares_label`) walks the
    /// member's leading `Identifier`/`Label` children.
    #[test]
    fn labeled_first_member_keeps_the_label_identifier() {
        let f = Fixture::parse(
            r"
void f(int x) {
  switch (x) {
    L:
    case 1:
      break;
  }
}
",
        );
        let ast = &f.ast;
        let block = (0..narrow_u32(ast.len()))
            .find(|&row| ast.kind(row) == NodeKind::SwitchBlock)
            .expect("the fixture has a switch block");
        let mut members = Vec::new();
        fill_direct(ast, block, &mut members);
        let [member] = members[..] else {
            panic!("one member asserted below")
        };
        let mut kids = Vec::new();
        fill_direct(ast, member, &mut kids);
        let kinds: Vec<NodeKind> = kids.iter().map(|&row| ast.kind(row)).collect();
        assert_eq!(
            kinds,
            [
                NodeKind::Identifier,
                NodeKind::Label,
                NodeKind::CaseExpression,
                NodeKind::Break
            ]
        );
        // The name row anchors at the identifier token and parents to the
        // member, so `declares_label` reads `L` off it.
        assert_eq!(
            f.tokens
                .lexeme(&f.source, ast.token_range(kids[0]).start as usize),
            "L"
        );
        assert_eq!(ast.parent(kids[0]), Some(member));
        assert_exactly_once(ast);
    }

    #[test]
    fn unknown_names_are_reported_back() {
        let (set, unknown) = EnableSet::resolve(["avoid_empty_else", "not_a_lint"]);
        assert_eq!(unknown, vec!["not_a_lint"]);
        assert!(set.is_enabled(rule_by_name("avoid_empty_else").unwrap()));
    }

    /// The pattern-binding feed: `var [__, y] = list;` binds `__` through a
    /// `DeclaredVariablePattern` (the name is payload word 0, never an
    /// `Identifier` row), so with the wildcards feature off the `__` use
    /// reports — upstream the binding is a `PatternVariableElement`
    /// (extends `LocalVariableElement`), the official rule's flagged kind.
    /// The report sits at the use; `y` binds normally and stays silent.
    #[test]
    fn pattern_binding_wildcard_use_reports() {
        let source = "f() {\n  var [__, y] = list;\n  __;\n  y;\n}\n";
        let mut f = Fixture::parse(source);
        f.features = Features {
            wildcard_variables: false,
            ..f.features
        };
        let lints = f.run(&one_rule("no_wildcard_variable_uses"));
        assert_eq!(lints.len(), 1, "the `__` use reports; `y` stays silent");
        let use_token = tok(source, &f.tokens, "__", 1);
        assert_eq!(
            lints[0].range.start,
            utf16_offset(&f.tokens, use_token as usize)
        );
    }

    /// The two skip faces of the same shape: the bare `_` pattern element is
    /// a `WildcardPattern` — the parser routes it there (matching the
    /// official parser) and the official scope walk binds no element, so its
    /// use is not a wildcard-variable use — and `_x` is not
    /// all-underscores. Neither binding enters the table.
    #[test]
    fn wildcard_pattern_and_named_bindings_stay_out_of_the_table() {
        let mut f = Fixture::parse("f() {\n  var [_, _x] = list;\n  _;\n  _x;\n}\n");
        f.features = Features {
            wildcard_variables: false,
            ..f.features
        };
        let lints = f.run(&one_rule("no_wildcard_variable_uses"));
        assert!(lints.is_empty(), "no wildcard-variable uses in the fixture");
    }

    /// The scope convention the pattern face carries: a switch-case binding
    /// has no `Block` between the case and the member, so the nearest `Block`
    /// ancestor is the member body — the case-bound `__` use reports, like
    /// the official `PatternVariableElement` scope does.
    #[test]
    fn case_pattern_wildcard_binds_over_the_member() {
        let mut f =
            Fixture::parse("f(x) {\n  switch (x) {\n    case [var __]:\n      __;\n  }\n}\n");
        f.features = Features {
            wildcard_variables: false,
            ..f.features
        };
        let lints = f.run(&one_rule("no_wildcard_variable_uses"));
        assert_eq!(lints.len(), 1, "the case-bound `__` use reports");
    }

    // —— Family T+P collapse pins (the shared token/payload helpers) ————————
    //
    // The helpers are the single fact source for the two families #1714
    // graded P2: `Check::token` (the unified-table lookup behind every
    // `tokens.get(id)` read) and `Check::payload_word` (the word-0 slot the
    // operator-shape nodes patch their token into). Their miss paths carry a
    // `debug_assert!(false, ...)` — a stale id or absent slot is a parser bug
    // by contract, so debug builds still die on it; release degrades to a
    // skipped verdict. These pins cover the *valid* faces end-to-end through
    // rules reading each helper — a regression in the helper flips the
    // report.

    /// `payload_word` over the production parse: `prefer_adjacent_string_
    /// concatenation` reads word 0 of `BinaryExpression` (the `+` token the
    /// bridge patches) and reports at it — the helper resolves the slot to
    /// the `+` token's UTF-16 anchor.
    #[test]
    fn payload_word_resolves_the_operator_token() {
        let source = "var s = 'a' + 'b';\n";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("prefer_adjacent_string_concatenation"));
        assert_eq!(lints.len(), 1, "the `+` reports through payload word 0");
        let plus = tok(source, &f.tokens, "+", 0);
        assert_eq!(lints[0].range.start, utf16_offset(&f.tokens, plus as usize));
        assert_eq!(lints[0].range.end, lints[0].range.start);
    }

    /// `utf16_of_token` over the production parse: `unnecessary_new` reports
    /// at the `new` keyword (the `NewExpression` span head) — the helper
    /// resolves the head id to the keyword's UTF-16 start.
    #[test]
    fn token_lookup_anchors_the_report() {
        let source = "var l = new List();\n";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("unnecessary_new"));
        assert_eq!(lints.len(), 1, "the `new` keyword reports");
        let keyword = tok(source, &f.tokens, "new", 0);
        assert_eq!(
            lints[0].range.start,
            utf16_offset(&f.tokens, keyword as usize)
        );
    }

    /// `payload_word` on the second operator shape: `unnecessary_null_
    /// aware_assignments` reads word 0 of `Assignment` (the `??=` token) —
    /// the same slot the `BinaryExpression` consumers read, resolved through
    /// the one helper.
    #[test]
    fn payload_word_resolves_the_assignment_operator() {
        let source = "void f(int? x) {\n  x ??= null;\n}\n";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("unnecessary_null_aware_assignments"));
        assert_eq!(lints.len(), 1, "the `??=` assignment reports");
    }

    // —— Aggregation dedup pins (the oracle's diagnostic set semantics) ————
    //
    // The oracle's listener collects into a set keyed by (code, offset,
    // length, message) (upstream Dart SDK source,
    // pkg/analyzer/lib/error/listener.dart), so two rule shapes reporting
    // the identical row surface once.

    /// The measured divergence: the core `print` descent and the typed
    /// non-nullable `String.toString()` face both fire one expression and
    /// report the same name span with the same message — one row survives,
    /// anchored at the inner `toString`.
    #[test]
    fn identical_rows_from_two_shapes_collapse_to_one() {
        let source = "void f() {\n  final s = 'x';\n  print(s.toString());\n}\n";
        let f = Fixture::parse(source);
        let lints = f.run_semantic(&one_rule("noop_primitive_operations"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The expression has no effect and can be removed."
        );
        let name = tok(source, &f.tokens, "toString", 0);
        assert_eq!(lints[0].range.start, utf16_offset(&f.tokens, name as usize));
    }

    /// The single-shape control: an `Object` receiver keeps the typed face
    /// silent (only the `print` descent fires), so the one row comes from
    /// emission alone — the collapse never manufactures a count.
    #[test]
    fn object_receiver_descent_reports_once() {
        let f = Fixture::parse("void f() {\n  final o = Object();\n  print(o.toString());\n}\n");
        let lints = f.run_semantic(&one_rule("noop_primitive_operations"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The expression has no effect and can be removed."
        );
    }

    /// The over-dedup guard: rows sharing the message but holding distinct
    /// spans keep their count — the dedup key is the full
    /// (rule, span, message) identity, so message text alone never merges.
    #[test]
    fn distinct_rows_keep_their_count() {
        let f = Fixture::parse("void f(String x, int y) {\n  x.toString();\n  y.toInt();\n}\n");
        let lints = f.run_semantic(&one_rule("noop_primitive_operations"));
        assert_eq!(lints.len(), 2, "{lints:?}");
        assert_ne!(lints[0].range, lints[1].range);
    }

    /// The `Features` fold must equal the registry's own truth, bit for
    /// bit, at every version that decides differently: each bit is
    /// exactly the `enabledIn <= version` predicate over its yaml entry
    /// (clamped at the data horizon like every version-derived face), and
    /// the default is the latest-version fold. The matrix brackets every
    /// shipped boundary the seven stances cross; a new stance bit without
    /// its registry truth — or a flip — fails here.
    #[test]
    fn features_fold_matches_the_registry_truth_across_versions() {
        use dartq_language::{Feature, FeatureSet, LanguageVersion, NEWEST_KNOWN_VERSION};
        const MATRIX: [LanguageVersion; 8] = [
            LanguageVersion::new(2, 12),
            LanguageVersion::new(2, 16),
            LanguageVersion::new(2, 18),
            LanguageVersion::new(2, 19),
            LanguageVersion::new(3, 0),
            LanguageVersion::new(3, 6),
            LanguageVersion::new(3, 7),
            LanguageVersion::new(3, 8),
        ];
        let shipped = |feature: Feature, version: LanguageVersion| {
            let version = if version > NEWEST_KNOWN_VERSION {
                NEWEST_KNOWN_VERSION
            } else {
                version
            };
            feature
                .meta()
                .enabled_in
                .is_some_and(|enabled_in| version >= enabled_in)
        };
        for version in MATRIX {
            let folded = Features::from_feature_set(FeatureSet::for_language_version(version));
            let expected = Features {
                wildcard_variables: shipped(Feature::WildcardVariables, version),
                enhanced_parts: shipped(Feature::EnhancedParts, version),
                unnamed_libraries: shipped(Feature::UnnamedLibraries, version),
                patterns: shipped(Feature::Patterns, version),
                primary_constructors: shipped(Feature::PrimaryConstructors, version),
                null_aware_elements: shipped(Feature::NullAwareElements, version),
                super_parameters: shipped(Feature::SuperParameters, version),
            };
            assert_eq!(folded, expected, "Features fold drifts at {version}");
            // The keyed lookup agrees with the registry truth on every
            // stanced feature (its inert arms answer `false` by stance,
            // pinned by the gate-resolution test in `registry`).
            for feature in [
                Feature::WildcardVariables,
                Feature::EnhancedParts,
                Feature::UnnamedLibraries,
                Feature::Patterns,
                Feature::PrimaryConstructors,
                Feature::NullAwareElements,
                Feature::SuperParameters,
            ] {
                assert_eq!(
                    folded.is_enabled(feature),
                    shipped(feature, version),
                    "is_enabled drifts from the registry truth at {version}"
                );
            }
        }
        // The default is the latest-version fold, and the horizon clamp
        // keeps everything past the vendored data at the latest posture.
        assert_eq!(
            Features::default(),
            Features::from_feature_set(FeatureSet::latest())
        );
        assert_eq!(
            Features::from_feature_set(FeatureSet::for_language_version(LanguageVersion::new(
                3, 15
            ))),
            Features::default(),
        );
    }
}
