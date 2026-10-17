// messages-example: analyzer/messages.yaml FfiCode.ffiNativeInvalidDuplicateDefaultAsset
@DefaultAsset('a')
@DefaultAsset('b')
library;

import 'dart:ffi';
