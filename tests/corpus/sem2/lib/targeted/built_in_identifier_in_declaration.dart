// Targets built_in_identifier_in_declaration and not_a_type: the built-in
// identifier 'Function' used as a declaration name (class/typedef/mixin/
// extension and type-parameter positions) and then as a type. Verbatim
// mirror of the regression-nail source (oracle 3.13.1 rows pinned there):
// each declaration position reports the built-in misuse; the type
// positions report not_a_type.
class Function {}

class C<Function> {}

typedef Function = int;

typedef F<Function> = int;

extension Function on List {}

extension E<Function> on List<Function> {}

mixin Function {}

mixin M<Function> implements List<Function> {}

void main() {
  Function ok;
  dynamic okToo;
}
