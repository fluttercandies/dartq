// messages-example: linter/messages.yaml LinterLintCode.invalidRuntimeCheckWithJsInteropTypesDartAsJs
import 'dart:js_interop';

bool f(Object b) => b is JSBoolean;
