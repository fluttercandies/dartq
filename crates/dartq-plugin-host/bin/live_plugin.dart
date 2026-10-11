import 'dart:io';

import 'package:dartq_plugin_host/dartq_plugin_host.dart';

/// The rule code the fixture registers its fix under (`Plugin.register`);
/// the Rust side (`crates/dartq-plugin/tests/live_host.rs`) feeds an engine
/// diagnostic carrying this code to give a `request/fixes` its fix context.
const String ruleCode = 'live/fixed';

/// The registered producer's answer, asserted by the Rust side: priority
/// [fixedPriority], title [fixedMessage], and one edit replacing the fixing
/// diagnostic's range (the producer context the host derives from the engine
/// batch) with [fixedReplacement] — real `compute` output, not a canned
/// reply.
const int fixedPriority = 1;
const String fixedMessage = 'live fix';
const String fixedReplacement = 'fixed';

/// Live E2E fixture for dartq-plugin's `tests/live_host.rs`: a real plugin
/// process on the package's integration path — [servePlugin] registers the
/// plugin, executes the registered producer's `compute` per `request/fixes`,
/// and serves its output. Advertises the `fix` capability and shuts down
/// cleanly once the core closes its stdin.
///
/// Launched with the `crash` argument the producer dies inside `compute` on
/// the first fix request — the mid-session death the runtime must isolate;
/// dying there also proves the host really executes registered producers.
/// Never run interactively; the core launches it (`dart run`).
Future<void> main(List<String> args) async {
  await servePlugin(
    _LivePlugin(crashOnFirstFix: args.contains('crash')),
    config: PluginConfig(
      name: 'live',
      version: '0.1.0',
      entrypoint: 'bin/live_plugin.dart',
      capabilities: CapabilitySet([Capability.lint, Capability.fix]),
    ),
    input: stdin,
    send: (line) async {
      stdout.writeln(line);
      await stdout.flush();
    },
  );
}

/// The fixture plugin: one registered fix, no resources of its own.
class _LivePlugin extends Plugin {
  const _LivePlugin({this.crashOnFirstFix = false});

  /// Whether the producer dies mid-`compute` instead of answering.
  final bool crashOnFirstFix;

  @override
  String get name => 'live';

  @override
  void register(PluginRegistry registry) {
    registry.registerFixForRule(
      ruleCode,
      (context) => _FixedSpanFix(crashOnFirstFix: crashOnFirstFix),
    );
  }
}

/// The registered fix: a fixed replacement over the fixing diagnostic's
/// range — the context the host passes is the diagnostic's span, whatever
/// range the request itself names.
class _FixedSpanFix implements Producer {
  const _FixedSpanFix({this.crashOnFirstFix = false});

  /// Whether `compute` dies mid-request (the crash mode).
  final bool crashOnFirstFix;

  @override
  int get priority => fixedPriority;

  @override
  String get message => fixedMessage;

  @override
  Future<List<SourceEdit>> compute(ProducerContext context) async {
    if (crashOnFirstFix) {
      // Mid-session death: no reply, the connection just drops — the core
      // must report the death, never follow the plugin down.
      exit(3);
    }
    return [
      SourceEdit(
        offset: context.offset,
        length: context.length,
        replacement: fixedReplacement,
      ),
    ];
  }
}
