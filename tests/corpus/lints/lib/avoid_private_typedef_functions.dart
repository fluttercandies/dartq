// avoid_private_typedef_functions — official rule faces
// (pkg/linter avoid_private_typedef_functions.dart): a private typedef
// used at most once — old `typedef int _Td();` and new
// `typedef _Td = int Function();` forms; generic-parameter and
// named/record aliased types skip. Same-file use-count face: uses in
// sibling part files over-report (the documented cross-file family).
// Expected rows: 2 — AVOID_PRIVATE_TYPEDEF_FUNCTIONS keyed
// avoid_private_typedef_functions.dart, message
// "The typedef is unnecessary because it is only used in one place.":
//   `_Td` -> new-form, used once
//   `_Old` -> old-form, used zero times
typedef _Td = int Function();
late _Td td;
typedef int _Old();

// GOOD: public names never report; a private typedef used twice is fine;
// a private alias of a named type is not a function typedef.
typedef Td = int Function();
typedef _Used = void Function();
_Used used1;
_Used used2;
typedef _Named = List<String>;
typedef _Rec = (int, int);
