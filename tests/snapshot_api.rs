use cuckoofilter::{CuckooError, CuckooFilter, ItemHash};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use std::collections::hash_map::DefaultHasher;
use std::panic::{catch_unwind, AssertUnwindSafe};

type Filter = CuckooFilter<DefaultHasher, ChaCha8Rng>;

fn filter(capacity: usize, bucket_size: usize) -> Filter {
    Filter::with_config_and_rng(capacity, bucket_size, 32, ChaCha8Rng::seed_from_u64(42)).unwrap()
}

fn assert_same(left: &Filter, right: &Filter) {
    assert_eq!(left.as_bytes(), right.as_bytes());
    assert_eq!(left.len(), right.len());
    assert_eq!(left.rng().get_word_pos(), right.rng().get_word_pos());
}

#[test]
fn hashed_operations_match_wrappers_across_configurations() {
    let hashes: Vec<ItemHash> = (0..300_u64).map(|item| Filter::hash_item(&item)).collect();
    for (capacity, bucket_size) in [(1, 1), (32, 2), (64, 4), (100, 8), (256, 255)] {
        let mut direct = filter(capacity, bucket_size);
        let mut hashed = direct.clone();
        let mut accepted = Vec::new();
        let mut failures = 0;
        for (item, h) in hashes.iter().enumerate() {
            let before = hashed.clone();
            let result = hashed.try_add_hashed(h);
            assert_eq!(result.is_ok(), direct.try_add(&(item as u64)).is_ok());
            assert_same(&direct, &hashed);
            if result.is_ok() {
                accepted.push(item);
            } else {
                failures += 1;
                assert!(matches!(result, Err(CuckooError::NotEnoughSpace)));
                assert_same(&hashed, &before);
            }
            for &item in &accepted {
                assert!(hashed.contains_hashed(&hashes[item]));
            }
        }
        if capacity < 100 {
            assert!(failures > 0);
        }
        for (item, h) in hashes.iter().enumerate() {
            assert_eq!(hashed.contains_hashed(h), direct.contains(&(item as u64)));
        }
        for item in accepted {
            assert!(hashed.delete_hashed(&hashes[item]));
            assert!(direct.delete(&(item as u64)));
            assert_same(&direct, &hashed);
        }
        assert!(hashed.is_empty());
        assert!(hashed.as_bytes().iter().all(|&fp| fp == 100));
    }
}

#[test]
fn duplicate_counts_and_no_eviction_failure() {
    // One bucket exercises coincident indices. Larger filters also exercise
    // the second candidate bucket while other buckets still have free space.
    let mut saw_two_buckets = false;
    for capacity in [2, 16] {
        for item in 0..32_u64 {
            let mut f = filter(capacity, 2);
            let h = Filter::hash_item(&item);
            assert!(!f.contains_hashed(&h));
            assert_eq!(f.count_hashed(&h), 0);
            assert!(!f.delete_hashed(&h));
            while f.try_add_no_evict_hashed(&h) {
                assert!(f.contains_hashed(&h));
                assert_eq!(f.count_hashed(&h), f.len());
                assert_eq!(f.rng().get_word_pos(), 0);
            }
            let copies = f.len();
            if capacity == 2 {
                assert_eq!(copies, 2);
            } else {
                assert!(copies == 2 || copies == 4);
                saw_two_buckets |= copies == 4;
                assert!(f.len() < f.as_bytes().len());
            }
            let before = f.clone();
            for _ in 0..3 {
                assert!(!f.try_add_no_evict_hashed(&h));
                assert_same(&f, &before);
                assert!(matches!(
                    f.try_add_hashed(&h),
                    Err(CuckooError::NotEnoughSpace)
                ));
                assert_same(&f, &before);
            }
            for remaining in (0..copies).rev() {
                assert!(f.delete_hashed(&h));
                assert_eq!(f.count_hashed(&h), remaining);
                assert_eq!(f.len(), remaining);
            }
            assert!(!f.delete_hashed(&h));
            assert!(!f.contains_hashed(&h));
            assert!(f.try_add_no_evict_hashed(&h));
            assert_eq!(f.len(), 1);
        }
    }
    assert!(saw_two_buckets);
}

#[test]
fn transactional_insertion_stores_every_duplicate() {
    let mut f = filter(4, 4);
    let h = Filter::hash_item("duplicate");
    for expected in 1..=4 {
        f.try_add_hashed(&h).unwrap();
        assert_eq!(f.len(), expected);
        assert_eq!(f.count_hashed(&h), expected);
    }
    assert!(f.delete_hashed(&h));
    assert_eq!(f.count_hashed(&h), 3);
    assert!(f.contains_hashed(&h));
}

