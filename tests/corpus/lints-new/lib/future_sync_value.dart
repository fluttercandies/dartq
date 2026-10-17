// future_sync_value — differential corpus (#1160).
// Source: upstream Dart SDK source,
// pkg/linter/test/rules/future_sync_value_test.dart (transcribed
// 2026-10-14, vendored SDK 3.15 main face).
// Inventory: 3 POSITIVE + 5 NEGATIVE = all 8 upstream tests. The
// language-gated face (upstream test_int_pre_valueSync, `// @dart = 3.9`)
// cannot share this library — a @dart comment governs the whole file — so
// it lives in future_sync_value_pre.dart.
// Expected rows (oracle leg): 3 — FUTURE_SYNC_VALUE keyed
// future_sync_value.dart, one per POSITIVE block (at the `value` name).
// The rule is gated on the library's effective language version >= 3.10;
// this library settles at the corpus 3.13, so every gate is open.

import 'dart:async';

// POSITIVE upstream test_dotShorthands (1 row at `value`; the dot-shorthand
// face rides the 3.10 feature at the corpus language version).
Future<int> fDotShorthands() => .value(1);

// NEGATIVE upstream test_dynamic (a dynamic argument defeats the check).
Future<int> fDynamic(p) => Future.value(p);

// NEGATIVE upstream test_FutureInt (a Future argument is not a sync value).
Future<int> fFutureInt(Future<int> f) => Future.value(f);

// NEGATIVE upstream test_futureOr.
Future<FutureOr> fFutureOr(FutureOr f) => Future.value(f);

// NEGATIVE upstream test_FutureOrInt.
Future<int> fFutureOrInt(FutureOr<int> f) => Future.value(f);

// POSITIVE upstream test_int (1 row at `value`).
Future<int> fInt() => Future.value(1);

// POSITIVE upstream test_object (1 row at `value`).
Future<Object> fObject(Object o) => Future.value(o);
