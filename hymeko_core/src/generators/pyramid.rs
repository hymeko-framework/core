//! **Pyramid hypergraphs of a lattice**: a lattice's points, then level after level of coarser cells,
//! each coarser cell the hyperedge of the finer cells its window reads.
//!
//! Level 0 is the lattice's points at their coordinates plus an offset, numbered row-major (the last
//! axis fastest). Level `l + 1` is the set of cells `⌊q / s_l⌋` over the level-`l` cells `q`, again
//! row-major. A level-`(l + 1)` cell `P` reads the level-`l` cells `q` in the window
//! `s_l P − a_l ≤ q < s_l P − a_l + k_l` on every axis, each at its place
//! `Σ_a (q_a − s_l P_a + a_l) k_l^a` (the first axis fastest).
//!
//! The level's stencil `k`, stride `s` and anchor `a` give the family:
//! - `k = s = 2`, `a = 0`: the orthants, a laminar level (each finer cell read once);
//! - `k = 3`, `s = 2`, `a = 0`: the overlapped level, neighbouring windows sharing a row and a column;
//! - `s = 1`, `a = (k − 1) / 2`: a same-resolution stencil, every cell's neighbourhood of radius `a`.
//!
//! A level needs `k ≥ s + a`, so that every finer cell lies in its own coarser cell's window: the
//! levels are covers. Levels whose stencils grow and then shrink are a widening and narrowing field;
//! the pyramid is the incidence, and a convolution reads it upward (gather) or through its transpose
//! (scatter) as it chooses.

use super::{GeneratorError, HypergraphDesign};

/// One level of a [`LatticePyramid`]: how the next coarser grid reads this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PyramidLevel {
    /// Cells a coarser cell's window spans on each axis.
    pub stencil: usize,
    /// Finer cells per coarser cell on each axis.
    pub stride: usize,
    /// Cells the window starts before `stride · P` on each axis: `0` at the coarser cell's corner,
    /// `(stencil − 1) / 2` centred on it.
    pub anchor: usize,
}

impl PyramidLevel {
    /// The laminar level: `2^dim` orthants, each finer cell read once.
    pub const ORTHANTS: Self = Self {
        stencil: 2,
        stride: 2,
        anchor: 0,
    };

    /// Places in a window over `dim` axes, `stencil^dim`; `None` past `usize`.
    fn places(self, dim: usize) -> Option<usize> {
        u32::try_from(dim)
            .ok()
            .and_then(|dim| self.stencil.checked_pow(dim))
    }

    /// Whether every finer cell lies in its own coarser cell's window.
    fn covers(self) -> bool {
        self.stride >= 1 && self.stencil >= self.stride + self.anchor
    }
}

/// One level's cover: the coarser cells as hyperedges over the finer cells, and each member's place.
///
/// # Invariants
/// `places[e]` is parallel to `design.edges[e]` and ascends; every place is below the level's
/// `stencil^dim`; every finer cell is a member of at least one edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PyramidCover {
    /// Vertices: the finer level's cells. Edges: the coarser level's cells, each its window's
    /// occupied cells by ascending place.
    pub design: HypergraphDesign,
    /// Each member's place in its window.
    pub places: Vec<Vec<usize>>,
}

/// The pyramid hypergraph of a lattice: the cells of every level, level 0 the lattice's points.
///
/// # Invariants
/// `positions[d]` holds `dim` coordinates per cell of depth `d`, the cells in row-major order and
/// distinct; `positions.len() == levels.len() + 1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatticePyramid {
    dim: usize,
    levels: Vec<PyramidLevel>,
    positions: Vec<Vec<usize>>,
}

