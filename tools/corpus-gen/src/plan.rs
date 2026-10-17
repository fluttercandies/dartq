//! Deterministic corpus layout.
//!
//! [`Plan`] turns a [`Scale`] into exact package/file budgets and
//! [`PlanIter`] yields every output file as a [`FileSpec`] in lexicographic
//! path order. The order matters twice: it is the order in which files are
//! written to disk, and the order in which the manifest fingerprint is
//! computed, which keeps both streaming and deterministic without buffering
//! the corpus.

/// Corpus size preset; file counts are exact targets, not estimates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scale {
    /// ~160 files (181 with the overlay): the checked-in golden slice
    /// (tests/corpus/mega-x/slice). Small enough to track in git, large
    /// enough to carry every adversarial shape, every replay-sequence
    /// capture path, and a non-trivial feature/core/app/tests mix.
    ///
    /// The slice is a byte-stability pin: it carries no version-ladder or
    /// workspace files — those axes would move the checked-in tree.
    Slice,
    K1,
    K10,
    K100,
}

impl Scale {
    #[must_use]
    pub fn parse(raw: &str) -> Option<Scale> {
        match raw {
            "slice" => Some(Scale::Slice),
            "1k" => Some(Scale::K1),
            "10k" => Some(Scale::K10),
            "100k" => Some(Scale::K100),
            _ => None,
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Scale::Slice => "slice",
            Scale::K1 => "1k",
            Scale::K10 => "10k",
            Scale::K100 => "100k",
        }
    }

    fn target_files(self) -> usize {
        match self {
            Scale::Slice => 160,
            Scale::K1 => 1_000,
            Scale::K10 => 10_000,
            Scale::K100 => 100_000,
        }
    }
}

/// Top-level layout of the generated tree.
///
/// `Flat` is the original multi-package mega-project (root `app` package +
/// `packages/` path deps). `Workspace` emits a pub-workspace shape instead:
/// a root `workspace:` pubspec, `resolution: workspace` members at varying
/// depths, per-member `.dart_tool/pub/workspace_ref.json`, vendored
/// non-member path deps, and a root `package_config.json` — the
/// `tests/corpus/nested-versions/` shape at generator scale.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Layout {
    #[default]
    Flat,
    Workspace,
}

impl Layout {
    #[must_use]
    pub fn parse(raw: &str) -> Option<Layout> {
        match raw {
            "flat" => Some(Layout::Flat),
            "workspace" => Some(Layout::Workspace),
            _ => None,
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Layout::Flat => "flat",
            Layout::Workspace => "workspace",
        }
    }
}

/// The package-level sdk-constraint ladder (#1812 axis 1d). Packages cycle
/// through these constraints deterministically when the version-ladder face
/// is on; each constraint's lower bound is the package's settled language
/// version (`major.minor`).
pub const SDK_LADDER: [&str; 8] = [
    "^2.12.0", "^2.17.0", "^2.19.0", "^3.0.0", "^3.3.0", "^3.5.0", "^3.13.0", "^3.15.0",
];

/// Lower bound of an [`SDK_LADDER`] constraint as `major.minor` — the
/// language version pub would settle for that package.
///
/// # Panics
///
/// Panics when `constraint` is not shaped `^major.minor.0`; every entry of
/// [`SDK_LADDER`] and [`VENDORED_PACKAGES`] satisfies the shape, so the
/// `expect` pins a table invariant rather than an input check.
#[must_use]
pub fn ladder_version(constraint: &str) -> &str {
    constraint
        .strip_prefix('^')
        .and_then(|v| v.strip_suffix(".0"))
        .expect("SDK_LADDER entries are ^major.minor.0")
}

/// Which package a file belongs to; drives package URIs and pubspecs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageId {
    Core,
    Feature(u32),
}

impl PackageId {
    #[must_use]
    pub fn name(self) -> String {
        match self {
            PackageId::Core => "core".to_string(),
            PackageId::Feature(index) => format!("feature_{index:04}"),
        }
    }
}

/// Import header emitted at the top of a rotation file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Header {
    /// No imports.
    Plain,
    /// `import 'package:core/core.dart';`
    Core,
    /// `import 'package:core/core.dart' deferred as core_lazy;`
    DeferredCore,
    /// `import 'package:feature_NNNN/feature_NNNN.dart';` — a cross-version
    /// import edge (1d): only planned when the version-ladder face is on,
    /// and only into a package whose settled version differs from the
    /// importer's. The payload is the target feature package index.
    CrossVersion(u32),
}

/// A `// @dart = major.minor` language-version banner (1d) carried on a
/// rotation file. The banner is emitted as the file's first comment line —
/// before the generated-file notice — so it sits inside the leading comment
/// block the scanner reads for version markers. `Some(..)` only appears when
/// the version-ladder face is on.
pub type Banner = (u32, u32);

