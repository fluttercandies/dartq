// Targets undefined_referenced_parameter: a @UseResult.unless annotation
// whose parameterDefined names no formal parameter of the annotated
// top-level function or method reports at the written value expression,
// quotes included (annotation_verifier.dart, _checkUseResult +
// _findUndefinedUseResultParameter). Negatives: foo.m names an existing
// parameter, bar has no unless selector, `annotated` has a non
// function/method parent, and the getter's null parameter list arm stays
// silent on both sides (official suite: test_method_parameterDefined,
// test_topLevelFunction, and the v2/v4/v5 probe boundaries).
import 'package:meta/meta.dart';

class Foo {
  @UseResult.unless(parameterDefined: 'undef')
  int foo([int? value]) => value ?? 0;

  @UseResult.unless(parameterDefined: 'value')
  int m([int? value]) => value ?? 0;

  @UseResult('documented')
  int plain([int? value]) => value ?? 0;

  @UseResult.unless(parameterDefined: 'undef')
  int get g => 1;
}

@UseResult.unless(parameterDefined: 'undef')
int top([int? value]) => value ?? 0;

@UseResult.unless(parameterDefined: 'value')
int topDefined([int? value]) => value ?? 0;

@UseResult.unless(parameterDefined: 'zzz')
const int annotated = 1;
