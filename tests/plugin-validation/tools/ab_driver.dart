// ab_driver.dart — drives one plugin A-B arm end to end and emits one
// machine-readable metrics JSON object (the #1089 plugin-overhead axis,
// measurement points of the A-B protocol in this fixture's README).
//
// Usage:
//   dart ab_driver.dart --daemon <dartq-bin> --dir <preset-dir>
//                      --sdk <dart-sdk-root> [--rounds <n>] [--timeout <s>]
//                      [--idle-gap <s>] [--dump-diagnostics <file>]
//                      [--pidfile <file>] [--quiet-window-ms <ms>]
//
// With --pidfile, the daemon child's PID is written to <file> immediately
// after spawn: the scripts' RSS sampler and survivor traps consume it, and
// must recover the PID even when the driver itself dies first.
//
// Protocol (dartq-RPC — see crates/dartq-daemon/src/wire.rs and the `run()`
// doc in crates/dartq-daemon/src/server.rs): there is no lifecycle
// handshake — the session is live from frame one. The startup sweep
// publishes the workspace's diagnostic-bearing files (the `dartq/diagnostics`
// push face; the diff-based face never pushes a clean file), so the cold
// segment — options walk + plugin discovery + per-plugin sidecar spawn +
// handshake — is measured to the wave's last publish, detected as a quiet
// window after the first publish. The window is 2 s by default;
// `--quiet-window-ms` widens it (the parity oracle's opt-in, so a slow
// first plugin merge on a cold VM lands inside the wave instead of being
// cut off by the session EOF). Each incremental round writes the target
// file on disk and reports it with the `dartq/sync` notification; the round
// completes when the edited file's push carries a diagnostic anchored in the
// appended region (the publish face is diff-based: a set-neutral edit never
// publishes, so each round appends one broken line to change the file's
// diagnostic set, and the row's line pins the round's own push — stale
// plugin-merge republishes of earlier rounds can never satisfy it). The
// session ends with EOF (stdin close — the protocol's only end signal).
// With --idle-gap <s> (default 0 = off), one extra edit+publish round runs
// after a <s>-second idle gap following the incremental rounds: its latency
// is the last element of rounds_ms and is also emitted as post_gap_round_ms
// — against an idle-evicting daemon it measures sidecar respawn +
// re-analysis cost. A phase timeout emits metrics with "valid": false and
// exits 3; a clean run exits 0.
//
// With --dump-diagnostics, every `dartq/diagnostics` payload is recorded in
// arrival order and written to <file> as one JSON array (even on a timeout
// run — the partial dump is the forensic record). The parity oracle's
// comparator consumes it; without the flag nothing changes.
import 'dart:convert';
import 'dart:io';

const _firstPublishWait = Duration(seconds: 120);
const _quietWindow = Duration(seconds: 2);
const _exitWait = Duration(seconds: 60);

