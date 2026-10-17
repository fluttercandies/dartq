//! Renders planned files to Dart/YAML text.
//!
//! Rotation files are composed from a fixed catalog of scene functions (one
//! per feature-matrix cell), scheduled round-robin so that consecutive files
//! visit every cell. `core_value_use` is gated: it only renders in files that
//! import the core package.

macro_rules! dw {
    ($dst:expr, $($arg:tt)*) => {
        $dst.write_fmt(format_args!($($arg)*)).expect("writing to a String cannot fail")
    };
}

mod adversarial;
mod scenes_a;
mod scenes_b;
mod scenes_c;
mod special;

/// The `--flutter-app` face (#2063): the in-corpus flutter stub package and
/// the real widget-shape emitters. `pub(crate)` so the crate tests can read
/// the surface tables they pin.
pub(crate) mod flutter;

use std::fmt::Write as _;

use crate::coverage::{CELL_CORE_VALUE_USE, CELL_LIST, Cell, Coverage, SCENE_CELL_COUNT};
use crate::plan::{FileKind, FileSpec, Header, PackageId, Plan};
use crate::rng::SplitMix64;

/// Per-run rendering state shared by all renderers.
pub struct RenderCtx<'a> {
    pub plan: &'a Plan,
    pub rng: &'a mut SplitMix64,
    pub coverage: &'a mut Coverage,
    /// Running count of rotation files rendered; drives matrix scheduling and
    /// file size targets.
    pub rotation_ordinal: usize,
}

/// Renders one planned file to its full text content.
pub fn render(ctx: &mut RenderCtx<'_>, item: &FileSpec) -> String {
    let mut w = String::with_capacity(4 * 1024);
    match &item.kind {
        FileKind::RootPubspec => special::root_pubspec(&mut w, ctx),
        FileKind::PackagePubspec(package) => special::package_pubspec(&mut w, ctx, *package),
        FileKind::MainEntry => special::main_entry(&mut w, ctx),
        FileKind::Rotation { header, banner } => rotation(&mut w, ctx, *header, *banner),
        FileKind::FeatureBarrel(_) => special::feature_barrel(&mut w, ctx, feature_package(item)),
        FileKind::FacetHub(p) => special::facet_hub(&mut w, ctx, *p),
        FileKind::FacetStub(p) => special::facet_stub(&mut w, *p),
        FileKind::FacetIo(p) => special::facet_io(&mut w, *p),
        FileKind::AnchorModel(p) => special::anchor_model(&mut w, *p),
        FileKind::NarrowGateway(p) => special::narrow_gateway(&mut w, ctx, *p),
        FileKind::Gateway(p) => special::gateway(&mut w, ctx, *p),
        FileKind::WidgetHost { package, index } => {
            // #2063: under the flutter-app face the host/part slots carry
            // real StatefulWidget/State shapes over the stub surface; the
            // plain render() pair stays the default.
            if ctx.plan.flutter_app {
                flutter::widget_host(&mut w, *package, *index);
            } else {
                special::widget_host(&mut w, ctx, *package, *index);
            }
        }
        FileKind::WidgetPart { package, index } => {
            if ctx.plan.flutter_app {
                flutter::widget_part(&mut w, *package, *index);
            } else {
                special::widget_part(&mut w, ctx, *package, *index);
            }
        }
        FileKind::CoreBarrel => special::core_barrel(&mut w, ctx),
        FileKind::LazyLib => special::lazy_lib(&mut w),
        FileKind::PlatformHub => special::platform_hub(&mut w, ctx),
        FileKind::PlatformIo => special::platform_backing(&mut w, "io"),
        FileKind::PlatformStub => special::platform_backing(&mut w, "stub"),
        FileKind::PlatformWeb => special::platform_backing(&mut w, "web"),
        FileKind::ChainLink(index) => special::chain_link(&mut w, ctx, *index),
        FileKind::RegistryBox => special::registry_box(&mut w),
        FileKind::FunctionTypes => special::function_types(&mut w),
        FileKind::CoreValueLib => special::core_value_lib(&mut w),
        FileKind::TestFile(ordinal) => special::test_file(&mut w, ctx, *ordinal),
        FileKind::Patho(index) => adversarial::patho(&mut w, ctx, *index),
        FileKind::UnicodeScene(index) => adversarial::unicode_scene(&mut w, ctx, *index),
        FileKind::ErrStorm { package, file } => {
            adversarial::errstorm(&mut w, ctx, *package, *file);
        }
        FileKind::ErrStormPubspec(package) => adversarial::errstorm_pubspec(&mut w, *package),
        FileKind::WorkspaceRef { depth } => special::workspace_ref(&mut w, ctx, *depth),
        FileKind::RootPackageConfig => special::root_package_config(&mut w, ctx),
        FileKind::VendoredPubspec(index) => special::vendored_pubspec(&mut w, *index),
        FileKind::VendoredLib(index) => special::vendored_lib(&mut w, *index),
        FileKind::FlutterStubPubspec => flutter::stub_pubspec(&mut w),
        FileKind::FlutterWidgetsLib => flutter::widgets_lib(&mut w),
        FileKind::FlutterStubPart(index) => flutter::stub_part(&mut w, *index),
        FileKind::PackageAnalysisOptions(package) => {
            special::package_analysis_options(&mut w, ctx, *package);
        }
        FileKind::RootAnalysisOptions => special::root_analysis_options(&mut w, ctx),
    }
    w
}

