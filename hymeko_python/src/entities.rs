//! Read-only value views over the canonical `Ir`. Each holds an `Arc<ModelInner>`
//! (so it keeps the model alive — mission "model lifetime") plus the core id it
//! addresses, and resolves on demand. Nothing here is mutable; there are no
//! setters. Ids handed out carry the model token (§4).

use std::sync::Arc;

use hymeko::common::ids::{
    DeclId as CDeclId, EdgeId as CEdgeId, HyperArcId as CArcId, NodeId as CNodeId,
};
use hymeko::ir::ir::{AnnoR, DeclKind, SignedRefR, ValueR};
use pyo3::IntoPyObjectExt;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

use crate::ids::{DeclId, EdgeId, HyperArcId, NodeId};
use crate::pipeline::ModelInner;

pub(crate) fn kind_str(k: DeclKind) -> &'static str {
    match k {
        DeclKind::Node => "node",
        DeclKind::Edge => "edge",
        DeclKind::HyperArc => "hyperarc",
    }
}

fn value_to_py<'py>(py: Python<'py>, inner: &ModelInner, v: &ValueR) -> PyResult<Py<PyAny>> {
    match v {
        ValueR::Str(s) => inner.name(*s).into_py_any(py),
        ValueR::Num(x) => x.into_py_any(py),
        ValueR::List(vs) => {
            let items: PyResult<Vec<Py<PyAny>>> =
                vs.iter().map(|w| value_to_py(py, inner, w)).collect();
            PyList::new(py, items?)?.into_py_any(py)
        }
        ValueR::Ref(d) => DeclId::new(d.raw(), inner.token).into_py_any(py),
    }
}

fn weights_to_py<'py>(
    py: Python<'py>,
    inner: &ModelInner,
    weights: &Option<Vec<ValueR>>,
) -> PyResult<Py<PyAny>> {
    match weights {
        None => Ok(py.None()),
        Some(ws) => {
            let items: PyResult<Vec<Py<PyAny>>> =
                ws.iter().map(|w| value_to_py(py, inner, w)).collect();
            PyList::new(py, items?)?.into_py_any(py)
        }
    }
}

/// `AnnoR` → `{"tags": [str], "value": <value or None>}`.
fn anno_to_py<'py>(py: Python<'py>, inner: &ModelInner, anno: &AnnoR) -> PyResult<Py<PyAny>> {
    let d = PyDict::new(py);
    let tags: Vec<&str> = anno.tags.iter().map(|&s| inner.name(s)).collect();
    d.set_item("tags", tags)?;
    match &anno.value {
        Some(v) => d.set_item("value", value_to_py(py, inner, v)?)?,
        None => d.set_item("value", py.None())?,
    }
    d.into_py_any(py)
}

/// A signed reference (a node/edge `base`/`isa` link). Value view.
#[pyclass(frozen, module = "hymeko")]
pub struct BaseRef {
    pub(crate) inner: Arc<ModelInner>,
    pub(crate) sref: SignedRefR,
}

#[pymethods]
impl BaseRef {
    /// The referenced declaration id (the base/parent in an `isa`/`link`).
    #[getter]
    fn target(&self) -> DeclId {
        DeclId::new(self.sref.target().raw(), self.inner.token)
    }
    /// `+1` / `-1` / `0` for `+` / `-` / `~`.
    #[getter]
    fn sign(&self) -> i8 {
        self.sref.sign()
    }
    /// The weight list (or `None`).
    #[getter]
    fn weights(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        weights_to_py(py, &self.inner, &self.sref.atom().weights)
    }
    /// The annotation `{"tags": [...], "value": ...}`.
    #[getter]
    fn annotation(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        anno_to_py(py, &self.inner, &self.sref.atom().anno)
    }
    fn __repr__(&self) -> String {
        format!(
            "BaseRef(target={}, sign={})",
            self.sref.target().raw(),
            self.sref.sign()
        )
    }
}

/// One incidence of a hyperarc: an arc reference resolved to `(arc, slot, sign,
/// target, weights, annotation)` — read directly from `ArcRec.refs` (forward,
/// arc-major; no reverse index). Value view.
#[pyclass(frozen, module = "hymeko")]
pub struct Incidence {
    pub(crate) inner: Arc<ModelInner>,
    pub(crate) arc: usize,
    pub(crate) slot: u32,
    pub(crate) sref: SignedRefR,
}

