//! Bootstrap tests: the generation faces are pure (content, hashing, the
//! host locator); the end-to-end row spawns a real official-style plugin
//! through the whole Leg-2 path (synthetic package, pub resolution, the
//! shim, wire v1) and is `#[ignore]`d per the crate's live-suite
//! convention — `cargo test -p dartq-plugin bootstrap_runs_an_official_plugin_end_to_end -- --exact --ignored`
//! under a shell timeout, in the serial window.

use std::path::{Path, PathBuf};
use std::time::Instant;

use super::{
    OfficialPlugin, OfficialSource, bootstrap, content_hash, dart_string_literal, entrypoint_dart,
    entrypoint_path, host_package_with, pubspec_yaml,
};
use crate::SpawnSpec;

/// A scratch directory cleaned on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("dartq-bootstrap-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn pubspec_generation_mirrors_the_official_dependency_shapes() {
    let host = Path::new("/host");
    let shim = Path::new("/host/lib/src/analysis_server_plugin_shim");
    let plugins = [
        OfficialPlugin {
            name: "path_plugin".to_owned(),
            source: OfficialSource::Path(PathBuf::from("/plugins/path_plugin")),
        },
        OfficialPlugin {
            name: "versioned_plugin".to_owned(),
            source: OfficialSource::Versioned {
                constraint: "^1.2.0".to_owned(),
                hosted: None,
            },
        },
        OfficialPlugin {
            name: "hosted_plugin".to_owned(),
            source: OfficialSource::Versioned {
                constraint: ">=2.0.0 <3.0.0".to_owned(),
                hosted: Some("https://pub.example.com".to_owned()),
            },
        },
        OfficialPlugin {
            name: "git_plugin".to_owned(),
            source: OfficialSource::Git {
                url: "https://example.com/repo.git".to_owned(),
                path: Some("packages/git_plugin".to_owned()),
                reference: Some("main".to_owned()),
                tag_pattern: Some("v{version}".to_owned()),
            },
        },
    ];
    let overrides = vec![(
        "transitive_dep".to_owned(),
        OfficialSource::Path(PathBuf::from("/overrides/transitive_dep")),
    )];

    let yaml = pubspec_yaml(host, shim, &plugins, &overrides);

    let expected = concat!(
        "name: plugin_entrypoint\n",
        "version: 0.0.1\n",
        "environment:\n",
        "  sdk: ^3.10.0\n",
        "dependencies:\n",
        "  dartq_plugin_host:\n",
        "    path: /host\n",
        "  path_plugin:\n",
        "    path: /plugins/path_plugin\n",
        "  versioned_plugin: ^1.2.0\n",
        "  hosted_plugin:\n",
        "    version: >=2.0.0 <3.0.0\n",
        "    hosted: https://pub.example.com\n",
        "  git_plugin:\n",
        "    git:\n",
        "      url: https://example.com/repo.git\n",
        "      ref: main\n",
        "      path: packages/git_plugin\n",
        "      tag_pattern: v{version}\n",
        "dependency_overrides:\n",
        "  analysis_server_plugin:\n",
        "    path: /host/lib/src/analysis_server_plugin_shim\n",
        "  transitive_dep:\n",
        "    path: /overrides/transitive_dep\n",
    );
    assert_eq!(yaml, expected);
}

#[test]
fn a_user_override_of_the_shim_name_wins() {
    let plugins = [OfficialPlugin {
        name: "p".to_owned(),
        source: OfficialSource::Versioned {
            constraint: "^1.0.0".to_owned(),
            hosted: None,
        },
    }];
    let overrides = vec![(
        "analysis_server_plugin".to_owned(),
        OfficialSource::Path(PathBuf::from("/my/shim")),
    )];
    let yaml = pubspec_yaml(
        Path::new("/host"),
        Path::new("/host/lib/src/analysis_server_plugin_shim"),
        &plugins,
        &overrides,
    );
    // Exactly one override of the name: the user's, not the built-in shim.
    assert_eq!(yaml.matches("analysis_server_plugin:").count(), 1);
    assert!(yaml.contains("path: /my/shim"));
    assert!(!yaml.contains("path: /host/lib/src/analysis_server_plugin_shim"));
}