/// Extracts the feature package index from a barrel spec. Barrels are only
/// planned for feature packages, so the fallback is unreachable.
fn feature_package(item: &FileSpec) -> u32 {
    match &item.kind {
        FileKind::FeatureBarrel(PackageId::Feature(p)) => *p,
        _ => unreachable!("FeatureBarrel is only planned for feature packages"),
    }
}

/// Dart type name for a vendored package (1e): `vendored_ext` ->
/// `VendoredExt`.
pub(crate) fn vendored_type_name(package: &str) -> String {
    package
        .split('_')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        })
        .collect()
}

fn line_count(text: &str) -> usize {
    text.matches('\n').count()
}

/// Size target for a rotation file: a deterministic spread of 20..=80 lines
/// with an occasional 200-line file (every 31st).
fn size_target(ordinal: usize, rng: &mut SplitMix64) -> usize {
    if ordinal.is_multiple_of(31) {
        200
    } else {
        20 + usize::try_from(rng.below(61)).expect("line count fits usize")
    }
}

fn rotation(w: &mut String, ctx: &mut RenderCtx<'_>, header: Header, banner: Option<(u32, u32)>) {
    let ordinal = ctx.rotation_ordinal;
    ctx.rotation_ordinal += 1;
    // The `// @dart =` banner must sit inside the file's leading comment
    // block — before the generated notice — or the scanner ignores it (1d).
    if let Some((major, minor)) = banner {
        dw!(w, "// @dart = {major}.{minor}\n");
        ctx.coverage.hit(Cell::DartVersionBanner);
    }
    // Flutter-app face (#2063): every 4th rotation file becomes a real
    // widget scene over the stub surface instead of a plain scene file.
    // Scene files skip the header machinery entirely and import only the
    // stub surface: the cadence CAN collide with a Core header (the
    // iterator-side rotation_counter skips the AppDir files the render-side
    // ordinal counts, so a core-package rotation with counter % 4 == 0 can
    // sit at ordinal % 4 == 2), and carrying the package:core import would
    // reference CoreValue — undeclared in the flutter stub — or dangle as
    // unused. DeferredCore (AppDir, ordinal % 16 == 7 -> % 4 == 3) and
    // cross-version edges (file % 8 == 1) never coincide with the cadence.
    if ctx.plan.flutter_app && ordinal % 4 == 2 {
        flutter::rotation_scene(w, ordinal);
        ctx.coverage.hit(Cell::PackageUriImport);
        return;
    }
    dw!(
        w,
        "// Generated by corpus-gen. Deterministic corpus; do not edit.\n"
    );
    match header {
        Header::Plain => {}
        Header::Core => {
            dw!(w, "\nimport 'package:core/core.dart';\n");
            ctx.coverage.hit(Cell::PackageUriImport);
        }
        Header::DeferredCore => {
            dw!(
                w,
                "\nimport 'package:core/core.dart' deferred as core_lazy;\n"
            );
            ctx.coverage.hit(Cell::DeferredImport);
        }
        Header::CrossVersion(target) => {
            dw!(
                w,
                "\nimport 'package:feature_{target:04}/feature_{target:04}.dart';\n"
            );
            ctx.coverage.hit(Cell::PackageUriImport);
            ctx.coverage.hit(Cell::CrossVersionImport);
        }
    }

    let target = size_target(ordinal, ctx.rng);
    if header == Header::DeferredCore {
        let k = ctx.rng.below(1_000);
        dw!(
            w,
            "\nFuture<String> lazyWiring{ordinal}() async {{\n  await core_lazy.loadLibrary();\n\n  final value = core_lazy.CoreValue({k});\n  return 'lazy-$k';\n}}\n"
        );
        ctx.coverage.hit(Cell::DeferredImport);
    }
    let mut scene_id: u32 = 1;
    if header == Header::Core {
        scenes_b::core_value_use(w, scene_id, ctx.rng);
        scene_id += 1;
        ctx.coverage.hit(Cell::CoreValueUse);
    }

    let mut lines = line_count(w);
    let mut cell = ordinal % SCENE_CELL_COUNT;
    let mut attempts = 0;
    while lines < target && attempts < SCENE_CELL_COUNT {
        attempts += 1;
        if cell == CELL_CORE_VALUE_USE {
            // Gated: only renderable in files that import core; those files
            // already emitted it above.
            cell = (cell + 1) % SCENE_CELL_COUNT;
            continue;
        }
        emit_cell(w, cell, scene_id, ctx.rng);
        scene_id += 1;
        ctx.coverage.hit(CELL_LIST[cell]);
        cell = (cell + 1) % SCENE_CELL_COUNT;
        lines = line_count(w);
    }
}

