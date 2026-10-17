// messages-example: linter/messages.yaml LinterLintCode.invalidRuntimeCheckWithJsInteropTypesDartAsJs (variant 3)
import 'dart:js_interop';

bool f(JSAny a) => a is String;
