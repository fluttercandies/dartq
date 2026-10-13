// Targets initializing_declaring_parameter,
// super_initializing_declaring_parameter,
// initializing_formal_for_non_existent_field and
// super_formal_parameter_without_associated_positional: primary-constructor
// declaring parameters can't be initializing ('this'/'super') formals, and
// super formals need an associated positional super-constructor parameter.
// Verbatim mirror of the regression-nail source (oracle 3.13.1 rows pinned
// there; co-fires missing_primary_constructor_parameters, frozen in the
// syntax tier).
class C(var this.str);
class C(final this.str);
class C(const this.str);
class D(var super.str);
class D(final super.str);
class D(const super.str);
