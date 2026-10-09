//! Strings interned to small stable integers (ast-layout-spec §5).

use std::sync::Mutex;

use rustc_hash::FxHashMap;

use crate::content_id::ContentId;

/// A small stable integer naming one interned string.
///
/// Issued by a [`StringTable`]; the integer is only meaningful together with
/// the table that created it. Like [`FileId`](crate::FileId), ids are
/// immortal: they are handed out from 0 in first-intern order and are never
/// reused or retracted, so a holder may keep one for the process lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StringId(u32);

impl StringId {
    /// The underlying integer, stable for the lifetime of the issuing
    /// [`StringTable`].
    #[must_use]
    pub const fn index(self) -> u32 {
        self.0
    }
}

/// Interning table for strings that must outlive a single file: element
/// names surfaced in results, diagnostic message fragments
/// (ast-layout-spec §5).
///
/// `intern` dedups by content: the same text always yields the same
/// [`StringId`], and ids are handed out from 0 in first-intern order. The
/// table is append-only with immortal ids — there is no remove or eviction,
/// because ids already recorded in results or snapshots must keep resolving;
/// bounded-memory duty stays with the higher layers, and the §5 reclamation
/// story (PRD §17 mark-sweep) is deferred.
///
/// Interning is [`Mutex`]-guarded so one table can be shared by worker
/// threads; sharding is deferred until a measured contention signal.
///
/// Storage: text is copied once into the table and leaked as `&'static str`
/// (the rust-analyzer `symbol.rs` shape minus the tagged pointer, with u32
/// ids). The leak is the price of [`resolve`](Self::resolve) returning slices
/// without unsafe under interior mutability; the measured per-entry footprint
/// is recorded in ast-layout-spec §5. `ContentId` origins recorded through
/// [`intern_with_origin`](Self::intern_with_origin) are kept for that future
/// sweep; plain [`intern`](Self::intern) records none.
#[derive(Debug, Default)]
pub struct StringTable {
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    ids_by_str: FxHashMap<&'static str, StringId>,
    strings: Vec<&'static str>,
    origins: Vec<Option<ContentId>>,
}

impl StringTable {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the [`StringId`] for `s`, allocating the next id on first
    /// sight. Repeated interning of the same text is pure hashing.
    ///
    /// # Panics
    ///
    /// Panics when interning more than [`u32::MAX`] distinct strings, which
    /// would exhaust the [`StringId`] space, or when the table mutex is
    /// poisoned by a panic in an earlier call (both are program bugs).
    #[must_use]
    pub fn intern(&self, s: &str) -> StringId {
        let mut inner = self.inner.lock().expect("StringTable mutex poisoned");
        intern_locked(&mut inner, s, None)
    }

    /// Like [`intern`](Self::intern), but records `origin` as the [`ContentId`]
    /// the string was first seen with. Origins are the input of the deferred
    /// mark-sweep reclamation (ast-layout-spec §5); they have no observable
    /// effect today. Interning the same text with and without an origin
    /// yields one id — the first call's origin wins.
    ///
    /// # Panics
    ///
    /// Same conditions as [`intern`](Self::intern).
    #[must_use]
    pub fn intern_with_origin(&self, s: &str, origin: ContentId) -> StringId {
        let mut inner = self.inner.lock().expect("StringTable mutex poisoned");
        intern_locked(&mut inner, s, Some(origin))
    }

    /// The interned text behind `id`.
    ///
    /// # Panics
    ///
    /// Panics when `id` was not issued by this table. Ids are immortal and
    /// only minted here, so an unknown id is a program bug and is exposed as
    /// one rather than reported as a lookup miss.
    #[must_use]
    pub fn resolve(&self, id: StringId) -> &str {
        let inner = self.inner.lock().expect("StringTable mutex poisoned");
        inner.strings[id.index() as usize]
    }
}

