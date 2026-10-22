// Fixture for the ignored live debug test: stops at the breakpoint on
// line 5 with `counter == 0`, then prints the incremented value.

Future<void> main() async {
  var counter = 0;
  counter += 1;
  await Future<void>.delayed(const Duration(milliseconds: 50));
  print('counter=$counter');
}
