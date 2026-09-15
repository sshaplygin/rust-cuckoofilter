//! Cuckoo filter probabilistic data structure for membership testing and cardinality counting.
//!
//! # Usage
//!
//! This fork is [on crates.io](https://crates.io/crates/valkey-cuckoo) and can be
//! used under the `cuckoofilter` dependency name in your project's `Cargo.toml`.
//!
//! ```toml
//! [dependencies]
//! cuckoofilter = { package = "valkey-cuckoo", version = "0.2.0" }
//! ```
//!
//! And this in your crate root:
//!
//! ```rust
//! extern crate cuckoofilter;
//! ```

mod bucket;
mod util;

use crate::bucket::{Fingerprint, BUCKET_SIZE};
use crate::util::{get_alt_index, get_fai, FaI};

use std::collections::hash_map::DefaultHasher;
use std::error::Error as StdError;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::mem;

use rand::Rng;
#[cfg(feature = "serde_support")]
use serde_derive::{Deserialize, Serialize};

/// If insertion fails, we will retry this many times.
pub const MAX_REBUCKET: u32 = 500;

/// The default number of buckets.
pub const DEFAULT_CAPACITY: usize = (1 << 20) - 1;

#[derive(Debug)]
pub enum CuckooError {
    NotEnoughSpace,
    InvalidConfiguration,
    InvalidExport,
}

impl fmt::Display for CuckooError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl StdError for CuckooError {
    fn description(&self) -> &str {
        "Not enough space to store this item, rebucketing failed."
    }
}

/// A cuckoo filter class exposes a Bloomier filter interface,
/// providing methods of add, delete, contains.
///
/// The default RNG is `ThreadRng`, which makes the filter neither `Send` nor
/// `Sync`. Use a custom RNG with the required traits when sharing filters
/// between threads.
///
/// # Examples
///
/// ```
/// extern crate cuckoofilter;
///
/// let words = vec!["foo", "bar", "xylophone", "milagro"];
/// let mut cf = cuckoofilter::CuckooFilter::new();
///
/// let mut insertions = 0;
/// for s in &words {
///     if cf.test_and_add(s).unwrap() {
///         insertions += 1;
///     }
/// }
///
/// assert_eq!(insertions, words.len());
/// assert_eq!(cf.len(), words.len());
///
/// // Re-add the first element.
/// cf.add(words[0]);
///
/// assert_eq!(cf.len(), words.len() + 1);
///
/// for s in &words {
///     cf.delete(s);
/// }
///
/// assert_eq!(cf.len(), 1);
/// assert!(!cf.is_empty());
///
/// cf.delete(words[0]);
///
/// assert_eq!(cf.len(), 0);
/// assert!(cf.is_empty());
///
/// for s in &words {
///     if cf.test_and_add(s).unwrap() {
///         insertions += 1;
///     }
/// }
///
/// cf.clear();
///
/// assert!(cf.is_empty());
///
/// ```
#[derive(Debug, Clone)]
pub struct CuckooFilter<H, R = rand::rngs::ThreadRng> {
    buckets: Box<[Fingerprint]>,
    bucket_size: usize,
    max_kicks: u32,
    len: usize,
    rng: R,
    _hasher: std::marker::PhantomData<H>,
}

impl Default for CuckooFilter<DefaultHasher> {
    fn default() -> Self {
        Self::new()
    }
}

impl CuckooFilter<DefaultHasher> {
    /// Construct a CuckooFilter with default capacity and hasher.
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }
}

impl<R: rand::RngCore> CuckooFilter<DefaultHasher, R> {
    /// Constructs a filter with the given capacity and RNG, using `DefaultHasher`.
    ///
    /// Identical RNG states and insertion sequences produce identical evictions
    /// when hashing produces identical results. Cross-platform or cross-version
    /// replication also requires stable hashing of the input values.
    ///
    /// ```
    /// use cuckoofilter::CuckooFilter;
    /// use rand::SeedableRng;
    /// use rand_chacha::ChaCha8Rng;
    ///
    /// let filter = CuckooFilter::with_rng(100, ChaCha8Rng::seed_from_u64(42));
    /// ```
    pub fn with_rng(capacity: usize, rng: R) -> Self {
        Self::with_hasher_and_rng(capacity, DefaultHasher::default(), rng)
    }
}

