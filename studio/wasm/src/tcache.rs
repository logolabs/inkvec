//! A bounded thread-caching allocator for the threaded WebAssembly build (`pkg-threads/`).
//!
//! # The problem
//!
//! On `wasm32-unknown-unknown` with shared memory, Rust's standard library allocates from one
//! `dlmalloc` heap behind one global lock, and that lock *spins*: "the main thread in a web
//! browser *cannot ever block*", so a thread that finds it taken loops until it is free
//! (`library/std/src/sys/alloc/wasm.rs`, whose own comment calls spinning "not a great
//! solution"). Studio Lite runs the engine on a rayon pool of up to 15 Web Workers plus the
//! engine worker, and the parallel stages that allocate a lot anti-scale on it. Measured by the
//! r2-qspeed research (39 images, headless Edge, threaded build against the one-core build):
//! `merge_bands` took 0.31x, `blend_absorb` 0.61x and `palette` 0.90x the *one-core* time, and
//! one emoji's gradient band merge took 15.3 s threaded against 1.07 s natively. Four threads
//! instead of sixteen made `merge_bands` 2.7x faster, which is contention, not work.
//!
//! # The method: a per-thread cache of free blocks in front of the global heap
//!
//! Every small block a thread frees goes onto that thread's own free list for its size class,
//! and the thread's next allocation of that class takes it back without touching the lock. Only
//! a miss (the list is empty), a block too large to cache, or a cache over its byte budget goes
//! to the system allocator, and so to the lock. The layout is this module's, as follows.
//!
//! * **Size classes.** A request of `s` bytes (alignment at most [`NATURAL_ALIGN`], size at
//!   most [`SMALL_MAX`]) is rounded up to its class size: multiples of 16 up to 64 bytes, then
//!   four classes per power of two (80, 96, 112, 128, 160, 192, ...), so a block is at most
//!   25% larger than asked. Each cached block is a whole class-size block of the system heap.
//! * **Free lists.** Intrusive and singly linked: a free block's first word holds the next free
//!   block of its class. Blocks are at least 16 bytes and 8-byte aligned, so the word fits.
//! * **Per-thread state.** One [`Lists`] per thread, in a `const`-initialised thread-local with
//!   no destructor: reaching it is a load from the thread's TLS block, it never allocates, and
//!   it is never torn down while the thread runs. (A destructor would register itself on first
//!   use, which allocates, inside the allocator.)
//! * **The bound.** A thread never holds more than [`THREAD_CAP`] bytes of free blocks. When a
//!   free would cross it, the cache first gives back blocks it has not needed (the low-water
//!   rule below); if there is still no room, that block goes straight back to the system heap.
//!   With the pool at its largest (16 threads) the caches together hold at most
//!   16 x [`THREAD_CAP`] bytes.
//!
//! The order of a call: `alloc` -> class of the layout -> pop that class's list, or ask the
//! system heap for a class-size block. `dealloc` -> class -> push onto the list if the budget
//! allows, after a scavenge if it must, or free to the system heap. Anything larger than
//! [`SMALL_MAX`], or more aligned than [`NATURAL_ALIGN`], passes through untouched.
//!
//! # Why the output is byte-identical
//!
//! An allocator decides only *where* a block lives, never what is written in it. The engine
//! never orders, hashes or prints an address (its only pointer comparisons are `Arc::ptr_eq`
//! between live objects, which distinct live objects can never satisfy under any correct
//! allocator), its hash maps hash keys rather than addresses, and every `f64` operation is
//! the same instruction on the same operands whichever block holds them. A cached block that
//! is handed out again is not zeroed by `alloc` (no allocator promises that), and is zeroed by
//! `alloc_zeroed`, exactly as the system heap behaves. Verified on 39 images by the research
//! prototype and on this module by `w2-lite` (identity tables in its report).
//!
//! # Literature
//!
//! * Inspired by: S. Ghemawat and P. Menage, "TCMalloc: Thread-Caching Malloc" (2007),
//!   <https://goog-perftools.sourceforge.net/doc/tcmalloc.html>. Taken from it: per-thread
//!   free lists by size class with no lock on a hit; size classes spaced so a request is not
//!   rounded up by much; small objects only ("size <= 32K"), larger ones from the central
//!   heap; a per-thread cache budget ("garbage collected when the combined size of all objects
//!   in the cache exceeds 2MB") enforced by moving L/2 blocks of each list back, where L is the
//!   list's low-water mark since the last collection. Adapted: there is no central free list
//!   or page heap of our own (the system `dlmalloc` is the central heap, behind its lock), the
//!   classes are four per power of two rather than TCMalloc's table, and the budget is checked
//!   on every free rather than by a separate collector.
//! * Inspired by: E. D. Berger, K. S. McKinley, R. D. Blumofe and P. R. Wilson, "Hoard: a
//!   scalable memory allocator for multithreaded applications", ASPLOS 2000,
//!   doi:10.1145/378993.379232: contention at one central heap is the scalability failure,
//!   and per-thread heaps must keep the memory blow-up bounded. Our bound is [`THREAD_CAP`].
//! * See also: D. Leijen, B. Zorn and L. de Moura, "Mimalloc: Free List Sharding in Action",
//!   APLAS 2019 (also MSR-TR-2019-18): thread-local sharded free lists. Not used as a crate because none
//!   of mimalloc, Hoard or TCMalloc builds for `wasm32-unknown-unknown` with shared memory
//!   (they need an OS: `mmap`, real threads, blocking locks), and the pure-Rust wasm
//!   allocators (`talc`, `lol_alloc`, `dlmalloc` itself) are single-heap behind a lock, which
//!   is the problem being solved.
//! * The bug being worked around: Rust std, `library/std/src/sys/alloc/wasm.rs` (one
//!   `dlmalloc` behind one spinning `LOCKED` atomic).
//!
//! # Where it sits
//!
//! Only in the threaded build (`--features threads`, nightly, `+atomics`): `lib.rs` installs it
//! as the `#[global_allocator]` there. The one-core build (`pkg/`) has no lock to contend for
//! and keeps std's allocator.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::UnsafeCell;
use std::ptr;

