#[cfg(test)]
mod bench_coo_builder_random {
    use env_logger::Env;
    use hymeko::module_store::module_store::{
        CompiledProgram, HymekoParser, ModuleLoadError, ModuleStore,
    };
    use hymeko::module_store::source_provider::StdFsProvider;
    use hymeko::tensor::aggregation::{AggCfg, SignAgg, WeightAgg};
    use hymeko::tensor::tensor_val::{EdgeWScalar, ScalarWeightExtractor};
    use hymeko_hnn::traversal::hypergraphview::HyperGraphView;
    use hymeko_hre::expansion::star_expansion_coo;
    use log::info;
    use parser::ast::AstStr;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::OnceLock;
    use std::time::Instant;

    const DEFAULT_AGG_CFG: AggCfg = AggCfg {
        weight: WeightAgg::Sum,
        sign: SignAgg::PreferNonNeutral,
        clamp01: false,
    };

    #[derive(Default, Debug, Clone, Copy)]
    struct ParseTiming {
        last_ms: f64,
        total_ms: f64,
        calls: usize,
    }

    static PARSE_TIMING: OnceLock<Mutex<ParseTiming>> = OnceLock::new();

    fn parse_timing_state() -> &'static Mutex<ParseTiming> {
        PARSE_TIMING.get_or_init(|| Mutex::new(ParseTiming::default()))
    }

    fn reset_parse_timing() {
        if let Ok(mut g) = parse_timing_state().lock() {
            *g = ParseTiming::default();
        }
    }

    fn snapshot_parse_timing() -> ParseTiming {
        parse_timing_state().lock().map(|g| *g).unwrap_or_default()
    }

    struct TimedLalrpopParser;

    impl HymekoParser for TimedLalrpopParser {
        fn parse<'a>(&self, src: &'a str) -> Result<AstStr<'a>, String> {
            let t0 = Instant::now();
            let out = parser::parse_description(src).map_err(|e| format!("{e:?}"));
            let elapsed_ms = t0.elapsed().as_secs_f64() * 1_000.0;

            if let Ok(mut g) = parse_timing_state().lock() {
                g.last_ms = elapsed_ms;
                g.total_ms += elapsed_ms;
                g.calls += 1;
            }

            out
        }
    }

    fn load_and_lower(
        path: impl AsRef<Path>,
    ) -> Result<
        (
            ModuleStore<StdFsProvider, TimedLalrpopParser>,
            Arc<CompiledProgram>,
        ),
        ModuleLoadError,
    > {
        let fs = StdFsProvider::new();
        let parser = TimedLalrpopParser;
        let mut store = ModuleStore::new(fs, parser);
        let compiled = store.compile(path.as_ref())?;
        Ok((store, compiled))
    }

    #[derive(Clone, Copy, Debug)]
    struct HypergraphCase {
        nodes: usize,
        edges: usize,
        density: f64,
        repeats: usize,
    }

    #[derive(Debug)]
    struct BenchRow {
        nodes: usize,
        edges: usize,
        density: f64,
        run_idx: usize,
        nnz: usize,
        parse_ms: f64,
        compile_ms: f64,
        view_ms: f64,
        coo_ms: f64,
        total_ms: f64,
        ns_per_entry: f64,
    }

    #[derive(Clone, Copy, Debug)]
    struct SimpleRng {
        state: u64,
    }

    impl SimpleRng {
        fn new(seed: u64) -> Self {
            let s = if seed == 0 { 0x9E3779B97F4A7C15 } else { seed };
            Self { state: s }
        }

        fn next_u64(&mut self) -> u64 {
            let mut x = self.state;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.state = x;
            x.wrapping_mul(0x2545F4914F6CDD1D)
        }

        fn next_f64(&mut self) -> f64 {
            let v = self.next_u64() >> 11;
            (v as f64) * (1.0 / ((1u64 << 53) as f64))
        }

        fn gen_index(&mut self, upper_exclusive: usize) -> usize {
            if upper_exclusive == 0 {
                return 0;
            }
            (self.next_u64() as usize) % upper_exclusive
        }

        fn bernoulli(&mut self, p: f64) -> bool {
            self.next_f64() < p
        }

        fn signed_weight(&mut self) -> (&'static str, f64) {
            let sign = if self.bernoulli(0.5) { "+" } else { "-" };
            let weight = 0.1 + self.next_f64() * 4.9;
            (sign, weight)
        }
    }

    fn input_root() -> PathBuf {
        let mut p = PathBuf::from("target");
        p.push("benchmarks");
        p.push("random_hymeko_inputs");
        p
    }

    fn csv_output_path() -> PathBuf {
        let mut p = PathBuf::from("target");
        p.push("benchmarks");
        p.push("coo_builder_random_benchmark.csv");
        p
    }

    fn build_random_hymeko_source(nodes: usize, edges: usize, density: f64, seed: u64) -> String {
        let mut rng = SimpleRng::new(seed);
        let mut out = String::new();
        out.push_str("BenchmarkCase\n{}\ncontext\n{\n");

        for n in 0..nodes {
            out.push_str(&format!("    node{}{{}}\n", n));
        }

        for e in 0..edges {
            let mut refs = Vec::new();
            for n in 0..nodes {
                if rng.bernoulli(density) {
                    let (sign, w) = rng.signed_weight();
                    refs.push(format!("{} node{}[{:.4}]", sign, n, w));
                }
            }

            if refs.is_empty() {
                let n = rng.gen_index(nodes.max(1));
                let (sign, w) = rng.signed_weight();
                refs.push(format!("{} node{}[{:.4}]", sign, n, w));
            }

            out.push_str(&format!("    @e{} {{({});}}\n", e, refs.join(", ")));
        }

        out.push_str("}\n");
        out
    }

    fn write_case_source(
        nodes: usize,
        edges: usize,
        density: f64,
        run_idx: usize,
        seed: u64,
    ) -> PathBuf {
        let mut path = input_root();
        let _ = fs::create_dir_all(&path);
        path.push(format!(
            "case_n{}_e{}_d{:03}_r{}_s{}.hymeko",
            nodes,
            edges,
            (density * 100.0).round() as usize,
            run_idx,
            seed
        ));

        let source = build_random_hymeko_source(nodes, edges, density, seed);
        fs::write(&path, source).expect("failed to write generated benchmark input");
        path
    }

    static BENCH_LOGGER: OnceLock<()> = OnceLock::new();

    fn init_bench_logger() {
        BENCH_LOGGER.get_or_init(|| {
            let _ = env_logger::Builder::from_env(Env::default().default_filter_or("info"))
                .is_test(true)
                .try_init();
        });
    }

    fn parser_backend_hint() -> &'static str {
        #[cfg(target_arch = "x86_64")]
        {
            if std::is_x86_feature_detected!("avx2") {
                return "avx2";
            }
            if std::is_x86_feature_detected!("sse2") {
                return "sse2";
            }
        }
        "scalar"
    }

    fn benchmark_case(case: HypergraphCase, seed_base: u64) -> Vec<BenchRow> {
        init_bench_logger();
        let mut rows = Vec::with_capacity(case.repeats);

        info!(
            "[bench] parser backend hint (via parser::parse_description auto-detect): {}",
            parser_backend_hint()
        );
        info!(
            "[bench] case start: nodes={}, edges={}, density={:.4}, repeats={}",
            case.nodes, case.edges, case.density, case.repeats
        );

        for run_idx in 0..case.repeats {
            let seed = seed_base
                ^ ((case.nodes as u64) << 32)
                ^ ((case.edges as u64) << 16)
                ^ (run_idx as u64);

            let input_path =
                write_case_source(case.nodes, case.edges, case.density, run_idx + 1, seed);
            info!(
                "[bench] run {}/{} source generated: {}",
                run_idx + 1,
                case.repeats,
                input_path.display()
            );
            info!(
                "[bench] run {}/{} compiling module...",
                run_idx + 1,
                case.repeats
            );
            reset_parse_timing();
            let t_compile = Instant::now();
            let (_store, compiled) =
                load_and_lower(&input_path).expect("module pipeline compile failed");
            let compile_ms = t_compile.elapsed().as_secs_f64() * 1_000.0;
            let parse_timing = snapshot_parse_timing();
            let parse_ms = parse_timing.total_ms;
            info!(
                "[bench] run {}/{} compiled: decls={}, nodes={}, edges={}, arcs={}, parse_ms={:.3}, parse_calls={}, compile_ms={:.3}",
                run_idx + 1,
                case.repeats,
                compiled.ir.decl_nodes.len(),
                compiled.ir.nodes.len(),
                compiled.ir.edges.len(),
                compiled.ir.arcs.len(),
                parse_ms,
                parse_timing.calls,
                compile_ms
            );

            info!(
                "[bench] run {}/{} building HyperGraphView...",
                run_idx + 1,
                case.repeats
            );
            let t_view = Instant::now();
            let ex = ScalarWeightExtractor::default();
            let hg = HyperGraphView::<f32, EdgeWScalar<f32>, f32>::from_ir(
                &compiled.ir,
                &DEFAULT_AGG_CFG,
                &ex,
            );
            let view_ms = t_view.elapsed().as_secs_f64() * 1_000.0;
            info!(
                "[bench] run {}/{} HyperGraphView ready: nodes={}, edges={}, flat_node_edges={}, flat_edge_nodes={}, view_ms={:.3}",
                run_idx + 1,
                case.repeats,
                hg.num_nodes(),
                hg.num_edges(),
                hg.flat_node_edges.len(),
                hg.flat_edge_nodes.len(),
                view_ms
            );

            info!(
                "[bench] run {}/{} building star COO + SOA...",
                run_idx + 1,
                case.repeats
            );
            let t_coo = Instant::now();
            let coo = star_expansion_coo(&hg);
            let nnz = coo.len();
            let num_slices = coo.num_slices;
            let dim_i = coo.dim_i;
            let dim_j = coo.dim_j;
            let _soa = coo.into_soa();
            let coo_ms = t_coo.elapsed().as_secs_f64() * 1_000.0;
            info!(
                "[bench] run {}/{} COO ready: slices={}, dim={}x{}, nnz={}, coo_ms={:.3}",
                run_idx + 1,
                case.repeats,
                num_slices,
                dim_i,
                dim_j,
                nnz,
                coo_ms
            );

            let total_ms = compile_ms + view_ms + coo_ms;
            let ns_per_entry = if nnz == 0 {
                0.0
            } else {
                (total_ms * 1_000_000.0) / (nnz as f64)
            };

            info!(
                "[bench] run {}/{} done: decls={}, nodes={}, edges={}, arcs={}, nnz={}, parse_ms={:.3}, compile_ms={:.3}, view_ms={:.3}, coo_ms={:.3}, total_ms={:.3}, ns_per_entry={:.3}",
                run_idx + 1,
                case.repeats,
                compiled.ir.decl_nodes.len(),
                compiled.ir.nodes.len(),
                compiled.ir.edges.len(),
                compiled.ir.arcs.len(),
                nnz,
                parse_ms,
                compile_ms,
                view_ms,
                coo_ms,
                total_ms,
                ns_per_entry
            );

            rows.push(BenchRow {
                nodes: case.nodes,
                edges: case.edges,
                density: case.density,
                run_idx: run_idx + 1,
                nnz,
                parse_ms,
                compile_ms,
                view_ms,
                coo_ms,
                total_ms,
                ns_per_entry,
            });
        }

        info!(
            "[bench] case complete: nodes={}, edges={}, density={:.4}, rows={}",
            case.nodes,
            case.edges,
            case.density,
            rows.len()
        );

        rows
    }

    fn write_csv(rows: &[BenchRow]) {
        let path = csv_output_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let header = "nodes,edges,density,run_idx,nnz,parse_ms,compile_ms,view_ms,coo_ms,total_ms,ns_per_entry";
        info!("[bench] preparing CSV serialization: rows={}", rows.len());
        let body = rows
            .iter()
            .map(|r| {
                format!(
                    "{},{},{:.4},{},{},{:.6},{:.6},{:.6},{:.6},{:.6},{:.3}",
                    r.nodes,
                    r.edges,
                    r.density,
                    r.run_idx,
                    r.nnz,
                    r.parse_ms,
                    r.compile_ms,
                    r.view_ms,
                    r.coo_ms,
                    r.total_ms,
                    r.ns_per_entry
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        let payload = if body.is_empty() {
            format!("{}\n", header)
        } else {
            format!("{}\n{}\n", header, body)
        };

        info!("[bench] writing CSV payload to {}", path.display());
        fs::write(&path, payload).expect("failed to write COO random benchmark CSV");
        info!(
            "[bench] wrote CSV: {} (rows={})",
            path.display(),
            rows.len()
        );
    }

    #[test]
    fn random_hypergraph_coo_builder_smoke() {
        init_bench_logger();
        info!(
            "[bench] smoke uses SIMD-aware parser::parse_description, backend hint={}.",
            parser_backend_hint()
        );
        let case = HypergraphCase {
            nodes: 16,
            edges: 8,
            density: 0.10,
            repeats: 1,
        };
        let rows = benchmark_case(case, 0xDEADBEEFCAFEBABE);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].nnz > 0, "smoke benchmark produced empty COO");
        assert!(rows[0].total_ms >= 0.0);
    }

    #[test]
    fn bench_random_hypergraph_coo_builder_suite() {
        init_bench_logger();
        info!(
            "[bench] suite uses SIMD-aware parser::parse_description, backend hint={}.",
            parser_backend_hint()
        );
        let cases = [
            HypergraphCase {
                nodes: 64,
                edges: 32,
                density: 0.01,
                repeats: 3,
            },
            HypergraphCase {
                nodes: 64,
                edges: 32,
                density: 0.05,
                repeats: 3,
            },
            HypergraphCase {
                nodes: 64,
                edges: 32,
                density: 0.20,
                repeats: 3,
            },
            HypergraphCase {
                nodes: 256,
                edges: 128,
                density: 0.01,
                repeats: 3,
            },
            HypergraphCase {
                nodes: 256,
                edges: 128,
                density: 0.05,
                repeats: 3,
            },
            HypergraphCase {
                nodes: 256,
                edges: 128,
                density: 0.20,
                repeats: 3,
            },
            HypergraphCase {
                nodes: 1024,
                edges: 512,
                density: 0.01,
                repeats: 2,
            },
            HypergraphCase {
                nodes: 1024,
                edges: 512,
                density: 0.05,
                repeats: 2,
            },
            HypergraphCase {
                nodes: 1024,
                edges: 512,
                density: 0.20,
                repeats: 2,
            },
            HypergraphCase {
                nodes: 2048,
                edges: 1024,
                density: 0.01,
                repeats: 2,
            },
            HypergraphCase {
                nodes: 2048,
                edges: 1024,
                density: 0.05,
                repeats: 2,
            },
            /*
            HypergraphCase { nodes: 4096, edges: 2048, density: 0.01, repeats: 2 },
            HypergraphCase { nodes: 4096, edges: 2048, density: 0.05, repeats: 2 },

            // Requested mid-step kept explicit.
            HypergraphCase { nodes: 5192, edges: 2596, density: 0.01, repeats: 2 },
            HypergraphCase { nodes: 5192, edges: 2596, density: 0.05, repeats: 2 },


            HypergraphCase { nodes: 8192, edges: 4096, density: 0.01, repeats: 2 },
            HypergraphCase { nodes: 8192, edges: 4096, density: 0.05, repeats: 2 },

            HypergraphCase { nodes: 16384, edges: 8192, density: 0.01, repeats: 1 },
            HypergraphCase { nodes: 16384, edges: 8192, density: 0.05, repeats: 1 },

            HypergraphCase { nodes: 32768, edges: 16384, density: 0.005, repeats: 1 },
            HypergraphCase { nodes: 32768, edges: 16384, density: 0.01, repeats: 1 },

            HypergraphCase { nodes: 65536, edges: 32768, density: 0.001, repeats: 1 },
            HypergraphCase { nodes: 65536, edges: 32768, density: 0.005, repeats: 1 },

            HypergraphCase { nodes: 100_000, edges: 50_000, density: 0.001, repeats: 1 },
            */
        ];

        let mut all_rows = Vec::new();
        info!("[bench] suite start: total_cases={}", cases.len());
        for (idx, case) in cases.iter().enumerate() {
            let seed_base = 0xA5A5A5A500000000u64 ^ (idx as u64);
            info!(
                "[bench] suite dispatch case {}/{}: nodes={}, edges={}, density={:.4}, repeats={}, seed_base={}",
                idx + 1,
                cases.len(),
                case.nodes,
                case.edges,
                case.density,
                case.repeats,
                seed_base
            );
            let mut rows = benchmark_case(*case, seed_base);
            all_rows.append(&mut rows);
        }

        write_csv(&all_rows);

        info!(
            "[bench] suite complete: cases={}, total_rows={}",
            cases.len(),
            all_rows.len()
        );

        assert!(!all_rows.is_empty(), "benchmark produced no rows");
        assert!(
            all_rows.iter().all(|r| r.nnz > 0),
            "at least one case produced zero nnz"
        );
    }
}
