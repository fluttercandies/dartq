// Valid Dart 3.15 acceptance corpus — async/await/generators (async functions, await in bodies and expressions, async* generators with yield/yield*, sync* generators, await-for loops, async closures, Future/Stream types).
//
// Provenance: shapes modeled on the official language tests
// upstream Dart SDK source, tests/language/async/, tests/language/async_star/, tests/language/sync_star/ (SDK main @ e6f4207a). Oracle-verified
// 2026-10-12: zero rows under fvm dart 3.13.1 `dart analyze --format=machine`
// in a probe package (`environment: sdk: ^3.13.0`). Covered by
// scripts/acceptance-gate.sh (zero-diagnostics bar, `dartq check` machine face).
Future<int> fetchValue(int seed) async {
  return seed * 2;
}

Future<void> orchestrate() async {
  final a = await fetchValue(1);
  final b = await fetchValue(a);
  if (a + b == 0) {}
}

Future<int> awaitInExpression() async => (await fetchValue(3)) + 1;

Stream<int> counter(int to) async* {
  for (var i = 0; i <= to; i++) {
    yield i;
  }
}

Stream<int> relayed(Stream<int> source) async* {
  await for (final v in source) {
    yield v;
  }
  yield* counter(2);
}

Iterable<int> naturals(int max) sync* {
  var n = 0;
  while (n < max) {
    yield n++;
  }
}

Future<int> sumStream(Stream<int> source) async {
  var total = 0;
  await for (final v in source) {
    total += v;
  }
  return total;
}

void exerciseClosures() {
  final cb = () async => 1;
  final withAwait = () async {
    return await fetchValue(9);
  };
  cb();
  withAwait();
}

void main() {
  final job = orchestrate();
  final expression = awaitInExpression();
  final stream = counter(3);
  final relay = relayed(counter(1));
  final sync = naturals(3);
  final pending = sumStream(stream);
  exerciseClosures();
  final futures = <Future<Object?>>[job, expression, pending];
  final streams = <Stream<int>>[stream, relay];
  if (futures.length + streams.length + sync.length != 8) {
    throw StateError('unreachable');
  }
}
