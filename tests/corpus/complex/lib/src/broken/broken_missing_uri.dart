// Intentional diagnostics: uri_does_not_exist. Two import URIs that do not
// resolve: one relative sibling, one relative subdirectory.
// expect: uri_does_not_exist
import 'no_such_helper.dart';

// expect: uri_does_not_exist
import 'missing/nothing.dart';

void use() {
  print('imports never resolve');
}
