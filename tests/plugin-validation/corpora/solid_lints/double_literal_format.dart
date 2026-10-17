// Trigger for solid_lints `double_literal_format`.
// Shape from the upstream suite
// (vendor/solid_lints-2.0.0/test/src/lints/double_literal_format/double_literal_format_rule_test.dart):
// double literals with a leading decimal point, leading zeros and
// redundant trailing zeros — the rule's flagged forms.
var leading = .23;

var leadingZeros = 05.23;

var trailing = 5.230;
