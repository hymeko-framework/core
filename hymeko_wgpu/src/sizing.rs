//! One checked size computation for every f32 storage allocation.
//!
//! # Why this is centralised
//!
//! Before Phase 9 the same arithmetic appeared four times — in
//! [`crate::ResourceFactory`], in [`crate::TensorArena`], in the readback
//! staging path, and implicitly wherever a size class was rounded — and three
//! of those wrote it as `(elements * size_of::<f32>()) as u64`. That expression
//! has two silent failure modes:
//!
//! * `elements * 4` can overflow `usize` and wrap to a small number, producing
//!   a buffer far smaller than the caller asked for;
//! * `as u64` truncates rather than reporting, so a 32-bit target would lose
//!   the high bits instead of erroring.
//!
//! Neither is reachable with today's shapes, which is exactly why they would
//! have survived until a shape made them reachable. Both are now checked, once.
//!
//! # The padding rule
//!
//! WGPU rejects a zero-sized buffer, so a logically empty tensor is backed by a
//! one-element allocation. That padding is a **storage** concept only: the
//! shape stays authoritative, every kernel bounds itself by `params`, never by
//! `arrayLength()`, and readback transfers `bytes`, not `padded_bytes`. Keeping
//! the two numbers in one struct is what stops a caller from padding a transfer
//! and reading four bytes of nothing back into a zero-element tensor.

use hymeko::tensor::compute::{ComputeError, TensorShape};

/// Bytes per `f32`, named so the arithmetic below reads as intent.
const F32_BYTES: usize = std::mem::size_of::<f32>();

/// WGPU rejects zero-sized buffers; this is the smallest legal allocation.
const MIN_BUFFER_BYTES: u64 = F32_BYTES as u64;

/// The checked result of sizing an f32 allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StorageExtent {
    /// Logical element count, from the shape.
    pub elements: usize,
    /// Logical bytes: `elements * 4`. Zero for an empty tensor.
    pub bytes: u64,
    /// Bytes to actually allocate — `bytes`, or the 4-byte floor when empty.
    pub padded_bytes: u64,
}

/// Size an f32 allocation for `shape`, checking every step.
///
/// # Postconditions
///
/// * `elements` is the shape's element count and did not overflow;
/// * `bytes == elements * 4` computed without wrapping, and fits in `u64`;
/// * `padded_bytes >= 4`, so it is always a legal WGPU allocation;
/// * `bytes <= max_buffer_size`.
///
/// # Errors
///
/// [`ComputeError::IndexOverflow`] if the element count or the byte length
/// overflows; [`ComputeError::ShapeMismatch`] if the allocation would exceed
/// the device's `max_buffer_size`, naming the limit it broke rather than
/// leaving a WGPU validation message to explain it.
pub(crate) fn f32_storage_extent(
    shape: TensorShape,
    max_buffer_size: u64,
) -> Result<StorageExtent, ComputeError> {
    let elements = shape
        .checked_elements()
        .ok_or(ComputeError::IndexOverflow {
            what: "tensor elements",
            value: usize::MAX,
        })?;
    f32_extent_for(elements, max_buffer_size)
}

/// As [`f32_storage_extent`], for a caller that already has an element count.
///
/// # Errors
///
/// As [`f32_storage_extent`].
pub(crate) fn f32_extent_for(
    elements: usize,
    max_buffer_size: u64,
) -> Result<StorageExtent, ComputeError> {
    let bytes = elements
        .checked_mul(F32_BYTES)
        .and_then(|b| u64::try_from(b).ok())
        .ok_or(ComputeError::IndexOverflow {
            what: "storage byte length",
            value: elements,
        })?;

    if bytes > max_buffer_size {
        return Err(ComputeError::ShapeMismatch {
            operand: "allocation exceeds the device maximum buffer size",
            expected: usize::try_from(max_buffer_size).unwrap_or(usize::MAX),
            actual: usize::try_from(bytes).unwrap_or(usize::MAX),
        });
    }

    Ok(StorageExtent {
        elements,
        bytes,
        padded_bytes: bytes.max(MIN_BUFFER_BYTES),
    })
}

/// Round a byte count up to a power-of-two size class, without wrapping.
///
/// `u64::next_power_of_two` panics in debug and wraps to zero in release when
/// the value exceeds `2^63`. A size class of zero would make an arena hand out
/// a zero-sized buffer for an enormous request — a wrong-results bug, not an
/// allocation failure. This reports instead.
///
/// # Errors
///
/// [`ComputeError::IndexOverflow`] when no power of two can hold `bytes`.
pub(crate) fn checked_size_class(bytes: u64) -> Result<u64, ComputeError> {
    bytes
        .checked_next_power_of_two()
        .ok_or(ComputeError::IndexOverflow {
            what: "buffer size class exceeds the largest representable power of two",
            value: usize::try_from(bytes).unwrap_or(usize::MAX),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMIT: u64 = 1 << 40;

    #[test]
    fn ordinary_shapes_size_exactly() {
        let e = f32_storage_extent(TensorShape::new(1000, 8), LIMIT).expect("ordinary");
        assert_eq!(e.elements, 8000);
        assert_eq!(e.bytes, 32_000);
        assert_eq!(e.padded_bytes, 32_000);
    }

    #[test]
    fn an_empty_shape_pads_storage_but_not_its_logical_size() {
        let e = f32_storage_extent(TensorShape::new(0, 16), LIMIT).expect("empty is legal");
        assert_eq!(e.elements, 0);
        assert_eq!(e.bytes, 0, "an empty tensor transfers no bytes");
        assert_eq!(
            e.padded_bytes, 4,
            "but must still be backed by a legal allocation"
        );
    }

    #[test]
    fn a_byte_length_that_would_wrap_is_reported_not_truncated() {
        // usize::MAX / 2 elements * 4 bytes overflows usize on 64-bit.
        let err = f32_extent_for(usize::MAX / 2, u64::MAX)
            .expect_err("an overflowing byte length must be refused");
        assert!(
            matches!(err, ComputeError::IndexOverflow { .. }),
            "expected IndexOverflow, got {err:?}"
        );
    }

    #[test]
    fn the_device_limit_is_enforced_by_name() {
        // 1024 elements = 4096 bytes.
        assert!(f32_extent_for(1024, 4096).is_ok(), "exact fit is allowed");
        let err = f32_extent_for(1024, 4095).expect_err("one byte over must be refused");
        match err {
            ComputeError::ShapeMismatch {
                operand,
                expected,
                actual,
            } => {
                assert!(operand.contains("maximum buffer size"), "{operand}");
                assert_eq!(expected, 4095);
                assert_eq!(actual, 4096);
            }
            other => panic!("expected ShapeMismatch, got {other:?}"),
        }
    }

    #[test]
    fn size_classes_round_up_and_refuse_to_wrap() {
        assert_eq!(checked_size_class(1).expect("1"), 1);
        assert_eq!(checked_size_class(5).expect("5"), 8);
        assert_eq!(checked_size_class(1 << 20).expect("1 MiB"), 1 << 20);
        // Above 2^63 there is no representable power of two. The old
        // `next_power_of_two` wrapped to 0 here in release builds.
        let err = checked_size_class((1u64 << 63) + 1).expect_err("must not wrap to zero");
        assert!(
            matches!(err, ComputeError::IndexOverflow { .. }),
            "expected IndexOverflow, got {err:?}"
        );
    }
}
