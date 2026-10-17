// cancel_subscriptions — differential corpus (#1160).
// Source: upstream Dart SDK source,
// pkg/linter/test/rules/cancel_subscriptions_test.dart (transcribed
// 2026-10-14, vendored SDK 3.15 main face).
// Inventory: 5 POSITIVE + 15 NEGATIVE + 1 SKIPPED = the 21 upstream tests.
// SKIPPED (1): test_privateField_canceled_outsideTheClass — upstream marks
// it @FailingTest (the oracle contradicts the test's own expectation;
// transcribing it would score the oracle's known-internal failure as a
// differential verdict).
// Expected rows (oracle leg): 5 — CANCEL_SUBSCRIPTIONS keyed
// cancel_subscriptions.dart, one per POSITIVE block. Upstream cases
// compiled standalone; declarations carry block-suffixed names (the rule
// keys on resolved StreamSubscription types, renames are face-neutral).
// ignore comments are transcribed verbatim: where the ignored diagnostic
// is not produced (e.g. unused_field on a used field) they additionally
// trigger unnecessary_ignore on the oracle leg — the documented
// ignore-comment interaction family (see README).

// POSITIVE upstream test_extensionType_implementsStreamSubscription_notCanceled
// (1 row at the variable declaration).
import 'dart:async';
void fExtTypeNotCanceled(Stream stream) {
  ECancelExt e = ECancelExt(stream.listen((_) {}));
}
extension type ECancelExt(StreamSubscription _s)
    implements StreamSubscription {}

// POSITIVE upstream test_localVariable_notCanceled (1 row at `s`).
void fLocalNotCanceled(Stream stream) {
  StreamSubscription s = stream.listen((_) {});
}

// POSITIVE upstream test_privateField_notCanceled (1 row at
// `_subscription`).
class CPrivateNotCanceled {
  late StreamSubscription _subscription;
  void f(Stream stream) {
    _subscription = stream.listen((_) {});
  }
}

// POSITIVE upstream
// test_privateField_originPrimaryConstructor_notCanceled (1 row at
// `var StreamSubscription _subscription`; primary-constructors face,
// enabled at the corpus language version).
// ignore: unused_field_from_primary_constructor
class CPrimaryNotCanceled(var StreamSubscription _subscription) {
  void f(Stream stream) {
    _subscription = stream.listen((_) {});
  }
}

// POSITIVE upstream test_publicField_notCanceled (1 row at `subscription`).
class CPublicNotCanceled {
  late StreamSubscription subscription;
  void f(Stream stream) {
    subscription = stream.listen((_) {});
  }
}

// NEGATIVE upstream
// test_extensionType_implementsStreamSubscription_canceled.
void fExtTypeCanceled(Stream stream) {
  ECancelExtClosed e = ECancelExtClosed(stream.listen((_) {}));
  e.cancel();
}
extension type ECancelExtClosed(StreamSubscription _s)
    implements StreamSubscription {}

// NEGATIVE upstream test_extensionType_representation_notCanceled (an
// extension type whose representation is a subscription, never assigned
// from listen, stays silent).
extension type ECancelRepr(StreamSubscription _s) {}

// NEGATIVE upstream test_localVariable_canceled_cascade.
void fLocalCascade(Stream stream) {
  var s = stream.listen((_) {})..cancel();
}

// NEGATIVE upstream test_localVariable_canceled_inMethodInvocation.
void fLocalMethodCancel(Stream stream) {
  StreamSubscription s = stream.listen((_) {});
  unawaited(s.cancel());
}

// NEGATIVE upstream test_localVariable_canceled_nullAware.
StreamSubscription? fLocalNullAware(StreamSubscription? s) {
  StreamSubscription? s2 = s;
  s2?.cancel();
  return s2;
}

// NEGATIVE upstream test_localVariable_canceled_tearOff (a tear-off counts
// as a cancel call — current official behavior).
void fLocalTearOff(Stream stream) {
  StreamSubscription s = stream.listen((_) {});
  s.cancel;
}

// NEGATIVE upstream test_parameter_notCanceled.
void fParameter(StreamSubscription s) {}

// NEGATIVE upstream test_privateField_canceled.
class CPrivateCanceled {
  late StreamSubscription _subscription;
  void f(Stream stream) {
    _subscription = stream.listen((_) {});
  }
  void g() {
    _subscription.cancel();
  }
}

// NEGATIVE upstream test_privateField_canceled_nullable.
class CPrivateCanceledNullable {
  StreamSubscription? _subscription;
  void f(Stream stream) {
    _subscription = stream.listen((_) {});
  }
  void g() {
    _subscription!.cancel();
  }
}

// NEGATIVE upstream test_privateField_canceled_withNullCheck.
class CPrivateCanceledNullCheck<T> {
  StreamSubscription<T>? subscription;
  void unsubscribe() {
    if (subscription != null) {
      subscription!.cancel();
      subscription = null;
    }
  }
}

// NEGATIVE upstream test_privateField_originPrimaryConstructor_canceled
// (primary-constructors face).
class CPrimaryCanceled(var StreamSubscription _subscription) {
  void f(Stream stream) {
    _subscription = stream.listen((_) {});
  }
  void g() {
    _subscription.cancel();
  }
}

// NEGATIVE upstream
// test_privateField_initializedInConstructorInitializer_notCanceled.
class CPrivateCtorInit {
  StreamSubscription _subscription;
  CPrivateCtorInit(StreamSubscription subscription) :
      _subscription = subscription;
  void f(Stream stream) {
    _subscription = stream.listen((_) {});
  }
}

// NEGATIVE upstream
// test_privateField_initializedInConstructorInitializer_primaryConstructorBody_notCanceled
// (primary-constructors face).
class CPrivateCtorInitPrimary(StreamSubscription subscription) {
  StreamSubscription _subscription;
  this : _subscription = subscription;
  void f(Stream stream) {
    _subscription = stream.listen((_) {});
  }
}

// NEGATIVE upstream
// test_privateField_initializedInFieldFormalParameter_notCanceled.
class CPrivateFieldFormal {
  StreamSubscription _subscription;
  CPrivateFieldFormal(this._subscription);
  void f(Stream stream) {
    _subscription = stream.listen((_) {});
  }
}

// NEGATIVE upstream test_topLevelVariable_notCanceled.
late StreamSubscription subscriptionTop;
void fTopNotCanceled(Stream stream) {
  subscriptionTop = stream.listen((_) {});
}
