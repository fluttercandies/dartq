// class-members family — fixture wave 2 kitchen file. No imports on
// purpose: unnecessary_this bails when any unprefixed import can provide
// the name (the conservative dartq face), and the file must stay on the
// both-sides-agree path. void_checks lives in its own file because its
// dart:async import would silence unnecessary_this on dartq only.
// Expected rows: 7, all keyed members_family.dart — HASH_AND_EQUALS
// (PairKey defines == without hashCode), AVOID_RETURN_TYPES_ON_SETTERS
// (the `void set tagged` return type), PREFER_FINAL_FIELDS (Tally._count
// initialized at declaration, never mutated), UNNECESSARY_GETTERS_SETTERS
// (Wrap's trivial _size wrap, reported at the getter), RECURSIVE_GETTERS
// (Loop.level reads itself), UNNECESSARY_THIS (WidthHost's `this.width`,
// no scope hit on either side), NULL_CHECK_ON_NULLABLE_TYPE_PARAMETER
// (Holder's `held!` under the written `T?` return context).
class PairKey {
  @override
  bool operator ==(Object other) => other is PairKey;
}

void set tagged(int input) {}

class Tally {
  int _count = 0;
  int get count => _count;
}

class Wrap {
  int _size = 0;
  int get size => _size;
  set size(int input) {
    _size = input;
  }
}

class Loop {
  int get level => level;
}

class WidthHost {
  int width = 0;
  int get doubled => this.width * 2;
}

class Holder<T> {
  T? held;
  T? take() {
    return held!;
  }
}
