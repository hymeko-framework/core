#[cfg(test)]
mod test_parse_files {
    use crate::test_helpers::{find_decl, load_and_lower, log_test_footer, log_test_header};
    use hymeko::common::ids::DeclId;
    use hymeko::ir::ir::DeclKind;
    use hymeko::tensor::aggregation::{AggCfg, SignAgg, WeightAgg};
    use hymeko::tensor::tensor_val::{EdgeWScalar, ScalarWeightExtractor};
    use hymeko_hnn::tensor::tensor::compute_bipartite_degrees;
    use hymeko_hnn::traversal::hypergraphview::HyperGraphView;
    use log::info;
    use std::time::Instant;

    const META_KINEMATICS_PATH: &str = "../data/robotics/meta_kinematics.hymeko";
    const AGG_WEIGHT_SUM: WeightAgg = WeightAgg::Sum;
    const AGG_SIGN_NON_NEUTRAL: SignAgg = SignAgg::PreferNonNeutral;
    const CLAMP_DISABLED: bool = false;
    const USE_ABS_DEFAULT: bool = true;
    const META_EXPECTED_NODE_COUNT: usize = 57;
    const META_EXPECTED_EDGE_COUNT: usize = 12;
    const EPS_F32: f32 = 1e-6;
    const CONTROLLER_DECLS: [&str; 5] = [
        "joint_trajectory_controller",
        "diff_drive_controller",
        "force_torque_sensor_controller",
        "forward_position_controller",
        "forward_velocity_controller",
    ];
    const AXIS_DECLS: [&str; 4] = ["AXIS_X", "AXIS_Y", "AXIS_Z", "AXIS_M_Z"];
    const SENSOR_DECLS: [&str; 2] = ["rgb_camera", "laser_scanner"];
    const PASSIVE_SENSOR_DECLS: [&str; 1] = ["joint_state_broadcaster"];

    fn start(name: &str, desc: &str) -> Instant {
        log_test_header(name, desc);
        Instant::now()
    }

    fn finish(name: &str, start: Instant, summary: &str) {
        log_test_footer(name, Some(start.elapsed()), summary);
    }

    #[test]
    fn test_load_meta_geometry() {
        let timer = start(
            "test_load_meta_geometry",
            "Loads the robotics meta kinematics file and validates inheritance + degrees.",
        );
        let (store, compiled) = load_and_lower(META_KINEMATICS_PATH).unwrap();

        let aggcfg = AggCfg {
            weight: AGG_WEIGHT_SUM,
            sign: AGG_SIGN_NON_NEUTRAL,
            clamp01: CLAMP_DISABLED,
        };
        let ex = ScalarWeightExtractor::default();
        let hg = HyperGraphView::<f32, EdgeWScalar<f32>, f32>::from_ir(&compiled.ir, &aggcfg, &ex);

        let node_count = hg.num_nodes();
        let edge_count = hg.num_edges();
        assert_eq!(
            node_count, META_EXPECTED_NODE_COUNT,
            "meta kinematics node count drifted"
        );
        assert_eq!(
            edge_count, META_EXPECTED_EDGE_COUNT,
            "meta kinematics edge count drifted"
        );

        let (deg_v, deg_e) = compute_bipartite_degrees(&hg, USE_ABS_DEFAULT);
        assert_eq!(deg_v.len(), node_count, "node degree slice mismatch");
        assert_eq!(deg_e.len(), edge_count, "edge degree slice mismatch");

        let sum_v: f32 = deg_v.iter().copied().sum();
        let sum_e: f32 = deg_e.iter().copied().sum();
        assert!(
            (sum_v - sum_e).abs() <= EPS_F32,
            "bipartite degree mass mismatch: nodes={} edges={}",
            sum_v,
            sum_e
        );

        let it = &store.it;
        let has_base = |decl: DeclId, expected: DeclId| -> bool {
            let nid = compiled
                .ir
                .as_node(decl)
                .expect("decl should lower to node");
            compiled.ir.nodes[nid.0].bases.iter().any(|r| match r {
                hymeko::ir::ir::SignedRefR::Plus(atom)
                | hymeko::ir::ir::SignedRefR::Minus(atom)
                | hymeko::ir::ir::SignedRefR::Neutral(atom) => atom.target == expected,
            })
        };

        let controller_base = find_decl(&compiled.ir, it, "meta_controller", DeclKind::Node);
        for name in CONTROLLER_DECLS {
            let decl = find_decl(&compiled.ir, it, name, DeclKind::Node);
            assert!(
                has_base(decl, controller_base),
                "controller `{}` no longer inherits meta_controller",
                name
            );
        }

        let axis_base = find_decl(&compiled.ir, it, "axis_definition", DeclKind::Node);
        for name in AXIS_DECLS {
            let decl = find_decl(&compiled.ir, it, name, DeclKind::Node);
            assert!(
                has_base(decl, axis_base),
                "axis `{}` no longer inherits axis_definition",
                name
            );
        }

        let sensor_base = find_decl(&compiled.ir, it, "sensor", DeclKind::Node);
        for name in SENSOR_DECLS {
            let decl = find_decl(&compiled.ir, it, name, DeclKind::Node);
            assert!(
                has_base(decl, sensor_base),
                "sensor `{}` no longer inherits sensor",
                name
            );
        }

        for name in PASSIVE_SENSOR_DECLS {
            let decl = find_decl(&compiled.ir, it, name, DeclKind::Node);
            let nid = compiled
                .ir
                .as_node(decl)
                .expect("decl should lower to node");
            assert!(
                compiled.ir.nodes[nid.0].bases.is_empty(),
                "passive sensor `{}` unexpectedly gained inheritance",
                name
            );
        }
        info!(
            "Meta geometry: nodes={} edges={} controllers={}, axes={}, sensors={} passive={}",
            node_count,
            edge_count,
            CONTROLLER_DECLS.len(),
            AXIS_DECLS.len(),
            SENSOR_DECLS.len(),
            PASSIVE_SENSOR_DECLS.len()
        );
        finish(
            "test_load_meta_geometry",
            timer,
            "Meta kinematics degrees and inheritance chains remained stable.",
        );
    }
}