/// Lock-held intern core shared by [`StringTable::intern`] and
/// [`StringTable::intern_with_origin`].
fn intern_locked(inner: &mut Inner, s: &str, origin: Option<ContentId>) -> StringId {
    if let Some(&id) = inner.ids_by_str.get(s) {
        return id;
    }
    let id =
        StringId(u32::try_from(inner.strings.len()).expect("more than u32::MAX strings interned"));
    let text: &'static str = Box::leak(Box::from(s));
    inner.ids_by_str.insert(text, id);
    inner.strings.push(text);
    inner.origins.push(origin);
    id
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_hash;
    use std::sync::Arc;

    #[test]
    fn duplicate_strings_share_one_id() {
        let table = StringTable::new();
        let hello = table.intern("hello");
        assert_eq!(table.intern("hello"), hello);
        assert_eq!(table.intern_with_origin("hello", content_hash(b"x")), hello);
        assert_eq!(table.resolve(hello), "hello");
    }

    #[test]
    fn distinct_strings_get_distinct_ids() {
        let table = StringTable::new();
        let a = table.intern("a");
        let b = table.intern("b");
        assert_ne!(a, b);
        assert_eq!(table.resolve(a), "a");
        assert_eq!(table.resolve(b), "b");
    }

    #[test]
    fn resolve_round_trips_byte_exact() {
        let cases = [
            "hello world",
            "héllo",
            "世界 🦁",
            "\0\0",
            "line\nbreak\ttab",
            "emoji 🎯 tail",
        ];
        let table = StringTable::new();
        let ids: Vec<StringId> = cases.iter().map(|s| table.intern(s)).collect();
        for (s, id) in cases.iter().zip(&ids) {
            assert_eq!(table.resolve(*id), *s);
            assert_eq!(table.resolve(*id).as_bytes(), s.as_bytes());
        }
    }

    #[test]
    fn empty_string_is_a_valid_distinct_entry() {
        let table = StringTable::new();
        let empty = table.intern("");
        assert_eq!(empty.index(), 0);
        assert_eq!(table.resolve(empty), "");
        assert_eq!(table.intern(""), empty, "empty text dedups like any other");
        assert_ne!(table.intern("a"), empty);
    }

    #[test]
    fn ids_follow_first_intern_order() {
        let table = StringTable::new();
        let b = table.intern("b");
        let a = table.intern("a");
        let c = table.intern("c");
        assert_eq!((b.index(), a.index(), c.index()), (0, 1, 2));
        assert_eq!(table.intern("a"), a, "dedup keeps the first id");
        assert_eq!(table.intern("b"), b);
        assert_eq!(table.intern("c"), c);
    }

    /// Property sweep over a fixed generated set: interning in order yields
    /// dense ascending ids, re-interning is a pure hit, and `resolve`
    /// round-trips every entry.
    #[test]
    fn fixed_set_round_trips_and_dedups() {
        const N: u32 = 1000;
        let table = StringTable::new();
        let ids: Vec<StringId> = (0..N).map(|i| table.intern(&i.to_string())).collect();
        for i in 0..N {
            assert_eq!(ids[i as usize].index(), i, "ids are dense and ordered");
            let id = table.intern(&i.to_string());
            assert_eq!(id, ids[i as usize]);
            assert_eq!(table.resolve(id), i.to_string());
        }
    }

    #[test]
    fn intern_with_origin_records_content_origin() {
        let table = StringTable::new();
        let origin = content_hash(b"lib.dart");
        let id = table.intern_with_origin("MyClass", origin);
        assert_eq!(
            table.intern("MyClass"),
            id,
            "origin variants dedup together"
        );

        let inner = table.inner.lock().expect("unpoisoned in test");
        assert_eq!(inner.origins[id.index() as usize], Some(origin));
        let plain = table.intern("plain");
        assert_eq!(inner.origins[plain.index() as usize], None);
    }

    /// The Mutex-guarded sharing contract: threads racing to intern the same
    /// texts all observe one id per text, and the table holds exactly one
    /// slot per distinct text (no double-insert between lookup and insert).
    #[test]
    fn shared_table_dedups_across_threads() {
        let table = Arc::new(StringTable::new());
        let evens: Vec<StringId> = (0..250_u32)
            .map(|i| table.intern(&(i * 2).to_string()))
            .collect();

        let results: Vec<Vec<StringId>> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..4)
                .map(|_| {
                    let table = Arc::clone(&table);
                    s.spawn(move || {
                        (0..500_u32)
                            .map(|i| table.intern(&i.to_string()))
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("test threads join"))
                .collect()
        });

        for r in &results {
            assert_eq!(r, &results[0], "every thread sees identical ids");
        }
        for (i, id) in results[0].iter().enumerate() {
            if i % 2 == 0 {
                assert_eq!(*id, evens[i / 2], "pre-interned texts keep their ids");
            }
        }
        let inner = table.inner.lock().expect("unpoisoned in test");
        assert_eq!(inner.strings.len(), 500, "one slot per distinct text");
    }
}
