use cuckoofilter::CuckooFilter;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use siphasher::sip::SipHasher13;
use std::hash::Hasher;

// Captured from dbaa5d0 before changing bucket storage or insertion dispatch.
// Do not regenerate these values merely to accommodate a failing test.
const EXPECTED_BUCKETS: [u8; 64] = [
    142, 84, 122, 190, 253, 101, 202, 223, 168, 103, 141, 47, 221, 105, 221, 15, 50, 229, 147, 164,
    244, 125, 133, 31, 188, 194, 107, 200, 149, 248, 254, 170, 122, 182, 136, 202, 204, 190, 140,
    108, 32, 180, 182, 57, 134, 24, 35, 136, 66, 207, 82, 161, 242, 82, 254, 62, 58, 232, 213, 48,
    214, 96, 91, 8,
];
const EXPECTED_RNG_WORD_POS: u128 = 291;

// Match Valkey's fixed keys and architecture-independent length encoding.
#[derive(Clone)]
struct FixedHasher(SipHasher13);

impl Default for FixedHasher {
    fn default() -> Self {
        Self(SipHasher13::new_with_keys(0, 0))
    }
}

impl Hasher for FixedHasher {
    fn finish(&self) -> u64 {
        self.0.finish()
    }

    fn write(&mut self, bytes: &[u8]) {
        self.0.write(bytes);
    }

    fn write_usize(&mut self, value: usize) {
        self.write(&(value as u64).to_le_bytes());
    }
}

#[test]
fn deterministic_snapshot_fixture() {
    let mut filter = CuckooFilter::<FixedHasher, _>::with_config_and_rng(
        64,
        4,
        32,
        ChaCha8Rng::seed_from_u64(42),
    )
    .unwrap();
    let mut accepted = Vec::new();
    for item in 0..100_u64 {
        if filter.try_add(&item.to_le_bytes()).is_ok() {
            accepted.push(item);
        }
    }
    assert_eq!(accepted, (0..63).chain([84]).collect::<Vec<_>>());
    assert_eq!(filter.len(), EXPECTED_BUCKETS.len());
    assert_eq!(filter.as_bytes(), EXPECTED_BUCKETS);
    assert_eq!(filter.rng().get_word_pos(), EXPECTED_RNG_WORD_POS);
    for item in &accepted {
        assert!(filter.contains(&item.to_le_bytes()));
    }

    let mut rng = ChaCha8Rng::seed_from_u64(42);
    rng.set_word_pos(EXPECTED_RNG_WORD_POS);
    let mut restored = CuckooFilter::<FixedHasher, _>::from_bytes_with_rng(
        Box::new(EXPECTED_BUCKETS),
        EXPECTED_BUCKETS.len(),
        4,
        32,
        rng,
    )
    .unwrap();
    for item in accepted.iter().step_by(3) {
        assert!(filter.delete(&item.to_le_bytes()));
        assert!(restored.delete(&item.to_le_bytes()));
    }
    let position = filter.rng().get_word_pos();
    for item in 100..150_u64 {
        let h = CuckooFilter::<FixedHasher, ChaCha8Rng>::hash_item(&item.to_le_bytes());
        assert_eq!(
            filter.try_add_hashed(&h).is_ok(),
            restored.try_add_hashed(&h).is_ok()
        );
        assert_eq!(filter.as_bytes(), restored.as_bytes());
        assert_eq!(filter.len(), restored.len());
        assert_eq!(filter.rng().get_word_pos(), restored.rng().get_word_pos());
    }
    assert!(filter.rng().get_word_pos() > position);
}