/// Kind of an output file; drives the renderer dispatch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileKind {
    RootPubspec,
    PackagePubspec(PackageId),
    MainEntry,
    Rotation {
        header: Header,
        banner: Option<Banner>,
    },
    FeatureBarrel(PackageId),
    FacetHub(u32),
    FacetStub(u32),
    FacetIo(u32),
    AnchorModel(u32),
    NarrowGateway(u32),
    Gateway(u32),
    WidgetHost {
        package: u32,
        index: usize,
    },
    WidgetPart {
        package: u32,
        index: usize,
    },
    CoreBarrel,
    LazyLib,
    PlatformHub,
    PlatformIo,
    PlatformStub,
    PlatformWeb,
    ChainLink(usize),
    RegistryBox,
    FunctionTypes,
    CoreValueLib,
    TestFile(usize),
    /// Adversarial single file (1a): deterministic pathological shape —
    /// deep nesting, mega-expression, giant switch, cascade or interpolation
    /// chain. `index` selects the shape and size budget.
    Patho(usize),
    /// Unicode edge family file (1b): unicode content in strings, comments
    /// and URI literals. `index` selects the shape mix.
    UnicodeScene(usize),
    /// Recovery-storm file (1c): >= 5 stacked malformed constructs drawn from
    /// the recovery damage taxonomy. `file` indexes within storm package
    /// `package`.
    ErrStorm {
        package: u32,
        file: usize,
    },
    /// Pubspec of one `zadv/errstorm_NNNN` storm package.
    ErrStormPubspec(u32),
    /// `.dart_tool/pub/workspace_ref.json` of a workspace member (1e):
    /// `{ "workspaceRoot": "<rel>" }` pointing back at the tree root.
    /// `depth` is the member directory's depth below the root.
    WorkspaceRef {
        depth: usize,
    },
    /// Root `.dart_tool/package_config.json` — the hand-written shared
    /// resolution covering every member and vendored package (1e).
    RootPackageConfig,
    /// `third_party/vendored_NN/pubspec.yaml` — vendored path dep, NOT a
    /// workspace member (1e). `index` selects its ladder constraint.
    VendoredPubspec(u32),
    /// `third_party/vendored_NN/lib/vendored_NN.dart` (1e).
    VendoredLib(u32),
    /// `analysis_options.yaml` of one package — the `--experiments`
    /// manifest face (1d): `analyzer.enable-experiment` list drawn from a
    /// fixed catalog. `package` selects the package.
    PackageAnalysisOptions(PackageId),
    /// Root `analysis_options.yaml` — the app package's entry of the same
    /// enable-experiment manifest face (1d).
    RootAnalysisOptions,
    /// `packages/flutter/pubspec.yaml` — the in-corpus flutter stub package
    /// (#2063). Only planned when the flutter-app face is on.
    FlutterStubPubspec,
    /// `packages/flutter/lib/widgets.dart` — the stub widget-surface library
    /// (#2063).
    FlutterWidgetsLib,
    /// One `part` file of the flutter stub library (#2063); the payload
    /// indexes [`FLUTTER_PARTS`].
    FlutterStubPart(usize),
}

/// One planned output file.
pub struct FileSpec {
    /// Path relative to the output root, `/`-separated.
    pub path: String,
    pub kind: FileKind,
}

fn spec(path: String, kind: FileKind) -> FileSpec {
    FileSpec { path, kind }
}

/// Fixed (non-rotation) files inside the core package: barrel, lazy lib,
/// registry box, 6 chain links, 4 platform files, function types, core value.
pub const CORE_FIXED: usize = 15;

/// `lib/src` subdirectories of the core package in lexicographic order; the
/// rotation file counts in [`Plan::core_dir_counts`] are indexed by this
/// array.
pub const CORE_DIRS: [&str; 7] = [
    "containers",
    "extensions",
    "interfaces",
    "mixins",
    "patterns",
    "typedefs",
    "values",
];

/// Exact file budgets for one scale.
pub struct Plan {
    pub scale: Scale,
    /// Top-level layout: flat multi-package project or pub workspace (1e).
    pub layout: Layout,
    /// Version-ladder face (1d): feature-package sdk constraints cycle
    /// through [`SDK_LADDER`], a deterministic subset of rotation files
    /// carry `// @dart =` banners (both directions around the package
    /// floor), a per-package `analysis_options.yaml` carries the
    /// `enable-experiment` manifest face, and some rotation files import
    /// across version boundaries.
    pub version_ladder: bool,
    /// Flutter-app face (#2063): the `packages/flutter` stub package is
    /// planned, feature pubspecs path-depend on it, the widget host/part
    /// slots render StatefulWidget/State shapes and every 4th rotation file
    /// renders a widget scene.
    pub flutter_app: bool,
    /// App wiring files under `lib/src/controllers`.
    pub controllers: usize,
    /// App wiring files under `lib/src/screens`.
    pub screens: usize,
    /// Number of feature packages.
    pub packages: u32,
    /// Base files per feature package.
    pub per_package: usize,
    /// Extra files folded into the last feature package.
    pub extra_last_package: usize,
    /// Rotation counts per [`CORE_DIRS`] entry.
    pub core_dir_counts: [usize; 7],
    /// Files under `tests/`.
    pub tests: usize,
    /// Pathological singles under `zadv/patho/` (~0.1% at 100k, floored at 3
    /// so the coverage gate holds at the smallest scale).
    pub patho: usize,
    /// Unicode edge files under `zadv/unicode/` (~1% of the corpus).
    pub unicode: usize,
    /// Number of `zadv/errstorm_NNNN` storm packages (~0.1% of the corpus as
    /// packages; each package is [`ERRSTORM_PACKAGE_FILES`] files).
    pub errstorm_packages: u32,
    /// Workspace-layout extra files (1e): per-member `workspace_ref.json`,
    /// vendored packages (pubspec + one lib file), the root
    /// `package_config.json`, and — with the version ladder — per-package
    /// `analysis_options.yaml`. Zero for `Layout::Flat`.
    pub workspace_extras: usize,
    /// Vendored non-member packages under `third_party/` (1e): two fixed
    /// packages mirroring the nested-versions shape (`ext_lib`-style at
    /// 3.0, `ext_lib_compat`-style at 2.19).
    pub vendored_packages: u32,
    /// Total planned files (excludes `manifest.json` itself).
    pub total_files: usize,
}

/// Files inside one `zadv/errstorm_NNNN` package: `pubspec.yaml` plus this
/// many `lib/` storm sources.
pub const ERRSTORM_LIB_FILES: usize = 6;

/// Total file count of one storm package (lib files + pubspec).
pub const ERRSTORM_PACKAGE_FILES: usize = ERRSTORM_LIB_FILES + 1;

/// Directory of the in-corpus flutter stub package (#2063). It is a plain
/// path dependency of the app and every feature package (never a workspace
/// member), mirroring how the real flutter SDK enters a dependency graph.
pub const FLUTTER_STUB_DIR: &str = "packages/flutter";

/// `part` basenames of the stub library, in lexicographic order — the order
/// both the part declarations and the plan iterator use.
pub const FLUTTER_PARTS: [&str; 3] = ["basic", "foundation", "material"];

/// File count of the flutter stub package: pubspec + `lib/widgets.dart` +
/// one file per [`FLUTTER_PARTS`] entry.
pub const FLUTTER_STUB_FILES: usize = FLUTTER_PARTS.len() + 2;

