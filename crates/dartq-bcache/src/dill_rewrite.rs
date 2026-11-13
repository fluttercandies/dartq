//! URI prefix rewriter for Dart kernel dill files (binary formats
//! 124–138, see [`FORMAT_VERSIONS`]).
//!
//! A `flutter test` kernel-cache dill embeds its tree's absolute URIs
//! (`file://<project-root>/lib/main.dart`) in exactly two tables per
//! component — the source table's `uri`/`importUri` strings and the
//! string table's entries — so a cached dill only replays into the tree
//! it was compiled in: frontend_server's initialize-from-dill pass
//! invalidates every library whose URI it cannot resolve, and a foreign
//! URI is always unresolvable. This module rewrites `file://<old>/`
//! prefixes to `file://<new>/` inside those tables, rebuilding every
//! offset-bearing structure around them, which lets a dill materialized
//! into a different worktree survive that validation.
//!
//! A dill is a multi-component append log: framing is
//! `[component bytes][u32 BE size]` per component and the kernel reader
//! splits it by walking the backward size chain from EOF. Each
//! component ends with its component index (footer): 9 fixed u32
//! fields, `libraryCount + 1` library offsets, then `libraryCount` and
//! `totalSize` re-read (`numberOfFixedFields = 9 + libraryCount + 1 +
//! 2`). Every table offset in the footer and the source table's
//! trailing entry index are absolute positions within their component.
//!
//! What is rewritten and what is not:
//!
//! * Source table entries: the `uri` and `importUri` strings. Embedded
//!   source TEXT may contain old-prefix bytes and is left verbatim —
//!   diagnostics-only content that no URI resolution reads.
//! * String table entries: rewritten, with the cumulative end offsets
//!   (u30 varints) rebuilt around the new lengths.
//! * The AST region before the source table, the constant/index/link
//!   tables, and the metadata payloads are byte-copied; metadata
//!   mapping offsets are section-relative, so shifting their sections
//!   needs no fixup. The metadata payloads and every component's total
//!   size stay 8-byte aligned (the official writer's own contract for
//!   concatenation), which is where the two padding runs come from.
//!
//! Safety gates: every component must carry the kernel magic, a modeled
//! binary format version, and an ASCII sdkHash near its head. A mismatch
//! on any gate, a non-ASCII string matched by the prefix (the string
//! table blob is WTF-8; the conservative contract rewrites ASCII-only
//! strings), or a table walk that does not land exactly on the
//! footer's recorded boundary refuses the whole file — the caller then
//! falls back to a placement that costs a recompile, never a wrong
//! artifact (see [`crate::materialize`]).
//!
//! Cost: the input is read through a bounded sliding window (a small
//! mmap remapped as the walk advances and unmapped behind it — an
//! empirical necessity, since darwin's kernel ignores `MADV_DONTNEED`
//! for file-backed pages), so the resident set never scales with the
//! dill (a full-app track dill is ~113 MB); the output streams through
//! a bounded writer; the tables are parsed by offset arithmetic, not
//! decoded. The layout mirrors the upstream Dart SDK's pkg/kernel
//! binary printer and reader.
//!
//! Hardening contract: no dill, however malformed, can fail the user's
//! build. Every offset and count carved out of the file is
//! bounds-checked (a value that does not describe real bytes refuses
//! the file), no capacity is ever allocated from an unchecked count,
//! and the whole rewrite runs inside `catch_unwind` so even an
//! unforeseen panic surfaces as the same refusal the gates produce.
//! Each window is PROT_READ for its whole lifetime and the input is a
//! cache-owned blob that nothing truncates mid-rewrite.

use std::fs;
use std::io::{BufWriter, Write};
use std::ops::Range;
use std::path::Path;

use memchr::memmem;
use memmap2::{Mmap, MmapOptions};

use crate::error::{BcacheError, Result};

#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};

/// `Tag.ComponentFile`: the u32 BE magic every kernel component starts
/// with.
const MAGIC: u32 = 0x90AB_CDEF;
/// The `Tag.BinaryFormatVersion`s whose component layout this module
/// rewrites: 124 (Dart 3.8, flutter 3.32), 125 (Dart 3.9 and 3.10,
/// flutter 3.35 and 3.38), 127 (Dart 3.11, flutter 3.41), 130 (Dart
/// 3.12, flutter 3.44), and 138 (Dart 3.13, flutter 3.47). The upstream
/// `pkg/kernel/binary.md` of each declares identical component-level
/// types — `ComponentFile`, `UriSource`/`SourceInfo`, `StringTable`,
/// `MetadataPayload`/`MetadataMapping`, `ComponentIndex` — so the
/// formats differ only inside the byte-copied AST region. Format 122
/// and older add a `compilationMode` field to the component index (a
/// footer this walk does not model) and are refused.
const FORMAT_VERSIONS: &[u32] = &[124, 125, 127, 130, 138];
/// The sdkHash region right after the version, in bytes: a Git short
/// hash. Its length never enters the offset arithmetic (all table
/// positions come from the footer), so the gate only checks that the
/// region is ASCII.
const SDK_HASH_LEN: usize = 10;

/// What one file rewrite did, for reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DillRewrite {
    /// Kernel components walked (an append-log dill holds many).
    pub components: usize,
    /// Source-table `uri`/`importUri` strings rewritten.
    pub source_uris: u64,
    /// String-table entries rewritten.
    pub strings: u64,
}

/// Rewrites every occurrence of the `from` byte prefix inside the URI
/// strings of the kernel dill at `src`, streaming the result to `dst`.
///
/// Both prefixes must be non-empty ASCII; a non-ASCII `to` would
/// corrupt the WTF-8 string table the same way a non-ASCII match
/// would. `dst` is created (or truncated) — callers write to a temp
/// sibling and rename, so a refused rewrite never lands. On error the
/// partial `dst` is left for the caller to discard.
///
/// The rewrite is panic-contained: any panic inside the parse or the
/// write is converted into the same refusal the gates produce, so a
/// malformed dill costs a verbatim fallback, never a crashed build.
///
/// # Errors
///
/// [`BcacheError::Invalid`] for an empty or non-ASCII prefix;
/// [`BcacheError::Unsupported`] for any gate or boundary refusal (a
/// layout this module does not model — the caller places the blob
/// verbatim); [`BcacheError::Io`] for filesystem failures.
pub fn rewrite_file(src: &Path, dst: &Path, from: &[u8], to: &[u8]) -> Result<DillRewrite> {
    check_prefix(from, "from")?;
    check_prefix(to, "to")?;
    // The refusals below cover every layout this module models, but
    // the containment is the guarantee that matters to the caller: the
    // closure only re-borrows caller-owned paths and bytes, and the
    // `dst` it creates is discarded by the caller on any error, so an
    // unwinding panic leaves no torn state behind.
    let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rewrite_mapped(src, dst, from, to)
    }));
    match attempt {
        Ok(result) => result,
        Err(payload) => Err(BcacheError::Unsupported(format!(
            "kernel dill rewrite panicked ({}); refusing",
            panic_message(payload.as_ref())
        ))),
    }
}

/// Test injection point for the panic-containment proof: when set, the
/// next rewrite panics before touching the filesystem.
#[cfg(test)]
static POISON_REWRITE: AtomicBool = AtomicBool::new(false);

fn rewrite_mapped(src: &Path, dst: &Path, from: &[u8], to: &[u8]) -> Result<DillRewrite> {
    #[cfg(test)]
    if POISON_REWRITE.swap(false, Ordering::SeqCst) {
        panic!("rewrite poisoned by test");
    }
    let mut input = Reader::open(src)?;
    let mut out = Out::new(BufWriter::new(fs::File::create(dst)?));
    let mut stats = DillRewrite::default();
    let ranges = component_ranges(input.len, |at| input.slice_u32(at))?;
    for range in ranges {
        check_header(&mut input, range.start, range.end - range.start)?;
        rewrite_component(
            &mut input,
            range.start,
            range.end,
            &mut out,
            from,
            to,
            &mut stats,
        )?;
        stats.components += 1;
    }
    out.inner.flush()?;
    Ok(stats)
}

/// The window size the dill is read through: big enough that a remap
/// amortizes over many reads, small enough that the resident set stays
/// a rounding error next to a 100 MB dill.
const WINDOW: usize = 256 * 1024;

/// A sliding-window reader over the dill. The file is mapped in
/// bounded windows — remapped as the walk advances, unmapped when
/// replaced — so the resident set never scales with the dill; whole
/// mappings must not be used for this, because darwin's kernel ignores
/// `MADV_DONTNEED` for file-backed pages (measured: a fully touched
/// 68 MB map stays ~68 MB resident either way, windowed drops to
/// ~2 MB).
///
/// One slice is live at a time: a `slice` call may re-map the window,
/// and the borrow checker enforces that the caller has consumed the
/// previous slice first — the sequential shape this module's walks
/// already have.
struct Reader {
    file: fs::File,
    len: usize,
    window: Mmap,
    win_start: usize,
}

impl Reader {
    fn open(path: &Path) -> Result<Self> {
        let file = fs::File::open(path)?;
        let len = usize::try_from(file.metadata()?.len())
            .map_err(|_| malformed(0, "dill exceeds the addressable size"))?;
        if len < 4 {
            return Err(malformed(len as isize, "under 4 bytes"));
        }
        let window = map_window(&file, 0, WINDOW.min(len))?;
        Ok(Self {
            file,
            len,
            window,
            win_start: 0,
        })
    }

