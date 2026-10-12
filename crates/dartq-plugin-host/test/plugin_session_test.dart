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
        onRequestFixes: (file, offset, length, queries) => const [],
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
        onRequestFixes: (file, offset, length, queries) {
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
        onRequestFixes: (file, offset, length, queries) =>
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
          onRequestFixes: (file, offset, length, queries) => const [],
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
          onRequestFixes: (file, offset, length, queries) => throw StateError('boom'),
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
        onRequestFixes: (file, offset, length, queries) =>
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
      final received = <EngineBatch>[];
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
      // `files` is additive: absent means the empty analyzed set.
      expect(received.single.files, isEmpty);
      expect(received.single.diagnostics, hasLength(1));
      final diagnostic = received.single.diagnostics.single;
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

    test('the analyzed file set parses and validates entry by entry', () async {
      final received = <EngineBatch>[];
      final session = await _ready(
        onEngineDiagnostics: (batch) {
          received.add(batch);
          return const [];
        },
      );

      await session.handleFrame(
        PluginFrame.notification(methodEngineDiagnostics, {
          'diagnostics': <Object?>[],
          'files': ['lib/a.dart', 'lib/b.dart'],
        }),
      );
      expect(received.single.files, ['lib/a.dart', 'lib/b.dart']);

      final error = await _protocolError(
        () => session.handleFrame(
          PluginFrame.notification(methodEngineDiagnostics, {
            'diagnostics': <Object?>[],
            'files': ['lib/a.dart', 7],
          }),
        ),
      );
      expect(
        error.message,
        'engine_diagnostics `files` entries must be strings',
      );
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

  group('plugin config notification', () {
    PluginFrame _configFrame(Map<String, Object?> params) =>
        PluginFrame.notification(methodPluginConfig, params);

    Map<String, Object?> _validParams() => {
      'enabled_rules': ['My_Rule'],
      'severity_overrides': {'my_plugin/lint': 'error'},
      'overlays': [
        {'file': 'lib/a.dart', 'content': 'void main() {}'},
        {'file': 'lib/b.dart'},
      ],
    };

    Future<PluginSession> _ready({PluginConfigHandler? onPluginConfig}) async {
      final session = PluginSession(
        config: _config(),
        onPluginConfig: onPluginConfig,
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
        () => session.handleFrame(_configFrame(_validParams())),
      );
      expect(
        error.message,
        'notification/plugin_config is only valid after initialize completes',
      );
    });

    test('dispatches the parsed config: enablement lower-cased, overlays '
        'with nullable content', () async {
      final received = <PluginCycleConfig>[];
      final session = await _ready(onPluginConfig: received.add);

      final reply = await session.handleFrame(_configFrame(_validParams()));

      expect(reply, isNull);
      expect(received, hasLength(1));
      expect(received.single.enabledRules, {'my_rule'});
      expect(
        received.single.severityOverrides['my_plugin/lint'],
        Severity.error,
      );
      expect(received.single.overlays, hasLength(2));
      expect(received.single.overlays[0].file, 'lib/a.dart');
      expect(received.single.overlays[0].content, 'void main() {}');
      // `content: null` — the file fell back to disk.
      expect(received.single.overlays[1].file, 'lib/b.dart');
      expect(received.single.overlays[1].content, isNull);
      expect(received.single.overlayFor('lib/a.dart'), 'void main() {}');
      expect(received.single.overlayFor('lib/absent.dart'), isNull);
    });

    test(
      'a malformed payload is a fatal protocol error, field by field',
      () async {
        final received = <PluginCycleConfig>[];
        final session = await _ready(onPluginConfig: received.add);

        Future<String> fatal(Map<String, Object?> params) async =>
            (await _protocolError(
              () => session.handleFrame(_configFrame(params)),
            )).message;

        expect(
          await fatal({}),
          'plugin_config payload requires an `enabled_rules` array',
        );
        expect(
          await fatal({'enabled_rules': 'x'}),
          'plugin_config payload requires an `enabled_rules` array',
        );
        expect(
          await fatal({
            'enabled_rules': ['a'],
          }),
          'plugin_config payload requires a `severity_overrides` object',
        );
        expect(
          await fatal({
            'enabled_rules': ['a'],
            'severity_overrides': <String, Object?>{},
          }),
          'plugin_config payload requires an `overlays` array',
        );
        expect(
          await fatal({
            'enabled_rules': [42],
            'severity_overrides': <String, Object?>{},
            'overlays': <Object?>[],
          }),
          'plugin_config `enabled_rules` entries must be strings',
        );
        expect(
          await fatal({
            'enabled_rules': ['a'],
            'severity_overrides': {'c': 3},
            'overlays': <Object?>[],
          }),
          'plugin_config severity for `c` must be a string',
        );
        expect(
          await fatal({
            'enabled_rules': ['a'],
            'severity_overrides': {'c': 'catastrophic'},
            'overlays': <Object?>[],
          }),
          'severity must be error/warning/info, got `catastrophic`',
        );
        expect(
          await fatal({
            'enabled_rules': ['a'],
            'severity_overrides': <String, Object?>{},
            'overlays': ['a.dart'],
          }),
          'plugin_config `overlays` entries must be objects',
        );
        expect(
          await fatal({
            'enabled_rules': ['a'],
            'severity_overrides': <String, Object?>{},
            'overlays': [
              {'content': 'x'},
            ],
          }),
          'plugin_config overlay field `file` must be a string',
        );
        expect(received, isEmpty);
      },
    );

    test('an unknown core notification still names the method (the config '
        'did not absorb the switch default)', () async {
      final session = await _ready();
      final error = await _protocolError(
        () => session.handleFrame(
          PluginFrame.notification('notification/exotic'),
        ),
      );
      expect(
        error.message,
        'unknown notification `notification/exotic` in state ready',
      );
    });
  });

  group('census in the initialize reply', () {
    test('the reply carries the rule census verbatim', () async {
      final session = PluginSession(
        config: _config(CapabilitySet([Capability.lint])),
        ruleCensus: const [
          RuleCensus(
            name: 'my_rule',
            kind: RuleKind.lint,
            codes: [RuleCodeInfo(name: 'my_code', severity: 'warning')],
          ),
        ],
      );
      final reply = await session.handleFrame(
        PluginFrame.request(1, methodInitialize, {
          'protocol_version': protocolVersion,
        }),
      );
      expect(reply!.params['rules'], [
        {
          'name': 'my_rule',
          'kind': 'lint',
          'codes': [
            {'name': 'my_code', 'severity': 'warning'},
          ],
        },
      ]);
    });

    test('a plugin without rules advertises an empty census', () async {
      final session = PluginSession(config: _config());
      final reply = await session.handleFrame(
        PluginFrame.request(1, methodInitialize, {
          'protocol_version': protocolVersion,
        }),
      );
      expect(reply!.params['rules'], isEmpty);
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
