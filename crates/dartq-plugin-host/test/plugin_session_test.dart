/// Unit tests for the host-side session state machine:
/// `package:dartq_plugin_host/plugin_session.dart`.
///
/// Pins the server-role contract of the wire protocol: lifecycle gates,
/// id correlation on every reply, error replies (never crashes) for
/// sequencing violations but typed exceptions for payload contract breaks.
library;

import 'package:dartq_plugin_host/dartq_plugin_host.dart';
import 'package:test/test.dart';

PluginConfig _config([CapabilitySet? capabilities]) => PluginConfig(
  name: 'test_plugin',
  version: '0.1.0',
  entrypoint: 'bin/plugin.dart',
  capabilities: capabilities,
);

/// Asserts [reply] is the error reply for request [id]: same id, same
/// method, failure message under the `error` key containing [messagePart].
void _expectErrorReply(
  PluginFrame? reply,
  int id,
  String method,
  String messagePart,
) {
  expect(reply, isNotNull, reason: 'expected an error reply frame');
  expect(reply!.id, id, reason: 'error replies must echo the request id');
  expect(reply.method, method);
  expect(reply.params['error'], isA<String>());
  expect(reply.params['error'] as String, contains(messagePart));
}

/// Runs [act] and returns the [PluginProtocolException] its failed future
/// carries — contract breaks inside the async [PluginSession.handleFrame]
/// surface as failed futures, never as synchronous throws.
Future<PluginProtocolException> _protocolError(
  Future<PluginFrame?> Function() act,
) async {
  try {
    await act();
  } on PluginProtocolException catch (error) {
    return error;
  }
  fail('expected PluginProtocolException');
}

