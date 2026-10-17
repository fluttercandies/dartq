// future_sync_value — differential corpus (#1160), LANGUAGE-GATE face.
// Source: upstream Dart SDK source,
// pkg/linter/test/rules/future_sync_value_test.dart,
// test_int_pre_valueSync (transcribed 2026-10-14, vendored SDK 3.15 main
// face). The `// @dart = 3.9` comment pins this library BELOW the rule's
// 3.10 language gate, so the identical call stays silent — the control
// twin of future_sync_value.dart. A @dart comment governs the whole file,
// which is why this face cannot share the main fixture's library.
// Expected rows: 0 on both legs — the rule bails below 3.10 (upstream rule
// source language gate). A dartq FUTURE_SYNC_VALUE row here is a @dart
// language-version resolution divergence, surfaced by the N5 gate.

// @dart = 3.9

// NEGATIVE upstream test_int_pre_valueSync (0 rows below the gate).
Future<int> fPreValueSync() => Future.value(1);
