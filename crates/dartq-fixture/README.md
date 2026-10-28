# dartq-fixture

dartq's behavioral lint-test fixture DSL: markup-annotated Dart sources run
through the production lint pipeline, asserted against the annotations —
plus the converter that ports the upstream linter's rule tests
(`pkg/linter/test/rules/*_test.dart`, upstream Dart SDK source) into that
format mechanically.

## The fixture grammar

One `.fixture` file carries a lint name and any number of cases; each case
holds one or more files of marked Dart, the first being the analyzed unit:

```text
//- lint: avoid_empty_else
//- case: test_else_emptyStatement_noElseIf
//- file: /lib/test.dart
void f() {
  if (x > y)
    print('');
  else [!;!]
    print('');
}
```

Inline markers follow the upstream `TestCode` grammar byte for byte
(`[!…!]` ranges, `/*[N*/…/*N]*/` numbered ranges, `/**/` escapes); every
range is one expected diagnostic of the case's lint, in UTF-16
coordinates. A case with no ranges expects no diagnostics. Line-anchored
markers (`// ^^^ rule_name`, the rust-analyzer form) are available for
hand-written cases and can pin the rendered message.

## The runner

`assert_doc` feeds each case's analyzed file through the real pipeline —
`dartq_lints::parse_unit`'s scan/parse/arena bridge, the registry enable
set, the engine run — mounting the semantic fold exactly when the enabled
rule consumes it, the driver's per-file shape. Payload-contract pins (the
AST shape a rule reads) stay with dartq-lints' hand-built arena fixtures;
this DSL is for behavioral cases.

## The converter

`port_upstream <rule>_test.dart [out.fixture]` ports one upstream rule
test: `assertDiagnosticsFromMarkup` / `assertNoDiagnostics` bodies become
cases (case names are the upstream test method names), `newFile` support
files ride along, and anything unrepresentable — raw `assertDiagnostics`
error rows, `code:` overrides, version-pinned classes — is rejected with a
reason in the tally, never silently dropped. The checked-in data files
regenerate through dartq-fixture's `port_from_upstream` test.
