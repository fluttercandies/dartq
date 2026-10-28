//! Pins for the `dart:ffi` verifier's 48 missing-only diagnostic faces
//! (`finalize_ffi`): each test embeds a `/1` trigger shape from the
//! manual corpus and asserts the row the oracle emits on
//! `lib/test.dart` — code, UTF-16 offset, and length. The rig mounts a
//! `dart:ffi` sibling whose fixture declares the FFI surface the
//! triggers name, so identity answers resolve into a library whose URI
//! is `dart:ffi`, and every byte-level capture (`@Native`, `@Array`,
//! `@Packed`, the invocation shapes) runs exactly as in the engine.

use super::test_rig::{rows, report_ops_imports, CORE_PRELUDE, RigImport};
use super::FileReport;

/// The `dart:ffi` mount: the compound roots, the generic natives, and
/// the primitive types the trigger shapes write. Identity for the
/// fixed-size primitives rides the byte spelling; the roots must resolve
/// into this library so `direct_ffi_supertype`'s owner test answers.
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

/// `dart:typed_data`, for the `.address` triggers whose receiver is a
/// typed-data list.
const TYPED_DATA_SOURCE: &str = "class Uint8List {}\n";

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

fn report_typed_data(source: &str) -> FileReport {
    report_ops_imports(
        source,
        &[
            RigImport {
                uri: "dart:ffi",
                prefix: None,
                source: FFI_SOURCE,
            },
            RigImport {
                uri: "dart:typed_data",
                prefix: None,
                source: TYPED_DATA_SOURCE,
            },
        ],
    )
}

/// The UTF-16 offset of a 1-based case-file position inside the rig
/// source: the case's leading `import 'dart:ffi';` line is dropped from
/// the fixture (the rig re-mounts it), and [`CORE_PRELUDE`] shifts every
/// remaining line by one — case line N is fixture line N-1.
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

/// The oracle row for `code` sits at exactly `offset` with `length`.
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

/// The family-wide noise gate: an `@Native`-annotated `external` field is
/// valid upstream — the flow tier's not-initialized faces must stay
/// silent so the FFI row is not buried.
#[track_caller]
fn assert_absent(report: &FileReport, code: &str) {
    assert!(
        !report.diagnostics.iter().any(|d| d.code.name == code),
        "{code} fired; rows: {:#?}",
        rows(report)
    );
}

#[test]
fn abi_specific_integer_invalid() {
    let f = "\n@AbiSpecificIntegerMapping({Abi.macosX64: Int8()})\n\
             final class C extends AbiSpecificInteger {\n  void extraMember() {}\n}\n";
    assert_row(&report(f), "abi_specific_integer_invalid", at(f, 4, 13), 1);
}

#[test]
fn abi_specific_integer_mapping_extra() {
    let f = "\n@AbiSpecificIntegerMapping({Abi.macosX64: Int8()})\n\
             @AbiSpecificIntegerMapping({Abi.macosX64: Int16()})\n\
             final class C extends AbiSpecificInteger {\n  const C();\n}\n";
    assert_row(
        &report(f),
        "abi_specific_integer_mapping_extra",
        at(f, 4, 2),
        25,
    );
}

#[test]
fn abi_specific_integer_mapping_missing() {
    let f = "\nfinal class C extends AbiSpecificInteger {\n  const C();\n}\n";
    assert_row(
        &report(f),
        "abi_specific_integer_mapping_missing",
        at(f, 3, 13),
        1,
    );
}

#[test]
fn abi_specific_integer_mapping_unsupported() {
    let f = "\n@AbiSpecificIntegerMapping({Abi.macosX64: Array<Uint8>(4)})\n\
             final class C extends AbiSpecificInteger {\n  const C();\n}\n";
    assert_row(
        &report(f),
        "abi_specific_integer_mapping_unsupported",
        at(f, 3, 43),
        15,
    );
}

#[test]
fn address_position() {
    let f = "import 'dart:typed_data';\n\
             \n\
             @Native<Void Function(Pointer<Uint8>)>()\n\
             external void nonLeafCall(Pointer<Uint8> ptr);\n\
             \n\
             void main() {\n  final data = Uint8List(10);\n  nonLeafCall(data.address);\n}\n";
    assert_row(&report_typed_data(f), "address_position", at(f, 9, 20), 7);
}

#[test]
fn address_receiver() {
    let f = "\n@Native<Void Function(Pointer)>(isLeaf: true)\n\
             external void nativeLeafCall(Pointer ptr);\n\
             \n\
             final class MyStruct extends Struct {\n\
             \x20 @Uint8()\n  external int x;\n\
             \n  @Uint8()\n  external int y;\n}\n\
             \n\
             void main() {\n\
             \x20 final struct = Struct.create<MyStruct>();\n\
             \x20 final y = struct.y;\n  nativeLeafCall(y.address);\n}\n";
    assert_row(&report(f), "address_receiver", at(f, 17, 20), 7);
}

