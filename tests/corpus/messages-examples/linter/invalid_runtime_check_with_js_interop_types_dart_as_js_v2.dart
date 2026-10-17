// messages-example: linter/messages.yaml LinterLintCode.invalidRuntimeCheckWithJsInteropTypesDartAsJs (variant 2)
import 'dart:js_interop';

bool f(List<Object> l) => l is List<JSString>;
