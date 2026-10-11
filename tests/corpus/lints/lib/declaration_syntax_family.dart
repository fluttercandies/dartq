// declaration-syntax family — fixture wave 2 kitchen file.
// avoid_types_as_parameter_names uses the type-parameter face (`<Map>`):
// the formal-parameter face (`void f(String) {}`) would co-fire
// non_constant_identifier_names, and the built-in list is the dartq
// lexical face (user-declared type names under-report and stay out).
// Expected rows: 6, all keyed declaration_syntax_family.dart —
// CAMEL_CASE_EXTENSIONS (string_helpers), AVOID_TYPES_AS_PARAMETER_NAMES
// (the `Map` type parameter — UpperCamel, so camel_case_types stays
// silent), USE_FUNCTION_TYPE_SYNTAX_FOR_PARAMETERS (the old `void
// action()` formal), PREFER_GENERIC_FUNCTION_TYPE_ALIASES (the old-form
// typedef without `=`), LIBRARY_PRIVATE_TYPES_IN_PUBLIC_API (_Vault in
// openVault's return type), AVOID_SHADOWING_TYPE_PARAMETERS (the local
// function's <T> over shadowHost's <T>; every matching ancestor reports
// separately, and here there is exactly one).
extension string_helpers on String {}

typedef int Compare(int left, int right);

void genericHost<Map>() {}

void schedule(void action()) {
  action();
}

class _Vault {}

_Vault openVault() => _Vault();

void shadowHost<T>() {
  void shadowGuest<T>() {
    print(0);
  }

  print(shadowGuest<T>);
}
