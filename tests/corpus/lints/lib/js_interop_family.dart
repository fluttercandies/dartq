// invalid_runtime_check_with_js_interop_types — the dart-is/as-js slice.
// The oracle's interop classification accepts dart:js_interop extension
// types, @staticInterop classes, and SDK web libraries; a plain @JS()
// class without @staticInterop classifies as a plain Dart type on the
// oracle and would be a dartq-only row, so the fixture uses the
// @JS() + @staticInterop shape both sides classify as a JS interop type
// (official test shapes: staticInteropIs_unrelated_type_is_static_
// interop_type / staticInteropAs_*). The left expression is `this` (the
// written-type face dartq ports); unrelated-to-A `is`/`as` report the
// dartIsJs/dartAsJs variants — one machine code.
// Expected rows: 2 — INVALID_RUNTIME_CHECK_WITH_JS_INTEROP_TYPES keyed
// js_interop_family.dart (`this is InteropSite`; `this as InteropSite`).
import 'dart:js_interop';

@JS()
@staticInterop
class InteropSite {}

class InteropGuest {
  bool checkIs() => this is InteropSite;

  Object castAs() => this as InteropSite;
}
