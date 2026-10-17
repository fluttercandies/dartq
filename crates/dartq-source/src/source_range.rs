//! The engine's external position convention: UTF-16 based ranges, line
//! indexing, and the byte/UTF-16 conversions.

/// A span of source text: `offset .. offset + length`.
///
/// Both fields are in **UTF-16 code units** — the official Dart analyzer and
/// LSP convention fixed by Byte-oriented internal passes must
/// convert through [`LineIndex`] and [`byte_offset_to_utf16`] before
/// producing a `SourceRange`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceRange {
    /// Range start, in UTF-16 code units from the start of the file.
    pub offset: u32,
    /// Range length, in UTF-16 code units.
    pub length: u32,
}

impl SourceRange {
    /// First position after the range (UTF-16 code units).
    ///
    /// # Panics
    ///
    /// Panics (in all build profiles, including `const` evaluation) when
    /// `offset + length` overflows [`u32`]. The crate-wide contract is to
    /// surface broken offsets, never clamp; a plain `+` only panicked in
    /// debug builds and wrapped silently in release, so the overflow check
    /// is explicit here to keep that contract profile-independent.
    #[must_use]
    pub const fn end(self) -> u32 {
        match self.offset.checked_add(self.length) {
            Some(end) => end,
            None => panic!("SourceRange offset + length overflows u32"),
        }
    }
}

/// Line start offsets of one source text.
///
/// Mirrors the official scanner (`LineInfo.fromContent`): lines end with
/// `\n`, `\r\n`, or `\r`; a `\r\n` pair is one break and the next line starts
/// after the `\n`. Line 1 starts at byte 0, and a trailing line terminator
/// starts a new final (possibly empty) line.
///
/// Binding coordinate decision: `LineIndex` works exclusively in **byte**
/// offsets — [`LineIndex::byte_to_line_col`] reports columns in bytes within
/// the line. Converting to the external UTF-16 convention is the job of
/// [`LineIndex::byte_to_utf16`] / [`LineIndex::utf16_to_byte`] (the indexed,
/// hot-path entry points), [`LineIndex::line_col_utf16`] (the fused
/// diagnostics position entry point), or the free functions
/// [`byte_offset_to_utf16`] / [`utf16_offset_to_byte`] / [`utf16_len_of`]
/// (the semantic fallback for
/// callers that hold no index); both faces agree by construction and the
/// free functions stay the single source of truth for the conversion.
#[derive(Clone, Debug)]
pub struct LineIndex {
    line_starts: Vec<u32>,
    /// Wide-character records for the lines that contain them, ascending by
    /// line number; empty for pure-ASCII sources.
    wide_lines: Vec<WideLine>,
    source_len: u32,
}

/// A non-ASCII (`wide`) character, recorded so that byte↔UTF-16 conversion
/// only touches lines that actually contain such characters. Layout follows
/// the reference implementation in rust-analyzer's `line-index` crate.
#[derive(Clone, Copy, Debug)]
struct WideChar {
    /// Character start, in bytes from the start of its line.
    start: u32,
    /// Character end, in bytes from the start of its line.
    end: u32,
}

impl WideChar {
    fn byte_len(self) -> u32 {
        self.end - self.start
    }

    /// UTF-16 length: only 4-byte (astral) characters take a surrogate pair;
    /// 2–3 byte BMP characters are single units.
    fn utf16_len(self) -> u32 {
        if self.byte_len() == 4 { 2 } else { 1 }
    }

    /// How many fewer code units UTF-16 needs than bytes for this character.
    fn deficit(self) -> u32 {
        self.byte_len() - self.utf16_len()
    }
}

/// Wide characters of one line plus the running byte−UTF-16 deficit through
/// it. Recorded only for lines that contain wide characters, ascending by
/// line number.
#[derive(Clone, Debug)]
struct WideLine {
    /// 0-based line number.
    line: u32,
    /// Sum of [`WideChar::deficit`] over all wide lines up to and including
    /// this one: the UTF-16 offset of a line start is its byte start minus
    /// the deficit of the lines before it.
    utf16_deficit: u32,
    /// The line's wide characters, ascending by in-line byte offset.
    chars: Box<[WideChar]>,
}

/// Records the scanned-out wide characters of one line into `wide_lines`,
/// advancing the running `deficit`. No-op for ASCII-only lines.
fn flush_wide_line(
    wide_lines: &mut Vec<WideLine>,
    wide_chars: &mut Vec<WideChar>,
    deficit: &mut u32,
    line: u32,
) {
    if wide_chars.is_empty() {
        return;
    }
    *deficit += wide_chars.iter().map(|c| c.deficit()).sum::<u32>();
    wide_lines.push(WideLine {
        line,
        utf16_deficit: *deficit,
        chars: std::mem::take(wide_chars).into_boxed_slice(),
    });
}

