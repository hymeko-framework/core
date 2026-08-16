//! Canonical combinatorial hypergraph generators.
//!
//! Dependency-free, index-level constructions of classic hypergraph families,
//! produced as a [`HypergraphDesign`] (vertex count + edges as member-index
//! lists). This is the single home for the generation *algorithm*; higher
//! layers (e.g. `hymeko_hive`) adapt a [`HypergraphDesign`] into their own node
//! and relation types rather than re-implementing it.
//!
//! Families:
//! * Steiner triple systems S(2,3,n) — canonical Fano (`n=7`), closed-form Bose
//!   construction (`n ≡ 3 mod 6`), most-constrained-pair backtracking
//!   (`n ≡ 1 mod 6`).
//! * Sunflower / Δ-systems — `petals` edges sharing a common `core`.
//! * Complete `r`-uniform K_n^(r) — every size-`r` subset of `n` vertices.
//!
//! The mirror of these generators in the web editor
//! (`docs/editor/views/generators.js`) is validated by the same "every pair
//! covered once" / "C(n,r) distinct edges" properties.

/// An index-level hypergraph: `n_vertices` vertices labelled `0..n_vertices`,
/// and `edges`, each a list of member-vertex indices.
///
/// # Invariants
/// Every index in every edge is `< n_vertices`. Edges are emitted in a
/// deterministic order per family.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HypergraphDesign {
    pub n_vertices: usize,
    pub edges: Vec<Vec<usize>>,
}

impl HypergraphDesign {
    /// Number of (hyper)edges.
    #[must_use]
    pub fn n_edges(&self) -> usize {
        self.edges.len()
    }
}

/// Error for an out-of-domain generator request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GeneratorError {
    /// `n` is not a valid Steiner triple order (`n ≡ 1 or 3 mod 6`, `n ≥ 7`).
    InvalidSteinerOrder(usize),
    /// The backtracking search hit its step cap (unreachable for valid `n`).
    SteinerSearchExhausted(usize),
    /// Sunflower parameters out of range (`petals ≥ 1`, `petal ≥ 1`).
    InvalidSunflower {
        petals: usize,
        core: usize,
        petal: usize,
    },
    /// Complete-uniform arity out of range (`2 ≤ r ≤ n`).
    InvalidComplete { n: usize, r: usize },
}

impl std::fmt::Display for GeneratorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSteinerOrder(n) => write!(
                f,
                "Steiner triple system S(2,3,n) needs n >= 7 and n = 1 or 3 (mod 6); got {n}"
            ),
            Self::SteinerSearchExhausted(n) => write!(
                f,
                "Steiner triple search for n={n} exceeded its step cap (no system found)"
            ),
            Self::InvalidSunflower {
                petals,
                core,
                petal,
            } => write!(
                f,
                "sunflower needs petals >= 1 and petal >= 1; got petals={petals}, core={core}, petal={petal}"
            ),
            Self::InvalidComplete { n, r } => write!(
                f,
                "complete r-uniform K_n^(r) needs 2 <= r <= n; got n={n}, r={r}"
            ),
        }
    }
}

impl std::error::Error for GeneratorError {}

/// Typed selector over the generator families. Validation lives in
/// [`HypergraphGenerator::design`], so an out-of-range request fails at the
/// boundary rather than inside a builder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HypergraphGenerator {
    /// Steiner triple system S(2,3,`n`); needs `n` = 1 or 3 (mod 6), `n >= 7`.
    SteinerTriple { n: usize },
    /// Sunflower / Δ-system with `petals` petals sharing a `core`-sized core.
    Sunflower {
        petals: usize,
        core: usize,
        petal: usize,
    },
    /// Complete `r`-uniform hypergraph K_n^(r): every size-`r` subset of `n`.
    CompleteUniform { n: usize, r: usize },
}

impl HypergraphGenerator {
    /// Dispatch to the matching builder.
    ///
    /// # Errors
    /// Returns the variant-specific [`GeneratorError`] when the parameters are
    /// out of the family's valid domain.
    pub fn design(&self) -> Result<HypergraphDesign, GeneratorError> {
        match *self {
            Self::SteinerTriple { n } => steiner_design(n),
            Self::Sunflower {
                petals,
                core,
                petal,
            } => sunflower_design(petals, core, petal),
            Self::CompleteUniform { n, r } => complete_uniform_design(n, r),
        }
    }
}

