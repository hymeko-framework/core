//! The `Model` — a read-only handle owning the canonical `Ir` + `Interner` (via
//! `Arc<ModelInner>`) and the entry points `hymeko.parse` / `hymeko.load`.

use std::path::PathBuf;
use std::sync::Arc;

use hymeko::common::ids::{
    DeclId as CDeclId, EdgeId as CEdgeId, HyperArcId as CArcId, NodeId as CNodeId,
};
use pyo3::prelude::*;

use crate::arrays::IncidenceArrays;
use crate::entities::{Declaration, Edge, HyperArc, Incidence, Node};
use crate::ids::{DeclId, EdgeId, HyperArcId, NodeId};
use crate::pipeline::{ModelInner, build_from_file, build_from_source};

/// A parsed, lowered, coherence-validated HyMeKo model — a read-only view over the
/// canonical `Ir`.
#[pyclass(frozen, module = "hymeko")]
pub struct Model {
    inner: Arc<ModelInner>,
}

impl Model {
    pub(crate) fn from_inner(inner: ModelInner) -> Self {
        Self {
            inner: Arc::new(inner),
        }
    }

    fn incidences_targeting(&self, py: Python<'_>, decl: CDeclId) -> PyResult<Vec<Py<Incidence>>> {
        let ir = self.inner.ir();
        let mut out = Vec::new();
        for (arc_id, rec) in ir.arc_records() {
            for (slot, sref) in rec.refs.iter().enumerate() {
                if sref.target() == decl {
                    out.push(Py::new(
                        py,
                        Incidence {
                            inner: Arc::clone(&self.inner),
                            arc: arc_id.raw(),
                            slot: slot as u32,
                            sref: sref.clone(),
                        },
                    )?);
                }
            }
        }
        Ok(out)
    }
}

#[pymethods]
impl Model {
    #[getter]
    fn declaration_count(&self) -> usize {
        self.inner.ir().decl_count()
    }
    #[getter]
    fn node_count(&self) -> usize {
        self.inner.ir().node_count()
    }
    #[getter]
    fn edge_count(&self) -> usize {
        self.inner.ir().edge_count()
    }
    #[getter]
    fn hyperarc_count(&self) -> usize {
        self.inner.ir().arc_count()
    }

    /// The 64-hex canonical program hash (bit-identical to the Rust
    /// `canonical_program_hash` oracle).
    fn canonical_hash(&self) -> String {
        self.inner.canonical_hash_hex()
    }

    fn declaration(&self, id: &DeclId) -> PyResult<Declaration> {
        let raw = id.checked(self.inner.token, self.inner.ir().decl_count())?;
        Ok(Declaration {
            inner: Arc::clone(&self.inner),
            id: CDeclId::new(raw),
        })
    }
    fn node(&self, id: &NodeId) -> PyResult<Node> {
        let raw = id.checked(self.inner.token, self.inner.ir().node_count())?;
        Ok(Node {
            inner: Arc::clone(&self.inner),
            id: CNodeId::new(raw),
        })
    }
    fn edge(&self, id: &EdgeId) -> PyResult<Edge> {
        let raw = id.checked(self.inner.token, self.inner.ir().edge_count())?;
        Ok(Edge {
            inner: Arc::clone(&self.inner),
            id: CEdgeId::new(raw),
        })
    }
    fn hyperarc(&self, id: &HyperArcId) -> PyResult<HyperArc> {
        let raw = id.checked(self.inner.token, self.inner.ir().arc_count())?;
        Ok(HyperArc {
            inner: Arc::clone(&self.inner),
            id: CArcId::new(raw),
        })
    }