/// The alignment `dlmalloc` gives every block without being asked: `2 * size_of::<usize>()`,
/// 8 bytes on wasm32. A class block is allocated at exactly this alignment, so the system heap
/// takes its plain `malloc` path; asking for more would send every block through `memalign`,
/// which over-allocates and splits the remainder off (the research prototype asked for 16).
/// Requests aligned more strictly than this (`u128`, `v128`) are not cached.
pub const NATURAL_ALIGN: usize = 2 * core::mem::size_of::<usize>();

/// The largest request served from the caches, in bytes. Larger blocks are rarer, their cost
/// is dominated by filling them rather than by the lock, and rounding them up to a class would
/// waste up to a quarter of a large buffer. TCMalloc draws the same line at 32 KiB. Measured
/// (39 images, threaded build, interleaved A/B in headless Edge, w2-lite): caching up to 1 MiB
/// with a 4 MiB budget was no faster than 32 KiB (stage sum 1.00x) and raised the memory
/// high-water from 179 to 194 MB; caching up to 4 MiB with no effective budget (the research
/// prototype's reach) was slower on every gradient image (e.g. 6.3 -> 18.8 s) and grew memory
/// to 863 MB, the pool spending its time growing memory instead of waiting on the lock.
pub const SMALL_MAX: usize = 1 << 15;

/// The most free-block bytes one thread may keep, in bytes (TCMalloc's 2007 figure). With it
/// and [`SMALL_MAX`] the memory cost is small: high-water 158 -> 161 MB on the 39 images,
/// where the unbounded research prototype went from 122 to 382 MB.
pub const THREAD_CAP: usize = 2 << 20;