// ---------------------------------------------------------------------------
// Steiner triple systems S(2,3,n)
// ---------------------------------------------------------------------------

/// Triples of a Steiner triple system on `n` points.
///
/// # Preconditions
/// `n >= 7` and `n ≡ 1 or 3 (mod 6)`.
///
/// # Postconditions
/// Returns `n(n-1)/6` triples; every unordered pair of points appears in
/// exactly one triple.
///
/// # Errors
/// [`GeneratorError::InvalidSteinerOrder`] for an invalid `n`;
/// [`GeneratorError::SteinerSearchExhausted`] if the backtracking search hits
/// its step cap (unreachable for the offered orders).
pub fn steiner_triples(n: usize) -> Result<Vec<[usize; 3]>, GeneratorError> {
    // Existence: a Steiner triple system S(2,3,n) exists iff n = 1 or 3 (mod 6).
    if n < 7 || !(n % 6 == 1 || n % 6 == 3) {
        return Err(GeneratorError::InvalidSteinerOrder(n));
    }
    if n == 7 {
        // Canonical Fano plane (matches data/typical_graphs fixtures).
        return Ok(vec![
            [0, 1, 3],
            [0, 2, 6],
            [0, 4, 5],
            [1, 2, 4],
            [2, 3, 5],
            [3, 4, 6],
            [1, 5, 6],
        ]);
    }
    if n % 6 == 3 {
        // n = 3v with v odd: closed-form Bose construction, no search.
        return Ok(bose_triples(n));
    }
    // n = 1 (mod 6), n >= 13: deterministic MRV backtracking.
    sts_backtrack(n)
}

/// Steiner triple system as a [`HypergraphDesign`] (arity-3 edges).
///
/// # Errors
/// As [`steiner_triples`].
pub fn steiner_design(n: usize) -> Result<HypergraphDesign, GeneratorError> {
    let triples = steiner_triples(n)?;
    let edges: Vec<Vec<usize>> = triples.into_iter().map(|t| t.to_vec()).collect();
    debug_assert!(
        every_pair_covered_once(n, &edges),
        "STS postcondition violated for n={n}"
    );
    Ok(HypergraphDesign {
        n_vertices: n,
        edges,
    })
}

fn bose_triples(n: usize) -> Vec<[usize; 3]> {
    let v = n / 3;
    let half = v.div_ceil(2); // multiplicative inverse of 2 (mod v), v odd
    let op = |a: usize, b: usize| (half * ((a + b) % v)) % v;
    let point = |layer: usize, i: usize| layer * v + i;
    let mut triples = Vec::with_capacity(n * (n - 1) / 6);

    for i in 0..v {
        triples.push([point(0, i), point(1, i), point(2, i)]);
    }
    for i in 0..v {
        for j in (i + 1)..v {
            let m = op(i, j);
            for layer in 0..3 {
                triples.push([point(layer, i), point(layer, j), point((layer + 1) % 3, m)]);
            }
        }
    }
    triples
}

/// Backtracking state for the `n = 1 (mod 6)` Steiner case.
///
/// Most-constrained-pair (MRV) selection with forward checking: always extend
/// the uncovered pair with the fewest legal third points, bailing immediately on
/// a pair with zero. A system always exists for valid `n`, so the search is
/// effectively backtrack-free at the offered orders; the step cap turns any
/// pathological larger `n` into a clean error rather than a hang.
struct SteinerSearch {
    n: usize,
    /// `used[min*n + max]` marks an already-covered unordered pair.
    used: Vec<bool>,
    triples: Vec<[usize; 3]>,
    steps: u64,
}

impl SteinerSearch {
    const STEP_CAP: u64 = 2_000_000;

    fn new(n: usize) -> Self {
        Self {
            n,
            used: vec![false; n * n],
            triples: Vec::with_capacity(n * (n - 1) / 6),
            steps: 0,
        }
    }

    #[inline]
    fn key(&self, a: usize, b: usize) -> usize {
        if a < b {
            a * self.n + b
        } else {
            b * self.n + a
        }
    }