    /// Bytes `[start..end)` of the dill: bounds-checked (a parsed
    /// value that does not describe real bytes is a refusal, never a
    /// slice panic), served from the current window when it covers the
    /// range, else through a fresh one.
    fn slice(&mut self, start: usize, end: usize) -> Result<&[u8]> {
        if start == end {
            return Ok(&[]);
        }
        if end < start || end > self.len {
            return Err(malformed(end as isize, "range past the dill end"));
        }
        let win_end = self.win_start + self.window.len();
        if start >= self.win_start && end <= win_end {
            return Ok(&self.window[start - self.win_start..end - self.win_start]);
        }
        // A window with slack for the reads that follow; a span larger
        // than one window (a hostile table) gets its own exact map,
        // transiently resident and dropped at the next remap.
        let want = WINDOW.max(end - start).min(self.len - start);
        self.window = map_window(&self.file, start, want)?;
        self.win_start = start;
        Ok(&self.window[..end - start])
    }

    /// A u32 BE at `at`, through the same bounds proof.
    fn slice_u32(&mut self, at: usize) -> Result<u32> {
        let b = self.slice(at, at + 4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
}

/// Maps `[start..start+len)` of the file. `MmapOptions` accepts an
/// unaligned `start` (it aligns the system call down and re-bases the
/// slice), so window boundaries can sit on any byte.
fn map_window(file: &fs::File, start: usize, len: usize) -> Result<Mmap> {
    // Safety: PROT_READ for the lifetime of the returned map; the file
    // is a cache-owned blob that nothing truncates mid-rewrite.
    unsafe { MmapOptions::new().offset(start as u64).len(len).map(file) }.map_err(BcacheError::from)
}

/// Streams `input[start..end]` verbatim in window-sized chunks: the
/// copy holds one window, never the region.
fn copy_span<W: Write>(
    out: &mut Out<W>,
    input: &mut Reader,
    start: usize,
    end: usize,
) -> Result<()> {
    let mut pos = start;
    while pos < end {
        let stop = (pos + WINDOW).min(end);
        out.bytes(input.slice(pos, stop)?)?;
        pos = stop;
    }
    Ok(())
}

/// Renders a caught panic payload for the refusal message: ordinary
/// `panic!`s carry a message worth surfacing, anything else still gets
/// a nameable cause.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".into()
    }
}

/// A rewrite prefix must be non-empty ASCII: empty would match
/// everywhere, and non-ASCII bytes could split a WTF-8 sequence in the
/// string table.
fn check_prefix(prefix: &[u8], name: &str) -> Result<()> {
    if prefix.is_empty() {
        return Err(BcacheError::Invalid(format!("{name} prefix is empty")));
    }
    if prefix.iter().any(|&b| b > 0x7F) {
        return Err(BcacheError::Invalid(format!("{name} prefix must be ASCII")));
    }
    Ok(())
}

/// Splits a dill into its components by walking the backward size
/// chain from EOF, exactly as the kernel reader's `_indexComponents`
/// does: a u32 BE size terminates each component and the previous
/// component's size u32 sits right before its start. Returns the
/// components in file order. `read_u32_at` supplies bounds-checked
/// field reads (a windowed reader in production, an in-memory closure
/// in tests).
fn component_ranges(
    len: usize,
    mut read_u32_at: impl FnMut(usize) -> Result<u32>,
) -> Result<Vec<Range<usize>>> {
    let malformed = |at: isize, why: &str| {
        BcacheError::Unsupported(format!("kernel dill layout not modeled: {why} (at {at})"))
    };
    let mut ranges = Vec::new();
    // Signed cursor: the walk ends when it would step before byte 0.
    let mut pos: isize = len as isize - 4;
    if pos < 0 {
        return Err(malformed(len as isize, "under 4 bytes"));
    }
    while pos >= 0 {
        let at = pos as usize;
        let size = read_u32_at(at).map_err(|_| malformed(pos, "truncated size field"))?;
        if size == 0 {
            return Err(malformed(pos, "zero component size"));
        }
        let start = pos + 4 - size as isize;
        if start < 0 {
            return Err(malformed(pos, "component size exceeds the bytes before it"));
        }
        ranges.push(start as usize..at + 4);
        pos = start - 4;
    }
    ranges.reverse();
    Ok(ranges)
}

/// The per-component header gate: magic, format version, ASCII sdkHash.
fn check_header(input: &mut Reader, comp_start: usize, comp_len: usize) -> Result<()> {
    if comp_len < 8 + SDK_HASH_LEN {
        return Err(malformed(
            comp_len as isize,
            "component shorter than its header",
        ));
    }
    let magic = input.slice_u32(comp_start)?;
    if magic != MAGIC {
        return Err(BcacheError::Unsupported(format!(
            "not a kernel component: magic {magic:#010x}"
        )));
    }
    let version = input.slice_u32(comp_start + 4)?;
    if !FORMAT_VERSIONS.contains(&version) {
        return Err(BcacheError::Unsupported(format!(
            "kernel format version {version} is not modeled ({FORMAT_VERSIONS:?})"
        )));
    }
    let hash = input.slice(comp_start + 8, comp_start + 8 + SDK_HASH_LEN)?;
    if hash.iter().any(|&b| !(0x21..=0x7E).contains(&b)) {
        return Err(BcacheError::Unsupported(format!(
            "kernel sdk hash is not ASCII: {hash:?}"
        )));
    }
    Ok(())
}

/// The component index footer. Field order is the official writer's;
/// `library_offsets` holds `library_count + 1` entries.
struct Footer {
    src_tbl: usize,
    const_tbl: usize,
    const_idx: usize,
    link_tbl: usize,
    meta_payloads: usize,
    meta_mappings: usize,
    str_tbl: usize,
    comp_idx_start: usize,
    main_ref: u32,
    library_count: u32,
    library_offsets: Vec<u32>,
}

impl Footer {
    /// Footer byte length: the fixed fields, including the re-read
    /// `libraryCount` + `totalSize` tail
    /// (`numberOfFixedFields = 9 + libraryCount + 1 + 2`).
    fn len(library_count: u32) -> usize {
        (9 + library_count as usize + 1 + 2) * 4
    }
}

fn parse_footer(input: &mut Reader, comp_start: usize, comp_len: usize) -> Result<Footer> {
    let comp_end = comp_start + comp_len;
    if comp_len < 12 {
        return Err(malformed(
            comp_len as isize,
            "component shorter than a footer",
        ));
    }
    let mut cursor = Cursor::new(input, comp_end - 8, comp_end);
    let library_count = cursor.u32()?;
    let total_size = cursor.u32()? as usize;
    if total_size != comp_len {
        return Err(malformed(
            comp_len as isize,
            "footer totalSize does not match the component length",
        ));
    }
    // The trailing libraryCount + totalSize are part of the fixed
    // fields; the reader re-reads them at the end of the sequence.
    let fixed = Footer::len(library_count);
    let start = comp_len.checked_sub(fixed).ok_or_else(|| {
        malformed(
            comp_len as isize,
            "component shorter than its footer claims",
        )
    })?;
    let mut cursor = Cursor::new(input, comp_start + start, comp_end);
    let footer = Footer {
        src_tbl: cursor.u32()? as usize,
        const_tbl: cursor.u32()? as usize,
        const_idx: cursor.u32()? as usize,
        link_tbl: cursor.u32()? as usize,
        meta_payloads: cursor.u32()? as usize,
        meta_mappings: cursor.u32()? as usize,
        str_tbl: cursor.u32()? as usize,
        comp_idx_start: cursor.u32()? as usize,
        main_ref: cursor.u32()?,
        library_count,
        library_offsets: (0..=library_count)
            .map(|_| cursor.u32())
            .collect::<Result<_>>()?,
    };
    if cursor.u32()? != library_count {
        return Err(malformed(
            comp_len as isize,
            "libraryCount re-read mismatch",
        ));
    }
    if cursor.u32()? as usize != comp_len {
        return Err(malformed(comp_len as isize, "totalSize re-read mismatch"));
    }
    // A monotonic, in-bounds table sequence: misparses and layout drift
    // land here instead of producing a wrong rewrite.
    let ordered = footer.src_tbl <= footer.const_tbl
        && footer.const_tbl <= footer.const_idx
        && footer.const_idx <= footer.link_tbl
        && footer.link_tbl <= footer.meta_payloads
        && footer.meta_payloads <= footer.meta_mappings
        && footer.meta_mappings <= footer.str_tbl
        && footer.str_tbl <= footer.comp_idx_start
        && footer.comp_idx_start + fixed <= comp_len;
    if !ordered {
        return Err(malformed(
            comp_len as isize,
            "footer table offsets are not ordered",
        ));
    }
    Ok(footer)
}

fn malformed(at: isize, why: &str) -> BcacheError {
    BcacheError::Unsupported(format!("kernel dill layout not modeled: {why} (at {at})"))
}

