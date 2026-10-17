// messages-example: analyzer/messages.yaml WarningCode.invalidVisibilityAnnotation
import 'package:meta/meta.dart';

@visibleForTesting
void _someFunction() {}

void f() => _someFunction();