    fn completions(&self, a: usize, b: usize) -> Vec<usize> {
        (0..self.n)
            .filter(|&c| {
                c != a && c != b && !self.used[self.key(a, c)] && !self.used[self.key(b, c)]
            })
            .collect()
    }

    fn pick_pair(&self) -> Option<([usize; 2], Vec<usize>)> {
        let mut best: Option<([usize; 2], Vec<usize>)> = None;
        let mut best_count = usize::MAX;
        for a in 0..self.n {
            for b in (a + 1)..self.n {
                if self.used[a * self.n + b] {
                    continue;
                }
                let list = self.completions(a, b);
                if list.len() < best_count {
                    best_count = list.len();
                    best = Some(([a, b], list));
                    if best_count <= 1 {
                        return best; // forced or dead end — stop searching
                    }
                }
            }
        }
        best
    }

    fn solve(&mut self) -> Result<bool, GeneratorError> {
        self.steps += 1;
        if self.steps > Self::STEP_CAP {
            return Err(GeneratorError::SteinerSearchExhausted(self.n));
        }
        let Some(([a, b], list)) = self.pick_pair() else {
            return Ok(true); // all pairs covered
        };
        if list.is_empty() {
            return Ok(false); // dead end — backtrack
        }
        for c in list {
            let (kab, kac, kbc) = (self.key(a, b), self.key(a, c), self.key(b, c));
            self.used[kab] = true;
            self.used[kac] = true;
            self.used[kbc] = true;
            self.triples.push([a, b, c]);
            if self.solve()? {
                return Ok(true);
            }
            self.triples.pop();
            self.used[kab] = false;
            self.used[kac] = false;
            self.used[kbc] = false;
        }
        Ok(false)
    }
}

fn sts_backtrack(n: usize) -> Result<Vec<[usize; 3]>, GeneratorError> {
    let mut search = SteinerSearch::new(n);
    if search.solve()? {
        Ok(search.triples)
    } else {
        // Unreachable for a valid n (a system always exists), but explicit.
        Err(GeneratorError::SteinerSearchExhausted(n))
    }
}

// ---------------------------------------------------------------------------
// Sunflower / Δ-system
// ---------------------------------------------------------------------------

/// Sunflower (Δ-system) as a [`HypergraphDesign`].
///
/// # Index layout (contract relied on by adapters)
/// Vertices `0..core` are the shared core points. For petal `p` in `0..petals`,
/// vertices `core + p*petal .. core + (p+1)*petal` are that petal's private
/// points. Edge `p` is the core followed by petal `p`'s private points, so any
/// two edges intersect in exactly the core.
///
/// # Preconditions
/// `petals >= 1` and `petal >= 1`.
///
/// # Errors
/// [`GeneratorError::InvalidSunflower`] otherwise.
pub fn sunflower_design(
    petals: usize,
    core: usize,
    petal: usize,
) -> Result<HypergraphDesign, GeneratorError> {
    if petals == 0 || petal == 0 {
        return Err(GeneratorError::InvalidSunflower {
            petals,
            core,
            petal,
        });
    }
    let n_vertices = core + petals * petal;
    let core_ids: Vec<usize> = (0..core).collect();
    let edges: Vec<Vec<usize>> = (0..petals)
        .map(|p| {
            let mut members = core_ids.clone();
            let base = core + p * petal;
            members.extend(base..base + petal);
            members
        })
        .collect();
    Ok(HypergraphDesign { n_vertices, edges })
}

// ---------------------------------------------------------------------------
// Complete r-uniform K_n^(r)
// ---------------------------------------------------------------------------

/// Complete `r`-uniform hypergraph K_n^(r) as a [`HypergraphDesign`]: every
/// size-`r` subset of `n` vertices.
///
/// # Preconditions
/// `2 <= r <= n`.
///
/// # Postconditions
/// `C(n,r)` distinct edges, each of arity `r`.
///
/// # Errors
/// [`GeneratorError::InvalidComplete`] when `r` is outside `2..=n`.
pub fn complete_uniform_design(n: usize, r: usize) -> Result<HypergraphDesign, GeneratorError> {
    if !(2..=n).contains(&r) {
        return Err(GeneratorError::InvalidComplete { n, r });
    }
    let edges = combinations(n, r);
    debug_assert_eq!(edges.len() as u128, binom(n, r));
    Ok(HypergraphDesign {
        n_vertices: n,
        edges,
    })
}

