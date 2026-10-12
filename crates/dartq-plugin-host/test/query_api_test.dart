/// Unit tests for the T4 Query API: the reverse `request/query` path of
/// `PluginSession` and the typed parsing of `QueryApi`.
///
/// Pins the client-role half of the reverse direction (the core's machine
/// is pinned in the Rust crate's tests): negotiation gating before any
/// write, write-before-park (the handler awaiting the answer cannot flush
/// anything itself), id correlation on the reply, the error channel as a
/// typed [QueryError], and an unmatched reply id as a contract break.
library;

import 'package:dartq_plugin_host/dartq_plugin_host.dart';
import 'package:test/test.dart';

PluginConfig _config([Set<Capability> capabilities = const {}]) => PluginConfig(
  name: 'test_plugin',
  version: '0.1.0',
  entrypoint: 'bin/plugin.dart',
  capabilities: CapabilitySet(capabilities),
);

/// A session handed its `initialize` with the core advertising [coreCaps].
/// Records every frame the session writes through its transport.
Future<(PluginSession, List<PluginFrame>)> _ready(
  Set<Capability> pluginCaps, [
  List<String> coreCaps = const ['query'],
  FixesHandler? onRequestFixes,
]) async {
  final written = <PluginFrame>[];
  final session = PluginSession(
    config: _config(pluginCaps),
    writeFrame: (frame) async {
      written.add(frame);
    },
    onRequestFixes: onRequestFixes,
  );
  await session.handleFrame(
    PluginFrame.request(1, methodInitialize, {
      'protocol_version': protocolVersion,
      'capabilities': coreCaps,
    }),
  );
  return (session, written);
}

/// Drains the microtask queue so an issued query's write has landed.
Future<void> _settle() => Future<void>.delayed(Duration.zero);

/// The [QueryError] matcher pinning a message part (the full text carries
/// more context — the wire's error string, the lifecycle state).
Matcher _queryError(Object messagePart) =>
    isA<QueryError>().having((error) => error.message, 'message', contains(messagePart));
