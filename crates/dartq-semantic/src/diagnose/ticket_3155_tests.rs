//! Pins for ticket #3155's FFI red-family gates (`NativeCallable`'s
//! listener/isolateLocal arms, upstream `ffi_verifier.dart:1915-2012`).
//! Each test embeds the conformance trigger and asserts the oracle row on
//! `lib/test.dart`.

use super::FileReport;
use super::test_rig::{CORE_PRELUDE, RigImport, report_ops_imports, rows};

const FFI_SOURCE: &str = "class Pointer<T> {}\n\
     class NativeFunction<T> {}\n\
     class Array<T> {}\n\
     class NativeCallable<T> {}\n\
     class Struct {}\n\
     class Union {}\n\
     class AbiSpecificInteger {}\n\
     class NativeFieldWrapperClass1 {}\n\
     class Finalizable {}\n\
     class Handle {}\n\
     class Void {}\n\
     class Native {}\n\
     class Int8 {}\n\
     class Int16 {}\n\
     class Int32 {}\n\
     class Int64 {}\n\
     class Uint8 {}\n\
     class IntPtr {}\n\
     class Int {}\n\
     class Float {}\n\
     class Double {}\n\
     class Bool {}\n\
     class DefaultAsset { const DefaultAsset(this.v); final String v; }\n";

fn report(source: &str) -> FileReport {
    report_ops_imports(
        source,
        &[RigImport {
            uri: "dart:ffi",
            prefix: None,
            source: FFI_SOURCE,
        }],
    )
}

fn at(fixture: &str, case_line: usize, col: usize) -> usize {
    let mut offset: usize = CORE_PRELUDE.chars().map(char::len_utf16).sum();
    for (index, line) in fixture.split('\n').enumerate() {
        if index + 2 == case_line {
            break;
        }
        offset += line.chars().map(char::len_utf16).sum::<usize>() + 1;
    }
    offset + col - 1
}

#[track_caller]
fn assert_row(report: &FileReport, code: &str, offset: usize, length: u32) {
    let hit = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == code)
        .unwrap_or_else(|| panic!("missing {code}; rows: {:#?}", rows(report)));
    assert_eq!(
        hit.range.offset as usize, offset,
        "offset for {code}; rows: {:#?}",
        rows(report)
    );
    assert_eq!(hit.range.length, length, "length for {code}");
}

#[track_caller]
fn assert_absent(report: &FileReport, code: &str) {
    assert!(
        !report.diagnostics.iter().any(|d| d.code.name == code),
        "{code} fired; rows: {:#?}",
        rows(report)
    );
}

// `NativeCallable.listener(f)` with no type argument: `Function` display,
// anchored at the constructor name (oracle 5:4+23).
#[test]
fn listener_must_have_type_args() {
    let f = "\nint f(int i) => i * 2;\nvoid g() {\n  NativeCallable.listener(f);\n}\n";
    assert_row(
        &report(f),
        "must_be_a_native_function_type",
        at(f, 5, 4),
        23,
    );
}

// A non-`Function` type argument fails the native-signature gate at the
// constructor name (oracle 5:4+43).
#[test]
fn listener_must_be_a_native_function_type() {
    let f = "\nvoid f(int i) => i * 2;\nvoid g() {\n  NativeCallable<void Function(int)>.listener(f);\n}\n";
    assert_row(
        &report(f),
        "must_be_a_native_function_type",
        at(f, 5, 4),
        43,
    );
}

// The Dart callback must be `dartToNative`-compatible; a mismatch reports
// at the callback `f` (oracle 5:51+1).
#[test]
fn listener_must_be_a_subtype() {
    let f = "\nvoid f(int i) => i * 2;\nvoid g() {\n  NativeCallable<Void Function(Double)>.listener(f);\n}\n";
    assert_row(&report(f), "must_be_a_subtype", at(f, 5, 51), 1);
}