impl LatticePyramid {
    /// The pyramid of the lattice `extent`, its points shifted by `offset` (none when empty), read
    /// through `levels`, the finest first.
    ///
    /// # Errors
    /// [`GeneratorError::InvalidPyramid`] for no axis, an empty axis, an offset of another length,
    /// no level, or a lattice whose size or coordinates pass `usize`;
    /// [`GeneratorError::InvalidPyramidLevel`] for a level that is not a cover (`stride ≥ 1` and
    /// `stencil ≥ stride + anchor`) or whose window's places pass `usize`.
    pub fn new(
        extent: &[usize],
        offset: &[usize],
        levels: Vec<PyramidLevel>,
    ) -> Result<Self, GeneratorError> {
        let dim = extent.len();
        let refused = || GeneratorError::InvalidPyramid {
            extent: extent.to_vec(),
            offset: offset.to_vec(),
        };
        if dim == 0 || extent.contains(&0) || levels.is_empty() {
            return Err(refused());
        }
        let offset = match offset.len() {
            0 => vec![0; dim],
            n if n == dim => offset.to_vec(),
            _ => return Err(refused()),
        };
        if let Some((level, &at)) = levels
            .iter()
            .enumerate()
            .find(|(_, level)| !level.covers() || level.places(dim).is_none())
        {
            return Err(GeneratorError::InvalidPyramidLevel {
                level,
                stencil: at.stencil,
                stride: at.stride,
                anchor: at.anchor,
            });
        }
        let points = lattice_points(extent, &offset).ok_or_else(refused)?;
        let mut positions = vec![points];
        for level in &levels {
            let finer = positions.last().map_or(&[][..], Vec::as_slice);
            positions.push(coarser_cells(finer, dim, level.stride));
        }
        Ok(Self {
            dim,
            levels,
            positions,
        })
    }

    /// Lattice axes.
    #[must_use]
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// Levels: the depth of the coarsest cells.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.levels.len()
    }

    /// Level `l`, how depth `l + 1` reads depth `l`.
    ///
    /// # Panics
    /// When `l >= depth()`.
    #[must_use]
    pub fn level(&self, l: usize) -> PyramidLevel {
        self.levels[l]
    }

    /// Cells at depth `d`: the lattice's points at `0`.
    ///
    /// # Panics
    /// When `d > depth()`.
    #[must_use]
    pub fn nodes(&self, d: usize) -> usize {
        self.positions[d].len() / self.dim
    }

    /// The coordinates of depth `d`'s cells, `dim` per cell, row-major.
    ///
    /// # Panics
    /// When `d > depth()`.
    #[must_use]
    pub fn positions(&self, d: usize) -> &[usize] {
        &self.positions[d]
    }

    /// Level `l`'s cover: each cell of depth `l + 1` over the cells of depth `l` its window reads.
    ///
    /// # Panics
    /// When `l >= depth()`.
    ///
    /// # Postconditions
    /// The [`PyramidCover`] invariants hold.
    #[must_use]
    pub fn cover(&self, l: usize) -> PyramidCover {
        let level = self.levels[l];
        let (finer, coarser) = (&self.positions[l], &self.positions[l + 1]);
        let grid = CellGrid::of(finer, self.dim);
        let window = level.stencil;
        let places = level
            .places(self.dim)
            .unwrap_or_else(|| unreachable!("new() refuses a level whose places pass usize"));
        let mut edges = Vec::with_capacity(coarser.len() / self.dim);
        let mut edge_places = Vec::with_capacity(coarser.len() / self.dim);
        let mut q = vec![0usize; self.dim];
        for cell in coarser.chunks_exact(self.dim) {
            let (mut members, mut at) = (Vec::new(), Vec::new());
            for place in 0..places {
                let mut rest = place;
                let inside = cell.iter().zip(&mut q).all(|(&c, q)| {
                    let step = rest % window;
                    rest /= window;
                    match (c * level.stride + step).checked_sub(level.anchor) {
                        Some(coordinate) => {
                            *q = coordinate;
                            true
                        }
                        None => false,
                    }
                });
                if let Some(member) = inside.then(|| grid.index(&q)).flatten() {
                    members.push(member);
                    at.push(place);
                }
            }
            edges.push(members);
            edge_places.push(at);
        }
        PyramidCover {
            design: HypergraphDesign {
                n_vertices: finer.len() / self.dim,
                edges,
            },
            // Ascending within each edge by construction: the loop runs over the places in order.
            places: edge_places,
        }
    }
}

