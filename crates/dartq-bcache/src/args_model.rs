//! Flutter command-line model: the slice of a `flutter build|run|test`
//! argv that feeds the cache key, and the name flutter_tools gives the
//! build directory of one build configuration.
//!
//! Pinned to flutter_tools 3.47.1 and the `package:args` 2.7.0 parser it
//! uses. Everything here is pure: no file system, no process environment,
//! no flutter invocation.
//!
//! # Parsing
//!
//! Tokens are classified exactly as `package:args` classifies them: `--`
//! ends option parsing; `-x` is a short option whose value, if it takes
//! one, is the next token; `-xyz` is either `-x` with the attached value
//! `yz` or a cluster of short flags; `--name[=value]` is a long option (a
//! value-taking option without `=` consumes the next token, even one that
//! starts with `-`); `--no-name` negates a negatable flag; every other
//! token is a positional argument. Global options are accepted before and
//! after the command, and a single-valued option keeps its last value.
//! Every option of the modeled commands is known with the arity
//! flutter_tools declares for it and one role:
//!
//! * structured — the build mode flags, `--flavor`, `-t/--target` and
//!   `--dart-define` land in [`TargetSpec`] and [`ParsedFlutterCmd`];
//! * keyed — every other option is kept in `extra_key_args` exactly as
//!   typed (option and value tokens, original order);
//! * neutral — the logging, terminal and analytics options `-v`,
//!   `--quiet`, `--prefixed-errors`, `--[no-]wrap`, `--wrap-column`,
//!   `--[no-]color`, `--[no-]version-check`, `--suppress-analytics`,
//!   `--print-dtd` and `--debug-logs-dir` change no artifact and are
//!   dropped.
//!
//! Options flutter_tools 3.47.1 does not declare are kept verbatim: worst
//! case a spurious miss, never a wrong hit. An unknown `--name` or `-x`
//! without an inline value may consume the next token, so from it on the
//! rest of the argv is kept verbatim and left uninterpreted.
//!
//! `build` and `run` take the first positional argument as the target file
//! when `-t` is absent (flutter's `targetFile`); `test` takes positional
//! arguments as test paths. flutter_tools has no runtime-argument
//! passthrough: tokens after `--` are positional arguments too, so for
//! `run` and `test` they stay in the key behind a `--` marker, and a `--`
//! in `flutter build` is rejected as unsupported.

use std::collections::BTreeMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use md5::{Digest, Md5};

use crate::types::TargetSpec;

/// The key-relevant model of one flutter invocation.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ParsedFlutterCmd {
    pub target: TargetSpec,
    /// `--dart-define` map exactly as passed (before define filtering). A
    /// repeated key keeps its last value, as the Dart compilers do; empty
    /// values are dropped, as flutter_tools drops them. A define without a
    /// key (`V`, `=V`) stays verbatim in `extra_key_args` instead.
    pub defines: BTreeMap<String, String>,
    /// `--dart-define-from-file` paths in argv order, relative to the
    /// flutter working directory. flutter_tools folds the files' contents
    /// into the define list and no depfile records them, so the cache key
    /// must hash their contents; the option tokens also stay in
    /// `extra_key_args`.
    pub define_files: Vec<PathBuf>,
}

/// Why an argv has no model.
#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
pub enum ArgModelError {
    /// A command or argv shape outside the model; the invocation must run
    /// uncached.
    #[error("unsupported flutter invocation: {0}")]
    Unsupported(String),
    /// An argv flutter_tools 3.47.1 itself rejects.
    #[error("invalid flutter invocation: {0}")]
    Invalid(String),
}

/// Models `argv`, the arguments after the `flutter` token, e.g.
/// `["build", "apk", "--release", "--dart-define", "K=V"]`.
///
/// Modeled commands: `build apk|appbundle|ios|macos|linux|windows|web|bundle`,
/// `run` and `test`. [`TargetSpec::command`] is the command chain
/// (`build apk`, `run`, `test`), so `apk` and `appbundle` never share a key
/// although both build for platform `android`; `bundle` has platform
/// `bundle`. `run` picks its platform only once a device is selected, so its
/// platform is `host-default` and any `-d/--device-id` stays verbatim in
/// `extra_key_args` (devices select different artifacts). `test` has
/// platform `dart-test` and always runs in debug mode.
///
/// Default build modes (each subcommand's `addBuildModeFlags`): `build
/// bundle`, `run` and `build ios --simulator` debug, every other `build`
/// release, `test` always debug. `entry` is the target file, defaulting to
/// `lib/main.dart`; it is `None` for `test`, and when the target file may be
/// set inside an uninterpreted argv tail.
///
/// # Errors
///
/// [`ArgModelError::Unsupported`] for commands and argv shapes outside the
/// model (including `--help`, `--version` and the analytics toggles, which
/// build nothing); [`ArgModelError::Invalid`] for argvs flutter rejects:
/// conflicting build modes, a missing option value, a value given to a flag,
/// a negated non-negatable option, the global `--machine` flag, and build
/// mode flags on `test`.
pub fn parse_flutter_args(argv: &[String]) -> Result<ParsedFlutterCmd, ArgModelError> {
    let mut parser = Parser::new(argv);
    let command = match parser.command_name(&[GLOBAL])? {
        "build" => {
            let sub = parser.command_name(&[GLOBAL])?;
            match BUILD_COMMANDS.iter().find(|(name, _)| *name == sub) {
                Some((_, command)) => command,
                None if sub == "aar" => {
                    return Err(unsupported(
                        "`flutter build aar` builds every mode in one run",
                    ));
                }
                None => return Err(unsupported(format!("`flutter build {sub}`"))),
            }
        }
        "run" => &RUN,
        "test" => &TEST,
        other => return Err(unsupported(format!("`flutter {other}`"))),
    };
    parser.command_options(command)?;
    Ok(parser.finish(command))
}

