#[cfg(test)]
mod ir_value_tests {
    use crate::test_helpers::{log_test_footer, log_test_header};
    use hymeko::common::pathkey::PathKey;
    use hymeko::ir::ir::ValueR;
    use hymeko::ir::lower::lower_to_ir;
    use hymeko::resolution::intern_pass;
    use hymeko::resolution::intern_pass::Interned;
    use hymeko::resolution::resolve::build_index_sym;
    use log::info;
    use std::f64;
    use std::time::Instant;

    const MINIMAL_FIELDS_PATH: &str = "../data/minimal_examples/minimal_example_with_fields.hymeko";
    const SYM_CONTEXT: &str = "context";
    const SYM_VAL0: &str = "val0";
    const SYM_VAL1: &str = "val1";
    const SYM_VECTOR: &str = "vector";
    const SYM_VAL_NEG: &str = "val_neg";
    const SYM_VAL_UNDEF: &str = "val_undef";
    const TAG_INT: &str = "int";
    const TAG_STRING: &str = "string";
    const TAG_REAL: &str = "real";
    const STR_VAKOND: &str = "vakond";
    const VECTOR_EXPECTED_LEN: usize = 7;
    const VECTOR_SECOND_VALUE: f64 = -17.8;
    const EPS_F64: f64 = f64::EPSILON;

    fn start(name: &str, desc: &str) -> Instant {
        log_test_header(name, desc);
        Instant::now()
    }

    fn finish(name: &str, start: Instant, summary: &str) {
        log_test_footer(name, Some(start.elapsed()), summary);
    }

    #[test]
    fn ir_value_resolution() {
        let timer = start(
            "ir_value_resolution",
            "Verifies tag/value lowering for scalars, vectors, and undef nodes.",
        );
        let source_code =
            parser::read_source_file(MINIMAL_FIELDS_PATH).expect("failed to read source file");
        let desc = parser::parse_description(&source_code).unwrap();
        // Intern and resolve as needed, then lower to IR.
        let Interned { ast, mut interner } = intern_pass::intern_ast(&desc);
        let idx = build_index_sym(&ast, &interner).unwrap();
        let ir = lower_to_ir(&ast, &idx, &mut interner).unwrap();

        let sid_context = interner.intern(SYM_CONTEXT);
        let sid_val0 = interner.intern(SYM_VAL0);
        let sid_val1 = interner.intern(SYM_VAL1);
        let sid_vector = interner.intern(SYM_VECTOR);

        let did_context = *idx
            .by_path
            .get(&PathKey(vec![sid_context]))
            .expect("context missing");
        let did_val0 = *idx
            .by_path
            .get(&PathKey(vec![sid_context, sid_val0]))
            .expect("val0 missing");
        let did_val1 = *idx
            .by_path
            .get(&PathKey(vec![sid_context, sid_val1]))
            .expect("val1 missing");
        let did_vector = *idx
            .by_path
            .get(&PathKey(vec![sid_context, sid_vector]))
            .expect("vector missing");

        // tags + value
        let sid_int = interner.intern(TAG_INT);
        assert!(ir.decl_nodes[did_val0.0].anno.tags.contains(&sid_int));
        assert_eq!(
            ir.decl_nodes[did_val0.0].anno.value,
            Some(ValueR::Num(56.0))
        );

        let sid_string = interner.intern(TAG_STRING);
        let sid_vakond = interner.intern(STR_VAKOND);
        assert!(ir.decl_nodes[did_val1.0].anno.tags.contains(&sid_string));
        assert_eq!(
            ir.decl_nodes[did_val1.0].anno.value,
            Some(ValueR::Str(sid_vakond))
        );

        // negative scalar
        let sid_val_neg = interner.intern(SYM_VAL_NEG);
        let did_val_neg = *idx
            .by_path
            .get(&PathKey(vec![sid_context, sid_val_neg]))
            .expect("val_neg missing");
        assert_eq!(
            ir.decl_nodes[did_val_neg.0].anno.value,
            Some(ValueR::Num(-42.0))
        );

        // vector list
        match ir.decl_nodes[did_vector.0]
            .anno
            .value
            .as_ref()
            .expect("vector has no value")
        {
            ValueR::List(xs) => {
                assert_eq!(xs.len(), VECTOR_EXPECTED_LEN);
                if let Some(ValueR::Num(v)) = xs.get(1) {
                    assert!(
                        (*v - VECTOR_SECOND_VALUE).abs() <= EPS_F64,
                        "second element should be {}, got {v}",
                        VECTOR_SECOND_VALUE
                    );
                } else {
                    panic!("second vector element should be numeric");
                }
            }
            other => panic!("vector should be list, got {other:?}"),
        }

        // val_undef: tags ok, value None
        let sid_val_undef = interner.intern(SYM_VAL_UNDEF);
        let sid_real = interner.intern(TAG_REAL);
        let did_val_undef = *idx
            .by_path
            .get(&PathKey(vec![sid_context, sid_val_undef]))
            .expect("val_undef missing");
        assert!(ir.decl_nodes[did_val_undef.0].anno.tags.contains(&sid_real));
        assert!(ir.decl_nodes[did_val_undef.0].anno.value.is_none());

        // sanity: context itself exists
        let _ = did_context;
        info!(
            "Resolved vector len={}, second value ~{:.2}",
            VECTOR_EXPECTED_LEN, VECTOR_SECOND_VALUE
        );
        finish(
            "ir_value_resolution",
            timer,
            "Scalar, vector, and undefined annotations matched the golden fixture.",
        );
    }
}
