// census_client.dart — speaks wire v1 to one vendored plugin's sidecar and
// prints its registration census in the deterministic expectations format.
//
// Usage: dart census_client.dart <plugin-dir>
//
// The plugin dir holds `dartq_plugin.json` (the manifest), `bin/dartq_serve.dart`
// (the manifest entrypoint) and `.dart_tool/package_config.json` (the
// injected config). This client plays the core's launch role exactly
// (crates/dartq-plugin, SpawnSpec::dart): it runs `dart <entrypoint>` inside
// the plugin dir, opens with `initialize`, prints the reply's rule census,
// then shuts the session down. Exit codes: 0 ok, 1 usage, 2 handshake
// failure or timeout (the child's captured stderr is printed first); a
// non-JSON stdout line from the child is also a protocol failure.
import 'dart:async';
import 'dart:convert';
import 'dart:io';

const _protocolVersion = 1;
const _initializeDeadline = Duration(seconds: 60);
const _shutdownDeadline = Duration(seconds: 15);

/// Wire v1 offers every capability the protocol defines; the effective set
/// is the intersection with the plugin's manifest, so the census and the
/// negotiation surface both land in the output.
const _offeredCapabilities = ['lint', 'fix', 'assist', 'query'];

Future<void> main(List<String> args) async {
  if (args.length != 1) {
    stderr.writeln('usage: dart census_client.dart <plugin-dir>');
    exit(1);
  }
  final pluginDir = Directory(args[0]);
  if (!pluginDir.existsSync()) {
    stderr.writeln('census_client: no such plugin dir: ${args[0]}');
    exit(1);
  }
  final manifest = jsonDecode(
    File('${pluginDir.path}/dartq_plugin.json').readAsStringSync(),
  ) as Map<String, Object?>;
  final entrypoint = manifest['entrypoint'] as String;

  final child = await Process.start(
    'dart',
    [entrypoint],
    workingDirectory: pluginDir.path,
  );
  final stderrTail = <String>[];
  child.stderr
      .transform(utf8.decoder)
      .listen(stderrTail.add, onDone: () {});

  // Broadcast: the handshake and shutdown waits subscribe sequentially, and
  // `firstWhere` cancels its subscription after matching, so a
  // single-subscription stream would reject the second listen. Every reply
  // follows a request this client writes after the previous wait has
  // resumed, so no reply can land in a no-listener window.
  final replies = StreamController<Map<String, Object?>>.broadcast();

  // A non-JSON stdout line (e.g. a Dart first-run analytics notice on a
  // fresh HOME) breaks the one-JSON-object-per-line wire contract and is
  // unrecoverable: report it with the child's stderr instead of dying on
  // an unhandled decode error.
  Never badStdoutLine(String line, Object error) {
    child.kill(ProcessSignal.sigkill);
    stderr.writeln(
      'census_client: non-JSON stdout line from child: $line ($error); '
      'child stderr:\n${stderrTail.join()}',
    );
    exit(2);
  }

  // The next reply frame matching [requestId] and [method]; null when the
  // budget expired or the child's stdout closed before a matching reply.
  // `orElse` keeps a closed stream (child death) from surfacing as a
  // "No element" state error.
  Future<Map<String, Object?>?> nextReply(
    int requestId,
    String method,
    Duration budget,
  ) async {
    final frame = await replies.stream
        .firstWhere(
          (frame) => frame['id'] == requestId && frame['method'] == method,
          orElse: () => const <String, Object?>{},
        )
        .timeout(budget, onTimeout: () => const <String, Object?>{});
    // A reply always carries id and method, so an empty frame can only be
    // the closed-stream or timeout marker.
    return frame.isEmpty ? null : frame;
  }

  child.stdout
      .transform(utf8.decoder)
      .transform(const LineSplitter())
      .listen(
        (line) {
          if (line.isEmpty) return;
          final Map<String, Object?> frame;
          try {
            frame = jsonDecode(line) as Map<String, Object?>;
          } on FormatException catch (error) {
            badStdoutLine(line, error);
          } on TypeError catch (error) {
            badStdoutLine(line, error);
          }
          // The wire carries the method payload under `params` (the Rust
          // core reads `frame.params`); lift it onto the frame so the
          // reply readers below see `protocol_version`, `capabilities`,
          // `rules`, and `error` directly.
          final params = frame['params'];
          replies.add(
            params is Map<String, Object?> ? {...frame, ...params} : frame,
          );
        },
        onDone: replies.close,
      );

  _write(child, {
    'schema_version': _protocolVersion,
    'id': 1,
    'method': 'initialize',
    'params': {
      'protocol_version': _protocolVersion,
      'capabilities': _offeredCapabilities,
    },
  });

  final reply = await nextReply(1, 'initialize', _initializeDeadline);
  if (reply == null) {
    child.kill(ProcessSignal.sigkill);
    stderr.writeln(
      'census_client: no initialize reply within '
      '${_initializeDeadline.inSeconds}s (timeout or child exit); '
      'child stderr:\n${stderrTail.join()}',
    );
    exit(2);
  }
  if (reply['error'] != null) {
    child.kill(ProcessSignal.sigkill);
    stderr.writeln('census_client: initialize error: ${reply['error']}');
    exit(2);
  }

  final version = reply['protocol_version'];
  if (version != _protocolVersion) {
    child.kill(ProcessSignal.sigkill);
    stderr.writeln(
      'census_client: protocol version mismatch: host said $version',
    );
    exit(2);
  }

  // Deterministic census format: fixed header, then one tab-separated row
  // per rule in registration order. This file is the enablement ground
  // truth (lint = opt-in, warning = default-on) for the preset options.
  final capabilities = (reply['capabilities'] as List<Object?>? ?? [])
      .cast<String>()
      .join(',');
  final rules = (reply['rules'] as List<Object?>? ?? const [])
      .cast<Map<String, Object?>>();
  var codeCount = 0;
  final rows = <String>[];
  for (final rule in rules) {
    final codes = (rule['codes'] as List<Object?>? ?? const [])
        .cast<Map<String, Object?>>()
        .map((c) => '${c['name']}=${c['severity']}')
        .join(',');
    codeCount += (rule['codes'] as List<Object?>? ?? const []).length;
    rows.add('${rule['name']}\t${rule['kind']}\t$codes');
  }
  stdout.writeln('# plugin census: ${manifest['name']}');
  stdout.writeln('# manifest_version: ${manifest['version']}');
  stdout.writeln('# protocol_version: $_protocolVersion');
  stdout.writeln('# negotiated_capabilities: $capabilities');
  stdout.writeln('# rule_count: ${rules.length}');
  stdout.writeln('# code_count: $codeCount');
  for (final row in rows) {
    stdout.writeln(row);
  }

  _write(child, {
    'schema_version': _protocolVersion,
    'id': 2,
    'method': 'shutdown',
    'params': {},
  });
  final shutdownReply = await nextReply(2, 'shutdown', _shutdownDeadline);
  if (shutdownReply == null) {
    stderr.writeln('census_client: no shutdown reply; killing child');
    child.kill(ProcessSignal.sigkill);
    exit(2);
  }
  // Mirror the core's graceful teardown (dartq-plugin runtime shutdown):
  // after the shutdown reply, close the child's stdin — the host's run
  // loop exits on that EOF — before awaiting exitCode. The timeout below
  // stays as the kill backstop, as in the core.
  await child.stdin.flush();
  child.stdin.close();
  final code = await child.exitCode.timeout(
    _shutdownDeadline,
    onTimeout: () {
      child.kill(ProcessSignal.sigkill);
      return -1;
    },
  );
  if (code != 0) {
    stderr.writeln(
      'census_client: child exited $code; stderr:\n${stderrTail.join()}',
    );
    exit(2);
  }
}

void _write(Process child, Map<String, Object?> frame) {
  child.stdin.writeln(jsonEncode(frame));
  child.stdin.flush();
}
