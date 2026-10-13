// Targets implements_non_class and mixin_super_class_constraint_non_interface:
// an implements clause naming a non-class and an on clause naming a
// non-interface. Verbatim mirror of the regression-nail source (oracle
// 3.13.1 rows pinned there; co-fires implements_before_on, syntax tier,
// and undefined_class, m1).
// Targets implements_before_on: the on clause must be before the implements
// clause.
mixin M implements B on A {}
