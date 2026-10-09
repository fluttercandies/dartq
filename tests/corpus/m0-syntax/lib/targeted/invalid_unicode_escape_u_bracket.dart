// Targets invalid_unicode_escape_u_bracket: an escape sequence starting with
// '\u{' must be followed by 1 to 6 hexadecimal digits followed by a '}'.
var s = '\u{ZZ}';
