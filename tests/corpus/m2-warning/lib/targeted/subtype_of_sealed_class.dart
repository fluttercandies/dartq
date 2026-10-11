// Targets subtype_of_sealed_class: a declaration whose supertype closure
// contains a cross-package @sealed class (package:meta annotation, matched
// by top-level getter `sealed` in library `meta`) reports once PER sealed
// supertype with no break between them (BestPracticesVerifier
// _checkForInvalidSealedSuperclass walks the full allSupertypes closure).
//
// Problem message: "The class '{0}' shouldn't be extended, mixed in, or
// implemented because it's sealed."
// Correction message: "Try composing instead of inheriting, or refer to the
// documentation of '{0}' for more information."
//
// Expected diagnostics (declaration span, STATIC_WARNING, oracle order):
//   L31 subtype_of_sealed_class (Guard)
//   L37 subtype_of_sealed_class (Guard)
//   L37 subtype_of_sealed_class (Vault)
//   L42 subtype_of_sealed_class (Guard)
//   L45 subtype_of_sealed_class (Guard, transitively through Mid)
//   L50 subtype_of_sealed_class (Vault)
import 'package:meta/meta.dart';
import 'package:sealed_kit/sealed_kit.dart';

// Negative: @sealed inside the analyzed root package stays silent
// (_isLibraryInWorkspacePackage), so Same never reports.
@sealed
class Local {}

class Same extends Local {}

// Single extends face: the canonical violation.
// expect: subtype_of_sealed_class
class Sub extends Guard {}

// Per-sealed-supertype: two distinct sealed supertypes report at the same
// declaration, once each, no break between them.
// expect: subtype_of_sealed_class
// expect: subtype_of_sealed_class
class Breach extends Guard implements Vault {}

// Transitive closure: the sealed supertype reaches Deep through same-package
// Mid, so both report, once per declaring node.
// expect: subtype_of_sealed_class
class Mid extends Guard {}

// expect: subtype_of_sealed_class
class Deep extends Mid {}

// Mixin implementing a sealed class is the subtype face, not the constraint
// face (only a direct `on` constraint reports mixin_on_sealed_class).
// expect: subtype_of_sealed_class
mixin Adopts implements Vault {}
