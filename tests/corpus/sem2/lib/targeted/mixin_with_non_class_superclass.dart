// Targets mixin_with_non_class_superclass: a class-mixin-application whose
// superclass names a non-class. Lines mirrored from the regression nail
// (oracle 3.13.1 rows pinned there: "Mixin can only be applied to class.";
// co-fires const_without_primary_constructor, sem2, and
// missing_primary_constructor_parameters, syntax tier).
class const C = O with M;

class const D<T> = O with M;
