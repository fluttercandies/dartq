// close_sinks — differential corpus (#1160).
// Source: upstream Dart SDK source,
// pkg/linter/test/rules/close_sinks_test.dart (transcribed 2026-10-14,
// vendored SDK 3.15 main face).
// Inventory: 5 POSITIVE + 14 NEGATIVE = all 19 upstream tests, none
// skipped. Primary-constructor faces ride the primary-constructors feature
// (enabled at the corpus language version 3.13). `// ignore: unused_field`
// comments are verbatim; where the ignored diagnostic is not produced they
// additionally trigger unnecessary_ignore on the oracle leg — the
// documented ignore-comment interaction family (see README).
// Expected rows (oracle leg): 5 — CLOSE_SINKS keyed close_sinks.dart, one
// per POSITIVE block.

// POSITIVE upstream test_extensionType_implementsSink_notClosed (1 row at
// the variable declaration).
import 'dart:io';
void fExtTypeNotClosed(IOSink sink) {
  ECloseExt e = ECloseExt(sink);
}
extension type ECloseExt(IOSink _s) implements IOSink {}

// POSITIVE upstream test_field_initializedInMethod_notClosed (1 row at
// `_sink`).
class CFieldNotClosed {
  // ignore: unused_field
  late IOSink _sink;
  void init() {
    _sink = File('').openWrite();
  }
}

// POSITIVE upstream
// test_field_initializedInMethod_originPrimaryConstructor_notClosed (1 row
// at `var IOSink _sink`; primary-constructors face).
// ignore: unused_field_from_primary_constructor
class CPrimaryNotClosed(var IOSink _sink) {
  void init() {
    _sink = File('').openWrite();
  }
}

// POSITIVE upstream
// test_field_initializedInMethod_withPrimaryConstructor_notClosed (1 row at
// `_sink`; primary-constructors face).
class CWithPrimaryNotClosed() {
  // ignore: unused_field
  late IOSink _sink;
  void init() {
    _sink = File('').openWrite();
  }
}

// POSITIVE upstream test_localVariable_inFunction_notInitialized (1 row at
// `sink`).
void someFunctionNotClosed() {
  IOSink sink;
}

// NEGATIVE upstream test_extensionType_implementsSink_closed.
void fExtTypeClosed(IOSink sink) {
  ECloseExtClosed e = ECloseExtClosed(sink);
  e.close();
}
extension type ECloseExtClosed(IOSink _s) implements IOSink {}

// NEGATIVE upstream test_field_closed.
class CFieldClosed {
  IOSink _sink;

  CFieldClosed(this._sink);

  void init() {
    _sink = File('').openWrite();
  }

  void dispose() {
    _sink.close();
  }
}

// NEGATIVE upstream test_field_closed_explicitThis.
class CFieldClosedThis {
  IOSink _sink;

  CFieldClosedThis(this._sink);

  void m1(filename) {
    _sink = File(filename).openWrite();
  }

  void m2(filename) {
    this._sink.close();
  }
}

// NEGATIVE upstream test_field_initializedFromExistingSink_notClosed.
class CFieldExistingSink {
  // ignore: unused_field
  late IOSink _sink;

  void m(IOSink sink) {
    _sink = sink;
  }
}

// NEGATIVE upstream
// test_field_initializedFromExistingSink_notClosed_explicitThis.
class CFieldExistingSinkThis {
  // ignore: unused_field
  IOSink _sink;

  CFieldExistingSinkThis(this._sink);

  void m(IOSink sink) {
    this._sink = sink;
  }
}

// NEGATIVE upstream
// test_field_initializedInConstructorInitializer_fromExistingSink.
class CFieldCtorInitSink {
  // ignore: unused_field
  final IOSink _sink;

  CFieldCtorInitSink(IOSink sink) : this._sink = sink;
}

// NEGATIVE upstream
// test_field_initializedInConstructorInitializer_inPrimaryConstructor_fromExistingSink
// (primary-constructors face).
class CFieldCtorInitPrimary(IOSink sink) {
  // ignore: unused_field
  final IOSink _sink;

  this : this._sink = sink;
}

// NEGATIVE upstream
// test_field_initializedInFieldFormalParameter_inPrimaryConstructor_notClosed
// (primary-constructors face).
class CFieldFormalPrimary(this._sink) {
  // ignore: unused_field
  late IOSink _sink;
  void init() {
    _sink = File('').openWrite();
  }
}

// NEGATIVE upstream test_field_initializedInFieldFormalParameter_notClosed.
class CFieldFormalSink {
  // ignore: unused_field
  final IOSink _sink;

  CFieldFormalSink(this._sink);
}

// NEGATIVE upstream test_field_originPrimaryConstructor_closed
// (primary-constructors face).
class CPrimaryClosed(var IOSink _sink) {
  void init() {
    _sink = File('').openWrite();
  }

  void dispose() {
    _sink.close();
  }
}

// NEGATIVE upstream test_localVariable_closed.
void fLocalClosed() {
  IOSink sink = File('').openWrite();
  sink.close();
}

// NEGATIVE upstream test_localVariable_inMethod_returned_notClosed.
class CLocalReturned {
  IOSink m(String filename) {
    IOSink sink = File(filename).openWrite();
    return sink;
  }
}

// NEGATIVE upstream test_socket_destroyed.
class CSocketDestroyed {
  Socket _socket;

  CSocketDestroyed(this._socket);

  Future m1() async {
    _socket = await Socket.connect(null /*address*/, 1234);
  }

  void m2() {
    _socket.destroy();
  }
}
