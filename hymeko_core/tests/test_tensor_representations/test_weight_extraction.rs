//! Regression tests for `hymeko::tensor::tensor_val::extract_ref_weight_scalar`.
//!
//! Written **before** the 2026-08-24 `extract-ref-weight-noalloc` change and
//! run against the pre-change implementation first, so they encode the existing
//! behaviour rather than the behaviour of the rewrite. Every assertion here
//! passed against the allocating version.
//!
//! The contract has three sharp edges that a naive rewrite loses:
//!
//! 1. **`None` and "no numeric values found" both yield `one()`, not `zero()`.**
//!    That includes `Some(vec![])` and a `Some` containing only `Str` / `Ref`.
//! 2. **Exactly one value is returned verbatim**, *not* folded from zero.
//!    `zero() + x == x` is false bitwise for `x = -0.0` (it gives `+0.0`), so
//!    collapsing the one-value case into the general fold is an observable
//!    change. `test_single_negative_zero_is_returned_verbatim` pins this.
//! 3. **`List` is descended exactly one level.** A `List` nested inside a
//!    `List` contributes nothing. `test_nested_list_is_not_recursed` pins this.
//!
//! Both `f32` and `f64` instantiations are checked, since `F::from_other`
//! takes a different path for each.

#[cfg(test)]
mod test_weight_extraction {
    use crate::test_helpers::{log_test_footer, log_test_header};
    use hymeko::common::ids::{DeclId, SymId};
    use hymeko::ir::ir::ValueR;
    use hymeko::tensor::tensor_val::extract_ref_weight_scalar;
    use std::time::Instant;

    fn nums(xs: &[f64]) -> Option<Vec<ValueR>> {
        Some(xs.iter().map(|&x| ValueR::Num(x)).collect())
    }

    #[test]
    fn test_none_yields_one() {
        log_test_header(
            "test_none_yields_one",
            "A reference with no weights defaults to 1.0, not 0.0.",
        );
        let start = Instant::now();

        let none: Option<Vec<ValueR>> = None;
        assert_eq!(extract_ref_weight_scalar::<f32>(&none), 1.0f32);
        assert_eq!(extract_ref_weight_scalar::<f64>(&none), 1.0f64);

        log_test_footer(
            "test_none_yields_one",
            Some(start.elapsed()),
            "None mapped to 1.0 in both f32 and f64.",
        );
    }

    #[test]
    fn test_empty_and_non_numeric_yield_one() {
        log_test_header(
            "test_empty_and_non_numeric_yield_one",
            "Some(empty) and Some(only non-numeric) must also default to 1.0.",
        );
        let start = Instant::now();

        let empty = Some(Vec::<ValueR>::new());
        assert_eq!(extract_ref_weight_scalar::<f32>(&empty), 1.0f32);
        assert_eq!(extract_ref_weight_scalar::<f64>(&empty), 1.0f64);

        // Str / Ref carry no numeric mass and must be skipped, leaving the
        // "nothing found" path -- which is 1.0, not 0.0.
        let non_numeric = Some(vec![
            ValueR::Str(SymId::new(3)),
            ValueR::Ref(DeclId::new(7)),
        ]);
        assert_eq!(extract_ref_weight_scalar::<f32>(&non_numeric), 1.0f32);
        assert_eq!(extract_ref_weight_scalar::<f64>(&non_numeric), 1.0f64);

        // An empty inner list is likewise "nothing found".
        let empty_list = Some(vec![ValueR::List(Vec::new())]);
        assert_eq!(extract_ref_weight_scalar::<f32>(&empty_list), 1.0f32);

        log_test_footer(
            "test_empty_and_non_numeric_yield_one",
            Some(start.elapsed()),
            "Empty and non-numeric weight lists all defaulted to 1.0.",
        );
    }

    #[test]
    fn test_single_value_is_returned_verbatim() {
        log_test_header(
            "test_single_value_is_returned_verbatim",
            "One numeric weight is returned as-is.",
        );
        let start = Instant::now();

        for &x in &[0.5f64, -2.25, 1e-30, 1e30, 0.0] {
            let w = nums(&[x]);
            assert_eq!(
                extract_ref_weight_scalar::<f64>(&w).to_bits(),
                x.to_bits(),
                "f64 single value {x} not returned verbatim"
            );
            assert_eq!(
                extract_ref_weight_scalar::<f32>(&w).to_bits(),
                (x as f32).to_bits(),
                "f32 single value {x} not returned verbatim"
            );
        }

        log_test_footer(
            "test_single_value_is_returned_verbatim",
            Some(start.elapsed()),
            "Single weights round-tripped bit-for-bit.",
        );
    }

    #[test]
    fn test_single_negative_zero_is_returned_verbatim() {
        log_test_header(
            "test_single_negative_zero_is_returned_verbatim",
            "The one-value case must NOT be folded from zero: 0.0 + (-0.0) = +0.0.",
        );
        let start = Instant::now();

        let w = nums(&[-0.0f64]);

        // This is the assertion that distinguishes "return the single value"
        // from "fold the general case from zero". Folding would yield +0.0.
        let got64 = extract_ref_weight_scalar::<f64>(&w);
        assert_eq!(
            got64.to_bits(),
            (-0.0f64).to_bits(),
            "expected -0.0 (sign bit set), got {got64} with bits {:#x}",
            got64.to_bits()
        );
        assert!(got64.is_sign_negative(), "sign bit was lost");

        let got32 = extract_ref_weight_scalar::<f32>(&w);
        assert_eq!(got32.to_bits(), (-0.0f32).to_bits(), "f32 -0.0 sign lost");

        // Sanity: the general (>=2) path *does* start from zero, so two
        // negative zeros collapse to +0.0. Pinning the asymmetry deliberately.
        let two = nums(&[-0.0f64, -0.0f64]);
        let got_two = extract_ref_weight_scalar::<f64>(&two);
        assert_eq!(
            got_two.to_bits(),
            (0.0f64).to_bits(),
            "the multi-value fold should start from +0.0"
        );

        log_test_footer(
            "test_single_negative_zero_is_returned_verbatim",
            Some(start.elapsed()),
            "Negative zero survived the single-value path; the multi-value fold still starts at +0.0.",
        );
    }

