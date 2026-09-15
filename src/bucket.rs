pub const FINGERPRINT_SIZE: usize = 1;
pub const BUCKET_SIZE: usize = 4;
const EMPTY_FINGERPRINT_DATA: [u8; FINGERPRINT_SIZE] = [100; FINGERPRINT_SIZE];

// Fingerprint Size is 1 byte so lets remove the Vec
#[derive(PartialEq, Eq, Copy, Clone, Hash, Debug)]
pub struct Fingerprint {
    pub data: [u8; FINGERPRINT_SIZE],
}

impl Fingerprint {
    /// Attempts to create a new Fingerprint based on the given
    /// number. If the created Fingerprint would be equal to the
    /// empty Fingerprint, None is returned.
    pub fn from_data(data: [u8; FINGERPRINT_SIZE]) -> Option<Self> {
        let result = Self { data };
        if result.is_empty() {
            None
        } else {
            Some(result)
        }
    }

    /// Returns the empty Fingerprint.
    pub fn empty() -> Self {
        Self {
            data: EMPTY_FINGERPRINT_DATA,
        }
    }

    /// Checks if this is the empty Fingerprint.
    pub fn is_empty(&self) -> bool {
        self.data == EMPTY_FINGERPRINT_DATA
    }
}
