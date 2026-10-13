/// End-to-end producer execution through the plugin integration:
/// `servePlugin` (`package:dartq_plugin_host/host.dart`).
///
/// A real plugin instance — registration, lifecycle, producers — is served
/// over in-memory streams: the engine batch is pushed
/// (`notification/engine_diagnostics`), a `request/fixes` arrives, and the
/// registered producers' `compute` output comes back in the reply. This is
/// the path `bin/live_plugin.dart` exercises against the real core
/// (dartq-plugin's `tests/live_host.rs`); here it runs without processes.
library;

import 'dart:async';
import 'dart:convert';

import 'package:dartq_plugin_host/dartq_plugin_host.dart';
import 'package:test/test.dart';

List<int> _line(String line) => utf8.encode('$line\n');

List<int> _frame(PluginFrame frame) => _line(frame.encode());

final PluginFrame _initialize = PluginFrame.request(1, methodInitialize, {
  'protocol_version': protocolVersion,
});

PluginFrame _fixes(int id, String file, int offset, int length) =>
    PluginFrame.request(id, methodRequestFixes, {
      'file': file,
      'offset': offset,
      'length': length,
    });

PluginFrame _shutdown(int id) => PluginFrame.request(id, methodShutdown);

/// The fixture's rule diagnostic as an engine batch of one row.
PluginFrame _ruleRow(String file, int offset, int length) =>
    PluginFrame.notification(methodEngineDiagnostics, {
      'diagnostics': [
        {
          'file': file,
          'severity': 'warning',
          'code': ruleCode,
          'message': 'engine row',
          'offset': offset,
          'length': length,
        },
      ],
    });

/// The fixture's rule code; both plugins register fixes under it.
const String ruleCode = 'p/rule';

/// The plugin under test for single-producer behavior: one fix for one
/// rule, no assists — every request over its rule's diagnostic yields
/// exactly that producer's change.
class _RuleFixPlugin extends Plugin {
  const _RuleFixPlugin();

  @override
  String get name => 'rule_fix';

  @override
  void register(PluginRegistry registry) {
    registry.registerFixForRule(
      ruleCode,
      (context) => const _RangeFix('fix one', 1),
    );
  }
}

/// The multi-producer plugin: one assist and two fixes for the rule, with
/// async computes that echo the context they were handed — the requested
/// range for the assist, the fixing diagnostic's range for the rule fixes.
class _EchoPlugin extends Plugin {
  const _EchoPlugin();

  @override
  String get name => 'echo';

  @override
  void register(PluginRegistry registry) {
    registry.registerAssist((context) => const _RangeFix('assist', 5));
    registry.registerFixForRule(
      ruleCode,
      (context) => const _RangeFix('fix one', 1),
    );
    registry.registerFixForRule(
      ruleCode,
      (context) => const _RangeFix('fix two', 5),
    );
  }
}

class _RangeFix implements Producer {
  const _RangeFix(this.message, this.priority);

  @override
  final int priority;

  @override
  final String message;

  @override
  // Async on purpose: the serve path must await producers, not trust the
  // registration to be synchronous.
  Future<List<SourceEdit>> compute(ProducerContext context) =>
      Future<List<SourceEdit>>.delayed(
        Duration.zero,
        () => [
          SourceEdit(
            offset: context.offset,
            length: context.length,
            replacement: message,
          ),
        ],
      );
}

/// A plugin whose producer dies mid-`compute` — a producer failure is a
/// session teardown, never a fabricated reply.
class _BoomPlugin extends Plugin {
  const _BoomPlugin();

  @override
  String get name => 'boom';

  @override
  void register(PluginRegistry registry) {
    registry.registerFixForRule(ruleCode, (context) => const _BoomFix());
  }
}

class _BoomFix implements Producer {
  const _BoomFix();

  @override
  int get priority => 1;

  @override
  String get message => 'boom';

  @override
  Future<List<SourceEdit>> compute(ProducerContext context) async =>
      throw StateError('producer exploded');
}

/// A plugin recording its lifecycle calls.
class _LifecyclePlugin extends Plugin {
  _LifecyclePlugin(this.calls);

  final List<String> calls;

  @override
  String get name => 'lifecycle';

  @override
  void register(PluginRegistry registry) => calls.add('register');

  @override
  Future<void> start() async => calls.add('start');

  @override
  Future<void> shutDown() async => calls.add('shutDown');
}