Future<void> main(List<String> args) async {
  var daemon = '';
  var dir = '';
  var sdk = '';
  var rounds = 10;
  var timeoutSeconds = 600;
  var idleGapSeconds = 0;
  var quietWindowMs = 0;
  var dumpPath = '';
  var pidPath = '';
  for (var i = 0; i + 1 < args.length; i += 2) {
    switch (args[i]) {
      case '--daemon':
        daemon = args[i + 1];
      case '--dir':
        dir = args[i + 1];
      case '--sdk':
        sdk = args[i + 1];
      case '--rounds':
        rounds = int.parse(args[i + 1]);
      case '--timeout':
        timeoutSeconds = int.parse(args[i + 1]);
      case '--idle-gap':
        idleGapSeconds = int.parse(args[i + 1]);
      case '--quiet-window-ms':
        quietWindowMs = int.parse(args[i + 1]);
      case '--dump-diagnostics':
        dumpPath = args[i + 1];
      case '--pidfile':
        pidPath = args[i + 1];
      default:
        stderr.writeln('ab_driver: unknown argument ${args[i]}');
        exit(1);
    }
  }
  if (daemon.isEmpty || dir.isEmpty || sdk.isEmpty) {
    stderr.writeln(
      'usage: dart ab_driver.dart --daemon <bin> --dir <preset> --sdk <root>',
    );
    exit(1);
  }

  final root = Directory(dir).absolute.path;
  final files =
      Directory('$root/lib')
          .listSync(recursive: true)
          .whereType<File>()
          .map((f) => f.path)
          .toList()
        ..sort();

  final session = _Session(
    daemon: daemon,
    root: root,
    sdk: sdk,
    deadline: DateTime.now().add(Duration(seconds: timeoutSeconds)),
    dumpPath: dumpPath.isEmpty ? null : dumpPath,
    pidPath: pidPath.isEmpty ? null : pidPath,
    // 0 = the driver's default quiet window; the flag is opt-in.
    quietWindow: quietWindowMs > 0
        ? Duration(milliseconds: quietWindowMs)
        : _quietWindow,
  );
  var exitCode = 0;
  try {
    await session.start();
    final coldWatch = Stopwatch()..start();
    await session.waitForColdWave(coldWatch);
    final coldMs = coldWatch.elapsedMilliseconds;

    // The incremental rounds edit one file in place: a disk write plus a
    // `dartq/sync` report per round, each round complete when the edited
    // file's push carries a diagnostic anchored in the appended region (the
    // round's own row — see the header note). The target is chosen the same
    // way the presets name it; any file works, since the appended broken
    // line forces a non-empty set regardless of the file's prior state.
    final target = files.firstWhere(
      (f) => f.contains('avoid_dynamic.dart'),
      orElse: () => files.first,
    );
    final original = File(target).readAsStringSync();
    // Zero-based line the first appended statement lands on: the count of
    // complete lines in the original, plus one for the separating newline.
    final baseLine = original.split('\n').length - 1;
    final roundMs = <int>[];
    for (var round = 1; round <= rounds; round++) {
      final watch = Stopwatch()..start();
      session.change(target, '$original\nvar round_edit_$round = ;\n');
      await session.waitForPublish(watch, of: target, line: baseLine + round);
      roundMs.add(watch.elapsedMilliseconds);
    }

    // Optional idle-gap probe: idle long enough for an idle-evicting daemon
    // to drop its sidecars, then one edit+publish round whose latency is the
    // respawn + re-analysis cost. Nothing edits during the gap, so nothing
    // publishes (the push face is diff-based); the drain before the round is
    // belt-and-braces against a slow plugin crash-row push.
    int? postGapMs;
    if (idleGapSeconds > 0) {
      stderr.writeln('ab_driver: idle gap — waiting ${idleGapSeconds}s');
      await Future<void>.delayed(Duration(seconds: idleGapSeconds));
      if (DateTime.now().isAfter(session.deadline)) {
        throw _DriverTimeout(
          'driver-wide deadline exceeded during the idle gap',
        );
      }
      await Future<void>.delayed(const Duration(seconds: 2));
      session.drainPublishes();
      final watch = Stopwatch()..start();
      session.change(target, '$original\nvar post_gap_edit = ;\n');
      await session.waitForPublish(watch, of: target, line: baseLine + 1);
      postGapMs = watch.elapsedMilliseconds;
      roundMs.add(postGapMs);
    }

    await session.shutdown();
    session.emit(
      valid: true,
      coldMs: coldMs,
      roundMs: roundMs,
      files: files,
      idleGapSeconds: idleGapSeconds,
      postGapMs: postGapMs,
    );
  } on _DriverTimeout catch (error) {
    session.emit(
      valid: false,
      coldMs: null,
      roundMs: const [],
      files: files,
      note: '$error',
      idleGapSeconds: idleGapSeconds,
    );
    exitCode = 3;
  } finally {
    // The dump lands even on a timeout run (the partial record is the
    // forensics); before kill so the process state is irrelevant anyway.
    session.writeDiagnostics();
    await session.kill();
  }
  exit(exitCode);
}