/// The `.dart_tool/flutter_build/<name>` directory name flutter_tools 3.47.1
/// gives one build configuration (the `Environment` factory):
///
/// `md5_hex(utf8(engine_revision ++ k1 ++ v1 ++ … ++ kn ++ vn ++ output_dir))`
///
/// — plain concatenation without separators, keys ordered by UTF-16 code
/// units (Dart's `String.compareTo`), lowercase hex. Pass `""` for
/// local-engine builds: flutter writes nothing for a null engine revision.
///
/// `defines` must be the complete `Environment.defines` map of the build and
/// `output_dir` the exact `Environment.outputDir.path` string; flutter
/// neither canonicalizes nor absolutizes it. The two main producers:
///
/// * `flutter assemble`, run by the Gradle/Xcode/CMake scripts behind
///   `build apk|appbundle|ios|macos|linux|windows`: every `-dKEY=VALUE` the
///   script passes (`TargetFile`, `TargetPlatform`, `BuildMode`, `Flavor`,
///   `TrackWidgetCreation`, `TreeShakeIcons`, `DartObfuscation`,
///   `SplitDebugInfo`, `AndroidArchs`, `MinSdkVersion`, …); every dart define
///   decoded from `--DartDefines` as its own top-level key, including the
///   injected `FLUTTER_APP_FLAVOR`, `FLUTTER_BUILD_NAME`,
///   `FLUTTER_BUILD_NUMBER`, `FLUTTER_VERSION`, `FLUTTER_CHANNEL`,
///   `FLUTTER_GIT_URL`, `FLUTTER_FRAMEWORK_REVISION`,
///   `FLUTTER_ENGINE_REVISION`, `FLUTTER_DART_VERSION` and
///   `FLUTTER_ENABLED_FEATURE_FLAGS`; `DartDefines` itself (the
///   comma-joined base64 list as received); `ExtraFrontEndOptions` and
///   `ExtraGenSnapshotOptions` when passed; and `DeferredComponents`.
///   `output_dir` is the `--output` directory, made absolute against the
///   project directory. This is the map [`crate::assemble`] captures and
///   this function recomputes a recorded dir name from.
/// * `BundleBuilder` (`build bundle`, flutter_tools 3.47.1
///   `bundle_builder.dart`): `TargetPlatform`, `TargetFile`,
///   `DeferredComponents=false` spread before
///   `BuildInfo.toBuildSystemEnvironment()` (`build_info.dart`) — always
///   `BuildMode`, `DartObfuscation`, `TrackWidgetCreation`,
///   `TreeShakeIcons`; plus, when the `BuildInfo` carries them:
///   `DartDefines` (the whole define vector base64-encoded as ONE value —
///   the pubspec-derived `FLUTTER_BUILD_NAME`/`FLUTTER_BUILD_NUMBER`,
///   `FLUTTER_VERSION` and kin, `FLUTTER_GIT_URL`, and the possibly short
///   framework/engine revisions all ride inside it, never as top-level
///   keys), `FrontendServerStarterPath` (non-null),
///   `ExtraFrontEndOptions`/`ExtraGenSnapshotOptions` (non-empty,
///   comma-joined), `SplitDebugInfo`, `CodeSizeDirectory`,
///   `FileSystemRoots` (non-empty), `FileSystemScheme`, `BuildName`,
///   `BuildNumber`, `UseLocalCanvasKit` (true only). `output_dir` is the
///   asset directory as given (relative `build/flutter_assets` by default).
///
///   This crate does NOT reproduce that map. The bundle face is
///   capture-not-predict: its dir names are tree-relative and replay
///   verbatim (see [`crate::orchestrator`] and
///   [`crate::types::NameRule::VerbatimRelative`]), so the bundle half of
///   this description documents upstream behavior, not a computation any
///   caller makes.
///
/// A non-UTF-8 `output_dir` is converted lossily (U+FFFD), as Dart decodes
/// such paths.
#[must_use]
pub fn flutter_build_dir_name(
    engine_revision: &str,
    defines: &BTreeMap<String, String>,
    output_dir: &Path,
) -> String {
    let mut entries: Vec<(&String, &String)> = defines.iter().collect();
    // The map iterates in byte order, which differs from UTF-16 order once
    // keys mix U+E000..U+FFFF with supplementary-plane characters.
    entries.sort_unstable_by(|(a, _), (b, _)| a.encode_utf16().cmp(b.encode_utf16()));
    let mut md5 = Md5::new();
    md5.update(engine_revision);
    for (key, value) in entries {
        md5.update(key);
        md5.update(value);
    }
    md5.update(output_dir.to_string_lossy().as_bytes());
    format!("{:x}", md5.finalize())
}