#[pymethods]
impl Incidence {
    /// The owning hyperarc id.
    #[getter]
    fn arc(&self) -> HyperArcId {
        HyperArcId::new(self.arc, self.inner.token)
    }
    /// The ref slot (position within the arc — the exact incidence coordinate).
    #[getter]
    fn slot(&self) -> u32 {
        self.slot
    }
    /// `+1` / `-1` / `0`.
    #[getter]
    fn sign(&self) -> i8 {
        self.sref.sign()
    }
    /// The referenced declaration id (the incidence's target).
    #[getter]
    fn target(&self) -> DeclId {
        DeclId::new(self.sref.target().raw(), self.inner.token)
    }
    #[getter]
    fn weights(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        weights_to_py(py, &self.inner, &self.sref.atom().weights)
    }
    #[getter]
    fn annotation(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        anno_to_py(py, &self.inner, &self.sref.atom().anno)
    }
    fn __repr__(&self) -> String {
        format!(
            "Incidence(arc={}, slot={}, sign={}, target={})",
            self.arc,
            self.slot,
            self.sref.sign(),
            self.sref.target().raw()
        )
    }
}

/// A declaration (the unified hierarchy node). Value view.
#[pyclass(frozen, module = "hymeko")]
pub struct Declaration {
    pub(crate) inner: Arc<ModelInner>,
    pub(crate) id: CDeclId,
}

#[pymethods]
impl Declaration {
    #[getter]
    fn id(&self) -> DeclId {
        DeclId::new(self.id.raw(), self.inner.token)
    }
    #[getter]
    fn name(&self) -> String {
        let ir = self.inner.ir();
        ir.decl_node(self.id)
            .map(|d| self.inner.name(d.name).to_owned())
            .unwrap_or_default()
    }
    #[getter]
    fn kind(&self) -> &'static str {
        self.inner
            .ir()
            .kind_of(self.id)
            .map(kind_str)
            .unwrap_or("unknown")
    }
    #[getter]
    fn canonical_path(&self) -> Option<String> {
        self.inner.canonical_path(self.id)
    }
    #[getter]
    fn parent(&self) -> Option<DeclId> {
        self.inner
            .ir()
            .parent_of(self.id)
            .filter(|p| p.is_some())
            .map(|p| DeclId::new(p.raw(), self.inner.token))
    }
    #[getter]
    fn children(&self) -> Vec<DeclId> {
        self.inner
            .ir()
            .child_decls(self.id)
            .map(|c| DeclId::new(c.raw(), self.inner.token))
            .collect()
    }
    #[getter]
    fn annotation(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        match self.inner.ir().decl_node(self.id) {
            Some(d) => anno_to_py(py, &self.inner, &d.anno),
            None => Ok(py.None()),
        }
    }
    fn __repr__(&self) -> String {
        format!(
            "Declaration(id={}, kind={}, name={:?})",
            self.id.raw(),
            self.kind(),
            self.name()
        )
    }
}

/// A node record. Value view.
#[pyclass(frozen, module = "hymeko")]
pub struct Node {
    pub(crate) inner: Arc<ModelInner>,
    pub(crate) id: CNodeId,
}

#[pymethods]
impl Node {
    #[getter]
    fn id(&self) -> NodeId {
        NodeId::new(self.id.raw(), self.inner.token)
    }
    #[getter]
    fn decl(&self) -> Option<DeclId> {
        self.inner
            .ir()
            .node(self.id)
            .map(|r| DeclId::new(r.decl.raw(), self.inner.token))
    }
    #[getter]
    fn name(&self) -> String {
        let ir = self.inner.ir();
        ir.node(self.id)
            .and_then(|r| ir.decl_node(r.decl))
            .map(|d| self.inner.name(d.name).to_owned())
            .unwrap_or_default()
    }
    #[getter]
    fn canonical_path(&self) -> Option<String> {
        let ir = self.inner.ir();
        let decl = ir.node(self.id)?.decl;
        self.inner.canonical_path(decl)
    }
    /// The node's `base`/`isa` links.
    #[getter]
    fn bases(&self, py: Python<'_>) -> PyResult<Vec<Py<BaseRef>>> {
        let ir = self.inner.ir();
        let bases = match ir.node(self.id) {
            Some(r) => &r.bases,
            None => return Ok(Vec::new()),
        };
        bases
            .iter()
            .map(|s| {
                Py::new(
                    py,
                    BaseRef {
                        inner: Arc::clone(&self.inner),
                        sref: s.clone(),
                    },
                )
            })
            .collect()
    }
    fn __repr__(&self) -> String {
        format!("Node(id={}, name={:?})", self.id.raw(), self.name())
    }
}

