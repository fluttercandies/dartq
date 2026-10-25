// The green suite of the test-face parity fixture: a group, an async
// test and a skipped test, so the parity row set covers every leaf
// outcome `dart test` reports (scripts/test-face-parity.sh).
import 'package:test/test.dart';

void main() {
  group('arithmetic', () {
    test('adds', () => expect(1 + 1, 2));

    test('awaits', () async {
      await Future<void>.delayed(const Duration(milliseconds: 5));
      expect(await Future<int>.value(7), 7);
    });
  });

  test('skips', () {}, skip: 'the fixture covers the skipped outcome');
}
