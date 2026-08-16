#[cfg(test)]
mod tests {
    use crate::test_helpers::{log_test_footer, log_test_header};
    use hymeko::tensor::aggregation::{AggCfg, SignAgg, WeightAgg, agg_sign, agg_weight, clamp01};
    use log::info;
    use std::time::Instant;

    fn cfg(weight: WeightAgg, sign: SignAgg, clamp01: bool) -> AggCfg {
        AggCfg {
            weight,
            sign,
            clamp01,
        }
    }

    const EPS_F32: f32 = 1e-6;
    const CLAMP_NEG_HALF: f32 = -0.5;
    const CLAMP_LOW: f32 = 0.2;
    const CLAMP_HIGH: f32 = 1.2;
    const WEIGHT_LOW: f32 = 0.2;
    const WEIGHT_MED: f32 = 0.3;
    const WEIGHT_TINY: f32 = 0.1;
    const WEIGHT_HIGH: f32 = 0.9;
    const PROB_BOUNDARY_ONE: f32 = 1.0;
    const VOTE_STRONG: f32 = 10.0;
    const VOTE_WEAK: f32 = 1.0;
    const VOTE_TIE: f32 = 2.0;
    const LUKAS_HIGH_LEFT: f32 = 0.7;
    const LUKAS_HIGH_RIGHT: f32 = 0.6;

    fn start(name: &str, desc: &str) -> Instant {
        log_test_header(name, desc);
        Instant::now()
    }

    fn finish(name: &str, timer: Instant, summary: &str) {
        log_test_footer(name, Some(timer.elapsed()), summary);
    }

    #[test]
    fn test_clamp01_basic() {
        let t = start(
            "test_clamp01_basic",
            "Validates clamp01 behavior and interaction with WeightAgg::Sum.",
        );
        let c = AggCfg {
            weight: WeightAgg::Sum,
            sign: SignAgg::PreferNonNeutral,
            clamp01: true,
        };

        assert_eq!(clamp01::<f32>(CLAMP_NEG_HALF), 0.0);
        assert_eq!(clamp01::<f32>(CLAMP_LOW), CLAMP_LOW);
        assert_eq!(clamp01::<f32>(CLAMP_HIGH), 1.0);

        // clamp01 flag only affects agg_weight output, clamp01() itself is unconditional
        let out = agg_weight(&c, CLAMP_NEG_HALF, CLAMP_LOW);
        assert_eq!(out, 0.0); // Sum = -0.3 -> clamp -> 0
        finish(
            "test_clamp01_basic",
            t,
            "Clamp logic zeroed negatives and capped positives as expected.",
        );
    }

    #[test]
    fn test_weight_agg_sum() {
        let t = start(
            "test_weight_agg_sum",
            "Checks WeightAgg::Sum adds contributions without clamping.",
        );
        let c = cfg(WeightAgg::Sum, SignAgg::PreferNonNeutral, false);
        let out = agg_weight(&c, WEIGHT_LOW, WEIGHT_MED);
        assert!((out - (WEIGHT_LOW + WEIGHT_MED)).abs() < EPS_F32);
        finish(
            "test_weight_agg_sum",
            t,
            "Simple sum matched arithmetic addition.",
        );
    }

    #[test]
    fn test_weight_agg_max() {
        let t = start(
            "test_weight_agg_max",
            "Ensures WeightAgg::Max picks the larger operand.",
        );
        let c = cfg(WeightAgg::Max, SignAgg::PreferNonNeutral, false);

        let out1 = agg_weight(&c, WEIGHT_LOW, WEIGHT_MED);
        assert!((out1 - WEIGHT_MED).abs() < EPS_F32);

        let out2 = agg_weight(&c, WEIGHT_HIGH, WEIGHT_TINY);
        assert!((out2 - WEIGHT_HIGH).abs() < EPS_F32);

        // extra: equal values
        let out3 = agg_weight(&c, 0.42_f32, 0.42_f32);
        assert!((out3 - 0.42).abs() < EPS_F32);
        finish(
            "test_weight_agg_max",
            t,
            "Max aggregator stayed stable for ties and extremes.",
        );
    }

    #[test]
    fn test_weight_agg_prob_sum01() {
        let t = start(
            "test_weight_agg_prob_sum01",
            "Verifies probabilistic sum formula and unit boundary behavior.",
        );
        let c = cfg(WeightAgg::ProbSum01, SignAgg::PreferNonNeutral, false);

        // 1 - (1-a)(1-b)
        let out = agg_weight(&c, WEIGHT_LOW, WEIGHT_MED);
        let expected = 1.0 - (1.0 - WEIGHT_LOW) * (1.0 - WEIGHT_MED); // 0.44
        assert!((out - expected).abs() < EPS_F32);

        // boundary
        let out2 = agg_weight(&c, PROB_BOUNDARY_ONE, WEIGHT_TINY);
        assert!((out2 - PROB_BOUNDARY_ONE).abs() < EPS_F32);
        finish(
            "test_weight_agg_prob_sum01",
            t,
            "ProbSum01 maintained bounds and identity.",
        );
    }

    #[test]
    fn test_weight_agg_lukasiewicz_sat01() {
        let t = start(
            "test_weight_agg_lukasiewicz_sat01",
            "Checks Lukasiewicz saturation hits 1.0 for overflowing sums.",
        );
        let c = cfg(WeightAgg::LukasSat01, SignAgg::PreferNonNeutral, false);

        let out1 = agg_weight(&c, WEIGHT_LOW, WEIGHT_MED);
        assert!((out1 - (WEIGHT_LOW + WEIGHT_MED)).abs() < EPS_F32);

        let out2 = agg_weight(&c, LUKAS_HIGH_LEFT, LUKAS_HIGH_RIGHT);
        assert!((out2 - 1.0).abs() < EPS_F32);
        finish(
            "test_weight_agg_lukasiewicz_sat01",
            t,
            "Lukasiewicz saturation capped at unity as expected.",
        );
    }

