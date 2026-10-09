// Targets with_before_extends: the extends clause must be before the with
// clause.
class B {}

mixin M {}

class C with M extends B {}
