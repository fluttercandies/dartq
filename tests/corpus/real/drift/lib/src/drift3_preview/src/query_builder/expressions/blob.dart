import 'dart:typed_data';

import 'expression.dart';
import 'functions.dart';

/// Defines operations on blob ([Uint8List]) values.
extension BlobExpressionOperators on Expression<Uint8List> {
  /// The length of this blob, in bytes.
  Expression<int> get length => FunctionCallExpression('LENGTH', [this]);
}
