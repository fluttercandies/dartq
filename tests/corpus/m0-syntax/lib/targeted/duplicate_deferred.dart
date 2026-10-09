// Targets duplicate_deferred: an import directive can only have one
// 'deferred' keyword.
import 'dart:math' deferred deferred as m;
