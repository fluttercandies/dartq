// parity_client.dart — headless driver for the OFFICIAL analysis server's
// plugin face: boots the pinned-SDK analysis_server snapshot over stdio
// (classic analyzer protocol, one JSON object per line), lets the corpus's
// analysis_options.yaml activate its declared plugins (the server builds
// its synthetic plugin package under ANALYZER_STATE_LOCATION_OVERRIDE),
// collects the merged server+plugin `analysis.errors` notifications, and
// writes every notification event to the output file as one JSON object
// per line (the comparator's input).
//
// Usage: dart parity_client.dart <corpus-dir> <output-file>
//
// Environment:
// - DARTQ_DART_SDK — pinned Dart SDK root; provides the server snapshot
//   `bin/snapshots/analysis_server.dart.snapshot`.
// - ANALYZER_STATE_LOCATION_OVERRIDE — per-run state dir; mandatory, the
//   default `$HOME/.dart-tool` state folder races between runs.
//
// Lifecycle mirrors the upstream in-process dev_server template (upstream
// Dart SDK source, pkg/analysis_server, lib/src/server/dev_server.dart):
// server.setSubscriptions ["STATUS"] -> analysis.setAnalysisRoots ->
// collect analysis.errors until server.status reports
// analysis:{isAnalyzing:false} after an analyzing phase (the plugin-aware
// idle — the only sound terminator; analysis.getErrors is engine-only and
// never carries plugin diagnostics, so notifications are the only plugin
// face) -> a short quiescence window against trailing notifications ->
// server.shutdown.
//
// Exit codes: 0 ok, 1 usage, 2 protocol failure (startup timeout,
// non-JSON line, fatal server error, unclean server exit).
import 'dart:async';
import 'dart:convert';
import 'dart:io';

const _connectDeadline = Duration(seconds: 60);
const _requestDeadline = Duration(seconds: 30);
const _analysisBudget = Duration(seconds: 600);
const _quiescence = Duration(seconds: 2);
const _shutdownDeadline = Duration(seconds: 15);

