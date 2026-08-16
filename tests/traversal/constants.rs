use hymeko::tensor::aggregation::{AggCfg, SignAgg, WeightAgg};

pub const BERGE_SRC: &str = r#"
        berge_demo {}

        D {
            A {}
            B {}
            Root {
                @E { (+A, +B); }
            }
        }
        "#;
pub const NAMESPACE_D: &str = "D";
pub const NODE_A: &str = "A";
pub const NODE_B: &str = "B";
pub const ROOT_NAME: &str = "Root";
pub const EDGE_E: &str = "E";
pub const AGG_CFG: AggCfg = AggCfg {
    weight: WeightAgg::Sum,
    sign: SignAgg::PreferNonNeutral,
    clamp01: false,
};