#[test]
fn entrypoint_generation_serves_the_plugin_with_startplugin() {
    let plugin = OfficialPlugin {
        name: "no_await_plugin".to_owned(),
        source: OfficialSource::Path(PathBuf::from("/p")),
    };
    let entry = entrypoint_path(0, &plugin);
    assert_eq!(entry, "bin/p0_no_await_plugin.dart");
    let dart = entrypoint_dart(&plugin, &entry);
    assert!(dart.contains("import 'package:no_await_plugin/main.dart' as p0;"));
    assert!(dart.contains("import 'package:analysis_server_plugin/plugin.dart' show startPlugin;"));
    assert!(dart.contains("name: 'no_await_plugin'"));
    assert!(dart.contains("entrypoint: 'bin/p0_no_await_plugin.dart'"));
    assert!(dart.contains("p0.plugin"));

    // A hostile name escapes into Dart string literals only as text.
    let hostile = OfficialPlugin {
        name: "x'$\n".to_owned(),
        source: OfficialSource::Path(PathBuf::from("/p")),
    };
    let dart = entrypoint_dart(&hostile, &entrypoint_path(1, &hostile));
    assert!(
        dart.contains("name: 'x\\'\\$\\n'"),
        "escaped literal: {dart}"
    );
}

#[test]
fn dart_string_literals_escape_interpolation_and_controls() {
    assert_eq!(dart_string_literal("plain"), "'plain'");
    assert_eq!(dart_string_literal("a\\b'c$d\ne"), "'a\\\\b\\'c\\$d\\ne'");
    assert_eq!(dart_string_literal("\u{1}"), "'\\x01'");
}

#[test]
fn the_cache_key_is_stable_and_content_addressed() {
    let host = Path::new("/host");
    let shim = Path::new("/host/lib/src/analysis_server_plugin_shim");
    let plugin = OfficialPlugin {
        name: "p".to_owned(),
        source: OfficialSource::Path(PathBuf::from("/p")),
    };
    let one = pubspec_yaml(host, shim, &[plugin.clone()], &[]);
    let two = pubspec_yaml(host, shim, &[plugin], &[]);
    assert_eq!(
        content_hash(&one),
        content_hash(&two),
        "same text, same key"
    );
    // A moved host (or any content change) re-keys the cache directory.
    let moved = pubspec_yaml(Path::new("/other-host"), shim, &[], &[]);
    assert_ne!(content_hash(&one), content_hash(&moved));
}

#[test]
fn the_host_locator_requires_the_shim_layout() {
    // The env override must point at a directory carrying the shim.
    let empty = TempDir::new("host-empty");
    assert!(
        host_package_with(Some(empty.path().as_os_str())).is_none()
            || host_package_with(Some(empty.path().as_os_str()))
                .is_some_and(|dir| dir != empty.path())
    );

    let hostish = TempDir::new("host-shape");
    let shim = hostish.path().join("lib/src/analysis_server_plugin_shim");
    std::fs::create_dir_all(&shim).expect("shim dir");
    std::fs::write(shim.join("pubspec.yaml"), "name: analysis_server_plugin\n").expect("marker");
    assert_eq!(
        host_package_with(Some(hostish.path().as_os_str())),
        Some(hostish.path().to_path_buf())
    );
}

#[test]
fn bootstrap_empty_input_touches_nothing() {
    // The absent path: no host lookup, no cache, no pub — `Ok([])`.
    assert_eq!(bootstrap(&[], &[]).expect("empty bootstrap"), Vec::new());
}

/// The minimal official-style plugin fixture (the smoke fixture's shape):
/// a pub package importing `package:analysis_server_plugin` verbatim and
/// registering one warning rule (`no_await`) through the official
/// registration API. Shared by the live tests (the E2E row and the
/// cold-start timing row).
fn write_official_plugin(root: &Path, name: &str) {
    std::fs::write(
        root.join("pubspec.yaml"),
        format!(
            "name: {name}\nenvironment:\n  sdk: ^3.10.0\ndependencies:\n  \
             analysis_server_plugin: any\n  analyzer: any\n"
        ),
    )
    .expect("plugin pubspec");
    std::fs::create_dir_all(root.join("lib")).expect("plugin lib");
    std::fs::write(
        root.join("lib").join("main.dart"),
        official_plugin_main(name),
    )
    .expect("plugin main");
}