/// A thread-local's value once its TLS block holds this thread's copy of the initial image.
///
/// A new rayon worker calls `malloc` twice before its TLS block exists (wasm-bindgen's thread
/// start allocates the worker's stack and then its TLS block, and only then runs
/// `__wasm_init_tls`). During those two calls the thread-local below reads whatever lies at the
/// TLS base the instance started with, not this thread's copy. The cache is used only once its
/// `ready` word reads this value, which `__wasm_init_tls` copies in from the module's `.tdata`
/// image; until then every call goes straight to the system heap and the cache is never written.
/// In this build `__tls_base` starts at 0 (read from the module's global section) and the main
/// stack fills the first 1 MiB (a stack-first layout), so that early read lands on the bottom of
/// a stack nothing has grown into, which reads zero.
const READY: u32 = 0x9E37_79B9;

/// Size class of a request of `s` bytes, `1 <= s <= SMALL_MAX`.
///
/// Classes 0..=3 are 16, 32, 48 and 64 bytes. Above 64, with `e = floor(log2(s - 1))` (so
/// `2^e < s <= 2^(e+1)`, `e >= 6`), the class size is the next multiple of `2^(e-2)`, one of
/// `5, 6, 7, 8` times it, and the index is `4 + 4 (e - 6) + (m - 4)` where
/// `m = (s - 1) >> (e - 2)` is 4..=7. Shifts and masks only: no `%`, which the wazero arm64
/// backend miscompiles (see the ledger), and no table to keep in step. O(1).
const fn class_of(s: usize) -> usize {
    if s <= 64 {
        // 1..=16 -> 0, 17..=32 -> 1, 33..=48 -> 2, 49..=64 -> 3.
        (s - 1) >> 4
    } else {
        let e = (usize::BITS - 1 - (s - 1).leading_zeros()) as usize;
        let m = (s - 1) >> (e - 2);
        4 + ((e - 6) << 2) + (m - 4)
    }
}

/// The block size of class `c`, the largest request it serves: the inverse of [`class_of`]
/// (`class_of(class_size(c)) == c` and `class_of(class_size(c) + 1) == c + 1`).
const fn class_size(c: usize) -> usize {
    if c < 4 {
        (c + 1) << 4
    } else {
        let k = c - 4;
        let e = 6 + (k >> 2);
        (5 + (k & 3)) << (e - 2)
    }
}

/// How many classes there are: those of 1..=SMALL_MAX bytes (40 for 32 KiB).
const CLASSES: usize = class_of(SMALL_MAX) + 1;

/// The class a layout is cached in, or `None` for a layout this cache leaves to the system
/// heap: empty, larger than [`SMALL_MAX`], or aligned beyond [`NATURAL_ALIGN`].
#[inline]
fn small_class(layout: &Layout) -> Option<usize> {
    let s = layout.size();
    (s != 0 && s <= SMALL_MAX && layout.align() <= NATURAL_ALIGN).then(|| class_of(s))
}

/// The layout every block of class `c` has in the system heap, whatever size was asked for.
#[inline]
fn class_layout(c: usize) -> Layout {
    // SAFETY: a class size is a positive multiple of 16, far below isize::MAX, and
    // NATURAL_ALIGN is a power of two.
    unsafe { Layout::from_size_align_unchecked(class_size(c), NATURAL_ALIGN) }
}

/// One thread's free lists.
///
/// Invariants, between calls: `len[c]` is the length of the list at `head[c]`; every block on
/// it is a live system-heap block of `class_layout(c)` that nothing else references; `bytes`
/// is `sum(len[c] * class_size(c)) <= THREAD_CAP`; `low[c] <= len[c]`.
struct Lists {
    /// [`READY`] once this thread's TLS block is initialised; anything else before that.
    ready: u32,
    /// Free bytes held, over all classes.
    bytes: usize,
    /// The first free block of each class, or null. A free block's first word is the next one.
    head: [*mut u8; CLASSES],
    /// How many blocks each list holds.
    len: [u32; CLASSES],
    /// Each list's low-water mark: the shortest it has been since the last scavenge. That many
    /// blocks sat unused for the whole interval, which is what TCMalloc's collection gives back.
    low: [u32; CLASSES],
}

