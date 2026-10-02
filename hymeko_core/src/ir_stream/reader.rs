//! Shared fixed-width little-endian decoding after the caller's bounds checks.

/// A wire value decoded from its fixed-size byte array, never a native-memory cast.
pub(super) trait LittleEndianValue: Sized {
    type Bytes: Default + AsMut<[u8]>;

    fn from_le_bytes(bytes: Self::Bytes) -> Self;
}

/// Borrows wire bytes; reads copy into aligned local arrays without allocation.
pub(super) struct LittleEndianReader<'a> {
    bytes: &'a [u8],
}

impl<'a> LittleEndianReader<'a> {
    pub(super) const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    /// Read a value at an absolute byte offset, with static type dispatch.
    ///
    /// # Panics
    /// The caller must establish that the complete field lies within the input.
    /// Protocol validators retain their own ordered, diagnostic bounds checks.
    pub(super) fn read<T: LittleEndianValue>(&self, offset: usize) -> T {
        let mut bytes = T::Bytes::default();
        let target = bytes.as_mut();
        let width = target.len();
        // Slice in two steps so even a malformed offset cannot overflow offset+width.
        target.copy_from_slice(&self.bytes[offset..][..width]);
        T::from_le_bytes(bytes)
    }
}

macro_rules! little_endian_values {
    ($($value:ty => $width:expr),+ $(,)?) => {
        $(impl LittleEndianValue for $value {
            type Bytes = [u8; $width];

            fn from_le_bytes(bytes: Self::Bytes) -> Self {
                <$value>::from_le_bytes(bytes)
            }
        })+
    };
}

little_endian_values!(u16 => 2, u32 => 4, u64 => 8, f32 => 4);

#[cfg(test)]
mod tests {
    use super::LittleEndianReader;

    #[test]
    fn reads_known_integer_images_at_unaligned_offsets_and_exact_ends() {
        let bytes = [0, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef];
        let reader = LittleEndianReader::new(&bytes);
        assert_eq!(reader.read::<u16>(1), 0x2301);
        assert_eq!(reader.read::<u32>(1), 0x6745_2301);
        assert_eq!(reader.read::<u64>(1), 0xefcd_ab89_6745_2301);
        assert_eq!(reader.read::<u16>(7), 0xefcd);
        let maximum = LittleEndianReader::new(&[0xff; 8]);
        assert_eq!(maximum.read::<u16>(0), u16::MAX);
        assert_eq!(maximum.read::<u32>(0), u32::MAX);
        assert_eq!(maximum.read::<u64>(0), u64::MAX);
    }

    #[test]
    fn float_reads_preserve_signed_zero_infinity_and_nan_payload_bits() {
        for (bytes, bits) in [
            ([0, 0, 0xc0, 0x3f], 0x3fc0_0000),
            ([0, 0, 0, 0x80], 0x8000_0000),
            ([0, 0, 0x80, 0x7f], 0x7f80_0000),
            ([0x45, 0x23, 0xc1, 0x7f], 0x7fc1_2345),
        ] {
            assert_eq!(
                LittleEndianReader::new(&bytes).read::<f32>(0).to_bits(),
                bits
            );
        }
    }

    #[test]
    fn invalid_internal_reads_panic_without_offset_arithmetic_overflow() {
        for (bytes, offset) in [(&[][..], 0), (&[0; 8][..], 1), (&[0; 8][..], usize::MAX)] {
            assert!(
                std::panic::catch_unwind(|| LittleEndianReader::new(bytes).read::<u64>(offset))
                    .is_err()
            );
        }
    }
}