impl<H: Hasher + Default, R: rand::RngCore> CuckooFilter<H, R> {
    /// Constructs a filter with the given capacity, hasher type, and RNG.
    ///
    /// The supplied hasher selects the type `H`; its state is not retained.
    /// The filter creates `H::default()` for each hash operation.
    /// The RNG is stored without consuming any random values.
    ///
    /// ```
    /// use cuckoofilter::CuckooFilter;
    /// use fnv::FnvHasher;
    /// use rand::SeedableRng;
    /// use rand_chacha::ChaCha8Rng;
    ///
    /// let filter = CuckooFilter::with_hasher_and_rng(
    ///     100,
    ///     FnvHasher::default(),
    ///     ChaCha8Rng::seed_from_u64(42),
    /// );
    /// ```
    pub fn with_hasher_and_rng(capacity: usize, _hasher: H, rng: R) -> Self {
        Self::with_config_and_rng(capacity, BUCKET_SIZE, MAX_REBUCKET, rng)
            .expect("invalid filter capacity")
    }

    /// Construct a filter with a runtime bucket size and eviction limit.
    pub fn with_config_and_rng(
        capacity: usize,
        bucket_size: usize,
        max_kicks: u32,
        rng: R,
    ) -> Result<Self, CuckooError> {
        let slots = Self::allocation_size(capacity, bucket_size)?;
        if max_kicks == 0 {
            return Err(CuckooError::InvalidConfiguration);
        }
        Ok(Self {
            buckets: vec![Fingerprint::empty(); slots].into_boxed_slice(),
            bucket_size,
            max_kicks,
            len: 0,
            rng,
            _hasher: PhantomData,
        })
    }

    /// Number of fingerprint bytes allocated for these parameters.
    pub fn allocation_size(capacity: usize, bucket_size: usize) -> Result<usize, CuckooError> {
        if !(1..=255).contains(&bucket_size) {
            return Err(CuckooError::InvalidConfiguration);
        }
        let count = capacity
            .max(1)
            .checked_add(bucket_size - 1)
            .and_then(|n| (n / bucket_size).checked_next_power_of_two())
            .and_then(|n| n.checked_mul(bucket_size))
            .filter(|n| *n <= isize::MAX as usize)
            .ok_or(CuckooError::InvalidConfiguration)?;
        Ok(count)
    }

    /// Restore raw buckets with an explicitly supplied RNG state and configuration.
    pub fn from_export_with_rng(
        exported: ExportedCuckooFilter,
        bucket_size: usize,
        max_kicks: u32,
        rng: R,
    ) -> Result<Self, CuckooError> {
        if !(1..=255).contains(&bucket_size)
            || max_kicks == 0
            || exported.values.is_empty()
            || !exported.values.len().is_multiple_of(bucket_size)
            || !(exported.values.len() / bucket_size).is_power_of_two()
        {
            return Err(CuckooError::InvalidExport);
        }
        let buckets: Box<[_]> = exported
            .values
            .into_iter()
            .map(|b| Fingerprint { data: [b] })
            .collect();
        if buckets.iter().filter(|f| !f.is_empty()).count() != exported.length {
            return Err(CuckooError::InvalidExport);
        }
        Ok(Self {
            buckets,
            bucket_size,
            max_kicks,
            len: exported.length,
            rng,
            _hasher: PhantomData,
        })
    }

    /// Access the RNG for saving its state alongside exported fingerprints.
    pub fn rng(&self) -> &R {
        &self.rng
    }

    /// Number of allocated buckets.
    pub fn bucket_count(&self) -> usize {
        self.buckets.len() / self.bucket_size
    }

    fn bucket(&self, index: usize) -> &[Fingerprint] {
        let start = (index % self.bucket_count()) * self.bucket_size;
        &self.buckets[start..start + self.bucket_size]
    }

    /// Checks if `data` is in the filter.
    pub fn contains<T: ?Sized + Hash>(&self, data: &T) -> bool {
        let FaI { fp, i1, i2 } = get_fai::<T, H>(data);
        self.bucket(i1).contains(&fp) || self.bucket(i2).contains(&fp)
    }

    /// Adds `data` to the filter. Returns `Ok` if the insertion was successful,
    /// but could fail with a `NotEnoughSpace` error, especially when the filter
    /// is nearing its capacity.
    /// Note that while you can put any hashable type in the same filter, beware
    /// for side effects like that the same number can have diferent hashes
    /// depending on the type.
    /// So for the filter, 4711i64 isn't the same as 4711u64.
    ///
    /// **Note:** When this returns `NotEnoughSpace`, the element given was
    /// actually added to the filter, but some random *other* element was
    /// removed. This might improve in the future.
    pub fn add<T: ?Sized + Hash>(&mut self, data: &T) -> Result<(), CuckooError> {
        self.insert(data, false)
    }

