// Targets invalid_super_in_initializer: 'super' in an initializer can only
// call a superclass constructor, e.g. 'super()' or 'super.named()'.
class A {}

class B extends A {
  B() : super().foo {}
}
