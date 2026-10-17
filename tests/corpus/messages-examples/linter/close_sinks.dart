// messages-example: linter/messages.yaml LinterLintCode.closeSinks
import 'dart:io';

void g(File f) {
  var sink = f.openWrite();
  sink.write('x');
}
