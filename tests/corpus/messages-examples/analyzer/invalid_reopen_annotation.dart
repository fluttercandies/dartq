// messages-example: analyzer/messages.yaml WarningCode.invalidReopenAnnotation
import 'package:meta/meta.dart';

sealed class A {}

@reopen
class B extends A {}
