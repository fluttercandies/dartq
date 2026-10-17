// messages-example: analyzer/messages.yaml CompileTimeErrorCode.prefixCollidesWithTopLevelMember
import 'dart:math' as f;

int f() => f.min(0, 1);