impl LineIndex {
    /// Precomputes line starts for `source`.
    ///
    /// Byte scanning is exact for UTF-8 input because `\n` and `\r` are ASCII
    /// and never occur inside multi-byte sequences.
    ///
    /// Allocation profile: the line-start count is computed in a first
    /// allocation-free pass (SIMD `memchr2`, candidate-by-candidate since a
    /// `\r` only counts when not followed by `\n`) so the vector is
    /// allocated exactly once at its
    /// final size (audit #99 F5: growth reallocations previously cost
    /// 4.66 allocs per file, 17 on a 65k-line file). Non-ASCII characters are
    /// recorded sparsely per line, so pure-ASCII sources allocate nothing
    /// beyond the line starts.
    ///
    /// Pass structure: a word-chunked `is_ascii` gate picks between two
    /// shapes. Pure-ASCII sources (the dominant case) never decode UTF-8 —
    /// wide-char recording is a no-op there, so a second `memchr2` pass
    /// replays the count scan and pushes the line starts directly. Any byte
    /// ≥ 0x80 falls through to the `char_indices` walk below, which decodes
    /// and records wide characters; it pays only the gate pass extra.
    ///
    /// # Panics
    ///
    /// Panics when `source` exceeds [`u32::MAX`] bytes; offsets in this
    /// crate are 32-bit by design. Unreachable through the engine: the
    /// intake gates reject sources over [`crate::MAX_SOURCE_BYTES`] at
    /// load and again at residency, before any position is derived — the
    /// check below is the crate's internal invariant statement, and direct
    /// callers outside the gates must size-check themselves.
    #[must_use]
    pub fn new(source: &str) -> Self {
        let source_len = u32::try_from(source.len()).expect("source exceeds u32::MAX bytes");
        let bytes = source.as_bytes();
        // Word-chunked gate: a pure-ASCII source makes wide-char recording a
        // no-op, so the UTF-8 decode walk below is pure overhead — line
        // starts come straight from the same `memchr2` scan the count uses.
        let is_ascii = bytes.is_ascii();
        let mut line_start_count = 0usize;
        let mut pos = 0usize;
        while let Some(found) = memchr::memchr2(b'\n', b'\r', &bytes[pos..]) {
            let i = pos + found;
            // A `\r\n` pair is one break, counted by its `\n`; skip the `\r`.
            pos = if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
                i + 1
            } else {
                line_start_count += 1;
                i + 1
            };
        }
        let mut line_starts = Vec::with_capacity(line_start_count + 1);
        line_starts.push(0);
        if is_ascii {
            // Fill pass: replays the count scan candidate-by-candidate, so it
            // pushes exactly `line_start_count` starts and the exact-
            // allocation invariant (`capacity() == len()`, audit #99 F5)
            // holds by construction. Break semantics mirror the char walk:
            // a `\r\n` pair records one start, at its `\n` (the `\r` is
            // skipped); a lone `\n` or `\r` records one at itself.
            let mut pos = 0usize;
            while let Some(found) = memchr::memchr2(b'\n', b'\r', &bytes[pos..]) {
                let i = pos + found;
                pos = i + 1;
                if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
                    continue;
                }
                line_starts.push(u32::try_from(i + 1).expect("line start exceeds u32::MAX"));
            }
            return Self {
                line_starts,
                wide_lines: Vec::new(),
                source_len,
            };
        }
        let mut wide_lines = Vec::new();
        let mut wide_chars: Vec<WideChar> = Vec::new(); // wide chars of the line being scanned
        let mut deficit = 0u32; // running deficit through the previous line
        let mut line = 0u32;
        let mut line_begin = 0usize;
        let mut src_chars = source.char_indices().peekable();
        while let Some((i, ch)) = src_chars.next() {
            if !ch.is_ascii() {
                let start = u32::try_from(i - line_begin).expect("offset exceeds u32::MAX");
                let end =
                    u32::try_from(i - line_begin + ch.len_utf8()).expect("offset exceeds u32::MAX");
                wide_chars.push(WideChar { start, end });
            }
            let ends_line = match ch {
                '\n' => true,
                '\r' => !matches!(src_chars.peek(), Some((_, '\n'))),
                _ => false,
            };
            if ends_line {
                let line_end = i + ch.len_utf8();
                flush_wide_line(&mut wide_lines, &mut wide_chars, &mut deficit, line);
                line += 1;
                line_begin = line_end;
                line_starts.push(u32::try_from(line_end).expect("line start exceeds u32::MAX"));
            }
        }
        // A final line without trailing terminator may still hold wide chars.
        flush_wide_line(&mut wide_lines, &mut wide_chars, &mut deficit, line);
        Self {
            line_starts,
            wide_lines,
            source_len,
        }
    }

    /// Number of lines; a trailing terminator yields a final empty line
    /// (official `LineInfo` semantics).
    ///
    /// # Panics
    ///
    /// Only reachable for sources beyond [`u32::MAX`] bytes, which
    /// [`LineIndex::new`] already rejects.
    #[must_use]
    pub fn line_count(&self) -> u32 {
        u32::try_from(self.line_starts.len()).expect("line count exceeds u32::MAX")
    }

    /// 1-based `(line, column)` for a byte offset, where the column counts
    /// **bytes** within the line (see the type-level coordinate decision).
    ///
    /// # Panics
    ///
    /// Panics when `byte_offset` is greater than the source length — an
    /// out-of-range offset is an engine bug and must surface, not clamp —
    /// or when the resulting line or column overflows [`u32`]. The
    /// overflow check mirrors [`SourceRange::end`]: the crate-wide contract
    /// is to surface broken offsets in every build profile, and a plain `+`
    /// only panicked in debug builds.
    #[must_use]
    pub fn byte_to_line_col(&self, byte_offset: u32) -> (u32, u32) {
        let line = u32::try_from(self.line_at(byte_offset)).expect("line number exceeds u32::MAX");
        (
            line.checked_add(1).expect("line number overflows u32"),
            (byte_offset - self.line_starts[line as usize])
                .checked_add(1)
                .expect("column overflows u32"),
        )
    }

    /// 1-based `(line, column)` for a 0-based UTF-16 code unit offset, where
    /// the column counts UTF-16 code units within the line (official
    /// `LineInfo.getLocation` semantics). This is the diagnostics position
    /// entry point: the line is located directly in the UTF-16 domain — one
    /// binary search, no intermediate byte offset, no allocation.
    ///
    /// Unlike the byte-domain converters ([`Self::utf16_to_byte`],
    /// [`utf16_offset_to_byte`]) there is **no mid-surrogate snap**: the
    /// oracle is pure arithmetic (`offset - lineStart + 1` in UTF-16 units),
    /// so an offset between the surrogates of an astral character reports
    /// the LOW surrogate's position.
    ///
    /// # Panics
    ///
    /// Panics when `utf16_offset` is greater than the source's UTF-16
    /// length, or when the resulting line or column overflows [`u32`]
    /// (same profile-independent contract as [`Self::byte_to_line_col`]).
    #[must_use]
    pub fn line_col_utf16(&self, _source: &str, utf16_offset: u32) -> (u32, u32) {
        let total = self.source_len - self.wide_lines.last().map_or(0, |wl| wl.utf16_deficit);
        assert!(
            utf16_offset <= total,
            "UTF-16 offset {utf16_offset} out of range (source has {total} UTF-16 code units)"
        );
        // Locate the line in the UTF-16 domain — the same locate as
        // `utf16_to_byte`'s general branch, minus the snap. A line's UTF-16
        // start is its byte start minus the deficit of the preceding lines;
        // pure-ASCII sources have no wide lines, so UTF-16 offsets are byte
        // offsets and the search is one plain binary search.
        let (line, line_start_utf16) = if self.wide_lines.is_empty() {
            let line = self.line_at(utf16_offset);
            (line, self.line_starts[line])
        } else {
            let mut lo = 0usize;
            let mut hi = self.line_starts.len();
            while lo < hi {
                let mid = usize::midpoint(lo, hi);
                if self.line_starts[mid] - self.wide_line(mid).0 <= utf16_offset {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            let line = lo - 1;
            let (before, _) = self.wide_line(line);
            (line, self.line_starts[line] - before)
        };
        let line = u32::try_from(line).expect("line number exceeds u32");
        (
            line.checked_add(1).expect("line number overflows u32"),
            (utf16_offset - line_start_utf16)
                .checked_add(1)
                .expect("column overflows u32"),
        )
    }

    /// Converts a byte offset into a 0-based UTF-16 code unit offset.
    ///
    /// The indexed hot-path counterpart of [`byte_offset_to_utf16`]: the
    /// containing line is located by binary search and only that line's
    /// wide-character records are applied, instead of rescanning from byte 0
    /// on every call. Prefer this method wherever a [`LineIndex`] for
    /// `source` is already at hand; the free function remains the fallback
    /// for callers that hold no index.
    ///
    /// The contract is that `source` is the exact text this index was built
    /// from — the index stores no content, so a mismatched pair is a caller
    /// bug that is not detected.
    ///
    /// # Panics
    ///
    /// Panics when `byte_offset` is out of range for `source` or not on a
    /// UTF-8 char boundary; both indicate a caller bug and must surface.
    #[must_use]
    pub fn byte_to_utf16(&self, source: &str, byte_offset: u32) -> u32 {
        let line = self.line_at(byte_offset);
        assert!(
            source.is_char_boundary(byte_offset as usize),
            "byte offset {byte_offset} is out of range or not a char boundary"
        );
        let (before, wide_chars) = self.wide_line(line);
        let col = byte_offset - self.line_starts[line];
        // Line start in UTF-16 units plus the in-line byte column; each wide
        // character fully before `col` removes its deficit.
        let mut utf16 = byte_offset - before;
        if let Some(chars) = wide_chars {
            for c in chars {
                if c.end <= col {
                    utf16 -= c.deficit();
                } else {
                    break;
                }
            }
        }
        utf16
    }

    /// Converts a 0-based UTF-16 code unit offset into a byte offset — the
    /// indexed hot-path counterpart of [`utf16_offset_to_byte`] (see
    /// [`Self::byte_to_utf16`] for the index/content contract and the
    /// free-function relationship).
    ///
    /// An offset pointing between the two surrogates of an astral character
    /// resolves to that character's first byte.
    ///
    /// # Panics
    ///
    /// Panics when `utf16_offset` is greater than the source's UTF-16 length.
    #[must_use]
    pub fn utf16_to_byte(&self, _source: &str, utf16_offset: u32) -> u32 {
        let total = self.source_len - self.wide_lines.last().map_or(0, |wl| wl.utf16_deficit);
        assert!(
            utf16_offset <= total,
            "UTF-16 offset {utf16_offset} out of range (source has {total} UTF-16 code units)"
        );
        // Locate the line in the UTF-16 domain: a line's UTF-16 start is its
        // byte start minus the deficit of the preceding lines. Pure-ASCII
        // sources have no wide lines, so UTF-16 offsets are byte offsets and
        // the search is one plain binary search; the general loop otherwise
        // pays a nested `wide_lines` search at every step.
        let line = if self.wide_lines.is_empty() {
            self.line_at(utf16_offset)
        } else {
            let mut lo = 0usize;
            let mut hi = self.line_starts.len();
            while lo < hi {
                let mid = usize::midpoint(lo, hi);
                if self.line_starts[mid] - self.wide_line(mid).0 <= utf16_offset {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            lo - 1
        };
        let (before, wide_chars) = self.wide_line(line);
        let line_start = self.line_starts[line];
        let target = utf16_offset - (line_start - before); // in-line UTF-16 offset
        let mut byte_col = target;
        if let Some(chars) = wide_chars {
            let mut deficit_so_far = 0u32;
            for c in chars {
                // The character's in-line UTF-16 start: its byte start minus
                // the deficits of the wide characters before it.
                let utf16_start = c.start - deficit_so_far;
                if target < utf16_start {
                    break; // target lies before this character
                }
                if target < utf16_start + c.utf16_len() {
                    // At the character's start, or a mid-surrogate offset
                    // that snaps to the character's first byte.
                    return line_start + c.start;
                }
                deficit_so_far += c.deficit();
                byte_col += c.deficit();
            }
        }
        line_start + byte_col
    }

    /// Wide-character record lookup for a 0-based `line`: the running
    /// deficit of all lines before it, plus the line's own wide characters
    /// (ascending by in-line byte offset) if it has any.
    fn wide_line(&self, line: usize) -> (u32, Option<&[WideChar]>) {
        let idx = self
            .wide_lines
            .partition_point(|wl| (wl.line as usize) < line);
        let before = if idx > 0 {
            self.wide_lines[idx - 1].utf16_deficit
        } else {
            0
        };
        let chars = self
            .wide_lines
            .get(idx)
            .filter(|wl| wl.line as usize == line)
            .map(|wl| &wl.chars[..]);
        (before, chars)
    }

    /// 0-based line containing `byte_offset`.
    ///
    /// # Panics
    ///
    /// Panics when `byte_offset` is greater than the source length.
    fn line_at(&self, byte_offset: u32) -> usize {
        assert!(
            byte_offset <= self.source_len,
            "byte offset {byte_offset} out of range (source is {} bytes)",
            self.source_len
        );
        self.line_starts
            .partition_point(|&start| start <= byte_offset)
            - 1
    }
}

/// UTF-16 length of `source` in code units.
///
/// # Panics
///
/// Panics when the length exceeds [`u32::MAX`] code units (a ~4-billion
/// code-unit source); offsets in this crate are 32-bit by design.
#[must_use]
pub fn utf16_len_of(source: &str) -> u32 {
    u32::try_from(source.chars().map(char::len_utf16).sum::<usize>())
        .expect("UTF-16 length exceeds u32::MAX")
}

/// Converts a byte offset into a 0-based UTF-16 code unit offset.
///
/// # Panics
///
/// Panics when `byte_offset` is out of range or not on a UTF-8 char boundary;
/// both indicate a caller bug and must surface.
#[must_use]
pub fn byte_offset_to_utf16(source: &str, byte_offset: usize) -> u32 {
    let prefix = source.get(..byte_offset).unwrap_or_else(|| {
        panic!("byte offset {byte_offset} is out of range or not a char boundary")
    });
    u32::try_from(prefix.chars().map(char::len_utf16).sum::<usize>())
        .expect("UTF-16 offset exceeds u32::MAX")
}

/// Converts a 0-based UTF-16 code unit offset back into a byte offset.
///
/// An offset pointing between the two surrogates of an astral character
/// resolves to that character's first byte.
///
/// # Panics
///
/// Panics when `utf16_offset` is greater than the source's UTF-16 length.
#[must_use]
pub fn utf16_offset_to_byte(source: &str, utf16_offset: u32) -> usize {
    let target = utf16_offset as usize;
    let mut consumed = 0usize;
    for (byte, ch) in source.char_indices() {
        if target < consumed + ch.len_utf16() {
            return byte;
        }
        consumed += ch.len_utf16();
    }
    assert!(
        target == consumed,
        "UTF-16 offset {utf16_offset} out of range (source has {consumed} UTF-16 code units)"
    );
    source.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_line_endings_match_official_semantics() {
        // Bytes: a(0) \n(1) b(2) \r(3) \n(4) c(5) \r(6) d(7)
        let index = LineIndex::new("a\nb\r\nc\rd");
        assert_eq!(index.line_count(), 4);
        assert_eq!(index.byte_to_line_col(0), (1, 1));
        assert_eq!(index.byte_to_line_col(1), (1, 2)); // the \n itself: still line 1
        assert_eq!(index.byte_to_line_col(2), (2, 1));
        assert_eq!(index.byte_to_line_col(3), (2, 2)); // \r of a \r\n pair
        assert_eq!(index.byte_to_line_col(5), (3, 1));
        assert_eq!(index.byte_to_line_col(6), (3, 2)); // lone \r
        assert_eq!(index.byte_to_line_col(7), (4, 1));
    }

    #[test]
    fn trailing_newline_and_empty_source() {
        assert_eq!(LineIndex::new("x\n").line_count(), 2);
        assert_eq!(LineIndex::new("x\n").byte_to_line_col(2), (2, 1));

        let empty = LineIndex::new("");
        assert_eq!(empty.line_count(), 1);
        assert_eq!(empty.byte_to_line_col(0), (1, 1));
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn byte_to_line_col_panics_beyond_source() {
        let _ = LineIndex::new("ab").byte_to_line_col(3);
    }

    #[test]
    fn utf16_len_counts_surrogates_and_bom() {
        // a(1) + emoji(2 surrogates) + b(1) + BOM(1)
        assert_eq!(utf16_len_of("a\u{1F600}b\u{FEFF}"), 5);
        assert_eq!(utf16_len_of(""), 0);
    }

    #[test]
    fn byte_utf16_conversions_with_astral_and_bom() {
        let source = "\u{FEFF}a\u{1F600}b";
        // Bytes: BOM 0..3, a 3..4, emoji 4..8, b 8..9.
        assert_eq!(byte_offset_to_utf16(source, 0), 0);
        assert_eq!(byte_offset_to_utf16(source, 3), 1);
        assert_eq!(byte_offset_to_utf16(source, 4), 2);
        assert_eq!(byte_offset_to_utf16(source, 8), 4); // emoji = 2 units
        assert_eq!(byte_offset_to_utf16(source, 9), 5);

        assert_eq!(utf16_offset_to_byte(source, 0), 0);
        assert_eq!(utf16_offset_to_byte(source, 1), 3);
        assert_eq!(utf16_offset_to_byte(source, 3), 4); // between surrogates snaps to char start
        assert_eq!(utf16_offset_to_byte(source, 4), 8);
        assert_eq!(utf16_offset_to_byte(source, 5), source.len());

        // Every char boundary round-trips through UTF-16.
        for boundary in source.char_indices().map(|(b, _)| b).chain([source.len()]) {
            let utf16 = byte_offset_to_utf16(source, boundary);
            assert_eq!(utf16_offset_to_byte(source, utf16), boundary);
        }
    }

    #[test]
    #[should_panic(expected = "not a char boundary")]
    fn byte_offset_to_utf16_rejects_mid_char() {
        let source = "\u{1F600}x";
        let _ = byte_offset_to_utf16(source, 2);
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn utf16_offset_to_byte_rejects_overflow() {
        let _ = utf16_offset_to_byte("ab", 3);
    }

    /// The indexed conversion methods must agree with the free functions
    /// (the semantic oracle) on every char boundary and every UTF-16 offset,
    /// and round-trip byte → UTF-16 → byte. Sources cover the classes that
    /// can break the conversion: empty, all three line-break forms, BOM,
    /// astral (surrogate-pair) chars, and BMP multi-byte text. The repo has
    /// no in-repo Dart corpus, so sampling is synthetic by design.
    #[test]
    fn indexed_utf16_conversions_match_free_functions_and_round_trip() {
        let sources = [
            "",
            "plain ascii\nthree lines\n",
            "a\nb\r\nc\rd",
            "\r\n\r\n\r\n",
            "\r\r\r",
            "x\n",
            "\u{FEFF}a\u{1F600}b\n\u{1F600}\r\nx",
            "汉字测试\n日本語テキスト\r\n한국어\n",
            "\u{1F600}\u{1F601}\u{1F602}",
            // U+2028/U+2029 are wide characters, not line breaks (pinned
            // directly by `u2028_u2029_are_not_line_breaks`).
            "\u{2028}a\u{2029}b\nc\n",
        ];
        for source in sources {
            let index = LineIndex::new(source);
            for boundary in source.char_indices().map(|(b, _)| b).chain([source.len()]) {
                let utf16 = byte_offset_to_utf16(source, boundary);
                let byte = u32::try_from(boundary).unwrap();
                assert_eq!(
                    index.byte_to_utf16(source, byte),
                    utf16,
                    "byte {boundary} of {source:?}"
                );
                assert_eq!(
                    index.utf16_to_byte(source, utf16),
                    byte,
                    "utf16 {utf16} of {source:?}"
                );
            }
            let total = utf16_len_of(source);
            for units in 0..=total {
                assert_eq!(
                    index.utf16_to_byte(source, units),
                    u32::try_from(utf16_offset_to_byte(source, units)).unwrap(),
                    "utf16 {units} of {source:?}"
                );
            }
        }
    }

    /// The official recipe (`LineInfo.fromContent` + `getLocation`), computed
    /// independently in the UTF-16 domain — Dart string offsets are UTF-16
    /// code units, so its line starts and columns are pure UTF-16 arithmetic
    /// with no byte-domain round-trip and no mid-surrogate snap.
    fn official_line_col(source: &str, utf16_offset: u32) -> (u32, u32) {
        let mut starts = vec![0u32];
        let mut units = 0u32;
        let mut chars = source.chars().peekable();
        while let Some(ch) = chars.next() {
            units += u32::try_from(ch.len_utf16()).unwrap();
            let breaks = match ch {
                // A `\r\n` pair is one break, counted at its `\n`.
                '\r' => chars.peek() != Some(&'\n'),
                '\n' => true,
                _ => false,
            };
            if breaks {
                starts.push(units);
            }
        }
        assert!(utf16_offset <= units, "recipe offset out of range");
        let line = starts.partition_point(|&start| start <= utf16_offset) - 1;
        (
            u32::try_from(line).unwrap() + 1,
            utf16_offset - starts[line] + 1,
        )
    }

    /// The fused `line_col_utf16` must agree with the official recipe
    /// (`official_line_col` above: UTF-16 line starts, pure
    /// `offset - lineStart + 1` arithmetic) on every UTF-16 offset of
    /// wide-character sources — hanzi, astral emoji, BOM — plus an ASCII
    /// control and the audit's astral-final unterminated-comment shape,
    /// whose eof-pinned diagnostics land between the surrogates of the
    /// final character.
    #[test]
    fn line_col_utf16_matches_external_recipe() {
        let sources = [
            "汉字测试\n日本語テキスト\r\n한국어\n",
            "\u{FEFF}a\u{1F600}b\n\u{1F600}\r\nx",
            "ascii\nlines\nonly\n",
            // Lone `\r` as the sole terminator, surrounded by astral chars:
            // line starts must track across the CR while surrogates surround
            // it on both lines.
            "\u{1F600}a\r\u{1F600}b",
            "a /* \u{1F600}",
        ];
        for source in sources {
            let index = LineIndex::new(source);
            let total = utf16_len_of(source);
            for units in 0..=total {
                assert_eq!(
                    index.line_col_utf16(source, units),
                    official_line_col(source, units),
                    "utf16 {units} of {source:?}"
                );
            }
        }
    }

    /// U+000B (VT), U+000C (FF), and U+0085 (NEL) are ordinary characters,
    /// not line terminators — the termination set is exactly `\n`, `\r\n`,
    /// and lone `\r` (the official scanner's set). Negative pin: with a
    /// trailing `\n` as the only break, the whole text stays on line 1 and
    /// the count is 2 (content line + the final empty line the terminator
    /// starts).
    #[test]
    fn ff_vt_nel_do_not_break_lines() {
        // Every character is one UTF-16 unit, so offsets are plain
        // positions: a 0, VT 1, b 2, FF 3, c 4, NEL 5, d 6, \n 7.
        let source = "a\u{000B}b\u{000C}c\u{0085}d\n";
        let index = LineIndex::new(source);
        assert_eq!(index.line_count(), 2);
        assert_eq!(index.line_col_utf16(source, 0), (1, 1));
        // The character right after each control is still on line 1.
        assert_eq!(index.line_col_utf16(source, 2), (1, 3)); // b, after VT
        assert_eq!(index.line_col_utf16(source, 4), (1, 5)); // c, after FF
        assert_eq!(index.line_col_utf16(source, 6), (1, 7)); // d, after NEL
        assert_eq!(index.line_col_utf16(source, 8), (2, 1)); // only \n breaks
    }

    /// A UTF-16 offset between the surrogates of an astral character must
    /// report the LOW surrogate's column: the official `getLocation` is pure
    /// UTF-16 arithmetic with no snap. Audit #221 — the previous byte-domain
    /// round-trip snapped such offsets to the character's first byte and
    /// reported the HIGH surrogate's column, one short on every pin-style
    /// diagnostic ending in an astral character (`a /* 😀`: offset 6 gave
    /// column 6 instead of 7).
    #[test]
    fn line_col_utf16_mid_surrogate_reports_low_surrogate_column() {
        // `a /* 😀`: the emoji spans UTF-16 units 5..7; unit 5 is the high
        // surrogate's position (the character start), unit 6 the low one.
        let source = "a /* \u{1F600}";
        let index = LineIndex::new(source);
        assert_eq!(index.line_col_utf16(source, 5), (1, 6));
        assert_eq!(index.line_col_utf16(source, 6), (1, 7)); // low surrogate
        assert_eq!(index.line_col_utf16(source, 7), (1, 8)); // eof

        // Astral chars mid-line, offsets probed on both surrogate halves.
        // Units: emoji 0..2, a 2, b 3, emoji 4..6, c 6, d 7.
        let source = "\u{1F600}ab\u{1F601}cd";
        let index = LineIndex::new(source);
        assert_eq!(index.line_col_utf16(source, 0), (1, 1));
        assert_eq!(index.line_col_utf16(source, 1), (1, 2)); // low surrogate
        assert_eq!(index.line_col_utf16(source, 2), (1, 3));
        assert_eq!(index.line_col_utf16(source, 4), (1, 5)); // high surrogate
        assert_eq!(index.line_col_utf16(source, 5), (1, 6)); // low surrogate
        assert_eq!(index.line_col_utf16(source, 6), (1, 7));
    }

    /// U+2028/U+2029 (LINE/PARAGRAPH SEPARATOR) are ordinary wide
    /// characters, not line terminators — the official scanner breaks only
    /// on `\n`, `\r\n`, and `\r`. Negative pin: the round-trip parity test
    /// cannot distinguish a breaker from a non-breaker, so the line count is
    /// asserted directly.
    #[test]
    fn u2028_u2029_are_not_line_breaks() {
        let source = "a\u{2028}b\u{2029}c\n";
        let index = LineIndex::new(source);
        assert_eq!(index.line_count(), 2);
        assert_eq!(
            index.byte_to_line_col(u32::try_from(source.len()).unwrap()),
            (2, 1)
        );
        let recorded: usize = index.wide_lines.iter().map(|wl| wl.chars.len()).sum();
        assert_eq!(recorded, 2, "both separators are recorded as wide chars");
    }

    /// End-to-end values across lines with astral text and `\r\n` breaks:
    /// each `\r`/`\n` is itself one UTF-16 unit, so line 2 starts at
    /// utf16 6 (BOM+a+emoji+\r+\n), line 3 at utf16 9.
    #[test]
    fn indexed_conversions_across_lines_with_astral_and_crlf() {
        let source = "\u{FEFF}a\u{1F600}\r\n汉字\nx";
        // Bytes: BOM 0..3, a 3..4, emoji 4..8, \r 8, \n 9, 汉 10..13,
        // 字 13..16, \n 16, x 17..18.
        let index = LineIndex::new(source);
        assert_eq!(index.line_count(), 3);
        assert_eq!(index.byte_to_utf16(source, 10), 6);
        assert_eq!(index.utf16_to_byte(source, 6), 10);
        assert_eq!(index.byte_to_utf16(source, 13), 7);
        assert_eq!(index.utf16_to_byte(source, 7), 13);
        assert_eq!(index.byte_to_utf16(source, 17), 9);
        assert_eq!(index.utf16_to_byte(source, 9), 17);
        // Inside line 1: the emoji spans utf16 2..4 and byte 4..8; a
        // mid-surrogate offset snaps to the char's first byte.
        assert_eq!(index.byte_to_utf16(source, 8), 4);
        assert_eq!(index.utf16_to_byte(source, 3), 4);
        // The golden-test column pattern stays a within-line difference.
        assert_eq!(index.byte_to_line_col(13), (2, 4));
        assert_eq!(
            index.byte_to_utf16(source, 13) - index.byte_to_utf16(source, 10) + 1,
            2
        );
    }

    /// Audit #99 F5: `LineIndex::new` must allocate the line vector exactly
    /// once at its final size, and a pure-ASCII source must produce no
    /// wide-character records at all. `capacity() == len()` on a large
    /// mixed-break source proves the pre-scan reserved precisely — the
    /// previous growth pattern cost 4.66 allocs per file (17 on a 65k-line
    /// file).
    #[test]
    fn ascii_sources_reserve_exactly_and_record_nothing() {
        // 9 bytes with 4 line starts per repetition (mixed \n, \r\n, lone \r).
        let source = "a\nb\r\nc\rd\n".repeat(20_000);
        let index = LineIndex::new(&source);
        assert_eq!(index.line_count(), 80_001);
        assert_eq!(index.line_starts.len(), index.line_starts.capacity());
        assert!(index.wide_lines.is_empty());
    }

    /// The sparse table records exactly the non-ASCII characters, grouped by
    /// their line — mixed sources only pay for the lines that hold wide
    /// characters (dispatch #105: 65k pure-ASCII lines cost zero records).
    #[test]
    fn wide_char_records_track_non_ascii_exactly() {
        // Line 0: BOM + astral emoji; line 1: BMP hanzi; line 2: ASCII only.
        let source = "\u{FEFF}\u{1F600}a\n汉字\nx\n";
        let index = LineIndex::new(source);
        let recorded: usize = index.wide_lines.iter().map(|wl| wl.chars.len()).sum();
        assert_eq!(recorded, source.chars().filter(|c| !c.is_ascii()).count());
        assert_eq!(index.wide_lines.len(), 2);
        assert_eq!(index.wide_lines[0].line, 0);
        assert_eq!(index.wide_lines[1].line, 1);
    }

    #[test]
    #[should_panic(expected = "not a char boundary")]
    fn indexed_byte_to_utf16_rejects_mid_char() {
        let source = "\u{1F600}x";
        let _ = LineIndex::new(source).byte_to_utf16(source, 2);
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn indexed_byte_to_utf16_rejects_overflow() {
        let _ = LineIndex::new("ab").byte_to_utf16("ab", 3);
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn indexed_utf16_to_byte_rejects_overflow() {
        let _ = LineIndex::new("ab").utf16_to_byte("ab", 3);
    }

    #[test]
    fn source_range_end_and_copy_semantics() {
        let range = SourceRange {
            offset: 10,
            length: 5,
        };
        assert_eq!(range.end(), 15);

        // Using `range` after binding it to `copied` proves Copy.
        let copied = range;
        assert_eq!(copied.end(), 15);
        assert_eq!(range.offset, 10);
        assert_eq!(
            range,
            SourceRange {
                offset: 10,
                length: 5
            }
        );
    }

    /// Audit A7: `end` surfaces overflow in every profile (the crate's
    /// "surface, don't clamp" contract) instead of wrapping in release.
    #[test]
    #[should_panic(expected = "overflows u32")]
    fn source_range_end_panics_on_overflow() {
        let _ = SourceRange {
            offset: u32::MAX,
            length: 1,
        }
        .end();
    }
}