/// An edge record. Value view.
#[pyclass(frozen, module = "hymeko")]
pub struct Edge {
    pub(crate) inner: Arc<ModelInner>,
    pub(crate) id: CEdgeId,
}

#[pymethods]
impl Edge {
    #[getter]
    fn id(&self) -> EdgeId {
        EdgeId::new(self.id.raw(), self.inner.token)
    }
    #[getter]
    fn decl(&self) -> Option<DeclId> {
        self.inner
            .ir()
            .edge(self.id)
            .map(|r| DeclId::new(r.decl.raw(), self.inner.token))
    }
    #[getter]
    fn name(&self) -> String {
        let ir = self.inner.ir();
        ir.edge(self.id)
            .and_then(|r| ir.decl_node(r.decl))
            .map(|d| self.inner.name(d.name).to_owned())
            .unwrap_or_default()
    }
    #[getter]
    fn canonical_path(&self) -> Option<String> {
        let ir = self.inner.ir();
        let decl = ir.edge(self.id)?.decl;
        self.inner.canonical_path(decl)
    }
    #[getter]
    fn bases(&self, py: Python<'_>) -> PyResult<Vec<Py<BaseRef>>> {
        let ir = self.inner.ir();
        let bases = match ir.edge(self.id) {
            Some(r) => &r.bases,
            None => return Ok(Vec::new()),
        };
        bases
            .iter()
            .map(|s| {
                Py::new(
                    py,
                    BaseRef {
                        inner: Arc::clone(&self.inner),
                        sref: s.clone(),
                    },
                )
            })
            .collect()
    }
    /// The hyperarc ids this edge owns.
    #[getter]
    fn hyperarcs(&self) -> Vec<HyperArcId> {
        match self.inner.ir().edge(self.id) {
            Some(r) => r
                .arcs
                .iter()
                .map(|a| HyperArcId::new(a.raw(), self.inner.token))
                .collect(),
            None => Vec::new(),
        }
    }
    fn __repr__(&self) -> String {
        format!("Edge(id={}, name={:?})", self.id.raw(), self.name())
    }
}

/// A hyperarc record. Value view.
#[pyclass(frozen, module = "hymeko")]
pub struct HyperArc {
    pub(crate) inner: Arc<ModelInner>,
    pub(crate) id: CArcId,
}

#[pymethods]
impl HyperArc {
    #[getter]
    fn id(&self) -> HyperArcId {
        HyperArcId::new(self.id.raw(), self.inner.token)
    }
    /// The owning edge's declaration id.
    #[getter]
    fn in_edge(&self) -> Option<DeclId> {
        let ir = self.inner.ir();
        let e = ir.arc(self.id)?.in_edge;
        e.is_some().then(|| DeclId::new(e.raw(), self.inner.token))
    }
    /// The arity (number of incidences).
    #[getter]
    fn arity(&self) -> usize {
        self.inner
            .ir()
            .arc(self.id)
            .map(|r| r.refs.len())
            .unwrap_or(0)
    }
    /// The incidences of this hyperarc, in canonical slot order.
    #[getter]
    fn incidences(&self, py: Python<'_>) -> PyResult<Vec<Py<Incidence>>> {
        let ir = self.inner.ir();
        let refs = match ir.arc(self.id) {
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
                        arc: self.id.raw(),
                        slot: slot as u32,
                        sref: s.clone(),
                    },
                )
            })
            .collect()
    }
    #[getter]
    fn annotation(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        match self.inner.ir().arc(self.id) {
            Some(r) => anno_to_py(py, &self.inner, &r.anno),
            None => Ok(py.None()),
        }
    }
    fn __repr__(&self) -> String {
        format!("HyperArc(id={}, arity={})", self.id.raw(), self.arity())
    }
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Declaration>()?;
    m.add_class::<Node>()?;
    m.add_class::<Edge>()?;
    m.add_class::<HyperArc>()?;
    m.add_class::<Incidence>()?;
    m.add_class::<BaseRef>()?;
    Ok(())
}