/// Generation options layered on top of a scale.
///
/// Both faces default OFF: `Plan::build(scale)` — the shape every pre-#1812
/// caller used — keeps emitting the flat, single-version tree, so existing
/// seed outputs stay byte-identical. Turning either face on changes only
/// that run's plan.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// Top-level layout (1e): `Flat` (default) or `Workspace`.
    pub layout: Layout,
    /// Version-ladder face (1d): sdk constraints ladder across packages,
    /// `// @dart =` banners, the enable-experiment manifest face and
    /// cross-version import edges.
    pub version_ladder: bool,
    /// Flutter-app face (#2063): an in-corpus `packages/flutter` stub package
    /// plus real widget-shape code — the widget host/part slots become
    /// StatefulWidget/State pairs building nested stub-widget trees, and
    /// every 4th rotation file becomes a widget scene. Default OFF: the
    /// pre-#2063 shape stays byte-identical.
    pub flutter_app: bool,
}

/// Vendored non-member packages emitted under `third_party/` for the
/// workspace layout (1e). Mirrors the nested-versions corpus's split: one
/// package at 3.0 (`vendored_ext`), one at 2.19 (`vendored_compat`), each a
/// path dep of the root package, never a workspace member.
pub const VENDORED_PACKAGES: [(&str, &str); 2] =
    [("vendored_ext", "^3.0.0"), ("vendored_compat", "^2.19.0")];

/// Experiment names for the per-package `analysis_options.yaml`
/// `enable-experiment` manifest face (1d): real historical experiment ids,
/// selected per package index so the catalog rotates deterministically.
pub const EXPERIMENT_CATALOG: [&str; 6] = [
    "digit-separators",
    "enhanced-parts",
    "macros",
    "null-aware-elements",
    "records",
    "triple-shift",
];

impl Plan {
    /// Builds the file plan for `scale` with default options (flat layout,
    /// no version ladder) — the byte-stable pre-#1812 shape.
    ///
    /// # Panics
    ///
    /// Panics only if the package count exceeds `u32::MAX` — impossible at
    /// every scale (the clamp caps it at 400); the `expect` pins that.
    #[must_use]
    pub fn build(scale: Scale) -> Plan {
        Self::build_with(scale, Options::default())
    }

    /// Builds the file plan for `scale` and `options`.
    ///
    /// # Panics
    ///
    /// Same package-count bound as [`Self::build`].
    #[must_use]
    pub fn build_with(scale: Scale, options: Options) -> Plan {
        let total = scale.target_files();
        let app_total = total / 10; // pubspec.yaml + lib/main.dart + wiring
        let core_total = total * 15 / 100;
        let tests = total * 15 / 100;
        let features = total - app_total - core_total - tests;
        // Adversarial overlay (#1810): emitted strictly AFTER every existing
        // file so the shared RNG stream, the rotation ordinal and the
        // per-package budgets of the base corpus stay bit-identical for
        // existing seeds — the emitted bytes of every pre-overlay file cannot
        // move. New total = target + overlay (the ledger pins per-file bytes,
        // not the manifest fingerprint value).
        // Slice pins one of every adversarial shape (6 patho / 8 unicode)
        // so the checked-in golden covers the whole overlay taxonomy; the
        // larger scales keep their proportional budgets.
        let (patho, unicode, errstorm_packages) = match scale {
            Scale::Slice => (6, 8, 1),
            _ => (
                (total / 1_000).max(3), // ~0.1%, floored for the 1k gate
                total / 100,            // ~1%
                u32::try_from((total / 1_000).max(1)).expect("package count fits u32"),
            ),
        };
        let packages =
            u32::try_from((features / 60).clamp(4, 400)).expect("package count fits u32");
        let per_package = features / packages as usize;
        let extra_last_package = features % packages as usize;
        let wiring = app_total - 2;
        let controllers = wiring.div_ceil(2);
        let screens = wiring - controllers;
        // The core package's pubspec.yaml is part of its budget too.
        let core_rotation = core_total - CORE_FIXED - 1;
        let base = core_rotation / CORE_DIRS.len();
        let remainder = core_rotation % CORE_DIRS.len();
        let core_dir_counts = [
            base + usize::from(remainder > 0),
            base + usize::from(remainder > 1),
            base + usize::from(remainder > 2),
            base + usize::from(remainder > 3),
            base + usize::from(remainder > 4),
            base + usize::from(remainder > 5),
            base + usize::from(remainder > 6),
        ];
        // Workspace extras (1e): per-member workspace_ref.json (core + every
        // feature member), the root package_config.json, and each vendored
        // package's pubspec + single lib file. The version ladder adds one
        // analysis_options.yaml per feature package plus the root (the
        // enable-experiment manifest face). These counters feed
        // `total_files`; the files themselves are planned in the Extras
        // tail phase (lexicographically interleaved paths would break the
        // sorted-order invariant mid-iteration, so they append after
        // `zadv/` — `zextras/` sorts last).
        let workspace_extras = if options.layout == Layout::Workspace {
            // Per-member workspace_ref.json (core + every feature), the
            // root package_config.json, and each vendored package's
            // pubspec + single lib file.
            (packages as usize + 1) + 1 + VENDORED_PACKAGES.len() * 2
        } else {
            0
        };
        let options_files = if options.version_ladder {
            // Root + core + every feature package's analysis_options.yaml.
            packages as usize + 2
        } else {
            0
        };
        // Flutter stub package (#2063): a fixed 5-file package appended to
        // the plan; its content is index-derived only (no RNG draws), so the
        // stub bytes are scale-invariant and the app-gate golden check can
        // pin them.
        let flutter_files = if options.flutter_app {
            FLUTTER_STUB_FILES
        } else {
            0
        };
        Plan {
            scale,
            layout: options.layout,
            version_ladder: options.version_ladder,
            flutter_app: options.flutter_app,
            controllers,
            screens,
            packages,
            per_package,
            extra_last_package,
            core_dir_counts,
            tests,
            patho,
            unicode,
            errstorm_packages,
            workspace_extras: workspace_extras + options_files,
            vendored_packages: u32::try_from(VENDORED_PACKAGES.len())
                .expect("vendored count fits u32"),
            total_files: app_total
                + features
                + core_total
                + tests
                + patho
                + unicode
                + errstorm_packages as usize * ERRSTORM_PACKAGE_FILES
                + workspace_extras
                + options_files
                + flutter_files,
        }
    }