/// A plugin whose assist producer pulls one reverse query before shaping
/// its change — the T4 Query API face over the real host loop.
class _QueryPlugin extends Plugin {
  const _QueryPlugin();

  @override
  String get name => 'query';

  @override
  void register(PluginRegistry registry) {
    registry.registerAssist((context) async {
      final elements = await context.queries!.elementAt(
        context.file,
        context.offset,
      );
      return _MessageProducer('elements: ${elements.length}');
    });
  }
}

/// The query answer as a message-only change.
class _MessageProducer implements Producer {
  const _MessageProducer(this.message);

  @override
  final int priority = 1;

  @override
  final String message;

  @override
  Future<List<SourceEdit>> compute(ProducerContext context) async =>
      const <SourceEdit>[];
}

/// Waits until [condition] holds, failing with [reason] after a bounded
/// wait — a deadlock must fail the test, not hang it.
Future<void> _waitFor(bool Function() condition, String reason) async {
  const budget = Duration(seconds: 10);
  final stop = DateTime.now().add(budget);
  while (!condition()) {
    if (DateTime.now().isAfter(stop)) {
      fail(reason);
    }
    await Future<void>.delayed(const Duration(milliseconds: 5));
  }
}

void main() {
  final config = PluginConfig(
    name: 'p',
    version: '0.1.0',
    entrypoint: 'bin/p.dart',
  );

  /// Starts [servePlugin] for [plugin] over a manual controller.
  (StreamController<List<int>>, List<String>, Future<void>) _start(
    Plugin plugin,
  ) {
    final sent = <String>[];
    final controller = StreamController<List<int>>();
    final done = servePlugin(
      plugin,
      config: config,
      input: controller.stream,
      send: (line) async => sent.add(line),
    );
    return (controller, sent, done);
  }

  /// Serves [chunks] for [plugin] to completion; returns every line the
  /// host sent, in send order.
  Future<List<String>> _serve(Plugin plugin, List<List<int>> chunks) async {
    final (controller, sent, done) = _start(plugin);
    for (final chunk in chunks) {
      controller.add(chunk);
    }
    await controller.close();
    await done;
    return sent;
  }

  test('a rule fix is served by running the registered producer', () async {
    final sent = await _serve(const _RuleFixPlugin(), [
      _frame(_initialize),
      // The engine batch is the fix context: the rule's diagnostic at
      // 10..14 on lib/a.dart.
      _frame(_ruleRow('lib/a.dart', 10, 4)),
      // The request names a different range; the producer still computes
      // for the diagnostic's span.
      _frame(_fixes(2, 'lib/a.dart', 12, 2)),
      _frame(_shutdown(3)),
    ]);

    final reply = sent.map(decodeFrame).singleWhere((frame) => frame.id == 2);
    expect(reply.params['fixes'], [
      {
        'priority': 1,
        'message': 'fix one',
        'edits': [
          // The edit echoes the diagnostic's range, not the request's —
          // the context the bridge passed came from the engine batch.
          {'offset': 10, 'length': 4, 'replacement': 'fix one'},
        ],
      },
    ]);
  });

  test(
    'a request matching no registered rule or file serves the empty set',
    () async {
      final sent = await _serve(const _RuleFixPlugin(), [
        _frame(_initialize),
        _frame(_ruleRow('lib/a.dart', 10, 4)),
        _frame(_fixes(2, 'lib/other.dart', 12, 2)), // wrong file
        _frame(_ruleRow('lib/a.dart', 30, 1)), // replaces the batch context
        _frame(_fixes(3, 'lib/a.dart', 100, 0)), // right file, disjoint range
        _frame(_shutdown(4)),
      ]);

      final replies = sent
          .map(decodeFrame)
          .where((frame) => frame.method == methodRequestFixes)
          .map((frame) => frame.params['fixes'])
          .toList();
      expect(replies, [isEmpty, isEmpty]);
    },
  );

  test('assists run at the requested range without an engine batch', () async {
    final sent = await _serve(const _EchoPlugin(), [
      _frame(_initialize),
      _frame(_fixes(2, 'lib/a.dart', 7, 3)),
      _frame(_shutdown(3)),
    ]);

    final reply = sent.map(decodeFrame).singleWhere((frame) => frame.id == 2);
    expect(reply.params['fixes'], [
      {
        'priority': 5,
        'message': 'assist',
        'edits': [
          {'offset': 7, 'length': 3, 'replacement': 'assist'},
        ],
      },
    ]);
  });

  test('changes are answered best first, ties in registration order', () async {
    final sent = await _serve(const _EchoPlugin(), [
      _frame(_initialize),
      _frame(_ruleRow('lib/a.dart', 10, 4)),
      _frame(_fixes(2, 'lib/a.dart', 10, 4)),
      _frame(_shutdown(3)),
    ]);

    final reply = sent.map(decodeFrame).singleWhere((frame) => frame.id == 2);
    expect(
      (reply.params['fixes'] as List).cast<Map>().map(
        (change) => change['message'],
      ),
      // fix one (priority 1) beats the priority-5 ties; the assist was
      // aggregated before the second fix generator, so it keeps its slot.
      ['fix one', 'assist', 'fix two'],
    );
  });

  test('only the latest engine batch is the fix context', () async {
    final sent = await _serve(const _RuleFixPlugin(), [
      _frame(_initialize),
      _frame(_ruleRow('lib/a.dart', 10, 4)), // analyze cycle 1
      _frame(
        PluginFrame.notification(methodEngineDiagnostics, const {
          'diagnostics': [],
        }),
      ), // analyze cycle 2: the rule is gone
      _frame(_fixes(2, 'lib/a.dart', 10, 4)),
      _frame(_shutdown(3)),
    ]);

    final reply = sent.map(decodeFrame).singleWhere((frame) => frame.id == 2);
    expect(reply.params['fixes'], isEmpty);
  });

  test(
    'register, start, and shutDown run in order around the session',
    () async {
      final calls = <String>[];
      await _serve(_LifecyclePlugin(calls), [
        _frame(_initialize),
        _frame(_shutdown(2)),
      ]);
      expect(calls, ['register', 'start', 'shutDown']);
    },
  );

  test('a producer failure tears the session down without a reply', () async {
    final (controller, sent, done) = _start(const _BoomPlugin());
    controller.add(_frame(_initialize));
    controller.add(_frame(_ruleRow('lib/a.dart', 10, 4)));
    controller.add(_frame(_fixes(2, 'lib/a.dart', 10, 4)));
    await controller.close();
    await expectLater(done, throwsStateError);

    final methods = sent.map(decodeFrame).map((frame) => frame.method);
    expect(methods, [
      methodInitialize,
    ], reason: 'the initialize reply went out, the fixes reply never did');
  });

  test(
    'a producer reverse query round trips while the loop keeps reading',
    () async {
      // The live shape: the query answer arrives on the input *while* the
      // fixes request is being served, so the loop must not hold the input
      // paused behind the parked handler (the query_probe deadlock).
      final sent = <String>[];
      final controller = StreamController<List<int>>();
      // Not awaited: on the deadlock regression this close would hang — the
      // paused subscription never delivers the done event.
      addTearDown(() {
        if (!controller.isClosed) {
          unawaited(controller.close());
        }
      });
      final done = servePlugin(
        const _QueryPlugin(),
        config: PluginConfig(
          name: 'p',
          version: '0.1.0',
          entrypoint: 'bin/p.dart',
          capabilities: CapabilitySet([Capability.query]),
        ),
        input: controller.stream,
        send: (line) async => sent.add(line),
      );
      controller.add(
        _frame(
          PluginFrame.request(1, methodInitialize, {
            'protocol_version': protocolVersion,
            'capabilities': ['query'],
          }),
        ),
      );
      controller.add(_frame(_fixes(2, 'lib/a.dart', 7, 3)));

      // The producer parks its fixes reply on the reverse query's answer.
      await _waitFor(
        () =>
            sent.any((line) => decodeFrame(line).method == methodRequestQuery),
        'the reverse query never reached the wire',
      );
      final query = sent
          .map(decodeFrame)
          .lastWhere((frame) => frame.method == methodRequestQuery);
      controller.add(
        _frame(
          PluginFrame.request(query.id!, methodRequestQuery, {
            'schema_version': 1,
            'file': 'lib/a.dart',
            'offset': 7,
            'elements': <Object?>[],
          }),
        ),
      );

      await _waitFor(
        () => sent.any((line) => decodeFrame(line).id == 2),
        'the fixes reply never came: the loop stopped reading while the '
        'producer was parked on its query answer',
      );
      await controller.close();
      await done;

      final reply = sent.map(decodeFrame).lastWhere((frame) => frame.id == 2);
      expect((reply.params['fixes'] as List).single['message'], 'elements: 0');
    },
  );
}
