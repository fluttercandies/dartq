// messages-example: analyzer/messages.yaml WarningCode.nonConstArgumentForConstParameter
import 'package:meta/meta.dart' show mustBeConst;

int f(int value) => g(value);

int g(@mustBeConst int value) => value + 1;
