// Targets type_argument_not_matching_bounds: an explicit type argument
// violates the substituted bound of its type parameter.
//
// Oracle rows (dart 3.13.1, `dart analyze --format=machine`; <file> is this
// file's path):
// ERROR|COMPILE_TIME_ERROR|TYPE_ARGUMENT_NOT_MATCHING_BOUNDS|<file>|9|10|6|'String' doesn't conform to the bound 'num' of the type parameter 'T'.
class A<T extends num> {}

void f(A<String> a) {}
