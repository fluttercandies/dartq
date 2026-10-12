// Targets both combinator codes in one file: undefined_shown_name (a show
// combinator names a member dart:async does not export) and
// undefined_hidden_name (a hide combinator names a member dart:math does
// not export). Combinator validation runs at import resolution, so the
// diagnostics fire regardless of use sites. Trigger conditions: the pinned
// resolution trigger table (lookup failures against the resolved import scope). Hand-crafted from the code semantics; no
// official test file.
import 'dart:async' show noSuchExport;
import 'dart:math' hide noSuchExport;

void main() {
  print(max(1, 2));
}
