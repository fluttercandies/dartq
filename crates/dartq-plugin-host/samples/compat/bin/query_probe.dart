/// The query face's live probe for the compat sample: spawns the sample's
/// real entrypoint (`dart bin/compat_plugin.dart query`) as a subprocess and
/// plays the core against it over the stdio wire — answering the reverse
/// `request/query` frames from a fixture table — so the T4 face has a live
/// round trip that needs no Rust harness (the Rust live suites spawn the
/// sample without the `query` argument, and a query-serving core is the
/// daemon's job, not theirs).
///
/// The scenario walks every path of the query-driven producer:
///
/// 1. the handshake advertises `query` (the plugin declared it via the
///    launch argument; this probe's core mirrors it),
/// 2. the rule face still runs over the overlay — query mode changes
///    nothing else,
/// 3. a fixes request on a referenced name round trips `element_at` +
///    `references` and answers with a concrete edit embedding the
///    resolution facts,
/// 4. a fixes request on an unreferenced declaration answers message-only
///    (the removal-candidate decision a parsed-level producer cannot make),
/// 5. a miss (nothing resolved at the position) answers message-only —
///    empty elements is a legal answer, not a failure,
/// 6. an unknown file rides the wire's error channel into a `QueryError`
///    and degrades to a message-only change — the session stays up.
///
/// Run from the package root: `dart samples/compat/bin/query_probe.dart`.
/// Prints one PASS line per face and exits 0; the first mismatch throws.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:dartq_plugin_host/dartq_plugin_host.dart';

import '../lib/main.dart' as compat;

/// The overlay text this probe's core holds for `lib/a.dart`: the test
/// fixture plus one unreferenced declaration on line 2. `print` sits at
/// UTF-16 offset 14, `unused` at 34 (all-ASCII text, so units are plain).
const String fixture = 'void main() { print(true); }\nvoid unused() {}\n';

const int printOffset = 14;
const int printLength = 5;
const int unusedOffset = 34;
const int unusedLength = 6;

/// The element rows of the fixture index (the frozen query schema's
/// declaration rows; line/column are 1-based).
final Map<String, Object?> printElement = {
  'file': 'lib/a.dart',
  'container': null,
  'kind': 'function',
  'name': 'print',
  'line': 1,
  'column': printOffset + 1,
  'length': printLength,
};

final Map<String, Object?> unusedElement = {
  'file': 'lib/a.dart',
  'container': null,
  'kind': 'function',
  'name': 'unused',
  'line': 2,
  // Line 1 plus its newline ends at offset 29, so the 1-based column of the
  // name span is 34 - 29 + 1.
  'column': 6,
  'length': unusedLength,
};

/// The reference rows of the fixture index: two resolved uses of `print`
/// across two files (one through an import prefix).
final List<Object?> printReferences = [
  {
    'target_file': 'lib/a.dart',
    'kind': 'invoked',
    'file': 'lib/a.dart',
    'offset': printOffset,
    'length': printLength,
    'qualified': null,
  },
  {
    'target_file': 'lib/a.dart',
    'kind': 'invoked',
    'file': 'lib/b.dart',
    'offset': 4,
    'length': printLength,
    'qualified': 'p.print',
  },
];

/// A query the fixture index declines (the wire's error channel).
final class QueryDecline implements Exception {
  /// Builds the decline with its wire message.
  const QueryDecline(this.message);

  /// The message the error reply carries.
  final String message;
}

/// The fixture index: answers one reverse query's params, or throws
/// [QueryDecline] to ride the error channel.
Map<String, Object?> serveQuery(Map<String, Object?> params) {
  final kind = params['kind'];
  if (kind == 'element_at') {
    final file = params['file'] as String?;
    final offset = params['offset'] as int?;
    if (file == 'lib/gone.dart') {
      throw QueryDecline('unknown file `$file`');
    }
    if (file == 'lib/a.dart' && offset == printOffset) {
      return {'elements': [printElement]};
    }
    if (file == 'lib/a.dart' && offset == unusedOffset) {
      return {'elements': [unusedElement]};
    }
    // Everywhere else: nothing is resolved there — the legal empty answer.
    return {'elements': <Object?>[]};
  }
  if (kind == 'references') {
    if (params['name'] == 'print') {
      return {'references': printReferences};
    }
    return {'references': <Object?>[]};
  }
  throw QueryDecline('unsupported query kind `$kind`');
}

/// One mismatch: the probe's only failure shape (non-zero exit).
void expect(Object? actual, Object? expected, String label) {
  if (actual != expected) {
    throw StateError('$label: expected `$expected`, got `$actual`');
  }
  print('query_probe PASS: $label');
}

Future<void> main(List<String> args) async {
  final sampleDir = Directory.fromUri(Platform.script.resolve('../')).path;
  final process = await Process.start(
    'dart',
    const ['bin/compat_plugin.dart', 'query'],
    workingDirectory: sampleDir,
  );
  try {
    await probe(process).timeout(const Duration(minutes: 2));
  } on Object {
    // Any failure (mismatch, timeout, dead wire) must not orphan the
    // sample process.
    process.kill();
    rethrow;
  }
}

