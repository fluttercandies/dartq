// void_checks — the ported assignment face: a value written where the
// written FutureOr<void> field type is expected. The file is separate
// from members_family.dart because its dart:async import would flip
// dartq's unnecessary_this import-bail while the oracle (which resolves
// precisely) still fires — an asymmetry, not a fixture face.
// `drained` stays public (out of prefer_final_fields' private face) and
// the constructor's initializing formal keeps it definitely assigned.
// Expected rows: 1 — VOID_CHECKS keyed void_checks.dart (`drained =
// piece;` — int parameter into the FutureOr<void> field).
import 'dart:async';

class VoidSink {
  FutureOr<void> drained;

  VoidSink(this.drained);

  void pour(int piece) {
    drained = piece;
  }
}