class _DriverTimeout implements Exception {
  _DriverTimeout(this.message);
  final String message;

  @override
  String toString() => message;
}

/// One daemon session: framed dartq-RPC over the child's stdio plus stderr
/// relay and stats-line capture.
class _Session {
  _Session({
    required this.daemon,
    required this.root,
    required this.sdk,
    required this.deadline,
    required this.quietWindow,
    this.dumpPath,
    this.pidPath,
  });

  final String daemon;
  final String root;
  final String sdk;
  final DateTime deadline;

  /// How long a publish-free stretch ends the cold wave (the quiescence
  /// probe in [waitForColdWave]). Defaults to the 2 s const; the parity
  /// oracle widens it via --quiet-window-ms so a slow first plugin merge on
  /// a cold VM lands inside the recorded dump instead of being cut off by
  /// the session EOF.
  final Duration quietWindow;

  /// When set, every `dartq/diagnostics` payload is recorded here in arrival
  /// order and written to the path by [writeDiagnostics].
  final String? dumpPath;

  /// When set, the daemon child's PID is written here immediately after
  /// spawn, so the scripts' sampler and survivor traps can recover it even
  /// from a driver death.
  final String? pidPath;

  final _statsLines = <String>[];
  final _pending = <int>[];
  final _publishes = <Map<String, Object?>>[];
  final _diagnostics = <Map<String, Object?>>[];
  Process? _process;

  /// Set when the child has exited (or is being killed): further outbound
  /// writes are dropped instead of throwing on the dead stdin sink.
  var _closed = false;
  var _closedNoted = false;

  /// Writes the recorded `dartq/diagnostics` payloads as one JSON array.
  void writeDiagnostics() {
    final path = dumpPath;
    if (path == null) return;
    File(path).writeAsStringSync('${jsonEncode(_diagnostics)}\n');
  }

  void emit({
    required bool valid,
    required int? coldMs,
    required List<int> roundMs,
    required List<String> files,
    String? note,
    int idleGapSeconds = 0,
    int? postGapMs,
  }) {
    final sorted = [...roundMs]..sort();
    int? percentile(double p) {
      if (sorted.isEmpty) return null;
      return sorted[(p * (sorted.length - 1)).round()];
    }

    stdout.writeln(
      jsonEncode({
        'valid': valid,
        'dir': root,
        'opened_files': files.length,
        'files': files,
        'cold_full_ms': coldMs,
        'rounds_ms': roundMs,
        'round_p50_ms': percentile(0.50),
        'round_p95_ms': percentile(0.95),
        if (idleGapSeconds > 0) ...{
          'idle_gap_seconds': idleGapSeconds,
          if (postGapMs != null) 'post_gap_round_ms': postGapMs,
        },
        'plugin_stats': _statsLines,
        if (note != null) 'note': note,
      }),
    );
  }

  Future<void> start() async {
    // Inherit the driver's environment and prepend the pinned SDK to PATH:
    // the daemon spawns each plugin as plain `dart` from PATH (the launch
    // contract), so children and oracle must share one SDK.
    final env = {...Platform.environment};
    env['PATH'] = '$sdk/bin:${Platform.environment['PATH'] ?? ''}';
    _process = await Process.start(daemon, [
      'daemon',
      root,
      '--sdk',
      sdk,
    ], environment: env);
    final pidPath = this.pidPath;
    if (pidPath != null) {
      File(pidPath).writeAsStringSync('${_process!.pid}\n');
    }
    _process!.stderr
        .transform(utf8.decoder)
        .transform(const LineSplitter())
        .listen((line) {
          if (line.contains('plugin stats')) _statsLines.add(line);
          stderr.writeln('[daemon] $line');
        });
    _process!.stdout.listen(_onChunk, onError: (Object _) {});
    _process!.exitCode.then((code) {
      _closed = true;
      // The exit code is the primary death evidence — a negative code
      // encodes the killing signal — so print it the moment it exists,
      // including a mid-run death, where this line is the
      // SIGTERM-vs-crash record the log otherwise lacks.
      stderr.writeln('[daemon] exit=$code');
    });
  }

