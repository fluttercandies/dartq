// messages-example: linter/messages.yaml LinterLintCode.unnecessaryUnawaited
import 'dart:async';

import 'package:meta/meta.dart';

@awaitNotRequired
Future<bool> log(String message) async => true;

void f() {
  unawaited(log('Message.'));
}
