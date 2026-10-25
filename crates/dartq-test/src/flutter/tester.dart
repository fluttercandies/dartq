// The tester side of `dartq test --flutter`'s grouped pass: the fixed part
// of each chunk's entry point. dartq inserts the chunk's suite imports at
// the `dartq:suites` line and appends `const int _chunk` and one
// `@pragma('vm:entry-point')` function per hosted suite.
//
// The tester's root engine runs `main`: it connects to the runner's control
// socket and, for every suite number it receives, spawns one engine (a new
// isolate in the chunk's isolate group) at that suite's entry point. Each
// suite engine reproduces the stock tool's bootstrap
// (`generateTestBootstrap` in flutter_tools): the uncaught-error listener,
// the golden file comparator rooted at the suite, `autoUpdateGoldenFiles`,
// the nearest `flutter_test_config.dart`, and a JSON WebSocket to the
// runner. Once the runner closes the suite's channel the engine flushes,
// writes its settle markers and exits; the root engine then answers the
// runner's `ping`, so every line the suite's engine wrote precedes it.

import 'dart:async';
import 'dart:convert';
import 'dart:ffi';
import 'dart:io';
import 'dart:isolate';

import 'package:flutter_test/flutter_test.dart';
import 'package:stack_trace/stack_trace.dart';
import 'package:stream_channel/stream_channel.dart';
import 'package:test_api/backend.dart';

// dartq:suites

@Native<Void Function(Pointer<Uint8>, Pointer<Uint8>)>(symbol: 'Spawn')
external void _spawnEngine(Pointer<Uint8> entrypoint, Pointer<Uint8> route);

@Native<Pointer<Uint8> Function(IntPtr)>(symbol: 'malloc')
external Pointer<Uint8> _malloc(int size);

@Native<Void Function(Pointer<Uint8>)>(symbol: 'free')
external void _free(Pointer<Uint8> pointer);

const String _marker = '\u0001dartq.settled ';

String get _server => 'ws://127.0.0.1:${Platform.environment['SERVER_PORT']}';

/// A NUL-terminated copy of `text` (ASCII) for the engine's `Spawn`,
/// which copies its arguments before returning.
Pointer<Uint8> _cString(String text) {
  final List<int> bytes = ascii.encode(text);
  final Pointer<Uint8> out = _malloc(bytes.length + 1);
  for (int i = 0; i < bytes.length; i++) {
    out[i] = bytes[i];
  }
  out[bytes.length] = 0;
  return out;
}

void _spawn(String entrypoint) {
  final Pointer<Uint8> name = _cString(entrypoint);
  final Pointer<Uint8> route = _cString('/');
  _spawnEngine(name, route);
  _free(name);
  _free(route);
}

/// Capture any top-level errors (mostly lazy syntax errors, since other are
/// caught below) and report them to the parent isolate.
void _catchIsolateErrors() {
  final ReceivePort errorPort = ReceivePort();
  // Treat errors non-fatal because otherwise they'll be double-printed.
  Isolate.current.setErrorsFatal(false);
  Isolate.current.addErrorListener(errorPort.sendPort);
  errorPort.listen((dynamic message) {
    // Masquerade as an IsolateSpawnException because that's what this would
    // be if the error had been detected statically.
    final IsolateSpawnException error = IsolateSpawnException(message[0] as String);
    final Trace stackTrace =
        message[1] == null ? Trace(const <Frame>[]) : Trace.parse(message[1] as String);
    Zone.current.handleUncaughtError(error, stackTrace);
  });
}

/// One suite engine: serve the suite as the stock bootstrap does, then
/// settle.
void _connect(int id, String url, Function Function() getMain) {
  final StreamChannel<dynamic> testChannel = RemoteListener.start(() {
    _catchIsolateErrors();
    goldenFileComparator = LocalFileComparator(Uri.parse(url));
    autoUpdateGoldenFiles = false;
    return getMain();
  });
  WebSocket.connect('$_server/suite/$_chunk/$id').then((WebSocket socket) {
    socket.map((dynamic message) => json.decode(message as String)).listen(
          testChannel.sink.add,
          onDone: () => _settle(id, testChannel),
        );
    socket.addStream(testChannel.stream.map(json.encode));
  });
}

/// The runner closed the suite: every write the suite made precedes the
/// markers on each stream, then the engine ends.
Future<void> _settle(int id, StreamChannel<dynamic> channel) async {
  await channel.sink.close();
  await Future.wait(<Future<void>>[stdout.flush(), stderr.flush()]);
  print('$_marker$id p');
  stdout.writeln('$_marker$id o');
  stderr.writeln('$_marker$id e');
  await Future.wait(<Future<void>>[stdout.flush(), stderr.flush()]);
  Isolate.current.kill();
}

@pragma('vm:entry-point')
Future<void> main() async {
  final WebSocket control = await WebSocket.connect('$_server/control/$pid');
  await for (final dynamic message in control) {
    final String text = message as String;
    if (text.startsWith('ping ')) {
      final String id = text.substring(5);
      stdout.writeln('$_marker$id q');
      stderr.writeln('$_marker$id r');
      await Future.wait(<Future<void>>[stdout.flush(), stderr.flush()]);
    } else {
      _spawn('_s$text');
    }
  }
  exit(0);
}
