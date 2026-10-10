/// Unit tests for the wire codec: `package:dartq_plugin_host/frame.dart`.
///
/// Mirrors the Rust codec's unit tests (`dartq-plugin`, `src/frame.rs`,
/// `mod tests`); every case here pins the **Dart** side's typed
/// `PluginProtocolException` shapes.
library;

import 'package:dartq_plugin_host/dartq_plugin_host.dart';
import 'package:test/test.dart';

PluginProtocolException _decodeError(String line) {
  try {
    decodeFrame(line);
  } on PluginProtocolException catch (error) {
    return error;
  }
  fail('expected decodeFrame to throw PluginProtocolException for: $line');
}

void main() {
  group('protocol constants', () {
    test('pin the shared wire schema (must match the Rust codec)', () {
      expect(protocolVersion, 1);
      expect(maxFrameBytes, 16 * 1024 * 1024);
    });
  });

  group('PluginFrame', () {
    test('request frame round-trips through encode and decode', () {
      final frame = PluginFrame.request(7, methodRequestFixes, {
        'file': 'lib/main.dart',
        'offset': 12,
      });
      final line = frame.encode();
      expect(line, isNot(contains('\n')));

      final decoded = decodeFrame(line);
      expect(decoded.schemaVersion, frame.schemaVersion);
      expect(decoded.id, frame.id);
      expect(decoded.method, frame.method);
      expect(decoded.params, frame.params);
      expect(decoded.isNotification, isFalse);
    });

    test('notification frame round-trips and has a null id', () {
      final frame = PluginFrame.notification(methodDiagnosticNotification, {
        'file': 'lib/main.dart',
        'severity': 'warning',
      });
      final decoded = decodeFrame(frame.encode());
      expect(decoded.id, isNull);
      expect(decoded.isNotification, isTrue);
      expect(decoded.method, methodDiagnosticNotification);
      expect(decoded.params, frame.params);
    });

    test('toJson carries the wire field names', () {
      final frame = PluginFrame.request(3, methodShutdown);
      expect(frame.toJson(), {
        'schema_version': 1,
        'id': 3,
        'method': 'shutdown',
        'params': const <String, Object?>{},
      });
    });

    test('error reply carries the message under the `error` key', () {
      final frame = PluginFrame.error(9, methodRequestFixes, 'plugin exploded');
      expect(frame.id, 9);
      expect(frame.params, {'error': 'plugin exploded'});
    });

    test('missing fields decode to typed defaults', () {
      // Mirrors the Rust `missing_fields_decode_to_typed_defaults`: a bare
      // notification has no `params` and an explicit null `id`.
      final frame = decodeFrame('{"schema_version":1,"id":null,"method":"x"}');
      expect(frame.isNotification, isTrue);
      expect(frame.params, isEmpty);

      // An absent `id` key decodes the same as an explicit null.
      final bare = decodeFrame('{"schema_version":1,"method":"x"}');
      expect(bare.isNotification, isTrue);

      // An absent `params` key becomes the empty map.
      expect(bare.params, isEmpty);
    });
  });

  group('decodeFrame happy path', () {
    test('accepts a request with explicit params and id', () {
      final frame = decodeFrame(
        '{"schema_version":1,"id":42,"method":"request/fixes",'
        '"params":{"file":"a.dart","offset":0,"length":3}}',
      );
      expect(frame.schemaVersion, 1);
      expect(frame.id, 42);
      expect(frame.method, methodRequestFixes);
      expect(frame.params, {'file': 'a.dart', 'offset': 0, 'length': 3});
    });

    test('accepts a null params alongside a null id', () {
      final frame = decodeFrame(
        '{"schema_version":1,"id":null,"method":"notification/diagnostic",'
        '"params":null}',
      );
      expect(frame.isNotification, isTrue);
      expect(frame.params, isEmpty);
    });
  });

  group('decodeFrame error paths', () {
    test('empty input is rejected before any decode', () {
      final error = _decodeError('');
      expect(error.message, 'frame is empty');
    });

    test('malformed JSON is rejected with the parser message', () {
      final error = _decodeError('{"schema_version":1,"method":');
      expect(error.message, startsWith('frame is not valid JSON:'));
    });

    test('valid JSON of the wrong shape is rejected', () {
      for (final line in ['[1,2,3]', '"hello"', '42', 'null', 'true']) {
        final error = _decodeError(line);
        expect(error.message, 'frame must be a JSON object', reason: 'input: $line');
      }
    });

    test('missing or non-integer schema_version is rejected', () {
      expect(
        _decodeError('{"method":"x"}').message,
        'frame `schema_version` must be an integer',
      );
      expect(
        _decodeError('{"schema_version":"1","method":"x"}').message,
        'frame `schema_version` must be an integer',
      );
      // JSON numbers may be fractional; a double is not an integer here.
      expect(
        _decodeError('{"schema_version":1.5,"method":"x"}').message,
        'frame `schema_version` must be an integer',
      );
    });

    test('foreign schema_version is rejected with both versions named', () {
      final error = _decodeError('{"schema_version":2,"method":"x"}');
      expect(error.message, 'unsupported schema_version 2; '
          'this build speaks version 1');
    });

    test('non-integer id is rejected', () {
      expect(
        _decodeError('{"schema_version":1,"id":"7","method":"x"}').message,
        'frame `id` must be an integer or null',
      );
      expect(
        _decodeError('{"schema_version":1,"id":7.5,"method":"x"}').message,
        'frame `id` must be an integer or null',
      );
    });

    test('missing, empty, or non-string method is rejected', () {
      final expected = 'frame `method` must be a non-empty string';
      expect(_decodeError('{"schema_version":1}').message, expected);
      expect(_decodeError('{"schema_version":1,"method":""}').message, expected);
      expect(_decodeError('{"schema_version":1,"method":42}').message, expected);
    });

    test('non-object params is rejected', () {
      expect(
        _decodeError('{"schema_version":1,"method":"x","params":[1]}').message,
        'frame `params` must be a JSON object',
      );
      expect(
        _decodeError('{"schema_version":1,"method":"x","params":"p"}').message,
        'frame `params` must be a JSON object',
      );
    });

    test('exception toString has the typed prefix', () {
      const error = PluginProtocolException('frame is empty');
      expect(error.toString(), 'PluginProtocolException: frame is empty');
    });
  });
}