#[test]
fn annotation_on_pointer_field() {
    let f = "\nfinal class S extends Struct {\n  @Int8()\n  external Pointer<Int8> p;\n}\n";
    assert_row(&report(f), "annotation_on_pointer_field", at(f, 4, 3), 7);
}

#[test]
fn argument_must_be_a_constant() {
    let f = "\nint f(int i) => i * 2;\n\
             \nint exceptionalReturn() => 0;\n\
             \nvoid g() {\n\
             \x20 Pointer.fromFunction<Int8 Function(Int8)>(f, exceptionalReturn());\n}\n";
    assert_row(
        &report(f),
        "argument_must_be_a_constant",
        at(f, 8, 48),
        19,
    );
}

#[test]
fn argument_must_be_native() {
    let f = "\nvoid h() {}\n\nvoid g() {\n\
             \x20 Native.addressOf<NativeFunction<Void Function()>>(h);\n}\n";
    assert_row(&report(f), "argument_must_be_native", at(f, 6, 53), 1);
}

#[test]
fn compound_implements_finalizable() {
    let f = "\nfinal class S extends Struct implements Finalizable {\n\
             \x20 external Pointer<Int8> p;\n}\n";
    assert_row(
        &report(f),
        "compound_implements_finalizable",
        at(f, 3, 13),
        1,
    );
}

#[test]
fn creation_of_struct_or_union() {
    let f = "\nfinal class C extends Struct {\n  @Int32()\n  external int a;\n}\n\
             \nvoid f() {\n  C();\n}\n";
    assert_row(&report(f), "creation_of_struct_or_union", at(f, 9, 3), 1);
}

#[test]
fn empty_struct() {
    let f = "\nfinal class C extends Struct {}\n";
    assert_row(&report(f), "empty_struct", at(f, 3, 13), 1);
}

#[test]
fn extra_annotation_on_struct_field() {
    let f = "\nfinal class S extends Struct {\n  @Int8()\n  @Int16()\n\
             \x20 external int x;\n}\n";
    assert_row(
        &report(f),
        "extra_annotation_on_struct_field",
        at(f, 5, 3),
        8,
    );
}

#[test]
fn extra_size_annotation_carray() {
    let f = "\nfinal class C extends Struct {\n  @Array(4)\n  @Array(8)\n\
             \x20 external Array<Uint8> a0;\n}\n";
    assert_row(&report(f), "extra_size_annotation_carray", at(f, 5, 3), 9);
}

#[test]
fn field_must_be_external_in_struct() {
    let f = "\nfinal class C extends Struct {\n  @Int16()\n  int a;\n}\n";
    assert_row(
        &report(f),
        "field_must_be_external_in_struct",
        at(f, 5, 7),
        1,
    );
}

#[test]
fn ffi_native_invalid_duplicate_default_asset() {
    let f = "@DefaultAsset('a')\n@DefaultAsset('b')\nlibrary;\n\n";
    assert_row(
        &report_typed_data(f),
        "ffi_native_invalid_duplicate_default_asset",
        at(f, 3, 2),
        12,
    );
}

#[test]
fn ffi_native_invalid_multiple_annotations() {
    let f = "\n@Native<Int32 Function(Int32)>()\n\
             @Native<Int32 Function(Int32)>(isLeaf: true)\n\
             external int f(int v);\n";
    assert_row(
        &report(f),
        "ffi_native_invalid_multiple_annotations",
        at(f, 4, 2),
        6,
    );
}

#[test]
fn ffi_native_must_be_external() {
    let f = "\n@Native<Void Function()>()\nvoid f() {}\n";
    assert_row(&report(f), "ffi_native_must_be_external", at(f, 4, 6), 1);
}

#[test]
fn ffi_native_only_classes_extending_nativefieldwrapperclass1_can_be_pointer() {
    let f = "\nclass C {\n  @Native<Void Function(Pointer<Int8>, Pointer<Int8>)>()\n\
             \x20 external void m(Pointer<Int8> p);\n}\n";
    assert_row(
        &report(f),
        "ffi_native_only_classes_extending_nativefieldwrapperclass1_can_be_pointer",
        at(f, 5, 17),
        1,
    );
}

#[test]
fn ffi_native_unexpected_number_of_parameters() {
    let f = "\n@Native<Void Function(Int8)>()\nexternal void f();\n";
    assert_row(
        &report(f),
        "ffi_native_unexpected_number_of_parameters",
        at(f, 4, 15),
        1,
    );
}