void main() {
  group('reverse query transport', () {
    test('a negotiated query writes the request and completes on the reply',
        () async {
      final (session, written) = await _ready({Capability.query});
      final answer = session.query(
        const QueryRequest.named(QueryKind.symbol, 'Foo'),
      );

      await _settle();
      expect(written, hasLength(1), reason: 'the request went out, not queued');
      final request = written.single;
      expect(request.method, methodRequestQuery);
      expect(request.params['kind'], 'symbol');
      expect(request.params['name'], 'Foo');

      final reply = PluginFrame.request(request.id!, methodRequestQuery, {
        'schema_version': 1,
        'name': 'Foo',
        'declarations': <Object?>[],
        'references': <Object?>[],
      });
      await session.handleFrame(reply);
      expect(await answer, reply.params);
    });

    test('a fixes handler queries mid-request without deadlocking', () async {
      final (session, written) = await _ready(
        {Capability.query, Capability.fix},
        const ['query', 'fix'],
        (file, offset, length, queries) async {
          final elements = await queries.elementAt(file, offset);
          return [
            PrioritizedChange(
              priority: 1,
              message: 'elements: ${elements.length}',
            ),
          ];
        },
      );

      // The fixes request starts; its handler queries before returning —
      // the query request must cross the (still unanswered) fixes request
      // on the wire, written while the handler is parked on its answer.
      final fixes = session.handleFrame(
        PluginFrame.request(9, methodRequestFixes, {
          'file': 'lib/a.dart',
          'offset': 3,
          'length': 0,
        }),
      );
      await _settle();
      final query = written.single;
      expect(query.method, methodRequestQuery);
      expect(query.id, isNot(9), reason: 'the plugin id space is its own');

      // The core's answer routes to the parked handler (no reply frame of
      // its own), which then serves the fixes request.
      final queryReply = await session.handleFrame(
        PluginFrame.request(query.id!, methodRequestQuery, {
          'schema_version': 1,
          'file': 'lib/a.dart',
          'offset': 3,
          'elements': [
            {
              'file': 'lib/a.dart',
              'container': null,
              'kind': 'function',
              'name': 'fa',
              'line': 1,
              'column': 6,
              'length': 2,
            },
          ],
        }),
      );
      expect(queryReply, isNull);

      final frame = await fixes;
      expect(frame, isNotNull);
      final changes = frame!.params['fixes'] as List<Object?>;
      expect(changes, hasLength(1));
      expect(
        (changes.single as Map)['message'],
        'elements: 1',
        reason: 'the handler resumed on its query answer',
      );
    });
  });

  group('negotiation and lifecycle gates', () {
    test('a query without the plugin-side capability throws before writing',
        () async {
      final (session, written) = await _ready({Capability.lint});
      expect(
        () => session.query(const QueryRequest.named(QueryKind.symbol, 'x')),
        throwsA(_queryError('the `query` capability was not negotiated')),
      );
      await _settle();
      expect(written, isEmpty, reason: 'nothing is written un-negotiated');
    });

    test('a query without the core-side capability throws before writing',
        () async {
      // The plugin declares `query`; the core advertised nothing.
      final (session, written) = await _ready({Capability.query}, const []);
      expect(
        () => session.query(const QueryRequest.named(QueryKind.symbol, 'x')),
        throwsA(_queryError('the `query` capability was not negotiated')),
      );
      await _settle();
      expect(written, isEmpty);
    });

    test('a query before the handshake throws', () async {
      final session = PluginSession(config: _config({Capability.query}));
      expect(
        () => session.query(const QueryRequest.named(QueryKind.symbol, 'x')),
        throwsA(_queryError('the session is awaitingInitialize')),
      );
    });

    test('a query without a transport throws', () async {
      final session = PluginSession(config: _config({Capability.query}));
      await session.handleFrame(
        PluginFrame.request(1, methodInitialize, {
          'protocol_version': protocolVersion,
          'capabilities': ['query'],
        }),
      );
      expect(
        () => session.query(const QueryRequest.named(QueryKind.symbol, 'x')),
        throwsA(_queryError('no frame transport')),
      );
    });
  });

  group('reply correlation', () {
    test('the error channel surfaces as QueryError', () async {
      final (session, written) = await _ready({Capability.query});
      final answer = session.query(const QueryRequest.at('lib/gone.dart', 0));
      await _settle();
      await session.handleFrame(
        PluginFrame.request(written.single.id!, methodRequestQuery, {
          'error': 'unknown file `lib/gone.dart`',
        }),
      );
      await expectLater(
        answer,
        throwsA(_queryError('unknown file `lib/gone.dart`')),
      );
    });

    test('a reply id matching no pending query is a contract break',
        () async {
      final (session, _) = await _ready({Capability.query});
      try {
        await session.handleFrame(
          PluginFrame.request(99, methodRequestQuery, const {}),
        );
        fail('expected PluginProtocolException');
      } on PluginProtocolException catch (error) {
        expect(error.message, contains('99'));
      }
    });
  });

  group('QueryApi typed parsing', () {
    test('symbol parses declarations and references', () async {
      Future<Map<String, Object?>> transport(QueryRequest request) async {
        expect(request.kind, QueryKind.symbol);
        expect(request.name, 'fa');
        return {
          'schema_version': 1,
          'name': 'fa',
          'declarations': [
            {
              'file': 'lib/a.dart',
              'container': null,
              'kind': 'function',
              'name': 'fa',
              'line': 1,
              'column': 6,
              'length': 2,
            },
          ],
          'references': [
            {
              'target_file': 'lib/a.dart',
              'kind': 'invoked',
              'file': 'lib/b.dart',
              'offset': 27,
              'length': 2,
              'qualified': 'p.fa',
            },
          ],
        };
      }

      final answer = await QueryApi(transport).symbol('fa');
      expect(answer.declarations.single.name, 'fa');
      expect(answer.declarations.single.line, 1);
      final reference = answer.references.single;
      expect(reference.kind, 'invoked');
      expect(reference.qualified, 'p.fa');
      expect(reference.offset, 27);
    });

    test('elementAt parses the elements array', () async {
      Future<Map<String, Object?>> transport(QueryRequest request) async {
        expect(request.kind, QueryKind.elementAt);
        expect(request.file, 'lib/a.dart');
        expect(request.offset, 3);
        return {
          'schema_version': 1,
          'file': 'lib/a.dart',
          'offset': 3,
          'elements': <Object?>[],
        };
      }

      expect(await QueryApi(transport).elementAt('lib/a.dart', 3), isEmpty);
    });

    test('dependencies parses the path rows', () async {
      Future<Map<String, Object?>> transport(QueryRequest request) async {
        return {
          'schema_version': 1,
          'name': 'Foo',
          'declarations': <Object?>[],
          'references': <Object?>[],
          'dependencies': [
            {'path': 'lib/b.dart'},
            {'path': 'lib/c.dart'},
          ],
        };
      }

      expect(await QueryApi(transport).dependencies('Foo'), [
        'lib/b.dart',
        'lib/c.dart',
      ]);
    });

    test('a malformed reply array is a typed protocol exception', () async {
      Future<Map<String, Object?>> transport(QueryRequest request) async =>
          {'schema_version': 1};
      await expectLater(
        QueryApi(transport).implementations('Foo'),
        throwsA(isA<PluginProtocolException>()),
      );
    });
  });
}