    /// Total files of feature package `index` (the last package absorbs the
    /// distribution remainder).
    #[must_use]
    pub fn package_file_count(&self, index: usize) -> usize {
        let mut count = self.per_package;
        if index + 1 == self.packages as usize {
            count += self.extra_last_package;
        }
        count
    }

    /// Directory of feature package `index` relative to the root (1e): under
    /// `Flat` every member lives at `packages/feature_NNNN`; under
    /// `Workspace` a deterministic subset relocates one level deeper to
    /// `packages/nested/feature_NNNN`, matching the nested-versions corpus's
    /// deep-member face.
    #[must_use]
    pub fn feature_root(&self, index: u32) -> String {
        if self.feature_is_nested(index) {
            format!("packages/nested/feature_{index:04}")
        } else {
            format!("packages/feature_{index:04}")
        }
    }

    /// Whether feature package `index` sits at `packages/nested/` (1e).
    #[must_use]
    pub fn feature_is_nested(&self, index: u32) -> bool {
        self.layout == Layout::Workspace && index % 4 == 1
    }

    /// The feature index visited at package cursor `cursor` (1-based; the
    /// iterator's `self.package`). Under `Flat` the order is the identity;
    /// under `Workspace` every shallow `packages/feature_*` member emits
    /// before any `packages/nested/` member, keeping the plan's global
    /// lexicographic path order (`feature_*` < `nested/`).
    ///
    /// # Panics
    ///
    /// Panics when `cursor` is out of range — the iterator bounds it at
    /// `self.packages`, so a panic means the cursor contract broke.
    #[must_use]
    pub fn feature_index_at(&self, cursor: usize) -> u32 {
        if self.layout == Layout::Flat {
            return u32::try_from(cursor).expect("feature cursor");
        }
        let nested: Vec<u32> = (0..self.packages)
            .filter(|&i| self.feature_is_nested(i))
            .collect();
        let shallow_count = self.packages as usize - nested.len();
        if cursor < shallow_count {
            let mut seen = 0usize;
            for i in 0..self.packages {
                if !self.feature_is_nested(i) {
                    if seen == cursor {
                        return i;
                    }
                    seen += 1;
                }
            }
            unreachable!("cursor inside the shallow range always resolves");
        }
        nested[cursor - shallow_count]
    }

    /// Depth of feature package `index`'s directory below the root — the
    /// `../..` count its `workspace_ref.json` needs (1e).
    #[must_use]
    pub fn feature_depth(&self, index: u32) -> usize {
        if self.feature_is_nested(index) { 3 } else { 2 }
    }

    /// The `environment.sdk` constraint of feature package `index` (1d).
    ///
    /// Flat + ladder: the full [`SDK_LADDER`] rotates by package index —
    /// the corpus does not need to pass `pub get`, so members may sit below
    /// the 3.5 workspace-resolution gate.
    ///
    /// Workspace + ladder: `resolution: workspace` is pub-valid only at
    /// language version >= 3.5 (the nested-versions corpus documents the
    /// gate), so members draw from the `>=3.5` tail of the ladder; the low
    /// end (2.12–3.3) is carried by the vendored non-member packages.
    ///
    /// Ladder off: every package stays at `^3.13.0` (the pre-#1812 pin).
    #[must_use]
    pub fn feature_sdk_constraint(&self, index: u32) -> &'static str {
        if !self.version_ladder {
            return "^3.13.0";
        }
        if self.layout == Layout::Workspace {
            SDK_LADDER[5 + (index as usize % 3)]
        } else {
            SDK_LADDER[index as usize % SDK_LADDER.len()]
        }
    }

    /// The core package's `environment.sdk` constraint (1d): the shared base
    /// stays at the middle of the ladder so feature packages bracket it on
    /// both sides. `^3.5.0` keeps it a legal workspace member too.
    #[must_use]
    pub fn core_sdk_constraint(&self) -> &'static str {
        if self.version_ladder {
            "^3.5.0"
        } else {
            "^3.13.0"
        }
    }

    /// Settled language version (`major.minor`) of feature package `index` —
    /// the lower bound of [`Self::feature_sdk_constraint`], used by the
    /// workspace `package_config.json` and the `// @dart =` banner overlay.
    #[must_use]
    pub fn feature_language_version(&self, index: u32) -> &'static str {
        ladder_version(self.feature_sdk_constraint(index))
    }

    /// Settled language version of the core package.
    #[must_use]
    pub fn core_language_version(&self) -> &'static str {
        ladder_version(self.core_sdk_constraint())
    }

    /// The `// @dart =` banner of feature package `index`'s `file`-th
    /// rotation file (1d). Every 4th rotation file in a ladder run carries
    /// a banner, alternating below/above the package floor: `file % 4 == 0`
    /// pins one step below the package's ladder entry (down to 2.12), `file
    /// % 4 == 2` pins one step above (up to 3.15) — the bidirectional
    /// version-override face. Ladder off: no banners anywhere.
    ///
    /// # Panics
    ///
    /// Never in practice: [`ladder_version`] only panics on a malformed
    /// [`SDK_LADDER`] entry — a table invariant, not an input check.
    #[must_use]
    pub fn feature_file_banner(&self, index: u32, file: usize) -> Option<Banner> {
        if !self.version_ladder || !file.is_multiple_of(4) && file % 4 != 2 {
            return None;
        }
        let position = if self.layout == Layout::Workspace {
            5 + (index as usize % 3)
        } else {
            index as usize % SDK_LADDER.len()
        };
        let rung = if file.is_multiple_of(4) {
            position.saturating_sub(1)
        } else {
            (position + 1).min(SDK_LADDER.len() - 1)
        };
        let version = ladder_version(SDK_LADDER[rung]);
        let (major, minor) = version.split_once('.').expect("major.minor");
        Some((
            major.parse().expect("ladder major"),
            minor.parse().expect("ladder minor"),
        ))
    }

    /// A cross-version import edge for feature package `index`'s `file`-th
    /// rotation file (1d): every 8th file in a ladder run imports the barrel
    /// of a feature package on a different ladder rung (`+3` mod package
    /// count lands on a different rung whenever `packages >= 3`). Ladder
    /// off: no edges.
    #[must_use]
    pub fn feature_cross_version_import(&self, index: u32, file: usize) -> Option<u32> {
        if !self.version_ladder || file % 8 != 1 || self.packages < 3 {
            return None;
        }
        let target = (index + 3) % self.packages;
        if self.feature_language_version(target) == self.feature_language_version(index) {
            return None;
        }
        Some(target)
    }

    /// Experiments listed in the `enable-experiment` face of package
    /// `index`'s `analysis_options.yaml` (1d): two catalog entries rotating
    /// by package index.
    #[must_use]
    pub fn package_experiments(&self, index: usize) -> [&'static str; 2] {
        [
            EXPERIMENT_CATALOG[index % EXPERIMENT_CATALOG.len()],
            EXPERIMENT_CATALOG[(index + 3) % EXPERIMENT_CATALOG.len()],
        ]
    }

    #[must_use]
    pub fn iter(&self) -> PlanIter<'_> {
        PlanIter {
            plan: self,
            phase: Phase::RootFiles,
            package: 0,
            step: 0,
            index: 0,
            part_pending: false,
            part_index: 0,
            pkg_files: 0,
            preamble: 0,
            m: 0,
            v: 0,
            s: 0,
            w: 0,
            widget_emitted: 0,
            core_files: [0; CORE_DIRS.len()],
            rotation_counter: 0,
            pkg_rotation: 0,
        }
    }
}

