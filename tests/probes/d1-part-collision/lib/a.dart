// D1 part 1 (second fragment in library order).
//
// * `libVsPart` collides with the defining library's declaration. The
//   official walk checks this unit against the earlier fragments'
//   namespace, so this file is the candidate reporting site for the
//   defining-vs-part face; within one variable declaration the implicit
//   getter and setter checks share the name token, and the reported
//   token is marked done, collapsing the pair to at most one row.
// * `partVsPart` is its FIRST occurrence (checked against d1.dart only,
//   which does not declare it) — expected silent here.
// * `aUnique` — control, collides nowhere.
part of 'd1.dart';

int libVsPart = 2;

int partVsPart = 10;

int aUnique = 11;