#[test]
fn raw_restore_reuses_allocation_and_resumes_rng() {
    let mut original = filter(32, 2);
    for item in 0..40_u64 {
        let _ = original.try_add(&item);
    }
    assert!(original.rng().get_word_pos() > 0);
    let exported = original.export();
    assert_eq!(original.as_bytes(), exported.values);
    let bytes = exported.values.into_boxed_slice();
    let pointer = bytes.as_ptr();
    let mut restored =
        Filter::from_bytes_with_rng(bytes, exported.length, 2, 32, original.rng().clone()).unwrap();
    assert_eq!(restored.as_bytes().as_ptr(), pointer);
    assert_eq!(
        restored.memory_usage(),
        std::mem::size_of_val(&restored) + restored.as_bytes().len()
    );
    assert_same(&original, &restored);
    original.clear();
    restored.clear();
    for item in 100..140_u64 {
        assert_eq!(
            original.try_add(&item).is_ok(),
            restored.try_add(&item).is_ok()
        );
        assert_same(&original, &restored);
    }
}

#[test]
fn raw_restore_rejects_invalid_configuration_and_lengths() {
    for (bytes, length, bucket_size, max_kicks) in [
        (vec![], 0, 4, 32),
        (vec![100; 4], 0, 0, 32),
        (vec![100; 256], 0, 256, 32),
        (vec![100; 4], 0, 4, 0),
        (vec![100; 5], 0, 4, 32),
        (vec![100; 12], 0, 4, 32),
        (vec![100; 4], 1, 4, 32),
        (vec![0; 4], 0, 4, 32),
        (vec![0; 4], 5, 4, 32),
        (vec![0; 4], usize::MAX, 4, 32),
    ] {
        assert!(matches!(
            Filter::from_bytes_with_rng(
                bytes.into_boxed_slice(),
                length,
                bucket_size,
                max_kicks,
                ChaCha8Rng::seed_from_u64(42),
            ),
            Err(CuckooError::InvalidExport)
        ));
    }
    // Every possible byte is valid. Only the empty sentinel is not counted.
    let all_bytes: Box<[u8]> = (0..=255).collect();
    let f =
        Filter::from_bytes_with_rng(all_bytes, 255, 4, 32, ChaCha8Rng::seed_from_u64(42)).unwrap();
    assert_eq!(f.len(), 255);
}

#[test]
fn realloc_preserves_bytes_rng_and_membership() {
    let mut f = filter(32, 2);
    for item in 0..30_u64 {
        let _ = f.try_add(&item);
    }
    let before = f.clone();
    let pointer = f.as_bytes().as_ptr();
    f.realloc_buckets(|bytes| bytes);
    assert_eq!(f.as_bytes().as_ptr(), pointer);
    assert_same(&f, &before);
    f.realloc_buckets(|bytes| {
        let moved = bytes.to_vec().into_boxed_slice();
        // Both allocations are alive, so their addresses must differ.
        assert_ne!(moved.as_ptr(), bytes.as_ptr());
        moved
    });
    assert_ne!(f.as_bytes().as_ptr(), pointer);
    assert_same(&f, &before);
    for item in 0..30_u64 {
        assert_eq!(f.contains(&item), before.contains(&item));
    }
    let mut expected = before;
    for item in 30..60_u64 {
        assert_eq!(f.try_add(&item).is_ok(), expected.try_add(&item).is_ok());
        assert_same(&f, &expected);
    }
}

#[test]
fn realloc_panics_leave_a_valid_empty_filter() {
    for bad_length in [None, Some(0), Some(4), Some(64)] {
        let mut f = filter(32, 2);
        f.try_add("item").unwrap();
        let position = f.rng().get_word_pos();
        assert!(catch_unwind(AssertUnwindSafe(|| {
            f.realloc_buckets(|_| match bad_length {
                None => panic!("callback failed"),
                Some(len) => vec![100; len].into_boxed_slice(),
            });
        }))
        .is_err());
        assert_eq!(f.as_bytes(), [100; 32]);
        assert!(f.is_empty());
        assert_eq!(f.bucket_count(), 16);
        assert_eq!(f.rng().get_word_pos(), position);
        f.try_add("after panic").unwrap();
        assert!(f.contains("after panic"));
        assert_eq!(f.len(), 1);
    }
}