Future<void> main(List<String> args) async {
  if (args.length != 2) {
    stderr.writeln('usage: dart parity_client.dart <corpus-dir> <output-file>');
    exit(1);
  }
  final corpus = Directory(args[0]).absolute.path;
  if (!Directory(corpus).existsSync()) {
    stderr.writeln('parity_client: no such corpus dir: ${args[0]}');
    exit(1);
  }
  final outputPath = args[1];

  final sdk = Platform.environment['DARTQ_DART_SDK'];
  if (sdk == null || sdk.isEmpty) {
    stderr.writeln(
      'parity_client: DARTQ_DART_SDK is required (pinned Dart SDK root)',
    );
    exit(2);
  }
  final snapshot = '$sdk/bin/snapshots/analysis_server.dart.snapshot';
  if (!File(snapshot).existsSync()) {
    stderr.writeln('parity_client: no server snapshot: $snapshot');
    exit(2);
  }
  final stateOverride = Platform.environment['ANALYZER_STATE_LOCATION_OVERRIDE'];
  if (stateOverride == null || stateOverride.isEmpty) {
    stderr.writeln(
      'parity_client: ANALYZER_STATE_LOCATION_OVERRIDE is required '
      '(per-run state dir; the default state folder races between runs)',
    );
    exit(2);
  }

  final child = await Process.start('$sdk/bin/dart', [
    snapshot,
    '--protocol=analyzer',
    '--client-id=dartq-parity-oracle',
    '--suppress-analytics',
  ]);
  final stderrTail = <String>[];
  child.stderr.transform(utf8.decoder).listen(stderrTail.add, onDone: () {});

  // Every notification event recorded for the comparator; flushed at each
  // exit path so a failed run still leaves its forensics on disk.
  final events = <Map<String, Object?>>[];

  void flushEvents() {
    File(outputPath).writeAsStringSync(
      '${events.map(jsonEncode).join('\n')}${events.isEmpty ? '' : '\n'}',
    );
  }

  // Broadcast: the startup, idle, and shutdown waits subscribe
  // sequentially, and `firstWhere`-style consumption cancels earlier
  // subscriptions (census_client's reasoning); notifications and replies
  // share one line-delimited JSON stream.
  final frames = StreamController<Map<String, Object?>>.broadcast();

  Never die(String message) {
    child.kill(ProcessSignal.sigkill);
    flushEvents();
    stderr.writeln(message);
    if (stderrTail.isNotEmpty) {
      stderr.writeln('parity_client: server stderr:\n${stderrTail.join()}');
    }
    exit(2);
  }

  // A non-JSON stdout line breaks the one-JSON-object-per-line wire
  // contract and is unrecoverable (census_client's badStdoutLine shape).
  Never badStdoutLine(String line, Object error) {
    die('parity_client: non-JSON stdout line from server: $line ($error)');
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
          frames.add(frame);
        },
        onDone: frames.close,
      );

  final replies = <String, Completer<Map<String, Object?>>>{};
  var nextId = 0;

  // Sends one request and resolves to its reply (null on timeout).
  Future<Map<String, Object?>?> request(
    String method,
    Map<String, Object?> params,
    Duration budget,
  ) {
    final id = '${nextId++}';
    final completer = Completer<Map<String, Object?>>();
    replies[id] = completer;
    child.stdin.writeln(
      jsonEncode({'id': id, 'method': method, 'params': params}),
    );
    child.stdin.flush();
    return completer.future.timeout(
      budget,
      onTimeout: () => const <String, Object?>{},
    );
  }

  final connected = Completer<void>();
  final idle = Completer<void>();
  var beganAnalysis = false;
  var fatalServerError = false;

  final notifications = frames.stream.listen((frame) {
    final event = frame['event'];
    if (event == null) return;
    events.add(frame);
    final params = frame['params'];
    switch (event as String) {
      case 'server.connected':
        if (!connected.isCompleted) connected.complete();
      case 'server.status':
        // Mirror dev_server's guard: only a false after a true phase is
        // the plugin-aware idle (legacy_analysis_server folds plugin
        // analysis and plugin-manager init into isAnalyzing).
        final analysis = params is Map<String, Object?>
            ? params['analysis']
            : null;
        if (analysis is! Map<String, Object?>) return;
        if (analysis['isAnalyzing'] == true) {
          beganAnalysis = true;
        } else if (analysis['isAnalyzing'] == false &&
            beganAnalysis &&
            !idle.isCompleted) {
          idle.complete();
        }
      case 'server.error':
        if (params is Map<String, Object?> && params['isFatal'] == true) {
          fatalServerError = true;
          stderr.writeln('parity_client: fatal server error: $params');
        }
    }
  });

  // Replies route by id; the awaited request flows below consume them.
  frames.stream.listen((frame) {
    final id = frame['id'];
    if (id is String) replies.remove(id)?.complete(frame);
  });

  // Startup gate: the server sends server.connected unconditionally at
  // boot, before any request is served.
  final connectedInTime = await connected.future
      .then((_) => true)
      .timeout(_connectDeadline, onTimeout: () => false);
  if (!connectedInTime) {
    die(
      'parity_client: no server.connected within '
      '${_connectDeadline.inSeconds}s; server stderr:\n${stderrTail.join()}',
    );
  }

  final subscribed = await request('server.setSubscriptions', {
    'subscriptions': ['STATUS'],
  }, _requestDeadline);
  if (subscribed == null) {
    die('parity_client: no server.setSubscriptions reply');
  }
  if (subscribed['error'] != null) {
    die('parity_client: server.setSubscriptions error: ${subscribed['error']}');
  }

  final roots = await request('analysis.setAnalysisRoots', {
    'included': [corpus],
    'excluded': const <String>[],
  }, _requestDeadline);
  if (roots == null) {
    die('parity_client: no analysis.setAnalysisRoots reply');
  }
  if (roots['error'] != null) {
    die('parity_client: analysis.setAnalysisRoots error: ${roots['error']}');
  }

  final reachedIdle = await idle.future
      .then((_) => true)
      .timeout(_analysisBudget, onTimeout: () => false);
  if (!reachedIdle) {
    die(
      'parity_client: analysis did not go idle within '
      '${_analysisBudget.inSeconds}s',
    );
  }

  // Quiescence: trailing plugin notifications can land after the first
  // idle status; keep recording through a short hold.
  await Future<void>.delayed(_quiescence);

  final shutdown = await request(
    'server.shutdown',
    const <String, Object?>{},
    _shutdownDeadline,
  );
  if (shutdown == null) {
    stderr.writeln('parity_client: no server.shutdown reply; killing server');
  }
  await child.stdin.flush();
  child.stdin.close();
  final code = await child.exitCode.timeout(
    _shutdownDeadline,
    onTimeout: () {
      child.kill(ProcessSignal.sigkill);
      return -1;
    },
  );
  await notifications.cancel();
  flushEvents();
  if (code != 0) {
    stderr.writeln(
      'parity_client: server exited $code; stderr:\n${stderrTail.join()}',
    );
    exit(2);
  }
  if (fatalServerError) exit(2);

  var diagnosticCount = 0;
  final files = <String>{};
  for (final event in events) {
    if (event['event'] != 'analysis.errors') continue;
    final params = event['params'] as Map<String, Object?>;
    files.add(params['file'] as String);
    diagnosticCount += (params['errors'] as List<Object?>).length;
  }
  stdout.writeln(
    'parity_client: $diagnosticCount diagnostics over ${files.length} files '
    '-> $outputPath',
  );
}
