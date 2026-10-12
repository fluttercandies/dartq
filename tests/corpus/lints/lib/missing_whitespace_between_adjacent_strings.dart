// missing_whitespace_between_adjacent_strings — official rule faces
// (pkg/linter missing_whitespace_between_adjacent_strings.dart:40-69):
// for each adjacent pair, a left part that carries whitespace but neither
// ends with whitespace nor is followed by a right part starting with
// whitespace reports at the left part. `RegExp(…)`/`matches(…)` arguments
// are exempt.
// Expected rows: 3 — MISSING_WHITESPACE_BETWEEN_ADJACENT_STRINGS keyed
// missing_whitespace_between_adjacent_strings.dart, message
// "Missing whitespace between adjacent strings.":
//   `'a long line'` -> the pair seam carries no whitespace
//   `'pair one'` -> the first pair of the three-part run
//   `'pair two'` -> the second pair of the three-part run
var y = 0;

var a = 'a long line' 'continues';
var b = 'pair one' 'pair two' 'pair three';

// GOOD: whitespace at the seam, a whitespace-free left part, an
// interpolation ending in whitespace, and the RegExp/matches exemptions
// stay silent.
var g1 = 'one ' 'two';
var g2 = 'ab' 'cd';
var g3 = 'one' ' two';
var g4 = 'x $y ' 'tail';
var g5 = new RegExp('a b' 'cd');
var g6 = matches('a b' 'cd');
