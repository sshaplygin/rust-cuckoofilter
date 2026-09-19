# valkey-cuckoo

[![Crates.io](https://img.shields.io/crates/v/valkey-cuckoo.svg)](https://crates.io/crates/valkey-cuckoo)

[Documentation](https://docs.rs/valkey-cuckoo)

An independently maintained fork of [axiomhq/rust-cuckoofilter](https://github.com/axiomhq/rust-cuckoofilter), published by Sam Shaplygin for work on deterministic cuckoo eviction in Valkey modules. This is not an official Valkey project release. The upstream MIT license and contributor attribution are retained.

## Installation

The library keeps the `cuckoofilter` import name. Replace the upstream dependency with:

```toml
[dependencies]
cuckoofilter = { package = "valkey-cuckoo", version = "0.2.0" }
```

## Deterministic eviction

Add `rand = "0.8"` and `rand_chacha = "0.3"` to your dependencies, then supply a seeded RNG:

```rust
use cuckoofilter::CuckooFilter;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

let mut filter = CuckooFilter::with_rng(100, ChaCha8Rng::seed_from_u64(42));
filter.add("hello world").unwrap();
assert!(filter.contains("hello world"));
```

Identical RNG states and operation sequences produce identical eviction choices when input hashes match. Replicas must use the same seed, operation order, hashing behavior, and RNG implementation. Cross-platform or cross-version replication also requires stable hashing of input values.

`new()` and `with_capacity()` continue to use `ThreadRng`; storing it makes the default filter neither `Send` nor `Sync`. Seeded RNGs such as `ChaCha8Rng` can provide those traits. `with_hasher_and_rng` selects the hasher type; it does not retain the supplied hasher's state.

Exports contain fingerprints and length only. To resume a snapshot, save the RNG state separately (available through `rng()`) and restore using `from_export_with_rng(exported, bucket_size, max_kicks, rng)`. For ChaCha8 with a fixed seed and stream, save `get_word_pos()` and restore it with `set_word_pos()`. The default `From` implementation still initializes `ThreadRng`.

`with_config_and_rng` accepts runtime bucket sizes (1–255) and a maximum eviction count. Fingerprints are stored in a contiguous byte allocation. Use `try_add` to roll back fingerprints and RNG state on failed insertion; this requires an RNG whose clone has independent state. The legacy `add` method retains its upstream behavior of dropping an existing fingerprint if eviction fails.

### Shared hashes and duplicate counts

Hash each item once with `CuckooFilter::<H, R>::hash_item`, then reuse the `ItemHash`
with `contains_hashed`, `count_hashed`, `try_add_hashed`, and `delete_hashed` across
filters of different capacities. All filters must use the same hasher type and
hashing behavior. The hasher's `Default` implementation determines its keys.

```rust
use cuckoofilter::CuckooFilter;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use std::collections::hash_map::DefaultHasher;

type Filter = CuckooFilter<DefaultHasher, ChaCha8Rng>;
let mut filter = Filter::with_config_and_rng(100, 4, 32, ChaCha8Rng::seed_from_u64(42)).unwrap();
let hash = Filter::hash_item("hello world");
filter.try_add_hashed(&hash).unwrap();
filter.try_add_hashed(&hash).unwrap();
assert_eq!(filter.count_hashed(&hash), 2);
assert!(filter.delete_hashed(&hash));
assert_eq!(filter.count_hashed(&hash), 1);
```

Insertions store another fingerprint even if one already matches. Counts are
approximate because different items can have matching fingerprints. Both
candidate buckets are counted, or just once when their indices coincide.
Only delete items known to have been inserted; deleting a false positive can
remove another item's fingerprint. `test_and_add` skips matches and therefore
inherits the false-positive risk of membership checks.

`try_add_no_evict_hashed` only tries free slots in the two candidate buckets,
takes O(bucket_size) time, and does not allocate or consume randomness. It returns
`false` without changing state when both buckets are full, even if other buckets
have room. This can bound the work spent retrying nearly full filters.

### Raw storage and defragmentation

`as_bytes()` borrows the fingerprint allocation without copying. There is one
byte per slot, with **100** marking an empty slot; zero is a valid fingerprint.
`from_bytes_with_rng(bytes, length, bucket_size, max_kicks, rng)` takes ownership
of a `Box<[u8]>` and validates its layout and occupied count without reallocating
it. Exported snapshots retain the same byte layout.

`realloc_buckets` passes ownership of the allocation to a callback and installs
the returned allocation. The callback must preserve its contents and length,
but may relocate it. The method itself does not allocate on success. A changed
length causes a panic; if the callback panics or returns a different length,
the filter is cleared while retaining capacity and RNG state.

The `deterministic_snapshot_fixture` test locks down expected bucket bytes and
the RNG word position using SipHash-1-3 with zero keys, canonical input encoding,
and ChaCha8 seeded with 42. Changes to these values require an explicit review
of snapshot and replication compatibility.

Version 0.2.0 is currently available on the `feat/valkey-snapshots` Git branch; the published crates.io release remains 0.1.0.

## About cuckoo filters

Cuckoo filter is a Bloom filter replacement for approximated set-membership queries. While Bloom filters are well-known space-efficient data structures to serve queries like "if item x is in a set?", they do not support deletion. Their variances to enable deletion (like counting Bloom filters) usually require much more space.

Cuckoo ﬁlters provide the ﬂexibility to add and remove items dynamically. A cuckoo filter is based on cuckoo hashing (and therefore named as cuckoo filter). It is essentially a cuckoo hash table storing each key's fingerprint. Cuckoo hash tables can be highly compact, thus a cuckoo filter could use less space than conventional Bloom ﬁlters, for applications that require low false positive rates (< 3%).

For details about the algorithm and citations please use this article for now

["Cuckoo Filter: Better Than Bloom" by Bin Fan, Dave Andersen and Michael Kaminsky](https://www.cs.cmu.edu/~dga/papers/cuckoo-conext2014.pdf)


## Example usage

```rust
extern crate cuckoofilter;

let value: &str = "hello world";

// Create cuckoo filter with default max capacity of 1000000 items
let mut cf = cuckoofilter::CuckooFilter::new();

// Add data to the filter
let success = cf.add(value).unwrap();
// success ==> ()

// Lookup if data is in the filter
let success = cf.contains(value);
// success ==> true

// Test and add to the filter (if data does not exists then add)
let success = cf.test_and_add(value).unwrap();
// success ==> false

// Remove data from the filter.
let success = cf.delete(value);
// success ==> true
```

## C Interface
The repository includes the upstream C interface in `cabi/`. It is not part of this crate's published package and is not published separately by this fork.


## Notes & TODOs
* The default bucket size is 4 fingerprints, configurable from 1 to 255. Each fingerprint occupies one byte.
* The legacy `add` method can drop an existing fingerprint on `NotEnoughSpace`. Use `try_add` or `try_add_hashed` to preserve existing entries on failure.
* There are no high-level bindings for other languages than C.
  One could add them e.g. for python using [milksnake](https://github.com/getsentry/milksnake).