/// The lattice's points shifted by `offset`, row-major, `dim` per point; `None` past `usize`.
fn lattice_points(extent: &[usize], offset: &[usize]) -> Option<Vec<usize>> {
    let n = extent.iter().try_fold(1usize, |n, &e| n.checked_mul(e))?;
    extent
        .iter()
        .zip(offset)
        .try_for_each(|(&e, &o)| e.checked_add(o).map(drop))?;
    let dim = extent.len();
    let mut points = vec![0usize; n.checked_mul(dim)?];
    for (t, point) in points.chunks_exact_mut(dim).enumerate() {
        let mut rest = t;
        for a in (0..dim).rev() {
            point[a] = rest % extent[a] + offset[a];
            rest /= extent[a];
        }
    }
    Some(points)
}

/// The distinct cells `⌊q / stride⌋` of the cells `finer`, row-major.
fn coarser_cells(finer: &[usize], dim: usize, stride: usize) -> Vec<usize> {
    let mut cells: Vec<Vec<usize>> = finer
        .chunks_exact(dim)
        .map(|q| q.iter().map(|&c| c / stride).collect())
        .collect();
    cells.sort_unstable();
    cells.dedup();
    cells.into_iter().flatten().collect()
}

/// A level's cells on their bounding grid: each grid point's cell index, if it holds one.
struct CellGrid {
    extent: Vec<usize>,
    index: Vec<Option<usize>>,
}

impl CellGrid {
    /// The grid of `cells` (`dim` coordinates each).
    fn of(cells: &[usize], dim: usize) -> Self {
        let mut extent = vec![0usize; dim];
        for cell in cells.chunks_exact(dim) {
            for (e, &c) in extent.iter_mut().zip(cell) {
                *e = (*e).max(c + 1);
            }
        }
        let size = extent.iter().product();
        let mut grid = Self {
            extent,
            index: vec![None; size],
        };
        for (n, cell) in cells.chunks_exact(dim).enumerate() {
            if let Some(at) = grid.flat(cell) {
                grid.index[at] = Some(n);
            }
        }
        grid
    }

    /// The row-major grid index of `cell`, `None` outside the grid.
    fn flat(&self, cell: &[usize]) -> Option<usize> {
        cell.iter()
            .zip(&self.extent)
            .try_fold(0usize, |acc, (&c, &e)| (c < e).then_some(acc * e + c))
    }

    /// The cell at `q`, if one is there.
    fn index(&self, q: &[usize]) -> Option<usize> {
        self.flat(q).and_then(|at| self.index[at])
    }
}

#[cfg(test)]
mod tests {
    use super::{LatticePyramid, PyramidLevel};
    use crate::generators::GeneratorError;

    const fn level(stencil: usize, stride: usize, anchor: usize) -> PyramidLevel {
        PyramidLevel {
            stencil,
            stride,
            anchor,
        }
    }

    /// How many edges read each vertex.
    fn readers(design: &crate::generators::HypergraphDesign) -> Vec<usize> {
        let mut count = vec![0usize; design.n_vertices];
        for edge in &design.edges {
            for &v in edge {
                count[v] += 1;
            }
        }
        count
    }

    /// The orthants over the 28 x 28 lattice: each level laminar (every finer cell read exactly
    /// once), and the node counts of Nagare's MNIST orthtree, 784, 196, 49, 16, 4, 1.
    #[test]
    fn orthants_over_28_are_laminar_with_the_orthtree_counts() {
        let pyramid = LatticePyramid::new(&[28, 28], &[], vec![PyramidLevel::ORTHANTS; 5]).unwrap();
        let counts: Vec<usize> = (0..=5).map(|d| pyramid.nodes(d)).collect();
        assert_eq!(counts, [784, 196, 49, 16, 4, 1]);
        for l in 0..5 {
            let cover = pyramid.cover(l);
            assert!(readers(&cover.design).iter().all(|&r| r == 1), "level {l}");
            assert!(cover.places.iter().flatten().all(|&p| p < 4));
        }
    }