    /// Insert without dropping existing fingerprints on failure.
    /// The RNG must clone into an independent state to roll back its stream too.
    pub fn try_add<T: ?Sized + Hash>(&mut self, data: &T) -> Result<(), CuckooError>
    where
        R: Clone,
    {
        let previous_rng = self.rng.clone();
        let result = self.insert(data, true);
        if result.is_err() {
            self.rng = previous_rng;
        }
        result
    }

    fn insert<T: ?Sized + Hash>(&mut self, data: &T, rollback: bool) -> Result<(), CuckooError> {
        let fai = get_fai::<T, H>(data);
        if self.put(fai.fp, fai.i1) || self.put(fai.fp, fai.i2) {
            return Ok(());
        }
        let mut index = fai.random_index(&mut self.rng);
        let mut fp = fai.fp;
        let mut changes = Vec::new();
        for _ in 0..self.max_kicks {
            let slot = (index % self.bucket_count()) * self.bucket_size
                + self.rng.gen_range(0..self.bucket_size);
            let displaced = self.buckets[slot];
            if rollback {
                changes.push((slot, displaced));
            }
            self.buckets[slot] = fp;
            index = get_alt_index::<H>(displaced, index);
            if self.put(displaced, index) {
                return Ok(());
            }
            fp = displaced;
        }
        if rollback {
            for (slot, previous) in changes.into_iter().rev() {
                self.buckets[slot] = previous;
            }
        }
        Err(CuckooError::NotEnoughSpace)
    }

    /// Adds `data` to the filter if it does not exist in the filter yet.
    /// Returns `Ok(true)` if `data` was not yet present in the filter and added
    /// successfully.
    pub fn test_and_add<T: ?Sized + Hash>(&mut self, data: &T) -> Result<bool, CuckooError> {
        if self.contains(data) {
            Ok(false)
        } else {
            self.add(data).map(|_| true)
        }
    }

    /// Number of items in the filter.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Exports fingerprints in all buckets, along with the filter's length for storage.
    /// The filter can be recovered by passing the `ExportedCuckooFilter` struct to the
    /// `from` method of `CuckooFilter`.
    /// RNG state is not exported; importing a filter initializes a new `ThreadRng`.
    pub fn export(&self) -> ExportedCuckooFilter {
        self.into()
    }

    /// Number of bytes the filter occupies in memory
    pub fn memory_usage(&self) -> usize {
        mem::size_of_val(self) + self.buckets.len() * mem::size_of::<Fingerprint>()
    }

    /// Check if filter is empty
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Deletes `data` from the filter. Returns true if `data` existed in the
    /// filter before.
    pub fn delete<T: ?Sized + Hash>(&mut self, data: &T) -> bool {
        let FaI { fp, i1, i2 } = get_fai::<T, H>(data);
        self.remove(fp, i1) || self.remove(fp, i2)
    }

    /// Empty all the buckets in a filter and reset the number of items.
    pub fn clear(&mut self) {
        if self.is_empty() {
            return;
        }

        for bucket in self.buckets.iter_mut() {
            *bucket = Fingerprint::empty();
        }
        self.len = 0;
    }

    /// Extracts fingerprint values from all buckets, used for exporting the filters data.
    fn values(&self) -> Vec<u8> {
        self.buckets.iter().map(|f| f.data[0]).collect()
    }

    fn remove(&mut self, fp: Fingerprint, index: usize) -> bool {
        let start = (index % self.bucket_count()) * self.bucket_size;
        if let Some(slot) = self.buckets[start..start + self.bucket_size]
            .iter_mut()
            .find(|f| **f == fp)
        {
            *slot = Fingerprint::empty();
            self.len -= 1;
            return true;
        }
        false
    }

    fn put(&mut self, fp: Fingerprint, index: usize) -> bool {
        let start = (index % self.bucket_count()) * self.bucket_size;
        if let Some(slot) = self.buckets[start..start + self.bucket_size]
            .iter_mut()
            .find(|f| f.is_empty())
        {
            *slot = fp;
            self.len += 1;
            return true;
        }
        false
    }
}

impl<H: Hasher + Default> CuckooFilter<H> {
    /// Construct a filter with default bucket size, eviction limit, and thread RNG.
    pub fn with_capacity(capacity: usize) -> Self {
        Self::with_config_and_rng(capacity, BUCKET_SIZE, MAX_REBUCKET, rand::thread_rng())
            .expect("invalid filter capacity")
    }
}

