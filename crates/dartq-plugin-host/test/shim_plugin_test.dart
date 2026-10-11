/// Tests for the `analysis_server_plugin` import shim
/// (`lib/src/analysis_server_plugin_shim/`): a plugin written verbatim in
/// official style (`shim_official_plugin.dart`, no dartq imports) registers
/// its fix and assist, the registrations flow through the dartq mirror
/// registry onto the wire v1, and every unsupported face fails fast into the
/// core's `dartq/plugin_crashed` containment.
///
/// Static level only: frames run over in-memory streams. The live-process
/// faces (the core spawning a shim-launched plugin) belong to dartq-plugin's
/// `compat_live.rs` in the serial phase.
library;

import 'dart:async';
import 'dart:convert';

import 'package:analysis_server_plugin/edit/dart/correction_producer.dart';
import 'package:analysis_server_plugin/edit/dart/dart_fix_kind_priority.dart';
import 'package:analysis_server_plugin/plugin.dart';
import 'package:analysis_server_plugin/registry.dart';
import 'package:dartq_plugin_host/dartq_plugin_host.dart' as dartq;
import 'package:test/test.dart';

import 'shim_official_plugin.dart';

/// The manifest identity the wire harness serves the fixture under.
final _config = dartq.PluginConfig(
  name: 'shim-fixture',
  version: '0.1.0',
  entrypoint: 'test/shim_official_plugin.dart',
  capabilities: dartq.CapabilitySet([
    dartq.Capability.lint,
    dartq.Capability.fix,
    dartq.Capability.assist,
  ]),
);

/// One in-memory wire session against [startPlugin].
final class _Harness {
  final _input = StreamController<List<int>>();
  final lines = <String>[];

  /// Feeds one frame (encoded with the trailing newline the codec expects).
  void write(Map<String, Object?> frame) =>
      _input.add(utf8.encode('${jsonEncode(frame)}\n'));

  /// Closes stdin; [run] completes when the session ends.
  Future<void> close() => _input.close();

  Future<void> run() => startPlugin(
    plugin,
    config: _config,
    input: _input.stream,
    send: (line) async => lines.add(line),
  );

  /// The decoded replies, keyed by correlation id.
  Map<int, Map<String, Object?>> repliesById() {
    final replies = <int, Map<String, Object?>>{};
    for (final line in lines) {
      final decoded = jsonDecode(line);
      if (decoded is Map && decoded['id'] is int) {
        replies[decoded['id'] as int] = Map<String, Object?>.from(decoded);
      }
    }
    return replies;
  }
}

Map<String, Object?> _request(
  int id,
  String method,
  Map<String, Object?> params,
) => {
  'schema_version': 1,
  'id': id,
  'method': method,
  'params': params,
};

/// The registration pair a test drives: the shim registry handed to the
/// fixture's `register` and the dartq mirror registry its delegations land
/// in.
final class _Registration {
  final dartq.PluginRegistryImpl mirror = dartq.PluginRegistryImpl();
  late final PluginRegistryImpl shim = PluginRegistryImpl(mirror);
}