  /// Resolves when the startup sweep's first publish wave has drained: the
  /// push face is diff-based (a clean file never publishes), so the wave's
  /// end is quiescence, not a per-file tally — the first publish starts the
  /// wave and a quiet window after the last one ends it (plugin merge waves
  /// ride later cycles and count when they land inside the window).
  Future<void> waitForColdWave(Stopwatch watch) async {
    final first = await _nextPublishWithin(_firstPublishWait);
    if (first == null) {
      throw _DriverTimeout('no diagnostics publish before the deadline');
    }
    while (true) {
      if (DateTime.now().isAfter(deadline)) {
        throw _DriverTimeout(
          'driver-wide deadline exceeded '
          '(${watch.elapsedMilliseconds} ms into the cold wave)',
        );
      }
      if (await _nextPublishWithin(quietWindow) == null) return;
    }
  }

  /// Writes [text] to [file] on disk and reports it with `dartq/sync` — the
  /// protocol's change-intake face (the session syncs the named paths from
  /// disk at its next drain; absolute paths pass through unchanged).
  void change(String file, String text) {
    File(file).writeAsStringSync(text);
    _notify('dartq/sync', {
      'paths': [file],
    });
  }

  /// Resolves when a `dartq/diagnostics` push for [of] has arrived that
  /// carries a diagnostic anchored within one line of [line] (zero-based) —
  /// the row the round's own edit created. The line pin is what makes the
  /// wait stale-proof without a version echo: leftover plugin-merge
  /// republishes of earlier rounds only carry rows above the appended
  /// region, and later rounds cannot publish before this round's wait
  /// resolves. The driver-wide deadline rides every queue pop.
  Future<void> waitForPublish(Stopwatch watch, {String? of, int? line}) async {
    while (true) {
      final publish = await _nextPublish();
      final uriOk = of == null || publish['uri'] == Uri.file(of).toString();
      var rowOk = line == null;
      if (!rowOk && uriOk) {
        final rows = (publish['diagnostics'] as List<Object?>?) ?? const [];
        for (final row in rows) {
          final start =
              ((row as Map<String, Object?>)['range']
                      as Map<String, Object?>?)?['start']
                  as Map<String, Object?>?;
          final rowLine = start?['line'];
          if (rowLine is int && rowLine >= line - 1 && rowLine <= line + 1) {
            rowOk = true;
            break;
          }
        }
      }
      if (uriOk && rowOk) return;
      if (DateTime.now().isAfter(deadline)) {
        throw _DriverTimeout(
          'driver-wide deadline exceeded '
          '(${watch.elapsedMilliseconds} ms into this phase)',
        );
      }
    }
  }

  /// Drops queued `dartq/diagnostics` pushes. Used before the post-idle-gap
  /// round: a push that arrived during the gap (a slow plugin crash row)
  /// must not satisfy the post-gap wait.
  void drainPublishes() {
    _publishes.clear();
  }

  /// Pops the next queued push, waiting (deadline-bounded) until one lands.
  Future<Map<String, Object?>> _nextPublish() async {
    while (_publishes.isEmpty) {
      if (DateTime.now().isAfter(deadline)) {
        throw _DriverTimeout('no diagnostics push before the deadline');
      }
      await Future<void>.delayed(const Duration(milliseconds: 10));
    }
    return _publishes.removeAt(0);
  }