void main() {
  group('lifecycle state machine', () {
    test('starts in awaitingInitialize', () {
      final session = PluginSession(config: _config());
      expect(session.state, SessionState.awaitingInitialize);
    });

    test('initialize completes the handshake and reports ready', () async {
      final session = PluginSession(
        config: _config(CapabilitySet([Capability.lint, Capability.assist])),
      );
      final reply = await session.handleFrame(
        PluginFrame.request(1, methodInitialize, {
          'protocol_version': protocolVersion,
        }),
      );

      expect(session.state, SessionState.ready);
      expect(reply!.id, 1);
      expect(reply.method, methodInitialize);
      expect(reply.params['protocol_version'], protocolVersion);
      // Advertised in protocol order regardless of set construction order.
      expect(reply.params['capabilities'], ['lint', 'assist']);
    });

    test('initialize with a foreign protocol_version is refused and the '
        'session stays awaiting', () async {
      for (final requested in [0, 2, '1', null]) {
        final session = PluginSession(config: _config());
        final reply = await session.handleFrame(
          PluginFrame.request(5, methodInitialize, {
            'protocol_version': requested,
          }),
        );
        _expectErrorReply(
          reply,
          5,
          methodInitialize,
          'unsupported protocol_version',
        );
        expect(session.state, SessionState.awaitingInitialize);

        // The session was not corrupted: a correct handshake still works.
        final retry = await session.handleFrame(
          PluginFrame.request(6, methodInitialize, {
            'protocol_version': protocolVersion,
          }),
        );
        expect(retry!.params['protocol_version'], protocolVersion);
        expect(session.state, SessionState.ready);
      }
    });

    test('a second initialize after the handshake is refused', () async {
      final session = PluginSession(config: _config());
      await session.handleFrame(
        PluginFrame.request(1, methodInitialize, {
          'protocol_version': protocolVersion,
        }),
      );
      final reply = await session.handleFrame(
        PluginFrame.request(2, methodInitialize, {
          'protocol_version': protocolVersion,
        }),
      );
      _expectErrorReply(
        reply,
        2,
        methodInitialize,
        'initialize is only valid before the session is ready',
      );
      expect(session.state, SessionState.ready);
    });

    test('request/fixes before initialize is refused, not served', () async {
      final session = PluginSession(config: _config());
      final reply = await session.handleFrame(
        PluginFrame.request(3, methodRequestFixes, {
          'file': 'lib/main.dart',
          'offset': 0,
          'length': 0,
        }),
      );
      _expectErrorReply(
        reply,
        3,
        methodRequestFixes,
        'request/fixes is only valid after initialize completes',
      );
      expect(session.state, SessionState.awaitingInitialize);
    });

    test(
      'shutdown before initialize is refused and state is unchanged',
      () async {
        final session = PluginSession(config: _config());
        final reply = await session.handleFrame(
          PluginFrame.request(4, methodShutdown),
        );
        _expectErrorReply(
          reply,
          4,
          methodShutdown,
          'shutdown is only valid after initialize completes',
        );
        expect(session.state, SessionState.awaitingInitialize);
      },
    );

    test(
      'full cycle: initialize then shutdown terminates the session',
      () async {
        final session = PluginSession(config: _config());
        await session.handleFrame(
          PluginFrame.request(1, methodInitialize, {
            'protocol_version': protocolVersion,
          }),
        );
        final reply = await session.handleFrame(
          PluginFrame.request(9, methodShutdown),
        );
        expect(session.state, SessionState.terminated);
        expect(reply!.id, 9);
        expect(reply.method, methodShutdown);
        expect(reply.params, isEmpty);
      },
    );

    test('nothing is served after shutdown', () async {
      final session = PluginSession(config: _config());
      await session.handleFrame(
        PluginFrame.request(1, methodInitialize, {
          'protocol_version': protocolVersion,
        }),
      );
      await session.handleFrame(PluginFrame.request(2, methodShutdown));

      final lateFixes = await session.handleFrame(
        PluginFrame.request(3, methodRequestFixes, {
          'file': 'a.dart',
          'offset': 0,
          'length': 0,
        }),
      );
      _expectErrorReply(
        lateFixes,
        3,
        methodRequestFixes,
        'request/fixes is only valid after initialize completes',
      );

      final lateShutdown = await session.handleFrame(
        PluginFrame.request(4, methodShutdown),
      );
      _expectErrorReply(
        lateShutdown,
        4,
        methodShutdown,
        'shutdown is only valid after initialize completes',
      );
      expect(session.state, SessionState.terminated);
    });
  });

  group('request correlation and unknown methods', () {
    test('every reply echoes the id of the request it answers', () async {
      final session = PluginSession(
        config: _config(),
        onRequestFixes: (file, offset, length) => const [],
      );
      await session.handleFrame(
        PluginFrame.request(41, methodInitialize, {
          'protocol_version': protocolVersion,
        }),
      );
      final fixes = await session.handleFrame(
        PluginFrame.request(42, methodRequestFixes, {
          'file': 'a.dart',
          'offset': 1,
          'length': 2,
        }),
      );
      final shutdown = await session.handleFrame(
        PluginFrame.request(43, methodShutdown),
      );
      expect(fixes!.id, 42);
      expect(shutdown!.id, 43);
    });

    test(
      'unknown method is answered with an error reply, not a crash',
      () async {
        final session = PluginSession(config: _config());
        final reply = await session.handleFrame(
          PluginFrame.request(7, 'plugin/getCompletions'),
        );
        _expectErrorReply(reply, 7, 'plugin/getCompletions', 'unknown method');
        expect(session.state, SessionState.awaitingInitialize);
      },
    );
  });

  group('request/fixes serving', () {
    Future<PluginSession> _ready({FixesHandler? onRequestFixes}) async {
      final session = PluginSession(
        config: _config(),
        onRequestFixes: onRequestFixes,
      );
      await session.handleFrame(
        PluginFrame.request(1, methodInitialize, {
          'protocol_version': protocolVersion,
        }),
      );
      return session;
    }

    test('without a handler the honest answer is the empty fix set', () async {
      final session = await _ready();
      final reply = await session.handleFrame(
        PluginFrame.request(2, methodRequestFixes, {
          'file': 'lib/main.dart',
          'offset': 4,
          'length': 6,
        }),
      );
      expect(reply!.params['fixes'], isEmpty);
    });

    test('with a handler the arguments pass through and the changes are '
        'serialized best-first', () async {
      final calls = <(String, int, int)>[];
      final session = await _ready(
        onRequestFixes: (file, offset, length) {
          calls.add((file, offset, length));
          return [
            const PrioritizedChange(
              priority: 1,
              message: 'add const',
              edits: [SourceEdit(offset: 4, length: 6, replacement: 'const x')],
            ),
            const PrioritizedChange(priority: 2, message: 'rename only'),
          ];
        },
      );

      final reply = await session.handleFrame(
        PluginFrame.request(2, methodRequestFixes, {
          'file': 'lib/main.dart',
          'offset': 4,
          'length': 6,
        }),
      );

      expect(calls, [('lib/main.dart', 4, 6)]);
      expect(reply!.params['fixes'], [
        {
          'priority': 1,
          'message': 'add const',
          'edits': [
            {'offset': 4, 'length': 6, 'replacement': 'const x'},
          ],
        },
        {'priority': 2, 'message': 'rename only', 'edits': []},
      ]);
    });

    test('an async handler is awaited: the reply carries the future\'s '
        'changes', () async {
      final session = await _ready(
        onRequestFixes: (file, offset, length) =>
            Future<List<PrioritizedChange>>.delayed(
              Duration.zero,
              () => [
                const PrioritizedChange(
                  priority: 3,
                  message: 'computed asynchronously',
                  edits: [SourceEdit(offset: 1, length: 2, replacement: 'x')],
                ),
              ],
            ),
      );

      final reply = await session.handleFrame(
        PluginFrame.request(2, methodRequestFixes, {
          'file': 'a.dart',
          'offset': 1,
          'length': 2,
        }),
      );
      expect(reply!.params['fixes'], [
        {
          'priority': 3,
          'message': 'computed asynchronously',
          'edits': [
            {'offset': 1, 'length': 2, 'replacement': 'x'},
          ],
        },
      ]);
    });

    test(
      'a malformed payload is a fatal protocol error, not an error reply',
      () async {
        final session = await _ready(
          onRequestFixes: (file, offset, length) => const [],
        );
        for (final params in [
          {'offset': 0, 'length': 0}, // missing file
          {'file': 'a.dart', 'offset': '0', 'length': 0}, // string offset
          {'file': 'a.dart', 'offset': 0}, // missing length
        ]) {
          final error = await _protocolError(
            () => session.handleFrame(
              PluginFrame.request(2, methodRequestFixes, params),
            ),
          );
          expect(
            error.message,
            'request/fixes payload requires string `file`, int `offset`, int `length`',
          );
        }
      },
    );

    test(
      'a throwing handler propagates — the session never fabricates a fix',
      () async {
        final session = await _ready(
          onRequestFixes: (file, offset, length) => throw StateError('boom'),
        );
        await expectLater(
          session.handleFrame(
            PluginFrame.request(2, methodRequestFixes, {
              'file': 'a.dart',
              'offset': 0,
              'length': 0,
            }),
          ),
          throwsStateError,
        );
      },
    );

    test('a handler whose future fails propagates the same way', () async {
      final session = await _ready(
        onRequestFixes: (file, offset, length) =>
            Future<List<PrioritizedChange>>.error(StateError('async boom')),
      );
      await expectLater(
        session.handleFrame(
          PluginFrame.request(2, methodRequestFixes, {
            'file': 'a.dart',
            'offset': 0,
            'length': 0,
          }),
        ),
        throwsStateError,
      );
    });
  });

  group('engine diagnostics notification', () {
    PluginDiagnostic _diagnostic() => const PluginDiagnostic(
      file: 'lib/main.dart',
      severity: Severity.warning,
      code: 'my_plugin/lint',
      message: 'prefer const',
      offset: 3,
      length: 4,
    );

    PluginFrame _engineFrame(Object? diagnostics) => PluginFrame.notification(
      methodEngineDiagnostics,
      {'diagnostics': diagnostics},
    );

    Future<PluginSession> _ready({
      EngineDiagnosticsHandler? onEngineDiagnostics,
    }) async {
      final session = PluginSession(
        config: _config(),
        onEngineDiagnostics: onEngineDiagnostics,
      );
      await session.handleFrame(
        PluginFrame.request(1, methodInitialize, {
          'protocol_version': protocolVersion,
        }),
      );
      return session;
    }

    test('before initialize is a fatal protocol error', () async {
      final session = PluginSession(config: _config());
      final error = await _protocolError(
        () => session.handleFrame(_engineFrame([_diagnostic().toJson()])),
      );
      expect(
        error.message,
        'notification/engine_diagnostics is only valid after initialize completes',
      );
    });

    test('an unknown core notification is a fatal protocol error naming the '
        'method and state', () async {
      final session = PluginSession(config: _config());
      final error = await _protocolError(
        () => session.handleFrame(
          PluginFrame.notification('notification/exotic'),
        ),
      );
      expect(
        error.message,
        'unknown notification `notification/exotic` in state awaitingInitialize',
      );
    });

    test('dispatches parsed diagnostics to the handler and queues the '
        'returned batch as notification frames, in order', () async {
      final received = <List<PluginDiagnostic>>[];
      final session = await _ready(
        onEngineDiagnostics: (batch) {
          received.add(batch);
          return [
            const PluginDiagnostic(
              file: 'a.dart',
              severity: Severity.error,
              code: 'p/rule',
              message: 'found one',
            ),
          ];
        },
      );

      final reply = await session.handleFrame(
        _engineFrame([_diagnostic().toJson()]),
      );

      expect(
        reply,
        isNull,
        reason: 'notifications never produce a reply frame',
      );
      expect(received, hasLength(1));
      final diagnostic = received.single.single;
      expect(diagnostic.file, 'lib/main.dart');
      expect(diagnostic.severity, Severity.warning);
      expect(diagnostic.code, 'my_plugin/lint');
      expect(diagnostic.offset, 3);
      expect(diagnostic.length, 4);

      final outbound = session.drainOutbound();
      expect(outbound, hasLength(1));
      expect(outbound.single.isNotification, isTrue);
      expect(outbound.single.method, methodDiagnosticNotification);
      expect(outbound.single.params, {
        'file': 'a.dart',
        'severity': 'error',
        'code': 'p/rule',
        'message': 'found one',
        'offset': 0,
        'length': 0,
      });
    });

    test(
      'without a handler nothing is queued and entries are not parsed',
      () async {
        final session = await _ready();
        // No handler means the batch is never touched: even malformed entries
        // go through (the parse is part of the skipped handler call).
        final reply = await session.handleFrame(_engineFrame([42]));
        expect(reply, isNull);
        expect(session.drainOutbound(), isEmpty);
      },
    );

    test('requires a `diagnostics` array', () async {
      final session = await _ready(onEngineDiagnostics: (batch) => const []);
      expect(
        (await _protocolError(
          () => session.handleFrame(
            PluginFrame.notification(methodEngineDiagnostics),
          ),
        )).message,
        'engine_diagnostics payload requires a `diagnostics` array',
      );
      expect(
        (await _protocolError(
          () => session.handleFrame(_engineFrame(42)),
        )).message,
        'engine_diagnostics payload requires a `diagnostics` array',
      );
    });

    test('engine diagnostic entries are validated field by field', () async {
      final session = await _ready(onEngineDiagnostics: (batch) => const []);
      final valid = _diagnostic().toJson();

      // A full entry parses.
      await session.handleFrame(_engineFrame([valid]));
      expect(session.drainOutbound(), isEmpty);

      // offset/length default to zero-width when missing or null.
      await session.handleFrame(
        _engineFrame([
          {...valid, 'offset': null, 'length': null},
        ]),
      );
      Future<void> expectFatal(Object? entry, String messagePart) async {
        final error = await _protocolError(
          () => session.handleFrame(_engineFrame([entry])),
        );
        expect(error.message, contains(messagePart));
      }

      await expectFatal(42, 'engine diagnostic must be a JSON object');
      await expectFatal({
        ...valid,
        'severity': 'catastrophic',
      }, 'must be one of error/warning/info');
      await expectFatal({
        ...valid,
        'severity': 2,
      }, 'field `severity` must be a string');
      await expectFatal({...valid, 'file': 3}, 'field `file` must be a string');
      await expectFatal({
        ...valid,
        'code': null,
      }, 'field `code` must be a string');
      await expectFatal({
        ...valid,
        'message': [],
      }, 'field `message` must be a string');
      await expectFatal({...valid, 'offset': -1}, 'non-negative integer');
      await expectFatal({...valid, 'offset': 1.5}, 'non-negative integer');
      await expectFatal({...valid, 'length': 'x'}, 'non-negative integer');
    });
  });

  group('outbound diagnostic queue', () {
    test(
      'emitDiagnostic queues the notification frame; drain empties once',
      () {
        final session = PluginSession(config: _config());
        const diagnostic = PluginDiagnostic(
          file: 'a.dart',
          severity: Severity.info,
          code: 'p/rule',
          message: 'note',
          offset: 1,
          length: 2,
        );

        expect(session.drainOutbound(), isEmpty);

        session.emitDiagnostic(diagnostic);
        session.emitDiagnostic(diagnostic);
        final drained = session.drainOutbound();
        expect(drained, hasLength(2));
        for (final frame in drained) {
          expect(frame.isNotification, isTrue);
          expect(frame.method, methodDiagnosticNotification);
          expect(frame.params, diagnostic.toJson());
        }
        expect(session.drainOutbound(), isEmpty, reason: 'drain must clear');
      },
    );
  });
}