void main() {
  group('registration delegation', () {
    test('fix and assist registrations land in the dartq registry', () async {
      final registration = _Registration();
      plugin.register(registration.shim);

      expect(registration.mirror.fixesByRule.keys, ['shim/fixture_rule']);
      expect(registration.mirror.assists, hasLength(1));
    });

    test('the fix key is the code lower-case name (official bookkeeping)',
        () async {
      final registration = _Registration();
      plugin.register(registration.shim);
      expect(registration.mirror.fixesByRule.keys, [
        ShimFixturePlugin.code.lowerCaseName,
      ]);
    });

    test('the adapted assist carries the official kind, priority negated',
        () async {
      final registration = _Registration();
      plugin.register(registration.shim);

      final producer = await registration.mirror.assists.single(
        const dartq.ProducerContext(file: '/w/lib/a.dart', offset: 7, length: 3),
      );
      // Official ordering is higher-priority-first (`Fix.compareFixes`);
      // the wire sorts lower-first, so the adapter negates.
      expect(producer.priority, -(DartFixKindPriority.standard + 10));
      expect(producer.message, 'Assist the fixture range');
      final edits = await producer.compute(
        const dartq.ProducerContext(file: '/w/lib/a.dart', offset: 7, length: 3),
      );
      expect(edits.single.replacement, '/* shim assist */');
    });

    test('a producer without a kind is an ArgumentError (official message)',
        () async {
      final registration = _Registration();
      await expectLater(
        () => registration.shim.registerFixForRule(
          ShimFixturePlugin.code,
          _KindlessFix.new,
        ),
        throwsA(
          isA<ArgumentError>().having(
            (error) => error.message,
            'message',
            contains("must declare a non-null 'fixKind'"),
          ),
        ),
      );
    });
  });

  group('unsupported faces', () {
    test('registerLintRule is an UnsupportedError naming the anchor',
        () async {
      final registration = _Registration();
      plugin.register(registration.shim);
      await expectLater(
        () => registration.shim.registerLintRule(Object()),
        throwsA(
          isA<UnsupportedError>().having(
            (error) => error.message,
            'message',
            allOf(contains('registerLintRule'), contains('PRD section 14.2')),
          ),
        ),
      );
    });

    test('registerWarningRule is an UnsupportedError naming the anchor',
        () async {
      final registration = _Registration();
      plugin.register(registration.shim);
      await expectLater(
        () => registration.shim.registerWarningRule(Object()),
        throwsA(
          isA<UnsupportedError>().having(
            (error) => error.message,
            'message',
            contains('registerWarningRule'),
          ),
        ),
      );
    });
  });

  group('wire flow through startPlugin', () {
    test('fix and assist registrations flow through to the fixes reply',
        () async {
      final harness = _Harness();
      final done = harness.run();
      harness.write(_request(1, 'initialize', {'protocol_version': 1}));
      harness.write({
        'schema_version': 1,
        'id': null,
        'method': 'notification/engine_diagnostics',
        'params': {
          'diagnostics': [
            {
              'file': '/w/lib/main.dart',
              'severity': 'warning',
              'code': 'shim/fixture_rule',
              'message': 'The fixture rule fired.',
              'offset': 100,
              'length': 5,
            },
          ],
        },
      });
      harness.write(
        _request(2, 'request/fixes', {
          'file': '/w/lib/main.dart',
          'offset': 102,
          'length': 2,
        }),
      );
      harness.write(_request(3, 'shutdown', {}));
      await harness.close();
      await done;

      final replies = harness.repliesById();

      // Handshake answered in the wire's version.
      expect(replies[1]!['method'], 'initialize');
      expect(
        (replies[1]!['params'] as Map)['protocol_version'],
        dartq.protocolVersion,
      );

      // The fixes reply: the assist first (official priority 60 negated to
      // -60), then the fix (standard 50 negated to -50), each anchored at the
      // range its request or diagnostic carried.
      expect(replies[2]!['method'], 'request/fixes');
      final fixes = (replies[2]!['params'] as Map)['fixes'] as List;
      expect(fixes, hasLength(2));

      final assist = fixes[0] as Map;
      expect(assist['priority'], -(DartFixKindPriority.standard + 10));
      expect(assist['message'], 'Assist the fixture range');
      expect(assist['edits'], [
        {'offset': 102, 'length': 2, 'replacement': '/* shim assist */'},
      ]);

      final fix = fixes[1] as Map;
      expect(fix['priority'], -DartFixKindPriority.standard);
      expect(fix['message'], 'Fix the fixture range');
      // The fix runs at the matched diagnostic's range (100/5), not the
      // request's (102/2) — the official fix-to-diagnostic anchoring.
      expect(fix['edits'], [
        {'offset': 100, 'length': 5, 'replacement': '/* shim fix */'},
      ]);

      expect(replies[3]!['method'], 'shutdown');
    });

    test('a foreign protocol version answers an error frame, never a hang',
        () async {
      final harness = _Harness();
      final done = harness.run();
      harness.write(_request(1, 'initialize', {'protocol_version': 99}));
      await harness.close();
      await done;

      final replies = harness.repliesById();
      final error = (replies[1]!['params'] as Map)['error'] as String?;
      expect(error, contains('protocol_version'));
    });
  });
}

/// A fix producer whose kind was "forgotten" — exercises the official
/// registration-time validation.
class _KindlessFix extends CorrectionProducer {
  _KindlessFix({required super.context});

  @override
  CorrectionApplicability get applicability =>
      CorrectionApplicability.singleLocation;

  @override
  Future<List<SourceEdit>> compute() async => const [];
}