Future<void> probe(Process process) async {
  final prefix = compat.CompatPlugin.queryMessagePrefix;
  process.stderr.transform(utf8.decoder).listen(stderr.write);

  Future<void> send(PluginFrame frame) async {
    process.stdin.writeln(frame.encode());
    await process.stdin.flush();
  }

  // One sequential scan over the child's stdout: reverse queries are
  // answered in line order (the host serves frames in arrival order), so
  // the awaited frame is deterministic.
  final frames = StreamIterator<PluginFrame>(
    process.stdout
        .transform(utf8.decoder)
        .transform(const LineSplitter())
        .map(decodeFrame),
  );
  final diagnostics = <PluginFrame>[];

  Future<PluginFrame> readWhen(bool Function(PluginFrame frame) take) async {
    while (await frames.moveNext()) {
      final frame = frames.current;
      final id = frame.id;
      if (frame.method == methodRequestQuery && id != null) {
        try {
          final params = serveQuery(frame.params);
          await send(PluginFrame.request(id, methodRequestQuery, params));
        } on QueryDecline catch (decline) {
          await send(
            PluginFrame.error(id, methodRequestQuery, decline.message),
          );
        }
        continue;
      }
      if (frame.method == methodDiagnosticNotification) {
        diagnostics.add(frame);
        continue;
      }
      if (take(frame)) {
        return frame;
      }
    }
    throw StateError('query_probe: the child closed the wire mid-scenario');
  }

  // Face 1: the handshake carries the query capability and the unchanged
  // rule census.
  await send(PluginFrame.request(1, methodInitialize, {
    'protocol_version': protocolVersion,
    'capabilities': ['lint', 'fix', 'query'],
  }));
  final init = await readWhen(
    (frame) => frame.id == 1 && frame.method == methodInitialize,
  );
  // List `==` is identity in Dart, so the capability list compares joined.
  expect(
    (init.params['capabilities'] as List).join(' '),
    'lint fix query',
    'handshake advertises query',
  );
  expect(
    (init.params['rules'] as List).length,
    1,
    'rule census unchanged in query mode',
  );

  // Face 2: the rule face over the overlay — one finding at the literal.
  await send(PluginFrame.notification(methodPluginConfig, {
    'enabled_rules': const ['boolean_literal_review'],
    'severity_overrides': const <String, String>{},
    'overlays': [
      {'file': 'lib/a.dart', 'content': fixture},
    ],
  }));
  await send(PluginFrame.notification(methodEngineDiagnostics, {
    'diagnostics': const <Object?>[],
    'files': const ['lib/a.dart'],
  }));
  await readWhen((frame) => frame.method == methodDiagnosticNotification);
  final finding = diagnostics.single.params;
  expect(finding['code'], 'boolean_literal', 'rule face intact in query mode');
  expect(finding['offset'], 20, 'finding at the UTF-16 offset');

  // The four producer faces, each one fixes request. The expected messages
  // are built from the sample's own prefix constant, so a wording change in
  // one place moves both sides.
  Future<List<Map<String, Object?>>> requestFixes(
    int id,
    String file,
    int offset,
    int length,
  ) async {
    await send(PluginFrame.request(id, methodRequestFixes, {
      'file': file,
      'offset': offset,
      'length': length,
    }));
    final reply = await readWhen((frame) => frame.id == id);
    return (reply.params['fixes'] as List).cast<Map<String, Object?>>();
  }

  // Face 3: a referenced name — a concrete edit with the resolution facts.
  var changes = await requestFixes(10, 'lib/a.dart', printOffset, printLength);
  expect(changes.length, 2, 'referenced name: review plus suggestion');
  expect(
    changes[0]['priority'],
    compat.CompatPlugin.queryReviewPriority,
    'review sorts first',
  );
  expect(
    changes[0]['message'],
    '$prefix: `print` has 2 reference(s) across 2 file(s)',
    'review message states the resolution facts',
  );
  var edits = (changes[0]['edits'] as List).cast<Map<String, Object?>>();
  expect(edits.length, 1, 'the review carries one edit');
  expect(edits.single['offset'], printOffset, 'edit at the requested offset');
  expect(
    edits.single['replacement'],
    'print /* compat: 2 refs */',
    'edit embeds the facts',
  );
  expect(
    (changes[1]['edits'] as List).length,
    0,
    'the suggestion is message-only',
  );

  // Face 4: an unreferenced declaration — the removal-candidate decision.
  changes = await requestFixes(11, 'lib/a.dart', unusedOffset, unusedLength);
  expect(
    changes[0]['message'],
    '$prefix: `unused` has no resolved references; a removal candidate',
    'unreferenced name: removal candidate',
  );
  expect(
    (changes[0]['edits'] as List).length,
    0,
    'the decision is message-only',
  );

  // Face 5: a miss — empty elements is a legal answer.
  changes = await requestFixes(12, 'lib/a.dart', 29, 4);
  expect(
    changes[0]['message'],
    '$prefix: nothing resolved at lib/a.dart:29',
    'a miss answers message-only',
  );

  // Face 6: an unknown file — the error channel degrades, the session
  // stays up (the suggestion still rides the same reply).
  changes = await requestFixes(13, 'lib/gone.dart', 0, 0);
  expect(
    changes[0]['message'],
    '$prefix failed: unknown file `lib/gone.dart`',
    'a declined query degrades to a message-only change',
  );
  expect(changes.length, 2, 'the session survived the failed query');

  await send(PluginFrame.request(20, methodShutdown));
  await readWhen((frame) => frame.id == 20 && frame.method == methodShutdown);
  await process.stdin.close();
  final code = await process.exitCode;
  expect(code, 0, 'the sample exited cleanly');
  print('query_probe: all faces passed');
}