  /// Pops the next queued push or returns `null` once [limit] passes with
  /// nothing arriving (the quiescence probe behind the cold-wave wait).
  Future<Map<String, Object?>?> _nextPublishWithin(Duration limit) async {
    final until = DateTime.now().add(limit);
    while (_publishes.isEmpty) {
      if (DateTime.now().isAfter(deadline)) {
        return null;
      }
      if (DateTime.now().isAfter(until)) return null;
      await Future<void>.delayed(const Duration(milliseconds: 10));
    }
    return _publishes.removeAt(0);
  }

  /// Ends the session the protocol's way: EOF on stdin is the daemon's only
  /// end signal (there is no shutdown method); the clean end is exit code 0.
  Future<void> shutdown() async {
    _process?.stdin.close();
    final code = await _process!.exitCode.timeout(
      _exitWait,
      onTimeout: () {
        _process!.kill();
        return -1;
      },
    );
    if (code != 0) {
      stderr.writeln('ab_driver: daemon exited $code after EOF');
    }
  }

  Future<void> kill() async {
    _closed = true;
    _process?.kill();
  }

  void _onChunk(List<int> chunk) {
    _pending.addAll(chunk);
    while (true) {
      var headerEnd = -1;
      for (var i = 0; i + 3 < _pending.length; i++) {
        if (_pending[i] == 13 &&
            _pending[i + 1] == 10 &&
            _pending[i + 2] == 13 &&
            _pending[i + 3] == 10) {
          headerEnd = i;
          break;
        }
      }
      if (headerEnd < 0) return;
      final header = utf8.decode(_pending.sublist(0, headerEnd));
      final match = RegExp(
        r'Content-Length:\s*(\d+)',
        caseSensitive: false,
      ).firstMatch(header);
      if (match == null) {
        stderr.writeln('ab_driver: malformed frame header: $header');
        return;
      }
      final length = int.parse(match.group(1)!);
      final bodyStart = headerEnd + 4;
      if (_pending.length < bodyStart + length) return;
      final body = utf8.decode(_pending.sublist(bodyStart, bodyStart + length));
      _pending.removeRange(0, bodyStart + length);
      _dispatch(jsonDecode(body) as Map<String, Object?>);
    }
  }

  void _dispatch(Map<String, Object?> message) {
    if (message['method'] == 'dartq/diagnostics') {
      final publish =
          (message['params'] ?? <String, Object?>{}) as Map<String, Object?>;
      _publishes.add(publish);
      if (dumpPath != null) _diagnostics.add(publish);
    }
  }

  void _notify(String method, Object? params) {
    _send({'jsonrpc': '2.0', 'method': method, 'params': params});
  }

  /// Single outbound choke point. A degraded child (e.g. a plugin crash
  /// taking the daemon down) closes the stdin sink; writing to a dead sink
  /// throws and would mask the measurement timeout paths. Drop the write
  /// with one stderr note per session instead — the waitForPublish*/cold
  /// deadlines then report the dead child as an invalid run.
  void _send(Map<String, Object?> message) {
    final process = _process;
    if (process == null) return;
    if (_closed) {
      _noteDropped(message);
      return;
    }
    final body = utf8.encode(jsonEncode(message));
    final head = utf8.encode('Content-Length: ${body.length}\r\n\r\n');
    try {
      process.stdin.add(head);
      process.stdin.add(body);
    } on StateError {
      _closed = true;
      _noteDropped(message);
      return;
    }
    // A broken pipe can also surface asynchronously on the flush future;
    // treat it as the same dead-child condition, printing the error itself
    // (it is death evidence) before the one-note drop record.
    process.stdin.flush().catchError((Object e) {
      stderr.writeln('[daemon] flush error: $e');
      _closed = true;
      _noteDropped(message);
    });
  }

  void _noteDropped(Map<String, Object?> message) {
    if (_closedNoted) return;
    _closedNoted = true;
    stderr.writeln(
      'ab_driver: child gone; dropping outbound ${message['method']}',
    );
  }
}