    fn declarations(&self, py: Python<'_>) -> PyResult<Vec<Py<Declaration>>> {
        (0..self.inner.ir().decl_count())
            .map(|i| {
                Py::new(
                    py,
                    Declaration {
                        inner: Arc::clone(&self.inner),
                        id: CDeclId::new(i),
                    },
                )
            })
            .collect()
    }
    fn nodes(&self, py: Python<'_>) -> PyResult<Vec<Py<Node>>> {
        (0..self.inner.ir().node_count())
            .map(|i| {
                Py::new(
                    py,
                    Node {
                        inner: Arc::clone(&self.inner),
                        id: CNodeId::new(i),
                    },
                )
            })
            .collect()
    }
    fn edges(&self, py: Python<'_>) -> PyResult<Vec<Py<Edge>>> {
        (0..self.inner.ir().edge_count())
            .map(|i| {
                Py::new(
                    py,
                    Edge {
                        inner: Arc::clone(&self.inner),
                        id: CEdgeId::new(i),
                    },
                )
            })
            .collect()
    }
    fn hyperarcs(&self, py: Python<'_>) -> PyResult<Vec<Py<HyperArc>>> {
        (0..self.inner.ir().arc_count())
            .map(|i| {
                Py::new(
                    py,
                    HyperArc {
                        inner: Arc::clone(&self.inner),
                        id: CArcId::new(i),
                    },
                )
            })
            .collect()
    }

    fn parent_of(&self, id: &DeclId) -> PyResult<Option<DeclId>> {
        let raw = id.checked(self.inner.token, self.inner.ir().decl_count())?;
        Ok(self
            .inner
            .ir()
            .parent_of(CDeclId::new(raw))
            .filter(|p| p.is_some())
            .map(|p| DeclId::new(p.raw(), self.inner.token)))
    }
    fn children_of(&self, id: &DeclId) -> PyResult<Vec<DeclId>> {
        let raw = id.checked(self.inner.token, self.inner.ir().decl_count())?;
        Ok(self
            .inner
            .ir()
            .child_decls(CDeclId::new(raw))
            .map(|c| DeclId::new(c.raw(), self.inner.token))
            .collect())
    }

    /// The incidences of a hyperarc (forward, in canonical slot order).
    fn incidences_of_arc(&self, py: Python<'_>, id: &HyperArcId) -> PyResult<Vec<Py<Incidence>>> {
        let raw = id.checked(self.inner.token, self.inner.ir().arc_count())?;
        let ir = self.inner.ir();
        let refs = match ir.arc(CArcId::new(raw)) {
            Some(r) => &r.refs,
            None => return Ok(Vec::new()),
        };
        refs.iter()
            .enumerate()
            .map(|(slot, s)| {
                Py::new(
                    py,
                    Incidence {
                        inner: Arc::clone(&self.inner),
                        arc: raw,
                        slot: slot as u32,
                        sref: s.clone(),
                    },
                )
            })
            .collect()
    }

    /// The incidences targeting a node — a query over the canonical `Ir`
    /// (`O(arcs·arity)`; the promoted core has no reverse index, and PoW-0 does not
    /// reimplement the unpromoted one). Order is deterministic (arc-major, slot).
    fn incidences_of_node(&self, py: Python<'_>, id: &NodeId) -> PyResult<Vec<Py<Incidence>>> {
        let raw = id.checked(self.inner.token, self.inner.ir().node_count())?;
        let ir = self.inner.ir();
        let decl = match ir.node(CNodeId::new(raw)) {
            Some(r) => r.decl,
            None => return Ok(Vec::new()),
        };
        self.incidences_targeting(py, decl)
    }

    /// Batch arc-major sparse-SoA incidence export (no per-incidence Python object).
    fn incidence_arrays(&self, py: Python<'_>) -> PyResult<IncidenceArrays> {
        IncidenceArrays::build(py, &self.inner)
    }

    fn __repr__(&self) -> String {
        format!(
            "Model(declarations={}, nodes={}, edges={}, hyperarcs={})",
            self.declaration_count(),
            self.node_count(),
            self.edge_count(),
            self.hyperarc_count()
        )
    }
}

/// Parse in-memory HyMeKo source into a read-only [`Model`].
#[pyfunction]
#[pyo3(signature = (source, filename = "<memory>"))]
pub fn parse(py: Python<'_>, source: &str, filename: &str) -> PyResult<Model> {
    Ok(Model::from_inner(build_from_source(py, source, filename)?))
}

/// Load and compile a `.hymeko` file (imports resolved) into a read-only [`Model`].
#[pyfunction]
pub fn load(py: Python<'_>, path: PathBuf) -> PyResult<Model> {
    Ok(Model::from_inner(build_from_file(py, &path)?))
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Model>()?;
    m.add_function(wrap_pyfunction!(parse, m)?)?;
    m.add_function(wrap_pyfunction!(load, m)?)?;
    Ok(())
}
