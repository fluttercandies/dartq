/// Unit tests for the host main loop's framing:
/// `package:dartq_plugin_host/host.dart` (`runPluginHost`).
///
/// The wire framing responsibilities — the 16 MiB raw-byte bound, partial
/// frame buffering, blank-line skipping, and CRLF tolerance — live in the
/// loop, not in `decodeFrame`, so they are pinned here. Everything runs over
/// in-memory streams and a collecting sink: no processes, no real I/O.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import 'package:dartq_plugin_host/dartq_plugin_host.dart';
import 'package:test/test.dart';

List<int> _line(String line) => utf8.encode('$line\n');

List<int> _frame(PluginFrame frame) => _line(frame.encode());

/// Drives [runPluginHost] over in-memory chunks and returns every line the
/// host sent, in send order.
Future<List<String>> _serve(
  List<List<int>> chunks, {
  EngineDiagnosticsHandler? onEngineDiagnostics,
}) async {
  final sent = <String>[];
  final controller = StreamController<List<int>>();
  final done = runPluginHost(
    PluginConfig(name: 'p', version: '0.1.0', entrypoint: 'bin/p.dart'),
    input: controller.stream,
    send: (line) async => sent.add(line),
    onEngineDiagnostics: onEngineDiagnostics,
  );
  for (final chunk in chunks) {
    controller.add(chunk);
  }
  await controller.close();
  await done;
  return sent;
}

