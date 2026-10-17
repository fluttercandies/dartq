// D1 defining library.
//
// Library fragment order for the official duplicate walk (upstream Dart
// SDK source, pkg/analyzer/lib/src/error/duplicate_definition_verifier.dart,
// `checkUnit`): the defining unit first, then parts in `part` directive
// order — here `d1.dart` -> `a.dart` -> `b.dart`. Each unit is checked
// against the namespace accumulated from strictly-earlier fragments only.
//
// Faces:
// * `libVsPart`   — defining-vs-part collision (second occurrence lives
//   in a.dart); the defining occurrence is the FIRST in library order, so
//   this file is expected silent.
// * `partVsPart`  — lives only in the two parts; the second occurrence
//   (b.dart) is the expected reporting site.
// * `d1Unique`    — control, collides nowhere.
part 'a.dart';
part 'b.dart';

String libVsPart = 'lib';

int d1Unique = 1;
