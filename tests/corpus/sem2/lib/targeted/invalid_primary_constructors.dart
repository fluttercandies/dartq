// Targets mixin_primary_constructor, extension_primary_constructor and
// const_without_primary_constructor: mixins and extensions can't declare
// primary constructors; 'const' on a class/enum/extension-type head
// requires one. Verbatim mirror of the regression-nail source (oracle
// 3.13.1 rows pinned there; co-fires duplicate_definition, m1, and
// enum_without_constants / missing_primary_constructor /
// expected_representation_field, frozen in the syntax tier).
class const C {}
enum const E {}
extension type const E {}
mixin M() {}
mixin M(int i) {}
mixin M.named() {}
mixin M.named(int i) {}
mixin M<T>() {}
mixin M<T>(int i) {}
mixin M<T>.named() {}
mixin M<T>.named(int i) {}
mixin const M() {}
mixin const M(int i) {}
mixin const M.named() {}
mixin const M.named(int i) {}
mixin const M<T>() {}
mixin const M<T>(int i) {}
mixin const M<T>.named() {}
mixin const M<T>.named(int i) {}
extension E() on int {}
extension E(int i) on int {}
extension E.named() on int {}
extension E.named(int i) on int {}
extension E<T>() on int {}
extension E<T>(int i) on int {}
extension E<T>.named() on int {}
extension E<T>.named(int i) on int {}
extension const E() on int {}
extension const E(int i) on int {}
extension const E.named() on int {}
extension const E.named(int i) on int {}
extension const E<T>() on int {}
extension const E<T>(int i) on int {}
extension const E<T>.named() on int {}
extension const E<T>.named(int i) on int {}