void main() {
  final initialize = PluginFrame.request(1, methodInitialize, {
    'protocol_version': protocolVersion,
  });

  group('line framing', () {
    test('serves each frame, skips blank lines, and tolerates CRLF', () async {
      final sent = await _serve([
        _frame(initialize),
        _line(''), // blank line between frames
        utf8.encode('${PluginFrame.request(2, methodShutdown).encode()}\r\n'),
      ]);

      expect(sent, hasLength(2));
      expect(decodeFrame(sent[0]).id, 1);
      expect(decodeFrame(sent[0]).method, methodInitialize);
      expect(decodeFrame(sent[1]).id, 2);
      expect(decodeFrame(sent[1]).method, methodShutdown);
    });

    test('buffers a partial frame until its newline arrives', () async {
      final line = utf8.encode('${initialize.encode()}\n');
      final sent = await _serve([
        line.sublist(0, 5), // partial, no newline
        line.sublist(5), // the rest including the newline
      ]);
      expect(sent, hasLength(1));
      expect(decodeFrame(sent.single).method, methodInitialize);
    });

    test('serves multiple frames delivered in one chunk', () async {
      final sent = await _serve([
        [..._frame(initialize), ..._frame(PluginFrame.request(2, methodShutdown))],
      ]);
      expect(sent, hasLength(2));
      expect(decodeFrame(sent[1]).method, methodShutdown);
    });

    test('one byte at a time still produces both replies', () async {
      final bytes = [
        ..._frame(initialize),
        ..._frame(PluginFrame.request(2, methodShutdown)),
      ];
      final sent = await _serve([for (final byte in bytes) [byte]]);
      expect(sent, hasLength(2));
      expect(decodeFrame(sent[0]).id, 1);
      expect(decodeFrame(sent[1]).id, 2);
    });

    test('malformed UTF-8 tears the host down (strict decode)', () async {
      final sent = <String>[];
      final controller = StreamController<List<int>>();
      final done = runPluginHost(
        PluginConfig(name: 'p', version: '0.1.0', entrypoint: 'bin/p.dart'),
        input: controller.stream,
        send: (line) async => sent.add(line),
      );
      controller.add([0xFF, 0x0A]); // invalid UTF-8, then a newline
      await controller.close();
      await expectLater(
        done,
        throwsA(isA<PluginProtocolException>().having(
          (error) => error.message,
          'message',
          contains('not valid UTF-8'),
        )),
      );
      expect(sent, isEmpty);
    });

    test('an unknown method after the handshake is answered with an error '
        'reply line', () async {
      final sent = await _serve([
        _frame(initialize),
        _frame(PluginFrame.request(7, 'plugin/getCompletions')),
      ]);
      expect(sent, hasLength(2));
      final reply = decodeFrame(sent[1]);
      expect(reply.id, 7);
      expect(reply.params['error'], contains('unknown method'));
    });
  });

  group('frame size bound', () {
    test('a chunk pushing the buffer past maxFrameBytes is fatal before any '
        'decode, with nothing sent', () async {
      final sent = <String>[];
      final controller = StreamController<List<int>>();
      final done = runPluginHost(
        PluginConfig(name: 'p', version: '0.1.0', entrypoint: 'bin/p.dart'),
        input: controller.stream,
        send: (line) async => sent.add(line),
      );
      controller.add(Uint8List(maxFrameBytes + 1));
      await controller.close();
      await expectLater(
        done,
        throwsA(isA<PluginProtocolException>().having(
          (error) => error.message,
          'message',
          'frame exceeds the $maxFrameBytes-byte frame limit',
        )),
      );
      expect(sent, isEmpty);
    });

    test('the bound counts raw buffered bytes including the newline: a '
        'maxFrameBytes-sized frame is rejected, maxFrameBytes-1 passes the '
        'gate', () async {
      // A frame whose *content* is exactly maxFrameBytes can never arrive:
      // with its newline the buffered total is maxFrameBytes+1, which trips
      // the gate whichever way the bytes are chunked.
      final controller = StreamController<List<int>>();
      final done = runPluginHost(
        PluginConfig(name: 'p', version: '0.1.0', entrypoint: 'bin/p.dart'),
        input: controller.stream,
        send: (line) async {},
      );
      controller.add(Uint8List(maxFrameBytes)); // content, no newline yet
      controller.add([0x0A]); // the newline pushes the total over the bound
      await controller.close();
      await expectLater(
        done,
        throwsA(isA<PluginProtocolException>().having(
          (error) => error.message,
          'message',
          contains('frame exceeds'),
        )),
      );

      // Content of maxFrameBytes-1 plus its newline is exactly the bound, so
      // the gate passes and the line reaches the decoder (which rejects it
      // as invalid JSON — proving it was not rejected as oversized).
      final controllerAtBound = StreamController<List<int>>();
      final atBound = runPluginHost(
        PluginConfig(name: 'p', version: '0.1.0', entrypoint: 'bin/p.dart'),
        input: controllerAtBound.stream,
        send: (line) async {},
      );
      controllerAtBound.add(
        Uint8List.fromList([...List.filled(maxFrameBytes - 1, 0x61), 0x0A]),
      );
      await controllerAtBound.close();
      await expectLater(
        atBound,
        throwsA(isA<PluginProtocolException>().having(
          (error) => error.message,
          'message',
          contains('not valid JSON'),
        )),
      );
    });
  });

  group('outbound flush ordering', () {
    test('replies and plugin-emitted diagnostics reach the sink in serve '
        'order', () async {
      final sent = await _serve([
        _frame(initialize),
        _frame(PluginFrame.notification(methodEngineDiagnostics, {
          'diagnostics': [],
        })),
      ], onEngineDiagnostics: (batch) {
        return const [
          PluginDiagnostic(
            file: 'a.dart',
            severity: Severity.warning,
            code: 'p/rule',
            message: 'found one',
          ),
        ];
      });

      expect(sent, hasLength(2));
      final reply = decodeFrame(sent[0]);
      expect(reply.method, methodInitialize);

      final diagnostic = decodeFrame(sent[1]);
      expect(diagnostic.isNotification, isTrue);
      expect(diagnostic.method, methodDiagnosticNotification);
      expect(diagnostic.params, const PluginDiagnostic(
        file: 'a.dart',
        severity: Severity.warning,
        code: 'p/rule',
        message: 'found one',
      ).toJson());
    });
  });

  group('trivial input', () {
    test('an empty stream completes without sending anything', () async {
      expect(await _serve(const []), isEmpty);
    });
  });
}
