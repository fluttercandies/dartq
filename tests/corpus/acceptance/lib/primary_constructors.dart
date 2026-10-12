// Valid Dart 3.15 acceptance corpus — primary constructors (shipped,
// enabledIn 3.13.0): header parameters (var/final/typed fields, plain
// parameters), class clauses (extends/with/implements), `;` empty bodies,
// `class const` primary constructors, `new` constructor declarations,
// mixin and extension-type primary forms, unnamed `extension on T;`.
//
// Provenance: shapes modeled on the official language tests
// upstream Dart SDK source, tests/language/primary_constructors/
// feature_enabled_test.dart, syntax/class_header_test.dart,
// syntax/empty_body_test.dart, and short_syntax/new_test.dart (SDK main @
// e6f4207a). Covered by scripts/acceptance-gate.sh (zero-diagnostics bar,
// `dartq check` machine face).

class Point(var int x, var int y);

class GenericsHeader<T>(final T x);

class Base {
  const Base();
}

class ExtendsHeader(final int x) extends Base;

abstract class Interface {
  int method();
}

class ImplementsHeader(final int x) implements Interface {
  @override
  int method() => x + 1;
}

mixin Mixin {}

class WithHeader(final int x) with Mixin;

class AllHeader<T>(final T x) extends Base with Mixin implements Interface {
  @override
  int method() => x;
}

class NewStyle {
  int x;
  new() : x = 1;
  new named() : x = 2;
}

class EmptyBody;
class EmptyExtends extends Base;
class EmptyWith with Mixin;
class EmptyAll extends Base with Mixin implements Interface;

class const ConstPrimary();
class const ConstPrimaryExtends() extends Base;

class NamedBody(int value) {
  int doubled() => value * 2;
}

class ImplementsContract(final int x) implements Contract;

interface class Contract {}

mixin EmptyMixin;
mixin OnMixin on Base;

class UsesEmptyMixin with EmptyMixin;
class UsesOnMixin extends Base with OnMixin;

extension type MetersType(final int meters);
extension type const ConstMeters(final int meters);
extension on Base;

void main() {
  final p = Point(1, 2);
  final g = GenericsHeader<int>(3);
  final e = ExtendsHeader(4);
  final i = ImplementsHeader(5);
  final w = WithHeader(6);
  final a = AllHeader<int>(7);
  final n = NewStyle();
  final nn = NewStyle.named();
  final nb = NamedBody(8);
  final m = MetersType(9);
  final c = ConstMeters(10);
  final ic = ImplementsContract(11);
  final uem = UsesEmptyMixin();
  final uom = UsesOnMixin();
  EmptyBody();
  EmptyExtends();
  EmptyWith();
  EmptyAll();
  ConstPrimary();
  ConstPrimaryExtends();

  if (p.x +
          p.y +
          g.x +
          e.x +
          i.x +
          i.method() +
          w.x +
          a.x +
          a.method() +
          n.x +
          nn.x +
          nb.doubled() +
          m.meters +
          c.meters +
          ic.x !=
      90) {
    throw StateError('unreachable');
  }
}
