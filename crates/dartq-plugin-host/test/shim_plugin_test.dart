/// Tests for the `analysis_server_plugin` import shim
/// (`lib/src/analysis_server_plugin_shim/`): a plugin written verbatim in
/// official style (`shim_official_plugin.dart`, no dartq imports) registers
/// a real analyzer lint rule, a fix, and an assist; the registrations flow
/// through the dartq mirror registry onto the wire v1 — census in the
/// handshake, rule execution over the analyzed set, the fixes reply — and a
/// producer without a kind fails fast into the core's `dartq/plugin_crashed`
/// containment.
///
/// Static level only: frames run over in-memory streams. The live-process
/// faces (the core spawning a shim-launched plugin) belong to dartq-plugin's
/// `compat_live.rs` in the serial phase.
library;

import 'dart:async';
import 'dart:convert';

import 'package:analysis_server_plugin/edit/change_builder/change_builder.dart';
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

  /// The plugin-emitted diagnostic rows, in send order.
  List<Map<String, Object?>> diagnostics() {
    final rows = <Map<String, Object?>>[];
    for (final line in lines) {
      final decoded = jsonDecode(line);
      if (decoded is Map &&
          decoded['id'] == null &&
          decoded['method'] == 'notification/diagnostic') {
        rows.add(Map<String, Object?>.from(decoded['params'] as Map));
      }
    }
    return rows;
  }
}

Map<String, Object?> _request(
  int id,
  String method,
  Map<String, Object?> params,
) => {'schema_version': 1, 'id': id, 'method': method, 'params': params};

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

      expect(registration.mirror.fixesByRule.keys, [
        'shim/fixture_rule',
        'shim/fixture_resolved_rule',
      ]);
      expect(registration.mirror.assists, hasLength(1));

      // The authoring-family fix reads its kind through the same adapted
      // generator face: the ResolvedCorrectionProducer tearoff fits the
      // official ProducerGenerator shape (covariant through
      // `ResolvedUnitResult implements ParsedUnitResult`).
      final generators =
          registration.mirror.fixesByRule['shim/fixture_resolved_rule']!;
      final producer = await generators.single(
        const dartq.ProducerContext(
          file: '/w/lib/a.dart',
          offset: 7,
          length: 3,
        ),
      );
      expect(producer.priority, -DartFixKindPriority.standard);
      expect(producer.message, 'Fix the fixture range (resolved)');
    });

    test(
      'the fix key is the code lower-case name (official bookkeeping)',
      () async {
        final registration = _Registration();
        plugin.register(registration.shim);
        expect(registration.mirror.fixesByRule.keys, [
          ShimFixturePlugin.code.lowerCaseName,
          ShimFixturePlugin.resolvedCode.lowerCaseName,
        ]);
      },
    );

    test(
      'the adapted assist carries the official kind, priority negated',
      () async {
        final registration = _Registration();
        plugin.register(registration.shim);

        final producer = await registration.mirror.assists.single(
          const dartq.ProducerContext(
            file: '/w/lib/a.dart',
            offset: 7,
            length: 3,
          ),
        );
        // Official ordering is higher-priority-first (`Fix.compareFixes`);
        // the wire sorts lower-first, so the adapter negates.
        expect(producer.priority, -(DartFixKindPriority.standard + 10));
        expect(producer.message, 'Assist the fixture range');
        final edits = await producer.compute(
          const dartq.ProducerContext(
            file: '/w/lib/a.dart',
            offset: 7,
            length: 3,
          ),
        );
        expect(edits.single.replacement, '/* shim assist */');
      },
    );

    test(
      'a producer without a kind is an ArgumentError (official message)',
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
      },
    );
  });

  group('rule delegation (T5)', () {
    test(
      'registerLintRule lands the analyzer rule in the dartq registry',
      () async {
        final registration = _Registration();
        plugin.register(registration.shim);

        expect(registration.mirror.lintRules.keys, ['fixture_boolean_review']);
        expect(
          registration.mirror.lintRules['fixture_boolean_review'],
          isA<FixtureRule>(),
        );
      },
    );

    test('registerWarningRule lands in the warning map', () async {
      final registration = _Registration();
      plugin.register(registration.shim);
      registration.shim.registerWarningRule(FixtureRule());

      expect(registration.mirror.warningRules.keys, ['fixture_boolean_review']);
      expect(registration.mirror.lintRules, isNotEmpty);
    });

    test('the census carries the shim-registered rule', () async {
      final registration = _Registration();
      plugin.register(registration.shim);

      final census = registration.mirror.census();
      expect(census, hasLength(1));
      expect(census.single.name, 'fixture_boolean_review');
      expect(census.single.kind, dartq.RuleKind.lint);
      expect(census.single.codes.single.name, 'fixture_boolean');
      // The LintCode's declared severity (the official default INFO), in
      // wire form.
      expect(census.single.codes.single.severity, 'info');
    });
  });

  group('wire flow through startPlugin', () {
    test(
      'fix and assist registrations flow through to the fixes reply',
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
      },
    );

    test(
      'a foreign protocol version answers an error frame, never a hang',
      () async {
        final harness = _Harness();
        final done = harness.run();
        harness.write(_request(1, 'initialize', {'protocol_version': 99}));
        await harness.close();
        await done;

        final replies = harness.repliesById();
        final error = (replies[1]!['params'] as Map)['error'] as String?;
        expect(error, contains('protocol_version'));
      },
    );

    test('the shim-registered rule is censused and executes over the '
        'analyzed set', () async {
      final harness = _Harness();
      final done = harness.run();
      harness.write(_request(1, 'initialize', {'protocol_version': 1}));
      harness.write({
        'schema_version': 1,
        'id': null,
        'method': 'notification/plugin_config',
        'params': {
          'enabled_rules': ['fixture_boolean_review'],
          'severity_overrides': <String, Object?>{},
          'overlays': [
            {
              'file': '/w/lib/main.dart',
              'content': 'void main() { print(true); }',
            },
          ],
        },
      });
      harness.write({
        'schema_version': 1,
        'id': null,
        'method': 'notification/engine_diagnostics',
        'params': {
          'diagnostics': <Object?>[],
          'files': ['/w/lib/main.dart'],
        },
      });
      harness.write(_request(2, 'shutdown', {}));
      await harness.close();
      await done;

      final replies = harness.repliesById();

      // The census rode the initialize reply (the core resolves enablement
      // against it).
      expect((replies[1]!['params'] as Map)['rules'], [
        {
          'name': 'fixture_boolean_review',
          'kind': 'lint',
          'codes': [
            {'name': 'fixture_boolean', 'severity': 'info'},
          ],
        },
      ]);

      // The enabled rule executed over the overlay text: one finding at the
      // boolean literal, declared severity (no override).
      final rows = harness.diagnostics();
      expect(rows, hasLength(1));
      expect(rows.single, {
        'file': '/w/lib/main.dart',
        'severity': 'info',
        'code': 'fixture_boolean',
        'message': 'Boolean literals need a fixture review.',
        'offset': 20,
        'length': 4,
      });
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
  Future<void> compute(ChangeBuilder builder) async {}
}