thread_local! {
    /// This thread's cache. `const` and destructor-free: no lazy initialisation, no
    /// registration, so reaching it can never allocate (see the module comment).
    static CACHE: UnsafeCell<Lists> = const {
        UnsafeCell::new(Lists {
            ready: READY,
            bytes: 0,
            head: [ptr::null_mut(); CLASSES],
            len: [0; CLASSES],
            low: [0; CLASSES],
        })
    };
}

/// Run `f` on this thread's lists, or return `fallback` when there are none to use: the TLS
/// block is not set up yet (a starting worker) or the thread-local cannot be reached.
#[inline]
fn with_lists<R>(fallback: R, f: impl FnOnce(&mut Lists) -> R) -> R {
    CACHE
        .try_with(|cell| {
            // SAFETY: the thread-local is this thread's alone, and no call made while the
            // reference lives re-enters the allocator on this thread (the system heap's calls
            // do not allocate through the global allocator).
            let lists = unsafe { &mut *cell.get() };
            if lists.ready == READY {
                Some(f(lists))
            } else {
                None
            }
        })
        .ok()
        .flatten()
        .unwrap_or(fallback)
}

/// Pop a free block of class `c` from this thread's list, if it has one. O(1).
#[inline]
fn take(c: usize) -> *mut u8 {
    with_lists(ptr::null_mut(), |l| {
        let p = l.head[c];
        if !p.is_null() {
            // SAFETY: a listed block is a free class block whose first word is the next link.
            l.head[c] = unsafe { p.cast::<*mut u8>().read() };
            l.len[c] -= 1;
            l.low[c] = l.low[c].min(l.len[c]);
            l.bytes -= class_size(c);
        }
        p
    })
}

/// Keep the freed class-`c` block `p` on this thread's list; false if it must go back to the
/// system heap instead (no room under [`THREAD_CAP`] even after a scavenge, or no cache yet).
/// O(1), except when the budget is reached: then one scavenge, O(CLASSES + blocks freed).
#[inline]
fn give(c: usize, p: *mut u8) -> bool {
    with_lists(false, |l| {
        let size = class_size(c);
        if l.bytes + size > THREAD_CAP {
            scavenge(l);
            if l.bytes + size > THREAD_CAP {
                return false;
            }
        }
        // SAFETY: `p` is a class-`c` block the caller has just freed; its first word is ours.
        unsafe { p.cast::<*mut u8>().write(l.head[c]) };
        l.head[c] = p;
        l.len[c] += 1;
        l.bytes += size;
        true
    })
}

/// TCMalloc's thread-cache collection: from each list, give back `ceil(L/2)` blocks to the
/// system heap, where `L` is the list's low-water mark (blocks that sat unused since the last
/// scavenge), then restart every mark at the list's length. A list in steady use has `L = 0`
/// and keeps everything; a class that stopped being asked for loses half its blocks per
/// scavenge. TCMalloc moves `L/2`; rounding up lets a single idle block go too.
fn scavenge(l: &mut Lists) {
    for c in 0..CLASSES {
        let mut give_back = l.low[c].div_ceil(2);
        while give_back > 0 {
            let p = l.head[c];
            // SAFETY: as in `take`; the block then goes back to the heap it came from, with
            // the layout it was allocated with.
            unsafe {
                l.head[c] = p.cast::<*mut u8>().read();
                System.dealloc(p, class_layout(c));
            }
            l.len[c] -= 1;
            l.bytes -= class_size(c);
            give_back -= 1;
        }
        l.low[c] = l.len[c];
    }
}

