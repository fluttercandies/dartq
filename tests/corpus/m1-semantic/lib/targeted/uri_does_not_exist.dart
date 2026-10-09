// Targets uri_does_not_exist: an import names a relative file that does not
// exist (frozen M1 uri family, resolution-design-spec 4.2; this fixture
// legitimately emits the code it targets).
import 'definitely_missing.dart';

void main() {}
