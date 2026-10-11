// override family — fixture wave 2 kitchen file. One hierarchy per rule,
// same-file ancestor chains (the dartq ported face is the written
// extends/with/on chain; cross-file ancestors under-report on dartq and
// stay out of shape). The @override annotation is present wherever the
// rule under test is not annotate_overrides, so exactly one rule fires
// per hierarchy.
// Expected rows: 4, all keyed override_family.dart — ANNOTATE_OVERRIDES
// (OverrideGuest.run, no annotation), AVOID_RENAMING_METHOD_PARAMETERS
// (RenameGuest.track positional `value` -> `other`; the lib-directory
// gate holds — every fixture lives under lib/), UNNECESSARY_OVERRIDES
// (ForwardGuest.go, the exact super forward), OVERRIDDEN_FIELDS
// (FieldGuest.level over the variable-backed getter).
class OverrideBase {
  void run() {
    print(0);
  }
}

class OverrideGuest extends OverrideBase {
  void run() {
    print(1);
  }
}

class RenameBase {
  void track(int value) {
    print(value);
  }
}

class RenameGuest extends RenameBase {
  @override
  void track(int other) {
    print(other);
  }
}

class ForwardBase {
  void go(int step) {
    print(step);
  }
}

class ForwardGuest extends ForwardBase {
  @override
  void go(int step) {
    super.go(step);
  }
}

class FieldBase {
  int level = 0;
}

class FieldGuest extends FieldBase {
  @override
  int level = 1;
}
