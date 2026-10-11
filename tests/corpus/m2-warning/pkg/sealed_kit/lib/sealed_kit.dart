// Sealed surface for the m2 WARNING sealed-supertype corpus: every public
// class carries package:meta's @sealed annotation, so subtyping from another
// package violates the sealed contract while same-package use stays silent.
import 'package:meta/meta.dart';

@sealed
class Guard {
  int get guard => 0;
}

@sealed
class Vault {
  int get vault => 0;
}