/// What parsing one option occurrence does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Role {
    /// Kept verbatim in `extra_key_args`.
    Key,
    /// Changes no artifact; dropped.
    Neutral,
    /// A build mode flag, carrying flutter's mode name.
    Mode(&'static str),
    Flavor,
    Target,
    Define,
    DefineFile,
    /// `build ios --[no-]simulator`: keyed, and `--simulator` makes debug
    /// the default mode.
    Simulator,
    /// Prints usage or exits before any command runs.
    NoBuild,
    /// Declared, but flutter_tools rejects it in this position.
    Reject(&'static str),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Arity {
    /// `negatable: false` flag.
    Flag,
    /// Flag that also accepts `--no-<name>`.
    Negatable,
    /// Option that requires a value.
    Value,
}

#[derive(Clone, Copy, Debug)]
struct Opt {
    name: &'static str,
    abbr: Option<u8>,
    alias: Option<&'static str>,
    arity: Arity,
    role: Role,
}

impl Opt {
    const fn new(name: &'static str, arity: Arity) -> Self {
        Self {
            name,
            abbr: None,
            alias: None,
            arity,
            role: Role::Key,
        }
    }

    const fn abbr(mut self, abbr: u8) -> Self {
        self.abbr = Some(abbr);
        self
    }

    const fn alias(mut self, alias: &'static str) -> Self {
        self.alias = Some(alias);
        self
    }

    const fn role(mut self, role: Role) -> Self {
        self.role = role;
        self
    }

    fn named(&self, name: &str) -> bool {
        self.name == name || self.alias == Some(name)
    }
}

const fn flag(name: &'static str) -> Opt {
    Opt::new(name, Arity::Flag)
}

const fn toggle(name: &'static str) -> Opt {
    Opt::new(name, Arity::Negatable)
}

const fn value(name: &'static str) -> Opt {
    Opt::new(name, Arity::Value)
}

/// The options one flutter_tools helper declares.
type Group = &'static [Opt];
/// Every option one parser level declares.
type Grammar = &'static [Group];

/// `FlutterCommandRunner`: valid before the command and, through the
/// parent-parser fallback of `package:args`, after it. Every command also
/// declares `-h/--help`, which this entry covers.
const GLOBAL: Grammar = &[&[
    flag("help").abbr(b'h').role(Role::NoBuild),
    flag("verbose").abbr(b'v').role(Role::Neutral),
    flag("prefixed-errors").role(Role::Neutral),
    flag("quiet").role(Role::Neutral),
    toggle("wrap").role(Role::Neutral),
    value("wrap-column").role(Role::Neutral),
    value("device-id").abbr(b'd'),
    flag("version").role(Role::NoBuild),
    flag("machine").role(Role::Reject(
        "the global `--machine` flag is only valid with `--version` or `analyze`",
    )),
    toggle("color").role(Role::Neutral),
    toggle("version-check").role(Role::Neutral),
    flag("enable-analytics").role(Role::NoBuild),
    flag("disable-analytics").role(Role::NoBuild),
    flag("suppress-analytics").role(Role::Neutral),
    value("packages"),
    flag("print-dtd").role(Role::Neutral),
    value("local-engine-src-path"),
    value("local-engine"),
    value("local-engine-host"),
    value("local-web-sdk"),
    flag("show-test-device"),
    flag("show-web-server-device"),
    flag("ci"),
    value("debug-logs-dir").role(Role::Neutral),
]];

// `FlutterCommand` helpers shared by several commands.
const BUILD_MODES: Group = &[
    flag("debug").role(Role::Mode("debug")),
    flag("profile").role(Role::Mode("profile")),
    flag("release").role(Role::Mode("release")),
    flag("jit-release").role(Role::Mode("jit_release")),
];
const TARGET: Group = &[value("target").abbr(b't').role(Role::Target)];
const FLAVOR: Group = &[value("flavor").role(Role::Flavor)];
const DART_DEFINES: Group = &[
    value("dart-define")
        .abbr(b'D')
        .alias("DartDefines")
        .role(Role::Define),
    value("dart-define-from-file").role(Role::DefineFile),
];
const EXTRA_DART_FLAGS: Group = &[
    value("extra-front-end-options").alias("ExtraFrontEndOptions"),
    value("extra-gen-snapshot-options").alias("ExtraGenSnapshotOptions"),
];
const FATAL_WARNINGS: Group = &[toggle("fatal-warnings")];
const PUB: Group = &[toggle("pub")];
const TRACK_WIDGET_CREATION: Group = &[toggle("track-widget-creation")];
const TREE_SHAKE_ICONS: Group = &[toggle("tree-shake-icons")];
const SPLIT_DEBUG_INFO: Group = &[value("split-debug-info")];
const OBFUSCATE: Group = &[toggle("obfuscate")];
const SHRINK: Group = &[toggle("shrink")];
const BUILD_NUMBER: Group = &[value("build-number")];
const BUILD_NAME: Group = &[value("build-name")];
const ENABLE_EXPERIMENT: Group = &[value("enable-experiment")];
const PERFORMANCE_FILE: Group = &[value("performance-measurement-file")];
const ANALYZE_SIZE: Group = &[toggle("analyze-size"), value("code-size-directory")];
const ANDROID_BUILD: Group = &[
    toggle("android-gradle-daemon"),
    toggle("android-skip-build-dependency-validation"),
    value("android-project-arg").abbr(b'P'),
    value("android-project-cache-dir"),
];
const IGNORE_DEPRECATION: Group = &[flag("ignore-deprecation")];
const FRONTEND_SERVER: Group = &[value("frontend-server-starter-path")];
const FILESYSTEM: Group = &[
    value("output-dill"),
    value("filesystem-root"),
    value("filesystem-scheme"),
];
const WEB_DEFINE: Group = &[value("web-define")];
const WEB_RESOURCES_CDN: Group = &[toggle("web-resources-cdn")];
const NATIVE_NULL_ASSERTIONS: Group = &[toggle("native-null-assertions")];

const APK_GRAMMAR: Grammar = &[
    FATAL_WARNINGS,
    TREE_SHAKE_ICONS,
    TARGET,
    BUILD_MODES,
    FLAVOR,
    PUB,
    BUILD_NUMBER,
    BUILD_NAME,
    SHRINK,
    SPLIT_DEBUG_INFO,
    OBFUSCATE,
    DART_DEFINES,
    EXTRA_DART_FLAGS,
    ENABLE_EXPERIMENT,
    PERFORMANCE_FILE,
    ANALYZE_SIZE,
    ANDROID_BUILD,
    IGNORE_DEPRECATION,
    TRACK_WIDGET_CREATION,
    &[
        flag("split-per-abi"),
        toggle("config-only"),
        value("target-platform"),
    ],
];

const APPBUNDLE_GRAMMAR: Grammar = &[
    FATAL_WARNINGS,
    TREE_SHAKE_ICONS,
    TARGET,
    BUILD_MODES,
    FLAVOR,
    PUB,
    BUILD_NUMBER,
    BUILD_NAME,
    SHRINK,
    SPLIT_DEBUG_INFO,
    OBFUSCATE,
    DART_DEFINES,
    EXTRA_DART_FLAGS,
    PERFORMANCE_FILE,
    TRACK_WIDGET_CREATION,
    ENABLE_EXPERIMENT,
    ANALYZE_SIZE,
    ANDROID_BUILD,
    IGNORE_DEPRECATION,
    &[
        value("target-platform"),
        toggle("deferred-components"),
        toggle("validate-deferred-components"),
    ],
];

const IOS_GRAMMAR: Grammar = &[
    FATAL_WARNINGS,
    TREE_SHAKE_ICONS,
    SPLIT_DEBUG_INFO,
    BUILD_MODES,
    TARGET,
    FLAVOR,
    PUB,
    BUILD_NUMBER,
    BUILD_NAME,
    OBFUSCATE,
    DART_DEFINES,
    EXTRA_DART_FLAGS,
    ENABLE_EXPERIMENT,
    PERFORMANCE_FILE,
    ANALYZE_SIZE,
    &[
        toggle("codesign"),
        toggle("publish-port"),
        toggle("config-only"),
        toggle("simulator").role(Role::Simulator),
    ],
];

/// `build macos` and `build windows` (`addCommonDesktopBuildOptions`).
const DESKTOP_GRAMMAR: Grammar = &[
    FATAL_WARNINGS,
    BUILD_MODES,
    PERFORMANCE_FILE,
    OBFUSCATE,
    ENABLE_EXPERIMENT,
    SPLIT_DEBUG_INFO,
    TREE_SHAKE_ICONS,
    ANALYZE_SIZE,
    DART_DEFINES,
    EXTRA_DART_FLAGS,
    PUB,
    TARGET,
    TRACK_WIDGET_CREATION,
    BUILD_NUMBER,
    BUILD_NAME,
    FLAVOR,
    &[toggle("config-only")],
];

const LINUX_GRAMMAR: Grammar = &[
    FATAL_WARNINGS,
    BUILD_MODES,
    PERFORMANCE_FILE,
    OBFUSCATE,
    ENABLE_EXPERIMENT,
    SPLIT_DEBUG_INFO,
    TREE_SHAKE_ICONS,
    ANALYZE_SIZE,
    DART_DEFINES,
    EXTRA_DART_FLAGS,
    PUB,
    TARGET,
    TRACK_WIDGET_CREATION,
    BUILD_NUMBER,
    BUILD_NAME,
    FLAVOR,
    &[
        value("target-platform"),
        value("target-sysroot"),
        toggle("config-only"),
    ],
];

const WEB_GRAMMAR: Grammar = &[
    FATAL_WARNINGS,
    TREE_SHAKE_ICONS,
    TARGET,
    PUB,
    BUILD_NUMBER,
    BUILD_NAME,
    BUILD_MODES,
    DART_DEFINES,
    WEB_DEFINE,
    ENABLE_EXPERIMENT,
    NATIVE_NULL_ASSERTIONS,
    WEB_RESOURCES_CDN,
    &[
        value("output").abbr(b'o').alias("output-dir"),
        value("base-href"),
        value("static-assets-url"),
        value("pwa-strategy"),
        value("optimization-level").abbr(b'O'),
        toggle("source-maps"),
        flag("csp"),
        value("dart2js-optimization"),
        flag("dump-info"),
        toggle("minify-js"),
        toggle("minify-wasm"),
        toggle("wasm-dry-run"),
        flag("no-frequency-based-minification"),
        flag("wasm"),
        toggle("strip-wasm"),
    ],
];

const BUNDLE_GRAMMAR: Grammar = &[
    FATAL_WARNINGS,
    TARGET,
    FILESYSTEM,
    BUILD_NUMBER,
    BUILD_MODES,
    DART_DEFINES,
    EXTRA_DART_FLAGS,
    PUB,
    TRACK_WIDGET_CREATION,
    &[
        value("depfile"),
        value("target-platform"),
        value("asset-dir"),
        toggle("tree-shake-icons"),
    ],
];

const RUN_GRAMMAR: Grammar = &[
    FATAL_WARNINGS,
    BUILD_MODES,
    DART_DEFINES,
    WEB_DEFINE,
    FLAVOR,
    WEB_RESOURCES_CDN,
    NATIVE_NULL_ASSERTIONS,
    TARGET,
    PUB,
    TRACK_WIDGET_CREATION,
    ANDROID_BUILD,
    FILESYSTEM,
    EXTRA_DART_FLAGS,
    FRONTEND_SERVER,
    ENABLE_EXPERIMENT,
    IGNORE_DEPRECATION,
    // `RunCommandBase`.
    &[
        value("use-application-binary"),
        flag("trace-startup"),
        toggle("cache-startup-profile"),
        flag("verbose-system-logs"),
        flag("purge-persistent-cache"),
        value("route"),
        value("vmservice-out-file"),
        flag("disable-service-auth-codes"),
        flag("disable-service-origin-check"),
        toggle("start-paused"),
        value("dart-flags"),
        flag("endless-trace-buffer"),
        flag("trace-systrace"),
        value("trace-to-file"),
        flag("profile-microtasks"),
        flag("trace-skia"),
        value("trace-allowlist"),
        value("trace-skia-allowlist"),
        toggle("enable-dart-profiling"),
        flag("profile-startup"),
        flag("enable-software-rendering"),
        flag("skia-deterministic-rendering"),
        value("dart-entrypoint-args").abbr(b'a'),
        toggle("uninstall-first"),
        flag("wasm"),
        flag("ios-profile-debugger"),
    ],
    // `usesWebOptions`.
    &[
        value("web-header"),
        value("web-hostname"),
        value("web-port"),
        value("web-tls-cert-path"),
        value("web-tls-cert-key-path"),
        value("web-server-debug-protocol"),
        value("web-server-debug-backend-protocol"),
        value("web-server-debug-injected-client-protocol"),
        toggle("web-allow-expose-url"),
        toggle("web-run-headless"),
        value("web-browser-debug-port"),
        toggle("web-enable-expression-evaluation"),
        toggle("web-experimental-hot-reload"),
        value("web-launch-url"),
        value("web-browser-flag"),
        toggle("cross-origin-isolation"),
    ],
    // Ports, device selection, DDS, DevTools, engine switches, `RunCommand`.
    &[
        value("vm-service-port"),
        value("device-vmservice-port"),
        value("host-vmservice-port"),
        flag("ipv6"),
        value("device-user"),
        value("device-timeout"),
        value("device-connection"),
        value("dds-port"),
        toggle("dds"),
        toggle("disable-dds"),
        toggle("devtools"),
        value("devtools-server-address"),
        toggle("enable-impeller"),
        toggle("enable-flutter-gpu"),
        toggle("enable-vulkan-validation"),
        toggle("enable-embedder-api"),
        toggle("enable-hcpp"),
        toggle("test-flag"),
        value("initialize-from-dill"),
        toggle("assume-initialize-from-dill-up-to-date"),
        value("native-assets-yaml-file"),
        toggle("publish-port"),
        flag("machine"),
        toggle("await-first-frame-when-tracing"),
        toggle("use-test-fonts"),
        toggle("build"),
        value("project-root"),
        toggle("hot"),
        toggle("resident"),
        value("pid-file"),
        toggle("report-ready"),
        flag("benchmark"),
    ],
];

/// `flutter test` declares no build mode flags (it always compiles in debug
/// mode), so `package:args` rejects them.
const TEST_MODE: Role = Role::Reject("`flutter test` always runs in debug mode");

const TEST_GRAMMAR: Grammar = &[
    FATAL_WARNINGS,
    PUB,
    FRONTEND_SERVER,
    TRACK_WIDGET_CREATION,
    ENABLE_EXPERIMENT,
    DART_DEFINES,
    FLAVOR,
    &[
        value("device-user"),
        toggle("enable-impeller"),
        flag("machine"),
        toggle("enable-flutter-gpu"),
        toggle("enable-hcpp"),
        flag("experimental-faster-testing"),
        value("name"),
        value("plain-name"),
        value("tags").abbr(b't'),
        value("exclude-tags").abbr(b'x'),
        flag("start-paused"),
        toggle("fail-fast"),
        toggle("run-skipped"),
        flag("disable-service-auth-codes"),
        flag("disable-service-origin-check"),
        flag("coverage"),
        flag("merge-coverage"),
        flag("branch-coverage"),
        flag("ipv6"),
        value("coverage-path"),
        value("coverage-package"),
        flag("update-goldens"),
        value("concurrency").abbr(b'j'),
        toggle("test-assets"),
        value("platform"),
        value("test-randomize-ordering-seed"),
        value("total-shards"),
        value("shard-index"),
        toggle("enable-vmservice"),
        value("reporter").abbr(b'r'),
        value("file-reporter"),
        value("timeout"),
        flag("ignore-timeouts"),
        flag("wasm"),
        toggle("cross-origin-isolation"),
        toggle("uninstall"),
        value("dds-port"),
        toggle("dds"),
        toggle("disable-dds"),
    ],
    &[
        flag("debug").role(TEST_MODE),
        flag("profile").role(TEST_MODE),
        flag("release").role(TEST_MODE),
        flag("jit-release").role(TEST_MODE),
    ],
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Family {
    Build,
    Run,
    Test,
}

/// One modeled command.
struct Command {
    /// [`TargetSpec::command`].
    chain: &'static str,
    platform: &'static str,
    default_mode: &'static str,
    family: Family,
    grammar: Grammar,
}

const fn build(
    chain: &'static str,
    platform: &'static str,
    mode: &'static str,
    grammar: Grammar,
) -> Command {
    Command {
        chain,
        platform,
        default_mode: mode,
        family: Family::Build,
        grammar,
    }
}

/// `flutter build <subcommand>`; default modes from each subcommand's
/// `addBuildModeFlags(defaultToRelease: …)`.
const BUILD_COMMANDS: &[(&str, Command)] = &[
    ("apk", build("build apk", "android", "release", APK_GRAMMAR)),
    (
        "appbundle",
        build("build appbundle", "android", "release", APPBUNDLE_GRAMMAR),
    ),
    ("ios", build("build ios", "ios", "release", IOS_GRAMMAR)),
    (
        "macos",
        build("build macos", "macos", "release", DESKTOP_GRAMMAR),
    ),
    (
        "linux",
        build("build linux", "linux", "release", LINUX_GRAMMAR),
    ),
    (
        "windows",
        build("build windows", "windows", "release", DESKTOP_GRAMMAR),
    ),
    ("web", build("build web", "web", "release", WEB_GRAMMAR)),
    (
        "bundle",
        build("build bundle", "bundle", "debug", BUNDLE_GRAMMAR),
    ),
];

const RUN: Command = Command {
    chain: "run",
    platform: "host-default",
    default_mode: "debug",
    family: Family::Run,
    grammar: RUN_GRAMMAR,
};

const TEST: Command = Command {
    chain: "test",
    platform: "dart-test",
    default_mode: "debug",
    family: Family::Test,
    grammar: TEST_GRAMMAR,
};

enum Lookup {
    Found(Opt),
    Negated(Opt),
    Unknown,
}

/// `package:args` long-option resolution: at each parser level an exact
/// name or alias first, then `no-<name>` as a negation, then the parent.
fn find_long(chain: &[Grammar], name: &str) -> Lookup {
    let positive = name.strip_prefix("no-");
    for grammar in chain {
        let options = || grammar.iter().flat_map(|group| group.iter());
        if let Some(opt) = options().find(|opt| opt.named(name)) {
            return Lookup::Found(*opt);
        }
        if let Some(positive) = positive
            && let Some(opt) = options().find(|opt| opt.named(positive))
        {
            return Lookup::Negated(*opt);
        }
    }
    Lookup::Unknown
}

fn find_abbr(chain: &[Grammar], abbr: u8) -> Option<Opt> {
    chain
        .iter()
        .flat_map(|grammar| grammar.iter())
        .flat_map(|group| group.iter())
        .find(|opt| opt.abbr == Some(abbr))
        .copied()
}

fn unsupported(reason: impl Into<String>) -> ArgModelError {
    ArgModelError::Unsupported(reason.into())
}

fn invalid(reason: impl Into<String>) -> ArgModelError {
    ArgModelError::Invalid(reason.into())
}

/// One kept argv index.
#[derive(Clone, Copy)]
enum Item {
    /// An option, option value or `--` marker, kept verbatim.
    Arg(usize),
    /// A positional argument: the target-file candidate of `build`/`run`.
    Positional(usize),
}

/// How [`Parser::option`] handled the token at the cursor.
enum Step {
    Consumed,
    Positional,
    /// An unknown option without an inline value: it may consume the next
    /// token.
    Ambiguous,
}

struct Parser<'a> {
    argv: &'a [String],
    pos: usize,
    items: Vec<Item>,
    mode: Option<&'static str>,
    flavor: Option<&'a str>,
    target: Option<&'a str>,
    simulator: bool,
    defines: BTreeMap<String, String>,
    define_files: Vec<PathBuf>,
    /// Start of the uninterpreted argv tail.
    tail: Option<usize>,
}

impl<'a> Parser<'a> {
    fn new(argv: &'a [String]) -> Self {
        Self {
            argv,
            pos: 0,
            items: Vec::new(),
            mode: None,
            flavor: None,
            target: None,
            simulator: false,
            defines: BTreeMap::new(),
            define_files: Vec::new(),
            tail: None,
        }
    }

    /// Parses options up to the next positional token and returns it: the
    /// command, or the `build` subcommand.
    fn command_name(&mut self, chain: &[Grammar]) -> Result<&'a str, ArgModelError> {
        let argv = self.argv;
        while let Some(token) = argv.get(self.pos) {
            if token == "--" {
                return Err(unsupported("`--` before the command"));
            }
            match self.option(chain)? {
                Step::Consumed => {}
                Step::Positional => {
                    self.pos += 1;
                    return Ok(token);
                }
                Step::Ambiguous => {
                    return Err(unsupported(format!(
                        "unknown option `{token}` before the command"
                    )));
                }
            }
        }
        Err(unsupported("no command"))
    }

    fn command_options(&mut self, command: &Command) -> Result<(), ArgModelError> {
        let argv = self.argv;
        let chain = [command.grammar, GLOBAL];
        while let Some(token) = argv.get(self.pos) {
            if token == "--" {
                if command.family == Family::Build {
                    return Err(unsupported(
                        "`--` in `flutter build`: the tokens after it are target-file candidates",
                    ));
                }
                self.items.push(Item::Arg(self.pos));
                self.items
                    .extend((self.pos + 1..argv.len()).map(Item::Positional));
                return Ok(());
            }
            match self.option(&chain)? {
                Step::Consumed => {}
                Step::Positional => {
                    self.items.push(Item::Positional(self.pos));
                    self.pos += 1;
                }
                Step::Ambiguous if self.pos + 1 == argv.len() => {
                    self.keep(self.pos..argv.len());
                    self.pos += 1;
                }
                Step::Ambiguous => return self.freeze_tail(command),
            }
        }
        Ok(())
    }

    /// Keeps everything from the cursor on verbatim and uninterpreted.
    fn freeze_tail(&mut self, command: &Command) -> Result<(), ArgModelError> {
        let rest = &self.argv[self.pos..];
        if command.family == Family::Build && rest.iter().any(|token| token == "--") {
            return Err(unsupported(
                "`--` in `flutter build`: the tokens after it are target-file candidates",
            ));
        }
        let names_define_file = |token: &String| {
            token
                .strip_prefix("--dart-define-from-file")
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('='))
        };
        if rest.iter().any(names_define_file) {
            return Err(unsupported(
                "`--dart-define-from-file` behind an unknown option: the file is unidentifiable",
            ));
        }
        self.tail = Some(self.pos);
        Ok(())
    }

    /// Classifies and applies the token at the cursor, advancing past every
    /// token it consumes.
    fn option(&mut self, chain: &[Grammar]) -> Result<Step, ArgModelError> {
        let argv = self.argv;
        let at = self.pos;
        let token = argv[at].as_str();
        let bytes = token.as_bytes();
        if bytes.len() >= 2 && bytes[0] == b'-' && bytes[1].is_ascii_alphanumeric() {
            if bytes.len() == 2 {
                // `-x`: a value-taking option reads the next token.
                let Some(opt) = find_abbr(chain, bytes[1]) else {
                    return Ok(Step::Ambiguous);
                };
                self.take(opt, None, at)?;
                return Ok(Step::Consumed);
            }
            // `-xyz`: never reads the next token.
            let end = 1 + bytes[1..]
                .iter()
                .take_while(|b| b.is_ascii_alphanumeric())
                .count();
            if token[end..].contains(['\n', '\r']) {
                return Ok(Step::Positional);
            }
            self.abbreviation(chain, at, end)?;
            return Ok(Step::Consumed);
        }
        if let Some(body) = token.strip_prefix("--") {
            let (name, inline) = match body.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (body, None),
            };
            let well_formed = name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                && inline.is_none_or(|value| !value.contains(['\n', '\r']));
            if well_formed {
                match find_long(chain, name) {
                    Lookup::Found(opt) => self.take(opt, inline, at)?,
                    Lookup::Negated(opt) => self.negate(opt, at)?,
                    Lookup::Unknown if inline.is_some() => {
                        self.keep(at..at + 1);
                        self.pos = at + 1;
                    }
                    Lookup::Unknown => return Ok(Step::Ambiguous),
                }
                return Ok(Step::Consumed);
            }
        }
        Ok(Step::Positional)
    }

    /// Applies `opt`, reading a value from `inline` or the next token.
    fn take(&mut self, opt: Opt, inline: Option<&'a str>, at: usize) -> Result<(), ArgModelError> {
        let argv = self.argv;
        if opt.arity != Arity::Value {
            if inline.is_some() {
                return Err(invalid(format!(
                    "flag `{}` does not take a value",
                    argv[at]
                )));
            }
            self.pos = at + 1;
            return self.apply(opt, "", false, at..at + 1);
        }
        let (value, end) = match inline {
            Some(value) => (value, at + 1),
            None => match argv.get(at + 1) {
                Some(next) => (next.as_str(), at + 2),
                None => {
                    return Err(invalid(format!(
                        "option `{}` is missing its value",
                        argv[at]
                    )));
                }
            },
        };
        self.pos = end;
        self.apply(opt, value, false, at..end)
    }

    fn negate(&mut self, opt: Opt, at: usize) -> Result<(), ArgModelError> {
        if opt.arity != Arity::Negatable {
            return Err(invalid(format!(
                "`{}` negates an option that cannot be negated",
                self.argv[at]
            )));
        }
        self.pos = at + 1;
        self.apply(opt, "", true, at..at + 1)
    }

    /// `-xyz` (letters and digits up to `end`, then the rest): `-x` with the
    /// attached value `yz…`, or the flag cluster `-x -y -z`.
    fn abbreviation(
        &mut self,
        chain: &[Grammar],
        at: usize,
        end: usize,
    ) -> Result<(), ArgModelError> {
        let argv = self.argv;
        let token = argv[at].as_str();
        let letters = &token.as_bytes()[1..end];
        self.pos = at + 1;
        let Some(first) = find_abbr(chain, letters[0]) else {
            // Unknown, but self-contained whatever its arity.
            self.keep(at..at + 1);
            return Ok(());
        };
        if first.arity == Arity::Value {
            return self.apply(first, &token[2..], false, at..at + 1);
        }
        if end < token.len() {
            return Err(invalid(format!(
                "flag `-{}` cannot take a value in `{token}`",
                char::from(letters[0])
            )));
        }
        let mut cluster = Vec::with_capacity(letters.len());
        for &letter in letters {
            match find_abbr(chain, letter) {
                None => {
                    self.keep(at..at + 1);
                    return Ok(());
                }
                Some(opt) if opt.arity == Arity::Value => {
                    return Err(invalid(format!(
                        "option `-{}` cannot be part of the flag cluster `{token}`",
                        char::from(letter)
                    )));
                }
                Some(opt) => cluster.push(opt),
            }
        }
        let mut keyed = false;
        for opt in cluster {
            keyed |= matches!(opt.role, Role::Key | Role::Simulator);
            // An empty token range: the cluster token is kept once below.
            self.apply(opt, "", false, at..at)?;
        }
        if keyed {
            self.keep(at..at + 1);
        }
        Ok(())
    }

    /// Applies one occurrence of `opt` spelled by the argv tokens in `tokens`.
    fn apply(
        &mut self,
        opt: Opt,
        value: &'a str,
        negated: bool,
        tokens: Range<usize>,
    ) -> Result<(), ArgModelError> {
        match opt.role {
            Role::Key => {}
            Role::Neutral => return Ok(()),
            Role::Mode(mode) => {
                if self.mode.is_some_and(|set| set != mode) {
                    return Err(invalid("conflicting build mode flags"));
                }
                self.mode = Some(mode);
                return Ok(());
            }
            Role::Flavor => {
                self.flavor = Some(value);
                return Ok(());
            }
            Role::Target => {
                self.target = Some(value);
                return Ok(());
            }
            Role::Define => {
                if value.is_empty() {
                    return Ok(());
                }
                if let Some((key, val)) = value.split_once('=')
                    && !key.is_empty()
                {
                    self.defines.insert(key.to_owned(), val.to_owned());
                    return Ok(());
                }
            }
            Role::DefineFile => self.define_files.push(PathBuf::from(value)),
            Role::Simulator => self.simulator = !negated,
            Role::NoBuild => {
                return Err(unsupported(format!(
                    "`{}` builds nothing",
                    self.argv[tokens.start]
                )));
            }
            Role::Reject(reason) => return Err(invalid(reason)),
        }
        self.keep(tokens);
        Ok(())
    }

    fn keep(&mut self, tokens: Range<usize>) {
        self.items.extend(tokens.map(Item::Arg));
    }

    fn finish(self, command: &Command) -> ParsedFlutterCmd {
        let argv = self.argv;
        let takes_target = command.family != Family::Test;
        let mut entry = self.target.map(PathBuf::from);
        let tail = self.tail.map_or(&[][..], |start| &argv[start..]);
        let mut extra = Vec::with_capacity(self.items.len() + tail.len());
        for item in self.items {
            match item {
                Item::Positional(at) if takes_target && entry.is_none() => {
                    entry = Some(PathBuf::from(&argv[at]));
                }
                Item::Arg(at) | Item::Positional(at) => extra.push(argv[at].clone()),
            }
        }
        extra.extend_from_slice(tail);
        if takes_target && entry.is_none() && self.tail.is_none() {
            // flutter's `targetFile` default.
            entry = Some(Path::new("lib").join("main.dart"));
        }
        let default_mode = if self.simulator {
            "debug"
        } else {
            command.default_mode
        };
        ParsedFlutterCmd {
            target: TargetSpec {
                command: command.chain.to_owned(),
                platform: command.platform.to_owned(),
                mode: self.mode.unwrap_or(default_mode).to_owned(),
                entry,
                flavor: self.flavor.map(str::to_owned),
                extra_key_args: extra,
            },
            defines: self.defines,
            define_files: self.define_files,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(tokens: &[&str]) -> Vec<String> {
        tokens.iter().map(|token| (*token).to_owned()).collect()
    }

    fn parse(tokens: &[&str]) -> ParsedFlutterCmd {
        parse_flutter_args(&strings(tokens)).unwrap_or_else(|err| panic!("{tokens:?}: {err}"))
    }

    fn parse_err(tokens: &[&str]) -> ArgModelError {
        match parse_flutter_args(&strings(tokens)) {
            Ok(parsed) => panic!("{tokens:?} unexpectedly parsed: {parsed:?}"),
            Err(err) => err,
        }
    }

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn parse_apk_release_with_defines_and_flavor() {
        let parsed = parse(&[
            "build",
            "apk",
            "--release",
            "--dart-define",
            "API=prod",
            "--dart-define=MODE=paid",
            "--flavor",
            "paid",
        ]);
        assert_eq!(
            parsed.target,
            TargetSpec {
                command: "build apk".to_owned(),
                platform: "android".to_owned(),
                mode: "release".to_owned(),
                entry: Some(PathBuf::from("lib/main.dart")),
                flavor: Some("paid".to_owned()),
                extra_key_args: Vec::new(),
            }
        );
        assert_eq!(parsed.defines, map(&[("API", "prod"), ("MODE", "paid")]));
        assert!(parsed.define_files.is_empty());
    }

    #[test]
    fn parse_run_keeps_device_id_verbatim() {
        let parsed = parse(&["run", "-d", "macos"]);
        assert_eq!(parsed.target.command, "run");
        assert_eq!(parsed.target.platform, "host-default");
        assert_eq!(parsed.target.mode, "debug");
        assert_eq!(parsed.target.extra_key_args, strings(&["-d", "macos"]));
        // A global option keeps its spelling before or after the command.
        assert_eq!(
            parse(&["-d", "macos", "run"]).target.extra_key_args,
            strings(&["-d", "macos"])
        );
        assert_eq!(
            parse(&["run", "--device-id=chrome"]).target.extra_key_args,
            strings(&["--device-id=chrome"])
        );
    }

    #[test]
    fn parse_default_modes_pinned_per_command() {
        let cases: [(&[&str], &str, &str); 15] = [
            (&["build", "apk"], "android", "release"),
            (&["build", "appbundle"], "android", "release"),
            (&["build", "ios"], "ios", "release"),
            (&["build", "ios", "--simulator"], "ios", "debug"),
            (
                &["build", "ios", "--simulator", "--no-simulator"],
                "ios",
                "release",
            ),
            (
                &["build", "ios", "--simulator", "--profile"],
                "ios",
                "profile",
            ),
            (&["build", "macos"], "macos", "release"),
            (&["build", "linux"], "linux", "release"),
            (&["build", "windows"], "windows", "release"),
            (&["build", "web"], "web", "release"),
            (&["build", "bundle"], "bundle", "debug"),
            (&["build", "apk", "--jit-release"], "android", "jit_release"),
            (&["run"], "host-default", "debug"),
            (&["run", "--release"], "host-default", "release"),
            (&["test"], "dart-test", "debug"),
        ];
        for (argv, platform, mode) in cases {
            let target = parse(argv).target;
            assert_eq!(
                (target.platform.as_str(), target.mode.as_str()),
                (platform, mode),
                "{argv:?}"
            );
        }
    }

    #[test]
    fn parse_dart_define_spellings() {
        let parsed = parse(&[
            "build",
            "web",
            "--dart-define=A=1",
            "-D",
            "B=2",
            "-DC=3",
            "--DartDefines=D=4",
            "--dart-define",
            "E=x=y,z",
            "--dart-define=",
            "--dart-define=A=5",
        ]);
        assert_eq!(
            parsed.defines,
            map(&[
                ("A", "5"),
                ("B", "2"),
                ("C", "3"),
                ("D", "4"),
                ("E", "x=y,z")
            ])
        );
        assert!(parsed.target.extra_key_args.is_empty());
        // A define without a key is not a map entry: it stays in the key.
        let keyless = parse(&["build", "web", "--dart-define", "NOVALUE", "-D=V"]);
        assert!(keyless.defines.is_empty());
        assert_eq!(
            keyless.target.extra_key_args,
            strings(&["--dart-define", "NOVALUE", "-D=V"])
        );
    }

    #[test]
    fn parse_unknown_options_kept_verbatim() {
        let parsed = parse(&[
            "build",
            "apk",
            "--frob=x",
            "--obfuscate",
            "--split-debug-info",
            "out/symbols",
            "-Zq",
            "--release",
            "--frobnicate",
        ]);
        assert_eq!(parsed.target.mode, "release");
        assert_eq!(
            parsed.target.extra_key_args,
            strings(&[
                "--frob=x",
                "--obfuscate",
                "--split-debug-info",
                "out/symbols",
                "-Zq",
                "--frobnicate",
            ])
        );
    }

    #[test]
    fn parse_unknown_option_freezes_remaining_argv() {
        // `--frob` may consume `lib/alt.dart`, so nothing after it is
        // interpreted: neither the target file nor `--release`.
        let parsed = parse(&[
            "build",
            "bundle",
            "--no-pub",
            "--frob",
            "lib/alt.dart",
            "--release",
        ]);
        assert_eq!(parsed.target.mode, "debug");
        assert_eq!(parsed.target.entry, None);
        assert_eq!(
            parsed.target.extra_key_args,
            strings(&["--no-pub", "--frob", "lib/alt.dart", "--release"])
        );
        assert_ne!(
            parsed,
            parse(&[
                "build",
                "bundle",
                "--no-pub",
                "-t",
                "lib/main.dart",
                "--frob",
                "lib/alt.dart",
                "--release"
            ])
        );
        for argv in [
            &["build", "apk", "--frob", "--", "lib/alt.dart"][..],
            &["run", "--frob", "x", "--dart-define-from-file=env.json"],
        ] {
            assert!(
                matches!(parse_err(argv), ArgModelError::Unsupported(_)),
                "{argv:?}"
            );
        }
    }

    #[test]
    fn parse_test_rejects_build_mode_flags() {
        for mode in ["--debug", "--profile", "--release", "--jit-release"] {
            assert!(
                matches!(parse_err(&["test", mode]), ArgModelError::Invalid(_)),
                "{mode}"
            );
        }
        // `-t` is `--tags` here, and test paths are keyed, never an entry.
        let parsed = parse(&["test", "-t", "slow", "test/a_test.dart"]);
        assert_eq!(parsed.target.entry, None);
        assert_eq!(
            parsed.target.extra_key_args,
            strings(&["-t", "slow", "test/a_test.dart"])
        );
    }

    #[test]
    fn parse_unmodeled_invocations_are_unsupported() {
        let cases: [&[&str]; 11] = [
            &["pub", "get"],
            &["build", "aar"],
            &["build", "ipa"],
            &["build"],
            &[],
            &["--version"],
            &["-h"],
            &["build", "apk", "--help"],
            &["--disable-analytics", "build", "apk"],
            &["build", "apk", "--", "lib/main.dart"],
            &["--frob", "build", "apk"],
        ];
        for argv in cases {
            assert!(
                matches!(parse_err(argv), ArgModelError::Unsupported(_)),
                "{argv:?}"
            );
        }
    }

    #[test]
    fn parse_rejected_argvs_are_invalid() {
        let cases: [&[&str]; 8] = [
            &["build", "apk", "--debug", "--release"],
            &["build", "apk", "--obfuscate=true"],
            &["build", "apk", "--no-release"],
            &["build", "apk", "--no-flavor"],
            &["build", "apk", "--flavor"],
            &["run", "-d"],
            &["build", "apk", "--machine"],
            &["build", "web", "-vO"],
        ];
        for argv in cases {
            assert!(
                matches!(parse_err(argv), ArgModelError::Invalid(_)),
                "{argv:?}"
            );
        }
        // Repeating the same mode is fine.
        assert_eq!(
            parse(&["build", "apk", "--profile", "--profile"])
                .target
                .mode,
            "profile"
        );
    }

    #[test]
    fn parse_double_dash_tokens_stay_positional() {
        // No runtime passthrough: after `--` come positional arguments.
        let parsed = parse(&["run", "--", "lib/alt.dart"]);
        assert_eq!(parsed.target.entry, Some(PathBuf::from("lib/alt.dart")));
        assert_eq!(parsed.target.extra_key_args, strings(&["--"]));
        let quoted = parse(&["run", "-t", "lib/main.dart", "--", "--trace-startup"]);
        let flagged = parse(&["run", "--trace-startup"]);
        assert_eq!(
            quoted.target.extra_key_args,
            strings(&["--", "--trace-startup"])
        );
        assert_eq!(flagged.target.extra_key_args, strings(&["--trace-startup"]));
        assert_ne!(quoted, flagged);
        let tests = parse(&["test", "--", "test/b_test.dart"]);
        assert_eq!(
            tests.target.extra_key_args,
            strings(&["--", "test/b_test.dart"])
        );
    }

    #[test]
    fn parse_target_resolution_mirrors_flutter() {
        assert_eq!(
            parse(&["build", "apk"]),
            parse(&["build", "apk", "-t", "lib/main.dart"])
        );
        assert_eq!(
            parse(&["build", "apk", "lib/a.dart"]),
            parse(&["build", "apk", "--target=lib/a.dart"])
        );
        assert_eq!(
            parse(&["build", "apk", "-tlib/a.dart"]).target.entry,
            Some(PathBuf::from("lib/a.dart"))
        );
        let both = parse(&["build", "apk", "lib/b.dart", "-t", "lib/a.dart"]);
        assert_eq!(both.target.entry, Some(PathBuf::from("lib/a.dart")));
        assert_eq!(both.target.extra_key_args, strings(&["lib/b.dart"]));
        // `apk` and `appbundle` share a platform but never a key.
        assert_ne!(
            parse(&["build", "apk"]).target,
            parse(&["build", "appbundle"]).target
        );
    }

    #[test]
    fn parse_neutral_globals_dropped_keyed_globals_kept() {
        let parsed = parse(&[
            "-v",
            "--no-version-check",
            "build",
            "apk",
            "--wrap-column",
            "80",
            "--local-engine",
            "android_release",
            "--suppress-analytics",
            "-vv",
        ]);
        assert_eq!(
            parsed.target.extra_key_args,
            strings(&["--local-engine", "android_release"])
        );
        // `run` declares its own `--machine` flag.
        assert_eq!(
            parse(&["run", "--machine"]).target.extra_key_args,
            strings(&["--machine"])
        );
    }

    #[test]
    fn parse_define_files_recorded() {
        let parsed = parse(&[
            "build",
            "web",
            "--dart-define-from-file=env/prod.json",
            "--dart-define-from-file",
            ".env",
        ]);
        assert_eq!(
            parsed.define_files,
            vec![PathBuf::from("env/prod.json"), PathBuf::from(".env")]
        );
        assert_eq!(
            parsed.target.extra_key_args,
            strings(&[
                "--dart-define-from-file=env/prod.json",
                "--dart-define-from-file",
                ".env"
            ])
        );
    }

    #[test]
    fn parse_is_deterministic() {
        let argv = strings(&[
            "-v",
            "build",
            "appbundle",
            "--flavor",
            "free",
            "-Dz=1",
            "-Da=2",
            "--target-platform",
            "android-arm64",
            "--obfuscate",
            "--split-debug-info=sym",
            "lib/free.dart",
        ]);
        let first = parse_flutter_args(&argv);
        assert_eq!(first, parse_flutter_args(&argv));
        let parsed = first.unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(parsed.target.entry, Some(PathBuf::from("lib/free.dart")));
        assert_eq!(parsed.target.flavor.as_deref(), Some("free"));
        assert_eq!(parsed.defines, map(&[("a", "2"), ("z", "1")]));
        assert_eq!(
            parsed.target.extra_key_args,
            strings(&[
                "--target-platform",
                "android-arm64",
                "--obfuscate",
                "--split-debug-info=sym"
            ])
        );
    }

    #[test]
    fn build_dir_name_golden_vectors() {
        // `flutter assemble` for a `paid` release APK: the decoded dart
        // defines, including the injected FLUTTER_APP_FLAVOR, are top-level
        // keys next to the raw base64 `DartDefines` list.
        let assemble = map(&[
            ("AndroidArchs", "android-arm android-arm64 android-x64"),
            ("BuildMode", "release"),
            (
                "DartDefines",
                "RkxVVFRFUl9BUFBfRkxBVk9SPXBhaWQ=,QVBJPXByb2Q=",
            ),
            ("DeferredComponents", "false"),
            ("FLUTTER_APP_FLAVOR", "paid"),
            ("API", "prod"),
            ("Flavor", "paid"),
            ("MinSdkVersion", "21"),
            ("TargetFile", "lib/main.dart"),
            ("TargetPlatform", "android"),
            ("TrackWidgetCreation", "false"),
            ("TreeShakeIcons", "true"),
        ]);
        assert_eq!(
            flutter_build_dir_name(
                "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678",
                &assemble,
                Path::new("/work/app/build/app/intermediates/flutter/paidRelease"),
            ),
            "553963f6beb888c5ac0d091018092b80"
        );
        // `BundleBuilder` debug bundle: the relative asset dir is hashed as given.
        let bundle = map(&[
            ("BuildMode", "debug"),
            ("DartObfuscation", "false"),
            ("DeferredComponents", "false"),
            ("TargetFile", "lib/main.dart"),
            ("TargetPlatform", "android-arm"),
            ("TrackWidgetCreation", "true"),
            ("TreeShakeIcons", "false"),
        ]);
        assert_eq!(
            flutter_build_dir_name(
                "0123456789abcdef0123456789abcdef01234567",
                &bundle,
                Path::new("build/flutter_assets"),
            ),
            "14f9d4da4c14df627713da895671159a"
        );
        // Local engine (no revision) and no defines: md5 of the path alone.
        assert_eq!(
            flutter_build_dir_name("", &BTreeMap::new(), Path::new("/out")),
            "43adf4d8e9f8251849179b34bbcfb9bb"
        );
    }

    #[test]
    fn build_dir_name_orders_keys_by_utf16_code_units() {
        // U+10000 (surrogates D800 DC00) sorts before U+E000 in Dart but
        // after it in byte order.
        let defines = map(&[("\u{E000}", "bmp"), ("\u{10000}", "astral")]);
        assert_eq!(
            flutter_build_dir_name("e", &defines, Path::new("/o")),
            "539b2345964b193bea0cd88e8e11bae8"
        );
    }
}
