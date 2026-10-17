// messages-example: analyzer/messages.yaml WarningCode.returnOfDoNotStore
import 'package:meta/meta.dart';

@doNotStore
int f() => 0;

int g() => f();
