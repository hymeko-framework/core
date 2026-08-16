pub const FANO_GRAPH_PATH: &str = "../data/typical_graphs/fano_graph.hymeko";
pub const FANO_DESCRIPTION_NAME: &str = "Fano_graph";
pub const FANO_BLOCK_NAME: &str = "fano";
pub const FANO_NODE_PREFIX: &str = "n";
pub const FANO_EDGE_PREFIX: &str = "e";
pub const FANO_POINT_NODE_COUNT: usize = 7;
pub const FANO_EDGE_COUNT: usize = 7;
pub const FANO_GRAPH_TOTAL_NODES: usize = FANO_POINT_NODE_COUNT + 1; // include neutral/meta node
pub const FANO_BODY_ITEM_COUNT: usize = FANO_POINT_NODE_COUNT + FANO_EDGE_COUNT;
pub const FANO_ARC_REF_COUNT: usize = 3;
pub const FANO_EDGE_DEGREE: usize = FANO_ARC_REF_COUNT;
pub const FANO_NODE_DEGREE: usize = 3;
pub const FANO_INCIDENT_NODE_COUNT: usize = FANO_POINT_NODE_COUNT;
pub const FANO_TOLERANCE: f32 = 1e-6;
pub const FANO_EXPECTED_EDGE_TARGETS: [(&str, [&str; 3]); FANO_EDGE_COUNT] = [
    ("e0", ["n0", "n1", "n3"]),
    ("e1", ["n0", "n2", "n6"]),
    ("e2", ["n0", "n4", "n5"]),
    ("e3", ["n1", "n2", "n4"]),
    ("e4", ["n2", "n3", "n5"]),
    ("e5", ["n3", "n4", "n6"]),
    ("e6", ["n1", "n5", "n6"]),
];
