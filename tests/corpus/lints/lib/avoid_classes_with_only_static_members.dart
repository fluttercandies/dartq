// avoid_classes_with_only_static_members — official rule faces
// (pkg/linter avoid_classes_with_only_static_members.dart:30-71): a
// class with no instance members, no (non-default) constructor, and —
// per the official `hasStaticMethod || hasNonConstField` seal — at
// least one static method or one non-const static field. All-const
// utility classes and classes with instance members are exempt.
// Expected rows: 2 — AVOID_CLASSES_WITH_ONLY_STATIC_MEMBERS keyed
// avoid_classes_with_only_static_members.dart:
//   `class A` -> static method present
//   `class B` -> non-const static field, no methods
class A {
  static const one = 1;

  static void m() {}
}

class B {
  static int two = 2;
}

// GOOD: all-const members, instance members, and a subclass inheriting
// instance members from a same-file supertype.
class C {
  static const three = 3;
}

class D {
  int four = 4;
}

class Base {
  int five = 5;
}

class Derived extends Base {
  static void m() {}
}
