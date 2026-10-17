//! Feature-matrix coverage tracking.
//!
//! Every [`Cell`] corresponds to one entry of the corpus feature matrix
//! (class modifiers, patterns, async shapes, ...). The generator increments a
//! counter each time a shape is emitted, including the structural cells
//! (part/part-of pairs, conditional imports, cross-file factory redirects)
//! that are emitted by the planner rather than by per-file scenes. The unit
//! tests assert every cell is covered at least three times at the 1k scale.

/// One entry of the corpus feature matrix.
///
/// The first 28 variants are per-file scenes scheduled round-robin across
/// rotation files; the remaining 7 are structural cells incremented by the
/// special-file renderers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cell {
    ClassModifiers,
    GenericClass,
    MixinApply,
    MixinConstraint,
    ExtensionType,
    ExtensionDeclare,
    EnumPlain,
    EnumEnhanced,
    RecordValues,
    Operators,
    FactoryCtor,
    ConstCtor,
    ExternalDecl,
    PatternObject,
    PatternRecord,
    PatternListMap,
    PatternRelational,
    PatternLogical,
    CollectionControlFlow,
    CoreValueUse,
    NullSafety,
    LabeledBreak,
    AsyncFuture,
    AsyncStream,
    TypedefFunctionType,
    Cascade,
    StringInterpolation,
    Numbers,
    InheritanceDeep,
    ConditionalImport,
    DeferredImport,
    PartOfPair,
    ExportCombinators,
    FactoryRedirectCross,
    PackageUriImport,
    /// Adversarial pathological single file (1a).
    PathoSingle,
    /// Unicode edge content in strings/comments/URI literals (1b).
    UnicodeContent,
    /// Recovery-storm stacked-damage file (1c).
    RecoveryStorm,
    /// `// @dart = major.minor` language-version banner (1d).
    DartVersionBanner,
    /// Cross-version package import edge (1d).
    CrossVersionImport,
    /// `analyzer.enable-experiment` manifest face (1d).
    EnableExperiment,
    /// Pub-workspace member artifact: `resolution: workspace` pubspec,
    /// `workspace_ref.json` or the shared root `package_config.json` (1e).
    WorkspaceLayout,
}

/// Number of matrix cells.
pub const CELL_COUNT: usize = 42;

/// Scene cells are the first [`SCENE_CELL_COUNT`] entries of [`CELL_LIST`];
/// the rotation scheduler cycles through them.
pub const SCENE_CELL_COUNT: usize = 28;

/// Cells present in every run regardless of faces — the first
/// [`UNCONDITIONAL_CELL_COUNT`] entries of [`CELL_LIST`]. The four
/// version/workspace cells (1d/1e) are face-gated: they only move when
/// `--layout workspace` or `--version-ladder` is on, so the default-run
/// coverage gate asserts only the unconditional range.
pub const UNCONDITIONAL_CELL_COUNT: usize = 38;

/// Index of the gated scene cell (requires a core package import).
pub const CELL_CORE_VALUE_USE: usize = 19;

/// All cells in declaration order; scene scheduling indexes into this array.
pub const CELL_LIST: [Cell; CELL_COUNT] = [
    Cell::ClassModifiers,
    Cell::GenericClass,
    Cell::MixinApply,
    Cell::MixinConstraint,
    Cell::ExtensionType,
    Cell::ExtensionDeclare,
    Cell::EnumPlain,
    Cell::EnumEnhanced,
    Cell::RecordValues,
    Cell::Operators,
    Cell::FactoryCtor,
    Cell::ConstCtor,
    Cell::ExternalDecl,
    Cell::PatternObject,
    Cell::PatternRecord,
    Cell::PatternListMap,
    Cell::PatternRelational,
    Cell::PatternLogical,
    Cell::CollectionControlFlow,
    Cell::CoreValueUse,
    Cell::NullSafety,
    Cell::LabeledBreak,
    Cell::AsyncFuture,
    Cell::AsyncStream,
    Cell::TypedefFunctionType,
    Cell::Cascade,
    Cell::StringInterpolation,
    Cell::Numbers,
    Cell::InheritanceDeep,
    Cell::ConditionalImport,
    Cell::DeferredImport,
    Cell::PartOfPair,
    Cell::ExportCombinators,
    Cell::FactoryRedirectCross,
    Cell::PackageUriImport,
    Cell::PathoSingle,
    Cell::UnicodeContent,
    Cell::RecoveryStorm,
    Cell::DartVersionBanner,
    Cell::CrossVersionImport,
    Cell::EnableExperiment,
    Cell::WorkspaceLayout,
];

/// Stable names for the cells, in [`CELL_LIST`] order (used by tests and
/// diagnostics).
pub const CELL_NAMES: [&str; CELL_COUNT] = [
    "class_modifiers",
    "generic_class",
    "mixin_apply",
    "mixin_constraint",
    "extension_type",
    "extension_declare",
    "enum_plain",
    "enum_enhanced",
    "record_values",
    "operators",
    "factory_ctor",
    "const_ctor",
    "external_decl",
    "pattern_object",
    "pattern_record",
    "pattern_list_map",
    "pattern_relational",
    "pattern_logical",
    "collection_control_flow",
    "core_value_use",
    "null_safety",
    "labeled_break",
    "async_future",
    "async_stream",
    "typedef_function_type",
    "cascade",
    "string_interpolation",
    "numbers",
    "inheritance_deep",
    "conditional_import",
    "deferred_import",
    "part_of_pair",
    "export_combinators",
    "factory_redirect_cross",
    "package_uri_import",
    "patho_single",
    "unicode_content",
    "recovery_storm",
    "dart_version_banner",
    "cross_version_import",
    "enable_experiment",
    "workspace_layout",
];

/// Per-cell emission counters collected during one generation run.
#[derive(Debug)]
pub struct Coverage {
    counts: [u32; CELL_COUNT],
}

impl Coverage {
    #[must_use]
    pub fn new() -> Self {
        Self {
            counts: [0; CELL_COUNT],
        }
    }

    pub fn hit(&mut self, cell: Cell) {
        self.counts[cell as usize] += 1;
    }

    #[must_use]
    pub fn count(&self, cell: Cell) -> u32 {
        self.counts[cell as usize]
    }

    /// Lowest per-cell count; the coverage gate compares this against the
    /// per-cell minimum.
    ///
    /// # Panics
    ///
    /// Panics if `CELL_COUNT` is zero — excluded by construction; the
    /// `expect` pins that invariant.
    #[must_use]
    pub fn minimum(&self) -> u32 {
        self.counts
            .iter()
            .min()
            .copied()
            .expect("CELL_COUNT is nonzero")
    }
}

impl Default for Coverage {
    fn default() -> Self {
        Self::new()
    }
}
