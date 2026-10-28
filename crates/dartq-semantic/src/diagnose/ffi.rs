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
                || FIXED_SIZE_INTEGER_TYPES.contains(
                    &text.strip_suffix("List").unwrap_or_default(),
                )
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
    fn chain_reaches_ffi(
        &self,
        start: &[(StringId, &'a Container, FileId)],
    ) -> bool {
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
            if let Some((super_container, super_owner)) =
                self.resolve_container_from(super_name)
            {
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
        let (super_container, owner) = self.resolve_container_from(super_name)?;
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
                        "variable" | "variableMulti" | "variableWithVariableDimension"
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
                // head is the constructed native type.
                let Some(open) = args_text.find('{') else {
                    return (face, Some(args_span));
                };
                let Some(brace_close) =
                    matching_close(args_text.as_bytes(), open, b'{', b'}')
                else {
                    return (face, Some(args_span));
                };
                for (rel, len) in split_top_level(&args_text[open + 1..brace_close], b',') {
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
                while bytes.get(end).is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_') {
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

/// Whether the named argument reads `name: true` at the group's top
/// level (the `_isLeaf` source walk).
fn named_arg_is_true(args_text: &str, name: &str) -> bool {
    for (rel, len) in split_top_level(args_text, b',') {
        let arg = &args_text[rel..rel + len];
        let Some(colon) = arg.find(':') else {
            continue;
        };
        if arg[..colon].trim() == name && arg[colon + 1..].trim() == "true" {
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
    match suffix {
        "variable" | "variableMulti" => {
            face.variable_length = true;
            face.dims.push((
                0,
                SourceRange {
                    offset: utf16(0),
                    length: 0,
                },
            ));
            let Some(close) = matching_close(bytes, 0, b'(', b')') else {
                return;
            };
            let inner = &args_text[1..close];
            // `variableMulti` wraps its items in a list literal.
            let (items, items_at) = if suffix == "variableMulti" {
                let Some(bracket) = inner.find('[') else {
                    return;
                };
                let Some(bracket_close) = matching_close(inner.as_bytes(), bracket, b'[', b']')
                else {
                    return;
                };
                (&inner[bracket + 1..bracket_close], bracket + 1)
            } else {
                (inner, 0)
            };
            for (rel, len) in split_top_level(items, b',') {
                let item_full = &items[rel..rel + len];
                let item = item_full.trim();
                let Some(value) = parse_int_literal(item) else {
                    continue;
                };
                let lead = item_full.len() - item_full.trim_start().len();
                let at = 1 + items_at + rel + lead;
                face.dims.push((
                    value,
                    SourceRange {
                        offset: utf16(at),
                        length: utf16(at + item.len()) - utf16(at),
                    },
                ));
            }
        }
        "variableWithVariableDimension" => {
            face.variable_length = true;
            let Some(close) = matching_close(bytes, 0, b'(', b')') else {
                return;
            };
            let inner = &args_text[1..close];
            let item = inner.trim();
            let Some(value) = parse_int_literal(item) else {
                return;
            };
            face.dims.push((
                value,
                SourceRange {
                    offset: utf16(1 + (inner.len() - inner.trim_start().len())),
                    length: utf16(1 + (inner.len() - inner.trim_start().len()) + item.len())
                        - utf16(1 + (inner.len() - inner.trim_start().len())),
                },
            ));
        }
        _ => {
            let Some(close) = matching_close(bytes, 0, b'(', b')') else {
                return;
            };
            let inner = &args_text[1..close];
            for (rel, len) in split_top_level(inner, b',') {
                let item_full = &inner[rel..rel + len];
                let item = item_full.trim();
                let Some(value) = parse_int_literal(item) else {
                    continue;
                };
                let lead = item_full.len() - item_full.trim_start().len();
                face.dims.push((
                    value,
                    SourceRange {
                        offset: utf16(1 + rel + lead),
                        length: utf16(1 + rel + lead + item.len()) - utf16(1 + rel + lead),
                    },
                ));
            }
        }
    }
}

// —— the post-parse pass ———————————————————————————————————————————

/// The keyword set whose presence before a field name means no written
/// type annotation (`external var str;`).
const NO_TYPE_KEYWORDS: &[&str] = &[
    "var", "final", "const", "external", "static", "late", "covariant", "abstract",
];

/// The native integer classes the field/annotation classifier maps onto
/// the Dart `int` face (upstream `_primitiveIntegerNativeTypes`).
const NATIVE_INT_TYPES: &[&str] = &[
    "Int8", "Int16", "Int32", "Int64", "Uint8", "Uint16", "Uint32", "Uint64", "IntPtr", "Int",
    "Uint", "Long", "LongLong", "ULong", "ULongLong", "Short", "SignedChar", "Char", "Size",
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
        while cursor > 0 && bytes.get(cursor - 1).is_some_and(|b| b.is_ascii_whitespace()) {
            cursor -= 1;
        }
        if cursor == 0 {
            return None;
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
            while head_end > 0 && bytes.get(head_end - 1).is_some_and(|b| b.is_ascii_whitespace())
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
            let close16 = byte_offset_to_utf16(self.source, cursor);
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
        let end16 = byte_offset_to_utf16(self.source, cursor);
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
            - text[open + 1 + rel..open + 1 + rel + len].trim_start().len();
        let arg_start = start + open + 1 + rel + lead;
        let arg_end = arg_start
            + text[open + 1 + rel..open + 1 + rel + len].trim().len();
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
            .is_some_and(|(super_container, _)| self.ffi_extends_native_field_wrapper(super_container))
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
                self.emit(
                    SemanticCode::EmptyStruct,
                    name_span,
                    &name_text,
                    direct,
                );
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
    fn ffi_check_clause_entries(&mut self, name: StringId, container: &Container) {
        let name_text = self.strings.resolve(name).to_owned();
        let Some(spans) = self.ffi_clause_spans.get(&name) else {
            return;
        };
        let spans = spans.clone();
        // The extends entry: report only when the head is NOT itself an
        // ffi class (the direct arm consumed that shape).
        if let Some(span) = spans.extends
            && let Some(super_name) = container.superclass
            && !self.ffi_class_spelling(super_name)
            && self.clause_entry_is_compound(super_name)
        {
            let super_text = self.strings.resolve(super_name).to_owned();
            self.emit(
                SemanticCode::SubtypeOfStructClassInExtends,
                span,
                &name_text,
                &super_text,
            );
        }
        let implements = spans
            .implements
            .iter()
            .zip(container.implements_clause.iter());
        for (span, entry) in implements {
            let text = self.strings.resolve(*entry);
            if matches!(text, "Allocator" | "Finalizable") {
                continue;
            }
            if self.clause_entry_is_compound(*entry) {
                let super_text = text.to_owned();
                self.emit(
                    SemanticCode::SubtypeOfStructClassInImplements,
                    *span,
                    &name_text,
                    &super_text,
                );
            }
        }
        let withs = spans.withs.iter().zip(container.with_clause.iter());
        for (span, entry) in withs {
            if self.clause_entry_is_compound(*entry) {
                let super_text = self.strings.resolve(*entry).to_owned();
                self.emit(
                    SemanticCode::SubtypeOfStructClassInWith,
                    *span,
                    &name_text,
                    &super_text,
                );
            }
        }
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
        let name_text = self.strings.resolve(name).to_owned();
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
            let Some(member) = elements.container(name).and_then(|c| c.members.get(&site.name))
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
            self.emit(
                SemanticCode::FieldMustBeExternalInStruct,
                name_span,
                "",
                "",
            );
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
            self.emit(
                SemanticCode::MissingFieldTypeInStruct,
                name_span,
                "",
                "",
            );
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
                let allow_variable =
                    self.ffi_last_external_field(container, sites)
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
                        && let Some((compound_container, _)) =
                            self.resolve_container_from(compound)
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
                    || self
                        .strings
                        .resolve(anno.name)
                        .starts_with("Array.")
            })
            .cloned()
            .collect();
        if array_annos.is_empty() {
            self.emit(
                SemanticCode::MissingSizeAnnotationCarray,
                type_span,
                "",
                "",
            );
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
            || matches!(head.as_str(), "Float" | "Double" | "Bool" | "Pointer" | "Array")
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
    /// walks the top-level sites.
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
                _ => false,
            })
            .map(|(index, site)| (index, site.clone()))
            .collect();
        for (index, site) in sites.iter() {
            let native_annos: Vec<FfiDeclAnno> = self
                .ffi_native_annos(*index)
                .into_iter()
                .cloned()
                .collect();
            if native_annos.is_empty() {
                continue;
            }
            if native_annos.len() > 1 {
                for extra in native_annos.iter().skip(1) {
                    self.emit(
                        SemanticCode::FfiNativeInvalidMultipleAnnotations,
                        extra.face.name_span,
                        "",
                        "",
                    );
                }
            }
            let mut native_annos = native_annos;
            let anno = native_annos.remove(0);
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
            DeclKind::Member { is_static: false, .. }
        );
        let (var_ty, return_ty, formals) = self.ffi_decl_types(site);
        let Some(sig) = anno.face.sig.as_ref() else {
            // The omitted `<T>`: infer from the Dart face (static
            // executables only — upstream's `DynamicType` arm).
            if instance_member {
                return;
            }
            let is_executable = matches!(
                site.kind,
                DeclKind::TopLevelFn
            ) || matches!(
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
                        _ => StringId::NONE,
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
    fn ffi_decl_types(
        &self,
        site: &DeclSite,
    ) -> (Ty, Ty, Option<&'a Box<Formals>>) {
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
                let member = elements
                    .container(*container)
                    .and_then(|c| c.members.get(&site.name));
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
    /// validity faces over the written signature.
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
        let sig_param_count = sig.param_heads.len();
        let ret_head = first_ident_head(&sig.display, self);
        if face.is_leaf {
            self.ffi_leaf_check(&ret_head, &sig.param_heads, site.range);
        }
        if instance_member {
            if dart_param_count + 1 != sig_param_count {
                let (a0, a1) = (
                    (dart_param_count + 1).to_string(),
                    sig_param_count.to_string(),
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
            if sig.param_heads.first() == Some(&self.intern("Pointer"))
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
        } else if dart_param_count != sig_param_count {
            let (a0, a1) = (sig_param_count.to_string(), dart_param_count.to_string());
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
        let param_offset = usize::from(instance_member);
        if let Some(formals) = formals {
            for (i, head) in sig.param_heads.iter().enumerate().skip(param_offset) {
                if self.strings.resolve(*head) != "Pointer" {
                    continue;
                }
                let formal_ty = formals
                    .positional
                    .get(i - param_offset)
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
        let param_heads: Vec<StringId> = sig.param_heads.to_vec();
        if !self.ffi_valid_native_signature(&ret_head, &param_heads) {
            self.emit(
                SemanticCode::MustBeANativeFunctionType,
                site.range,
                &sig.display,
                "Native",
            );
        }
    }

    /// `NativeFieldWrapperClass1` reachability by container name.
    fn ffi_extends_wrapper_by_name(&self, name: StringId) -> bool {
        self.resolve_container_from(name)
            .is_some_and(|(container, _)| self.ffi_extends_native_field_wrapper(container))
    }

    /// `_checkFfiNativeField`: the static posture, the omitted-signature
    /// inference, and the Handle/NativeFunction rejections.
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
        let Ty::Named { name: dart_head, .. } = var_ty else {
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
        if matches!(native_text, "Handle" | "NativeFunction") {
            let display = self.ffi_render(*native_ty);
            self.emit(
                SemanticCode::NativeFieldInvalidType,
                site.range,
                &display,
                "",
            );
            return;
        }
        if native_text == "Array" {
            // Array native fields need an `@Array` size annotation.
            self.ffi_validate_size_annotation(site_index, 1, false, site.range);
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

    /// `_isValidFfiNativeType` at the head level: native primitives, the
    /// void/handle gates, and compound/abi subtypes.
    fn ffi_valid_native_type(&self, head: StringId, allow_void: bool, allow_handle: bool) -> bool {
        let text = self.strings.resolve(head);
        match text {
            "Void" => allow_void,
            "Handle" => allow_handle,
            "Dynamic" => false,
            _ if NATIVE_INT_TYPES.contains(&text) => true,
            _ if matches!(text, "Float" | "Double" | "Bool") => true,
            "Pointer" | "Opaque" => true,
            _ => self.clause_entry_is_compound(head),
        }
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
            if !self.ffi_compatible_functions(dart_ret, dart_params, &ret_head, &native_params) {
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
        let void_like = matches!(native_ret_text.as_str(), "Void")
            || self.ffi_valid_pointer_like(ret_head);
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
            self.emit(SemanticCode::MissingExceptionValue, invocation.head, callee, "");
            return;
        }
        let exceptional = &invocation.args[1].1;
        let compatible = self.ffi_compatible_native_value(exceptional.ty, ret_head);
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
        if exceptional.invocation_shaped
            && exceptional.head != StringId::NONE
            && !self.const_ctor_target(exceptional.head)
        {
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
                && shape.invocation_shaped
                && !self.const_ctor_target(shape.head)
            {
                self.emit(
                    SemanticCode::ArgumentMustBeAConstant,
                    shape.range,
                    "isLeaf",
                    "",
                );
            }
        }
        // The written Dart signature must be subtype-compatible.
        if let Some(written) = self.ffi_type_arg(invocation)
            && let Some((dart_ret, dart_params)) = self.ffi_type_arg_shape(&written)
        {
            if !self.ffi_compatible_functions(
                &dart_ret,
                &dart_params,
                &sig_ret,
                &sig_params,
            ) {
                let dart_display = self.ffi_render_function(&dart_ret, &dart_params);
                let native_display = self.ffi_render_function(&sig_ret, &sig_params);
                let written = written.display.to_string();
                self.emit3(
                    SemanticCode::MustBeASubtype,
                    invocation.whole,
                    &written,
                    &dart_display,
                    "asFunction",
                );
            }
        }
    }

    /// `_validateLookupFunction`'s native-signature face.
    fn ffi_check_lookup_function(&mut self, invocation: &FfiInvocation) {
        let Some(native) = self.ffi_type_arg(invocation) else {
            return;
        };
        let Some((ret, params)) = self.ffi_type_arg_shape(&native) else {
            self.emit(
                SemanticCode::MustBeANativeFunctionType,
                native.span,
                &native.display,
                "lookupFunction",
            );
            return;
        };
        let is_leaf = invocation.args.iter().any(|(name, shape)| {
            name.is_some_and(|name| self.strings.resolve(name) == "isLeaf")
                && self.ffi_arg_is_true(shape)
        });
        if is_leaf {
            self.ffi_leaf_check(&ret, &params, invocation.head);
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
            self.emit(SemanticCode::NonConstantTypeArgument, invocation.whole, &name, "");
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

    /// `_validateNativeAddressOf`: the argument must name a `@Native`
    /// declaration.
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
            self.emit(
                SemanticCode::ArgumentMustBeNative,
                shape.range,
                "",
                "",
            );
        }
    }

    /// `_validateNativeCallable`'s listener arm: the signature must return
    /// `void`.
    fn ffi_check_native_callable_listener(&mut self, invocation: &FfiInvocation) {
        let Some(native) = self.ffi_type_arg(invocation) else {
            return;
        };
        let Some((ret, _)) = self.ffi_type_arg_shape(&native) else {
            self.emit(
                SemanticCode::MustBeANativeFunctionType,
                native.span,
                &native.display,
                "NativeCallable",
            );
            return;
        };
        if self.strings.resolve(ret) != "Void" {
            let display = self.ffi_render(ret_ty(ret));
            let Some((_, shape)) = invocation.args.first() else {
                return;
            };
            self.emit(SemanticCode::MustReturnVoid, shape.range, &display, "");
        }
    }

    /// `isolateLocal`'s exception-value faces (the fromFunction shape).
    fn ffi_check_native_callable_isolate_local(&mut self, invocation: &FfiInvocation) {
        if invocation.args.is_empty() || invocation.args.len() > 2 {
            return;
        }
        self.ffi_check_from_function(invocation, "isolateLocal");
    }

    /// A generative creation of a compound subtype (upstream
    /// `visitInstanceCreationExpression`: only factory creations are
    /// legal — a factory target never reaches this capture, which keys on
    /// the written head).
    fn ffi_check_compound_creation(&mut self, invocation: &FfiInvocation) {
        if !self.clause_entry_is_compound(invocation.callee) {
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
    fn ffi_check_address_read(
        &mut self,
        read: &FfiAddressRead,
        invocations: &[FfiInvocation],
    ) {
        // The position face: some argument expression contains the read.
        let container = invocations.iter().find(|invocation| {
            invocation.args.iter().any(|(_, shape)| {
                shape.range.offset <= read.recv_span.offset
                    && read.recv_span.end() <= shape.range.end()
            })
        });
        let leaf = container.is_some_and(|invocation| {
            self.ffi_invocation_is_leaf(invocation)
        });
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
    /// declaration (upstream `isNativeLeafInvocation`).
    fn ffi_invocation_is_leaf(&self, invocation: &FfiInvocation) -> bool {
        self.decl_sites.iter().any(|site| {
            site.name == invocation.callee
                && site.annotations.contains(&self.intern("Native"))
                && self
                    .ffi_decl_annos
                    .values()
                    .flatten()
                    .any(|anno| {
                        anno.name == self.intern("Native")
                            && anno.face.is_leaf
                            && anno.at.offset < site.range.offset
                            && site.range.offset - anno.at.offset < 512
                    })
        })
    }

    // —— the library directive ————————————————————————————————————————

    /// `_visitLibraryDirective`: at most one `@DefaultAsset` annotation
    /// attaches to the library.
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
            // The annotation chain after this occurrence walks the
            // sibling annotations (`@DefaultAsset('a') @DefaultAsset('b')
            // library;` — every chain member attaches); a chain ending at
            // any other token is a declaration's annotation.
            while bytes.get(cursor).is_some_and(|b| b.is_ascii_whitespace()) {
                cursor += 1;
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
                if bytes.get(chain) == Some(&b'@') {
                    chain += 1;
                    while bytes
                        .get(chain)
                        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'$')
                    {
                        chain += 1;
                    }
                    while bytes.get(chain).is_some_and(|b| b.is_ascii_whitespace()) {
                        chain += 1;
                    }
                    if bytes.get(chain) == Some(&b'(')
                        && let Some(close) = matching_close(bytes, chain, b'(', b')')
                    {
                        chain = close + 1;
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
                let params: Vec<StringId> =
                    fn_shape.positional.iter().filter_map(heads).collect();
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
        let params: Vec<StringId> =
            formals.positional.iter().filter_map(|f| heads(&f.ty)).collect();
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
        if !self.ffi_compatible_native_value(Ty::named(*dart_ret), *native_ret) {
            return false;
        }
        dart_params
            .iter()
            .zip(native_params.iter())
            .all(|(dart, native)| {
                // The parameter direction reverses: the native type must
                // accept the Dart value.
                self.ffi_compatible_native_value(Ty::named(*dart), *native)
            })
    }

    /// `_validateCompatibleNativeType` at the head level (nativeToDart):
    /// the native class maps onto the Dart face.
    fn ffi_compatible_native_value(&self, dart_ty: Ty, native_head: StringId) -> bool {
        let native_text = self.strings.resolve(native_head);
        let dart_head = match dart_ty {
            Ty::Named { name, .. } => self.strings.resolve(name).to_owned(),
            Ty::Void => "void".to_owned(),
            _ => return false,
        };
        if NATIVE_INT_TYPES.contains(&native_text) {
            return dart_head == "int";
        }
        match native_text {
            "Float" | "Double" => dart_head == "double",
            "Bool" => dart_head == "bool",
            "Void" => dart_head == "void",
            "Handle" => true,
            "Pointer" | "Array" => {
                dart_head == native_text || self.clause_entry_is_compound(match dart_ty {
                    Ty::Named { name, .. } => name,
                    _ => StringId::NONE,
                })
            }
            _ => self.clause_entry_is_compound(match dart_ty {
                Ty::Named { name, .. } => name,
                _ => StringId::NONE,
            }),
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