/// The fixture's plugin source: the smoke fixture's warning rule verbatim
/// in official style (an await-expression rule through
/// `registerWarningRule`/`reportAtNode` — the code shape the engine face
/// must surface).
fn official_plugin_main(name: &str) -> String {
    format!(
        r#"import 'package:analysis_server_plugin/plugin.dart';
import 'package:analysis_server_plugin/registry.dart';
import 'package:analyzer/analysis_rule/analysis_rule.dart';
import 'package:analyzer/analysis_rule/rule_context.dart';
import 'package:analyzer/analysis_rule/rule_visitor_registry.dart';
import 'package:analyzer/dart/ast/ast.dart';
import 'package:analyzer/dart/ast/visitor.dart';
import 'package:analyzer/error/error.dart';

final plugin = _ProbePlugin();

class _ProbePlugin extends Plugin {{
  @override
  String get name => '{name}';

  @override
  void register(PluginRegistry registry) {{
    registry.registerWarningRule(_NoAwaitRule());
  }}
}}

class _NoAwaitRule extends AnalysisRule {{
  static const LintCode code = LintCode('no_await', 'No await expressions');

  _NoAwaitRule() : super(name: 'no_await', description: 'Flags await expressions.');

  @override
  LintCode get diagnosticCode => code;

  @override
  void registerNodeProcessors(RuleVisitorRegistry registry, RuleContext context) {{
    registry.addAwaitExpression(this, _Visitor(this, context));
  }}
}}

class _Visitor extends SimpleAstVisitor<void> {{
  final _NoAwaitRule rule;
  final RuleContext context;
  _Visitor(this.rule, this.context);

  @override
  void visitAwaitExpression(AwaitExpression node) {{
    rule.reportAtNode(node);
  }}
}}
"#
    )
}

/// The snapshot cache key tracks exactly what a trained image embeds —
/// the entrypoint text, pub's resolution, the host tree, each path
/// dependency's tree — and nothing else: a restored edit restores the key
/// (content-folded, no mtime sensitivity, no spurious retraining).
#[test]
fn snapshot_keys_track_embedded_content() {
    let host = TempDir::new("key-host");
    let shim = host.path().join("lib/src/analysis_server_plugin_shim");
    std::fs::create_dir_all(&shim).expect("shim dir");
    std::fs::write(shim.join("pubspec.yaml"), "name: analysis_server_plugin\n").expect("marker");
    std::fs::create_dir_all(host.path().join("lib/src")).expect("host lib");
    std::fs::write(host.path().join("lib/src/host.dart"), "var a = 1;\n").expect("host source");

    let dir = TempDir::new("key-dir");
    std::fs::create_dir_all(dir.path().join("bin")).expect("bin");
    std::fs::create_dir_all(dir.path().join(".dart_tool")).expect("pub state");
    let entry = "bin/p0_p.dart";
    std::fs::write(dir.path().join(entry), "// one\n").expect("entrypoint");
    std::fs::write(dir.path().join(".dart_tool/package_config.json"), "{}").expect("resolution");

    let plugin_root = TempDir::new("key-plugin");
    std::fs::create_dir_all(plugin_root.path().join("lib")).expect("plugin lib");
    std::fs::write(plugin_root.path().join("lib/main.dart"), "var b = 2;\n").expect("source");
    let plugin = OfficialPlugin {
        name: "p".to_owned(),
        source: OfficialSource::Path(plugin_root.path().to_path_buf()),
    };

    let key = |dir: &TempDir, host: &TempDir| {
        super::snapshot_key(dir.path(), entry, host.path(), &[plugin.clone()], &[])
    };
    let base = key(&dir, &host);
    assert_eq!(base, key(&dir, &host), "stable across calls");

    std::fs::write(dir.path().join(entry), "// two\n").expect("entrypoint edit");
    assert_ne!(base, key(&dir, &host), "entrypoint text change re-keys");
    std::fs::write(dir.path().join(entry), "// one\n").expect("entrypoint restore");

    std::fs::write(
        dir.path().join(".dart_tool/package_config.json"),
        "{\"v\":2}",
    )
    .expect("edit");
    assert_ne!(base, key(&dir, &host), "resolution change re-keys");
    std::fs::write(dir.path().join(".dart_tool/package_config.json"), "{}").expect("restore");

    std::fs::write(host.path().join("lib/src/host.dart"), "var a = 2;\n").expect("edit");
    assert_ne!(base, key(&dir, &host), "host source change re-keys");
    std::fs::write(host.path().join("lib/src/host.dart"), "var a = 1;\n").expect("restore");

    std::fs::write(plugin_root.path().join("lib/main.dart"), "var b = 3;\n").expect("edit");
    assert_ne!(base, key(&dir, &host), "path-plugin content change re-keys");
    std::fs::write(plugin_root.path().join("lib/main.dart"), "var b = 2;\n").expect("restore");

    assert_eq!(base, key(&dir, &host), "restored inputs restore the key");
}