impl<'a> IntoIterator for &'a Plan {
    type Item = FileSpec;
    type IntoIter = PlanIter<'a>;

    fn into_iter(self) -> PlanIter<'a> {
        self.iter()
    }
}

/// Feature-package `lib/src` rotation budgets derived from the package file
/// count: `(models, repositories, services, widgets)`. The budget also covers
/// the barrel, the 3 facet files, and the pubspec (4 + 1), so a package
/// yields exactly `package_files` entries. `widgets` counts emitted FILES,
/// part siblings included (the widget step is file-count-driven). Invariant:
/// `package_files >= 10`, so `models >= 2` and `services >= 1`, keeping the
/// anchor/gateway special slots disjoint from plain rotation files.
#[must_use]
pub fn feature_dir_counts(package_files: usize) -> (usize, usize, usize, usize) {
    debug_assert!(package_files >= 10);
    let rest = package_files - 5; // barrel + 3 facet files + pubspec
    let base = rest / 4;
    let remainder = rest % 4;
    (
        base + usize::from(remainder > 0),
        base + usize::from(remainder > 1),
        base + usize::from(remainder > 2),
        base + usize::from(remainder > 3),
    )
}

enum Phase {
    /// Root-level dot/options files, workspace layout only: `.dart_tool/`
    /// sorts before every other root entry, and `analysis_options.yaml`
    /// before `lib/`. Both are skipped (zero iterations) under `Flat`.
    RootFiles,
    AppMain,
    AppDir {
        screens: bool,
    },
    Packages,
    /// The `packages/flutter` stub package (#2063). Under `Flat` it sorts
    /// lexicographically right after the last `packages/feature_*` member.
    /// Under `Workspace` the nested members (`packages/nested/feature_*`)
    /// sort after `packages/flutter`, so the combo emits the stub after
    /// them — emission order is plan order, which is the only thing the
    /// fingerprint contract needs; no flutter-app + workspace consumer
    /// exists.
    FlutterStub,
    RootPubspec,
    Tests,
    /// `third_party/vendored_*/` packages (1e): sorts after `tests/` and
    /// before `zadv/`. Zero iterations under `Flat`.
    ThirdParty,
    /// Adversarial overlay tail: storm packages, then patho, then unicode —
    /// path order inside `zadv/` is lexicographic (`errstorm_*` < `patho` <
    /// `unicode`), matching the emission order.
    Adversarial,
    Done,
}

/// Streaming plan iterator: yields every [`FileSpec`] exactly once, in
/// lexicographic path order, with memory bounded by scalar cursors.
pub struct PlanIter<'a> {
    plan: &'a Plan,
    phase: Phase,
    package: usize,
    step: usize,
    index: usize,
    part_pending: bool,
    part_index: usize,
    pkg_files: usize,
    /// Member preamble cursor (1d/1e): 0 = unstarted, 1 = `workspace_ref`
    /// emitted/skipped, 2 = `analysis_options` emitted/skipped.
    preamble: u8,
    m: usize,
    v: usize,
    s: usize,
    w: usize,
    widget_emitted: usize,
    core_files: [usize; CORE_DIRS.len()],
    rotation_counter: usize,
    /// Rotation files emitted inside the current feature package (1d):
    /// drives the `// @dart =` banner and cross-version-edge selection.
    pkg_rotation: usize,
}

