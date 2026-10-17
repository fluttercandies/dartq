// messages-example: analyzer/messages.yaml WarningCode.undefinedReferencedParameter
import 'package:meta/meta.dart';

@UseResult.unless(parameterDefined: 'b')
int f([int? a]) => a ?? 0;