/// The whole Leg-2 path over a real process: an official-style plugin
/// package (the smoke fixture's shape — a top-level `plugin` variable and
/// a warning rule, imports `package:analysis_server_plugin` verbatim) is
/// declared with a `path:` source; the bootstrap materializes the
/// synthetic package, resolves it with pub, and the runtime spawns the
/// generated entrypoint, whose census must name the warning rule and
/// whose findings must include the plugin's code — warning rules default
/// on with an empty cycle config.
///
/// Skips when `dart` is absent or pub cannot resolve (an offline
/// sandbox); both are environment facts, not product failures. Bounded by
/// the pub timeout and the manifest's handshake budgets.
#[test]
#[ignore = "spawns a real dart process and runs pub; run in the serial window via --ignored under a shell timeout"]
fn bootstrap_runs_an_official_plugin_end_to_end() {
    if !std::process::Command::new("dart")
        .arg("--version")
        .output()
        .is_ok()
    {
        println!("skipping: no `dart` on PATH, the live bootstrap cannot run");
        return;
    }
    let plugin_root = TempDir::new("live-plugin");
    write_official_plugin(plugin_root.path(), "dq_boot_probe");

    let declared = [OfficialPlugin {
        name: "dq_boot_probe".to_owned(),
        source: OfficialSource::Path(plugin_root.path().to_path_buf()),
    }];
    let discovered = match bootstrap(&declared, &[]) {
        Ok(discovered) => discovered,
        Err(error @ crate::BootstrapError::Pub { .. }) => {
            println!("skipping: pub could not resolve the bootstrap package: {error}");
            return;
        }
        Err(error) => panic!("bootstrap failed: {error}"),
    };
    assert_eq!(discovered.len(), 1);
    let discovered = &discovered[0];
    assert_eq!(discovered.manifest.name, "dq_boot_probe");
    assert!(discovered.manifest.entrypoint.starts_with("bin/"));

    let mut runtime = crate::PluginRuntime::spawn(SpawnSpec::dart(
        &discovered.dir,
        discovered.manifest.clone(),
    ))
    .expect("the generated entrypoint spawns and completes the handshake");

    // Warning rules run under an empty config (default-on), so the census
    // must already name the rule and its code.
    let census = runtime.rule_census();
    assert_eq!(census.len(), 1, "census: {census:?}");
    assert_eq!(census[0].name, "no_await");
    assert_eq!(census[0].kind, crate::RuleKind::Warning);
    assert_eq!(census[0].codes.len(), 1);
    assert_eq!(census[0].codes[0].name, "no_await");

    // One analyze cycle over a file with an await expression. The cycle
    // config carries the core-resolved enablement — the wire contract the
    // CLI/daemon weaves drive: the mirror registry's `enabled` is plain
    // membership over the census names the core resolved (a warning rule
    // under the no-configuration posture resolves enabled).
    let analyzed = plugin_root_path_string(&plugin_root);
    let push = crate::CyclePush {
        analyzed: vec![analyzed],
        config: crate::CycleConfig {
            enabled_rules: vec!["no_await".to_owned()],
            ..crate::CycleConfig::default()
        },
    };
    let files = dartq_source::FileTable::new();
    runtime
        .feed(&[], &files, &push)
        .expect("the cycle feed lands");
    let deadline = std::time::Duration::from_secs(30);
    let drain = runtime.wait_output(deadline).expect("the drain completes");
    assert!(
        drain
            .diagnostics
            .iter()
            .any(|row| row.code == "no_await" && row.file.ends_with("consumer/lib/main.dart")),
        "findings: {:?}",
        drain.diagnostics
    );

    runtime.shutdown().expect("graceful shutdown");
}

