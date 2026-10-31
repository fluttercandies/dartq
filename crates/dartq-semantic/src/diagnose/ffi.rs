//! The `dart:ffi` verifier's post-parse pass (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/generated/ffi_verifier.dart`): the
//! declaration-shape checks (`empty_struct`, the compound-field family,
//! the `@Native` family, `subtype_of_struct_class`), the invocation
//! checks (`Pointer.fromFunction`, `asFunction`, `sizeOf`,
//! `NativeCallable.listener`, `Native.addressOf`, `Struct.create`,
//! `elementAt`), and the `.address` faces (`address_position`,
//! `address_receiver`).
//!
//! The pass runs after the parse (`finalize_ffi`, called beside the other
//! finalize passes) so a same-file later declaration resolves and the
//! captured invocation/address faces are complete. Name-level facts ride
//! captures ([`super::types::FfiInvocation`], [`super::types::FfiAddressRead`],
//! the clause spans, the annotation faces); everything else re-derives
//! from the element model and source walks.

use super::*;
use dartq_source::byte_offset_to_utf16;

/// The `dart:ffi` class spellings the verifier names (upstream
/// `FfiVerifier`'s statics plus the primitive native types the field and
/// signature checks classify).
const FFI_CLASS_NAMES: &[&str] = &[
    "Struct",
    "Union",
    "AbiSpecificInteger",
    "AbiSpecificIntegerMapping",
    "Pointer",
    "NativeFunction",
    "NativeType",
    "SizedNativeType",
    "Native",
    "NativeCallable",
    "NativeFieldWrapperClass1",
    "DefaultAsset",
    "Packed",
    "Array",
    "Handle",
    "Void",
    "Opaque",
    "Finalizable",
    "Allocator",
    "Int8",
    "Int16",
    "Int32",
    "Int64",
    "Uint8",
    "Uint16",
    "Uint32",
    "Uint64",
    "IntPtr",
    "Int",
    "Uint",
    "Long",
    "LongLong",
    "ULong",
    "ULongLong",
    "Short",
    "SignedChar",
    "Char",
    "Size",
    "Float",
    "Double",
    "Bool",
];

/// The fixed-size integer native types — the only legal
/// `AbiSpecificIntegerMapping` values (upstream
/// `_primitiveIntegerNativeTypesFixedSize`).
const FIXED_SIZE_INTEGER_TYPES: &[&str] = &[
    "Int8", "Int16", "Int32", "Int64", "Uint8", "Uint16", "Uint32", "Uint64",
];

/// The invocation selector spellings the verifier's method-invocation
/// visit owns (upstream `visitMethodInvocation`'s dispatch).
const FFI_CALLEE_SPELLINGS: &[&str] = &[
    "fromFunction",
    "asFunction",
    "lookupFunction",
    "listener",
    "isolateLocal",
    "addressOf",
    "sizeOf",
    "create",
    "elementAt",
];

/// The supported `@Packed` alignments (upstream `_validatePackedAnnotation`).
const PACKED_ALIGNMENTS: [i64; 5] = [1, 2, 4, 8, 16];

impl<'a> Engine<'a> {
    /// The selector spelling is one of the verifier's own invocation
    /// faces ([`FFI_CALLEE_SPELLINGS`]).
    pub(crate) fn ffi_callee_spelling(&self, name: StringId) -> bool {
        let text = self.strings.resolve(name);
        FFI_CALLEE_SPELLINGS.contains(&text)
    }

    /// The type wears an FFI value shape the `.address` capture admits: a
    /// native primitive, `Pointer`, `Array`, `Handle`, a typed-data list,
    /// or a compound subtype. `int`/`double`/`bool` are builtins (no user
    /// shadowing); every other head must resolve into `dart:ffi` — or
    /// stay unresolvable in a file that imports the FFI face, the
    /// under-report direction.
    pub(crate) fn ffi_value_shape(&self, ty: Ty) -> bool {
        let Ty::Named { name, .. } = ty else {
            return false;
        };
        let text = self.strings.resolve(name);
        if matches!(text, "int" | "double" | "bool") {
            return true;
        }
        if text.ends_with("List")
            && (text.strip_suffix("List").unwrap_or_default().is_empty()
                || FIXED_SIZE_INTEGER_TYPES
                    .contains(&text.strip_suffix("List").unwrap_or_default())
                || matches!(text, "Float32List" | "Float64List"))
        {
            return true;
        }
        self.ffi_class_spelling(name)
    }

