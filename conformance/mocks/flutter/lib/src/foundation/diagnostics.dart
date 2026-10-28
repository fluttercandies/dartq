
mixin Diagnosticable {
  @protected
  @mustCallSuper
  void debugFillProperties(DiagnosticPropertiesBuilder properties) {}
}

abstract class DiagnosticableTree with Diagnosticable {
  const DiagnosticableTree();

  @protected
  external List<DiagnosticsNode> debugDescribeChildren();
}

class DiagnosticPropertiesBuilder {
  void add(DiagnosticsNode property) {}
}

abstract class DiagnosticsNode {}

enum DiagnosticsTreeStyle {
  singleLine;
}

class DiagnosticsProperty<T> extends DiagnosticsNode {
  DiagnosticsProperty(
    String? name,
    T? value, {
    String? description,
    String? ifNull,
    String? ifEmpty,
    super.showName,
    super.showSeparator,
    Object? defaultValue = kNoDefaultValue,
    String? tooltip,
    bool missingIfNull = false,
    super.linePrefix,
    bool expandableValue = false,
    bool allowWrap = true,
    bool allowNameWrap = true,
    DiagnosticsTreeStyle super.style = DiagnosticsTreeStyle.singleLine,
    DiagnosticLevel level = DiagnosticLevel.info,
  });
}