fn emit_cell(w: &mut String, cell: usize, id: u32, rng: &mut SplitMix64) {
    match cell {
        0 => scenes_a::class_modifiers(w, id, rng),
        1 => scenes_a::generic_class(w, id, rng),
        2 => scenes_a::mixin_apply(w, id, rng),
        3 => scenes_a::mixin_constraint(w, id, rng),
        4 => scenes_a::extension_type(w, id, rng),
        5 => scenes_a::extension_declare(w, id, rng),
        6 => scenes_a::enum_plain(w, id, rng),
        7 => scenes_a::enum_enhanced(w, id, rng),
        8 => scenes_a::record_values(w, id, rng),
        9 => scenes_a::operators(w, id, rng),
        10 => scenes_a::factory_ctor(w, id, rng),
        11 => scenes_a::const_ctor(w, id, rng),
        12 => scenes_a::external_decl(w, id, rng),
        13 => scenes_b::pattern_object(w, id, rng),
        14 => scenes_b::pattern_record(w, id, rng),
        15 => scenes_b::pattern_list_map(w, id, rng),
        16 => scenes_b::pattern_relational(w, id, rng),
        17 => scenes_b::pattern_logical(w, id, rng),
        18 => scenes_b::collection_control_flow(w, id, rng),
        19 => unreachable!("core_value_use is gated and handled by the caller"),
        20 => scenes_c::null_safety(w, id, rng),
        21 => scenes_c::labeled_break(w, id, rng),
        22 => scenes_c::async_future(w, id, rng),
        23 => scenes_c::async_stream(w, id, rng),
        24 => scenes_c::typedef_function_type(w, id, rng),
        25 => scenes_c::cascade(w, id, rng),
        26 => scenes_c::string_interpolation(w, id, rng),
        27 => scenes_c::numbers(w, id, rng),
        other => unreachable!("scene cell index {other} out of range"),
    }
}
