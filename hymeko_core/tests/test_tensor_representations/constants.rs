use hymeko::tensor::aggregation::{AggCfg, SignAgg, WeightAgg};

pub const LINEAR_EDGE_VALUES_PATH: &str =
    "../data/minimal_examples/testing_edges/linear_edge_values.hymeko";
pub const MINIMAL_TENSOR_VALUES_PATH: &str =
    "../data/minimal_examples/testing_edges/minimal_test_tensor_values_2nodes_1_edge.hymeko";
pub const FANO_GRAPH_PATH: &str = "../data/typical_graphs/fano_graph.hymeko";

pub const STAR_NODE_COUNT: usize = 6;
pub const STAR_EDGE_COUNT: usize = 4;
pub const STAR_EDGE_BASE: usize = STAR_NODE_COUNT;
pub const STAR_EXPECTED_DIM: usize = STAR_NODE_COUNT + STAR_EDGE_COUNT;

pub const EPS_F32_DEFAULT: f32 = 1e-4;
pub const EPS_F32_STRICT: f32 = 1e-5;
pub const EPS_F32_ULTRA: f32 = 1e-6;
pub const EPS_F64_DEFAULT: f64 = 1e-6;

pub const STAR_NORMALIZATION_EPS: f32 = 1e-12;
pub const INCIDENCE_SCALE_FACTOR: f32 = 10.0;
pub const SCATTER_SENTINEL: f32 = 999.0;

pub const DEFAULT_AGG_CFG: AggCfg = AggCfg {
    weight: WeightAgg::Sum,
    sign: SignAgg::PreferNonNeutral,
    clamp01: false,
};