// A non-`Void` native return reports `mustReturnVoid` at the callback
// (oracle 5:51+1).
#[test]
fn listener_must_return_void() {
    let f = "\nint f(int i) => i * 2;\nvoid g() {\n  NativeCallable<Int32 Function(Int32)>.listener(f);\n}\n";
    assert_row(&report(f), "must_return_void", at(f, 5, 51), 1);
}

// A valid listener reports nothing.
#[test]
fn listener_ok() {
    let f = "\nvoid f(int i) => i * 2;\nvoid g() {\n  NativeCallable<Void Function(Int32)>.listener(f);\n}\n";
    let r = report(f);
    assert_absent(&r, "must_return_void");
    assert_absent(&r, "must_be_a_subtype");
    assert_absent(&r, "must_be_a_native_function_type");
}

// `isolateLocal` with no type argument: `Function` display at the
// constructor name (oracle 5:4+27).
#[test]
fn isolate_local_must_have_type_args() {
    let f = "\nint f(int i) => i * 2;\nvoid g() {\n  NativeCallable.isolateLocal(f, exceptionalReturn: 4);\n}\n";
    assert_row(
        &report(f),
        "must_be_a_native_function_type",
        at(f, 5, 4),
        27,
    );
}

// A non-`Void`/`Handle`/`Pointer`/compound return with no exception value
// reports `missingExceptionValue` over the whole creation (oracle
// 5:4+53).
#[test]
fn isolate_local_missing_exception_value() {
    let f = "\nint f(int i) => i * 2;\nvoid g() {\n  NativeCallable<Int32 Function(Int32)>.isolateLocal(f);\n}\n";
    assert_row(&report(f), "missing_exception_value", at(f, 5, 4), 53);
}

// A `Void`-returning signature cannot take an exception value: the second
// argument reports `invalidExceptionValue` (oracle 5:57+20).
#[test]
fn isolate_local_invalid_exception_value() {
    let f = "\nvoid f(int i) => i * 2;\nvoid g() {\n  NativeCallable<Void Function(Int32)>.isolateLocal(f, exceptionalReturn: 4);\n}\n";
    assert_row(&report(f), "invalid_exception_value", at(f, 5, 57), 20);
}

// A non-const exception value reports `argumentMustBeAConstant` at the
// value (oracle 6:77+1).
#[test]
fn isolate_local_argument_must_be_a_constant() {
    let f = "\nint f(int i) => i * 2;\nvoid g() {\n  int e = 123;\n  NativeCallable<Int32 Function(Int32)>.isolateLocal(f, exceptionalReturn: e);\n}\n";
    assert_row(
        &report(f),
        "argument_must_be_a_constant",
        at(f, 6, 77),
        1,
    );
}

// A `dartToNative`-incompatible exception value reports `mustBeASubtype`
// at the value (oracle 5:77+3).
#[test]
fn isolate_local_exception_must_be_a_subtype() {
    let f = "\nint f(int i) => i * 2;\nvoid g() {\n  NativeCallable<Int32 Function(Int32)>.isolateLocal(f, exceptionalReturn: '?');\n}\n";
    assert_row(&report(f), "must_be_a_subtype", at(f, 5, 77), 3);
}

// A `dartToNative`-incompatible callback reports `mustBeASubtype` at the
// callback (oracle 5:56+1).
#[test]
fn isolate_local_must_be_a_subtype() {
    let f = "\nint f(int i) => i * 2;\nvoid g() {\n  NativeCallable<Int32 Function(Double)>.isolateLocal(f, exceptionalReturn: 4);\n}\n";
    assert_row(&report(f), "must_be_a_subtype", at(f, 5, 56), 1);
}

// A valid isolateLocal reports nothing.
#[test]
fn isolate_local_ok() {
    let f = "\nint f(int i) => i * 2;\nvoid g() {\n  NativeCallable<Int32 Function(Int32)>.isolateLocal(f, exceptionalReturn: 4);\n}\n";
    let r = report(f);
    assert_absent(&r, "must_be_a_subtype");
    assert_absent(&r, "missing_exception_value");
    assert_absent(&r, "invalid_exception_value");
    assert_absent(&r, "argument_must_be_a_constant");
}
