// messages-example: analyzer/messages.yaml WarningCode.assignmentOfDoNotStore
import 'package:meta/meta.dart';

@doNotStore
int f() => 1;

var x = f();