/// Rewrites one component, streaming. Every offset written is derived
/// from the writer's own position counter, so the output is consistent
/// by construction; the input-side boundary checks are what guard
/// against a misparsed layout. The cursor below speaks file-relative
/// positions bounded by `comp_end`, while the footer's table offsets
/// are component-relative — `rel` converts.
fn rewrite_component<W: Write>(
    input: &mut Reader,
    comp_start: usize,
    comp_end: usize,
    out: &mut Out<W>,
    from: &[u8],
    to: &[u8],
    stats: &mut DillRewrite,
) -> Result<()> {
    let comp_len = comp_end - comp_start;
    let footer = parse_footer(input, comp_start, comp_len)?;
    let finder = memmem::Finder::new(from);
    // The writer is shared across components, so its counter is
    // file-relative while every offset the tables record is
    // component-relative; `base` converts between them. Each finished
    // component is padded to a multiple of 8, so the base is always
    // 8-aligned and file-relative alignment decisions equal
    // component-relative ones.
    let base = out.len;
    let at = |out: &Out<W>| -> Result<usize> {
        out.len
            .checked_sub(base)
            .ok_or_else(|| BcacheError::Unsupported("component base underflowed".into()))
    };
    let rel = |pos: usize| comp_start + pos;

    // 1. Magic, sdk hash, problemsAsJson, and the libraries AST:
    //    byte-identical; no AST parsing.
    copy_span(out, input, comp_start, rel(footer.src_tbl))?;

    // 2. Source table: rewrite uri/importUri strings in place, rebuild
    //    the trailing absolute-entry-offset index from the write
    //    position. Entry layout: uri, embedded source, line-start
    //    deltas, importUri, coverage references. Each span is consumed
    //    as it is read — the windowed reader hands out one slice at a
    //    time.
    let mut cursor = Cursor::new(input, rel(footer.src_tbl), comp_end);
    let count = cursor.u32()?;
    out.u32(count)?;
    // Capacity grows only with bytes really parsed: a hostile count
    // runs the walk into an end-of-component refusal long before any
    // large allocation.
    let mut offsets = Vec::new();
    let mut scratch = Vec::new();
    for _ in 0..count {
        offsets.push(u32::try_from(at(out)?).map_err(|_| offset_overflow())?);
        let uri = cursor.lp_bytes()?;
        write_source_string(out, uri, &finder, to, &mut scratch, stats)?;
        let source = cursor.lp_bytes()?;
        out.u30(source.len())?;
        out.bytes(source)?;
        let lines = cursor.u30_span()?;
        out.bytes(lines)?;
        let import_uri = cursor.lp_bytes()?;
        write_source_string(out, import_uri, &finder, to, &mut scratch, stats)?;
        let coverage = cursor.u30_span()?;
        out.bytes(coverage)?;
    }
    cursor.skip_u32s(count)?; // the old trailing index
    if cursor.pos != rel(footer.const_tbl) {
        return Err(malformed(
            (cursor.pos - comp_start) as isize,
            "source table walk did not end at the constant table",
        ));
    }
    for offset in offsets {
        out.u32(offset)?;
    }

    // 3. Constant table + constant index + link table: byte-identical,
    //    shifted by whatever the source table grew or shrank. The
    //    delta is signed: a shorter replacement prefix shrinks tables.
    let delta = at(out)? as i64 - footer.const_tbl as i64;
    copy_span(out, input, rel(footer.const_tbl), rel(footer.meta_payloads))?;

    // 4. Metadata payloads stay 8-byte aligned.
    let payloads_at = out.len.div_ceil(8) * 8;
    out.pad(payloads_at - out.len)?;
    let new_payloads = payloads_at - base;
    let shift = new_payloads as i64 - footer.meta_payloads as i64;

    // 5. Metadata payloads + mappings: byte-identical (mapping offsets
    //    are section-relative, so the shift needs no fixup).
    copy_span(out, input, rel(footer.meta_payloads), rel(footer.str_tbl))?;

    // 6. String table: two streaming passes over the blob — cumulative
    //    end offsets first (rewrites change entry lengths), then the
    //    blob with rewrites inline. The blob itself is never held
    //    whole: entries are fetched one window at a time, twice.
    let mut cursor = Cursor::new(input, rel(footer.str_tbl), comp_end);
    let string_count = cursor.u30()?;
    let mut ends = Vec::new();
    for _ in 0..string_count {
        ends.push(cursor.u30()? as usize);
    }
    let blob_len = ends.last().copied().unwrap_or(0);
    let blob_start = cursor.pos;
    cursor.skip(blob_len)?; // bounds-check the blob without mapping it
    if cursor.pos != rel(footer.comp_idx_start) {
        return Err(malformed(
            (cursor.pos - comp_start) as isize,
            "string table blob does not end at the component index",
        ));
    }
    let length_delta = to.len() as i64 - from.len() as i64;
    // Capacity from `ends.len()` is bytes already parsed, never a
    // hostile count.
    let mut new_ends = Vec::with_capacity(ends.len());
    let mut matched = Vec::with_capacity(ends.len());
    let mut new_end = 0i64;
    let mut prev = 0usize;
    for &end in &ends {
        if end < prev {
            return Err(malformed(
                end as isize,
                "string table ends are not monotonic",
            ));
        }
        if end > blob_len {
            return Err(malformed(end as isize, "string table end beyond the blob"));
        }
        // In bounds by the two checks above plus the blob's own skip
        // proof; the `?` keeps even a broken invariant a refusal.
        let entry = input.slice(blob_start + prev, blob_start + end)?;
        prev = end;
        let occurrences = if finder.find(entry).is_some() {
            ensure_ascii(entry)?;
            count_matches(entry, &finder)
        } else {
            0
        };
        new_end += entry.len() as i64 + occurrences as i64 * length_delta;
        new_ends.push(u30_value(new_end)?);
        matched.push(occurrences > 0);
    }
    out.u30(string_count as usize)?;
    for &end in &new_ends {
        out.u30(end as usize)?;
    }
    let mut prev = 0usize;
    for (&end, &hit) in ends.iter().zip(&matched) {
        let entry = input
            .slice(blob_start + prev, blob_start + end)
            .map_err(|_| malformed(end as isize, "string table end beyond the blob"))?;
        prev = end;
        if hit {
            write_replaced(out, entry, &finder, to)?;
            stats.strings += 1;
        } else {
            out.bytes(entry)?;
        }
    }

    // 7. Component index footer: padding first so the component's
    //    total size stays 8-aligned (the concatenation contract), the
    //    index start recorded at the pre-padding position, and
    //    totalSize as the final size. Mirrors the official writer's
    //    own arithmetic.
    let idx_start = at(out)?;
    let total = idx_start + Footer::len(footer.library_count);
    out.pad((base + total).div_ceil(8) * 8 - (base + total))?;
    out.u32(u32_field(footer.src_tbl as i64)?)?;
    out.u32(u32_field(footer.const_tbl as i64 + delta)?)?;
    out.u32(u32_field(footer.const_idx as i64 + delta)?)?;
    out.u32(u32_field(footer.link_tbl as i64 + delta)?)?;
    out.u32(u32_field(new_payloads as i64)?)?;
    out.u32(u32_field(footer.meta_mappings as i64 + shift)?)?;
    out.u32(u32_field(footer.str_tbl as i64 + shift)?)?;
    out.u32(u32_field(idx_start as i64)?)?;
    out.u32(footer.main_ref)?;
    for &offset in &footer.library_offsets {
        out.u32(offset)?;
    }
    out.u32(footer.library_count)?;
    // totalSize counts the u32 that carries it.
    let size = at(out)? + 4;
    debug_assert_eq!(size % 8, 0, "total size must stay 8-aligned");
    out.u32(u32_field(size as i64)?)
}

/// Writes one source-table string, rewritten when it matches the
/// needle (length prefix first, then the bytes).
fn write_source_string<W: Write>(
    out: &mut Out<W>,
    string: &[u8],
    finder: &memmem::Finder<'_>,
    to: &[u8],
    scratch: &mut Vec<u8>,
    stats: &mut DillRewrite,
) -> Result<()> {
    scratch.clear();
    if rewrite_into(string, finder, to, scratch)?.is_some() {
        out.u30(scratch.len())?;
        out.bytes(scratch)?;
        stats.source_uris += 1;
    } else {
        out.u30(string.len())?;
        out.bytes(string)?;
    }
    Ok(())
}

/// Counts non-overlapping left-to-right matches of `finder`'s needle.
fn count_matches(bytes: &[u8], finder: &memmem::Finder<'_>) -> usize {
    let mut count = 0usize;
    let mut cursor = 0usize;
    while let Some(at) = finder.find(&bytes[cursor..]) {
        cursor += at + finder.needle().len();
        count += 1;
    }
    count
}

/// Rewrites `string` into `scratch` when it matches the needle;
/// `Ok(None)` (with `scratch` cleared) when it does not. Refuses a
/// matched string that is not pure ASCII.
fn rewrite_into(
    string: &[u8],
    finder: &memmem::Finder<'_>,
    to: &[u8],
    scratch: &mut Vec<u8>,
) -> Result<Option<()>> {
    scratch.clear();
    if finder.find(string).is_none() {
        return Ok(None);
    }
    ensure_ascii(string)?;
    let mut cursor = 0usize;
    while let Some(at) = finder.find(&string[cursor..]) {
        let at = cursor + at;
        scratch.extend_from_slice(&string[cursor..at]);
        scratch.extend_from_slice(to);
        cursor = at + finder.needle().len();
    }
    scratch.extend_from_slice(&string[cursor..]);
    Ok(Some(()))
}

/// Streams `bytes` with every needle occurrence replaced by `to`. The
/// caller has already validated the ASCII and match guarantees.
fn write_replaced<W: Write>(
    out: &mut Out<W>,
    bytes: &[u8],
    finder: &memmem::Finder<'_>,
    to: &[u8],
) -> Result<()> {
    let needle = finder.needle().len();
    let mut cursor = 0usize;
    while let Some(at) = finder.find(&bytes[cursor..]) {
        let at = cursor + at;
        out.bytes(&bytes[cursor..at])?;
        out.bytes(to)?;
        cursor = at + needle;
    }
    out.bytes(&bytes[cursor..])
}

