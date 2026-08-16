pub const PARSE_DESC_SRC: &str = r#"
        MyDesc
        { }

        A ;
        B ;

        @E1 {
          (+A, -B);
        }
        "#;

pub const MULTI_ARC_DESC_SRC: &str = r#"
        D
        { }

        A ;
        B ;
        C ;

        @E1 {
          (+A, -B );
          (+A, -C );
        }
        "#;

pub const MISSING_SEMI_DESC_SRC: &str = r#"
        D
        { }

        A ;
        B ;

        @E1 {
          +A -B
        }
        "#;

pub const MINIMAL_EXAMPLE_PATH: &str = "../data/minimal_examples/minimal_example.hymeko";
pub const MINIMAL_EXAMPLE_BASE_ELEMENTS_PATH: &str =
    "../data/minimal_examples/minimal_example_base_elements.hymeko";
pub const MINIMAL_WITH_FIELDS_PATH: &str =
    "../data/minimal_examples/minimal_example_with_fields.hymeko";
pub const MINIMAL_BASIC_HIERARCHY_PATH: &str =
    "../data/minimal_examples/minimal_example_basic_hierarchy.hymeko";
pub const MINIMAL_FIELDS_REF_PATH: &str =
    "../data/minimal_examples/minimal_example_fields_with_reference.hymeko";
pub const MINIMAL_FIELDS_REF_ALT_PATH: &str =
    "../data/minimal_examples/minimal_example_fields_with_reference2.hymeko";
pub const MINIMAL_COMMENTS_WITH_FIELDS_PATH: &str =
    "../data/minimal_examples/comments/minimal_example_with_fields_with_comments.hymeko";
pub const MINIMAL_COMMENTS_WITH_LINE_PATH: &str =
    "../data/minimal_examples/comments/minimal_example_with_fields_with_line_comments.hymeko";
pub const MINIMAL_COMMENTS_WITH_HEADER_PATH: &str = "../data/minimal_examples/comments/minimal_example_with_fields_with_block_header_comment.hymeko";
pub const MINIMAL_COMMENTS_WITH_BAD_PATH: &str =
    "../data/minimal_examples/comments/minimal_example_with_fields_with_bad_comments.hymeko";
pub const EDGE_REF_VALUES_PATH: &str = "../data/minimal_examples/testing_edges/minimal_example_with_hierarchy_ref_edges_with_values.hymeko";
pub const TAG_ANNOTATION_PATH: &str =
    "../data/minimal_examples/tag_annotations/minimal_tag_annotation.hymeko";
pub const MULTI_TAG_ANNOTATION_PATH: &str =
    "../data/minimal_examples/tag_annotations/minimal_multi_tag_annotation.hymeko";
pub const MODULE_STORE_ROOT_FILE: &str = "root.hmk";
pub const MODULE_STORE_DEP_FILE: &str = "dep.hmk";
pub const MODULE_STORE_SIMPLE_ROOT_SRC: &str = r#"
        Root {
            @"dep.hmk" -> A;
        }
        // items üres
    "#;
pub const MODULE_STORE_SIMPLE_DEP_SRC: &str = r#"
        Dep { }
    "#;
pub const MODULE_STORE_RESOLVE_ROOT_SRC: &str = r#"
Root { @"dep.hmk" -> A; }

elem {
  B{}
  @E {(+A.Dep.Foo);}
}
"#;
pub const MODULE_STORE_RESOLVE_DEP_SRC: &str = r#"
dep_src{}
Dep { Foo; }
"#;

pub const DESC_MY_DESC: &str = "MyDesc";
pub const DESC_MINIMAL_EXAMPLE_NAME: &str = "Minimal_Example";
pub const CONTEXT_NODE_NAME: &str = "context";
pub const EDGE_E1_NAME: &str = "E1";
pub const NODE_A_NAME: &str = "A";
pub const NODE_B_NAME: &str = "B";
pub const NODE_C_NAME: &str = "C";
pub const NODE_LEVEL0_NAME: &str = "node_lev_0";
pub const NODE_LEVEL1_NAME: &str = "node_lev_1";
pub const NODE0_NAME: &str = "node0";
pub const NODE1_NAME: &str = "node1";
pub const NODE2_NAME: &str = "node2";
pub const NODE10_NAME: &str = "node10";
pub const NODE11_NAME: &str = "node11";
pub const EDGE_E0_NAME: &str = "e0";
pub const MODULE_STORE_ALIAS: &str = "A";
pub const MODULE_STORE_DEP_NAMESPACE: &str = "Dep";
pub const MODULE_STORE_DEP_NODE: &str = "Foo";
pub const SMOKE_NODE_NAME: &str = "n";
pub const SMOKE_EDGE_NAME: &str = "smoke_edge";
pub const SMOKE_ARC_WEIGHT: f64 = 0.2;

pub const BASE_ITEM_COUNT: usize = 3;
pub const MULTI_ARC_COUNT: usize = 2;
pub const MULTI_ARC_EDGE_INDEX: usize = 3;
pub const SINGLE_ARC_COUNT: usize = 1;
pub const ARC_REF_PAIR_COUNT: usize = 2;
pub const BASIC_CONTEXT_CHILD_COUNT: usize = 2;
pub const BASIC_LEVEL0_BODY_NAMES: &[&str] = &["node0", "node1", "node2", "node3"];
pub const BASIC_NODE0_CHILD_COUNT: usize = 1;
pub const BASIC_LEVEL1_BODY_NAMES: &[&str] = &["node0"];

