import 'dart:io';

import 'package:dartq_plugin_host/dartq_plugin_host.dart';

/// The rule code the fixture registers its fix under (`Plugin.register`).
const String ruleCode = 'live/fixed';

/// The deterministic answer to every `request/fixes`: one edit with a fixed
/// replacement on a fixed span, independent of the requested location. The
/// Rust side (`crates/dartq-plugin/tests/live_host.rs`) asserts these exact
/// values.
const int fixedPriority = 1;
const String fixedMessage = 'live fix';
const SourceEdit fixedEdit = SourceEdit(
  offset: 10,
  length: 4,
  replacement: 'fixed',
);

/// Live E2E fixture for dartq-plugin's `tests/live_host.rs`: a real plugin
/// process built on the package's [Plugin] lifecycle. Advertises the `fix`
/// capability, registers one deterministic fix, serves `request/fixes` with
/// it, and shuts down cleanly once the core closes its stdin.
///
/// Launched with the `crash` argument it answers the handshake, then exits
/// on the first work request — the mid-session death the runtime must
/// isolate. Never run interactively; the core launches it (`dart run`).
Future<void> main(List<String> args) async {
  final crashOnFirstFix = args.contains('crash');
  const plugin = _LivePlugin();
  final registry = PluginRegistryImpl();
  plugin.register(registry);
  await plugin.start();
  await runPluginHost(
    PluginConfig(
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
    onRequestFixes: (_file, _offset, _length) {
      if (crashOnFirstFix) {
        // Mid-session death: no reply, the connection just drops — the core
        // must report the death, never follow the plugin down.
        exit(3);
      }
      // The registered rule is what this session serves: without a live
      // registration the honest answer is the empty set.
      if (!registry.fixesByRule.containsKey(ruleCode)) {
        return const <PrioritizedChange>[];
      }
      return const <PrioritizedChange>[
        PrioritizedChange(
          priority: fixedPriority,
          message: fixedMessage,
          edits: [fixedEdit],
        ),
      ];
    },
  );
  // The core closed its stdin: the session is over, release resources.
  await plugin.shutDown();
}

/// The fixture plugin: one registered fix, no resources of its own.
class _LivePlugin extends Plugin {
  const _LivePlugin();

  @override
  String get name => 'live';

  @override
  void register(PluginRegistry registry) {
    registry.registerFixForRule(ruleCode, _fixedSpanFix);
  }
}

/// The registered fix: a fixed replacement on a fixed span for any location.
Producer _fixedSpanFix(ProducerContext context) => const _FixedSpanFix();

class _FixedSpanFix implements Producer {
  const _FixedSpanFix();

  @override
  int get priority => fixedPriority;

  @override
  String get message => fixedMessage;

  @override
  Future<List<SourceEdit>> compute(ProducerContext context) async => [
    fixedEdit,
  ];
}
