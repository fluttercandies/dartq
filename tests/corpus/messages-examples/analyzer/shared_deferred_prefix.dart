// messages-example: analyzer/messages.yaml CompileTimeErrorCode.sharedDeferredPrefix
import 'dart:math' deferred as x;
import 'dart:convert' as x;

var y = x.json.encode(x.min(0, 1));
