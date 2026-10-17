// messages-example: linter/messages.yaml LinterLintCode.cancelSubscriptions
import 'dart:async';

void f(Stream stream) {
  // ignore: unused_local_variable
  var subscription = stream.listen((_) {});
}
