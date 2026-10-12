/// Tests for the compatibility sample plugin's contract
/// (`samples/compat`, the porting guide of `README.md`).
///
/// Everything runs in-process over in-memory streams against the sample's
/// own `run` function — the same function `bin/compat_plugin.dart` launches:
/// the on-disk manifest parses, the handshake advertises exactly the
/// manifest's capabilities plus the rule census, the enabled rule executes
/// over the analyzed set (overlay text in, findings out), the rule is
/// disabled by default, severity overrides re-level the row, the fixes face
/// serves the rule fix and the assist in priority order at UTF-16 ranges,
/// and the crash mode tears the session down before answering.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:dartq_plugin_host/dartq_plugin_host.dart';
import 'package:test/test.dart';

import '../samples/compat/lib/main.dart' as compat;

List<int> _line(String line) => utf8.encode('$line\n');

List<int> _frame(PluginFrame frame) => _line(frame.encode());

/// Drives the sample's [compat.run] over in-memory chunks and returns every
/// line the host sent, in send order.
Future<List<String>> _runSample(
  List<List<int>> chunks, {
  List<String> args = const [],
}) async {
  final sent = <String>[];
  final controller = StreamController<List<int>>();
  final done = compat.run(
    args,
    input: controller.stream,
    send: (line) async => sent.add(line),
  );
  for (final chunk in chunks) {
    controller.add(chunk);
  }
  await controller.close();
  await done;
  return sent;
}

PluginFrame _initialize(int id) => PluginFrame.request(id, methodInitialize, {
  'protocol_version': protocolVersion,
});

PluginFrame _shutdown(int id) => PluginFrame.request(id, methodShutdown);

PluginFrame _config({
  List<String> enabledRules = const ['boolean_literal_review'],
  Map<String, String> severityOverrides = const {},
  List<Map<String, Object?>> overlays = const [],
}) => PluginFrame.notification(methodPluginConfig, {
  'enabled_rules': enabledRules,
  'severity_overrides': severityOverrides,
  'overlays': overlays,
});

PluginFrame _engineBatch({
  List<Map<String, Object?>> diagnostics = const [],
  List<String> files = const ['lib/a.dart'],
}) => PluginFrame.notification(methodEngineDiagnostics, {
  'diagnostics': diagnostics,
  'files': files,
});

/// The analyzed fixture, pushed as an overlay (the host parses this text —
/// no disk file involved). `true` sits at UTF-16 offset 20, length 4.
const String fixture = 'void main() { print(true); }\n';

const int trueOffset = 20;
const int trueLength = 4;

