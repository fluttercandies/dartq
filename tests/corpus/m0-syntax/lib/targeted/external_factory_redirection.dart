// Targets external_factory_redirection: a redirecting factory cannot be
// external.
class C {
  external factory C() = C.b;

  C.b();
}
