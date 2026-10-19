//! Optional evidence collected by the sole marshal parser, without decoding instructions.
use crate::object::PyObject;
use std::{collections::BTreeMap, rc::Rc};

#[derive(Clone, Copy, Debug)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug)]
pub struct FieldEvidence {
    pub span: Span,
    pub occurrence: Option<usize>,
}

#[derive(Debug)]
pub struct Occurrence {
    pub tag: u8,
    pub span: Span,
    pub payload: Option<Span>,
    pub definition: usize,
    pub is_reference: bool,
    pub object: Option<Rc<PyObject>>,
    /// Immediate serialized child occurrences; references do not duplicate children.
    pub children: Vec<usize>,
    pub fields: BTreeMap<&'static str, FieldEvidence>,
}

#[derive(Debug)]
pub struct MarshalEvidence {
    pub occurrences: Vec<Occurrence>,
    pub entities: usize,
    pub work: usize,
    pub max_entities: usize,
    pub max_work: usize,
}