void main() {
  group('manifest', () {
    test('the on-disk dartq_plugin.json parses to the sample contract', () {
      final manifestFile = File(
        '${Directory.current.path}/samples/compat/dartq_plugin.json',
      );
      expect(
        manifestFile.existsSync(),
        isTrue,
        reason: 'run from the package root',
      );
      final decoded = jsonDecode(manifestFile.readAsStringSync());
      final config = PluginConfig.fromJson(
        Map<String, Object?>.from(decoded as Map),
      );

      expect(config.name, 'compat');
      expect(config.version, '0.1.0');
      expect(config.entrypoint, 'bin/compat_plugin.dart');
      expect(config.capabilities.names(), ['lint', 'fix']);
      expect(config.timeouts.callbackMs, 30000);
      expect(config.timeouts.requestMs, 60000);
      expect(config.timeouts.requestRetries, 3);
      expect(config.memoryBudgetMb, 512);
    });

    test('the handshake advertises the manifest capabilities and the rule '
        'census', () async {
      final sent = await _runSample([
        _frame(_initialize(1)),
        _frame(_shutdown(2)),
      ]);

      final init = decodeFrame(sent[0]);
      expect(init.method, methodInitialize);
      expect(init.params['protocol_version'], protocolVersion);
      expect(init.params['capabilities'], ['lint', 'fix']);
      // The census: the registered lint rule with its code and declared
      // severity — the core resolves per-cycle enablement against this.
      expect(init.params['rules'], [
        {
          'name': 'boolean_literal_review',
          'kind': 'lint',
          'codes': [
            {'name': 'boolean_literal', 'severity': 'warning'},
          ],
        },
      ]);
    });
  });

  group('rule execution', () {
    test('the enabled rule reports the fixture boolean literal over the '
        'overlay text', () async {
      final sent = await _runSample([
        _frame(_initialize(1)),
        _frame(
          _config(
            overlays: [
              {'file': 'lib/a.dart', 'content': fixture},
            ],
          ),
        ),
        _frame(_engineBatch()),
        _frame(_shutdown(2)),
      ]);

      final diagnostics = sent
          .map(decodeFrame)
          .where((frame) => frame.method == methodDiagnosticNotification)
          .toList();
      expect(diagnostics, hasLength(1));
      expect(
        diagnostics.single.params,
        PluginDiagnostic(
          file: 'lib/a.dart',
          severity: Severity.warning,
          code: 'boolean_literal',
          message: 'Boolean literals need a compat review.',
          offset: trueOffset,
          length: trueLength,
        ).toJson(),
      );
    });

    test('the rule is disabled by default: no config, no findings', () async {
      final sent = await _runSample([
        _frame(_initialize(1)),
        _frame(_engineBatch()),
        _frame(_shutdown(2)),
      ]);

      final diagnostics = sent
          .map(decodeFrame)
          .where((frame) => frame.method == methodDiagnosticNotification)
          .toList();
      expect(diagnostics, isEmpty);
    });

    test(
      'an empty enabled set keeps the rule off even with analyzed files',
      () async {
        final sent = await _runSample([
          _frame(_initialize(1)),
          _frame(
            _config(
              enabledRules: [],
              overlays: [
                {'file': 'lib/a.dart', 'content': fixture},
              ],
            ),
          ),
          _frame(_engineBatch()),
          _frame(_shutdown(2)),
        ]);

        final diagnostics = sent
            .map(decodeFrame)
            .where((frame) => frame.method == methodDiagnosticNotification)
            .toList();
        expect(diagnostics, isEmpty);
      },
    );

    test('a severity override re-levels the rule row', () async {
      final sent = await _runSample([
        _frame(_initialize(1)),
        _frame(
          _config(
            severityOverrides: {'boolean_literal': 'error'},
            overlays: [
              {'file': 'lib/a.dart', 'content': fixture},
            ],
          ),
        ),
        _frame(_engineBatch()),
        _frame(_shutdown(2)),
      ]);

      final diagnostics = sent
          .map(decodeFrame)
          .where((frame) => frame.method == methodDiagnosticNotification)
          .toList();
      expect(diagnostics, hasLength(1));
      expect(diagnostics.single.params['severity'], 'error');
      // Only the severity moved; the rest of the row stands.
      expect(diagnostics.single.params['code'], 'boolean_literal');
      expect(diagnostics.single.params['offset'], trueOffset);
    });
  });

  group('fix flow', () {
    test('request/fixes serves the rule fix at the finding and the assist at '
        'the requested range, best priority first', () async {
      final sent = await _runSample([
        _frame(_initialize(1)),
        _frame(
          _config(
            overlays: [
              {'file': 'lib/a.dart', 'content': fixture},
            ],
          ),
        ),
        _frame(_engineBatch()),
        _frame(
          PluginFrame.request(7, methodRequestFixes, {
            'file': 'lib/a.dart',
            'offset': trueOffset,
            'length': trueLength,
          }),
        ),
        _frame(_shutdown(2)),
      ]);

      final reply = sent.map(decodeFrame).singleWhere((frame) => frame.id == 7);
      expect(reply.method, methodRequestFixes);
      final fixes = (reply.params['fixes'] as List).cast<Map>();
      expect(fixes, hasLength(2));

      // First fix (priority 1): the finding's own range replaced with the
      // marker — the rule fix fired on the plugin's own finding.
      expect(fixes[0]['priority'], compat.CompatPlugin.fixPriority);
      expect(fixes[0]['message'], compat.CompatPlugin.fixMessage);
      final edits = (fixes[0]['edits'] as List).cast<Map>();
      expect(edits, hasLength(1));
      expect(edits[0]['offset'], trueOffset);
      expect(edits[0]['length'], trueLength);
      expect(edits[0]['replacement'], compat.CompatPlugin.fixReplacement);

      // Second (priority 50): the message-only assist.
      expect(fixes[1]['priority'], compat.CompatPlugin.suggestionPriority);
      expect(fixes[1]['message'], compat.CompatPlugin.suggestionMessage);
      expect((fixes[1]['edits'] as List), isEmpty);
    });
  });

  group('crash flow', () {
    test('crash mode tears the session down on the first interaction, '
        'answering nothing', () async {
      final sent = <String>[];
      final controller = StreamController<List<int>>();
      final done = compat.run(
        const ['crash'],
        input: controller.stream,
        send: (line) async => sent.add(line),
      );
      controller.add(_frame(_initialize(1)));
      // The handshake itself is answered; the fixes request is the first
      // producer interaction, and the crash throws before any reply is
      // produced.
      controller.add(
        _frame(
          PluginFrame.request(7, methodRequestFixes, {
            'file': 'lib/a.dart',
            'offset': 0,
            'length': 1,
          }),
        ),
      );
      await controller.close();
      await expectLater(done, throwsA(isA<StateError>()));

      final frames = sent.map(decodeFrame).toList();
      expect(frames, hasLength(1), reason: 'the crash answered nothing');
      expect(frames.single.method, methodInitialize);
    });
  });
}