fn ensure_ascii(bytes: &[u8]) -> Result<()> {
    if bytes.iter().any(|&b| b > 0x7F) {
        return Err(BcacheError::Unsupported(
            "non-ASCII string matched by the rewrite prefix; refusing".into(),
        ));
    }
    Ok(())
}

/// A bounds-checked read cursor over the dill, serving bytes through
/// the reader's window. Positions are file-relative and reads never
/// pass `limit` (the enclosing component's end, restoring the
/// component-local refusal behavior of the original whole-component
/// cursor). One span is live at a time: the next read may re-map the
/// window underneath an unconsumed slice, and the borrow checker
/// enforces that.
struct Cursor<'a> {
    reader: &'a mut Reader,
    pos: usize,
    limit: usize,
}

impl Cursor<'_> {
    fn new(reader: &mut Reader, pos: usize, limit: usize) -> Cursor<'_> {
        Cursor { reader, pos, limit }
    }

    fn u8(&mut self) -> Result<u8> {
        if self.pos >= self.limit {
            return Err(malformed(self.pos as isize, "read past the component end"));
        }
        let byte = self.reader.slice(self.pos, self.pos + 1)?[0];
        self.pos += 1;
        Ok(byte)
    }

    /// The kernel u30 varint: 1, 2, or 4 bytes, big-endian payload.
    fn u30(&mut self) -> Result<u32> {
        let x = self.u8()?;
        if x & 0x80 == 0 {
            return Ok(u32::from(x));
        }
        if x & 0x40 == 0 {
            let lo = self.u8()?;
            return Ok((u32::from(x & 0x3F) << 8) | u32::from(lo));
        }
        let [b1, b2, b3] = [self.u8()?, self.u8()?, self.u8()?];
        Ok((u32::from(x & 0x3F) << 24)
            | (u32::from(b1) << 16)
            | (u32::from(b2) << 8)
            | u32::from(b3))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok((u32::from(self.u8()?) << 24)
            | (u32::from(self.u8()?) << 16)
            | (u32::from(self.u8()?) << 8)
            | u32::from(self.u8()?))
    }

    fn skip_u32s(&mut self, count: u32) -> Result<()> {
        for _ in 0..count {
            self.u32()?;
        }
        Ok(())
    }

    /// Bounds-checks `len` bytes and advances without reading them —
    /// the blob is streamed entry by entry later, never held whole.
    fn skip(&mut self, len: usize) -> Result<()> {
        let Some(end) = self.pos.checked_add(len) else {
            return Err(malformed(self.pos as isize, "length overflows"));
        };
        if end > self.limit {
            return Err(malformed(self.pos as isize, "slice past the component end"));
        }
        self.pos = end;
        Ok(())
    }

    fn take(&mut self, len: usize) -> Result<&[u8]> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or_else(|| malformed(self.pos as isize, "length overflows"))?;
        if end > self.limit {
            return Err(malformed(self.pos as isize, "slice past the component end"));
        }
        let slice = self.reader.slice(self.pos, end)?;
        self.pos = end;
        Ok(slice)
    }

    /// A u30-length-prefixed byte string (source-table strings).
    fn lp_bytes(&mut self) -> Result<&[u8]> {
        let len = self.u30()? as usize;
        self.take(len)
    }

    /// The raw span of a counted u30 run (line-start deltas, coverage
    /// references): the count and its values, skipped structurally and
    /// copied verbatim.
    fn u30_span(&mut self) -> Result<&[u8]> {
        let start = self.pos;
        let count = self.u30()?;
        for _ in 0..count {
            self.u30()?;
        }
        self.reader.slice(start, self.pos)
    }
}

/// A position-counting writer: every offset the rewrite emits comes
/// from `len`, so footer fields and the source-table index can never
/// disagree with the bytes on disk.
struct Out<W: Write> {
    inner: W,
    len: usize,
}

impl<W: Write> Out<W> {
    fn new(inner: W) -> Self {
        Self { inner, len: 0 }
    }

    fn bytes(&mut self, bytes: &[u8]) -> Result<()> {
        self.inner.write_all(bytes)?;
        self.len += bytes.len();
        Ok(())
    }

    fn pad(&mut self, len: usize) -> Result<()> {
        const ZEROS: [u8; 8] = [0; 8];
        let mut left = len;
        while left > 0 {
            let take = left.min(ZEROS.len());
            self.bytes(&ZEROS[..take])?;
            left -= take;
        }
        Ok(())
    }

    fn u32(&mut self, value: u32) -> Result<()> {
        self.bytes(&value.to_be_bytes())
    }

    /// The minimal u30 encoding of a length.
    fn u30(&mut self, value: usize) -> Result<()> {
        let value = u30_value(value as i64)?;
        if value < 0x80 {
            self.bytes(&[value as u8])
        } else if value < 0x4000 {
            self.bytes(&[(value >> 8) as u8 | 0x80, value as u8])
        } else {
            self.bytes(&[
                (value >> 24) as u8 | 0xC0,
                ((value >> 16) & 0xFF) as u8,
                ((value >> 8) & 0xFF) as u8,
                (value & 0xFF) as u8,
            ])
        }
    }
}
/// Validates a u30-bound non-negative length.
fn u30_value(value: i64) -> Result<u32> {
    if value < 0 {
        return Err(BcacheError::Unsupported(
            "rewritten length underflowed".into(),
        ));
    }
    let value = u32::try_from(value)
        .map_err(|_| BcacheError::Unsupported(format!("length {value} exceeds the u32 range")))?;
    if value >> 30 != 0 {
        return Err(BcacheError::Unsupported(format!(
            "length {value} exceeds the u30 range"
        )));
    }
    Ok(value)
}

fn offset_overflow() -> BcacheError {
    BcacheError::Unsupported("component grew past the u32 offset range".into())
}

/// An i64 table position narrowed to the footer's u32 range (negative
/// only if the layout arithmetic itself broke, which the `try_from`
/// turns into a refusal).
fn u32_field(value: i64) -> Result<u32> {
    u32::try_from(value).map_err(|_| offset_overflow())
}

/// Appends a u30 in its minimal encoding (what the parser reads).
#[cfg(test)]
pub(crate) fn push_u30(out: &mut Vec<u8>, value: usize) {
    let value = value as u32;
    if value < 0x80 {
        out.push(value as u8);
    } else if value < 0x4000 {
        out.push((value >> 8) as u8 | 0x80);
        out.push(value as u8);
    } else {
        out.push((value >> 24) as u8 | 0xC0);
        out.push(((value >> 16) & 0xFF) as u8);
        out.push(((value >> 8) & 0xFF) as u8);
        out.push((value & 0xFF) as u8);
    }
}