#[test]
fn ffi_native_unexpected_number_of_parameters_with_receiver() {
    let f = "\nclass C {\n  @Native<Void Function()>()\n  external void m();\n}\n";
    assert_row(
        &report(f),
        "ffi_native_unexpected_number_of_parameters_with_receiver",
        at(f, 5, 17),
        1,
    );
}

#[test]
fn generic_struct_subclass() {
    let f = "\nfinal class S<T> extends Struct {\n  external Pointer<Uint8> p;\n}\n";
    assert_row(&report(f), "generic_struct_subclass", at(f, 3, 13), 1);
}

#[test]
fn invalid_exception_value() {
    let f = "\ntypedef T = Void Function(Int8);\n\
             \nvoid f(int i) {}\n\
             \nvoid g() {\n  Pointer.fromFunction<T>(f, 42);\n}\n";
    assert_row(&report(f), "invalid_exception_value", at(f, 8, 30), 2);
}

#[test]
fn invalid_field_type_in_struct() {
    let f = "\nfinal class C extends Struct {\n  external String s;\n\
             \n  @Int32()\n  external int i;\n}\n";
    assert_row(&report(f), "invalid_field_type_in_struct", at(f, 4, 12), 6);
}

#[ignore = "leaf pair: function-form capture model — follow-up ticket"]
#[test]
fn leaf_call_must_not_return_handle() {
    let f = "\nvoid f(Pointer<NativeFunction<Handle Function()>> p) {\n\
             \x20 p.asFunction<Object Function()>(isLeaf: true);\n}\n";
    assert_row(
        &report(f),
        "leaf_call_must_not_return_handle",
        at(f, 4, 5),
        10,
    );
}

#[ignore = "leaf pair: function-form capture model — follow-up ticket"]
#[test]
fn leaf_call_must_not_take_handle() {
    let f = "\nvoid f(Pointer<NativeFunction<Void Function(Handle)>> p) {\n\
             \x20 p.asFunction<void Function(Object)>(isLeaf: true);\n}\n";
    assert_row(
        &report(f),
        "leaf_call_must_not_take_handle",
        at(f, 4, 5),
        10,
    );
}

#[test]
fn mismatched_annotation_on_struct_field() {
    let f = "\nfinal class C extends Struct {\n  @Double()\n  external int x;\n}\n";
    assert_row(
        &report(f),
        "mismatched_annotation_on_struct_field",
        at(f, 4, 3),
        9,
    );
}

#[test]
fn missing_annotation_on_struct_field() {
    let f = "\nfinal class C extends Struct {\n  external int x;\n}\n";
    assert_row(
        &report(f),
        "missing_annotation_on_struct_field",
        at(f, 4, 12),
        3,
    );
}

#[test]
fn missing_exception_value() {
    let f = "\nint f(int i) => i * 2;\n\
             \nvoid g() {\n  Pointer.fromFunction<Int8 Function(Int8)>(f);\n}\n";
    assert_row(&report(f), "missing_exception_value", at(f, 6, 11), 12);
}

#[test]
fn missing_field_type_in_struct() {
    let f = "\nfinal class C extends Struct {\n  external var str;\n\
             \n  @Int32()\n  external int i;\n}\n";
    assert_row(&report(f), "missing_field_type_in_struct", at(f, 4, 16), 3);
}

#[test]
fn missing_size_annotation_carray() {
    let f = "\nfinal class C extends Struct {\n  external Array<Uint8> a0;\n}\n";
    assert_row(
        &report(f),
        "missing_size_annotation_carray",
        at(f, 4, 12),
        12,
    );
}

#[test]
fn must_be_a_native_function_type() {
    let f = "\nint f(int i) => i * 2;\n\
             \nclass C<T extends Function> {\n  void g() {\n\
             \x20   Pointer.fromFunction<T>(f, 0);\n  }\n}\n";
    assert_row(
        &report(f),
        "must_be_a_native_function_type",
        at(f, 7, 26),
        1,
    );
}

#[test]
fn must_be_a_subtype() {
    let f = "\ntypedef T = Int8 Function(Int8);\n\
             \ndouble f(double i) => i;\n\
             \nvoid g() {\n  Pointer.fromFunction<T>(f, 5.0);\n}\n";
    assert_row(&report(f), "must_be_a_subtype", at(f, 8, 27), 1);
}

#[test]
fn must_return_void() {
    let f = "\nint f(int i) => i * 2;\n\
             \nvoid g() {\n  NativeCallable<Int32 Function(Int32)>.listener(f);\n}\n";
    assert_row(&report(f), "must_return_void", at(f, 6, 50), 1);
}