    #[test]
    fn test_weight_agg_clamp01_flag() {
        let t = start(
            "test_weight_agg_clamp01_flag",
            "Ensures clamp01 flag truncates negative and >1 sums.",
        );
        let c = cfg(WeightAgg::Sum, SignAgg::PreferNonNeutral, true);

        let out1 = agg_weight(&c, -WEIGHT_LOW, WEIGHT_TINY); // -0.1 -> clamp to 0
        assert!((out1 - 0.0).abs() < EPS_F32);

        let out2 = agg_weight(&c, WEIGHT_HIGH, WEIGHT_HIGH); // 1.8 -> clamp to 1
        assert!((out2 - 1.0).abs() < EPS_F32);
        finish(
            "test_weight_agg_clamp01_flag",
            t,
            "Clamp flag respected probability bounds.",
        );
    }

    // --------------------------
    // Sign aggregation tests
    // --------------------------

    #[test]
    fn test_sign_agg_prefer_non_neutral() {
        let t = start(
            "test_sign_agg_prefer_non_neutral",
            "Checks PreferNonNeutral resolves ties and conflicts consistently.",
        );
        let c = cfg(WeightAgg::Sum, SignAgg::PreferNonNeutral, false);

        // equal stays equal
        assert_eq!(agg_sign(&c, 1, 1, VOTE_WEAK, VOTE_WEAK), 1);
        assert_eq!(agg_sign(&c, -1, -1, VOTE_WEAK, VOTE_WEAK), -1);
        assert_eq!(agg_sign(&c, 0, 0, VOTE_WEAK, VOTE_WEAK), 0);

        // neutral yields the other
        assert_eq!(agg_sign(&c, 0, 1, VOTE_WEAK, VOTE_WEAK), 1);
        assert_eq!(agg_sign(&c, 1, 0, VOTE_WEAK, VOTE_WEAK), 1);
        assert_eq!(agg_sign(&c, 0, -1, VOTE_WEAK, VOTE_WEAK), -1);

        // conflict -> neutral
        assert_eq!(agg_sign(&c, 1, -1, VOTE_WEAK, VOTE_WEAK), 0);
        assert_eq!(agg_sign(&c, -1, 1, VOTE_WEAK, VOTE_WEAK), 0);
        finish(
            "test_sign_agg_prefer_non_neutral",
            t,
            "PreferNonNeutral favored non-zero inputs and fell back to 0 for conflicts.",
        );
    }

    #[test]
    fn test_sign_agg_weighted_vote() {
        let t = start(
            "test_sign_agg_weighted_vote",
            "Confirms WeightedVote compares weighted sums and handles ties.",
        );
        let c = cfg(WeightAgg::Sum, SignAgg::WeightedVote, false);

        // Positive wins if weighted sum > 0
        assert_eq!(agg_sign(&c, 1, -1, VOTE_STRONG, VOTE_WEAK), 1);

        // Negative wins if weighted sum < 0
        assert_eq!(agg_sign(&c, 1, -1, VOTE_WEAK, VOTE_STRONG), -1);

        // Tie -> 0
        assert_eq!(agg_sign(&c, 1, -1, VOTE_TIE, VOTE_TIE), 0);

        // Neutral treated as 0 in vote
        assert_eq!(agg_sign(&c, 0, 1, VOTE_STRONG, VOTE_WEAK), 1);
        assert_eq!(agg_sign(&c, 0, -1, VOTE_STRONG, VOTE_WEAK), -1);
        finish(
            "test_sign_agg_weighted_vote",
            t,
            "WeightedVote respected neutral/tie semantics.",
        );
    }

    #[test]
    fn test_sign_agg_channels3() {
        let t = start(
            "test_sign_agg_channels3",
            "Validates Channels3 collapses mismatched signs to zero channel.",
        );
        let c = cfg(WeightAgg::Sum, SignAgg::Channels3, false);

        // equal -> itself
        assert_eq!(agg_sign(&c, 1, 1, VOTE_WEAK, VOTE_WEAK), 1);
        assert_eq!(agg_sign(&c, -1, -1, VOTE_WEAK, VOTE_WEAK), -1);
        assert_eq!(agg_sign(&c, 0, 0, VOTE_WEAK, VOTE_WEAK), 0);

        // mismatch -> neutral channel
        assert_eq!(agg_sign(&c, 1, 0, VOTE_WEAK, VOTE_WEAK), 0);
        assert_eq!(agg_sign(&c, 1, -1, VOTE_WEAK, VOTE_WEAK), 0);
        assert_eq!(agg_sign(&c, -1, 0, VOTE_WEAK, VOTE_WEAK), 0);
        finish(
            "test_sign_agg_channels3",
            t,
            "Channels3 forced disagreements into the neutral lane.",
        );
    }

    #[test]
    fn test_agg_sign_works_for_f64_too() {
        let t = start(
            "test_agg_sign_works_for_f64_too",
            "Smoke test to ensure generic agg_sign compiles for f64 weights.",
        );
        let c = cfg(WeightAgg::Sum, SignAgg::WeightedVote, false);
        let s = agg_sign(&c, 1, -1, 0.1_f64, 0.01_f64);
        assert_eq!(s, 1);
        info!("f64 vote result {}", s);
        finish(
            "test_agg_sign_works_for_f64_too",
            t,
            "Generic implementation handled f64 weights.",
        );
    }
}