    /// The overlapped level (3 x 3 at stride 2, anchored at the corner): an interior finer cell at
    /// even coordinates on both axes is read by 4 windows, at one even coordinate by 2, at none by 1;
    /// members ascend in place and sit where their place says.
    #[test]
    fn the_overlapped_level_shares_rows_and_columns() {
        let pyramid = LatticePyramid::new(&[28, 28], &[], vec![level(3, 2, 0)]).unwrap();
        let cover = pyramid.cover(0);
        let (finer, coarser) = (pyramid.positions(0), pyramid.positions(1));
        let count = readers(&cover.design);
        for (v, q) in finer.chunks_exact(2).enumerate() {
            if q.iter().all(|&c| (2..26).contains(&c)) {
                let even = q.iter().filter(|&&c| c % 2 == 0).count();
                assert_eq!(count[v], [1, 2, 4][even], "cell {q:?}");
            }
        }
        for ((edge, places), p) in cover
            .design
            .edges
            .iter()
            .zip(&cover.places)
            .zip(coarser.chunks_exact(2))
        {
            assert!(places.windows(2).all(|w| w[0] < w[1]));
            for (&v, &place) in edge.iter().zip(places) {
                let q = &finer[2 * v..2 * v + 2];
                assert_eq!(q, [2 * p[0] + place % 3, 2 * p[1] + place / 3]);
            }
        }
    }

    /// A sequence (one axis): dyadic levels halve with a ceiling, 10, 5, 3, 2, 1, and the overlapped
    /// level reads each window's three cells where they exist.
    #[test]
    fn a_sequence_halves_and_overlaps() {
        let dyadic = LatticePyramid::new(&[10], &[], vec![PyramidLevel::ORTHANTS; 4]).unwrap();
        let counts: Vec<usize> = (0..=4).map(|d| dyadic.nodes(d)).collect();
        assert_eq!(counts, [10, 5, 3, 2, 1]);
        let overlapped = LatticePyramid::new(&[10], &[], vec![level(3, 2, 0)]).unwrap();
        let edges = overlapped.cover(0).design.edges;
        assert_eq!(
            edges,
            vec![
                vec![0, 1, 2],
                vec![2, 3, 4],
                vec![4, 5, 6],
                vec![6, 7, 8],
                vec![8, 9]
            ]
        );
    }

    /// Three axes: 4 x 4 x 4 under orthants is 64, 8, 1, each level laminar.
    #[test]
    fn a_cube_is_an_octree() {
        let pyramid =
            LatticePyramid::new(&[4, 4, 4], &[], vec![PyramidLevel::ORTHANTS; 2]).unwrap();
        assert_eq!(
            (pyramid.nodes(0), pyramid.nodes(1), pyramid.nodes(2)),
            (64, 8, 1)
        );
        assert!(readers(&pyramid.cover(1).design).iter().all(|&r| r == 1));
    }

    /// An offset moves the cuts: five points at 0..4 pair as {0, 1}, {2, 3}, {4}; at 1..5, as {1},
    /// {2, 3}, {4, 5}, so the lone point moves from the last window to the first.
    #[test]
    fn an_offset_moves_the_cuts() {
        let unshifted = LatticePyramid::new(&[5], &[], vec![PyramidLevel::ORTHANTS]).unwrap();
        assert_eq!(
            unshifted.cover(0).design.edges,
            vec![vec![0, 1], vec![2, 3], vec![4]]
        );
        let shifted = LatticePyramid::new(&[5], &[1], vec![PyramidLevel::ORTHANTS]).unwrap();
        assert_eq!(
            shifted.cover(0).design.edges,
            vec![vec![0], vec![1, 2], vec![3, 4]]
        );
        assert_eq!(shifted.positions(0), [1, 2, 3, 4, 5]);
    }

