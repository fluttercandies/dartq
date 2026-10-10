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
    K1,
    K10,
    K100,
}

impl Scale {
    pub fn parse(raw: &str) -> Option<Scale> {
        match raw {
            "1k" => Some(Scale::K1),
            "10k" => Some(Scale::K10),
            "100k" => Some(Scale::K100),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Scale::K1 => "1k",
            Scale::K10 => "10k",
            Scale::K100 => "100k",
        }
    }

    fn target_files(self) -> usize {
        match self {
            Scale::K1 => 1_000,
            Scale::K10 => 10_000,
            Scale::K100 => 100_000,
        }
    }
}

/// Which package a file belongs to; drives package URIs and pubspecs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageId {
    Core,
    Feature(u32),
}

impl PackageId {
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
}

/// Kind of an output file; drives the renderer dispatch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileKind {
    RootPubspec,
    PackagePubspec(PackageId),
    MainEntry,
    Rotation {
        header: Header,
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
    /// Total planned files (excludes `manifest.json` itself).
    pub total_files: usize,
}

impl Plan {
    pub fn build(scale: Scale) -> Plan {
        let total = scale.target_files();
        let app_total = total / 10; // pubspec.yaml + lib/main.dart + wiring
        let core_total = total * 15 / 100;
        let tests = total * 15 / 100;
        let features = total - app_total - core_total - tests;
        let packages = ((features / 60).max(4).min(400)) as u32;
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
        Plan {
            scale,
            controllers,
            screens,
            packages,
            per_package,
            extra_last_package,
            core_dir_counts,
            tests,
            total_files: app_total + features + core_total + tests,
        }
    }

    /// Total files of feature package `index` (the last package absorbs the
    /// distribution remainder).
    pub fn package_file_count(&self, index: usize) -> usize {
        let mut count = self.per_package;
        if index + 1 == self.packages as usize {
            count += self.extra_last_package;
        }
        count
    }

    pub fn iter(&self) -> PlanIter<'_> {
        PlanIter {
            plan: self,
            phase: Phase::AppMain,
            package: 0,
            step: 0,
            index: 0,
            part_pending: false,
            part_index: 0,
            pkg_files: 0,
            m: 0,
            v: 0,
            s: 0,
            w: 0,
            widget_emitted: 0,
            core_files: [0; CORE_DIRS.len()],
            rotation_counter: 0,
        }
    }
}

/// Feature-package `lib/src` rotation budgets derived from the package file
/// count: `(models, repositories, services, widgets)`. The budget also covers
/// the barrel, the 3 facet files, and the pubspec (4 + 1), so a package
/// yields exactly `package_files` entries. `widgets` counts emitted FILES,
/// part siblings included (the widget step is file-count-driven). Invariant:
/// `package_files >= 10`, so `models >= 2` and `services >= 1`, keeping the
/// anchor/gateway special slots disjoint from plain rotation files.
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
    AppMain,
    AppDir { screens: bool },
    Packages,
    RootPubspec,
    Tests,
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
    m: usize,
    v: usize,
    s: usize,
    w: usize,
    widget_emitted: usize,
    core_files: [usize; CORE_DIRS.len()],
    rotation_counter: usize,
}

impl<'a> Iterator for PlanIter<'a> {
    type Item = FileSpec;

    fn next(&mut self) -> Option<FileSpec> {
        loop {
            match self.phase {
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
                            FileKind::Rotation { header },
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
                        self.phase = Phase::RootPubspec;
                        continue;
                    }
                    if let Some(item) = self.next_package_file() {
                        return Some(item);
                    }
                    self.package += 1;
                    self.step = 0;
                    self.index = 0;
                    self.part_pending = false;
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
                    self.phase = Phase::Done;
                }
                Phase::Done => return None,
            }
        }
    }
}

