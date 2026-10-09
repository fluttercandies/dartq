// Targets undefined_referenced_parameter: meta's @UseResult.unless names a
// parameter that the annotated member does not declare (requires package:meta,
// the only emission site in analyzer 8.x annotation_verifier).
import 'package:meta/meta.dart';

class C {
  @UseResult.unless(parameterDefined: 'noSuchParameter')
  int m([int? value]) => value ?? 0;
}