impl Iterator for PlanIter<'_> {
    type Item = FileSpec;

    // The phase table is one sequential dispatch — splitting it would
    // scatter the emission-order contract it documents.
    #[expect(clippy::too_many_lines, reason = "see above")]
    fn next(&mut self) -> Option<FileSpec> {
        loop {
            match self.phase {
                Phase::RootFiles => {
                    // `.dart_tool/package_config.json` and the root
                    // `analysis_options.yaml` — both workspace/ladder-only,
                    // both sorting before `lib/`. `index` counts 0..2.
                    if self.index == 0 && self.plan.layout == Layout::Workspace {
                        self.index += 1;
                        return Some(spec(
                            ".dart_tool/package_config.json".to_string(),
                            FileKind::RootPackageConfig,
                        ));
                    }
                    if self.index <= 1 && self.plan.version_ladder {
                        self.index = 2;
                        return Some(spec(
                            "analysis_options.yaml".to_string(),
                            FileKind::RootAnalysisOptions,
                        ));
                    }
                    self.phase = Phase::AppMain;
                }
                Phase::AppMain => {
                    self.phase = Phase::AppDir { screens: false };
                    self.index = 0;
                    return Some(spec("lib/main.dart".to_string(), FileKind::MainEntry));
                }
                Phase::AppDir { screens } => {
                    let count = if screens {
                        self.plan.screens
                    } else {
                        self.plan.controllers
                    };
                    if self.index < count {
                        let index = self.index;
                        self.index += 1;
                        let rot = if screens {
                            self.plan.controllers + index
                        } else {
                            index
                        };
                        let header = if rot % 16 == 7 {
                            Header::DeferredCore
                        } else if rot % 4 == 0 {
                            Header::Core
                        } else {
                            Header::Plain
                        };
                        let (dir, base) = if screens {
                            ("screens", "screen")
                        } else {
                            ("controllers", "controller")
                        };
                        return Some(spec(
                            format!("lib/src/{dir}/{base}_{index:05}.dart"),
                            FileKind::Rotation {
                                header,
                                banner: None,
                            },
                        ));
                    }
                    if screens {
                        self.phase = Phase::Packages;
                        self.package = 0;
                        self.step = 0;
                        self.index = 0;
                        self.part_pending = false;
                    } else {
                        self.phase = Phase::AppDir { screens: true };
                        self.index = 0;
                    }
                }
                Phase::Packages => {
                    if self.package > self.plan.packages as usize {
                        self.phase = if self.plan.flutter_app {
                            Phase::FlutterStub
                        } else {
                            Phase::RootPubspec
                        };
                        self.step = 0;
                        continue;
                    }
                    if let Some(item) = self.next_package_file() {
                        return Some(item);
                    }
                    self.package += 1;
                    self.step = 0;
                    self.index = 0;
                    self.part_pending = false;
                    self.preamble = 0;
                }
                Phase::FlutterStub => {
                    // The stub's five files in lexicographic path order:
                    // lib/src/<part>.dart x3, lib/widgets.dart, pubspec.yaml.
                    if self.step < FLUTTER_STUB_FILES {
                        let step = self.step;
                        self.step += 1;
                        if step < FLUTTER_PARTS.len() {
                            return Some(spec(
                                format!("{FLUTTER_STUB_DIR}/lib/src/{}.dart", FLUTTER_PARTS[step]),
                                FileKind::FlutterStubPart(step),
                            ));
                        }
                        if step == FLUTTER_PARTS.len() {
                            return Some(spec(
                                format!("{FLUTTER_STUB_DIR}/lib/widgets.dart"),
                                FileKind::FlutterWidgetsLib,
                            ));
                        }
                        return Some(spec(
                            format!("{FLUTTER_STUB_DIR}/pubspec.yaml"),
                            FileKind::FlutterStubPubspec,
                        ));
                    }
                    self.phase = Phase::RootPubspec;
                }
                Phase::RootPubspec => {
                    self.phase = Phase::Tests;
                    self.index = 0;
                    return Some(spec("pubspec.yaml".to_string(), FileKind::RootPubspec));
                }
                Phase::Tests => {
                    if self.index < self.plan.tests {
                        let index = self.index;
                        self.index += 1;
                        return Some(spec(
                            format!("tests/unit_{index:05}_test.dart"),
                            FileKind::TestFile(index),
                        ));
                    }
                    self.phase = Phase::ThirdParty;
                    self.index = 0;
                    self.step = 0;
                    self.package = 0;
                }
                Phase::ThirdParty => {
                    // Vendored packages (1e): `lib/vendored_NN.dart` before
                    // `pubspec.yaml`, `vendored_compat` before `vendored_ext`
                    // (lexicographic), so the iterator walks the sorted
                    // (name, path) order rather than VENDORED_PACKAGES order.
                    if self.plan.layout == Layout::Workspace {
                        let mut vendored: Vec<&str> =
                            VENDORED_PACKAGES.iter().map(|(name, _)| *name).collect();
                        vendored.sort_unstable();
                        if self.package < vendored.len() {
                            let index = self.package;
                            let name = vendored[index];
                            let root = format!("third_party/{name}");
                            if self.step == 0 {
                                self.step = 1;
                                return Some(spec(
                                    format!("{root}/lib/{name}.dart"),
                                    FileKind::VendoredLib(
                                        u32::try_from(index).expect("vendored index"),
                                    ),
                                ));
                            }
                            self.package += 1;
                            self.step = 0;
                            return Some(spec(
                                format!("{root}/pubspec.yaml"),
                                FileKind::VendoredPubspec(
                                    u32::try_from(index).expect("vendored index"),
                                ),
                            ));
                        }
                    }
                    self.phase = Phase::Adversarial;
                    self.index = 0;
                    self.step = 0;
                    self.package = 0;
                }
                Phase::Adversarial => {
                    if let Some(item) = self.next_adversarial_file() {
                        return Some(item);
                    }
                    self.phase = Phase::Done;
                }
                Phase::Done => return None,
            }
        }
    }
}