pub const VECTOR_VALUES: &[f64] = &[15.6, -17.8, 16.3, 12.3, 67.8, 45.0, 2.0];
pub const EPS_NUM_CMP: f64 = 1e-9;
pub const VECTOR_VALUES_COMMENTS: &[f64] = &[15.6, 17.8, 16.3, 12.3, 67.8, 45.0, 2.0];
pub const BAD_INLINE_COMMENT_SRC: &str = r#"
        Minimal_Example { author "Csaba"; }
        context {
        /* Multi-line comment without closing tag
        val0 <int> 56;
        }
    "#;
pub const TAG_ISA: &str = "isa";
pub const TAG_IMPL: &str = "impl";

#[derive(Debug)]
pub enum FieldValue {
    Number(f64),
    Str(&'static str),
    NumList(&'static [f64]),
    None,
    Ref(&'static [&'static str]),
}

#[derive(Debug)]
pub struct FieldExpectation {
    pub name: &'static str,
    pub tags: &'static [&'static str],
    pub value: FieldValue,
}

pub const MINIMAL_WITH_FIELDS_EXPECTATIONS: &[FieldExpectation] = &[
    FieldExpectation {
        name: "val0",
        tags: &["int"],
        value: FieldValue::Number(56.0),
    },
    FieldExpectation {
        name: "val1",
        tags: &["string"],
        value: FieldValue::Str("vakond"),
    },
    FieldExpectation {
        name: "val2",
        tags: &["real"],
        value: FieldValue::Number(56.891),
    },
    FieldExpectation {
        name: "val_undef",
        tags: &["real"],
        value: FieldValue::None,
    },
    FieldExpectation {
        name: "val3",
        tags: &[],
        value: FieldValue::Number(3444.4623),
    },
    FieldExpectation {
        name: "pi",
        tags: &[],
        value: FieldValue::Number(3.14156),
    },
    FieldExpectation {
        name: "val_float",
        tags: &[],
        value: FieldValue::None,
    },
    FieldExpectation {
        name: "vector",
        tags: &[],
        value: FieldValue::NumList(VECTOR_VALUES),
    },
];

pub const FIELD_REF_EXPECTATIONS: &[FieldExpectation] = &[
    FieldExpectation {
        name: "val0",
        tags: &["int"],
        value: FieldValue::Number(56.0),
    },
    FieldExpectation {
        name: "val1",
        tags: &["string"],
        value: FieldValue::Str("vakond"),
    },
    FieldExpectation {
        name: "val_node",
        tags: &[],
        value: FieldValue::Ref(&["node", "node0"]),
    },
];

pub const MINIMAL_WITH_FIELDS_COMMENTS_EXPECTATIONS: &[FieldExpectation] = &[
    FieldExpectation {
        name: "val0",
        tags: &["int"],
        value: FieldValue::Number(56.0),
    },
    FieldExpectation {
        name: "val1",
        tags: &["string"],
        value: FieldValue::Str("vakond"),
    },
    FieldExpectation {
        name: "val2",
        tags: &["real"],
        value: FieldValue::Number(56.891),
    },
    FieldExpectation {
        name: "val_undef",
        tags: &["real"],
        value: FieldValue::None,
    },
    FieldExpectation {
        name: "val3",
        tags: &[],
        value: FieldValue::Number(3444.4623),
    },
    FieldExpectation {
        name: "pi",
        tags: &[],
        value: FieldValue::Number(3.14156),
    },
    FieldExpectation {
        name: "val_float",
        tags: &[],
        value: FieldValue::None,
    },
    FieldExpectation {
        name: "vector",
        tags: &[],
        value: FieldValue::NumList(VECTOR_VALUES_COMMENTS),
    },
];

#[derive(Debug)]
pub struct EdgeRefExpectation {
    pub dir: &'static str,
    pub path: &'static [&'static str],
    pub weights: &'static [f64],
}

pub const EDGE_REF_EXPECTATIONS: &[EdgeRefExpectation] = &[
    EdgeRefExpectation {
        dir: "-",
        path: &[NODE0_NAME],
        weights: &[0.85],
    },
    EdgeRefExpectation {
        dir: "+",
        path: &[CONTEXT_NODE_NAME, NODE_LEVEL0_NAME, NODE1_NAME],
        weights: &[0.9],
    },
    EdgeRefExpectation {
        dir: "-",
        path: &[CONTEXT_NODE_NAME, NODE_LEVEL0_NAME, NODE2_NAME],
        weights: &[-0.615],
    },
    EdgeRefExpectation {
        dir: "-",
        path: &[CONTEXT_NODE_NAME, NODE_LEVEL0_NAME, NODE0_NAME],
        weights: &[0.5, 0.6],
    },
];

pub const EDGE_REF_FLAT_WEIGHTS: &[f64] = &[0.85, 0.9, -0.615, 0.5, 0.6];