    #[test]
    fn test_multiple_values_sum_in_order() {
        log_test_header(
            "test_multiple_values_sum_in_order",
            "Two or more weights fold left-to-right starting from zero.",
        );
        let start = Instant::now();

        let w = nums(&[1.0, 2.0, 3.5]);
        assert_eq!(extract_ref_weight_scalar::<f64>(&w), 6.5f64);
        assert_eq!(extract_ref_weight_scalar::<f32>(&w), 6.5f32);

        // Order matters in floating point. This sequence sums to a different
        // value if reassociated, so it pins left-to-right accumulation.
        let tricky = nums(&[1.0e16, 1.0, -1.0e16]);
        let expected = ((0.0f64 + 1.0e16) + 1.0) + -1.0e16;
        assert_eq!(
            extract_ref_weight_scalar::<f64>(&tricky).to_bits(),
            expected.to_bits(),
            "accumulation order changed"
        );

        log_test_footer(
            "test_multiple_values_sum_in_order",
            Some(start.elapsed()),
            "Multi-value weights folded left-to-right from zero.",
        );
    }

    #[test]
    fn test_list_is_flattened_one_level() {
        log_test_header(
            "test_list_is_flattened_one_level",
            "Numbers inside a List contribute; the List counts as its members.",
        );
        let start = Instant::now();

        let w = Some(vec![ValueR::List(vec![ValueR::Num(1.0), ValueR::Num(2.0)])]);
        assert_eq!(extract_ref_weight_scalar::<f64>(&w), 3.0f64);

        // A single number reached through a List still takes the one-value
        // path, so it is returned verbatim rather than folded.
        let single_in_list = Some(vec![ValueR::List(vec![ValueR::Num(-0.0)])]);
        assert_eq!(
            extract_ref_weight_scalar::<f64>(&single_in_list).to_bits(),
            (-0.0f64).to_bits(),
            "a single value inside a List must still take the verbatim path"
        );

        // Mixed top-level and nested values accumulate together.
        let mixed = Some(vec![
            ValueR::Num(1.0),
            ValueR::List(vec![ValueR::Num(2.0), ValueR::Str(SymId::new(1))]),
            ValueR::Num(4.0),
        ]);
        assert_eq!(extract_ref_weight_scalar::<f64>(&mixed), 7.0f64);

        log_test_footer(
            "test_list_is_flattened_one_level",
            Some(start.elapsed()),
            "List members contributed, and a lone nested value stayed verbatim.",
        );
    }

    #[test]
    fn test_nested_list_is_not_recursed() {
        log_test_header(
            "test_nested_list_is_not_recursed",
            "A List inside a List contributes nothing - recursion is one level only.",
        );
        let start = Instant::now();

        // Only the inner-most `Num` would contribute if this recursed. It must
        // not: the implementation matches `Num` inside a `List`, nothing deeper.
        let doubly_nested = Some(vec![ValueR::List(vec![ValueR::List(vec![ValueR::Num(
            5.0,
        )])])]);
        assert_eq!(
            extract_ref_weight_scalar::<f64>(&doubly_nested),
            1.0f64,
            "a doubly-nested list must find no values and fall back to 1.0"
        );

        // One real value beside a nested list: only the real value counts, and
        // being the only one it takes the verbatim path.
        let mixed = Some(vec![
            ValueR::Num(3.0),
            ValueR::List(vec![ValueR::List(vec![ValueR::Num(99.0)])]),
        ]);
        assert_eq!(extract_ref_weight_scalar::<f64>(&mixed), 3.0f64);

        log_test_footer(
            "test_nested_list_is_not_recursed",
            Some(start.elapsed()),
            "Doubly-nested lists contributed nothing, as before.",
        );
    }

    #[test]
    fn test_f32_and_f64_conversion_paths() {
        log_test_header(
            "test_f32_and_f64_conversion_paths",
            "F::from_other takes a different route per type; both must match the old behaviour.",
        );
        let start = Instant::now();

        // A value that is not exactly representable in f32 exercises the
        // narrowing conversion, and must narrow per-element (as the old code
        // did) rather than summing in f64 and narrowing once.
        let w = nums(&[0.1, 0.2]);
        let expected32 = (0.0f32 + 0.1f64 as f32) + 0.2f64 as f32;
        assert_eq!(
            extract_ref_weight_scalar::<f32>(&w).to_bits(),
            expected32.to_bits(),
            "f32 accumulation must narrow each term before adding"
        );

        let expected64 = (0.0f64 + 0.1) + 0.2;
        assert_eq!(
            extract_ref_weight_scalar::<f64>(&w).to_bits(),
            expected64.to_bits(),
            "f64 accumulation mismatch"
        );

        log_test_footer(
            "test_f32_and_f64_conversion_paths",
            Some(start.elapsed()),
            "Per-element narrowing preserved for f32; f64 path exact.",
        );
    }
}