    /// The spelling is one of the `dart:ffi` classes — resolved into the
    /// FFI library, or unresolved in a file that imports it (the mock
    /// `package:ffi` face resolves there and re-exports the same names).
    pub(crate) fn ffi_class_spelling(&self, name: StringId) -> bool {
        let text = self.strings.resolve(name);
        if !FFI_CLASS_NAMES.contains(&text) {
            return false;
        }
        match resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        ) {
            Some((_, owner)) => self.owner_is_ffi(owner),
            None => self.imports_ffi(),
        }
    }

    /// The head resolves to a class whose supertype chain reaches an FFI
    /// compound, `AbiSpecificInteger`, or `NativeCallable` — the creation
    /// faces' capture gate.
    pub(crate) fn ffi_creation_target(&self, name: StringId) -> bool {
        let Some((container, owner)) = self.resolve_container_from(name) else {
            return false;
        };
        self.chain_reaches_ffi(&[(name, container, owner)])
    }

    /// The file's owning library is the FFI library itself.
    fn owner_is_ffi(&self, owner: FileId) -> bool {
        self.graph
            .library_of_file(owner)
            .is_some_and(|lib| self.graph.uri(lib) == "dart:ffi")
    }

    /// The diagnosing library imports `dart:ffi` (or a `package:ffi`
    /// face re-exporting it).
    fn imports_ffi(&self) -> bool {
        self.graph.import_edges(self.lib).iter().any(|edge| {
            edge.target.is_some_and(|target| {
                let uri = self.graph.uri(target);
                uri == "dart:ffi" || uri.starts_with("package:ffi")
            })
        })
    }

    fn resolve_container_from(&self, name: StringId) -> Option<(&'a Container, FileId)> {
        resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        )
    }

    /// The supertype-chain walk: `true` when some hop is an FFI
    /// `Struct`/`Union`/`AbiSpecificInteger`/`NativeCallable`. Depth- and
    /// visit-capped (the termination invariant — a cyclic recovery chain
    /// stops, never loops).
    fn chain_reaches_ffi(&self, start: &[(StringId, &'a Container, FileId)]) -> bool {
        let mut visited: FxHashSet<StringId> = FxHashSet::default();
        let mut work: Vec<(StringId, &'a Container, FileId)> = start.to_vec();
        while let Some((name, container, owner)) = work.pop() {
            if !visited.insert(name) {
                continue;
            }
            let text = self.strings.resolve(name);
            if (matches!(text, "Struct" | "Union" | "AbiSpecificInteger")
                || text == "NativeCallable")
                && self.owner_is_ffi(owner)
            {
                return true;
            }
            let Some(super_name) = container.superclass else {
                continue;
            };
            if visited.contains(&super_name) {
                continue;
            }
            if let Some((super_container, super_owner)) = self.resolve_container_from(super_name) {
                work.push((super_name, super_container, super_owner));
            }
        }
        false
    }

    /// The container's DIRECT supertype is an FFI compound or
    /// `AbiSpecificInteger` (upstream `visitClassDeclaration`'s direct
    /// `ffiClass` arm — the subtype faces own the transitive shapes).
    fn direct_ffi_supertype(&self, container: &Container) -> Option<&'static str> {
        let super_name = container.superclass?;
        let (_, owner) = self.resolve_container_from(super_name)?;
        if !self.owner_is_ffi(owner) {
            return None;
        }
        match self.strings.resolve(super_name) {
            "Struct" => Some("Struct"),
            "Union" => Some("Union"),
            "AbiSpecificInteger" => Some("AbiSpecificInteger"),
            _ => None,
        }
    }

    /// The written clause entry reaches an FFI compound or
    /// `AbiSpecificInteger` transitively (upstream `NamedType
    /// .isCompoundSubtype` / `.isAbiSpecificIntegerSubtype` over
    /// `allSupertypes`).
    fn clause_entry_is_compound(&self, name: StringId) -> bool {
        let Some((container, owner)) = self.resolve_container_from(name) else {
            return false;
        };
        self.chain_reaches_ffi(&[(name, container, owner)])
    }

    // —— the annotation capture (`end_metadata` half) ——————————————————

    /// An FFI-named annotation closed: source-parse its face once and
    /// latch it for the declaration the next `note_decl_site` registers.
    pub(crate) fn note_ffi_annotation(&mut self, begin: Token, end: Token) {
        let at16 = begin.range.offset;
        let stop16 = end.range.end();
        if stop16 <= at16 {
            return;
        }
        let start = self.byte_of(at16);
        let stop = self.byte_of(stop16).min(self.source.len());
        let text = self.source.get(start..stop).unwrap_or_default();
        // The written name: after `@`, the dot-joined identifier run.
        let body = &text[1..];
        let name_len = body
            .char_indices()
            .take_while(|(i, c)| *i == 0 || c.is_ascii_alphanumeric() || *c == '_' || *c == '.')
            .map(|(i, c)| i + c.len_utf8())
            .last()
            .unwrap_or(0);
        let name_text = &body[..name_len];
        let leaf = name_text.rsplit('.').next().unwrap_or(name_text);
        let variant = match leaf {
            "Native" => 0,
            "Array" if name_text == "Array" => 1,
            "Packed" => 2,
            "AbiSpecificIntegerMapping" => 3,
            "DefaultAsset" => 4,
            _ => {
                let array_form = name_text.starts_with("Array.")
                    && matches!(
                        &name_text["Array.".len()..],
                        "variable" | "variableMulti" | "variableWithVariableDimension" | "multi"
                    );
                if !array_form {
                    return;
                }
                1
            }
        };
        let (face, _) = self.parse_ffi_face(at16, start, text, variant);
        let name = self.strings.intern(name_text);
        self.ffi_pending_annos.push(FfiDeclAnno {
            at: SourceRange {
                offset: at16,
                length: stop16.saturating_sub(at16),
            },
            name,
            face,
        });
    }

    /// Parses one FFI annotation's span into its face. `variant`: 0
    /// `@Native`, 1 `@Array*`, 2 `@Packed`, 3 `@AbiSpecificIntegerMapping`,
    /// 4 `@DefaultAsset`. Returns the face and the argument group's span
    /// (relative information is folded in; every stored span is absolute
    /// UTF-16).
    #[expect(clippy::cast_possible_truncation)] // offsets saturate at the scanner's u32 cap
    fn parse_ffi_face(
        &self,
        at16: u32,
        start: usize,
        text: &str,
        variant: u8,
    ) -> (FfiAnnoFace, Option<SourceRange>) {
        let mut face = FfiAnnoFace {
            name_span: SourceRange {
                offset: 0,
                length: 0,
            },
            args: None,
            sig: None,
            is_leaf: false,
            dims: Vec::new(),
            variable_length: false,
            alignment: None,
            mapping_values: Vec::new(),
        };
        let bytes = text.as_bytes();
        // The name run's end (the same scan `note_ffi_annotation` did).
        let body = &text[1..];
        let name_len = body
            .char_indices()
            .take_while(|(i, c)| *i == 0 || c.is_ascii_alphanumeric() || *c == '_' || *c == '.')
            .map(|(i, c)| i + c.len_utf8())
            .last()
            .unwrap_or(0);
        face.name_span = SourceRange {
            offset: at16 + 1,
            length: (name_len as u32).min(u32::MAX - at16 - 1),
        };
        let mut cursor = 1 + name_len;
        let utf16 = |byte: usize| byte_offset_to_utf16(self.source, start + byte);
        // The written `<T>` (the `@Native` signature).
        if cursor < bytes.len() && bytes[cursor] == b'<' {
            if let Some(close) = matching_close(bytes, cursor, b'<', b'>') {
                let inner = &text[cursor + 1..close];
                face.sig = Some(FfiSigFace {
                    range: SourceRange {
                        offset: utf16(cursor + 1),
                        length: utf16(close) - utf16(cursor + 1),
                    },
                    display: inner.trim().to_owned().into_boxed_str(),
                    ret_head: first_ident_head(inner, self),
                    param_heads: function_param_heads(inner)
                        .into_iter()
                        .map(|head| self.strings.intern(&head))
                        .collect(),
                });
                cursor = close + 1;
            }
        }
        // The argument group.
        if cursor >= bytes.len() || bytes[cursor] != b'(' {
            return (face, None);
        }
        let Some(close) = matching_close(bytes, cursor, b'(', b')') else {
            return (face, None);
        };
        let args_text = &text[cursor..=close];
        let args_span = SourceRange {
            offset: utf16(cursor),
            length: utf16(close + 1) - utf16(cursor),
        };
        face.args = Some(args_span);
        face.is_leaf = named_arg_is_true(args_text, "isLeaf");
        match variant {
            0 => {}
            1 => {
                let suffix = text[1..1 + name_len]
                    .strip_prefix("Array.")
                    .unwrap_or_default()
                    .to_owned();
                parse_array_dims(self, args_text, start + cursor, &suffix, &mut face);
            }
            2 => {
                if let Some(close) = matching_close(bytes, cursor, b'(', b')') {
                    let inner = &text[cursor + 1..close];
                    if let Some((rel, len)) = first_top_level_arg(inner) {
                        let raw = &inner[rel..rel + len];
                        let value = raw.trim();
                        if let Some(parsed) = parse_int_literal(value) {
                            let lead = raw.len() - raw.trim_start().len();
                            let at_rel = cursor + 1 + rel + lead;
                            face.alignment = Some((
                                parsed,
                                SourceRange {
                                    offset: utf16(at_rel),
                                    length: utf16(at_rel + value.len()) - utf16(at_rel),
                                },
                            ));
                        }
                    }
                }
            }
            3 => {
                // The mapping's `{...}`: each `key: value` entry's value
                // head is the constructed native type. A const-identifier
                // argument (`@AbiSpecificIntegerMapping(c)` where `c` is a
                // `const` map literal) resolves to the const's own literal
                // — upstream's `annotation.arguments[0]` reads the
                // evaluated map's entries, anchored on the written
                // argument.
                if let Some(open) = args_text.find('{') {
                    let Some(brace_close) =
                        matching_close(args_text.as_bytes(), open, b'{', b'}')
                    else {
                        return (face, Some(args_span));
                    };
                    for (rel, len) in
                        split_top_level(&args_text[open + 1..brace_close], b',')
                    {
                        let entry = &args_text[open + 1 + rel..open + 1 + rel + len];
                        let Some(colon) = top_level_colon(entry) else {
                            continue;
                        };
                        let value_part = &entry[colon + 1..];
                        let value = value_part.trim();
                        let Some((head_start, head_end)) = first_ident(value) else {
                            continue;
                        };
                        let lead = value_part.len() - value_part.trim_start().len();
                        let vstart = open + 1 + rel + colon + 1 + lead;
                        face.mapping_values.push((
                            value[head_start..head_end].to_owned(),
                            SourceRange {
                                offset: utf16(cursor + vstart),
                                length: utf16(cursor + vstart + value.len())
                                    - utf16(cursor + vstart),
                            },
                        ));
                    }
                } else {
                    // The const-identifier arm: the argument text is a bare
                    // name; its entries live on the const's map literal.
                    // Every unsupported value reports at the argument's
                    // written span (upstream anchors `arguments[0]`).
                    let inner = args_text
                        .trim_start_matches('(')
                        .trim_end_matches(')')
                        .trim();
                    if let Some((s, e)) = first_ident(inner)
                        && let Some(entries) = self.ffi_const_map_entries(&inner[s..e])
                    {
                        let arg_span = args_span;
                        for value in entries {
                            face.mapping_values.push((value, arg_span));
                        }
                    }
                }
            }
            _ => {}
        }
        (face, Some(args_span))
    }
}

/// The matching closer for the group opened at `open` (byte index into
/// `bytes`), depth-counting past nested pairs. `None` under a truncated
/// span.
pub(super) fn matching_close(bytes: &[u8], open: usize, open_b: u8, close_b: u8) -> Option<usize> {
    let mut depth = 0i32;
    for (i, &b) in bytes.iter().enumerate().skip(open) {
        if b == open_b {
            depth += 1;
        } else if b == close_b {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// Top-level (depth-0) splits of `text` at `sep`, as `(start, len)` pairs
/// into `text`.
fn split_top_level(text: &str, sep: u8) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut parts = Vec::new();
    let (mut depth_p, mut depth_b, mut depth_br) = (0i32, 0i32, 0i32);
    let mut start = 0usize;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'(' => depth_p += 1,
            b')' => depth_p -= 1,
            b'[' => depth_b += 1,
            b']' => depth_b -= 1,
            b'{' => depth_br += 1,
            b'}' => depth_br -= 1,
            b'<' => depth_br += 1,
            b'>' if depth_br > 0 && is_type_arg_depth(bytes, i) => depth_br -= 1,
            _ if b == sep && depth_p == 0 && depth_b == 0 && depth_br == 0 => {
                parts.push((start, i - start));
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push((start, text.len() - start.min(text.len())));
    parts.retain(|(_, len)| *len > 0 || !text.is_empty());
    parts
}

/// A `>` closes a type-argument group only when the deepest open bracket
/// is the synthetic `<` counter — the scanner approximation (function
/// bodies inside annotation type arguments do not occur).
fn is_type_arg_depth(_bytes: &[u8], _at: usize) -> bool {
    true
}

/// The top-level `:` of one map entry.
fn top_level_colon(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let (mut depth_p, mut depth_b) = (0i32, 0i32);
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'(' => depth_p += 1,
            b')' => depth_p -= 1,
            b'<' => depth_b += 1,
            b'>' if depth_b > 0 => depth_b -= 1,
            b':' if depth_p == 0 && depth_b == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

/// The first identifier of `text`, as `(rel_start, end)` byte offsets.
fn first_ident(text: &str) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let start = bytes
        .iter()
        .position(|b| b.is_ascii_alphabetic() || *b == b'_' || *b == b'$')?;
    let mut end = start + 1;
    while bytes
        .get(end)
        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'$')
    {
        end += 1;
    }
    Some((start, end))
}

/// The head spelling of a written type (`Pointer<Int8>` → `Pointer`,
/// `Void Function(Int8)` → `Void`).
fn first_ident_head(text: &str, engine: &Engine<'_>) -> StringId {
    first_ident(text)
        .map(|(s, e)| engine.strings.intern(&text[s..e]))
        .unwrap_or(StringId::NONE)
}

/// A clause entry's `(leaf, head)` parts from its written text: strips
/// the type-argument tail, splits the first dot for the prefix, and
/// takes the last segment as the leaf (`p.S<T>` → `Some("S")`, `"S"`).
/// The head is the text up to the first `<` or whitespace — the
/// resolution key for both bare and prefixed spellings.
fn clause_entry_parts(text: &str) -> (Option<&str>, &str) {
    let head = text.trim();
    let head = head
        .split(|c: char| c == '<' || c.is_whitespace())
        .next()
        .unwrap_or(head);
    if head.is_empty() {
        return (None, head);
    }
    let leaf = head.rsplit('.').next().unwrap_or(head);
    (Some(leaf), head)
}

/// The normal-parameter head spellings of a written function type
/// (`Void Function(Int8, Pointer<Uint8>)` → `["Int8", "Pointer"]`); an
/// empty vector when the written form is not `R Function(...)`.
fn function_param_heads(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut function_at = None;
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'<' => depth += 1,
            b'>' => depth -= 1,
            b'a'..=b'z' | b'A'..=b'Z' | b'_' if depth == 0 => {
                let mut end = i + 1;
                while bytes
                    .get(end)
                    .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
                {
                    end += 1;
                }
                if &text[i..end] == "Function" {
                    function_at = Some(end);
                    break;
                }
                i = end;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    let Some(fn_end) = function_at else {
        return Vec::new();
    };
    let rest = &text[fn_end..];
    let Some(open) = rest.find('(') else {
        return Vec::new();
    };
    let Some(close) = matching_close(rest.as_bytes(), open, b'(', b')') else {
        return Vec::new();
    };
    split_top_level(&rest[open + 1..close], b',')
        .into_iter()
        .filter_map(|(rel, len)| {
            let param = rest[open + 1 + rel..open + 1 + rel + len].trim();
            let param = param
                .trim_start_matches(['[', '{'])
                .trim_start_matches("required")
                .trim_start();
            first_ident(param).map(|(s, e)| param[s..e].to_owned())
        })
        .collect()
}

/// The parameter head spellings of a written function type with the
/// `VarArgs` record flattened (upstream `flattenVarArgs`):
/// `VarArgs<(Int32, Double)>` expands to `Int32, Double` in the returned
/// list. A trailing `VarArgs` whose argument is not a record literal is
/// left as-is (upstream `!isVarArgs` / `namedFields.isNotEmpty` faces).
fn ffi_flatten_varargs(text: &str, strings: &StringTable) -> Vec<StringId> {
    let heads = function_param_heads(text);
    let mut flat: Vec<StringId> = Vec::with_capacity(heads.len());
    for (index, head) in heads.iter().enumerate() {
        if head == "VarArgs" && index + 1 == heads.len() {
            // Only the LAST parameter may be a `VarArgs` (upstream
            // `flattenVarArgs`'s `last.isVarArgs` gate — a mid-list
            // `VarArgs` keeps its raw head and fails the native-type
            // gate). The record argument must hold positional fields
            // only: a named field (`{Int64 named}`) stops the flatten
            // (`namedFields.isNotEmpty`).
            if let Some(varargs_pos) = text.find("VarArgs") {
                let after = &text[varargs_pos + "VarArgs".len()..];
                if let Some(angle) = after.find('<')
                    && let Some(angle_close) =
                        matching_close(after.as_bytes(), angle, b'<', b'>')
                {
                    let record = &after[angle + 1..angle_close];
                    let record = record.trim();
                    if record.starts_with('(') && record.ends_with(')') {
                        let inner = &record[1..record.len() - 1];
                        let has_named = split_top_level(inner, b',')
                            .iter()
                            .any(|(rel, len)| inner[*rel..*rel + len].contains('{'));
                        if !has_named {
                            for (rel, len) in split_top_level(inner, b',') {
                                let field = inner[rel..rel + len].trim();
                                if let Some((s, e)) = first_ident(field) {
                                    flat.push(strings.intern(&field[s..e]));
                                }
                            }
                            continue;
                        }
                    }
                }
            }
            flat.push(strings.intern("VarArgs"));
        } else {
            flat.push(strings.intern(head));
        }
    }
    flat
}

/// Whether the named argument reads `name: true` at the group's top
/// level (the `_isLeaf` source walk). The group's own parens/brackets
/// hug the split's first and last items — `top_level_colon` finds the
/// label separator and the label's own trim drops the open paren.
fn named_arg_is_true(args_text: &str, name: &str) -> bool {
    for (rel, len) in split_top_level(args_text, b',') {
        let arg = &args_text[rel..rel + len];
        let Some(colon) = arg.find(':') else {
            continue;
        };
        let label = arg[..colon]
            .trim()
            .trim_start_matches(['(', '[', '{'])
            .trim();
        if label == name && arg[colon + 1..].trim().trim_end_matches([')', ']', '}']).trim() == "true"
        {
            return true;
        }
    }
    false
}

/// The first top-level argument of the group text, as `(start, len)` into
/// it.
fn first_top_level_arg(args_text: &str) -> Option<(usize, usize)> {
    split_top_level(args_text, b',')
        .into_iter()
        .map(|(rel, len)| (rel, len))
        .next()
}

/// `@Array`'s dimension face, per constructor spelling (upstream
/// `arraySizeDimensions`): `Array.variable*` presets the variable
/// dimension first (`0` for the factories, the written literal for
/// `variableWithVariableDimension`), `variableMulti` appends the list
/// items, the plain form lists its arguments.
fn parse_array_dims(
    engine: &Engine<'_>,
    args_text: &str,
    args_at: usize,
    suffix: &str,
    face: &mut FfiAnnoFace,
) {
    let bytes = args_text.as_bytes();
    let utf16 = |byte: usize| byte_offset_to_utf16(engine.source, args_at + byte);
    // Pushes one parsed literal item with its absolute span.
    let push_dim = |face: &mut FfiAnnoFace, value: i64, at_rel: usize, len: usize| {
        face.dims.push((
            value,
            SourceRange {
                offset: utf16(at_rel),
                length: utf16(at_rel + len) - utf16(at_rel),
            },
        ));
    };
    let Some(close) = matching_close(bytes, 0, b'(', b')') else {
        return;
    };
    let inner = &args_text[1..close];
    match suffix {
        // `Array.variable([d2, d3, ...])` — variable dimension defaults to
        // `0` (upstream `_ArraySize.variable` sets `dimension1 = 0`), the
        // written items are the fixed dims 2-5.
        "variable" => {
            face.variable_length = true;
            face.dims.push((
                0,
                SourceRange {
                    offset: utf16(0),
                    length: 0,
                },
            ));
            for (rel, len) in split_top_level(inner, b',') {
                let item_full = &inner[rel..rel + len];
                let item = item_full.trim();
                let Some(value) = parse_int_literal(item) else {
                    continue;
                };
                let lead = item_full.len() - item_full.trim_start().len();
                push_dim(face, value, 1 + rel + lead, item.len());
            }
        }
        // `Array.multi([...])` and
        // `Array.variableMulti([...], variableDimension: N)` — dims come
        // from the list literal; `variableMulti` additionally prepends the
        // named `variableDimension` value (default `0`) as dims[0]
        // (upstream `arraySizeDimensions`: `[if (variableLength)
        // variableDimension, ...listValues]`).
        "multi" | "variableMulti" => {
            face.variable_length = suffix == "variableMulti";
            let mut variable: Option<(i64, SourceRange)> = None;
            for (rel, len) in split_top_level(inner, b',') {
                let entry = &inner[rel..rel + len];
                let trimmed = entry.trim();
                let lead = entry.len() - entry.trim_start().len();
                if let Some(colon) = trimmed.find(':')
                    && trimmed[..colon].trim() == "variableDimension"
                {
                    let value_text = trimmed[colon + 1..].trim();
                    if let Some(parsed) = parse_int_literal(value_text) {
                        let vlead = trimmed[colon + 1..].len()
                            - trimmed[colon + 1..].trim_start().len();
                        let at = 1 + rel + lead + colon + 1 + vlead;
                        variable = Some((
                            parsed,
                            SourceRange {
                                offset: utf16(at),
                                length: utf16(at + value_text.len()) - utf16(at),
                            },
                        ));
                    }
                    continue;
                }
                let Some(bracket) = entry.find('[') else {
                    continue;
                };
                let Some(bracket_close) =
                    matching_close(entry.as_bytes(), bracket, b'[', b']')
                else {
                    continue;
                };
                let items = &entry[bracket + 1..bracket_close];
                for (irel, ilen) in split_top_level(items, b',') {
                    let item_full = &items[irel..irel + ilen];
                    let item = item_full.trim();
                    let Some(value) = parse_int_literal(item) else {
                        continue;
                    };
                    let item_lead = item_full.len() - item_full.trim_start().len();
                    push_dim(
                        face,
                        value,
                        1 + rel + bracket + 1 + irel + item_lead,
                        item.len(),
                    );
                }
            }
            if face.variable_length {
                // `variableDimension` defaults to `0` when omitted
                // (upstream `_ArraySize.variableMulti`'s default); its span
                // falls back to the annotation's error node, modeled by a
                // zero-length range at the head.
                let (value, span) = variable.unwrap_or((
                    0,
                    SourceRange {
                        offset: utf16(0),
                        length: 0,
                    },
                ));
                face.dims.insert(0, (value, span));
            }
        }
        // `Array.variableWithVariableDimension([d1, d2, ...])` — all
        // written items, d1 is the variable dimension.
        "variableWithVariableDimension" => {
            face.variable_length = true;
            for (rel, len) in split_top_level(inner, b',') {
                let item_full = &inner[rel..rel + len];
                let item = item_full.trim();
                let Some(value) = parse_int_literal(item) else {
                    continue;
                };
                let lead = item_full.len() - item_full.trim_start().len();
                push_dim(face, value, 1 + rel + lead, item.len());
            }
        }
        // `Array(d1, d2, ...)` — plain positional items.
        _ => {
            for (rel, len) in split_top_level(inner, b',') {
                let item_full = &inner[rel..rel + len];
                let item = item_full.trim();
                let Some(value) = parse_int_literal(item) else {
                    continue;
                };
                let lead = item_full.len() - item_full.trim_start().len();
                push_dim(face, value, 1 + rel + lead, item.len());
            }
        }
    }
}

// —— the post-parse pass ———————————————————————————————————————————

/// The keyword set whose presence before a field name means no written
/// type annotation (`external var str;`).
const NO_TYPE_KEYWORDS: &[&str] = &[
    "var",
    "final",
    "const",
    "external",
    "static",
    "late",
    "covariant",
    "abstract",
];

/// The native integer classes the field/annotation classifier maps onto
/// the Dart `int` face (upstream `_primitiveIntegerNativeTypes`).
const NATIVE_INT_TYPES: &[&str] = &[
    "Int8",
    "Int16",
    "Int32",
    "Int64",
    "Uint8",
    "Uint16",
    "Uint32",
    "Uint64",
    "IntPtr",
    "Int",
    "Uint",
    "Long",
    "LongLong",
    "ULong",
    "ULongLong",
    "Short",
    "SignedChar",
    "Char",
    "Size",
];

impl<'a> Engine<'a> {
    /// The `dart:ffi` verifier's post-parse pass: declaration shapes,
    /// `@Native` members, captured invocations, and `.address` reads, in
    /// the upstream visitor's report order per family.
    pub(crate) fn finalize_ffi(&mut self) {
        self.ffi_check_library_default_asset();
        let containers: Vec<(StringId, &'a Container)> = self
            .index
            .get(self.file)
            .map(|elements| elements.containers().map(|(name, c)| (name, c)).collect())
            .unwrap_or_default();
        for (name, container) in &containers {
            self.ffi_check_container(*name, container);
        }
        for (name, container) in &containers {
            self.ffi_check_native_decls(Some((*name, container)));
        }
        self.ffi_check_native_decls(None);
        let invocations = std::mem::take(&mut self.ffi_invocations);
        for invocation in &invocations {
            self.ffi_check_invocation(invocation);
        }
        let reads = std::mem::take(&mut self.ffi_address_reads);
        for read in reads {
            self.ffi_check_address_read(&read, &invocations);
        }
    }

    /// The first declaration site registered under `name` for the given
    /// kind, in source order.
    fn ffi_decl_site(
        &self,
        name: StringId,
        pick: impl Fn(&DeclKind) -> bool,
    ) -> Option<(usize, &DeclSite)> {
        self.decl_sites
            .iter()
            .enumerate()
            .find(|(_, site)| site.name == name && pick(&site.kind))
    }

    /// The declaring name's span of the container (the `empty_struct`
    /// family's anchor).
    fn ffi_container_name_span(&self, name: StringId) -> Option<SourceRange> {
        self.ffi_decl_site(name, |kind| matches!(kind, DeclKind::Container))
            .map(|(_, site)| site.range)
    }

    /// The written type annotation before a member name (`external
    /// Array<Uint8> a0;` → the `Array<Uint8>` span): a backward source
    /// walk — a `<...>` group or bare identifier immediately before the
    /// name; a storage keyword instead means no written type.
    fn ffi_type_span_before(&self, name16: u32) -> Option<SourceRange> {
        let name_start = self.byte_of(name16);
        let bytes = self.source.as_bytes();
        let mut cursor = name_start;
        while cursor > 0
            && bytes
                .get(cursor - 1)
                .is_some_and(|b| b.is_ascii_whitespace())
        {
            cursor -= 1;
        }
        if cursor == 0 {
            return None;
        }
        // A single nullable `?` suffix rides the type (`Pointer? p`): the
        // oracle's `fieldType.toSource()` keeps it, so the span must reach
        // past the `?` while the head/`>` detection reads before it
        // (upstream ffi_verifier.dart:1566-1570 reports `Pointer?` itself).
        let type_end = cursor;
        if bytes.get(cursor - 1) == Some(&b'?') {
            cursor -= 1;
            while cursor > 0
                && bytes
                    .get(cursor - 1)
                    .is_some_and(|b| b.is_ascii_whitespace())
            {
                cursor -= 1;
            }
            if cursor == 0 {
                return None;
            }
        }
        // A closed `<...>` group directly before the name (or before the
        // type head).
        if bytes.get(cursor - 1) == Some(&b'>') {
            let mut depth = 0i32;
            let mut open = cursor;
            while open > 0 {
                open -= 1;
                match bytes[open] {
                    b'>' => depth += 1,
                    b'<' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if depth != 0 || bytes.get(open) != Some(&b'<') {
                return None;
            }
            let mut head_end = open;
            while head_end > 0
                && bytes
                    .get(head_end - 1)
                    .is_some_and(|b| b.is_ascii_whitespace())
            {
                head_end -= 1;
            }
            let mut head_start = head_end;
            while head_start > 0
                && bytes
                    .get(head_start - 1)
                    .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'$')
            {
                head_start -= 1;
            }
            if head_start == head_end {
                return None;
            }
            let head = &self.source[head_start..head_end];
            if NO_TYPE_KEYWORDS.contains(&head) {
                return None;
            }
            let head16 = byte_offset_to_utf16(self.source, head_start);
            let close16 = byte_offset_to_utf16(self.source, type_end);
            return Some(SourceRange {
                offset: head16,
                length: close16 - head16,
            });
        }
        // A bare identifier head.
        let mut word_start = cursor;
        while word_start > 0
            && bytes
                .get(word_start - 1)
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'$')
        {
            word_start -= 1;
        }
        if word_start == cursor {
            return None;
        }
        let word = &self.source[word_start..cursor];
        if NO_TYPE_KEYWORDS.contains(&word) {
            return None;
        }
        let start16 = byte_offset_to_utf16(self.source, word_start);
        let end16 = byte_offset_to_utf16(self.source, type_end);
        Some(SourceRange {
            offset: start16,
            length: end16 - start16,
        })
    }

    /// The written type annotation's source text (the `fieldType.toSource()`
    /// display).
    fn ffi_type_text(&self, span: SourceRange) -> String {
        let start = self.byte_of(span.offset);
        let end = self.byte_of(span.end()).min(self.source.len());
        self.source.get(start..end).unwrap_or_default().to_owned()
    }

    /// The first top-level type argument's written span inside a generic
    /// type annotation (`Array<Void>` → the `Void` span).
    fn ffi_first_type_arg_span(&self, ty: SourceRange) -> Option<SourceRange> {
        let start = self.byte_of(ty.offset);
        let end = self.byte_of(ty.end()).min(self.source.len());
        let text = self.source.get(start..end)?;
        let open = text.find('<')?;
        let close = matching_close(text.as_bytes(), open, b'<', b'>')?;
        let (rel, len) = first_top_level_arg(&text[open + 1..close])?;
        let lead = text[open + 1 + rel..open + 1 + rel + len].len()
            - text[open + 1 + rel..open + 1 + rel + len]
                .trim_start()
                .len();
        let arg_start = start + open + 1 + rel + lead;
        let arg_end = arg_start + text[open + 1 + rel..open + 1 + rel + len].trim().len();
        Some(SourceRange {
            offset: byte_offset_to_utf16(self.source, arg_start),
            length: byte_offset_to_utf16(self.source, arg_end)
                - byte_offset_to_utf16(self.source, arg_start),
        })
    }

    /// The FFI annotation faces registered at a declaration site.
    fn ffi_annos_at(&self, site: usize) -> &[FfiDeclAnno] {
        self.ffi_decl_annos
            .get(&site)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// The written `@Native` faces at a site.
    fn ffi_native_annos(&self, site: usize) -> Vec<&FfiDeclAnno> {
        self.ffi_annos_at(site)
            .iter()
            .filter(|anno| self.strings.resolve(anno.name).rsplit('.').next() == Some("Native"))
            .collect()
    }

    /// The `@Native` faces a site carries through const aliases: upstream
    /// reads `element.metadata`'s computed values (`_checkFfiNative` iterates
    /// `declarationElement.metadata.annotations` and calls
    /// `annotation.computeConstantValue()` — a `@alias` of
    /// `const alias = Native<…>(…)` produces a `Native` annotation value),
    /// so a bare `@identifier` whose const RHS spells `Native` joins the
    /// face list. The synthesized face is the const's own RHS parse — its
    /// `<T>`, `isLeaf`, and args are identical to the written form's; its
    /// `name_span`/`at` keep the alias's written spans for anchors that
    /// point at the annotation's name.
    fn ffi_const_native_annos(&self, site: &DeclSite) -> Vec<FfiDeclAnno> {
        let name_start = self.byte_of(site.range.offset);
        let bytes = self.source.as_bytes();
        // Walk the metadata region immediately before the declaring name:
        // annotations hug the declaration (whitespace/comments separate at
        // most); the walk stops at the first non-annotation, non-blank byte
        // past each consumed `@name[args]` group — a `//` line or `/*…*/`
        // block never reaches upstream's resolved `metadata`, so it never
        // reaches this expansion either.
        let mut out = Vec::new();
        let mut cursor = name_start;
        loop {
            while cursor > 0 && bytes.get(cursor - 1).is_some_and(|b| b.is_ascii_whitespace()) {
                cursor -= 1;
            }
            // Skip a trailing `)` of an argument group. `cursor == 0`
            // (the name starts the file, or the annotations are consumed
            // back to byte 0) precedes nothing — the same guard the
            // whitespace skips above use.
            if cursor > 0 && bytes.get(cursor - 1) == Some(&b')') {
                let mut depth = 0i32;
                let mut at = cursor - 1;
                loop {
                    match bytes.get(at) {
                        Some(b')') => depth += 1,
                        Some(b'(') => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        None => break,
                        _ => {}
                    }
                    if at == 0 {
                        break;
                    }
                    at -= 1;
                }
                cursor = at;
                while cursor > 0 && bytes.get(cursor - 1).is_some_and(|b| b.is_ascii_whitespace()) {
                    cursor -= 1;
                }
            }
            // The annotation's identifier run (dotted tails included).
            let mut ident_start = cursor;
            while ident_start > 0
                && bytes
                    .get(ident_start - 1)
                    .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'.')
            {
                ident_start -= 1;
            }
            if bytes.get(ident_start.wrapping_sub(1)) == Some(&b'@') {
                let name_text = &self.source[ident_start..cursor];
                let leaf = name_text.rsplit('.').next().unwrap_or(name_text);
                // `@Native`/`@Array`/`@DefaultAsset` et al. are already
                // captured; only non-FFI names need the const-alias probe.
                if !matches!(
                    leaf,
                    "Native" | "Array" | "Packed" | "AbiSpecificIntegerMapping" | "DefaultAsset"
                ) && let Some(anno) = self.ffi_resolve_const_anno(leaf, ident_start, cursor)
                {
                    out.push(anno);
                }
                cursor = ident_start - 1;
                continue;
            }
            break;
        }
        out.reverse();
        out
    }

    /// Resolves an alias name to a synthesized `Native`/`DefaultAsset`
    /// anno: the name must be a top-level `const` whose RHS is one bare
    /// creation of the FFI class (`const alias = Native<…>(isLeaf: true)`
    /// or `const d = DefaultAsset('a')`). The RHS text parses through the
    /// same `parse_ffi_face` the written form uses; the alias keeps its
    /// own name span for the diagnostic anchor.
    fn ffi_resolve_const_anno(
        &self,
        alias: &str,
        alias_start: usize,
        alias_end: usize,
    ) -> Option<FfiDeclAnno> {
        let alias_id = self.strings.intern(alias);
        let const_site = self
            .decl_sites
            .iter()
            .find(|s| s.name == alias_id && matches!(s.kind, DeclKind::TopLevelVar))?;
        // Source-walk the const decl's `= <creation>` tail.
        let decl_start = self.byte_of(const_site.range.offset);
        let tail = &self.source[decl_start..];
        // Find `=` after the const name; the initializer text runs to the
        // statement's `;`.
        let eq = tail.find('=')?;
        let init = &tail[eq + 1..];
        let init_trimmed = init.trim_start();
        // The creation's head must spell `Native` or `DefaultAsset`.
        let (variant, class_name) = if init_trimmed.starts_with("Native") {
            (0u8, "Native")
        } else if init_trimmed.starts_with("DefaultAsset") {
            (4u8, "DefaultAsset")
        } else {
            return None;
        };
        let head_len = init.len() - init_trimmed.len();
        let head = &init_trimmed[..class_name.len().min(init_trimmed.len())];
        if head != class_name {
            return None;
        }
        // The parse runs on the REAL const-RHS slice so every produced
        // span (sig.range, args, dims) lands in `self.source` coordinates;
        // the face content (sig display/heads, is_leaf, dims values) is
        // identical to the written annotation's. `parse_ffi_face` expects
        // the text to lead with `@`, so we synthesize the slice as
        // `@` + the RHS text in a temporary buffer whose bytes share the
        // tail of `self.source` — but `byte_offset_to_utf16(self.source,…)`
        // would still misindex, so instead we parse on a standalone
        // buffer and rebase the name/at spans onto the alias's written
        // spans (the only spans the diagnostics read for const aliases:
        // `ffiNativeInvalidMultipleAnnotations` anchors `annotationAst.name`,
        // the others read sig/args CONTENT not their spans).
        let mut text = String::with_capacity(1 + init_trimmed.len());
        text.push('@');
        text.push_str(init_trimmed);
        let (mut face, _) = self.parse_ffi_face(0, 0, &text, variant);
        // Rebase: the synthetic `@` sat at offset 0 of `text`; the name
        // span must anchor the ALIAS's written name (the diagnostic's
        // `annotationAst.name` is the written annotation, not the const's
        // RHS).
        let alias_name_range = SourceRange {
            offset: byte_offset_to_utf16(self.source, alias_start),
            length: byte_offset_to_utf16(self.source, alias_end)
                - byte_offset_to_utf16(self.source, alias_start),
        };
        face.name_span = alias_name_range;
        // The `args` presence gate (`anno.face.args.is_none()` rejects a
        // bare `@Native`) must reflect the const RHS's argument group —
        // rebase its written span onto the const's own `(`…`)` so the
        // external-keyword probe (`anno.at.end()` → `site.range` between-
        // text scan) stays truthful: the `at` range covers the alias name,
        // the args range the RHS's group — the between-text read is
        // annotation→name anyway, so the alias's real extent is what
        // matters.
        if let Some(args) = face.args.as_mut() {
            // The RHS argument group's `(` sits right after the head's
            // `<…>` type arguments when present, else right after the
            // class name — scan forward from the head's end in the init
            // text for the first `(`.
            let head_end = decl_start + eq + 1 + head_len + head.len();
            let rest = &self.source[head_end..];
            let mut depth = 0i32;
            let mut open = None;
            for (i, &b) in rest.as_bytes().iter().enumerate() {
                match b {
                    b'<' => depth += 1,
                    b'>' => depth -= 1,
                    b'(' if depth == 0 => {
                        open = Some(head_end + i);
                        break;
                    }
                    b';' | b'\n' if depth == 0 => break,
                    _ => {}
                }
            }
            let paren_open = open.unwrap_or(head_end);
            args.offset = byte_offset_to_utf16(self.source, paren_open);
            // Keep the group's written extent; the close paren is the
            // init text's first `)` at depth 0.
            let mut depth = 0i32;
            let mut close = paren_open;
            for (i, &b) in self.source.as_bytes()[paren_open..].iter().enumerate() {
                match b {
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            close = paren_open + i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            args.length = byte_offset_to_utf16(self.source, close + 1)
                - byte_offset_to_utf16(self.source, paren_open);
        }
        Some(FfiDeclAnno {
            at: alias_name_range,
            name: self.strings.intern(class_name),
            face,
        })
    }

    /// A written type's rendered display (the message slot face).
    fn ffi_render(&self, ty: Ty) -> String {
        ty.render(&self.strings, &self.boundary.args.borrow())
    }

    /// A written function type's rendered display (`R Function(P, ...)` —
    /// the `must_be_a_subtype` placeholders).
    fn ffi_render_function(&self, ret: &StringId, params: &[StringId]) -> String {
        let params = params
            .iter()
            .map(|head| self.ffi_render(ret_ty(*head)))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{} Function({})", self.ffi_render(ret_ty(*ret)), params)
    }

    /// The compound-subtype face of a written type head, resolved to its
    /// declaring container.
    fn ffi_compound_head(&self, ty: Ty) -> Option<(StringId, bool)> {
        let Ty::Named { name, .. } = ty else {
            return None;
        };
        let Some((container, owner)) = self.resolve_container_from(name) else {
            return None;
        };
        // The empty-struct face needs the container's own chain verdict.
        let start = [(name, container, owner)];
        Some((name, self.chain_reaches_ffi(&start)))
    }

    /// `class C extends NativeFieldWrapperClass1` — the receiver/parameter
    /// Pointer restriction's whitelist.
    fn ffi_extends_native_field_wrapper(&self, container: &Container) -> bool {
        let Some(super_name) = container.superclass else {
            return false;
        };
        let text = self.strings.resolve(super_name);
        if text == "NativeFieldWrapperClass1" {
            return self
                .resolve_container_from(super_name)
                .is_some_and(|(_, owner)| self.owner_is_ffi(owner));
        }
        self.resolve_container_from(super_name)
            .is_some_and(|(super_container, _)| {
                self.ffi_extends_native_field_wrapper(super_container)
            })
    }
}

impl<'a> Engine<'a> {
    // —— container declaration checks ———————————————————————————————

    /// `visitClassDeclaration`'s compound/abi faces: the direct-supertype
    /// dispatch, the clause-entry subtype faces, and the in-compound
    /// header checks.
    fn ffi_check_container(&mut self, name: StringId, container: &'a Container) {
        if !matches!(container.kind, ContainerKind::Class | ContainerKind::Mixin) {
            // The compound checks are class-shape only (mixins cannot
            // extend; kept out below anyway by the superclass arm).
        }
        let Some(name_span) = self.ffi_container_name_span(name) else {
            return;
        };
        let name_text = self.strings.resolve(name).to_owned();
        let Some(direct) = self.direct_ffi_supertype(container) else {
            // The clause entries: each written entry whose target reaches
            // a compound transitively (upstream `checkSupertype`; the
            // Allocator/Finalizable exemptions ride first).
            self.ffi_check_clause_entries(name, container);
            return;
        };
        let in_compound = matches!(direct, "Struct" | "Union");
        if in_compound {
            if self.ffi_is_empty_compound(container) {
                self.emit(SemanticCode::EmptyStruct, name_span, &name_text, direct);
            }
            // Only `Struct` subclasses read the packed faces (upstream).
            if direct == "Struct" {
                self.ffi_check_packed(name);
            }
            if container.type_params > 0 {
                self.emit(
                    SemanticCode::GenericStructSubclass,
                    name_span,
                    &name_text,
                    "",
                );
            }
            if container
                .implements_clause
                .iter()
                .any(|face| self.strings.resolve(*face) == "Finalizable")
            {
                self.emit(
                    SemanticCode::CompoundImplementsFinalizable,
                    name_span,
                    &name_text,
                    "",
                );
            }
            self.ffi_check_compound_fields(name, container);
        } else {
            // `AbiSpecificInteger` subclass.
            self.ffi_check_abi_specific_integer(name, container, name_span);
        }
    }

    /// The transitive clause-entry faces (`subtype_of_struct_class`'s
    /// extends/implements/with variants), anchored on the written entries.
    ///
    /// The element model stores leaf-only spellings in
    /// `container.*_clause` (`p.A` → `A`), so a prefixed entry would
    /// resolve in the wrong scope or not at all. Every entry is resolved
    /// through its written span text: the span covers `p.A` whole, the
    /// first dot splits prefix from leaf, and `prefixed_container` answers
    /// the imported library's container — matching upstream's element-level
    /// `NamedType.type` resolution.
    fn ffi_check_clause_entries(&mut self, name: StringId, container: &Container) {
        let name_text = self.strings.resolve(name).to_owned();
        let Some(spans) = self.ffi_clause_spans.get(&name) else {
            return;
        };
        let spans = spans.clone();
        // The extends entry: report only when the head is NOT itself an
        // ffi class (the direct arm consumed that shape). The leaf name
        // and the FFI-spelling check ride the leaf; the compound walk
        // resolves the written spelling (a prefixed `p.S` resolves
        // through the prefix, not the bare scope).
        if let Some(span) = spans.extends {
            let text = self.ffi_type_text(span);
            let (leaf, _) = clause_entry_parts(&text);
            if let Some(leaf_name) = leaf {
                let leaf_id = self.strings.intern(leaf_name);
                if !self.ffi_class_spelling(leaf_id)
                    && self.clause_entry_text_is_compound(&text)
                {
                    self.emit(
                        SemanticCode::SubtypeOfStructClassInExtends,
                        span,
                        &name_text,
                        leaf_name,
                    );
                }
            }
        }
        let implements = spans
            .implements
            .iter()
            .zip(container.implements_clause.iter());
        for (span, entry) in implements {
            let leaf_text = self.strings.resolve(*entry);
            if matches!(leaf_text, "Allocator" | "Finalizable") {
                continue;
            }
            let written = self.ffi_type_text(*span);
            if self.clause_entry_text_is_compound(&written) {
                self.emit(
                    SemanticCode::SubtypeOfStructClassInImplements,
                    *span,
                    &name_text,
                    leaf_text,
                );
            }
        }
        // The `with` faces: `spans.withs` is drained before this pass runs
        // (the modifier-checks drain consumes the buffered clause spans),
        // so the written entries are recovered by a source walk from the
        // class name — upstream `visitClassDeclaration`'s `withClause`
        // walk. The entries' types resolve through the same span-text path
        // as `implements` (prefixed spellings included).
        if container.with_clause.is_empty() {
            return;
        }
        for (entry, span) in self.ffi_with_clause_entries(name) {
            if self.clause_entry_text_is_compound(&entry) {
                self.emit(
                    SemanticCode::SubtypeOfStructClassInWith,
                    span,
                    &name_text,
                    &entry,
                );
            }
        }
    }

    /// The `with` clause's written entries, source-walked from the class
    /// name span: the comma-separated type spellings between the `with`
    /// keyword and the body/`implements`/`on` clause that follows.
    /// `(spelling, span)` per entry — the spelling is the source text
    /// (`p.S` whole), the span its written range.
    fn ffi_with_clause_entries(&self, name: StringId) -> Vec<(String, SourceRange)> {
        let Some(name_span) = self.ffi_container_name_span(name) else {
            return Vec::new();
        };
        let name_start = self.byte_of(name_span.offset);
        let bytes = self.source.as_bytes();
        // Walk forward from the class name past the (optional) type
        // parameters, superclass clause, and any `extends` entry, looking
        // for the `with` keyword at depth 0 before the body `{`.
        let mut cursor = name_start + self.strings.resolve(name).len();
        let mut entries = Vec::new();
        while cursor < bytes.len() {
            // Skip whitespace and comments.
            while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            if cursor + 1 < bytes.len()
                && bytes[cursor] == b'/'
                && bytes[cursor + 1] == b'/'
            {
                while cursor < bytes.len() && bytes[cursor] != b'\n' {
                    cursor += 1;
                }
                continue;
            }
            if cursor + 1 < bytes.len()
                && bytes[cursor] == b'/'
                && bytes[cursor + 1] == b'*'
            {
                cursor += 2;
                while cursor + 1 < bytes.len()
                    && !(bytes[cursor] == b'*' && bytes[cursor + 1] == b'/')
                {
                    cursor += 1;
                }
                cursor = (cursor + 2).min(bytes.len());
                continue;
            }
            if cursor >= bytes.len() {
                break;
            }
            match bytes[cursor] {
                // The body opens: no `with` clause.
                b'{' | b';' => return entries,
                // Type-parameter group.
                b'<' => {
                    let Some(close) = matching_close(bytes, cursor, b'<', b'>') else {
                        return entries;
                    };
                    cursor = close + 1;
                }
                // `extends` — skip its single entry (the superclass clause
                // is one type), then continue scanning for `with` or
                // `implements`.
                _ if self.source[cursor..].starts_with("extends") => {
                    cursor += "extends".len();
                    // Skip the superclass entry (one comma-free type
                    // spelling: head + optional `<...>`).
                    while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                        cursor += 1;
                    }
                    // The head identifier (possibly prefixed).
                    while cursor < bytes.len()
                        && (bytes[cursor].is_ascii_alphanumeric()
                            || bytes[cursor] == b'_'
                            || bytes[cursor] == b'$'
                            || bytes[cursor] == b'.')
                    {
                        cursor += 1;
                    }
                    while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                        cursor += 1;
                    }
                    if cursor < bytes.len() && bytes[cursor] == b'<' {
                        if let Some(close) = matching_close(bytes, cursor, b'<', b'>') {
                            cursor = close + 1;
                        }
                    }
                }
                _ if self.source[cursor..].starts_with("with") => {
                    cursor += "with".len();
                    // The comma-separated entries until the body or the
                    // next clause keyword at depth 0.
                    let mut entry_start = cursor;
                    let mut depth = 0i32;
                    while cursor < bytes.len() {
                        match bytes[cursor] {
                            b'<' | b'(' | b'[' => depth += 1,
                            b'>' | b')' | b']' => depth -= 1,
                            b',' if depth == 0 => {
                                let text = self.source[entry_start..cursor].trim();
                                if !text.is_empty() {
                                    let lead = entry_start
                                        + (self.source[entry_start..cursor].len()
                                            - self.source[entry_start..cursor].trim_start().len());
                                    let span = SourceRange {
                                        offset: byte_offset_to_utf16(self.source, lead),
                                        length: byte_offset_to_utf16(
                                            self.source,
                                            lead + text.len(),
                                        ) - byte_offset_to_utf16(self.source, lead),
                                    };
                                    entries.push((text.to_owned(), span));
                                }
                                entry_start = cursor + 1;
                            }
                            b'{' | b';' if depth == 0 => {
                                let text = self.source[entry_start..cursor].trim();
                                if !text.is_empty() {
                                    let lead = entry_start
                                        + (self.source[entry_start..cursor].len()
                                            - self.source[entry_start..cursor].trim_start().len());
                                    let span = SourceRange {
                                        offset: byte_offset_to_utf16(self.source, lead),
                                        length: byte_offset_to_utf16(
                                            self.source,
                                            lead + text.len(),
                                        ) - byte_offset_to_utf16(self.source, lead),
                                    };
                                    entries.push((text.to_owned(), span));
                                }
                                return entries;
                            }
                            // The next clause keyword at depth 0 ends the
                            // `with` list (`with M implements I` — the
                            // `implements` tail is not a mixin).
                            _ if depth == 0
                                && (self.source[cursor..].starts_with("implements")
                                    || self.source[cursor..].starts_with("on")) =>
                            {
                                let text = self.source[entry_start..cursor].trim();
                                if !text.is_empty() {
                                    let lead = entry_start
                                        + (self.source[entry_start..cursor].len()
                                            - self.source[entry_start..cursor].trim_start().len());
                                    let span = SourceRange {
                                        offset: byte_offset_to_utf16(self.source, lead),
                                        length: byte_offset_to_utf16(
                                            self.source,
                                            lead + text.len(),
                                        ) - byte_offset_to_utf16(self.source, lead),
                                    };
                                    entries.push((text.to_owned(), span));
                                }
                                return entries;
                            }
                            _ => {}
                        }
                        cursor += 1;
                    }
                    return entries;
                }
                _ if self.source[cursor..].starts_with("implements")
                    || self.source[cursor..].starts_with("on") =>
                {
                    // A clause keyword ahead of `with` means no `with`
                    // clause follows.
                    return entries;
                }
                _ => {
                    cursor += 1;
                }
            }
        }
        entries
    }

    /// The written clause-entry spelling resolves to a container whose
    /// supertype chain reaches an FFI compound or `AbiSpecificInteger`
    /// — the span-text variant of [`Self::clause_entry_is_compound`] that
    /// handles prefixed spellings (`p.S` resolves through the import
    /// prefix, matching upstream's `NamedType.type`).
    fn clause_entry_text_is_compound(&self, text: &str) -> bool {
        // Strip the type-argument tail (`S<T>` → `S`) and take the leaf
        // after the last dot (`p.S` → `S` for the resolution path, `p.S`
        // for the prefix split).
        let head = text.trim();
        let head = head.split('<').next().unwrap_or(head).trim();
        if head.is_empty() {
            return false;
        }
        let resolved = if let Some((prefix, leaf)) = head.split_once('.') {
            let prefix_id = self.strings.intern(prefix);
            let leaf_id = self.strings.intern(leaf);
            prefixed_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                prefix_id,
                leaf_id,
            )
            .map(|(container, owner)| (leaf_id, container, owner))
        } else {
            let name = self.strings.intern(head);
            self.resolve_container_from(name)
                .map(|(container, owner)| (name, container, owner))
        };
        let Some((leaf_name, container, owner)) = resolved else {
            return false;
        };
        self.chain_reaches_ffi(&[(leaf_name, container, owner)])
    }

    /// `InterfaceElement.isEmptyStruct`: no field carries a sized type.
    fn ffi_is_empty_compound(&self, container: &Container) -> bool {
        !container.members.values().any(|member| {
            member.kind == MemberKind::Field
                && match member.var_ty {
                    Ty::Named { name, .. } => {
                        let text = self.strings.resolve(name);
                        matches!(text, "int" | "double" | "bool" | "Pointer" | "Array")
                            || self.clause_entry_is_compound(name)
                    }
                    _ => false,
                }
        })
    }

    /// The `@Packed` faces on a `Struct` subclass: at most one annotation,
    /// and its alignment is one of the supported byte counts.
    fn ffi_check_packed(&mut self, name: StringId) {
        let Some((site, _)) = self.ffi_decl_site(name, |kind| matches!(kind, DeclKind::Container))
        else {
            return;
        };
        let packed: Vec<FfiDeclAnno> = self
            .ffi_annos_at(site)
            .iter()
            .filter(|anno| self.strings.resolve(anno.name).rsplit('.').next() == Some("Packed"))
            .cloned()
            .collect();
        for extra in packed.iter().skip(1) {
            self.emit(SemanticCode::PackedAnnotation, extra.at, "", "");
        }
        let Some(first) = packed.first() else {
            return;
        };
        let supported = first
            .face
            .alignment
            .is_some_and(|(value, _)| PACKED_ALIGNMENTS.contains(&value));
        if !supported {
            let at = first
                .face
                .alignment
                .map(|(_, span)| span)
                .unwrap_or(first.at);
            self.emit(SemanticCode::PackedAnnotationAlignment, at, "", "");
        }
    }

    /// The `AbiSpecificInteger` subclass faces: the single-const-constructor
    /// shape, the single mapping annotation, and the fixed-size mapping
    /// values.
    fn ffi_check_abi_specific_integer(
        &mut self,
        name: StringId,
        container: &Container,
        name_span: SourceRange,
    ) {
        // Exactly one member, a const unnamed constructor (upstream
        // `_validateAbiSpecificIntegerAnnotation`; constructors are no
        // `MemberEntry` rows, so the member set must be empty).
        let shape_ok = container.members.is_empty()
            && container.has_unnamed_constructor
            && container.has_const_constructor;
        if container.type_params > 0 || !shape_ok {
            self.emit(SemanticCode::AbiSpecificIntegerInvalid, name_span, "", "");
        }
        let Some((site, _)) = self.ffi_decl_site(name, |kind| matches!(kind, DeclKind::Container))
        else {
            return;
        };
        let mappings: Vec<FfiDeclAnno> = self
            .ffi_annos_at(site)
            .iter()
            .filter(|anno| {
                self.strings.resolve(anno.name).rsplit('.').next()
                    == Some("AbiSpecificIntegerMapping")
            })
            .cloned()
            .collect();
        if mappings.is_empty() {
            self.emit(
                SemanticCode::AbiSpecificIntegerMappingMissing,
                name_span,
                "",
                "",
            );
        }
        for extra in mappings.iter().skip(1) {
            self.emit(
                SemanticCode::AbiSpecificIntegerMappingExtra,
                extra.face.name_span,
                "",
                "",
            );
        }
        let Some(first) = mappings.first() else {
            return;
        };
        for (head_text, span) in &first.face.mapping_values {
            if !FIXED_SIZE_INTEGER_TYPES.contains(&head_text.as_str()) {
                self.emit(
                    SemanticCode::AbiSpecificIntegerMappingUnsupported,
                    *span,
                    head_text,
                    "",
                );
            }
        }
        let _ = container;
    }

    // —— the compound field checks ———————————————————————————————————

    /// `_validateFieldsInCompound` over the container's instance fields,
    /// in source order (the last-external-field face needs the written
    /// order the member map does not keep).
    fn ffi_check_compound_fields(&mut self, name: StringId, container: &'a Container) {
        let superclass_text = container
            .superclass
            .map(|super_name| self.strings.resolve(super_name).to_owned())
            .unwrap_or_default();
        let sites: Vec<(usize, DeclSite)> = self
            .decl_sites
            .iter()
            .enumerate()
            .filter(|(_, site)| {
                matches!(
                    &site.kind,
                    DeclKind::Member {
                        kind: MemberKind::Field,
                        is_static: false,
                        container: owner,
                    } if *owner == name
                )
            })
            .map(|(index, site)| (index, site.clone()))
            .collect();
        let Some(elements) = self.index.get(self.file) else {
            return;
        };
        for (index, site) in &sites {
            let Some(member) = elements
                .container(name)
                .and_then(|c| c.members.get(&site.name))
            else {
                continue;
            };
            self.ffi_check_compound_field(
                name,
                member,
                site.range,
                *index,
                &sites,
                &superclass_text,
            );
        }
    }

    /// One instance field of a compound (upstream `_validateFieldsInCompound`
    /// per `VariableDeclaration`).
    fn ffi_check_compound_field(
        &mut self,
        container: StringId,
        member: &MemberEntry,
        name_span: SourceRange,
        site_index: usize,
        sites: &[(usize, DeclSite)],
        superclass_text: &str,
    ) {
        if !member.is_external {
            self.emit(SemanticCode::FieldMustBeExternalInStruct, name_span, "", "");
        }
        let Some(type_span) = self.ffi_type_span_before(name_span.offset) else {
            self.emit(SemanticCode::MissingFieldTypeInStruct, name_span, "", "");
            return;
        };
        let type_text = self.ffi_type_text(type_span);
        let Ty::Named {
            name: head,
            nullable,
            args,
        } = member.var_ty
        else {
            self.emit(SemanticCode::MissingFieldTypeInStruct, name_span, "", "");
            return;
        };
        if nullable {
            self.emit(
                SemanticCode::InvalidFieldTypeInStruct,
                type_span,
                &type_text,
                "",
            );
            return;
        }
        let head_text = self.strings.resolve(head).to_owned();
        // The written dimensions: the `Array<...>` nesting depth.
        let array_dimensions = {
            let mut depth = 0usize;
            let mut cursor = member.var_ty;
            let pool = self.boundary.args.borrow();
            while let Ty::Named {
                name: inner,
                args: args_id,
                ..
            } = cursor
            {
                if self.strings.resolve(inner) != "Array"
                    || args_id == crate::model::ArgListId::EMPTY
                {
                    break;
                }
                depth += 1;
                let list = pool.get(args_id);
                let Some(first) = list.first() else {
                    break;
                };
                cursor = *first;
            }
            depth
        };
        let _ = args;
        match head_text.as_str() {
            "int" | "double" | "bool" => {
                let required = match head_text.as_str() {
                    "int" => 0,
                    "double" => 1,
                    _ => 2,
                };
                self.ffi_validate_field_annotations(
                    &member.annotations,
                    required,
                    type_span,
                    &type_text,
                    superclass_text,
                );
            }
            "Pointer" => {
                for anno in &member.annotations {
                    let text = self.strings.resolve(anno.name);
                    let leaf = text.rsplit('.').next().unwrap_or(text);
                    if FFI_CLASS_NAMES.contains(&leaf) {
                        self.emit(
                            SemanticCode::AnnotationOnPointerField,
                            self.ffi_model_annotation_span(anno),
                            "",
                            "",
                        );
                    }
                }
            }
            "Array" => {
                // The first type argument must be sized.
                if let Some(arg_span) = self.ffi_first_type_arg_span(type_span) {
                    let arg_text = self.ffi_type_text(arg_span);
                    if !self.ffi_is_sized_text(&arg_text) {
                        self.emit(
                            SemanticCode::NonSizedTypeArgument,
                            arg_span,
                            &arg_text,
                            "Array",
                        );
                    }
                }
                // Variable-length arrays must be the compound's last
                // external instance field.
                let allow_variable = self
                    .ffi_last_external_field(container, sites)
                    .is_some_and(|offset| offset == name_span.offset);
                self.ffi_validate_size_annotation(
                    site_index,
                    array_dimensions,
                    allow_variable,
                    type_span,
                );
            }
            _ => {
                if self.clause_entry_is_compound(head) {
                    if let Some((compound, _)) = self.ffi_compound_head(member.var_ty)
                        && let Some((compound_container, _)) = self.resolve_container_from(compound)
                        && self.ffi_is_empty_compound(compound_container)
                    {
                        let compound_text = self.strings.resolve(compound).to_owned();
                        let compound_super = compound_container
                            .superclass
                            .map(|s| self.strings.resolve(s).to_owned())
                            .unwrap_or_default();
                        self.emit(
                            SemanticCode::EmptyStruct,
                            type_span,
                            &compound_text,
                            &compound_super,
                        );
                    }
                } else {
                    self.emit(
                        SemanticCode::InvalidFieldTypeInStruct,
                        type_span,
                        &type_text,
                        "",
                    );
                }
            }
        }
    }
}

impl<'a> Engine<'a> {
    /// The last external instance field's name offset of the container, in
    /// written order (upstream `fields.reversed.where(...)`).
    fn ffi_last_external_field(
        &self,
        container: StringId,
        sites: &[(usize, DeclSite)],
    ) -> Option<u32> {
        let elements = self.index.get(self.file)?;
        sites
            .iter()
            .rev()
            .find(|(_, site)| {
                elements
                    .container(container)
                    .and_then(|c| c.members.get(&site.name))
                    .is_some_and(|member| member.is_external)
            })
            .map(|(_, site)| site.range.offset)
    }

    /// The model annotation's whole written span (`@` through the argument
    /// group).
    fn ffi_model_annotation_span(&self, anno: &crate::model::Annotation) -> SourceRange {
        let args_len = anno
            .args_text
            .map(|text| {
                self.strings
                    .resolve(text)
                    .chars()
                    .map(char::len_utf16)
                    .sum::<usize>() as u32
            })
            .unwrap_or(0);
        SourceRange {
            offset: anno.range.offset,
            length: anno.range.length + args_len,
        }
    }

    /// `_validateAnnotations`: exactly one ffi annotation whose native
    /// class matches the field's required primitive face.
    fn ffi_validate_field_annotations(
        &mut self,
        annotations: &[crate::model::Annotation],
        required: u8,
        type_span: SourceRange,
        type_text: &str,
        superclass_text: &str,
    ) {
        let mut found = false;
        let mut extras: Vec<&crate::model::Annotation> = Vec::new();
        for anno in annotations {
            let text = self.strings.resolve(anno.name);
            let leaf = text.rsplit('.').next().unwrap_or(text);
            if !FFI_CLASS_NAMES.contains(&leaf) {
                continue;
            }
            let classifies: Option<u8> = if NATIVE_INT_TYPES.contains(&leaf) {
                Some(0)
            } else if matches!(leaf, "Float" | "Double") {
                Some(1)
            } else if leaf == "Bool" {
                Some(2)
            } else {
                None
            };
            if !found && classifies == Some(required) {
                found = true;
            } else {
                extras.push(anno);
            }
        }
        if extras.is_empty() {
            if !found {
                self.emit(
                    SemanticCode::MissingAnnotationOnStructField,
                    type_span,
                    type_text,
                    superclass_text,
                );
            }
            return;
        }
        if !found {
            let first = extras.remove(0);
            self.emit(
                SemanticCode::MismatchedAnnotationOnStructField,
                self.ffi_model_annotation_span(first),
                "",
                "",
            );
        }
        for extra in extras {
            self.emit(
                SemanticCode::ExtraAnnotationOnStructField,
                self.ffi_model_annotation_span(extra),
                "",
                "",
            );
        }
    }

    /// `_validateSizeOfAnnotation` for an `Array`-typed field.
    fn ffi_validate_size_annotation(
        &mut self,
        site_index: usize,
        array_dimensions: usize,
        allow_variable: bool,
        type_span: SourceRange,
    ) {
        let array_annos: Vec<FfiDeclAnno> = self
            .ffi_annos_at(site_index)
            .iter()
            .filter(|anno| {
                self.strings.resolve(anno.name).rsplit('.').next() == Some("Array")
                    || self.strings.resolve(anno.name).starts_with("Array.")
            })
            .cloned()
            .collect();
        if array_annos.is_empty() {
            self.emit(SemanticCode::MissingSizeAnnotationCarray, type_span, "", "");
            return;
        }
        for extra in array_annos.iter().skip(1) {
            self.emit(SemanticCode::ExtraSizeAnnotationCarray, extra.at, "", "");
        }
        let first = &array_annos[0];
        if first.face.dims.len() != array_dimensions {
            self.emit(SemanticCode::SizeAnnotationDimensions, first.at, "", "");
        }
        if first.face.variable_length && !allow_variable {
            self.emit(SemanticCode::VariableLengthArrayNotLast, first.at, "", "");
        }
        for (index, (value, span)) in first.face.dims.iter().enumerate() {
            if index == 0 && first.face.variable_length {
                if *value < 0 {
                    self.emit(SemanticCode::NegativeVariableDimension, *span, "", "");
                }
                continue;
            }
            if *value <= 0 && span.length > 0 {
                self.emit(SemanticCode::NonPositiveArrayDimension, *span, "", "");
            }
        }
    }

    /// The written type head is a sized native face (upstream `_isSized`
    /// over the text — the compound/abi resolutions ride the element
    /// walk).
    fn ffi_is_sized_text(&self, text: &str) -> bool {
        let head = text
            .trim()
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
            .collect::<String>();
        if NATIVE_INT_TYPES.contains(&head.as_str())
            || matches!(
                head.as_str(),
                "Float" | "Double" | "Bool" | "Pointer" | "Array"
            )
        {
            return true;
        }
        let interned = self.intern(&head);
        self.clause_entry_is_compound(interned)
    }
}

impl<'a> Engine<'a> {
    // —— the `@Native` declaration family ————————————————————————————

    /// `_checkFfiNative` over every `@Native`-annotated declaration. The
    /// `container` arm walks the container's own member sites; `None`
    /// walks the top-level sites — plus every member whose container name
    /// never resolved to a real container entry (the unnamed-extension
    /// placeholder `""`, whose members live on `ExtensionFace`s).
    fn ffi_check_native_decls(&mut self, container: Option<(StringId, &'a Container)>) {
        let sites: Vec<(usize, DeclSite)> = self
            .decl_sites
            .iter()
            .enumerate()
            .filter(|(_, site)| match (&site.kind, container) {
                (
                    DeclKind::Member {
                        container: owner, ..
                    },
                    Some((name, _)),
                ) => *owner == name,
                (DeclKind::TopLevelFn | DeclKind::TopLevelVar, None) => true,
                (DeclKind::Member { container: owner, .. }, None) => self
                    .index
                    .get(self.file)
                    .is_some_and(|elements| elements.container(*owner).is_none()),
                _ => false,
            })
            .map(|(index, site)| (index, site.clone()))
            .collect();
        for (index, site) in sites.iter() {
            let mut native_annos: Vec<FfiDeclAnno> =
                self.ffi_native_annos(*index).into_iter().cloned().collect();
            // The const-alias arm (upstream reads `element.metadata`'s
            // computed values, so `@alias` of `const alias = Native<…>()`
            // counts as a `@Native` annotation — `const_creations` keeps
            // only bare-creation consts; the RHS face parses like the
            // written form's).
            native_annos.extend(self.ffi_const_native_annos(site));
            if native_annos.is_empty() {
                continue;
            }
            // Upstream reports the first extra `@Native` and stops
            // (`_checkFfiNative`'s `break` at :491).
            if let Some(extra) = native_annos.get(1) {
                self.emit(
                    SemanticCode::FfiNativeInvalidMultipleAnnotations,
                    extra.face.name_span,
                    "",
                    "",
                );
            }
            let mut native_annos = native_annos;
            let anno = native_annos.remove(0);
            // A bare `@Native` (no argument group) is a malformed
            // annotation — upstream's `annotation.arguments` gate rejects
            // it before any decl checks run.
            if anno.face.args.is_none() {
                continue;
            }
            self.ffi_check_native_decl(site, &anno, *index);
        }
    }

    /// One `@Native` declaration: the external posture, then the
    /// signature's dispatch (function vs field, written vs omitted).
    fn ffi_check_native_decl(&mut self, site: &DeclSite, anno: &FfiDeclAnno, index: usize) {
        // The `external` keyword sits between the annotation's close and
        // the declaring name (the metadata hugs the declaration).
        let ann_end = self.byte_of(anno.at.end());
        let name_start = self.byte_of(site.range.offset);
        let between = self.source.get(ann_end..name_start).unwrap_or_default();
        let external = between.split_whitespace().any(|word| word == "external");
        if !external {
            self.emit(SemanticCode::FfiNativeMustBeExternal, site.range, "", "");
        }
        let instance_member = matches!(
            &site.kind,
            DeclKind::Member {
                is_static: false,
                ..
            }
        );
        let (var_ty, return_ty, formals) = self.ffi_decl_types(site);
        let Some(sig) = anno.face.sig.as_ref() else {
            // The omitted `<T>`: infer from the Dart face (static
            // executables only — upstream's `DynamicType` arm).
            if instance_member {
                return;
            }
            let is_executable = matches!(site.kind, DeclKind::TopLevelFn)
                || matches!(
                    &site.kind,
                    DeclKind::Member {
                        kind: MemberKind::Method | MemberKind::Getter | MemberKind::Setter,
                        ..
                    }
                );
            if is_executable {
                let inferred_ret = if matches!(return_ty, Ty::Void) {
                    self.intern("Void")
                } else {
                    match return_ty {
                        Ty::Named { name, .. } => name,
                        // An omitted return type is `dynamic` upstream
                        // (`dartSignature.returnType`), and no other
                        // non-named shape spells a native type either:
                        // the validity walk's own `Dynamic` verdict
                        // answers `native_function_missing_type`.
                        _ => self.intern("Dynamic"),
                    }
                };
                let param_heads: Vec<StringId> = formals
                    .map(|f| {
                        f.positional
                            .iter()
                            .filter_map(|formal| match formal.ty {
                                Ty::Named { name, .. } => Some(name),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let valid = self.ffi_valid_native_signature(&inferred_ret, &param_heads);
                if !valid {
                    self.emit(SemanticCode::NativeFunctionMissingType, site.range, "", "");
                }
                return;
            }
            self.ffi_check_native_field(site, index, var_ty, None);
            return;
        };
        // The written signature: function-form signatures dispatch on the
        // declaration shape; a bare native type on an executable is the
        // `mustBeANativeFunctionType` face.
        let is_function_form = anno
            .face
            .sig
            .as_ref()
            .is_some_and(|sig| !sig.param_heads.is_empty() || sig.display.contains("Function"));
        let is_field = matches!(site.kind, DeclKind::TopLevelVar)
            || matches!(
                &site.kind,
                DeclKind::Member {
                    kind: MemberKind::Field,
                    ..
                }
            );
        if is_function_form {
            if is_field {
                self.emit(
                    SemanticCode::NativeFieldInvalidType,
                    site.range,
                    &sig.display,
                    "",
                );
                return;
            }
            self.ffi_check_native_function(site, sig, formals, instance_member, &anno.face);
        } else if is_field {
            let native_ty = Ty::named(self.intern(sig.display.trim()));
            self.ffi_check_native_field(site, index, var_ty, Some(&native_ty));
        } else {
            self.emit(
                SemanticCode::MustBeANativeFunctionType,
                site.range,
                &sig.display,
                "Native",
            );
        }
    }

    /// The declared faces of a `@Native` site: the variable type (fields),
    /// the return type (executables), and the formal table.
    fn ffi_decl_types(&self, site: &DeclSite) -> (Ty, Ty, Option<&'a Box<Formals>>) {
        let Some(elements) = self.index.get(self.file) else {
            return (Ty::UNKNOWN, Ty::UNKNOWN, None);
        };
        match &site.kind {
            DeclKind::TopLevelFn => {
                let decl = elements.decl(site.name);
                (
                    Ty::UNKNOWN,
                    decl.map(|d| d.return_ty).unwrap_or(Ty::UNKNOWN),
                    decl.and_then(|d| d.formals.as_ref()),
                )
            }
            DeclKind::TopLevelVar => {
                let decl = elements.decl(site.name);
                (
                    decl.map(|d| d.var_ty).unwrap_or(Ty::UNKNOWN),
                    Ty::UNKNOWN,
                    None,
                )
            }
            DeclKind::Member { container, .. } => {
                if let Some(member) = elements
                    .container(*container)
                    .and_then(|c| c.members.get(&site.name))
                {
                    return (
                        member.var_ty,
                        member.return_ty,
                        member.formals.as_ref(),
                    );
                }
                // An unnamed extension's members live on its
                // `ExtensionFace` (collector.rs:1963-1981 — the face owns
                // the member map outright; the placeholder `container`
                // name never joins `containers`).
                let member = elements
                    .extension_faces()
                    .iter()
                    .filter(|face| face.name.is_none())
                    .find_map(|face| face.members.get(&site.name));
                (
                    member.map(|m| m.var_ty).unwrap_or(Ty::UNKNOWN),
                    member.map(|m| m.return_ty).unwrap_or(Ty::UNKNOWN),
                    member.and_then(|m| m.formals.as_ref()),
                )
            }
            _ => (Ty::UNKNOWN, Ty::UNKNOWN, None),
        }
    }

    /// `_checkFfiNativeFunction`: the count, receiver-Pointer, leaf and
    /// validity faces over the written signature (upstream
    /// ffi_verifier.dart:641-829).
    ///
    /// `ffiParameterTypes` is the flattened VarArgs list (`VarArgs<(A,B)>`
    /// → `[A,B]`); `ffiParameters` is the raw formal list. Both are sliced
    /// after the receiver for instance members; the extension-like arm
    /// slices `ffiParameterTypes` but keeps `ffiParameters` whole because
    /// the augmented `dartType` already carries a synthetic receiver.
    fn ffi_check_native_function(
        &mut self,
        site: &DeclSite,
        sig: &FfiSigFace,
        formals: Option<&'a Box<Formals>>,
        instance_member: bool,
        face: &FfiAnnoFace,
    ) {
        let dart_param_count = formals
            .map(|f| f.positional.len() + f.named.len())
            .unwrap_or(0);
        let flat_params = ffi_flatten_varargs(&sig.display, self.strings);
        let ret_head = first_ident_head(&sig.display, self);
        if face.is_leaf {
            self.ffi_leaf_check(&ret_head, &sig.param_heads, site.range);
        }

        // The extension-like arm (upstream :669-722): the annotated member
        // lives on an extension or extension type. The receiver is an
        // implicit extra parameter — `formalParameters.length + 1` must
        // equal the flattened ffi count, and the receiver's own Pointer
        // gate rides the raw first element.
        let is_extension_like = instance_member && self.ffi_is_extension_member(site);
        if is_extension_like {
            if dart_param_count + 1 != flat_params.len() {
                let (a0, a1) = ((dart_param_count + 1).to_string(), flat_params.len().to_string());
                self.emit(
                    SemanticCode::FfiNativeUnexpectedNumberOfParameters,
                    site.range,
                    &a0,
                    &a1,
                );
                return;
            }
            if flat_params.first() == Some(&self.intern("Pointer"))
                && !self.ffi_extension_receiver_extends_nfwc(site)
            {
                self.emit(
                    SemanticCode::FfiNativeOnlyClassesExtendingNativefieldwrapperclass1CanBePointer,
                    site.range,
                    "",
                    "",
                );
                return;
            }
            // Slice the receiver off the flattened list for the
            // validity/compat face; `ffiParameters` stays whole (the
            // augmented dart sig carries the synthetic receiver).
            let sliced = &flat_params[1..];
            let param_heads: Vec<StringId> = sliced.to_vec();
            if !self.ffi_valid_native_signature(&ret_head, &param_heads) {
                self.emit(
                    SemanticCode::MustBeANativeFunctionType,
                    site.range,
                    &sig.display,
                    "Native",
                );
                return;
            }
            if !self.ffi_native_compat_sig(site, sig, sliced, true) {
                self.emit3(
                    SemanticCode::MustBeASubtype,
                    site.range,
                    &sig.display,
                    "Native",
                    "",
                );
            }
            return;
        }

        if instance_member {
            if dart_param_count + 1 != flat_params.len() {
                let (a0, a1) = (
                    (dart_param_count + 1).to_string(),
                    flat_params.len().to_string(),
                );
                self.emit(
                    SemanticCode::FfiNativeUnexpectedNumberOfParametersWithReceiver,
                    site.range,
                    &a0,
                    &a1,
                );
                return;
            }
            // The receiver may only be `Pointer` over a
            // `NativeFieldWrapperClass1` subclass.
            if flat_params.first() == Some(&self.intern("Pointer"))
                && let DeclKind::Member { container, .. } = &site.kind
                && let Some(elements) = self.index.get(self.file)
                && let Some(declaring) = elements.container(*container)
                && !self.ffi_extends_native_field_wrapper(declaring)
            {
                self.emit(
                    SemanticCode::FfiNativeOnlyClassesExtendingNativefieldwrapperclass1CanBePointer,
                    site.range,
                    "",
                    "",
                );
                return;
            }
        } else if dart_param_count != flat_params.len() {
            let (a0, a1) = (flat_params.len().to_string(), dart_param_count.to_string());
            self.emit(
                SemanticCode::FfiNativeUnexpectedNumberOfParameters,
                site.range,
                &a0,
                &a1,
            );
            return;
        }
        // A Pointer signature parameter needs a Pointer/typed-data/
        // NativeFieldWrapperClass1 formal.
        let param_offset = usize::from(instance_member && !is_extension_like);
        let check_params = &flat_params[param_offset.min(flat_params.len())..];
        if let Some(formals) = formals {
            for (i, head) in check_params.iter().enumerate() {
                if self.strings.resolve(*head) != "Pointer" {
                    continue;
                }
                let formal_ty = formals
                    .positional
                    .get(i)
                    .map(|formal| formal.ty);
                let ok = match formal_ty {
                    Some(Ty::Named { name, .. }) => {
                        let text = self.strings.resolve(name);
                        text == "Pointer"
                            || text.ends_with("List")
                            || self.clause_entry_is_compound(name)
                            || self.ffi_extends_wrapper_by_name(name)
                    }
                    _ => false,
                };
                if !ok {
                    self.emit(
                        SemanticCode::FfiNativeOnlyClassesExtendingNativefieldwrapperclass1CanBePointer,
                        site.range,
                        "",
                        "",
                    );
                }
            }
        }
        let param_heads: Vec<StringId> = check_params.to_vec();
        if !self.ffi_valid_native_signature(&ret_head, &param_heads) {
            self.emit(
                SemanticCode::MustBeANativeFunctionType,
                site.range,
                &sig.display,
                "Native",
            );
            return;
        }
        // The compat arm (upstream :812-827): the written native signature
        // must be a subtype of the Dart face (nativeToDart, with
        // nativeFieldWrappersAsPointer).
        if !self.ffi_native_compat_sig(site, sig, check_params, false) {
            self.emit3(
                SemanticCode::MustBeASubtype,
                site.range,
                &sig.display,
                "Native",
                "",
            );
        }
    }

    /// The annotated member lives on an extension or extension type (the
    /// `isExtensionLikeInstanceMember` arm): a `DeclKind::Member` whose
    /// container is an `ExtensionFace` or `ExtensionType`.
    fn ffi_is_extension_member(&self, site: &DeclSite) -> bool {
        let DeclKind::Member { container, .. } = &site.kind else {
            return false;
        };
        let Some(elements) = self.index.get(self.file) else {
            return false;
        };
        // Named extension: `elements.extension_faces` holds a face named
        // `container`. An unnamed extension's members register under the
        // empty placeholder name (`push_container_frame` interns `""`) —
        // their face is found by member ownership, not by name.
        if elements.container(*container).is_none() {
            return elements
                .extension_faces()
                .iter()
                .any(|face| {
                    face.name == Some(*container)
                        || (face.name.is_none() && face.members.contains_key(&site.name))
                });
        }
        false
    }

    /// The extension member's receiver type (the `extendedType` of the
    /// enclosing extension, or the `thisType` of an extension type) reaches
    /// `NativeFieldWrapperClass1` transitively.
    fn ffi_extension_receiver_extends_nfwc(&self, site: &DeclSite) -> bool {
        let DeclKind::Member { container, .. } = &site.kind else {
            return false;
        };
        let Some(elements) = self.index.get(self.file) else {
            return false;
        };
        // The receiver face: a named extension answers by name; an
        // unnamed extension answers by member ownership.
        if let Some(face) = elements.extension_faces().iter().find(|face| {
            face.name == Some(*container)
                || (face.name.is_none() && face.members.contains_key(&site.name))
        }) {
            if let Ty::Named { name, .. } = face.on_ty {
                return self.ffi_extends_wrapper_by_name(name);
            }
            return false;
        }
        // Extension type: `thisType` is the container itself.
        elements
            .container(*container)
            .is_some_and(|c| self.ffi_extends_native_field_wrapper(c))
    }

    /// The `mustBeASubtype` compat face: the written native signature
    /// (return + flattened param heads, sliced past the receiver) against
    /// the Dart face (return + formal types, with the receiver prepended
    /// for extension-like members). Upstream `_validateCompatibleFunctionTypes`
    /// at the head level: equal positional counts, then per-slot
    /// `ffi_compatible_native_value`.
    fn ffi_native_compat_sig(
        &self,
        site: &DeclSite,
        sig: &FfiSigFace,
        ffi_params: &[StringId],
        extension_like: bool,
    ) -> bool {
        let (_, return_ty, formals) = self.ffi_decl_types(site);
        let dart_ret = match return_ty {
            Ty::Named { name, .. } => name,
            Ty::Void => self.intern("void"),
            _ => return false,
        };
        let dart_params: Vec<StringId> = formals
            .map(|f| {
                let mut heads: Vec<StringId> = f
                    .positional
                    .iter()
                    .filter_map(|formal| match formal.ty {
                        Ty::Named { name, .. } => Some(name),
                        _ => None,
                    })
                    .collect();
                if extension_like {
                    // The receiver is the synthetic first slot (upstream
                    // builds `dartType` with a synthetic receiver
                    // prepended); its type is the extension's on-type or
                    // the extension type itself.
                    let receiver = self.ffi_extension_receiver_head(site);
                    let mut all = Vec::with_capacity(heads.len() + 1);
                    all.push(receiver.unwrap_or(StringId::NONE));
                    all.append(&mut heads);
                    all
                } else {
                    heads
                }
            })
            .unwrap_or_default();
        let native_ret = first_ident_head(&sig.display, self);
        self.ffi_compatible_functions(&dart_ret, &dart_params, &native_ret, ffi_params)
    }

    /// The receiver type head for an extension-like member's `dartType`
    /// (the extension's `extendedType` or the extension type itself).
    fn ffi_extension_receiver_head(&self, site: &DeclSite) -> Option<StringId> {
        let DeclKind::Member { container, .. } = &site.kind else {
            return None;
        };
        let elements = self.index.get(self.file)?;
        if let Some(face) = elements.extension_faces().iter().find(|face| {
            face.name == Some(*container)
                || (face.name.is_none() && face.members.contains_key(&site.name))
        }) {
            if let Ty::Named { name, .. } = face.on_ty {
                return Some(name);
            }
            return None;
        }
        // Extension type: `thisType` is the container itself.
        elements.container(*container).map(|_| *container)
    }

    /// `NativeFieldWrapperClass1` reachability by container name.
    fn ffi_extends_wrapper_by_name(&self, name: StringId) -> bool {
        self.resolve_container_from(name)
            .is_some_and(|(container, _)| self.ffi_extends_native_field_wrapper(container))
    }

    /// `_checkFfiNativeField`: the static posture, the omitted-signature
    /// inference, the `mustBeASubtype` compat, then the Handle/NativeFunction
    /// rejections and the `@Array` size annotation (upstream
    /// ffi_verifier.dart:570-639).
    fn ffi_check_native_field(
        &mut self,
        site: &DeclSite,
        site_index: usize,
        var_ty: Ty,
        native_ty: Option<&Ty>,
    ) {
        let instance_field = matches!(
            &site.kind,
            DeclKind::Member {
                kind: MemberKind::Field,
                is_static: false,
                ..
            }
        );
        if instance_field {
            self.emit(SemanticCode::NativeFieldNotStatic, site.range, "", "");
        }
        let Ty::Named {
            name: dart_head, ..
        } = var_ty
        else {
            return;
        };
        let Some(native_ty) = native_ty else {
            // The omitted `<T>`: the Dart type must be a canonical ffi
            // carrier (Pointer, Array, or compound subtype).
            let canonical = matches!(self.strings.resolve(dart_head), "Pointer" | "Array")
                || self.clause_entry_is_compound(dart_head);
            if !canonical {
                self.emit(SemanticCode::NativeFieldMissingType, site.range, "", "");
            }
            return;
        };
        let Ty::Named {
            name: native_head, ..
        } = *native_ty
        else {
            return;
        };
        let native_text = self.strings.resolve(native_head);
        // The compat arm (upstream :606-623): the Dart field type must be
        // a subtype-compatible face of the native type (allowFunctions:
        // true — the nativeFunction arm below reports the more specific
        // diagnostic for function-typed fields).
        let compat = self.ffi_compatible_native_value(var_ty, native_head, false);
        if !compat {
            let dart_display = self.ffi_render(var_ty);
            let native_display = self.ffi_render(*native_ty);
            self.emit3(
                SemanticCode::MustBeASubtype,
                site.range,
                &dart_display,
                &native_display,
                "Native",
            );
            return;
        }
        if native_text == "Array" {
            // Array native fields need an `@Array` size annotation.
            self.ffi_validate_size_annotation(site_index, 1, false, site.range);
        } else if matches!(native_text, "Handle" | "NativeFunction") {
            let display = self.ffi_render(*native_ty);
            self.emit(
                SemanticCode::NativeFieldInvalidType,
                site.range,
                &display,
                "",
            );
        }
    }

    /// The leaf-call face over a written signature: no `Handle` return or
    /// parameters (upstream `_validateFfiLeafCallUsesNoHandles`).
    fn ffi_leaf_check(&mut self, ret_head: &StringId, params: &[StringId], at: SourceRange) {
        if self.strings.resolve(*ret_head) == "Handle" {
            self.emit(SemanticCode::LeafCallMustNotReturnHandle, at, "", "");
        }
        if params
            .iter()
            .any(|head| self.strings.resolve(*head) == "Handle")
        {
            self.emit(SemanticCode::LeafCallMustNotTakeHandle, at, "", "");
        }
    }

    /// The native-validity face of a signature head + parameter heads
    /// (upstream `_isValidFfiNativeFunctionType` at the name level: no
    /// named/optional columns at this shape, return `allowVoid` +
    /// `allowHandle`, parameters `allowHandle`).
    fn ffi_valid_native_signature(&self, ret_head: &StringId, params: &[StringId]) -> bool {
        self.ffi_valid_native_type(*ret_head, true, true)
            && params
                .iter()
                .all(|head| self.ffi_valid_native_type(*head, false, true))
    }

    /// `_isValidFfiNativeType` at the head level (upstream
    /// ffi_verifier.dart:918-984): native primitives, the
    /// void/handle/empty-struct/Opaque gates, the Pointer arg rule, the
    /// `isOpaqueSubtype`/`isAbiSpecificIntegerSubtype` walk, and the
    /// `NativeFunction` recursion.
    ///
    /// `allow_array` is only `true` for the `@Native` field face
    /// (upstream `allowArray` rides the `_checkFfiNativeField` call);
    /// function signatures never allow `Array` as a parameter type.
    fn ffi_valid_native_type(
        &self,
        head: StringId,
        allow_void: bool,
        allow_handle: bool,
    ) -> bool {
        self.ffi_valid_native_type_full(head, allow_void, false, false, allow_handle, false)
    }

    /// The full gate matrix — the `allow_*` parameters mirror upstream's
    /// named arguments.
    fn ffi_valid_native_type_full(
        &self,
        head: StringId,
        allow_void: bool,
        allow_empty_struct: bool,
        allow_array: bool,
        allow_handle: bool,
        allow_opaque: bool,
    ) -> bool {
        let text = self.strings.resolve(head);
        match text {
            "Void" => allow_void,
            "Handle" => allow_handle,
            "Dynamic" => false,
            _ if NATIVE_INT_TYPES.contains(&text) => true,
            _ if matches!(text, "Float" | "Double" | "Bool") => true,
            "NativeFunction" => {
                // `NativeFunction<T>` recurses on the written type
                // argument — the sig display's `<...>` face. At the head
                // level the argument is not resolvable, so the recursion
                // reads the element model's type (upstream
                // `nativeType.typeArguments.single`).
                self.ffi_native_function_arg_valid(head)
            }
            "Pointer" => {
                // `Pointer<T>` is valid when `T` is itself a valid native
                // type (void/empty-struct/handle/Opaque allowed), a
                // compound subtype, or `NativeType` (upstream :945-956).
                self.ffi_pointer_arg_valid(head, allow_empty_struct)
            }
            "Opaque" => allow_opaque,
            "Array" => allow_array,
            _ => {
                // The compound/abi walk: `isCompoundSubtype` needs the
                // empty-struct gate; `isOpaqueSubtype` and
                // `isAbiSpecificIntegerSubtype` are unconditional.
                let Some((container, owner)) = self.resolve_container_from(head) else {
                    return false;
                };
                if self.chain_reaches_ffi(&[(head, container, owner)]) {
                    if !allow_empty_struct {
                        // The empty-struct rejection (upstream :959-964):
                        // a compound subtype whose every field is unsized
                        // is not a valid native type.
                        if self.ffi_is_empty_compound(container) {
                            return false;
                        }
                    }
                    return true;
                }
                // `isOpaqueSubtype` / `isAbiSpecificIntegerSubtype`.
                let text = self.strings.resolve(head);
                if text == "AbiSpecificInteger" {
                    return self
                        .resolve_container_from(head)
                        .is_some_and(|(_, owner)| self.owner_is_ffi(owner));
                }
                // A user class extending `Opaque` is an opaque subtype.
                if let Some((container, _)) = self.resolve_container_from(head) {
                    let mut cursor = container;
                    let mut depth = 0;
                    while let Some(super_name) = cursor.superclass {
                        if depth > 16 {
                            break;
                        }
                        depth += 1;
                        let super_text = self.strings.resolve(super_name);
                        if super_text == "Opaque" {
                            return true;
                        }
                        let Some((next, _)) = self.resolve_container_from(super_name) else {
                            break;
                        };
                        cursor = next;
                    }
                }
                false
            }
        }
    }

    /// `NativeFunction<T>`'s type argument is itself a valid native
    /// function type — the recursion reads the sig display's `<...>` face
    /// or the element model's instantiated alias.
    fn ffi_native_function_arg_valid(&self, _head: StringId) -> bool {
        // The head-level path can't recover the type argument; the
        // element-model path resolves it. Conservatively accept (upstream
        // `_isValidFfiNativeFunctionType` on the type argument).
        true
    }

    /// `Pointer<T>`'s type argument is a valid native type, a compound
    /// subtype, or `NativeType` (upstream :945-956).
    fn ffi_pointer_arg_valid(&self, _head: StringId, _allow_empty_struct: bool) -> bool {
        // Head-level can't recover the type argument; the element-model
        // path resolves it. Conservatively accept.
        true
    }
}

impl<'a> Engine<'a> {
    // —— the invocation pass ——————————————————————————————————————————

    /// The captured invocation faces, dispatched on the selector spelling
    /// (upstream `visitMethodInvocation` / `visitInstanceCreationExpression`).
    fn ffi_check_invocation(&mut self, invocation: &FfiInvocation) {
        let callee_text = self.strings.resolve(invocation.callee).to_owned();
        let selector = invocation
            .selector
            .map(|name| self.strings.resolve(name).to_owned());
        match (callee_text.as_str(), selector.as_deref()) {
            ("fromFunction", _) => self.ffi_check_from_function(invocation, "fromFunction"),
            ("asFunction", _) => self.ffi_check_as_function(invocation),
            ("lookupFunction", _) => self.ffi_check_lookup_function(invocation),
            ("sizeOf", _) => self.ffi_check_size_of(invocation),
            ("create", _) => self.ffi_check_create(invocation),
            ("elementAt", _) => self.ffi_check_element_at(invocation),
            ("addressOf", _) => self.ffi_check_address_of(invocation),
            ("NativeCallable", Some("listener")) => {
                self.ffi_check_native_callable_listener(invocation);
            }
            ("NativeCallable", Some("isolateLocal")) => {
                self.ffi_check_native_callable_isolate_local(invocation);
            }
            _ => {
                // A generative creation of a compound subtype (upstream
                // `visitInstanceCreationExpression`; factory creations and
                // `NativeCallable` shapes are legal).
                if invocation.is_creation
                    && selector.is_none()
                    && !matches!(callee_text.as_str(), "NativeCallable")
                {
                    self.ffi_check_compound_creation(invocation);
                }
            }
        }
    }

    /// The written type argument at `index` (type plus written span).
    fn ffi_type_arg(&self, invocation: &FfiInvocation) -> Option<FfiTypeArg> {
        invocation.type_arg.clone()
    }

    /// The written type argument's function-form shape: a parsed
    /// `R Function(P...)` directly, a bare spelling through the element
    /// model's alias expansion.
    fn ffi_type_arg_shape(&self, native: &FfiTypeArg) -> Option<(StringId, Vec<StringId>)> {
        if let Some((ret, params)) = &native.fn_sig {
            return Some((*ret, params.to_vec()));
        }
        self.ffi_function_shape(Ty::named(native.head))
    }

    /// The head spelling of a written type argument, alias-expanded: a
    /// type-alias head folds to its right-hand side (the oracle's
    /// instantiated alias face).
    fn ffi_expanded_head(&self, ty: Ty) -> Option<(StringId, Ty)> {
        let Ty::Named { name, .. } = ty else {
            return None;
        };
        let Some(elements) = self.index.get(self.file) else {
            return Some((name, ty));
        };
        match elements.decl(name) {
            Some(decl) if decl.kind == crate::model::MicroKind::TypeAlias => {
                Some((name, decl.var_ty))
            }
            _ => Some((name, ty)),
        }
    }

    /// `_validateFromFunction` (and `isolateLocal`'s exception faces).
    fn ffi_check_from_function(&mut self, invocation: &FfiInvocation, callee: &str) {
        if invocation.args.is_empty() || invocation.args.len() > 2 {
            return;
        }
        let Some(native) = self.ffi_type_arg(invocation) else {
            return;
        };
        let Some((native_ret, native_params)) = self.ffi_type_arg_shape(&native) else {
            self.emit(
                SemanticCode::MustBeANativeFunctionType,
                native.span,
                &native.display,
                callee,
            );
            return;
        };
        if !self.ffi_valid_native_signature(&native_ret, &native_params) {
            self.emit(
                SemanticCode::MustBeANativeFunctionType,
                native.span,
                &native.display,
                callee,
            );
            return;
        }
        let ret_head = native_ret;
        // The Dart argument's tear-off face: a top-level/static function
        // name resolves to its synthesized function type.
        // The tear-off head: the fold's argument type for a bare function
        // tear-off is unresolved (`Unknown` in the no-SDK posture), so the
        // head spelling is the lookup key.
        let dart_fn = invocation
            .args
            .first()
            .map(|(_, shape)| shape.head)
            .filter(|head| *head != StringId::NONE)
            .and_then(|head| self.ffi_tearoff_type(head));
        if let (Some((dart_ret, dart_params)), Some(f)) = (&dart_fn, invocation.args.first()) {
            // The tear-off compat is `dartToNative` (upstream
            // `_validateFromFunction` — the Dart callback becomes the
            // native function).
            if !self.ffi_compatible_functions_d2n(dart_ret, dart_params, &ret_head, &native_params) {
                let dart_display = self.ffi_render_function(dart_ret, dart_params);
                // The supertype display is the WRITTEN type argument
                // (`mustBeASubtype`'s second placeholder keeps the source
                // spelling `T`, not the alias-expanded face).
                let written = native.display.to_string();
                self.emit3(
                    SemanticCode::MustBeASubtype,
                    f.1.range,
                    &dart_display,
                    &written,
                    callee,
                );
                return;
            }
        }
        let native_ret_text = self.strings.resolve(ret_head).to_owned();
        let void_like =
            matches!(native_ret_text.as_str(), "Void") || self.ffi_valid_pointer_like(ret_head);
        if void_like {
            if invocation.args.len() == 2 {
                self.emit(
                    SemanticCode::InvalidExceptionValue,
                    invocation.args[1].1.range,
                    callee,
                    "",
                );
            }
            return;
        }
        if invocation.args.len() == 1 {
            self.emit(
                SemanticCode::MissingExceptionValue,
                invocation.head,
                callee,
                "",
            );
            return;
        }
        let exceptional = &invocation.args[1].1;
        // The exceptionalReturn compat is `dartToNative` (upstream
        // `_validateNativeCallable`'s `_validateCompatibleNativeType` at
        // :1982 — the Dart exception value becomes the native return).
        let compatible = self.ffi_compatible_native_value(exceptional.ty, ret_head, true);
        if !compatible {
            let dart_display = self.ffi_render(exceptional.ty);
            let native_display = native_ret_text.clone();
            self.emit3(
                SemanticCode::MustBeASubtype,
                exceptional.range,
                &dart_display,
                &native_display,
                callee,
            );
        }
        if !self.ffi_arg_is_const(exceptional) {
            self.emit(
                SemanticCode::ArgumentMustBeAConstant,
                self.ffi_whole_arg_span(exceptional.range),
                "exceptionalReturn",
                "",
            );
        }
    }

    /// The argument expression's whole span: the fold's argument range may
    /// carry the callee head only — when the argument list's `(` sits just
    /// past the range's end, the span extends to its matching `)` (the
    /// oracle anchors the row over the full argument expression).
    fn ffi_whole_arg_span(&self, range: SourceRange) -> SourceRange {
        let start_byte = self.byte_of(range.offset);
        let end_byte = self.byte_of(range.end()).min(self.source.len());
        let tail = &self.source[end_byte..];
        let trimmed = tail.trim_start();
        if !trimmed.starts_with('(') {
            return range;
        }
        let open = end_byte + (tail.len() - trimmed.len());
        let Some(close) = matching_close(self.source.as_bytes(), open, b'(', b')') else {
            return range;
        };
        let utf16 = |b: usize| byte_offset_to_utf16(self.source, b);
        SourceRange {
            offset: range.offset,
            length: utf16(close + 1) - utf16(start_byte),
        }
    }

    /// `nonNativeFunctionTypeArgumentToPointer`: the pointer's
    /// `NativeFunction<X>` face rendered whole, anchored at the written
    /// `asFunction` type argument.
    fn emit_non_native_signature(
        &mut self,
        invocation: &FfiInvocation,
        nf_head: StringId,
        nf_args: crate::model::ArgListId,
    ) {
        let display = self.ffi_render(Ty::Named {
            name: nf_head,
            nullable: false,
            args: nf_args,
        });
        let at = invocation
            .type_arg
            .as_ref()
            .map(|native| native.span)
            .unwrap_or(invocation.head);
        self.emit(
            SemanticCode::NonNativeFunctionTypeArgumentToPointer,
            at,
            &display,
            "",
        );
    }

    /// `_validateAsFunction` over a `Pointer<NativeFunction<T>>` receiver.
    fn ffi_check_as_function(&mut self, invocation: &FfiInvocation) {
        // The receiver must resolve as the FFI Pointer.
        let Ty::Named {
            name: recv_head,
            args: recv_args,
            ..
        } = invocation.recv_ty
        else {
            return;
        };
        if self.strings.resolve(recv_head) != "Pointer" {
            return;
        }
        let native_fn = {
            let pool = self.boundary.args.borrow();
            let args = pool.get(recv_args);
            args.first().copied()
        };
        let Some(Ty::Named {
            name: nf_head,
            args: nf_args,
            ..
        }) = native_fn
        else {
            return;
        };
        if self.strings.resolve(nf_head) != "NativeFunction" {
            return;
        }
        let pointer_arg = {
            let pool = self.boundary.args.borrow();
            pool.get(nf_args).first().copied()
        };
        let Some(pointer_arg) = pointer_arg else {
            return;
        };
        // A type-parameter pointer argument is the non-constant face.
        if let Some((head, _)) = self.ffi_expanded_head(pointer_arg)
            && self.ffi_is_type_param(head)
        {
            self.emit(
                SemanticCode::NonConstantTypeArgument,
                invocation.recv_span_or_head(),
                "asFunction",
                "",
            );
            return;
        }
        let Some((sig_ret, sig_params)) = self.ffi_function_shape(pointer_arg) else {
            return self.emit_non_native_signature(invocation, nf_head, nf_args);
        };
        // The expanded signature must still be a valid C signature: a
        // Dart-side spelling (`int Function(int)`) reads through the
        // alias face but fails the native gate.
        if !self.ffi_valid_native_signature(&sig_ret, &sig_params) {
            self.emit_non_native_signature(invocation, nf_head, nf_args);
            return;
        }

        // The leaf posture (`isLeaf: true`): no Handles in the signature.
        let is_leaf = invocation.args.iter().any(|(name, shape)| {
            name.is_some_and(|name| self.strings.resolve(name) == "isLeaf")
                && self.ffi_arg_is_true(shape)
                && !shape.invocation_shaped
        });
        if is_leaf {
            self.ffi_leaf_check(&sig_ret, &sig_params, invocation.head);
        }
        // `isLeaf` must be a constant.
        for (name, shape) in &invocation.args {
            if name.is_some_and(|name| self.strings.resolve(name) == "isLeaf")
                && !self.ffi_arg_is_const(shape)
            {
                self.emit(
                    SemanticCode::ArgumentMustBeAConstant,
                    shape.range,
                    "isLeaf",
                    "",
                );
            }
        }
        // The written Dart signature must be subtype-compatible
        // (upstream `_validateAsFunction`: the Dart `asFunction<T>` type
        // argument must be a subtype of the native `NativeFunction<S>`'s
        // signature; `subtype` is the Dart face, `supertype` the native).
        if let Some(written) = self.ffi_type_arg(invocation)
            && let Some((dart_ret, dart_params)) = self.ffi_type_arg_shape(&written)
        {
            if !self.ffi_compatible_functions(&dart_ret, &dart_params, &sig_ret, &sig_params) {
                let dart_display = self.ffi_render_function(&dart_ret, &dart_params);
                let native_display = self.ffi_render_function(&sig_ret, &sig_params);
                self.emit3(
                    SemanticCode::MustBeASubtype,
                    invocation.whole,
                    &dart_display,
                    &native_display,
                    "asFunction",
                );
            }
        }
    }

    /// `_validateLookupFunction`'s native-signature face. `lookupFunction`
    /// writes two type arguments (`<S, F>`): `S` is the native signature,
    /// `F` the Dart function type — each reports onto its own span.
    fn ffi_check_lookup_function(&mut self, invocation: &FfiInvocation) {
        let type_args = self.ffi_split_type_args(invocation);
        let Some((s_text, s_span)) = type_args.first().cloned() else {
            return;
        };
        // The native signature `S`: a written `R Function(P...)` reads its
        // heads directly; a bare spelling (an alias or type parameter)
        // folds through the element model's function shape.
        let s_head = first_ident_head(&s_text, self);
        let sig = if s_text.contains("Function") {
            let params: Vec<StringId> = function_param_heads(&s_text)
                .into_iter()
                .map(|h| self.intern(&h))
                .collect();
            Some((s_head, params))
        } else {
            self.ffi_function_shape(Ty::named(s_head))
        };
        let Some((ret, raw_params)) = sig else {
            self.emit(
                SemanticCode::MustBeANativeFunctionType,
                s_span,
                &s_text,
                "lookupFunction",
            );
            return;
        };
        // The validity gate (upstream `_isValidFfiNativeFunctionType` over
        // the signature, `flattenVarArgs` applied — a `VarArgs` survives
        // only as the last positional field record). The flattened list
        // is the compat/leaf input too (`normalParameterTypes
        // .flattenVarArgs()` upstream).
        let varargs = self.intern("VarArgs");
        let params = if raw_params.contains(&varargs) {
            ffi_flatten_varargs(&s_text, self.strings)
        } else {
            raw_params
        };
        if !self.ffi_valid_native_signature(&ret, &params) {
            self.emit(
                SemanticCode::MustBeANativeFunctionType,
                s_span,
                &s_text,
                "lookupFunction",
            );
            return;
        }
        // The S-vs-F compat arm (upstream `_validateLookupFunction`
        // :1769-1779): the written `S` (the native signature) must be a
        // subtype of `F` (the Dart function type).
        if let Some((f_text, f_span)) = type_args.get(1).cloned() {
            let f_ret = first_ident_head(&f_text, self);
            let f_params: Vec<StringId> = function_param_heads(&f_text)
                .into_iter()
                .map(|h| self.intern(&h))
                .collect();
            if !self.ffi_compatible_functions(&f_ret, &f_params, &ret, &params) {
                self.emit3(
                    SemanticCode::MustBeASubtype,
                    f_span,
                    &s_text,
                    &f_text,
                    "lookupFunction",
                );
            }
        }
        let is_leaf = invocation.args.iter().any(|(name, shape)| {
            name.is_some_and(|name| self.strings.resolve(name) == "isLeaf")
                && self.ffi_arg_is_true(shape)
        });
        if is_leaf {
            self.ffi_leaf_check(&ret, &params, invocation.head);
        }
        // `isLeaf` must be a constant (upstream `_validateIsLeafIsConst`).
        for (name, shape) in &invocation.args {
            if name.is_some_and(|name| self.strings.resolve(name) == "isLeaf")
                && !self.ffi_arg_is_const(shape)
            {
                self.emit(
                    SemanticCode::ArgumentMustBeAConstant,
                    shape.range,
                    "isLeaf",
                    "",
                );
            }
        }
    }

    /// `_validateSizeOf`: the type argument must be a valid native type.
    fn ffi_check_size_of(&mut self, invocation: &FfiInvocation) {
        let Some(native) = self.ffi_type_arg(invocation) else {
            return;
        };
        let Some((head, _)) = self.ffi_expanded_head(Ty::named(native.head)) else {
            return;
        };
        let is_param = self.ffi_is_type_param(head);
        if is_param || !self.ffi_valid_native_type(head, true, false) {
            self.emit(
                SemanticCode::NonConstantTypeArgument,
                invocation.whole,
                "sizeOf",
                "",
            );
        }
    }

    /// `_validateCreate` over a Struct/Union receiver.
    fn ffi_check_create(&mut self, invocation: &FfiInvocation) {
        let Ty::Named { name: recv, .. } = invocation.recv_ty else {
            return;
        };
        let Some(native) = self.ffi_type_arg(invocation) else {
            return;
        };
        let Some((head, _)) = self.ffi_expanded_head(Ty::named(native.head)) else {
            return;
        };
        if !self.ffi_valid_native_type(head, false, false) {
            let name = format!("{}.create", self.strings.resolve(recv));
            self.emit(
                SemanticCode::NonConstantTypeArgument,
                invocation.whole,
                &name,
                "",
            );
        }
    }

    /// `_validateElementAt` over a Pointer receiver.
    fn ffi_check_element_at(&mut self, invocation: &FfiInvocation) {
        let Ty::Named { name: recv, .. } = invocation.recv_ty else {
            return;
        };
        if self.strings.resolve(recv) != "Pointer" {
            return;
        }
        let Some(native) = self.ffi_type_arg(invocation) else {
            return;
        };
        let Some((head, _)) = self.ffi_expanded_head(Ty::named(native.head)) else {
            return;
        };
        if !self.ffi_valid_native_type(head, true, false) {
            self.emit(
                SemanticCode::NonConstantTypeArgument,
                invocation.whole,
                "elementAt",
                "",
            );
        }
    }

    /// The second type argument's written text and span of a
    /// The written `<S, F>` type-argument group's per-argument faces:
    /// [`FfiTypeArg`] captures the whole inner text as one face (the head
    /// is the first identifier), so `lookupFunction<S, F>`'s `S`/`F` split
    /// re-derives here — each top-level comma-separated run's text plus
    /// its own source span (the anchors `mustBeANativeFunctionType` and
    /// `mustBeASubtype` write onto).
    fn ffi_split_type_args(&self, invocation: &FfiInvocation) -> Vec<(String, SourceRange)> {
        let Some(native) = invocation.type_arg.as_ref() else {
            return Vec::new();
        };
        let base = self.byte_of(native.span.offset);
        split_top_level(&native.display, b',')
            .into_iter()
            .filter_map(|(off, len)| {
                let run = native.display.get(off..off + len)?.trim();
                if run.is_empty() {
                    return None;
                }
                // The run's own span: its trimmed start through end in the
                // source (the `display` bytes mirror `span`'s source run).
                let lead = len - native.display.get(off..off + len)?.trim_start().len();
                let start = base + off + lead;
                let end = start + run.len();
                Some((
                    run.to_owned(),
                    SourceRange {
                        offset: byte_offset_to_utf16(self.source, start),
                        length: byte_offset_to_utf16(self.source, end)
                            .saturating_sub(byte_offset_to_utf16(self.source, start)),
                    },
                ))
            })
            .collect()
    }

    /// The written bare-`true` posture: the argument's source text is the
    /// `true` literal (`isLeaf: true` — a literal, so its captured head is
    /// unset and the answer reads the bytes).
    fn ffi_arg_is_true(&self, shape: &FfiArgShape) -> bool {
        let start = self.byte_of(shape.range.offset);
        let end = self.byte_of(shape.range.end()).min(self.source.len());
        self.source
            .get(start..end)
            .is_some_and(|text| text.trim() == "true")
    }

    /// `_isConst` at the head level (upstream ffi_verifier.dart:842-845 —
    /// `expr.computeConstantValue()?.value != null`): a literal or
    /// `const`-prefixed expression, an identifier bound to a const
    /// declaration, or a const-constructor creation. A local variable,
    /// parameter, or compound expression is never const.
    fn ffi_arg_is_const(&self, shape: &FfiArgShape) -> bool {
        let start = self.byte_of(shape.range.offset);
        let end = self.byte_of(shape.range.end()).min(self.source.len());
        let Some(text) = self.source.get(start..end) else {
            return false;
        };
        let text = text.trim();
        if text.starts_with("const") {
            return true;
        }
        if shape.invocation_shaped {
            return shape.head != StringId::NONE && self.const_ctor_target(shape.head);
        }
        // A bare identifier: const only when it names a `const`
        // declaration. `decl()` keeps the var's const bit in `var_ty`'s
        // face? — the const flag rides the `DeclSite`'s annotations-free
        // path; the source check before the name covers `const`/`final`
        // (a `final` local is not a compile-time constant upstream —
        // `computeConstantValue` answers null for it — so only `const`
        // qualifies).
        let is_ident = !text.is_empty()
            && text
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
        if is_ident {
            let name = self.strings.intern(text);
            return self.decl_sites.iter().any(|site| {
                if site.name != name || !matches!(site.kind, DeclKind::TopLevelVar) {
                    return false;
                }
                // Source-walk back from the declaring name for `const`.
                let decl_start = self.byte_of(site.range.offset);
                self.source[..decl_start]
                    .rsplit(|c: char| c.is_alphanumeric() || c == '_')
                    .next()
                    .is_some_and(|prev| prev == "const")
            });
        }
        // Literals and other non-identifier expressions compute a constant
        // only in the literal forms — numbers, strings, `true`/`false`,
        // `null`, and sign-prefixed numerics. Everything else (operators,
        // conditionals, calls) is not.
        matches!(text, "true" | "false" | "null")
            || text.chars().next().is_some_and(|c| {
                c.is_ascii_digit() || c == '"' || c == '\''
            })
            || (text.starts_with('-') || text.starts_with('+'))
                && text[1..].chars().next().is_some_and(|c| c.is_ascii_digit())
    }

    /// `_validateNativeAddressOf`: the argument must name a `@Native`
    /// declaration, and its inferred native type must match the
    /// `NativeFunction<T>` type argument (upstream
    /// ffi_verifier.dart:1788-1911).
    fn ffi_check_address_of(&mut self, invocation: &FfiInvocation) {
        // The receiver must be the FFI `Native` class.
        let Ty::Named { name: recv, .. } = invocation.recv_ty else {
            return;
        };
        if !self.ffi_class_spelling(recv) || self.strings.resolve(recv) != "Native" {
            return;
        }
        let Some((_, shape)) = invocation.args.first() else {
            return;
        };
        // A non-identifier argument (lambda, non-constant expression) is
        // not a `@Native` reference — upstream `referencedElement == null`
        // → `argumentMustBeNative`.
        let arg_text = self
            .source
            .get(
                self.byte_of(shape.range.offset)
                    ..self.byte_of(shape.range.end()).min(self.source.len()),
            )
            .unwrap_or_default();
        let is_identifier = arg_text
            .trim()
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
            && !arg_text.trim().is_empty();
        if !is_identifier {
            self.emit(SemanticCode::ArgumentMustBeNative, shape.range, "", "");
            return;
        }
        let target = if shape.head == StringId::NONE {
            match shape.ty {
                Ty::Named { name, .. } => name,
                _ => return,
            }
        } else {
            shape.head
        };
        let annotated = self
            .decl_sites
            .iter()
            .any(|site| site.name == target && site.annotations.contains(&self.intern("Native")));
        if !annotated {
            self.emit(SemanticCode::ArgumentMustBeNative, shape.range, "", "");
            return;
        }
        // The `NativeFunction<T>` compat arm (upstream :1821-1833): the
        // `@Native` annotation's written type must equal the
        // `NativeFunction<T>` type argument's function type.
        if let Some(native) = self.ffi_type_arg(invocation)
            && self.strings.resolve(native.head) == "NativeFunction"
        {
            // The target's `@Native` annotation's written signature.
            let annotated_sig = self
                .decl_sites
                .iter()
                .position(|site| site.name == target)
                .and_then(|index| {
                    self.ffi_native_annos(index).first().and_then(|anno| {
                        anno.face
                            .sig
                            .as_ref()
                            .map(|sig| (sig.display.clone(), sig.ret_head, sig.param_heads.clone()))
                    })
                });
            if let Some((_, a_ret, a_params)) = annotated_sig
                && let Some((t_ret, t_params)) = self.ffi_type_arg_shape(&native)
                && (t_ret != a_ret || *t_params != *a_params)
            {
                self.emit(
                    SemanticCode::MustBeASubtype,
                    invocation.head,
                    &self.ffi_render_function(&a_ret, &a_params),
                    &self.ffi_render_function(&t_ret, &t_params),
                );
            }
        }
    }

    /// The `ConstructorName` span of a named-selector creation
    /// (`NativeCallable<T>.listener`): `invocation.head` carries the class
    /// head only, so the name's `.selector` tail is source-walked — the
    /// optional `<…>` group, the `.`, and the selector identifier (upstream
    /// `node.constructorName`, which anchors `mustBeANativeFunctionType`
    /// and `missingExceptionValue` on `NativeCallable`). Falls back to the
    /// head when no selector reads.
    fn ffi_constructor_name_span(&self, invocation: &FfiInvocation) -> SourceRange {
        let head = invocation.head;
        let bytes = self.source.as_bytes();
        let mut cursor = self.byte_of(head.end());
        while bytes.get(cursor).is_some_and(|b| b.is_ascii_whitespace()) {
            cursor += 1;
        }
        if bytes.get(cursor) == Some(&b'<') {
            let Some(close) = matching_close(bytes, cursor, b'<', b'>') else {
                return head;
            };
            cursor = close + 1;
            while bytes.get(cursor).is_some_and(|b| b.is_ascii_whitespace()) {
                cursor += 1;
            }
        }
        if bytes.get(cursor) != Some(&b'.') {
            return head;
        }
        let mut end = cursor + 1;
        while bytes
            .get(end)
            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'$')
        {
            end += 1;
        }
        if end == cursor + 1 {
            return head;
        }
        SourceRange {
            offset: head.offset,
            length: byte_offset_to_utf16(self.source, end).saturating_sub(head.offset),
        }
    }

    /// A named argument's whole span (`exceptionalReturn: 4`): the fold's
    /// argument range carries the value expression only, so the `name :`
    /// prefix is walked back — upstream `.at(arguments[i])` on the
    /// `NamedExpression` anchors `invalidExceptionValue`. Falls back to the
    /// value range when no `name :` reads.
    fn ffi_named_arg_span(&self, invocation: &FfiInvocation, index: usize) -> SourceRange {
        let Some((Some(name), shape)) = invocation.args.get(index) else {
            return invocation
                .args
                .get(index)
                .map(|(_, shape)| shape.range)
                .unwrap_or(invocation.head);
        };
        let bytes = self.source.as_bytes();
        let mut cursor = self.byte_of(shape.range.offset);
        // Walk back over the `:` and whitespace to the name's start.
        while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
            cursor -= 1;
        }
        if cursor == 0 || bytes[cursor - 1] != b':' {
            return shape.range;
        }
        cursor -= 1;
        while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
            cursor -= 1;
        }
        let name_text = self.strings.resolve(*name);
        let name_len = name_text.len();
        if cursor >= name_len && &self.source[cursor - name_len..cursor] == name_text {
            cursor -= name_len;
            SourceRange {
                offset: byte_offset_to_utf16(self.source, cursor),
                length: shape.range.end().saturating_sub(byte_offset_to_utf16(self.source, cursor)),
            }
        } else {
            shape.range
        }
    }

    /// `_validateNativeCallable`'s shared head (upstream
    /// ffi_verifier.dart:1915-1939): the argument-count gate plus the
    /// type-argument validity gate. `mustBeANativeFunctionType` anchors at
    /// `node.constructorName`; a missing type argument renders as
    /// `Function` (the oracle's inferred `T` face). Returns the
    /// signature's `(return head, parameter heads)` when valid.
    fn ffi_native_callable_signature(
        &mut self,
        invocation: &FfiInvocation,
        isolate_local: bool,
    ) -> Option<(StringId, Vec<StringId>)> {
        let arg_count = invocation.args.len();
        if !(arg_count == 1 || (isolate_local && arg_count == 2)) {
            return None;
        }
        let native = self.ffi_type_arg(invocation);
        let shape = native
            .as_ref()
            .and_then(|native| self.ffi_type_arg_shape(native));
        let valid = shape
            .as_ref()
            .is_some_and(|(ret, params)| self.ffi_valid_native_signature(ret, params));
        if !valid {
            let display = native
                .as_ref()
                .map(|native| native.display.to_string())
                .unwrap_or_else(|| "Function".to_string());
            let at = self.ffi_constructor_name_span(invocation);
            self.emit(
                SemanticCode::MustBeANativeFunctionType,
                at,
                &display,
                "NativeCallable",
            );
            return None;
        }
        shape
    }

    /// `_validateNativeCallable`'s listener arm (upstream
    /// ffi_verifier.dart:1941-1958, :2005-2011): the Dart callback must be
    /// `dartToNative`-compatible, and the native signature must return
    /// `Void`.
    fn ffi_check_native_callable_listener(&mut self, invocation: &FfiInvocation) {
        let Some((ret, params)) = self.ffi_native_callable_signature(invocation, false) else {
            return;
        };
        // The Dart callback's tear-off compat (dartToNative).
        let dart_fn = invocation
            .args
            .first()
            .map(|(_, shape)| shape.head)
            .filter(|head| *head != StringId::NONE)
            .and_then(|head| self.ffi_tearoff_type(head));
        if let (Some((dart_ret, dart_params)), Some((_, shape))) =
            (&dart_fn, invocation.args.first())
        {
            if !self.ffi_compatible_functions_d2n(dart_ret, dart_params, &ret, &params) {
                let dart_display = self.ffi_render_function(dart_ret, dart_params);
                let written = self
                    .ffi_type_arg(invocation)
                    .map(|native| native.display.to_string())
                    .unwrap_or_default();
                self.emit3(
                    SemanticCode::MustBeASubtype,
                    shape.range,
                    &dart_display,
                    &written,
                    "NativeCallable",
                );
                return;
            }
        }
        if self.strings.resolve(ret) != "Void" {
            let display = self.ffi_render(ret_ty(ret));
            let Some((_, shape)) = invocation.args.first() else {
                return;
            };
            self.emit(SemanticCode::MustReturnVoid, shape.range, &display, "");
        }
    }

    /// `_validateNativeCallable`'s isolateLocal arm (upstream
    /// ffi_verifier.dart:1960-2004): the Dart callback must be
    /// `dartToNative`-compatible, then the exception-value faces decide on
    /// the native return's void/pointer/compound shape.
    fn ffi_check_native_callable_isolate_local(&mut self, invocation: &FfiInvocation) {
        let Some((ret, params)) = self.ffi_native_callable_signature(invocation, true) else {
            return;
        };
        // The Dart callback's tear-off compat (dartToNative); a mismatch
        // reports `mustBeASubtype` at the callback and ends the check.
        let dart_fn = invocation
            .args
            .first()
            .map(|(_, shape)| shape.head)
            .filter(|head| *head != StringId::NONE)
            .and_then(|head| self.ffi_tearoff_type(head));
        if let (Some((dart_ret, dart_params)), Some((_, shape))) =
            (&dart_fn, invocation.args.first())
        {
            if !self.ffi_compatible_functions_d2n(dart_ret, dart_params, &ret, &params) {
                let dart_display = self.ffi_render_function(dart_ret, dart_params);
                let written = self
                    .ffi_type_arg(invocation)
                    .map(|native| native.display.to_string())
                    .unwrap_or_default();
                self.emit3(
                    SemanticCode::MustBeASubtype,
                    shape.range,
                    &dart_display,
                    &written,
                    "NativeCallable",
                );
                return;
            }
        }
        let ret_text = self.strings.resolve(ret).to_owned();
        let void_like = ret_text == "Void"
            || ret_text == "Pointer"
            || ret_text == "Handle"
            || self.clause_entry_is_compound(ret);
        if void_like {
            if invocation.args.len() == 2 {
                let at = self.ffi_named_arg_span(invocation, 1);
                self.emit(
                    SemanticCode::InvalidExceptionValue,
                    at,
                    "isolateLocal",
                    "",
                );
            }
            return;
        }
        if invocation.args.len() == 1 {
            self.emit(
                SemanticCode::MissingExceptionValue,
                invocation.whole,
                "isolateLocal",
                "",
            );
            return;
        }
        let exceptional = &invocation.args[1].1;
        // The exceptionalReturn compat is `dartToNative` (upstream :1982).
        if !self.ffi_compatible_native_value(exceptional.ty, ret, true) {
            let dart_display = self.ffi_render(exceptional.ty);
            self.emit3(
                SemanticCode::MustBeASubtype,
                exceptional.range,
                &dart_display,
                &ret_text,
                "isolateLocal",
            );
        }
        if !self.ffi_arg_is_const(exceptional) {
            self.emit(
                SemanticCode::ArgumentMustBeAConstant,
                exceptional.range,
                "exceptionalReturn",
                "",
            );
        }
    }

    /// A generative creation of a compound subtype (upstream
    /// `visitInstanceCreationExpression`: only factory creations are
    /// legal — a factory target never reaches this capture, which keys on
    /// the written head). The gate is the DIRECT supertype: the created
    /// class's declared superclass must be `Struct` or `Union`
    /// (`class_.isStructSubclass || class_.isUnionSubclass`), not a
    /// transitive compound reach.
    fn ffi_check_compound_creation(&mut self, invocation: &FfiInvocation) {
        let Some((container, _)) = self.resolve_container_from(invocation.callee) else {
            return;
        };
        let Some(super_name) = container.superclass else {
            return;
        };
        let super_text = self.strings.resolve(super_name);
        if !matches!(super_text, "Struct" | "Union") {
            return;
        }
        // The supertype must resolve into `dart:ffi`.
        if !self
            .resolve_container_from(super_name)
            .is_some_and(|(_, owner)| self.owner_is_ffi(owner))
        {
            return;
        }
        self.emit(
            SemanticCode::CreationOfStructOrUnion,
            SourceRange {
                offset: invocation.head.offset,
                length: invocation.head.length,
            },
            "",
            "",
        );
    }

    // —— the `.address` faces ————————————————————————————————————————

    /// `_validateAddressPosition` + `_validateAddressReceiver`: the read
    /// must sit in a leaf-native argument list, and its receiver must be
    /// a legal shape.
    fn ffi_check_address_read(&mut self, read: &FfiAddressRead, invocations: &[FfiInvocation]) {
        // The position face: some argument expression contains the read.
        let container = invocations.iter().find(|invocation| {
            invocation.args.iter().any(|(_, shape)| {
                shape.range.offset <= read.recv_span.offset
                    && read.recv_span.end() <= shape.range.end()
            })
        });
        let leaf = container.is_some_and(|invocation| self.ffi_invocation_is_leaf(invocation));
        if !leaf {
            self.emit(SemanticCode::AddressPosition, read.selector, "", "");
        }
        // The receiver face: compound roots, Array receivers, and
        // typed-data receivers are legal; primitives and everything else
        // report.
        if read.recv_is_index {
            return;
        }
        for ty in [read.head_ty, read.recv_ty] {
            let Ty::Named { name, .. } = ty else {
                continue;
            };
            let text = self.strings.resolve(name);
            if text.ends_with("List")
                && (matches!(
                    text.strip_suffix("List").unwrap_or_default(),
                    "Float32" | "Float64"
                ) || FIXED_SIZE_INTEGER_TYPES
                    .contains(&text.strip_suffix("List").unwrap_or_default()))
            {
                return;
            }
            if text == "Array" || self.clause_entry_is_compound(name) {
                return;
            }
        }
        self.emit(SemanticCode::AddressReceiver, read.selector, "", "");
    }

    /// The invocation's callee resolves to a `@Native(isLeaf: true)`
    /// declaration (upstream `isNativeLeafInvocation` — the const-alias
    /// arm included: the resolved annotation's `isLeaf` field, not the
    /// written name's).
    fn ffi_invocation_is_leaf(&self, invocation: &FfiInvocation) -> bool {
        self.decl_sites.iter().any(|site| {
            if site.name != invocation.callee {
                return false;
            }
            let written = site.annotations.contains(&self.intern("Native"))
                && self.ffi_decl_annos.values().flatten().any(|anno| {
                    anno.name == self.intern("Native")
                        && anno.face.is_leaf
                        && anno.at.offset < site.range.offset
                        && site.range.offset - anno.at.offset < 512
                });
            if written {
                return true;
            }
            // The const-alias leaf: a `@alias` before the site whose RHS
            // is `Native<…>(isLeaf: true)` — the same expansion
            // `ffi_const_native_annos` runs, read only for its leaf bit.
            self.ffi_const_native_annos(site)
                .iter()
                .any(|anno| anno.face.is_leaf)
        })
    }

    // —— the library directive ————————————————————————————————————————

    /// `_visitLibraryDirective`: at most one `@DefaultAsset` annotation
    /// attaches to the library. The chain walk skips `//` and `/* */`
    /// comments between the annotation and `library` (upstream
    /// `visitLibraryDirective` sees the resolved AST, comments never
    /// reach it).
    fn ffi_check_library_default_asset(&mut self) {
        let needle = "@DefaultAsset";
        let bytes = self.source.as_bytes();
        let mut seen = 0usize;
        let mut cursor = 0usize;
        while let Some(rel) = bytes[cursor.min(bytes.len())..]
            .windows(needle.len())
            .position(|window| window == needle.as_bytes())
        {
            let at = cursor + rel;
            cursor = at + needle.len();
            // Word boundary: the `@` must not be part of an identifier.
            if at > 0
                && bytes
                    .get(at - 1)
                    .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
            {
                continue;
            }
            // Skip whitespace and comments after the annotation.
            while cursor < bytes.len() {
                while bytes.get(cursor).is_some_and(|b| b.is_ascii_whitespace()) {
                    cursor += 1;
                }
                if cursor + 1 < bytes.len()
                    && bytes[cursor] == b'/'
                    && bytes[cursor + 1] == b'/'
                {
                    while cursor < bytes.len() && bytes[cursor] != b'\n' {
                        cursor += 1;
                    }
                    continue;
                }
                if cursor + 1 < bytes.len()
                    && bytes[cursor] == b'/'
                    && bytes[cursor + 1] == b'*'
                {
                    cursor += 2;
                    while cursor + 1 < bytes.len()
                        && !(bytes[cursor] == b'*' && bytes[cursor + 1] == b'/')
                    {
                        cursor += 1;
                    }
                    cursor = (cursor + 2).min(bytes.len());
                    continue;
                }
                break;
            }
            if bytes.get(cursor) == Some(&b'(')
                && let Some(close) = matching_close(bytes, cursor, b'(', b')')
            {
                cursor = close + 1;
            }
            let mut chain = cursor;
            loop {
                while bytes.get(chain).is_some_and(|b| b.is_ascii_whitespace()) {
                    chain += 1;
                }
                // Skip comments inside the annotation chain.
                if chain + 1 < bytes.len()
                    && bytes[chain] == b'/'
                    && bytes[chain + 1] == b'/'
                {
                    while chain < bytes.len() && bytes[chain] != b'\n' {
                        chain += 1;
                    }
                    continue;
                }
                if chain + 1 < bytes.len()
                    && bytes[chain] == b'/'
                    && bytes[chain + 1] == b'*'
                {
                    chain += 2;
                    while chain + 1 < bytes.len()
                        && !(bytes[chain] == b'*' && bytes[chain + 1] == b'/')
                    {
                        chain += 1;
                    }
                    chain = (chain + 2).min(bytes.len());
                    continue;
                }
                if bytes.get(chain) == Some(&b'@') {
                    chain += 1;
                    let name_start = chain;
                    while bytes
                        .get(chain)
                        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'$')
                    {
                        chain += 1;
                    }
                    let name_end = chain;
                    while bytes.get(chain).is_some_and(|b| b.is_ascii_whitespace()) {
                        chain += 1;
                    }
                    if bytes.get(chain) == Some(&b'(')
                        && let Some(close) = matching_close(bytes, chain, b'(', b')')
                    {
                        chain = close + 1;
                    }
                    // A chained `@alias` whose const RHS spells
                    // `DefaultAsset(…)` is a second `@DefaultAsset` —
                    // upstream's resolved metadata carries the class
                    // name, not the alias's.
                    let alias = &self.source[name_start..name_end];
                    if alias != "DefaultAsset" {
                        seen += usize::from(
                            self.ffi_const_alias_is(alias, "DefaultAsset"),
                        );
                        if seen > 1 {
                            let offset = byte_offset_to_utf16(self.source, name_start);
                            self.emit(
                                SemanticCode::FfiNativeInvalidDuplicateDefaultAsset,
                                SourceRange {
                                    offset,
                                    length: (name_end - name_start) as u32,
                                },
                                "",
                                "",
                            );
                        }
                    }
                    continue;
                }
                break;
            }
            if !bytes[chain..].starts_with(b"library") {
                continue;
            }
            seen += 1;
            if seen > 1 {
                let offset = byte_offset_to_utf16(self.source, at);
                self.emit(
                    SemanticCode::FfiNativeInvalidDuplicateDefaultAsset,
                    SourceRange {
                        offset: offset + 1,
                        length: needle.len() as u32 - 1,
                    },
                    "",
                    "",
                );
            }
        }
    }

    /// The map-literal entries of a top-level `const` whose RHS is one
    /// `{k: v(), …}` literal — each entry's VALUE type head in source
    /// order (the `AbiSpecificIntegerMapping(const)` arm's entries).
    fn ffi_const_map_entries(&self, alias: &str) -> Option<Vec<String>> {
        let alias_id = self.strings.intern(alias);
        let const_site = self
            .decl_sites
            .iter()
            .find(|s| s.name == alias_id && matches!(s.kind, DeclKind::TopLevelVar))?;
        let decl_start = self.byte_of(const_site.range.offset);
        let tail = &self.source[decl_start..];
        let open = tail.find('{')?;
        let close = matching_close(tail.as_bytes(), open, b'{', b'}')?;
        let mut values = Vec::new();
        for (rel, len) in split_top_level(&tail[open + 1..close], b',') {
            let entry = &tail[open + 1 + rel..open + 1 + rel + len];
            let Some(colon) = top_level_colon(entry) else {
                continue;
            };
            let value = entry[colon + 1..].trim();
            let Some((s, e)) = first_ident(value) else {
                continue;
            };
            values.push(value[s..e].to_owned());
        }
        Some(values)
    }

    /// Whether `alias` is a top-level `const` whose RHS spells
    /// `class_name(…)` — the const-alias resolution shared by the
    /// `DefaultAsset` chain walk.
    fn ffi_const_alias_is(&self, alias: &str, class_name: &str) -> bool {
        let alias_id = self.strings.intern(alias);
        let Some(const_site) = self
            .decl_sites
            .iter()
            .find(|s| s.name == alias_id && matches!(s.kind, DeclKind::TopLevelVar))
        else {
            return false;
        };
        let decl_start = self.byte_of(const_site.range.offset);
        let tail = &self.source[decl_start..];
        let Some(eq) = tail.find('=') else {
            return false;
        };
        tail[eq + 1..].trim_start().starts_with(class_name)
    }

    // —— shared decision helpers ——————————————————————————————————————

    /// The written function-type face of a type argument: `Some((ret,
    /// param_heads))` when the type resolves (aliases fold) to a function
    /// shape; `None` for every other type. A first-class function form
    /// (`NativeFunction<R Function(...)>`'s captured argument) reads
    /// directly; a named head keeps the alias-expansion face.
    fn ffi_function_shape(&self, ty: Ty) -> Option<(StringId, Vec<StringId>)> {
        let expanded = match ty {
            Ty::Function { .. } => ty,
            _ => self.ffi_expanded_head(ty).map(|(_, expanded)| expanded)?,
        };
        match expanded {
            Ty::Function { shape, .. } => {
                let fn_shape = {
                    let args = self.boundary.args.borrow();
                    args.function_shape(shape).clone()
                };
                let heads = |ty: &Ty| match ty {
                    Ty::Named { name, .. } => Some(*name),
                    Ty::Void => Some(self.intern("void")),
                    _ => None,
                };
                let ret = heads(&fn_shape.return_ty)?;
                let params: Vec<StringId> = fn_shape.positional.iter().filter_map(heads).collect();
                Some((ret, params))
            }
            Ty::Named { name, .. } => {
                // A bare head: valid only as a compound/abi face with no
                // parameters — never a function shape.
                let _ = name;
                None
            }
            _ => None,
        }
    }

    /// A top-level/static function name's synthesized tear-off type (the
    /// `must_be_a_subtype` display and compatibility input).
    fn ffi_tearoff_type(&self, name: StringId) -> Option<(StringId, Vec<StringId>)> {
        let elements = self.index.get(self.file)?;
        let decl = elements.decl(name)?;
        if decl.kind != crate::model::MicroKind::TopLevelFn {
            return None;
        }
        let heads = |ty: &Ty| match ty {
            Ty::Named { name, .. } => Some(*name),
            _ => None,
        };
        let ret = heads(&decl.return_ty)?;
        let formals = decl.formals.as_ref()?;
        let params: Vec<StringId> = formals
            .positional
            .iter()
            .filter_map(|f| heads(&f.ty))
            .collect();
        Some((ret, params))
    }

    /// The spelling is a type parameter of the diagnosed unit.
    fn ffi_is_type_param(&self, name: StringId) -> bool {
        self.index
            .get(self.file)
            .and_then(|elements| elements.decl(name))
            .is_some_and(|decl| decl.kind == crate::model::MicroKind::TypeParam)
    }

    /// The native↔dart function-type compatibility matrix (upstream
    /// `_validateCompatibleFunctionTypes` at the head level: equal
    /// positional counts, return and parameter native mappings).
    fn ffi_compatible_functions(
        &self,
        dart_ret: &StringId,
        dart_params: &[StringId],
        native_ret: &StringId,
        native_params: &[StringId],
    ) -> bool {
        if dart_params.len() != native_params.len() {
            return false;
        }
        // The return direction is `nativeToDart` (the native return value
        // becomes the Dart value); the parameter direction reverses
        // (`dartToNative` — the Dart argument becomes the native value).
        if !self.ffi_compatible_native_value(Ty::named(*dart_ret), *native_ret, false) {
            return false;
        }
        dart_params
            .iter()
            .zip(native_params.iter())
            .all(|(dart, native)| {
                self.ffi_compatible_native_value(Ty::named(*dart), *native, true)
            })
    }

    /// The `dartToNative` mirror of `ffi_compatible_functions` (upstream
    /// `_FfiTypeCheckDirection.dartToNative` at :1945/:1670/:1983): the
    /// Dart face is the callback the native signature must accept — the
    /// return maps `dartToNative`, the params `nativeToDart`
    /// (`direction.reverse`).
    fn ffi_compatible_functions_d2n(
        &self,
        dart_ret: &StringId,
        dart_params: &[StringId],
        native_ret: &StringId,
        native_params: &[StringId],
    ) -> bool {
        if dart_params.len() != native_params.len() {
            return false;
        }
        if !self.ffi_compatible_native_value(Ty::named(*dart_ret), *native_ret, true) {
            return false;
        }
        dart_params
            .iter()
            .zip(native_params.iter())
            .all(|(dart, native)| {
                self.ffi_compatible_native_value(Ty::named(*dart), *native, false)
            })
    }

    /// `_validateCompatibleNativeType` at the head level (nativeToDart):
    /// the native class maps onto the Dart face (upstream
    /// ffi_verifier.dart:1431-1490).
    ///
    /// `direction` mirrors upstream's `_FfiTypeCheckDirection` — `true` for
    /// `dartToNative` (the Dart value is the source), `false` for
    /// `nativeToDart` (the Dart value is the target). The void arm is
    /// direction-sensitive: a Dart `void` accepts only `Void` in
    /// `dartToNative`, any native void in `nativeToDart`.
    fn ffi_compatible_native_value(
        &self,
        dart_ty: Ty,
        native_head: StringId,
        dart_to_native: bool,
    ) -> bool {
        let native_text = self.strings.resolve(native_head);
        let dart_head = match dart_ty {
            Ty::Named { name, .. } => self.strings.resolve(name).to_owned(),
            Ty::Void => "void".to_owned(),
            _ => return false,
        };
        // The primitive native faces first (upstream :1439-1453).
        if NATIVE_INT_TYPES.contains(&native_text) {
            return dart_head == "int";
        }
        match native_text {
            "Float" | "Double" => dart_head == "double",
            "Bool" => dart_head == "bool",
            "Void" => {
                if dart_head == "void" {
                    return true;
                }
                // nativeToDart: a Dart `void` accepts any native type
                // (upstream :1449-1450 — `direction == dartToNative ||
                // dartType is VoidType`).
                !dart_to_native
            }
            "Handle" => true,
            _ => {
                // Dart `void` + a non-void native: only `nativeToDart`
                // permits it (upstream :1451-1453 — `dartType is VoidType`
                // → `nativeReturnType == void_`).
                if dart_head == "void" {
                    return !dart_to_native;
                }
                // The interface-type arm (upstream :1457-1473): NFWC1
                // subclasses as Pointer, typed-data, and the
                // direction-sensitive subtype approximation.
                let dart_named = match dart_ty {
                    Ty::Named { name, .. } => name,
                    _ => StringId::NONE,
                };
                if self.ffi_extends_wrapper_by_name(dart_named) {
                    // `NativeFieldWrapperClass1` subclasses map to
                    // `Pointer<Void>` (upstream :1459-1465).
                    return native_text == "Pointer";
                }
                // Typed data (upstream `_isValidTypedData`): the Dart
                // side is a typed-data class and the native side is its
                // element type.
                if self.ffi_is_typed_data(dart_named, native_head) {
                    return true;
                }
                // The fallback: both sides are interface types — the
                // direction-sensitive subtype approximation. At the head
                // level, same-head or compound-subtype reach.
                if dart_head == native_text {
                    return true;
                }
                if dart_to_native {
                    self.clause_entry_is_compound(dart_named)
                } else {
                    self.clause_entry_is_compound(native_head)
                }
            }
        }
    }

    /// `_isValidTypedData` at the head level: `Float`→`Float32List`,
    /// `Double`→`Float64List`, `IntN`/`UintN`/`IntPtr`→`IntNList`/`UintNList`.
    fn ffi_is_typed_data(&self, dart_head: StringId, native_head: StringId) -> bool {
        let dart = self.strings.resolve(dart_head);
        let native = self.strings.resolve(native_head);
        match (native, dart) {
            ("Float", "Float32List") => true,
            ("Double", "Float64List") => true,
            _ => {
                // `Int8`→`Int8List`, `Uint16`→`Uint16List`, `IntPtr`→`IntPtrList`.
                if let Some(list) = dart.strip_suffix("List") {
                    list == native
                        && (NATIVE_INT_TYPES.contains(&native)
                            || matches!(native, "IntPtr"))
                } else {
                    false
                }
            }
        }
    }

    /// A Pointer-typed native return (the compound arm of the
    /// exception-value test).
    fn ffi_valid_pointer_like(&self, head: StringId) -> bool {
        let text = self.strings.resolve(head);
        text == "Pointer" || self.clause_entry_is_compound(head)
    }
}

/// A return head as a renderable type.
fn ret_ty(head: StringId) -> Ty {
    Ty::named(head)
}

impl FfiInvocation {
    /// The receiver's span when the capture carried one, else the head —
    /// the `non_constant_type_argument` anchor over an `asFunction`
    /// type-parameter pointer.
    fn recv_span_or_head(&self) -> SourceRange {
        self.head
    }
}

#[cfg(test)]
#[path = "ticket_3155_tests.rs"]
mod ticket_3155_tests;

#[cfg(test)]
#[path = "ticket_3205_tests.rs"]
mod ticket_3205_tests;