impl PlanIter<'_> {
    fn next_package_file(&mut self) -> Option<FileSpec> {
        // Package cursor 0 is `core` (sorts before `feature_NNNN`), cursor
        // `1..=packages` are the feature packages.
        if self.package == 0 {
            self.next_core_file()
        } else {
            self.next_feature_file()
        }
    }

    fn rotation(&mut self) -> FileKind {
        let header = if self.rotation_counter.is_multiple_of(4) {
            Header::Core
        } else {
            Header::Plain
        };
        self.rotation_counter += 1;
        FileKind::Rotation {
            header,
            banner: None,
        }
    }

    /// Feature-package rotation (1d): the shared rotation stream is
    /// untouched — banner and cross-version-edge selections layer on top,
    /// keyed by the per-package rotation ordinal. `None` fields keep every
    /// flat non-ladder file byte-identical.
    fn feature_rotation(&mut self, package_index: u32) -> FileKind {
        let file = self.pkg_rotation;
        self.pkg_rotation += 1;
        let FileKind::Rotation { header, .. } = self.rotation() else {
            unreachable!("rotation() always yields FileKind::Rotation");
        };
        let banner = self.plan.feature_file_banner(package_index, file);
        let header = self
            .plan
            .feature_cross_version_import(package_index, file)
            .map_or(header, Header::CrossVersion);
        FileKind::Rotation { header, banner }
    }

    /// Steps through one feature package. Step transitions loop internally:
    /// `None` is returned only after the pubspec, so the caller never
    /// misreads a mid-package transition as end-of-package.
    // The step arms mirror the emitted file order (member preamble, barrel,
    // facets, models, repositories, ...); a split would scatter the
    // transition table.
    #[expect(clippy::too_many_lines, reason = "see above")]
    fn next_feature_file(&mut self) -> Option<FileSpec> {
        let package_index = self.plan.feature_index_at(self.package - 1);
        let root = self.plan.feature_root(package_index);
        // Member preamble (1d/1e): `.dart_tool/pub/workspace_ref.json` and
        // `analysis_options.yaml` sort before `lib/` — they are the
        // package's first emitted files, each gated on its own layout face
        // so the flat default emits neither.
        if self.preamble == 0 {
            self.preamble = 1;
            if self.plan.layout == Layout::Workspace {
                return Some(spec(
                    format!("{root}/.dart_tool/pub/workspace_ref.json"),
                    FileKind::WorkspaceRef {
                        depth: self.plan.feature_depth(package_index),
                    },
                ));
            }
        }
        if self.preamble == 1 {
            self.preamble = 2;
            if self.plan.version_ladder {
                return Some(spec(
                    format!("{root}/analysis_options.yaml"),
                    FileKind::PackageAnalysisOptions(PackageId::Feature(package_index)),
                ));
            }
        }
        if self.step == 0 && self.index == 0 && !self.part_pending {
            self.pkg_files = self.plan.package_file_count(package_index as usize);
            let (m, v, s, w) = feature_dir_counts(self.pkg_files);
            self.m = m;
            self.v = v;
            self.s = s;
            self.w = w;
            self.widget_emitted = 0;
            self.pkg_rotation = 0;
        }
        loop {
            match self.step {
                0 => {
                    self.step = 1;
                    return Some(spec(
                        format!("{root}/lib/feature_{package_index:04}.dart"),
                        FileKind::FeatureBarrel(PackageId::Feature(package_index)),
                    ));
                }
                1 => {
                    self.step = 2;
                    return Some(spec(
                        format!("{root}/lib/src/facets/accel.dart"),
                        FileKind::FacetHub(package_index),
                    ));
                }
                // Facet trio emits in lexicographic name order: accel.dart <
                // accel_io.dart < accel_stub.dart ('.' sorts before '_').
                2 => {
                    self.step = 3;
                    return Some(spec(
                        format!("{root}/lib/src/facets/accel_io.dart"),
                        FileKind::FacetIo(package_index),
                    ));
                }
                3 => {
                    self.step = 4;
                    return Some(spec(
                        format!("{root}/lib/src/facets/accel_stub.dart"),
                        FileKind::FacetStub(package_index),
                    ));
                }
                4 => {
                    if self.index < self.m {
                        let i = self.index;
                        self.index += 1;
                        let kind = if i == 0 {
                            FileKind::AnchorModel(package_index)
                        } else if i == self.m - 1 {
                            FileKind::NarrowGateway(package_index)
                        } else {
                            self.feature_rotation(package_index)
                        };
                        return Some(spec(
                            format!("{root}/lib/src/models/model_{i:05}.dart"),
                            kind,
                        ));
                    }
                    self.step = 5;
                    self.index = 0;
                }
                5 => {
                    if self.index < self.v {
                        let i = self.index;
                        self.index += 1;
                        return Some(spec(
                            format!("{root}/lib/src/repositories/repository_{i:05}.dart"),
                            self.feature_rotation(package_index),
                        ));
                    }
                    self.step = 6;
                    self.index = 0;
                }
                6 => {
                    if self.index < self.s {
                        let i = self.index;
                        self.index += 1;
                        let kind = if i == self.s - 1 {
                            FileKind::Gateway(package_index)
                        } else {
                            self.feature_rotation(package_index)
                        };
                        return Some(spec(
                            format!("{root}/lib/src/services/service_{i:05}.dart"),
                            kind,
                        ));
                    }
                    self.step = 7;
                    self.index = 0;
                }
                // The widget budget counts FILES, part siblings included:
                // hosts sit at every 3rd index and emit a `part` sibling, so
                // a pair only starts when two file slots remain; a trailing
                // single slot emits a plain rotation, keeping the package
                // total exact.
                7 => {
                    if self.part_pending {
                        self.part_pending = false;
                        let i = self.part_index;
                        self.index += 1;
                        return Some(spec(
                            format!("{root}/lib/src/widgets/widget_{i:05}_part.dart"),
                            FileKind::WidgetPart {
                                package: package_index,
                                index: i,
                            },
                        ));
                    }
                    if self.widget_emitted < self.w {
                        let i = self.index;
                        if i % 3 == 2 && self.widget_emitted + 2 <= self.w {
                            self.part_pending = true;
                            self.part_index = i;
                            self.widget_emitted += 2;
                            return Some(spec(
                                format!("{root}/lib/src/widgets/widget_{i:05}.dart"),
                                FileKind::WidgetHost {
                                    package: package_index,
                                    index: i,
                                },
                            ));
                        }
                        self.index += 1;
                        self.widget_emitted += 1;
                        return Some(spec(
                            format!("{root}/lib/src/widgets/widget_{i:05}.dart"),
                            self.feature_rotation(package_index),
                        ));
                    }
                    self.step = 8;
                }
                8 => {
                    self.step = 9;
                    return Some(spec(
                        format!("{root}/pubspec.yaml"),
                        FileKind::PackagePubspec(PackageId::Feature(package_index)),
                    ));
                }
                _ => return None,
            }
        }
    }

    /// Steps through the core package; same loop-until-exhausted contract as
    /// [`Self::next_feature_file`].
    #[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
    fn next_core_file(&mut self) -> Option<FileSpec> {
        // Core is a workspace member too (1d/1e): same preamble rule as the
        // feature members — the ref/options files sort before `lib/`.
        if self.preamble == 0 {
            self.preamble = 1;
            if self.plan.layout == Layout::Workspace {
                return Some(spec(
                    "packages/core/.dart_tool/pub/workspace_ref.json".to_string(),
                    FileKind::WorkspaceRef { depth: 2 },
                ));
            }
        }
        if self.preamble == 1 {
            self.preamble = 2;
            if self.plan.version_ladder {
                return Some(spec(
                    "packages/core/analysis_options.yaml".to_string(),
                    FileKind::PackageAnalysisOptions(PackageId::Core),
                ));
            }
        }
        if self.step == 0 && self.index == 0 {
            self.core_files = self.plan.core_dir_counts;
        }
        let base = "packages/core/lib";
        loop {
            match self.step {
                0 => {
                    self.step = 1;
                    return Some(spec(format!("{base}/core.dart"), FileKind::CoreBarrel));
                }
                1 => {
                    self.step = 2;
                    return Some(spec(format!("{base}/lazy.dart"), FileKind::LazyLib));
                }
                2 => {
                    if let Some(item) = self.rotation_dir(0, "containers", "container") {
                        return Some(item);
                    }
                }
                3 => {
                    self.step = 4;
                    self.index = 0;
                    return Some(spec(
                        format!("{base}/src/containers/registry_box.dart"),
                        FileKind::RegistryBox,
                    ));
                }
                4 => {
                    if let Some(item) = self.rotation_dir(1, "extensions", "extension") {
                        return Some(item);
                    }
                }
                5 => {
                    if self.index < 6 {
                        let i = self.index;
                        self.index += 1;
                        return Some(spec(
                            format!("{base}/src/interfaces/chain_link_{i}.dart"),
                            FileKind::ChainLink(i),
                        ));
                    }
                    self.step = 6;
                    self.index = 0;
                }
                6 => {
                    if let Some(item) = self.rotation_dir(2, "interfaces", "interface") {
                        return Some(item);
                    }
                }
                7 => {
                    if let Some(item) = self.rotation_dir(3, "mixins", "mixin") {
                        return Some(item);
                    }
                }
                8 => {
                    if let Some(item) = self.rotation_dir(4, "patterns", "pattern") {
                        return Some(item);
                    }
                }
                9 => {
                    if self.index < 4 {
                        let i = self.index;
                        self.index += 1;
                        let (name, kind) = match i {
                            0 => ("platform.dart", FileKind::PlatformHub),
                            1 => ("platform_io.dart", FileKind::PlatformIo),
                            2 => ("platform_stub.dart", FileKind::PlatformStub),
                            _ => ("platform_web.dart", FileKind::PlatformWeb),
                        };
                        return Some(spec(format!("{base}/src/platform/{name}"), kind));
                    }
                    self.step = 10;
                    self.index = 0;
                }
                10 => {
                    self.step = 11;
                    return Some(spec(
                        format!("{base}/src/typedefs/function_types.dart"),
                        FileKind::FunctionTypes,
                    ));
                }
                11 => {
                    if let Some(item) = self.rotation_dir(5, "typedefs", "typedef") {
                        return Some(item);
                    }
                }
                12 => {
                    self.step = 13;
                    return Some(spec(
                        format!("{base}/src/values/core_value.dart"),
                        FileKind::CoreValueLib,
                    ));
                }
                13 => {
                    if let Some(item) = self.rotation_dir(6, "values", "value") {
                        return Some(item);
                    }
                }
                14 => {
                    self.step = 15;
                    return Some(spec(
                        "packages/core/pubspec.yaml".to_string(),
                        FileKind::PackagePubspec(PackageId::Core),
                    ));
                }
                _ => return None,
            }
        }
    }

    /// Steps through the adversarial overlay in `zadv/` path order:
    /// `errstorm_NNNN/` packages (each `lib/storm_*.dart` then `pubspec.yaml`),
    /// then `patho/patho_*.dart`, then `unicode/uni_*.dart`. All overlay paths
    /// sort after `tests/` so no existing file's emission position moves.
    fn next_adversarial_file(&mut self) -> Option<FileSpec> {
        loop {
            match self.step {
                // Storm packages: lib sources first (`lib/` < `pubspec.yaml`).
                0 => {
                    // `errstorm_packages <= 200` at every scale — the
                    // `try_from` pins that bound instead of a truncating cast.
                    if (self.package as u64) < u64::from(self.plan.errstorm_packages) {
                        let package =
                            u32::try_from(self.package).expect("storm package count fits u32");
                        let root = format!("zadv/errstorm_{package:04}");
                        if self.index < ERRSTORM_LIB_FILES {
                            let file = self.index;
                            self.index += 1;
                            return Some(spec(
                                format!("{root}/lib/storm_{file:05}.dart"),
                                FileKind::ErrStorm { package, file },
                            ));
                        }
                        self.index = 0;
                        self.package += 1;
                        return Some(spec(
                            format!("{root}/pubspec.yaml"),
                            FileKind::ErrStormPubspec(package),
                        ));
                    }
                    self.step = 1;
                    self.index = 0;
                }
                1 => {
                    if self.index < self.plan.patho {
                        let index = self.index;
                        self.index += 1;
                        return Some(spec(
                            format!("zadv/patho/patho_{index:05}.dart"),
                            FileKind::Patho(index),
                        ));
                    }
                    self.step = 2;
                    self.index = 0;
                }
                2 => {
                    if self.index < self.plan.unicode {
                        let index = self.index;
                        self.index += 1;
                        return Some(spec(
                            format!("zadv/unicode/uni_{index:05}.dart"),
                            FileKind::UnicodeScene(index),
                        ));
                    }
                    return None;
                }
                _ => return None,
            }
        }
    }

    /// Emits the rotation files of one core `lib/src` subdirectory; directory
    /// order follows [`CORE_DIRS`], keeping the global path sort valid.
    fn rotation_dir(&mut self, slot: usize, dir: &str, base: &str) -> Option<FileSpec> {
        if self.index < self.core_files[slot] {
            let i = self.index;
            self.index += 1;
            return Some(spec(
                format!("packages/core/lib/src/{dir}/{base}_{i:05}.dart"),
                self.rotation(),
            ));
        }
        self.step += 1;
        self.index = 0;
        None
    }
}