// ---------------------------------------------------------------------------
// Combinatorial helpers
// ---------------------------------------------------------------------------

/// Exact binomial coefficient C(n, r). Returns 0 for `r > n`.
#[must_use]
pub fn binom(n: usize, r: usize) -> u128 {
    if r > n {
        return 0;
    }
    let r = r.min(n - r);
    let mut c: u128 = 1;
    for i in 0..r {
        // Each partial product is itself a binomial coefficient, so the integer
        // division is always exact.
        c = c * (n - i) as u128 / (i as u128 + 1);
    }
    c
}

/// All `r`-subsets of `{0,…,n-1}` as ascending index vectors, lexicographic
/// order.
#[must_use]
pub fn combinations(n: usize, r: usize) -> Vec<Vec<usize>> {
    if r == 0 {
        return vec![Vec::new()];
    }
    if r > n {
        return Vec::new();
    }
    let mut out: Vec<Vec<usize>> = Vec::with_capacity(binom(n, r) as usize);
    let mut idx: Vec<usize> = (0..r).collect();
    loop {
        out.push(idx.clone());
        // Find the rightmost index that can still be incremented.
        let mut pivot = None;
        for i in (0..r).rev() {
            if idx[i] != n - r + i {
                pivot = Some(i);
                break;
            }
        }
        let Some(i) = pivot else {
            break; // last combination reached
        };
        idx[i] += 1;
        for j in (i + 1)..r {
            idx[j] = idx[j - 1] + 1;
        }
    }
    out
}