/// A minimal representation of the CuckooFilter which can be transfered or stored, then recovered at a later stage.
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde_support", derive(Deserialize, Serialize))]
pub struct ExportedCuckooFilter {
    #[cfg_attr(feature = "serde_support", serde(with = "serde_bytes"))]
    pub values: Vec<u8>,
    pub length: usize,
}

impl<H: Hasher + Default> From<ExportedCuckooFilter> for CuckooFilter<H> {
    fn from(exported: ExportedCuckooFilter) -> Self {
        Self::from_export_with_rng(exported, BUCKET_SIZE, MAX_REBUCKET, rand::thread_rng())
            .expect("invalid exported filter")
    }
}

impl<H, R> From<&CuckooFilter<H, R>> for ExportedCuckooFilter
where
    H: Hasher + Default,
    R: rand::RngCore,
{
    /// Converts a `CuckooFilter` into a simplified version which can be serialized and stored
    /// for later use.
    fn from(cuckoo: &CuckooFilter<H, R>) -> Self {
        Self {
            values: cuckoo.values(),
            length: cuckoo.len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    #[test]
    fn test_deterministic_eviction() {
        let mut filter1 = CuckooFilter::with_rng(100, ChaCha8Rng::seed_from_u64(42));
        let mut filter2 = CuckooFilter::with_rng(100, ChaCha8Rng::seed_from_u64(42));
        let mut failures = 0;

        // Capacity rounds up to 128 slots, so 150 insertions force eviction
        // and exercise the failure path after the filter fills up.
        for item in 0..150_u64 {
            let result1 = filter1.add(&item);
            let result2 = filter2.add(&item);
            assert_eq!(result1.is_ok(), result2.is_ok(), "item {item}");
            if result1.is_err() {
                failures += 1;
            }

            let state1 = filter1.export();
            let state2 = filter2.export();
            assert_eq!(state1.values, state2.values, "item {item}");
            assert_eq!(state1.length, state2.length, "item {item}");
        }

        assert!(failures > 0);
        // Check that eviction actually consumed the injected RNG.
        assert!(filter1.rng.get_word_pos() > 0);
        assert_eq!(filter1.rng.get_word_pos(), filter2.rng.get_word_pos());
    }
    #[test]
    fn transactional_insert_and_snapshot_resume() {
        let mut filter = CuckooFilter::<DefaultHasher, _>::with_config_and_rng(
            32,
            2,
            3,
            ChaCha8Rng::seed_from_u64(42),
        )
        .unwrap();
        let mut successes = Vec::new();
        let mut failures = 0;
        for item in 0..100_u64 {
            let before = filter.export();
            let position = filter.rng().get_word_pos();
            if filter.try_add(&item).is_ok() {
                successes.push(item);
            } else {
                failures += 1;
                assert_eq!(filter.export().values, before.values);
                assert_eq!(filter.len(), before.length);
                assert_eq!(filter.rng().get_word_pos(), position);
            }
            for value in &successes {
                assert!(filter.contains(value));
            }
        }
        assert!(failures > 0);
        let mut restored = CuckooFilter::<DefaultHasher, _>::from_export_with_rng(
            filter.export(),
            2,
            3,
            filter.rng().clone(),
        )
        .unwrap();
        for item in 100..200_u64 {
            assert_eq!(
                filter.try_add(&item).is_ok(),
                restored.try_add(&item).is_ok()
            );
            assert_eq!(filter.export().values, restored.export().values);
            assert_eq!(filter.rng().get_word_pos(), restored.rng().get_word_pos());
        }
    }

    #[test]
    fn configurable_buckets_and_invalid_snapshots() {
        for size in [1, 2, 4, 8, 255] {
            let filter = CuckooFilter::<DefaultHasher, _>::with_config_and_rng(
                100,
                size,
                10,
                ChaCha8Rng::seed_from_u64(42),
            )
            .unwrap();
            assert!(filter.bucket_count().is_power_of_two());
            assert_eq!(filter.export().values.len(), filter.bucket_count() * size);
            assert!(filter.export().values.len() >= 100);
            let mut invalid = filter.export();
            invalid.length = 1;
            assert!(CuckooFilter::<DefaultHasher, _>::from_export_with_rng(
                invalid,
                size,
                10,
                ChaCha8Rng::seed_from_u64(42),
            )
            .is_err());
        }
        assert!(CuckooFilter::<DefaultHasher>::allocation_size(usize::MAX, 4).is_err());
    }
}