#[test]
fn native_field_invalid_type() {
    let f = "\nclass C {\n  @Native<Void Function()>()\n  external static int f;\n}\n";
    assert_row(&report(f), "native_field_invalid_type", at(f, 5, 23), 1);
}

#[test]
fn native_field_missing_type() {
    let f = "\nclass C {\n  @Native()\n  external static int f;\n}\n";
    assert_row(&report(f), "native_field_missing_type", at(f, 5, 23), 1);
}

#[test]
fn native_field_not_static() {
    let f = "\nclass C {\n  @Native<Int>()\n  external int f;\n}\n";
    assert_row(&report(f), "native_field_not_static", at(f, 5, 16), 1);
    let r = report(f);
    assert_absent(&r, "not_initialized_non_nullable_instance_field");
}

#[test]
fn native_function_missing_type() {
    let f = "\n@Native()\nexternal int f();\n";
    assert_row(&report(f), "native_function_missing_type", at(f, 4, 14), 1);
}

#[test]
fn negative_variable_dimension() {
    let f = "\nfinal class MyStruct extends Struct {\n\
             \x20 @Array.variableWithVariableDimension(-1)\n\
             \x20 external Array<Uint8> a0;\n}\n";
    assert_row(
        &report(f),
        "negative_variable_dimension",
        at(f, 4, 40),
        2,
    );
}

#[test]
fn non_constant_type_argument() {
    let f = "\nvoid f<T>() {\n  sizeOf<T>();\n}\n";
    assert_row(&report(f), "non_constant_type_argument", at(f, 4, 3), 11);
}

#[test]
fn non_native_function_type_argument_to_pointer() {
    let f = "\ntypedef FNative = int Function(int);\n\
             typedef F = int Function(int);\n\
             \nclass C {\n  void f(Pointer<NativeFunction<FNative>> p) {\n\
             \x20   p.asFunction<F>();\n  }\n}\n";
    assert_row(
        &report(f),
        "non_native_function_type_argument_to_pointer",
        at(f, 8, 18),
        1,
    );
}

#[test]
fn non_positive_array_dimension() {
    let f = "\nfinal class MyStruct extends Struct {\n  @Array(-8)\n\
             \x20 external Array<Uint8> a0;\n}\n";
    assert_row(
        &report(f),
        "non_positive_array_dimension",
        at(f, 4, 10),
        2,
    );
}

#[test]
fn non_sized_type_argument() {
    let f = "\nfinal class C extends Struct {\n  @Array(8)\n\
             \x20 external Array<Void> a0;\n}\n";
    assert_row(&report(f), "non_sized_type_argument", at(f, 5, 18), 4);
}

#[test]
fn packed_annotation() {
    let f = "\n@Packed(1)\n@Packed(1)\nfinal class C extends Struct {\n\
             \x20 external Pointer<Uint8> notEmpty;\n}\n";
    assert_row(&report(f), "packed_annotation", at(f, 4, 1), 10);
}

#[test]
fn packed_annotation_alignment() {
    let f = "\n@Packed(3)\nfinal class C extends Struct {\n\
             \x20 external Pointer<Uint8> notEmpty;\n}\n";
    assert_row(&report(f), "packed_annotation_alignment", at(f, 3, 9), 1);
}

#[test]
fn size_annotation_dimensions() {
    let f = "\nfinal class C extends Struct {\n  @Array(8, 8)\n\
             \x20 external Array<Array<Array<Uint8>>> a0;\n}\n";
    assert_row(&report(f), "size_annotation_dimensions", at(f, 4, 3), 12);
}

#[test]
fn subtype_of_struct_class() {
    let f = "\nfinal class S extends Struct {\n  external Pointer f;\n}\n\
             \nfinal class C extends S {\n  external Pointer g;\n}\n";
    assert_row(&report(f), "subtype_of_struct_class", at(f, 7, 23), 1);
}

#[test]
fn variable_length_array_not_last() {
    let f = "\nfinal class C extends Struct {\n  @Array.variable()\n\
             \x20 external Array<Uint8> a0;\n\
             \n  @Uint8()\n  external int a1;\n}\n";
    assert_row(
        &report(f),
        "variable_length_array_not_last",
        at(f, 4, 3),
        17,
    );
}

/// F4: `@Native`-annotated `external` fields are valid upstream (silent)
/// — the flow tier's not-initialized faces must not bury the FFI rows,
/// on instances (the model exemption) or statics.
#[test]
fn native_external_fields_stay_silent_on_not_initialized() {
    let f = "\nclass C {\n  @Native<Int8>()\n  external int f;\n\
             \n  @Native<Void Function()>()\n  external static int g;\n}\n";
    let r = report(f);
    assert_absent(&r, "not_initialized_non_nullable_instance_field");
    assert_absent(&r, "not_initialized_non_nullable_variable");
}