    /// A same-resolution stencil (stride 1, centred) is every cell's clipped neighbourhood of radius
    /// 1, each member at the place `(o_0 + 1) + 3 (o_1 + 1)`, the cells themselves unchanged.
    #[test]
    fn a_centred_stride_one_level_is_the_neighbourhood() {
        let (side, radius) = (4usize, 1usize);
        let pyramid = LatticePyramid::new(&[side, side], &[], vec![level(3, 1, radius)]).unwrap();
        assert_eq!(pyramid.positions(1), pyramid.positions(0));
        let cover = pyramid.cover(0);
        for (p, (edge, places)) in cover.design.edges.iter().zip(&cover.places).enumerate() {
            let (r, c) = (p / side, p % side);
            let mut want = Vec::new();
            for dc in 0..3usize {
                for dr in 0..3usize {
                    let (q0, q1) = ((r + dr).checked_sub(radius), (c + dc).checked_sub(radius));
                    if let (Some(q0), Some(q1)) = (q0, q1) {
                        if q0 < side && q1 < side {
                            want.push((q0 * side + q1, dr + 3 * dc));
                        }
                    }
                }
            }
            let got: Vec<(usize, usize)> =
                edge.iter().copied().zip(places.iter().copied()).collect();
            assert_eq!(got, want, "cell ({r}, {c})");
        }
    }

    /// The MNIST lattice's overlapped pyramid, every level's cover built, within the plan's budget:
    /// a one-time compile cost (plan 2026-10-15-pyramid-hypergraphs, budget under 1 ms; the guard
    /// allows 5 ms for a busy host).
    #[test]
    #[ignore = "performance budget measurement; opt in with --ignored --exact"]
    fn the_mnist_pyramid_builds_under_budget() {
        use std::time::Instant;
        let build = || {
            let pyramid = LatticePyramid::new(&[28, 28], &[], vec![level(3, 2, 0); 5]).unwrap();
            (0..5).map(|l| pyramid.cover(l).places.len()).sum::<usize>()
        };
        let _warm = build();
        let mut samples: Vec<u128> = (0..5)
            .map(|_| {
                let t0 = Instant::now();
                std::hint::black_box(build());
                t0.elapsed().as_micros()
            })
            .collect();
        samples.sort_unstable();
        let median_ms = samples[2] as f64 / 1000.0;
        assert!(
            median_ms < 5.0,
            "the 28 x 28 overlapped pyramid's median build {median_ms:.3} ms exceeds the 5 ms guard"
        );
    }

    /// Refusals: a level that leaves a finer cell outside its own coarser window, a zero stride, no
    /// level, no axis, an empty axis, an offset of another length.
    #[test]
    fn a_pyramid_that_is_not_a_cover_is_refused() {
        let at = |levels: Vec<PyramidLevel>| LatticePyramid::new(&[8, 8], &[], levels).unwrap_err();
        assert_eq!(
            at(vec![PyramidLevel::ORTHANTS, level(2, 3, 0)]),
            GeneratorError::InvalidPyramidLevel {
                level: 1,
                stencil: 2,
                stride: 3,
                anchor: 0
            }
        );
        assert!(matches!(
            at(vec![level(3, 2, 2)]),
            GeneratorError::InvalidPyramidLevel { level: 0, .. }
        ));
        assert!(matches!(
            at(vec![level(1, 0, 0)]),
            GeneratorError::InvalidPyramidLevel { .. }
        ));
        for (extent, offset, levels) in [
            (vec![8, 8], vec![], vec![]),
            (vec![], vec![], vec![PyramidLevel::ORTHANTS]),
            (vec![8, 0], vec![], vec![PyramidLevel::ORTHANTS]),
            (vec![8, 8], vec![1], vec![PyramidLevel::ORTHANTS]),
        ] {
            assert!(
                matches!(
                    LatticePyramid::new(&extent, &offset, levels),
                    Err(GeneratorError::InvalidPyramid { .. })
                ),
                "{extent:?} {offset:?}"
            );
        }
    }
}
