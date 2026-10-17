// Trigger for dart_code_linter `only-barrel-import`.
// Shape: an import reaching past a package's barrel file into `src/` — the
// rule's flagged form.
import 'package:some_pkg/src/internal/thing.dart';

void f() {}
