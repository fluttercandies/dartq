import 'dart:typed_data';

import '../query_builder.dart';

/// Defines methods that operate on a column storing [Uint8List] values.
extension BlobExpressionOperators on Expression<Uint8List> {
  /// The length of this blob, in bytes.
  Expression<int> get length {
    return FunctionCallExpression('LENGTH', [this]);
  }
}
