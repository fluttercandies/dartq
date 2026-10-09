// Targets invalid_this_in_initializer: 'this' in an initializer can only be
// used for field initialization or constructor redirection.
class A {}

class B extends A {
  B() : this().foo;
}
