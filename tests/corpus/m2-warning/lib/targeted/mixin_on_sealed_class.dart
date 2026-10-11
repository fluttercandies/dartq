// Targets mixin_on_sealed_class: a mixin whose direct `on` constraint is a
// cross-package @sealed class (package:meta annotation) reports the
// constraint-specific variant once (BestPracticesVerifier
// _checkForInvalidSealedSuperclass, MixinElement.superclassConstraints face).
//
// Problem message: "The class '{0}' shouldn't be used as a mixin constraint
// because it is sealed, and any class mixing in this mixin must have '{0}'
// as a superclass."
// Correction message: "Try composing with this class, or refer to its
// documentation for more information."
//
// Expected diagnostics (declaration span, STATIC_WARNING, oracle order):
//   L17 mixin_on_sealed_class (Guard)
import 'package:sealed_kit/sealed_kit.dart';

// expect: mixin_on_sealed_class
mixin NeedsGuard on Guard {}
