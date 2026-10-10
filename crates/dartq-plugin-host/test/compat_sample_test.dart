/// Tests for the compatibility sample plugin's contract
/// (`samples/compat`, the porting guide of `README.md`).
///
/// Everything runs in-process over in-memory streams against the sample's
/// own `run` function — the same function `bin/compat_plugin.dart` launches:
/// the on-disk manifest parses, the handshake advertises exactly the
/// manifest's capabilities, the rule's diagnostics flow over a file set, the
/// fixes face serves prioritized edits at the requested UTF-16 range, and
/// the crash mode tears the session down before answering.
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

PluginFrame _engineBatch(List<Map<String, Object?>> rows) =>
    PluginFrame.notification(methodEngineDiagnostics, {'diagnostics': rows});

Map<String, Object?> _engineRow(
  String file,
  String code,
  int offset,
  int length,
) => {
  'file': file,
  'severity': 'error',
  'code': code,
  'message': 'engine row',
  'offset': offset,
  'length': length,
};

void main() {
  group('manifest', () {
    test('the on-disk dartq_plugin.json parses to the sample contract', () {
      final manifestFile = File(
        '${Directory.current.path}/samples/compat/dartq_plugin.json',
      );
      expect(manifestFile.existsSync(), isTrue, reason: 'run from the package root');
      final decoded = jsonDecode(manifestFile.readAsStringSync());
      final config = PluginConfig.fromJson(Map<String, Object?>.from(decoded as Map));

      expect(config.name, 'compat');
      expect(config.version, '0.1.0');
      expect(config.entrypoint, 'bin/compat_plugin.dart');
      expect(config.capabilities.names(), ['lint', 'fix']);
      expect(config.timeouts.callbackMs, 30000);
      expect(config.timeouts.requestMs, 60000);
      expect(config.timeouts.requestRetries, 3);
      expect(config.memoryBudgetMb, 512);
    });

    test('the sample runner advertises exactly the manifest capabilities', () async {
      final sent = await _runSample([
        _frame(_initialize(1)),
        _frame(_shutdown(2)),
      ]);

      final init = decodeFrame(sent[0]);
      expect(init.method, methodInitialize);
      expect(init.params['protocol_version'], protocolVersion);
      expect(init.params['capabilities'], ['lint', 'fix']);
    });
  });

  group('diagnostics flow', () {
    test('the rule reports companions for .dart engine rows, in arrival order',
        () async {
      final sent = await _runSample([
        _frame(_initialize(1)),
        _frame(
          _engineBatch([
            // Offset 8..10 spans the surrogate pair of an emoji: UTF-16
            // positions pass through untouched.
            _engineRow('lib/a.dart', 'uri_does_not_exist', 8, 2),
            _engineRow('notes.txt', 'some_code', 0, 4),
            _engineRow('lib/b.dart', 'unused_import', 10, 4),
          ]),
        ),
        _frame(_shutdown(2)),
      ]);

      // The .txt row is skipped; both .dart rows emit, in arrival order,
      // after the initialize reply and before the shutdown reply.
      final diagnostics = sent
          .map(decodeFrame)
          .where((frame) => frame.method == methodDiagnosticNotification)
          .toList();
      expect(diagnostics, hasLength(2));

      final first = diagnostics[0];
      expect(first.params, PluginDiagnostic(
        file: 'lib/a.dart',
        severity: Severity.warning,
        code: 'compat/companion',
        message: 'companion for `uri_does_not_exist`',
        offset: 8,
        length: 2,
      ).toJson());
      final second = diagnostics[1];
      expect(second.params['file'], 'lib/b.dart');
      expect(second.params['offset'], 10);
    });
  });

  group('fix flow', () {
    test('request/fixes serves the range fix then the suggestion, in UTF-16',
        () async {
      final sent = await _runSample([
        _frame(_initialize(1)),
        _frame(
          PluginFrame.request(7, methodRequestFixes, {
            'file': 'lib/a.dart',
            'offset': 10,
            'length': 4,
          }),
        ),
        _frame(_shutdown(2)),
      ]);

      final reply = sent.map(decodeFrame).singleWhere(
        (frame) => frame.id == 7,
      );
      expect(reply.method, methodRequestFixes);
      final fixes = (reply.params['fixes'] as List).cast<Map>();
      expect(fixes, hasLength(2));

      // First fix: the requested UTF-16 range replaced with the marker.
      expect(fixes[0]['priority'], compat.CompatPlugin.fixPriority);
      expect(fixes[0]['message'], compat.CompatPlugin.fixMessage);
      final edits = (fixes[0]['edits'] as List).cast<Map>();
      expect(edits, hasLength(1));
      expect(edits[0]['offset'], 10);
      expect(edits[0]['length'], 4);
      expect(edits[0]['replacement'], compat.CompatPlugin.fixReplacement);

      // Second fix: a message-only suggestion (no edits).
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
      // The handshake itself is answered; the engine batch is the first
      // interaction, and the crash throws before any reply is produced.
      controller.add(_frame(_engineBatch([_engineRow('lib/a.dart', 'c', 0, 1)])));
      await controller.close();
      await expectLater(done, throwsA(isA<StateError>()));

      final frames = sent.map(decodeFrame).toList();
      expect(frames, hasLength(1), reason: 'the crash answered nothing');
      expect(frames.single.method, methodInitialize);
    });
  });
}
