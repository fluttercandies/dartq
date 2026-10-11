// constructor family — fixture wave 2 kitchen file. Each constructor
// face on its own class; bodies are non-empty (or `;`-shaped) exactly
// where empty_constructor_bodies must stay silent so one rule fires per
// face.
// Expected rows: 5, all keyed constructor_family.dart —
// UNNECESSARY_CONSTRUCTOR_NAME (NamedGate.new() instance creation; the
// `.new` member invocation over a bare receiver), EMPTY_CONSTRUCTOR_BODIES
// (BareStart's empty block, no preceding comments), USE_SUPER_PARAMETERS
// (SuperGuest's positional bare-identifier super arguments, unreferenced
// in its body), TYPE_INIT_FORMALS (Tagged's typed `String this.label`),
// PREFER_INITIALIZING_FORMALS (Point's `: x = x` initializer).
class NamedGate {}

void gateUser() {
  NamedGate.new();
}

class BareStart {
  BareStart() {}
}

class SuperBase {
  SuperBase(int first, int second);
}

class SuperGuest extends SuperBase {
  SuperGuest(int first, int second) : super(first, second);
}

class Tagged {
  String label;
  Tagged(String this.label);
}

class Point {
  int? x;
  Point(int? x) : x = x;
}
