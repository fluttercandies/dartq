// The red suite of the Flutter test-face parity fixture: one green test
// and one failing one, so parity covers failure rows and the failure
// exit code.
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  testWidgets('passes', (tester) async {
    await tester.pumpWidget(const MaterialApp(
      home: Scaffold(body: Text('ok')),
    ));
    expect(find.text('ok'), findsOneWidget);
  });

  testWidgets('fails', (tester) async {
    await tester.pumpWidget(const MaterialApp(
      home: Scaffold(body: Text('ok')),
    ));
    expect(find.text('missing'), findsOneWidget);
  });
}
