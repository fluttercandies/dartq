// Targets invalid_unicode_escape_u_started: an escape sequence starting with
// '\u' must be followed by 4 hexadecimal digits or from 1 to 6 digits between
// '{' and '}'.
var s = 'ab\u';