impl<'a> PlanIter<'a> {
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
        let header = if self.rotation_counter % 4 == 0 {
            Header::Core
        } else {
            Header::Plain
        };
        self.rotation_counter += 1;
        FileKind::Rotation { header }
    }

    /// Steps through one feature package. Step transitions loop internally:
    /// `None` is returned only after the pubspec, so the caller never
    /// misreads a mid-package transition as end-of-package.
    fn next_feature_file(&mut self) -> Option<FileSpec> {
        let p = (self.package - 1) as u32;
        let root = format!("packages/feature_{p:04}");
        if self.step == 0 && self.index == 0 && !self.part_pending {
            self.pkg_files = self.plan.package_file_count(self.package - 1);
            let (m, v, s, w) = feature_dir_counts(self.pkg_files);
            self.m = m;
            self.v = v;
            self.s = s;
            self.w = w;
            self.widget_emitted = 0;
        }
        loop {
            match self.step {
                0 => {
                    self.step = 1;
                    return Some(spec(
                        format!("{root}/lib/feature_{p:04}.dart"),
                        FileKind::FeatureBarrel(PackageId::Feature(p)),
                    ));
                }
                1 => {
                    self.step = 2;
                    return Some(spec(format!("{root}/lib/src/facets/accel.dart"), FileKind::FacetHub(p)));
                }
                // Facet trio emits in lexicographic name order: accel.dart <
                // accel_io.dart < accel_stub.dart ('.' sorts before '_').
                2 => {
                    self.step = 3;
                    return Some(spec(
                        format!("{root}/lib/src/facets/accel_io.dart"),
                        FileKind::FacetIo(p),
                    ));
                }
                3 => {
                    self.step = 4;
                    return Some(spec(
                        format!("{root}/lib/src/facets/accel_stub.dart"),
                        FileKind::FacetStub(p),
                    ));
                }
                4 => {
                    if self.index < self.m {
                        let i = self.index;
                        self.index += 1;
                        let kind = if i == 0 {
                            FileKind::AnchorModel(p)
                        } else if i == self.m - 1 {
                            FileKind::NarrowGateway(p)
                        } else {
                            self.rotation()
                        };
                        return Some(spec(format!("{root}/lib/src/models/model_{i:05}.dart"), kind));
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
                            self.rotation(),
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
                            FileKind::Gateway(p)
                        } else {
                            self.rotation()
                        };
                        return Some(spec(format!("{root}/lib/src/services/service_{i:05}.dart"), kind));
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
                            FileKind::WidgetPart { package: p, index: i },
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
                                FileKind::WidgetHost { package: p, index: i },
                            ));
                        }
                        self.index += 1;
                        self.widget_emitted += 1;
                        return Some(spec(
                            format!("{root}/lib/src/widgets/widget_{i:05}.dart"),
                            self.rotation(),
                        ));
                    }
                    self.step = 8;
                }
                8 => {
                    self.step = 9;
                    return Some(spec(format!("{root}/pubspec.yaml"), FileKind::PackagePubspec(PackageId::Feature(p))));
                }
                _ => return None,
            }
        }
    }

    /// Steps through the core package; same loop-until-exhausted contract as
    /// [`Self::next_feature_file`].
    fn next_core_file(&mut self) -> Option<FileSpec> {
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
                    return Some(spec(format!("{base}/src/containers/registry_box.dart"), FileKind::RegistryBox));
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
                    return Some(spec(format!("{base}/src/typedefs/function_types.dart"), FileKind::FunctionTypes));
                }
                11 => {
                    if let Some(item) = self.rotation_dir(5, "typedefs", "typedef") {
                        return Some(item);
                    }
                }
                12 => {
                    self.step = 13;
                    return Some(spec(format!("{base}/src/values/core_value.dart"), FileKind::CoreValueLib));
                }
                13 => {
                    if let Some(item) = self.rotation_dir(6, "values", "value") {
                        return Some(item);
                    }
                }
                14 => {
                    self.step = 15;
                    return Some(spec("packages/core/pubspec.yaml".to_string(), FileKind::PackagePubspec(PackageId::Core)));
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
