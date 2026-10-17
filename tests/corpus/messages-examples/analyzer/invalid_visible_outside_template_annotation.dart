// messages-example: analyzer/messages.yaml WarningCode.invalidVisibleOutsideTemplateAnnotation
import 'package:angular_meta/angular_meta.dart';

class C {
  @visibleOutsideTemplate
  int m() {
    return 1;
  }
}