/// The threaded build's global allocator: [`System`] (std's `dlmalloc` behind its lock), with
/// a bounded per-thread cache of small blocks in front of it. See the module comment.
pub struct ThreadCaching;

// SAFETY: every block handed out is a live block of the system heap at least as large and as
// aligned as the layout asked for (a class block is `class_size(c) >= size` bytes at
// NATURAL_ALIGN >= align); a block is on at most one list, and only while freed; the layout a
// block is returned to the system heap with is the one it was allocated with (`class_layout`
// for cached classes, the caller's own otherwise), because `small_class` maps a layout to the
// same class on allocation, reallocation and free.
unsafe impl GlobalAlloc for ThreadCaching {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        match small_class(&layout) {
            Some(c) => {
                let p = take(c);
                if p.is_null() {
                    System.alloc(class_layout(c))
                } else {
                    p
                }
            }
            None => System.alloc(layout),
        }
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        match small_class(&layout) {
            Some(c) => {
                let p = take(c);
                if p.is_null() {
                    // The system heap knows when fresh memory is already zero (calloc).
                    System.alloc_zeroed(class_layout(c))
                } else {
                    // Only the bytes asked for: the rest of the class block is never read.
                    p.write_bytes(0, layout.size());
                    p
                }
            }
            None => System.alloc_zeroed(layout),
        }
    }

    #[inline]
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        match small_class(&layout) {
            Some(c) => {
                if !give(c, p) {
                    System.dealloc(p, class_layout(c));
                }
            }
            None => System.dealloc(p, layout),
        }
    }

    /// Growing or shrinking: in place when old and new sizes share a class (the block already
    /// has room); through the system heap's own `realloc` when both are uncached (it may grow
    /// in place); otherwise allocate, copy `min(old, new)` bytes, free.
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller guarantees new_size, rounded up to layout.align(), fits isize.
        let new = Layout::from_size_align_unchecked(new_size, layout.align());
        match (small_class(&layout), small_class(&new)) {
            (Some(a), Some(b)) if a == b => p,
            (None, None) => System.realloc(p, layout, new_size),
            _ => {
                let q = self.alloc(new);
                if !q.is_null() {
                    ptr::copy_nonoverlapping(p, q, layout.size().min(new_size));
                    self.dealloc(p, layout);
                }
                q
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_size_maps_to_the_smallest_class_that_holds_it() {
        let mut last = 0;
        for s in 1..=SMALL_MAX {
            let c = class_of(s);
            assert!(c < CLASSES, "{s} -> {c}");
            assert!(class_size(c) >= s, "{s} does not fit class {c}");
            assert!(
                c == 0 || class_size(c - 1) < s,
                "{s} fits a smaller class than {c}"
            );
            assert!(c == last || c == last + 1, "classes are consecutive");
            last = c;
        }
        assert_eq!(class_size(CLASSES - 1), SMALL_MAX);
    }

    #[test]
    fn a_class_wastes_at_most_a_quarter_and_holds_a_link() {
        for c in 0..CLASSES {
            let size = class_size(c);
            assert!(size >= 16 && size.is_multiple_of(NATURAL_ALIGN));
            assert_eq!(class_of(size), c);
            let smallest = if c == 0 { 1 } else { class_size(c - 1) + 1 };
            // Above 64 bytes, the smallest request of a class is rounded up by under a quarter.
            if size > 64 {
                assert!(size * 4 < smallest * 5, "class {c}: {smallest}..{size}");
            }
        }
    }

    #[test]
    fn the_first_classes_are_the_documented_ones() {
        let sizes: Vec<usize> = (0..12).map(class_size).collect();
        assert_eq!(
            sizes,
            [16, 32, 48, 64, 80, 96, 112, 128, 160, 192, 224, 256]
        );
        assert_eq!(CLASSES, 40);
    }

    #[test]
    fn layouts_beyond_the_cache_pass_through() {
        assert_eq!(small_class(&Layout::from_size_align(0, 1).unwrap()), None);
        assert_eq!(
            small_class(&Layout::from_size_align(SMALL_MAX + 1, 8).unwrap()),
            None
        );
        assert_eq!(small_class(&Layout::from_size_align(64, 32).unwrap()), None);
        assert_eq!(
            small_class(&Layout::from_size_align(64, NATURAL_ALIGN).unwrap()),
            Some(3)
        );
    }

    /// Allocate, grow, shrink and free through the cache on several threads, checking the
    /// contents survive and the budget holds. The test binary's system allocator is the
    /// host's, so this checks the bookkeeping, not dlmalloc.
    #[test]
    fn blocks_round_trip_and_the_budget_holds() {
        let threads: Vec<_> = (0..4)
            .map(|t| {
                std::thread::spawn(move || unsafe {
                    let a = ThreadCaching;
                    let mut held: Vec<(*mut u8, Layout)> = Vec::new();
                    let mut seed = 0x1234_5678_u32 ^ t;
                    for round in 0..20_000u32 {
                        seed ^= seed << 13;
                        seed ^= seed >> 17;
                        seed ^= seed << 5;
                        let size = 1 + (seed as usize & 0xFFFF);
                        let layout = Layout::from_size_align(size, 8).unwrap();
                        let p = if round & 3 == 0 {
                            a.alloc_zeroed(layout)
                        } else {
                            a.alloc(layout)
                        };
                        assert!(!p.is_null());
                        assert_eq!(p as usize & 7, 0);
                        if round & 3 == 0 {
                            assert!((0..size).all(|i| *p.add(i) == 0));
                        }
                        p.write_bytes(round as u8, size);
                        held.push((p, layout));
                        if held.len() > 64 || seed & 1 == 0 {
                            let k = seed as usize % held.len();
                            let (q, l) = held.swap_remove(k);
                            let new_size = 1 + ((seed >> 8) as usize & 0x3FFF);
                            let r = a.realloc(q, l, new_size);
                            assert!(!r.is_null());
                            let fill = *r;
                            assert!((0..l.size().min(new_size)).all(|i| *r.add(i) == fill));
                            a.dealloc(r, Layout::from_size_align(new_size, 8).unwrap());
                        }
                        let bytes = with_lists(0, |l| l.bytes);
                        assert!(bytes <= THREAD_CAP);
                    }
                    for (p, l) in held {
                        a.dealloc(p, l);
                    }
                    with_lists(0, |l| {
                        let counted: usize = (0..CLASSES)
                            .map(|c| l.len[c] as usize * class_size(c))
                            .sum();
                        assert_eq!(counted, l.bytes);
                        l.bytes
                    })
                })
            })
            .collect();
        for t in threads {
            assert!(t.join().unwrap() <= THREAD_CAP);
        }
    }

    #[test]
    fn a_scavenge_frees_half_of_what_sat_unused() {
        let a = ThreadCaching;
        std::thread::spawn(move || unsafe {
            let layout = Layout::from_size_align(100, 8).unwrap();
            let blocks: Vec<*mut u8> = (0..8).map(|_| a.alloc(layout)).collect();
            for &p in &blocks {
                a.dealloc(p, layout);
            }
            let c = class_of(100);
            with_lists((), |l| {
                assert_eq!(l.len[c], 8);
                // Nothing was taken since: the mark restarts at the length...
                scavenge(l);
                assert_eq!(l.len[c], 8, "the first scavenge's mark was 0");
                // ...and the next scavenge gives back half of the 8 unused blocks.
                scavenge(l);
                assert_eq!(l.len[c], 4);
                assert_eq!(l.bytes, 4 * class_size(c));
            });
        })
        .join()
        .unwrap();
    }
}
