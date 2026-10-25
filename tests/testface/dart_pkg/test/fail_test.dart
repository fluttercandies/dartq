// The red suite of the test-face parity fixture: one green test and one
// failing one, so parity covers failure rows and the failure exit code.
import 'package:test/test.dart';

void main() {
  test('passes', () => expect('a', 'a'));

  test('fails', () => expect(1, equals(2)));
}
