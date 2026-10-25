// The green suite of the Flutter test-face parity fixture: a rendering
// test, a stateful interaction test and a skipped one, so the parity row
// set covers every leaf outcome `flutter test` reports
// (scripts/test-face-parity.sh).
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  testWidgets('renders text', (tester) async {
    await tester.pumpWidget(const MaterialApp(
      home: Scaffold(body: Center(child: Text('parity'))),
    ));
    expect(find.text('parity'), findsOneWidget);
  });

  testWidgets('increments on tap', (tester) async {
    await tester.pumpWidget(const MaterialApp(home: _Counter()));
    await tester.tap(find.byIcon(Icons.add));
    await tester.pump();
    expect(find.text('1'), findsOneWidget);
  });

  testWidgets('skips', (tester) async {}, skip: true);
}

class _Counter extends StatefulWidget {
  const _Counter();

  @override
  State<_Counter> createState() => _CounterState();
}

class _CounterState extends State<_Counter> {
  var _count = 0;

  @override
  Widget build(BuildContext context) => Scaffold(
        body: Center(child: Text('$_count')),
        floatingActionButton: FloatingActionButton(
          onPressed: () => setState(() => _count++),
          child: const Icon(Icons.add),
        ),
      );
}