/// The cold-start timing row (#2748): the CURRENT official-plugin spawn
/// path — bootstrap → `PluginRuntime::spawn` (spawn+handshake) → the first
/// census cycle (feed + drain) — measured per phase over n fresh
/// sidecars. Each iteration is a cold VM start, the per-spawn cost the
/// ticket prices; bootstrap is measured both cold (first materialize/pub
/// or snapshot work) and on the content-addressed cache hit every later
/// run takes. Prints one `timing:` line per iteration; asserts the
/// semantic floor (census names the rule, the cycle finds it) so a broken
/// fast path cannot post fake numbers.
///
/// Skips when `dart` is absent or pub cannot resolve (an offline
/// sandbox); bounded by the pub timeout and the manifest budgets.
#[test]
#[ignore = "spawns real dart processes and runs pub; run in the serial window via --ignored under a shell timeout"]
fn spawn_cold_start_timing() {
    if !std::process::Command::new("dart")
        .arg("--version")
        .output()
        .is_ok()
    {
        println!("skipping: no `dart` on PATH, the timing row cannot run");
        return;
    }
    let plugin_root = TempDir::new("timing-plugin");
    write_official_plugin(plugin_root.path(), "dq_timing_probe");
    let declared = [OfficialPlugin {
        name: "dq_timing_probe".to_owned(),
        source: OfficialSource::Path(plugin_root.path().to_path_buf()),
    }];

    let boot = Instant::now();
    let discovered = match bootstrap(&declared, &[]) {
        Ok(discovered) => discovered,
        Err(error @ crate::BootstrapError::Pub { .. }) => {
            println!("skipping: pub could not resolve the bootstrap package: {error}");
            return;
        }
        Err(error) => panic!("bootstrap failed: {error}"),
    };
    let bootstrap_cold = boot.elapsed();
    let boot = Instant::now();
    let discovered = bootstrap(&declared, &[]).expect("cached bootstrap");
    let bootstrap_cached = boot.elapsed();
    assert_eq!(discovered.len(), 1);
    let plugin = discovered.into_iter().next().expect("the timing plugin");

    let analyzed = plugin_root_path_string(&plugin_root);
    let files = dartq_source::FileTable::new();
    for run in 0..3 {
        let t = Instant::now();
        let mut runtime =
            crate::PluginRuntime::spawn(SpawnSpec::dart(&plugin.dir, plugin.manifest.clone()))
                .expect("spawn+handshake");
        let spawn_handshake = t.elapsed();

        let census = runtime.rule_census();
        assert!(
            census.iter().any(|rule| rule.name == "no_await"),
            "census: {census:?}"
        );

        let t = Instant::now();
        let push = crate::CyclePush {
            analyzed: vec![analyzed.clone()],
            config: crate::CycleConfig {
                enabled_rules: vec!["no_await".to_owned()],
                ..crate::CycleConfig::default()
            },
        };
        runtime.feed(&[], &files, &push).expect("feed");
        let drain = runtime
            .wait_output(std::time::Duration::from_secs(30))
            .expect("drain");
        let first_cycle = t.elapsed();
        assert!(
            drain.diagnostics.iter().any(|row| row.code == "no_await"),
            "findings: {:?}",
            drain.diagnostics
        );

        let t = Instant::now();
        runtime.shutdown().expect("shutdown");
        let shutdown = t.elapsed();

        println!(
            "timing run {run}: bootstrap_cold={bootstrap_cold:?} \
             bootstrap_cached={bootstrap_cached:?} entrypoint={} \
             spawn+handshake={spawn_handshake:?} first_cycle={first_cycle:?} \
             shutdown={shutdown:?}",
            plugin.manifest.entrypoint
        );
    }
}

/// The analyzed file's wire-form path: a real consumer package beside the
/// plugin (pubspec + `lib/main.dart`), the smoke fixture's shape — the
/// rule resolves (`canUseParsedResult` is false by default), and a bare
/// file outside any package has no analysis context to resolve through.
fn plugin_root_path_string(plugin_root: &TempDir) -> String {
    let consumer = plugin_root.path().join("consumer");
    std::fs::create_dir_all(consumer.join("lib")).expect("consumer lib");
    std::fs::write(
        consumer.join("pubspec.yaml"),
        "name: dq_boot_consumer\nenvironment:\n  sdk: ^3.10.0\n",
    )
    .expect("consumer pubspec");
    let main = consumer.join("lib").join("main.dart");
    std::fs::write(
        &main,
        "Future<void> main() async {\n  await Future.delayed(Duration.zero);\n}\n",
    )
    .expect("consumer file");
    main.display().to_string()
}
