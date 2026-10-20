//! Owner-aware views keep the existing inline wire while storage uses IDs.
use super::{IncidenceSign, PortDirection, RefAtomR, ReferenceDraft, SignedRefR};
use crate::common::ids::{DeclId, HyperArcId};
use crate::ir::{
    ir::{AnnoR, ArcRec, DeclNode, EdgeRec, Ir, NodeRec, ValueR},
    table::IrTable,
};
use serde::{
    Deserialize, Serialize, Serializer,
    ser::{SerializeSeq, SerializeStruct},
};

#[derive(Serialize)]
#[serde(rename = "RefAtomR")]
struct Atom<'a> {
    target: DeclId,
    anno: &'a AnnoR,
    weights: Option<&'a [ValueR]>,
    #[serde(skip_serializing_if = "PortDirection::is_unspecified")]
    direction: PortDirection,
}

struct Reference<'a> {
    ir: &'a Ir,
    reference: &'a SignedRefR,
}
impl Serialize for Reference<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let value = self
            .ir
            .reference_atom(self.reference)
            .map_err(serde::ser::Error::custom)?;
        let atom = Atom {
            target: value.target,
            anno: value.anno,
            weights: value.weights,
            direction: value.direction,
        };
        let (ordinal, name) = match self.reference.sign_kind() {
            IncidenceSign::Plus => (0, "Plus"),
            IncidenceSign::Minus => (1, "Minus"),
            IncidenceSign::Neutral => (2, "Neutral"),
        };
        serializer.serialize_newtype_variant("SignedRefR", ordinal, name, &atom)
    }
}

pub(crate) struct References<'a> {
    pub ir: &'a Ir,
    pub values: &'a [SignedRefR],
}
impl Serialize for References<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.values.iter().map(|reference| Reference {
            ir: self.ir,
            reference,
        }))
    }
}

pub(crate) trait ReferenceRecord {
    fn serialize_record<S: Serializer>(&self, ir: &Ir, serializer: S) -> Result<S::Ok, S::Error>;
}

pub(crate) struct Record<'a, T> {
    pub ir: &'a Ir,
    pub value: &'a T,
}
impl<T: ReferenceRecord> Serialize for Record<'_, T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.value.serialize_record(self.ir, serializer)
    }
}

pub(crate) struct Records<'a, T> {
    pub ir: &'a Ir,
    pub values: &'a IrTable<T>,
}
impl<T: ReferenceRecord> Serialize for Records<'_, T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.values.len()))?;
        for value in self.values {
            sequence.serialize_element(&Record { ir: self.ir, value })?;
        }
        sequence.end()
    }
}

struct NamedRecord<'a> {
    decl: DeclId,
    bases: References<'a>,
    arcs: Option<&'a [HyperArcId]>,
}
impl Serialize for NamedRecord<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut fields = serializer.serialize_struct(
            if self.arcs.is_some() {
                "EdgeRec"
            } else {
                "NodeRec"
            },
            2 + usize::from(self.arcs.is_some()),
        )?;
        fields.serialize_field("decl", &self.decl)?;
        fields.serialize_field("bases", &self.bases)?;
        if let Some(arcs) = self.arcs {
            fields.serialize_field("arcs", arcs)?;
        }
        fields.end()
    }
}
impl ReferenceRecord for DeclNode {
    fn serialize_record<S: Serializer>(&self, ir: &Ir, serializer: S) -> Result<S::Ok, S::Error> {
        let anno = ir
            .annotation(self.anno)
            .ok_or_else(|| serde::ser::Error::custom("invalid annotation ID"))?;
        self.map_annotation(|_| anno).serialize(serializer)
    }
}
impl ReferenceRecord for NodeRec {
    fn serialize_record<S: Serializer>(&self, ir: &Ir, serializer: S) -> Result<S::Ok, S::Error> {
        NamedRecord {
            decl: self.decl,
            bases: References {
                ir,
                values: ir
                    .bases(self.bases)
                    .ok_or_else(|| serde::ser::Error::custom("invalid base-list ID"))?,
            },
            arcs: None,
        }
        .serialize(serializer)
    }
}
impl ReferenceRecord for EdgeRec {
    fn serialize_record<S: Serializer>(&self, ir: &Ir, serializer: S) -> Result<S::Ok, S::Error> {
        NamedRecord {
            decl: self.decl,
            bases: References {
                ir,
                values: ir
                    .bases(self.bases)
                    .ok_or_else(|| serde::ser::Error::custom("invalid base-list ID"))?,
            },
            arcs: Some(&self.arcs),
        }
        .serialize(serializer)
    }
}
impl ReferenceRecord for ArcRec {
    fn serialize_record<S: Serializer>(&self, ir: &Ir, serializer: S) -> Result<S::Ok, S::Error> {
        let mut fields = serializer.serialize_struct("ArcRec", 3)?;
        fields.serialize_field(
            "anno",
            ir.annotation(self.anno)
                .ok_or_else(|| serde::ser::Error::custom("invalid annotation ID"))?,
        )?;
        fields.serialize_field("in_edge", &self.in_edge)?;
        fields.serialize_field(
            "refs",
            &References {
                ir,
                values: &self.refs,
            },
        )?;
        fields.end()
    }
}

#[derive(Deserialize)]
#[serde(rename = "SignedRefR")]
enum ReferenceInput {
    Plus(RefAtomR),
    Minus(RefAtomR),
    Neutral(RefAtomR),
}
impl ReferenceInput {
    fn into_draft(self) -> ReferenceDraft {
        let (sign, atom) = match self {
            Self::Plus(atom) => (IncidenceSign::Plus, atom),
            Self::Minus(atom) => (IncidenceSign::Minus, atom),
            Self::Neutral(atom) => (IncidenceSign::Neutral, atom),
        };
        ReferenceDraft::new(sign, atom)
    }
    fn store_all(values: Vec<Self>, ir: &mut Ir) -> Vec<SignedRefR> {
        ir.store_references(values.into_iter().map(Self::into_draft))
    }
    fn store_bases(values: Vec<Self>, ir: &mut Ir) -> crate::common::ids::BaseListId {
        ir.store_base_drafts(values.into_iter().map(Self::into_draft).collect())
    }
}

#[derive(Deserialize)]
pub(crate) struct NodeInput {
    pub decl: DeclId,
    bases: Vec<ReferenceInput>,
}
impl NodeInput {
    pub fn store(self, ir: &mut Ir) -> NodeRec {
        NodeRec::new(self.decl, ReferenceInput::store_bases(self.bases, ir))
    }
}
#[derive(Deserialize)]
pub(crate) struct EdgeInput {
    pub decl: DeclId,
    bases: Vec<ReferenceInput>,
    arcs: Vec<HyperArcId>,
}
impl EdgeInput {
    pub fn store(self, ir: &mut Ir) -> EdgeRec {
        EdgeRec {
            decl: self.decl,
            bases: ReferenceInput::store_bases(self.bases, ir),
            arcs: self.arcs,
        }
    }
}
#[derive(Deserialize)]
pub(crate) struct ArcInput {
    anno: AnnoR,
    in_edge: DeclId,
    refs: Vec<ReferenceInput>,
}
impl ArcInput {
    pub fn store(self, ir: &mut Ir) -> ArcRec {
        ArcRec {
            anno: ir.store_annotation(self.anno),
            in_edge: self.in_edge,
            refs: ReferenceInput::store_all(self.refs, ir),
        }
    }
}
