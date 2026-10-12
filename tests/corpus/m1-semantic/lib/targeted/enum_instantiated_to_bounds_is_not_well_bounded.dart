// Targets enum_instantiated_to_bounds_is_not_well_bounded: the enum's
// implicit values field type (the enum instantiated to bounds) is not
// well-bounded. At dart 3.13.1 the only confirmed live trigger shape is a
// typedef-to-function-type bound on the enum's type parameter; a plain
// F-bounded class bound co-reports type_argument_not_matching_bounds
// instead and does not reach this code.
//
// Oracle rows (dart 3.13.1, `dart analyze --format=machine`; <file> is this
// file's path):
// ERROR|COMPILE_TIME_ERROR|ENUM_INSTANTIATED_TO_BOUNDS_IS_NOT_WELL_BOUNDED|<file>|13|6|1|The result of instantiating the enum to bounds is not well-bounded.
typedef A<X> = X Function(X);

enum E<T extends A<T>, U> {
  v<Never, int>()
}
