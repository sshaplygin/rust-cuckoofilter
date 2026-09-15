# valkey-cuckoo

[![Crates.io](https://img.shields.io/crates/v/valkey-cuckoo.svg)](https://crates.io/crates/valkey-cuckoo)

[Documentation](https://docs.rs/valkey-cuckoo)

An independently maintained fork of [axiomhq/rust-cuckoofilter](https://github.com/axiomhq/rust-cuckoofilter), published by Sam Shaplygin for work on deterministic cuckoo eviction in Valkey modules. This is not an official Valkey project release. The upstream MIT license and contributor attribution are retained.

## Installation

The library keeps the `cuckoofilter` import name. Replace the upstream dependency with:

```toml
[dependencies]
cuckoofilter = { package = "valkey-cuckoo", version = "0.1.0" }
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

Exports contain fingerprints and length only. They do not preserve RNG state, and imports initialize `ThreadRng`, so the current export/import API does not resume deterministic eviction after restoring a snapshot.

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
* This implementation uses a a static bucket size of 4 fingerprints and a fingerprint size of 1 byte based on my understanding of an optimal bucket/fingerprint/size ratio from the aforementioned paper.
* When the filter returns `NotEnoughSpace`, the element given is actually added to the filter, but some random *other*
  element gets removed. This could be improved by implementing a single-item eviction cache for that removed item.
* There are no high-level bindings for other languages than C.
  One could add them e.g. for python using [milksnake](https://github.com/getsentry/milksnake).