/// Builds one synthetic kernel component with minimal u30 encodings, a
/// truthful footer, 8-aligned payloads, and an 8-aligned total size
/// (the official writer's contract), so a no-op rewrite of it is
/// byte-identical. Shared by this module's tests and the
/// materialize/orchestrator integration tests.
#[cfg(test)]
pub(crate) fn synthetic_component(
    uris: &[(&str, &str)],
    strings: &[&str],
    library_count: u32,
) -> Vec<u8> {
    // AST region: header + arbitrary filler (copied verbatim).
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(&MAGIC.to_be_bytes());
    out.extend_from_slice(&138_u32.to_be_bytes());
    out.extend_from_slice(b"852b3e3608"); // ascii sdk hash
    out.push(0); // empty problemsAsJson list
    out.extend_from_slice(b"AST-FILLER");
    let src_tbl = out.len();

    // Source table: count, entries, trailing absolute offsets.
    out.extend_from_slice(&(uris.len() as u32).to_be_bytes());
    let mut entry_offsets = Vec::new();
    for (uri, import_uri) in uris {
        entry_offsets.push(out.len());
        push_u30(&mut out, uri.len());
        out.extend_from_slice(uri.as_bytes());
        let source = format!("// source of {uri}\nconst k = 1;\n");
        push_u30(&mut out, source.len());
        out.extend_from_slice(source.as_bytes());
        push_u30(&mut out, 2); // two line starts
        push_u30(&mut out, 0);
        push_u30(&mut out, 14);
        push_u30(&mut out, import_uri.len());
        out.extend_from_slice(import_uri.as_bytes());
        push_u30(&mut out, 0); // no coverage refs
    }
    for offset in entry_offsets {
        out.extend_from_slice(&(offset as u32).to_be_bytes());
    }
    let const_tbl = out.len();

    // Constant + link + metadata region: filler with the payloads
    // 8-aligned, as the layout promises.
    out.extend_from_slice(b"CONST-TABLE");
    out.extend_from_slice(b"CONST-IDX");
    out.extend_from_slice(b"LINK");
    while out.len() % 8 != 0 {
        out.push(0);
    }
    let meta_payloads = out.len();
    out.extend_from_slice(b"PAYLOADS-MAPPINGS-REGION");
    let str_tbl = out.len();

    // String table: count, cumulative ends, blob.
    push_u30(&mut out, strings.len());
    let mut blob = Vec::new();
    let mut ends = Vec::new();
    for s in strings {
        blob.extend_from_slice(s.as_bytes());
        ends.push(blob.len());
    }
    for end in &ends {
        push_u30(&mut out, *end);
    }
    out.extend_from_slice(&blob);
    let comp_idx_start = out.len();

    // Footer, padded so the total size is 8-aligned; the index start
    // is recorded at the pre-padding position.
    let mut footer = Vec::new();
    footer.extend_from_slice(&(src_tbl as u32).to_be_bytes());
    footer.extend_from_slice(&(const_tbl as u32).to_be_bytes());
    footer.extend_from_slice(&((const_tbl + 4) as u32).to_be_bytes());
    footer.extend_from_slice(&((const_tbl + 8) as u32).to_be_bytes());
    footer.extend_from_slice(&(meta_payloads as u32).to_be_bytes());
    footer.extend_from_slice(&((meta_payloads + 8) as u32).to_be_bytes());
    footer.extend_from_slice(&(str_tbl as u32).to_be_bytes());
    footer.extend_from_slice(&(comp_idx_start as u32).to_be_bytes());
    footer.extend_from_slice(&0x1234_u32.to_be_bytes()); // main ref
    for i in 0..=library_count {
        footer.extend_from_slice(&(100 * (i + 1)).to_be_bytes());
    }
    footer.extend_from_slice(&library_count.to_be_bytes());
    footer.extend_from_slice(&[0; 4]); // totalSize, patched below
    let unaligned = out.len() + footer.len();
    let padding = unaligned.div_ceil(8) * 8 - unaligned;
    let total = unaligned + padding;
    let total_at = out.len() + padding + footer.len() - 4;
    for _ in 0..padding {
        out.push(0);
    }
    out.extend_from_slice(&footer);
    out[total_at..total_at + 4].copy_from_slice(&(total as u32).to_be_bytes());
    assert_eq!(out.len() % 8, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn write_fixture(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, bytes).expect("write fixture");
        path
    }

    /// An independent structural walk of one rewritten component of
    /// the dill under `input`: framing, footer, both tables, rebuilt
    /// indexes, and alignment — the checks the Dart spike's verify
    /// pass ran, on the OUTPUT.
    fn verify_component(
        input: &mut Reader,
        comp_start: usize,
        comp_len: usize,
        expect_uris: &[&str],
        expect_strings: &[&str],
    ) {
        let comp_end = comp_start + comp_len;
        let footer = parse_footer(input, comp_start, comp_len).expect("footer parses");
        assert_eq!(comp_len % 8, 0, "total size 8-aligned");
        assert_eq!(footer.meta_payloads % 8, 0, "payloads 8-aligned");
        assert_eq!(
            input.slice(comp_start, comp_start + 4).expect("head"),
            &MAGIC.to_be_bytes()[..],
            "header survives verbatim"
        );

        // Source table walk: uris, then the rebuilt trailing index.
        let mut cursor = Cursor::new(input, comp_start + footer.src_tbl, comp_end);
        let count = cursor.u32().expect("count");
        assert_eq!(count as usize, expect_uris.len());
        let mut uris = Vec::new();
        for _ in 0..count {
            uris.push(read_lp_string(&mut cursor));
            cursor.lp_bytes().expect("source");
            cursor.u30_span().expect("line starts");
            cursor.lp_bytes().expect("import uri");
            cursor.u30_span().expect("coverage");
        }
        let mut recorded = Vec::new();
        for _ in 0..count {
            recorded.push(cursor.u32().expect("offset"));
        }
        assert_eq!(
            cursor.pos,
            comp_start + footer.const_tbl,
            "source table walk ends at the constant table"
        );
        assert_eq!(uris, expect_uris);
        for offset in recorded {
            let mut probe = Cursor::new(input, comp_start + offset as usize, comp_end);
            let uri = read_lp_string(&mut probe);
            assert!(expect_uris.contains(&uri.as_str()), "index points at {uri}");
        }

        // String table walk: ends, then entries sliced one at a time.
        let mut cursor = Cursor::new(input, comp_start + footer.str_tbl, comp_end);
        let string_count = cursor.u30().expect("count");
        assert_eq!(string_count as usize, expect_strings.len());
        let mut ends = Vec::new();
        for _ in 0..string_count {
            ends.push(cursor.u30().expect("end") as usize);
        }
        let blob_len = ends.last().copied().unwrap_or(0);
        let blob_start = cursor.pos;
        assert_eq!(
            blob_start + blob_len,
            comp_start + footer.comp_idx_start,
            "blob ends at the component index"
        );
        let mut prev = 0usize;
        let mut strings = Vec::new();
        for end in ends {
            let entry = input
                .slice(blob_start + prev, blob_start + end)
                .expect("entry");
            strings.push(String::from_utf8(entry.to_vec()).expect("utf8"));
            prev = end;
        }
        assert_eq!(strings, expect_strings);
    }

    /// Verifies the single component occupying a whole file.
    fn verify_file_component(path: &Path, expect_uris: &[&str], expect_strings: &[&str]) {
        let mut reader = Reader::open(path).expect("open output");
        let len = reader.len;
        verify_component(&mut reader, 0, len, expect_uris, expect_strings);
    }

    /// The framing walk over a dill file on disk.
    fn file_component_ranges(path: &Path) -> Vec<Range<usize>> {
        let mut reader = Reader::open(path).expect("open");
        let len = reader.len;
        component_ranges(len, |at| reader.slice_u32(at)).expect("ranges")
    }

    fn read_lp_string(cursor: &mut Cursor<'_>) -> String {
        let len = cursor.u30().expect("length") as usize;
        String::from_utf8(cursor.take(len).expect("bytes").to_vec()).expect("utf8")
    }

    /// The framing walk over a multi-component file: components split
    /// by the backward size chain, and a no-match rewrite re-emits the
    /// file byte-identically.
    #[test]
    fn framing_walk_splits_multi_component_file() {
        let dirs = tempfile::tempdir().expect("tempdir");
        let a = synthetic_component(
            &[("file:///tree/a/lib/main.dart", "package:a/main.dart")],
            &["file:///tree/a/lib/main.dart", "dart:core"],
            1,
        );
        let b = synthetic_component(
            &[("file:///tree/b/t.dart", "")],
            &["file:///tree/b/t.dart"],
            0,
        );
        let mut file = a.clone();
        file.extend_from_slice(&b);
        let src = write_fixture(dirs.path(), "multi.dill", &file);
        let dst = dirs.path().join("out.dill");

        let stats =
            rewrite_file(&src, &dst, b"file:///nowhere/", b"file:///elsewhere/").expect("rewrite");
        assert_eq!(stats.components, 2);
        assert_eq!(stats.source_uris, 0);
        assert_eq!(stats.strings, 0);
        assert_eq!(
            fs::read(&dst).expect("output"),
            file,
            "no-op rewrite is byte-identical"
        );

        // The framing of the output still splits into the same sizes.
        let ranges = file_component_ranges(&dst);
        assert_eq!(ranges.len(), 2);
        assert_eq!(ranges[0].len(), a.len());
        assert_eq!(ranges[1].len(), b.len());
    }

    /// URI rewrites in both tables with BOTH length directions, proving
    /// the offset rebuild (trailing entry index, string-table ends,
    /// footer offsets) follows the growth and shrinkage.
    #[test]
    fn uri_rewrite_rebuilds_offsets_with_length_changes() {
        let dirs = tempfile::tempdir().expect("tempdir");
        let uris = [
            ("file:///tree/a/lib/main.dart", "package:a/main.dart"),
            (
                "file:///tree/a/test/widget_test.dart",
                "file:///tree/a/test/widget_test.dart",
            ),
            ("file:///pub-cache/flutter/lib/framework.dart", ""),
        ];
        let strings = [
            "file:///tree/a/lib/main.dart",
            "package:flutter/src/widgets/framework.dart",
            "file:///tree/a/.dart_tool/package_config.json",
        ];
        let input = synthetic_component(&uris, &strings, 2);
        let src = write_fixture(dirs.path(), "in.dill", &input);
        let dst = dirs.path().join("out.dill");

        // Longer target prefix.
        let stats = rewrite_file(
            &src,
            &dst,
            b"file:///tree/a/",
            b"file:///private/var/much-longer-tree-b/",
        )
        .expect("rewrite");
        assert_eq!(stats.components, 1);
        // uri of entries 1+2 and importUri of entry 2 (the package:
        // import of entry 1 and the pub-cache uri never match).
        assert_eq!(stats.source_uris, 3);
        assert_eq!(stats.strings, 2);
        let output = fs::read(&dst).expect("output");
        let expect_uris = [
            "file:///private/var/much-longer-tree-b/lib/main.dart",
            "file:///private/var/much-longer-tree-b/test/widget_test.dart",
            "file:///pub-cache/flutter/lib/framework.dart",
        ];
        let expect_imports = [
            "package:a/main.dart",
            "file:///private/var/much-longer-tree-b/test/widget_test.dart",
            "",
        ];
        let expect_strings = [
            "file:///private/var/much-longer-tree-b/lib/main.dart",
            "package:flutter/src/widgets/framework.dart",
            "file:///private/var/much-longer-tree-b/.dart_tool/package_config.json",
        ];
        verify_file_component(&dst, &expect_uris, &expect_strings);
        // The embedded source keeps the old prefix verbatim.
        assert!(
            memmem::find(&output, b"// source of file:///tree/a/lib/main.dart").is_some(),
            "embedded source text stays verbatim"
        );
        // Import URIs land through a fresh parse.
        let mut reader = Reader::open(&dst).expect("open output");
        let footer = parse_footer(&mut reader, 0, output.len()).expect("footer");
        let mut cursor = Cursor::new(&mut reader, footer.src_tbl, output.len());
        let count = cursor.u32().expect("count");
        let mut imports = Vec::new();
        for _ in 0..count {
            cursor.lp_bytes().expect("uri");
            cursor.lp_bytes().expect("source");
            cursor.u30_span().expect("lines");
            imports.push(read_lp_string(&mut cursor));
            cursor.u30_span().expect("coverage");
        }
        assert_eq!(imports, expect_imports);

        // Shorter target prefix off the same input: the offset rebuild
        // must shrink correctly too.
        let dst_short = dirs.path().join("out-short.dill");
        let stats =
            rewrite_file(&src, &dst_short, b"file:///tree/a/", b"file:///b/").expect("rewrite");
        assert_eq!(stats.source_uris, 3);
        assert_eq!(stats.strings, 2);
        verify_file_component(
            &dst_short,
            &[
                "file:///b/lib/main.dart",
                "file:///b/test/widget_test.dart",
                "file:///pub-cache/flutter/lib/framework.dart",
            ],
            &[
                "file:///b/lib/main.dart",
                "package:flutter/src/widgets/framework.dart",
                "file:///b/.dart_tool/package_config.json",
            ],
        );
    }

    /// Lengths straddling the u30 1→2 byte encoding boundary: after the
    /// rewrite, uri lengths and cumulative string ends cross it, so a
    /// mis-encoded width would break the verify walk.
    #[test]
    fn u30_boundary_lengths_change_encoding_widths() {
        let dirs = tempfile::tempdir().expect("tempdir");
        let base = "file:///t/a/.dart".len();
        let short = format!("file:///t/a/{}.dart", "x".repeat(127 - base));
        let long = format!("file:///t/a/{}.dart", "x".repeat(128 - base));
        assert_eq!(short.len(), 127);
        assert_eq!(long.len(), 128);
        let input = synthetic_component(
            &[(short.as_str(), ""), (long.as_str(), "")],
            &[short.as_str(), long.as_str()],
            0,
        );
        let src = write_fixture(dirs.path(), "u30.dill", &input);
        let dst = dirs.path().join("out.dill");
        rewrite_file(&src, &dst, b"file:///t/a/", b"file:///t/ab/").expect("rewrite");
        let expect_short = short.replace("file:///t/a/", "file:///t/ab/");
        let expect_long = long.replace("file:///t/a/", "file:///t/ab/");
        assert_eq!(expect_short.len(), 128, "crossed into 2-byte u30");
        assert_eq!(expect_long.len(), 129);
        verify_file_component(
            &dst,
            &[expect_short.as_str(), expect_long.as_str()],
            &[expect_short.as_str(), expect_long.as_str()],
        );
    }

    /// The version gate: wrong magic, an unmodeled format version (122,
    /// the last layout with the `compilationMode` footer field, and an
    /// unknown newer one), non-ASCII sdkHash, and unframeable garbage
    /// each refuse the file.
    #[test]
    fn version_and_magic_gates_refuse() {
        let dirs = tempfile::tempdir().expect("tempdir");
        let dst = dirs.path().join("o.dill");

        for version in [122_u32, 139] {
            let mut wrong_version =
                synthetic_component(&[("file:///a.dart", "")], &["file:///a.dart"], 0);
            wrong_version[4..8].copy_from_slice(&version.to_be_bytes());
            let src = write_fixture(dirs.path(), "wrong-version.dill", &wrong_version);
            let err =
                rewrite_file(&src, &dst, b"file:///a/", b"file:///b/").expect_err("version gate");
            assert!(err.to_string().contains("version"), "{err}");
        }

        let mut wrong_magic =
            synthetic_component(&[("file:///a.dart", "")], &["file:///a.dart"], 0);
        wrong_magic[0..4].copy_from_slice(&0xDEAD_BEEF_u32.to_be_bytes());
        let src = write_fixture(dirs.path(), "wrong-magic.dill", &wrong_magic);
        let err = rewrite_file(&src, &dst, b"file:///a/", b"file:///b/").expect_err("magic gate");
        assert!(err.to_string().contains("magic"), "{err}");

        let mut bad_hash = synthetic_component(&[("file:///a.dart", "")], &["file:///a.dart"], 0);
        bad_hash[8..18].copy_from_slice(&[0xFF; 10]);
        let src = write_fixture(dirs.path(), "bad-hash.dill", &bad_hash);
        let err =
            rewrite_file(&src, &dst, b"file:///a/", b"file:///b/").expect_err("sdk hash gate");
        assert!(err.to_string().contains("sdk hash"), "{err}");

        let src = write_fixture(dirs.path(), "garbage.dill", b"\x00\x00\x00\x00");
        let err = rewrite_file(&src, &dst, b"file:///a/", b"file:///b/").expect_err("framing");
        assert!(matches!(err, BcacheError::Unsupported(_)), "{err}");
    }

    /// Every modeled format passes the version gate and rewrites the
    /// same tables (the layouts are identical, see [`FORMAT_VERSIONS`];
    /// real SDK dills of each enabled flutter line are exercised by the
    /// orchestrator's region harness).
    #[test]
    fn modeled_formats_pass_the_version_gate() {
        let dirs = tempfile::tempdir().expect("tempdir");
        let dst = dirs.path().join("o.dill");
        for &version in FORMAT_VERSIONS {
            let mut dill =
                synthetic_component(&[("file:///a/x.dart", "")], &["file:///a/x.dart"], 0);
            dill[4..8].copy_from_slice(&version.to_be_bytes());
            let src = write_fixture(dirs.path(), "modeled.dill", &dill);
            let stats = rewrite_file(&src, &dst, b"file:///a/", b"file:///b/")
                .unwrap_or_else(|err| panic!("format {version} refused: {err}"));
            assert_eq!(
                (stats.source_uris, stats.strings),
                (1, 1),
                "format {version}"
            );
        }
    }

    /// A matched non-ASCII string refuses the whole file; an UNmatched
    /// non-ASCII string (foreign UTF-8 elsewhere in the tree) does not.
    #[test]
    fn non_ascii_matched_string_refuses_and_unmatched_is_fine() {
        let dirs = tempfile::tempdir().expect("tempdir");
        let mixed = "file:///t/a/über.dart";
        let input = synthetic_component(
            &[("file:///t/a/plain.dart", ""), (mixed, "")],
            &["file:///t/a/plain.dart", mixed],
            0,
        );
        let src = write_fixture(dirs.path(), "mixed.dill", &input);
        let dst = dirs.path().join("out.dill");
        let err = rewrite_file(&src, &dst, b"file:///t/a/", b"file:///t/b/")
            .expect_err("non-ASCII matched string");
        assert!(err.to_string().contains("non-ASCII"), "{err}");

        let ok = synthetic_component(
            &[("file:///t/a/plain.dart", "")],
            &["file:///t/a/plain.dart", "package:ü/esc.dart"],
            0,
        );
        let src = write_fixture(dirs.path(), "unmatched.dill", &ok);
        let stats =
            rewrite_file(&src, &dst, b"file:///t/a/", b"file:///t/b/").expect("rewrite proceeds");
        assert_eq!(stats.strings, 1);
    }

    /// Determinism: the same input and prefixes produce identical
    /// bytes and identical stats.
    #[test]
    fn same_input_twice_produces_identical_bytes() {
        let dirs = tempfile::tempdir().expect("tempdir");
        let input = synthetic_component(
            &[
                ("file:///tree/a/lib/main.dart", "package:a/main.dart"),
                ("file:///tree/a/test/x.dart", "file:///tree/a/test/x.dart"),
            ],
            &["file:///tree/a/lib/main.dart", "dart:core"],
            1,
        );
        let src = write_fixture(dirs.path(), "det.dill", &input);
        let first = dirs.path().join("det-1.dill");
        let second = dirs.path().join("det-2.dill");
        let stats_a = rewrite_file(
            &src,
            &first,
            b"file:///tree/a/",
            b"file:///private/tmp/tree-b/",
        )
        .expect("first");
        let stats_b = rewrite_file(
            &src,
            &second,
            b"file:///tree/a/",
            b"file:///private/tmp/tree-b/",
        )
        .expect("second");
        assert_eq!(stats_a, stats_b);
        assert_eq!(
            fs::read(&first).expect("first"),
            fs::read(&second).expect("second"),
            "deterministic output"
        );
    }

    /// The prefix pair the materializer builds is anchored by scheme
    /// and trailing slash: a sibling tree whose root EXTENDS the old
    /// root never matches.
    #[test]
    fn prefix_is_anchored_by_scheme_and_trailing_slash() {
        let dirs = tempfile::tempdir().expect("tempdir");
        let input = synthetic_component(
            &[
                ("file:///tree/a/lib/main.dart", ""),
                ("file:///tree/aux/lib/main.dart", ""),
            ],
            &["file:///tree/a/lib/main.dart"],
            0,
        );
        let src = write_fixture(dirs.path(), "anchor.dill", &input);
        let dst = dirs.path().join("out.dill");
        let stats =
            rewrite_file(&src, &dst, b"file:///tree/a/", b"file:///tree/b/").expect("rewrite");
        assert_eq!(stats.source_uris, 1, "the sibling tree does not match");
        verify_file_component(
            &dst,
            &[
                "file:///tree/b/lib/main.dart",
                "file:///tree/aux/lib/main.dart",
            ],
            &["file:///tree/b/lib/main.dart"],
        );
    }

    /// Empty and non-ASCII prefixes are invalid inputs.
    #[test]
    fn invalid_prefixes_are_rejected() {
        let dirs = tempfile::tempdir().expect("tempdir");
        let src = write_fixture(dirs.path(), "x.dill", &synthetic_component(&[], &[], 0));
        let dst = dirs.path().join("o");
        let err = rewrite_file(&src, &dst, b"", b"/b/").expect_err("empty from");
        assert!(matches!(err, BcacheError::Invalid(_)), "{err}");
        let err = rewrite_file(&src, &dst, b"/a/", "ü".as_bytes()).expect_err("non-ASCII to");
        assert!(matches!(err, BcacheError::Invalid(_)), "{err}");
    }

    /// A footer whose totalSize disagrees with the framing refuses.
    #[test]
    fn footer_size_mismatch_refuses() {
        let dirs = tempfile::tempdir().expect("tempdir");
        let mut input = synthetic_component(&[("file:///a.dart", "")], &["file:///a.dart"], 0);
        let patched = (input.len() + 8) as u32;
        let last = input.len();
        input[last - 4..].copy_from_slice(&patched.to_be_bytes());
        let src = write_fixture(dirs.path(), "bad-size.dill", &input);
        let err = rewrite_file(&src, &dirs.path().join("o"), b"file:///a/", b"file:///b/")
            .expect_err("size mismatch");
        assert!(matches!(err, BcacheError::Unsupported(_)), "{err}");
    }

    /// Byte-equality golden: the streaming rewrite must produce the
    /// exact bytes the original whole-file implementation produced for
    /// the same input. The constant was captured from the pre-streaming
    /// implementation; set `DQ_GOLDEN_CAPTURE=1` to print the current
    /// bytes (regenerate only after an INTENTIONAL format change, and
    /// re-verify against a real dill).
    #[test]
    fn golden_rewrite_bytes_are_pinned() {
        let dirs = tempfile::tempdir().expect("tempdir");
        let mut input = synthetic_component(
            &[
                ("file:///tree/a/lib/main.dart", "package:a/main.dart"),
                (
                    "file:///tree/a/test/widget_test.dart",
                    "file:///tree/a/test/widget_test.dart",
                ),
            ],
            &[
                "file:///tree/a/lib/main.dart",
                "dart:core",
                "package:flutter/src/widgets/framework.dart",
            ],
            1,
        );
        input.extend_from_slice(&synthetic_component(
            &[("file:///tree/a/lib/src/other.dart", "")],
            &[
                "file:///tree/a/lib/src/other.dart",
                "file:///tree/a/.dart_tool/package_config.json",
            ],
            0,
        ));
        let src = write_fixture(dirs.path(), "golden.dill", &input);
        let dst = dirs.path().join("out.dill");
        let stats = rewrite_file(
            &src,
            &dst,
            b"file:///tree/a/",
            b"file:///private/var/much-longer-tree-b/",
        )
        .expect("rewrite");
        assert_eq!(stats.components, 2);
        assert_eq!(stats.source_uris, 4);
        assert_eq!(stats.strings, 3);
        let output = fs::read(&dst).expect("output");
        if std::env::var_os("DQ_GOLDEN_CAPTURE").is_some() {
            println!("GOLDEN: {}", output.escape_ascii());
            return;
        }
        const GOLDEN: &[u8] = b"\x90\xab\xcd\xef\x00\x00\x00\x8a852b3e3608\x00AST-FILLER\x00\x00\x00\x024file:///private/var/much-longer-tree-b/lib/main.dart7// source of file:///tree/a/lib/main.dart\nconst k = 1;\n\x02\x00\x0e\x13package:a/main.dart\x00<file:///private/var/much-longer-tree-b/test/widget_test.dart?// source of file:///tree/a/test/widget_test.dart\nconst k = 1;\n\x02\x00\x0e<file:///private/var/much-longer-tree-b/test/widget_test.dart\x00\x00\x00\x00!\x00\x00\x00\xa6CONST-TABLECONST-IDXLINK\x00\x00\x00\x00PAYLOADS-MAPPINGS-REGION\x034=gfile:///private/var/much-longer-tree-b/lib/main.dartdart:corepackage:flutter/src/widgets/framework.dart\x00\x00\x00\x00\x1d\x00\x00\x01l\x00\x00\x01p\x00\x00\x01t\x00\x00\x01\x88\x00\x00\x01\x90\x00\x00\x01\xa0\x00\x00\x02\x0b\x00\x00\x124\x00\x00\x00d\x00\x00\x00\xc8\x00\x00\x00\x01\x00\x00\x02@\x90\xab\xcd\xef\x00\x00\x00\x8a852b3e3608\x00AST-FILLER\x00\x00\x00\x019file:///private/var/much-longer-tree-b/lib/src/other.dart<// source of file:///tree/a/lib/src/other.dart\nconst k = 1;\n\x02\x00\x0e\x00\x00\x00\x00\x00!CONST-TABLECONST-IDXLINK\x00\x00\x00\x00\x00\x00\x00PAYLOADS-MAPPINGS-REGION\x029~file:///private/var/much-longer-tree-b/lib/src/other.dartfile:///private/var/much-longer-tree-b/.dart_tool/package_config.json\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x1d\x00\x00\x00\xa1\x00\x00\x00\xa5\x00\x00\x00\xa9\x00\x00\x00\xc0\x00\x00\x00\xc8\x00\x00\x00\xd8\x00\x00\x01Y\x00\x00\x124\x00\x00\x00d\x00\x00\x00\x00\x00\x00\x01\x90";
        assert_eq!(output, GOLDEN, "rewrite bytes must stay pinned");
    }

    /// Malformed dills refuse everywhere — no panic, no abort, and no
    /// rescue by the panic containment: each refusal message must come
    /// from a checked parse path, not the wrapper ("panicked" would
    /// mean a slice or arithmetic path fired instead of a check).
    #[test]
    fn malformed_dills_refuse_without_panicking() {
        let dirs = tempfile::tempdir().expect("tempdir");
        let dst = dirs.path().join("out.dill");
        let from = b"file:///tree/a/" as &[u8];
        let to = b"file:///tree/b/" as &[u8];
        let refuse = |name: &str, bytes: &[u8]| {
            let src = write_fixture(dirs.path(), &format!("{name}.dill"), bytes);
            match rewrite_file(&src, &dst, from, to) {
                Ok(stats) => panic!("{name}: malformed dill rewrote ({stats:?})"),
                Err(err) => {
                    assert!(matches!(err, BcacheError::Unsupported(_)), "{name}: {err}");
                    assert!(
                        !err.to_string().contains("panicked"),
                        "{name}: refused by panic containment, not by a check: {err}"
                    );
                }
            }
        };

        let valid = synthetic_component(
            &[("file:///tree/a/lib/main.dart", "package:a/main.dart")],
            &["file:///tree/a/lib/main.dart", "dart:core"],
            1,
        );
        let footer = {
            let path = write_fixture(dirs.path(), "valid.dill", &valid);
            let mut reader = Reader::open(&path).expect("reader");
            parse_footer(&mut reader, 0, valid.len()).expect("fixture footer")
        };

        // Truncation at every framing and structure boundary class:
        // file level, header fields, table starts, component index,
        // and the footer's re-read fields.
        let cuts = [
            0,
            1,
            3,
            4,
            5,
            7,
            8,
            9,
            17,
            18,
            footer.src_tbl,
            footer.src_tbl + 1,
            footer.const_tbl,
            footer.meta_payloads,
            footer.str_tbl,
            footer.str_tbl + 1,
            footer.comp_idx_start,
            valid.len() - 8,
            valid.len() - 4,
            valid.len() - 1,
        ];
        for cut in cuts {
            if cut < valid.len() {
                refuse(&format!("cut-{cut}"), &valid[..cut]);
            }
        }
        // The same, inside the second component of an append log.
        let mut two = valid.clone();
        two.extend_from_slice(&synthetic_component(
            &[("file:///tree/a/b.dart", "")],
            &["file:///tree/a/b.dart"],
            0,
        ));
        refuse("cut-mid-second-component", &two[..valid.len() + 10]);

        // A string-table end larger than the blob it indexes into (the
        // classic early-end-vs-late-end slice panic shape).
        let mut ends_hostile = synthetic_component(
            &[("file:///tree/a/x.dart", "")],
            &["aaaaaaaaaaaaaaaa", "b"],
            0,
        );
        let hostile_footer = {
            let path = write_fixture(dirs.path(), "ends-hostile-base.dill", &ends_hostile);
            let mut reader = Reader::open(&path).expect("reader");
            parse_footer(&mut reader, 0, ends_hostile.len()).expect("fixture footer")
        };
        // The ends sit after the one-byte u30 count; 16 becomes 32,
        // past the 17-byte blob, without changing any encoding width.
        ends_hostile[hostile_footer.str_tbl + 1] = 32;
        refuse("string-end-beyond-blob", &ends_hostile);

        // Absurd counts: capacities never derive from parsed counts,
        // so each of these is a bounded walk to an end-of-bytes
        // refusal, never a multi-gigabyte allocation.
        let mut source_count = valid.clone();
        source_count[footer.src_tbl..footer.src_tbl + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        refuse("source-count-u32-max", &source_count);

        let mut string_count = valid.clone();
        // 0xFF is a 4-byte u30 head: the count swallows the next three
        // bytes and lands far past the file's end.
        string_count[footer.str_tbl] = 0xFF;
        refuse("string-count-u30-max", &string_count);

        let mut library_count = valid.clone();
        let last = library_count.len();
        library_count[last - 8..last - 4].copy_from_slice(&u32::MAX.to_be_bytes());
        refuse("library-count-u32-max", &library_count);

        let mut chain = valid.clone();
        let last = chain.len();
        chain[last - 4..].copy_from_slice(&u32::MAX.to_be_bytes());
        refuse("component-size-u32-max", &chain);

        // A footer whose table offsets are out of order. The footer's
        // bytes begin at `len - fixed fields` (the recorded
        // comp_idx_start is the pre-padding position), and its first
        // field is src_tbl.
        let mut order = valid.clone();
        let footer_at = valid.len() - Footer::len(footer.library_count);
        order[footer_at..footer_at + 4]
            .copy_from_slice(&(footer.const_tbl as u32 + 1).to_be_bytes());
        refuse("table-order-violated", &order);

        // Zero-length tables are valid, not hostile: both tables empty
        // and the rewrite is a byte-identical no-op.
        let empty = synthetic_component(&[], &[], 0);
        let src = write_fixture(dirs.path(), "empty-tables.dill", &empty);
        let stats = rewrite_file(&src, &dst, from, to).expect("empty tables rewrite");
        assert_eq!(
            stats,
            DillRewrite {
                components: 1,
                source_uris: 0,
                strings: 0,
            }
        );
        assert_eq!(
            fs::read(&dst).expect("output"),
            empty,
            "no-op stays byte-identical"
        );
    }

    /// Panic containment (A17): even a panic on a poisoned path must
    /// surface as a refusal — never a crashed build. The flag trips
    /// the next rewrite before it touches the filesystem.
    #[test]
    fn poisoned_rewrite_returns_refusal_not_panic() {
        let dirs = tempfile::tempdir().expect("tempdir");
        let src = write_fixture(
            dirs.path(),
            "valid.dill",
            &synthetic_component(
                &[("file:///tree/a/m.dart", "")],
                &["file:///tree/a/m.dart"],
                0,
            ),
        );
        let dst = dirs.path().join("out.dill");
        POISON_REWRITE.store(true, Ordering::SeqCst);
        let err = rewrite_file(&src, &dst, b"file:///tree/a/", b"file:///tree/b/")
            .expect_err("poisoned rewrite must refuse");
        assert!(matches!(err, BcacheError::Unsupported(_)), "{err}");
        assert!(err.to_string().contains("panicked"), "{err}");
    }

    /// Peak-RSS bound (A20): the rewrite streams — resident memory
    /// tracks the release window and the per-string bookkeeping, never
    /// the dill. Measured as a difference: the parent builds two dills
    /// an order of magnitude apart in size (unmeasured), a spawned
    /// copy of this test binary rewrites each, and the bigger child's
    /// polled peak RSS must stay within one flat budget of the
    /// smaller's.
    ///
    /// The observation channel is `ps -o rss=` sampled while the child
    /// runs — CURRENT resident bytes, not `ru_maxrss`. That distinction
    /// is what makes the bound platform-independent: Linux carries the
    /// pre-exec address space's high-water RSS across `exec`
    /// (`setmax_mm_hiwater_rss`), so a `ru_maxrss`-based child reports
    /// its parent's fixture-building footprint (~240 MB observed on CI
    /// runners) no matter what the rewrite does; differencing two such
    /// readings only cancels the floor when both children inherit the
    /// same one, and any asymmetry in spawn timing or allocator state
    /// leaks straight into the delta. `ps rss` reads the live resident
    /// set instead — page cache, allocator arenas, and kernel
    /// accounting differences all land inside the same KiB unit on
    /// Linux and macOS — so the two children differ only by the memory
    /// the rewrite itself holds.
    #[test]
    #[cfg(unix)]
    fn rewrite_peak_rss_is_independent_of_dill_size() {
        if std::env::var_os("DQ_BCACHE_RSS_CHILD").is_some() {
            // Child payload: rewrite only; the parent measures this
            // process's peak.
            let src =
                PathBuf::from(std::env::var_os("DQ_BCACHE_RSS_SRC").expect("DQ_BCACHE_RSS_SRC"));
            let dst =
                PathBuf::from(std::env::var_os("DQ_BCACHE_RSS_DST").expect("DQ_BCACHE_RSS_DST"));
            rewrite_file(&src, &dst, b"file:///tree/a/", b"file:///tree/b/")
                .expect("child rewrite");
            return;
        }
        let dirs = tempfile::tempdir().expect("tempdir");
        let big = dirs.path().join("big.dill");
        build_large_dill(&big, 260_000);
        let small = dirs.path().join("small.dill");
        build_large_dill(&small, 16_000);
        let peak_big = child_peak_rss(&big, &dirs.path().join("out-big.dill"));
        let peak_small = child_peak_rss(&small, &dirs.path().join("out-small.dill"));
        let growth = peak_big.saturating_sub(peak_small);
        assert!(
            growth < RSS_DELTA_BUDGET_BYTES,
            "peak RSS grew by {growth} bytes (small {peak_small} -> big {peak_big}) with dill size; the rewrite is holding the file, not streaming it"
        );
    }

    /// Rewrites `src` in a fresh child (a copy of this binary) and
    /// returns the largest RSS `ps` observed for it while it ran, in
    /// bytes. `ps -o rss=` reports KiB on both Linux and macOS.
    ///
    /// Polling rather than `wait4`'s `ru_maxrss` is deliberate: the
    /// rewrite's live set is what must stay flat, and only a live
    /// reading sees it — a lifetime peak is polluted by whatever the
    /// parent process happened to hold at fork/exec time.
    #[cfg(unix)]
    fn child_peak_rss(src: &Path, dst: &Path) -> u64 {
        let mut child = std::process::Command::new(std::env::current_exe().expect("current exe"))
            // A substring filter (not `--exact`, which wants the
            // module-qualified path) selecting exactly this one test.
            .arg("rewrite_peak_rss_is_independent_of_dill_size")
            .arg("--test-threads=1")
            .env("DQ_BCACHE_RSS_CHILD", "1")
            .env("DQ_BCACHE_RSS_SRC", src)
            .env("DQ_BCACHE_RSS_DST", dst)
            .spawn()
            .expect("spawn child");
        let pid = child.id().to_string();
        let mut peak_kib = 0u64;
        let mut samples = 0u64;
        // The rewrite is I/O-bound over tens of MB — orders of
        // magnitude longer than one ps spawn — so a poll every few ms
        // cannot miss the plateau the string-table pass holds for most
        // of the run. A `ps` failure is a skipped sample, never fatal:
        // the loop still terminates on child exit.
        let status = loop {
            match child.try_wait().expect("try_wait") {
                Some(status) => break status,
                None => {
                    if let Ok(output) = std::process::Command::new("ps")
                        .args(["-o", "rss=", "-p", &pid])
                        .output()
                    {
                        if output.status.success() {
                            if let Ok(text) = std::str::from_utf8(&output.stdout) {
                                if let Ok(kib) = text.trim().parse::<u64>() {
                                    peak_kib = peak_kib.max(kib);
                                    samples += 1;
                                }
                            }
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            }
        };
        assert!(
            status.success(),
            "child rewrite failed: {status} ({samples} rss samples, peak {peak_kib} KiB)"
        );
        assert!(
            samples > 0,
            "ps never reported the child's RSS — the rewrite finished before one poll; the fixture is too small to measure"
        );
        peak_kib * 1024
    }

    /// The RSS delta budget: how far the bigger dill's polled peak may
    /// exceed the smaller's. Streaming leaves only workload-scaled
    /// bookkeeping between the two runs — the string-table
    /// ends/new-ends/matched vectors cost ~13 bytes per entry, ~3.4 MB
    /// at 260k strings — while any whole-file posture adds at least
    /// the dills' ~64 MB size gap. 16 MB sits far above the
    /// bookkeeping delta and four times under the failure signal; the
    /// polled channel carries no fork/exec baseline, so the same flat
    /// bound holds on every platform.
    const RSS_DELTA_BUDGET_BYTES: u64 = 16 * 1024 * 1024;

    /// A valid dill with `count` string-table entries (~257 bytes
    /// each; 260_000 makes a ~68 MB file) whose string table dominates
    /// the file, so the rewrite's memory posture — not the fixture
    /// builder's — is what a resident-set measurement sees.
    fn build_large_dill(path: &Path, count: usize) {
        let strings: Vec<String> = (0..count)
            .map(|i| {
                format!(
                    "file:///tree/a/lib/src/gen/module{i:06}.dart{}",
                    " ".repeat(213)
                )
            })
            .collect();
        let entries: Vec<&str> = strings.iter().map(String::as_str).collect();
        let mut file = synthetic_component(&[("file:///tree/a/lib/main.dart", "")], &entries, 1);
        // A second, small component keeps the framing walk real.
        file.extend_from_slice(&synthetic_component(
            &[("file:///tree/a/t.dart", "")],
            &["file:///tree/a/t.dart"],
            0,
        ));
        fs::write(path, file).expect("write large dill");
    }

    /// A large append log: every rewritten component stays
    /// independently framed, aligned, and structurally verifiable.
    #[test]
    fn many_components_stay_independently_framed() {
        let dirs = tempfile::tempdir().expect("tempdir");
        let mut file = Vec::new();
        let mut expected = Vec::new();
        for i in 0..17u32 {
            let uri = format!("file:///tree/a/lib/src/module{i}.dart");
            file.extend_from_slice(&synthetic_component(
                &[(uri.as_str(), "")],
                &[uri.as_str(), "dart:core"],
                i % 3,
            ));
            expected.push(uri.replace("file:///tree/a/", "file:///tree/b/"));
        }
        let src = write_fixture(dirs.path(), "many.dill", &file);
        let dst = dirs.path().join("out.dill");
        let stats =
            rewrite_file(&src, &dst, b"file:///tree/a/", b"file:///tree/b/").expect("rewrite");
        assert_eq!(stats.components, 17);
        assert_eq!(stats.source_uris, 17);
        assert_eq!(stats.strings, 17);
        let ranges = file_component_ranges(&dst);
        let mut reader = Reader::open(&dst).expect("open output");
        for (range, uri) in ranges.iter().zip(&expected) {
            verify_component(
                &mut reader,
                range.start,
                range.len(),
                &[uri.as_str()],
                &[uri.as_str(), "dart:core"],
            );
        }
    }
}
