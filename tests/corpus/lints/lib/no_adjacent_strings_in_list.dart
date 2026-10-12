// no_adjacent_strings_in_list — official rule faces
// (pkg/linter no_adjacent_strings_in_list.dart:38-82): an adjacent-strings
// group inside a list literal, a set literal (never a map), a for-element
// body, an if-element's then arm when no else follows or its else arm, or
// a direct element of a switch case's top-level list pattern reports at
// the whole group.
// Expected rows: 6 — NO_ADJACENT_STRINGS_IN_LIST keyed
// no_adjacent_strings_in_list.dart, message
// "Don't use adjacent strings in a list literal.":
//   `'l one' 'l two'` -> list element group
//   `'s one' 's two'` -> set element group
//   `'f one' 'f two'` -> for-element body group
//   `'i one' 'i two'` -> if-element then arm without an else
//   `'e one' 'e two'` -> if-element else arm
//   `'p one' 'p two'` -> list-pattern element group
var x = 0;

var a = ['l one' 'l two'];
var b = <String>{'s one' 's two'};
var c = [for (var i in [1]) 'f one' 'f two'];
var d = [if (x == 0) 'i one' 'i two'];
var e = [if (x == 0) 't' 'u' else 'e one' 'e two'];

// GOOD: a map entry, comma-separated elements, a then arm with an else
// following and a guard-free constant case stay silent; the switch pins
// the list-pattern face.
var g1 = <String, String>{'k': 'v one' 'v two'};
var g2 = ['plain', 'separate', 'elements'];
var g3 = [if (x == 0) 'has' 'else' else 'arm'];

void h(Object v) {
  switch (v) {
    case ['p one' 'p two']:
      break;
  }
}
