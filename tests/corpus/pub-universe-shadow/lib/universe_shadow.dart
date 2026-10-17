// Root-context library of the per-context package universe face: the ROOT
// config maps `sibling` to pkgs/sibling_a and lists no `tool` at all.
// Expected rows (approximate, N2 pins exact): the package:tool import is
// uri_does_not_exist (outside this universe; the crossed imports are
// intentionally unreferenced, so the oracle may pair an unused_import
// hint with it), siblingBOnly() is undefined here — it exists only in the
// sibling_b face used by the OTHER context — while siblingAOnly() resolves.
// A flattened universe (one global name->directory map) would resolve
// siblingBOnly() or leak tool.dart in here; either way a divergence row.
import 'package:sibling/sibling.dart';
import 'package:tool/tool.dart';

void main() {
  siblingAOnly();
  siblingBOnly();
}
