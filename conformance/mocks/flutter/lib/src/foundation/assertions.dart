
import 'diagnostics.dart';

class FlutterErrorDetails with Diagnosticable {
  const FlutterErrorDetails({
    required Object exception,
    StackTrace? stack,
    String? library = 'Flutter framework',
    DiagnosticsNode? context,
    Iterable<String> Function(Iterable<String>)? stackFilter,
    Iterable<DiagnosticsNode> Function()? informationCollector,
    bool silent = false,
  });
}

class FlutterError extends Error
    with DiagnosticableTreeMixin
    implements AssertionError {
  static void reportError(FlutterErrorDetails details) {}
}
