// Tool-context library of the per-context package universe face: pkgs/tool's
// config maps `sibling` to pkgs/sibling_b and lists no universe_shadow.
// Expected rows (approximate, N2 pins exact): the package:universe_shadow
// import is uri_does_not_exist (may pair with an unused_import hint — the
// crossed imports are intentionally unreferenced) and siblingAOnly() is
// undefined here, while siblingBOnly() resolves — the mirror image of the
// root library. A flattened universe resolving either side differently is
// a divergence row.
import 'package:sibling/sibling.dart';
import 'package:universe_shadow/universe_shadow.dart';

void main() {
  siblingBOnly();
  siblingAOnly();
}
