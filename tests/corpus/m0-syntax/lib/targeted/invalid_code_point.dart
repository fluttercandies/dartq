// Targets invalid_code_point: a '\u{...}' escape whose code point is above
// 0x10FFFF is not a valid Unicode code point.
var s = '\u{110000}';