/// True iff every unordered vertex pair is covered by exactly one edge.
/// Used as a `debug_assert` postcondition for Steiner designs.
fn every_pair_covered_once(n: usize, edges: &[Vec<usize>]) -> bool {
    let mut count = vec![0u32; n * n];
    for edge in edges {
        for i in 0..edge.len() {
            for j in (i + 1)..edge.len() {
                let (a, b) = (edge[i].min(edge[j]), edge[i].max(edge[j]));
                count[a * n + b] += 1;
            }
        }
    }
    let expected_pairs = n * (n - 1) / 2;
    let covered = count.iter().filter(|&&c| c == 1).count();
    let over = count.iter().any(|&c| c > 1);
    covered == expected_pairs && !over
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binom_matches_small_known_values() {
        assert_eq!(binom(5, 3), 10);
        assert_eq!(binom(6, 3), 20);
        assert_eq!(binom(4, 2), 6);
        assert_eq!(binom(7, 0), 1);
        assert_eq!(binom(3, 5), 0);
    }

    #[test]
    fn combinations_are_distinct_ascending_and_counted() {
        let combos = combinations(5, 3);
        assert_eq!(combos.len() as u128, binom(5, 3));
        let keys: std::collections::BTreeSet<Vec<usize>> = combos.iter().cloned().collect();
        assert_eq!(keys.len(), combos.len(), "duplicate combination");
        for subset in &combos {
            assert_eq!(subset.len(), 3);
            assert!(
                subset.windows(2).all(|w| w[0] < w[1]),
                "not strictly ascending: {subset:?}"
            );
        }
        assert_eq!(combinations(4, 0), vec![Vec::<usize>::new()]);
        assert!(combinations(3, 5).is_empty());
    }

    #[test]
    fn steiner_design_valid_for_every_offered_order() {
        // 7 (Fano), 9/15/21 (Bose), 13/19/25 (MRV backtracking).
        for n in [7usize, 9, 13, 15, 19, 21, 25] {
            let d = steiner_design(n).unwrap();
            assert_eq!(d.n_vertices, n);
            assert_eq!(d.n_edges(), n * (n - 1) / 6, "triple count for n={n}");
            assert!(d.edges.iter().all(|e| e.len() == 3));
            assert!(
                every_pair_covered_once(n, &d.edges),
                "pair coverage for n={n}"
            );
        }
    }

    #[test]
    fn steiner_rejects_orders_outside_existence_condition() {
        for bad in [4usize, 5, 6, 8, 10, 11, 12, 14] {
            assert_eq!(
                steiner_triples(bad).unwrap_err(),
                GeneratorError::InvalidSteinerOrder(bad),
                "should reject n={bad}"
            );
        }
    }

    #[test]
    fn complete_uniform_has_binom_edges_each_of_arity_r() {
        for (n, r) in [(4usize, 3usize), (5, 3), (6, 2), (6, 4)] {
            let d = complete_uniform_design(n, r).unwrap();
            assert_eq!(d.n_vertices, n);
            assert_eq!(d.n_edges() as u128, binom(n, r));
            assert!(d.edges.iter().all(|e| e.len() == r));
            let keys: std::collections::BTreeSet<Vec<usize>> = d.edges.iter().cloned().collect();
            assert_eq!(keys.len(), d.n_edges(), "duplicate hyperedge");
        }
    }

    #[test]
    fn complete_uniform_rejects_arity_outside_2_to_n() {
        assert_eq!(
            complete_uniform_design(3, 5).unwrap_err(),
            GeneratorError::InvalidComplete { n: 3, r: 5 }
        );
        assert_eq!(
            complete_uniform_design(5, 1).unwrap_err(),
            GeneratorError::InvalidComplete { n: 5, r: 1 }
        );
    }

    #[test]
    fn sunflower_edges_pairwise_intersect_in_exactly_the_core() {
        for (petals, core, petal) in [(3usize, 2usize, 2usize), (5, 1, 3), (4, 0, 2)] {
            let d = sunflower_design(petals, core, petal).unwrap();
            assert_eq!(d.n_vertices, core + petals * petal);
            assert_eq!(d.n_edges(), petals);
            assert!(d.edges.iter().all(|e| e.len() == core + petal));
            let core_set: std::collections::BTreeSet<usize> = (0..core).collect();
            for i in 0..d.edges.len() {
                for j in (i + 1)..d.edges.len() {
                    let ei: std::collections::BTreeSet<usize> =
                        d.edges[i].iter().copied().collect();
                    let ej: std::collections::BTreeSet<usize> =
                        d.edges[j].iter().copied().collect();
                    let inter: std::collections::BTreeSet<usize> =
                        ei.intersection(&ej).copied().collect();
                    assert_eq!(inter, core_set, "intersection != core");
                }
            }
        }
    }

    #[test]
    fn sunflower_rejects_zero_petals_or_petal_size() {
        assert!(matches!(
            sunflower_design(0, 2, 2),
            Err(GeneratorError::InvalidSunflower { .. })
        ));
        assert!(matches!(
            sunflower_design(3, 2, 0),
            Err(GeneratorError::InvalidSunflower { .. })
        ));
    }

    #[test]
    fn generator_enum_dispatches_to_the_free_functions() {
        assert_eq!(
            HypergraphGenerator::SteinerTriple { n: 13 }
                .design()
                .unwrap(),
            steiner_design(13).unwrap()
        );
        assert_eq!(
            HypergraphGenerator::Sunflower {
                petals: 4,
                core: 2,
                petal: 2
            }
            .design()
            .unwrap(),
            sunflower_design(4, 2, 2).unwrap()
        );
        assert_eq!(
            HypergraphGenerator::CompleteUniform { n: 5, r: 3 }
                .design()
                .unwrap(),
            complete_uniform_design(5, 3).unwrap()
        );
        assert_eq!(
            HypergraphGenerator::CompleteUniform { n: 3, r: 5 }
                .design()
                .unwrap_err(),
            GeneratorError::InvalidComplete { n: 3, r: 5 }
        );
    }

    #[test]
    fn steiner_25_builds_under_budget() {
        use std::time::Instant;
        let _warm = steiner_design(25).unwrap();
        let mut samples: Vec<u128> = (0..5)
            .map(|_| {
                let t0 = Instant::now();
                let _ = steiner_design(25).unwrap();
                t0.elapsed().as_micros()
            })
            .collect();
        samples.sort_unstable();
        let median_ms = samples[2] as f64 / 1000.0;
        assert!(
            median_ms < 100.0,
            "S(2,3,25) median build {median_ms:.2} ms exceeds the 100 ms guard"
        );
    }
}
