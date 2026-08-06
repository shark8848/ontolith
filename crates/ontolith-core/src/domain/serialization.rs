//! Deterministic Knowledge Object serialization (SAS-0401 Part II).
//!
//! Dependency-free binary codec with round-trip fidelity for the Knowledge
//! Object containers defined in [`crate::domain::knowledge`] and
//! [`crate::domain::identity`]. The encoding is length-prefixed and
//! deterministic so identical objects always produce identical bytes,
//! supporting export, replication, and migration checksums.

use crate::domain::identity::{
    KnowledgeObjectHeader, ObjectId, ObjectState, ObjectType, ObjectVersion, VersionId,
};
use crate::domain::knowledge::{
    DatasetObject, GraphId, GraphObject, GraphStatistics, ObjectMetadata, OntologyObject,
    RuleObject, VersionObject,
};
use crate::domain::resource::Iri;
use crate::error::OntolithError;

/// Incremental byte encoder.
#[derive(Debug, Default)]
pub struct Encoder {
    buf: Vec<u8>,
}

impl Encoder {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    pub fn write_tag(&mut self, tag: u8) {
        self.buf.push(tag);
    }

    pub fn write_u64(&mut self, value: u64) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.write_u64(bytes.len() as u64);
        self.buf.extend_from_slice(bytes);
    }

    pub fn write_str(&mut self, value: &str) {
        self.write_bytes(value.as_bytes());
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }
}

/// Incremental byte decoder with bounds-checked reads.
#[derive(Debug, Clone)]
pub struct Decoder<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> Decoder<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    pub fn read_tag(&mut self) -> Result<u8, OntolithError> {
        let tag = self.take(1)?[0];
        Ok(tag)
    }

    pub fn read_u64(&mut self) -> Result<u64, OntolithError> {
        let raw = self.take(8)?;
        Ok(u64::from_le_bytes(raw.try_into().expect("8 bytes")))
    }

    pub fn read_bytes(&mut self) -> Result<&'a [u8], OntolithError> {
        let len = self.read_u64()? as usize;
        if len > self.remaining() {
            return Err(OntolithError::Failed(
                "serialization: length prefix exceeds remaining bytes".into(),
            ));
        }
        let start = self.cursor;
        self.cursor += len;
        Ok(&self.bytes[start..start + len])
    }

    pub fn read_str(&mut self) -> Result<&'a str, OntolithError> {
        let raw = self.read_bytes()?;
        std::str::from_utf8(raw)
            .map_err(|_| OntolithError::Failed("serialization: invalid utf-8 string".into()))
    }

    pub fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.cursor)
    }

    pub fn finish(self) -> Result<(), OntolithError> {
        if self.remaining() != 0 {
            return Err(OntolithError::Failed(
                "serialization: trailing bytes after object".into(),
            ));
        }
        Ok(())
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], OntolithError> {
        if self.remaining() < len {
            return Err(OntolithError::Failed(
                "serialization: truncated input".into(),
            ));
        }
        let start = self.cursor;
        self.cursor += len;
        Ok(&self.bytes[start..start + len])
    }
}

/// Deterministic codec for Knowledge Object containers.
pub trait KoCodec: Sized {
    fn encode_into(&self, enc: &mut Encoder);

    fn decode_from(dec: &mut Decoder<'_>) -> Result<Self, OntolithError>;
}

/// Serialize a Knowledge Object container to deterministic bytes.
pub fn encode_ko<T: KoCodec>(value: &T) -> Vec<u8> {
    let mut enc = Encoder::new();
    value.encode_into(&mut enc);
    enc.into_bytes()
}

/// Deserialize a Knowledge Object container, rejecting trailing bytes.
pub fn decode_ko<T: KoCodec>(bytes: &[u8]) -> Result<T, OntolithError> {
    let mut dec = Decoder::new(bytes);
    let value = T::decode_from(&mut dec)?;
    dec.finish()?;
    Ok(value)
}

fn decode_str(dec: &mut Decoder<'_>) -> Result<String, OntolithError> {
    Ok(dec.read_str()?.to_owned())
}

fn encode_iri(enc: &mut Encoder, iri: &Iri) {
    enc.write_str(iri.as_str());
}

fn decode_iri(dec: &mut Decoder<'_>) -> Result<Iri, OntolithError> {
    Iri::parse(decode_str(dec)?)
}

impl From<std::str::Utf8Error> for OntolithError {
    fn from(_: std::str::Utf8Error) -> Self {
        OntolithError::Failed("serialization: invalid utf-8 string".into())
    }
}

impl KoCodec for ObjectId {
    fn encode_into(&self, enc: &mut Encoder) {
        enc.write_str(self.as_str());
    }

    fn decode_from(dec: &mut Decoder<'_>) -> Result<Self, OntolithError> {
        let value = decode_str(dec)?;
        Self::new(value).map_err(|_| OntolithError::Failed("invalid object id".into()))
    }
}

impl KoCodec for ObjectType {
    fn encode_into(&self, enc: &mut Encoder) {
        let tag: u8 = match self {
            Self::Resource => 0,
            Self::Statement => 1,
            Self::Graph => 2,
            Self::Dataset => 3,
            Self::Ontology => 4,
            Self::Rule => 5,
            Self::Version => 6,
            Self::Metadata => 7,
        };
        enc.write_tag(tag);
    }

    fn decode_from(dec: &mut Decoder<'_>) -> Result<Self, OntolithError> {
        let tag = dec.read_tag()?;
        match tag {
            0 => Ok(Self::Resource),
            1 => Ok(Self::Statement),
            2 => Ok(Self::Graph),
            3 => Ok(Self::Dataset),
            4 => Ok(Self::Ontology),
            5 => Ok(Self::Rule),
            6 => Ok(Self::Version),
            7 => Ok(Self::Metadata),
            _ => Err(OntolithError::Failed(format!(
                "serialization: unknown object type tag {tag}"
            ))),
        }
    }
}

impl KoCodec for ObjectState {
    fn encode_into(&self, enc: &mut Encoder) {
        let tag: u8 = match self {
            Self::Created => 0,
            Self::Persisted => 1,
            Self::Indexed => 2,
            Self::Replicated => 3,
            Self::Versioned => 4,
            Self::Archived => 5,
            Self::Deleted => 6,
        };
        enc.write_tag(tag);
    }

    fn decode_from(dec: &mut Decoder<'_>) -> Result<Self, OntolithError> {
        let tag = dec.read_tag()?;
        match tag {
            0 => Ok(Self::Created),
            1 => Ok(Self::Persisted),
            2 => Ok(Self::Indexed),
            3 => Ok(Self::Replicated),
            4 => Ok(Self::Versioned),
            5 => Ok(Self::Archived),
            6 => Ok(Self::Deleted),
            _ => Err(OntolithError::Failed(format!(
                "serialization: unknown object state tag {tag}"
            ))),
        }
    }
}

impl KoCodec for ObjectVersion {
    fn encode_into(&self, enc: &mut Encoder) {
        enc.write_u64(self.get());
    }

    fn decode_from(dec: &mut Decoder<'_>) -> Result<Self, OntolithError> {
        Ok(Self::new(dec.read_u64()?))
    }
}

impl KoCodec for VersionId {
    fn encode_into(&self, enc: &mut Encoder) {
        enc.write_u64(self.get());
    }

    fn decode_from(dec: &mut Decoder<'_>) -> Result<Self, OntolithError> {
        Ok(Self::new(dec.read_u64()?))
    }
}

impl KoCodec for KnowledgeObjectHeader {
    fn encode_into(&self, enc: &mut Encoder) {
        self.id.encode_into(enc);
        self.object_type.encode_into(enc);
        self.version.encode_into(enc);
        enc.write_u64(self.created_at);
        enc.write_u64(self.updated_at);
        self.state.encode_into(enc);
    }

    fn decode_from(dec: &mut Decoder<'_>) -> Result<Self, OntolithError> {
        Ok(Self {
            id: ObjectId::decode_from(dec)?,
            object_type: ObjectType::decode_from(dec)?,
            version: ObjectVersion::decode_from(dec)?,
            created_at: dec.read_u64()?,
            updated_at: dec.read_u64()?,
            state: ObjectState::decode_from(dec)?,
        })
    }
}

impl KoCodec for ObjectMetadata {
    fn encode_into(&self, enc: &mut Encoder) {
        // Sort for deterministic encoding regardless of insertion order,
        // mirroring `CanonicalEncode` semantics (SAS-0401 §10).
        let mut pairs = self.labels.clone();
        pairs.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        enc.write_u64(pairs.len() as u64);
        for (key, value) in pairs {
            enc.write_str(&key);
            enc.write_str(&value);
        }
    }

    fn decode_from(dec: &mut Decoder<'_>) -> Result<Self, OntolithError> {
        let count = dec.read_u64()? as usize;
        if count > dec.remaining() {
            return Err(OntolithError::Failed(
                "serialization: metadata count exceeds remaining bytes".into(),
            ));
        }
        let mut labels = Vec::with_capacity(count);
        for _ in 0..count {
            labels.push((decode_str(dec)?, decode_str(dec)?));
        }
        Ok(Self { labels })
    }
}

impl KoCodec for GraphId {
    fn encode_into(&self, enc: &mut Encoder) {
        match self {
            Self::Default => enc.write_tag(0),
            Self::Named(iri) => {
                enc.write_tag(1);
                encode_iri(enc, iri);
            }
        }
    }

    fn decode_from(dec: &mut Decoder<'_>) -> Result<Self, OntolithError> {
        match dec.read_tag()? {
            0 => Ok(Self::Default),
            1 => Ok(Self::Named(decode_iri(dec)?)),
            other => Err(OntolithError::Failed(format!(
                "serialization: unknown graph id tag {other}"
            ))),
        }
    }
}

impl KoCodec for GraphStatistics {
    fn encode_into(&self, enc: &mut Encoder) {
        enc.write_u64(self.triple_count);
        enc.write_u64(self.distinct_subjects);
        enc.write_u64(self.distinct_predicates);
        enc.write_u64(self.distinct_objects);
    }

    fn decode_from(dec: &mut Decoder<'_>) -> Result<Self, OntolithError> {
        Ok(Self {
            triple_count: dec.read_u64()?,
            distinct_subjects: dec.read_u64()?,
            distinct_predicates: dec.read_u64()?,
            distinct_objects: dec.read_u64()?,
        })
    }
}

impl KoCodec for GraphObject {
    fn encode_into(&self, enc: &mut Encoder) {
        self.header.encode_into(enc);
        self.graph_id.encode_into(enc);
        self.metadata.encode_into(enc);
        self.statistics.encode_into(enc);
    }

    fn decode_from(dec: &mut Decoder<'_>) -> Result<Self, OntolithError> {
        Ok(Self {
            header: KnowledgeObjectHeader::decode_from(dec)?,
            graph_id: GraphId::decode_from(dec)?,
            metadata: ObjectMetadata::decode_from(dec)?,
            statistics: GraphStatistics::decode_from(dec)?,
        })
    }
}

impl KoCodec for DatasetObject {
    fn encode_into(&self, enc: &mut Encoder) {
        self.header.encode_into(enc);
        self.default_graph.encode_into(enc);
        self.metadata.encode_into(enc);
        enc.write_u64(self.named_graphs.len() as u64);
        for graph in &self.named_graphs {
            graph.encode_into(enc);
        }
    }

    fn decode_from(dec: &mut Decoder<'_>) -> Result<Self, OntolithError> {
        let header = KnowledgeObjectHeader::decode_from(dec)?;
        let default_graph = GraphObject::decode_from(dec)?;
        let metadata = ObjectMetadata::decode_from(dec)?;
        let count = dec.read_u64()? as usize;
        if count > dec.remaining() {
            return Err(OntolithError::Failed(
                "serialization: named graph count exceeds remaining bytes".into(),
            ));
        }
        let mut named_graphs = Vec::with_capacity(count);
        for _ in 0..count {
            named_graphs.push(GraphObject::decode_from(dec)?);
        }
        Ok(Self {
            header,
            default_graph,
            named_graphs,
            metadata,
        })
    }
}

fn encode_optional_graph_id(enc: &mut Encoder, graph: &Option<GraphId>) {
    match graph {
        Some(graph_id) => {
            enc.write_tag(1);
            graph_id.encode_into(enc);
        }
        None => enc.write_tag(0),
    }
}

fn decode_optional_graph_id(dec: &mut Decoder<'_>) -> Result<Option<GraphId>, OntolithError> {
    match dec.read_tag()? {
        0 => Ok(None),
        1 => Ok(Some(GraphId::decode_from(dec)?)),
        other => Err(OntolithError::Failed(format!(
            "serialization: unknown optional graph tag {other}"
        ))),
    }
}

impl KoCodec for OntologyObject {
    fn encode_into(&self, enc: &mut Encoder) {
        self.dataset.encode_into(enc);
        encode_optional_graph_id(enc, &self.tbox_graph);
        encode_optional_graph_id(enc, &self.abox_graph);
        encode_optional_graph_id(enc, &self.annotation_graph);
        encode_optional_graph_id(enc, &self.rule_graph);
        encode_optional_graph_id(enc, &self.provenance_graph);
    }

    fn decode_from(dec: &mut Decoder<'_>) -> Result<Self, OntolithError> {
        Ok(Self {
            dataset: DatasetObject::decode_from(dec)?,
            tbox_graph: decode_optional_graph_id(dec)?,
            abox_graph: decode_optional_graph_id(dec)?,
            annotation_graph: decode_optional_graph_id(dec)?,
            rule_graph: decode_optional_graph_id(dec)?,
            provenance_graph: decode_optional_graph_id(dec)?,
        })
    }
}

impl KoCodec for RuleObject {
    fn encode_into(&self, enc: &mut Encoder) {
        self.header.encode_into(enc);
        match &self.rule_iri {
            Some(iri) => {
                enc.write_tag(1);
                encode_iri(enc, iri);
            }
            None => enc.write_tag(0),
        }
        enc.write_str(&self.label);
    }

    fn decode_from(dec: &mut Decoder<'_>) -> Result<Self, OntolithError> {
        let header = KnowledgeObjectHeader::decode_from(dec)?;
        let rule_iri = match dec.read_tag()? {
            0 => None,
            1 => Some(decode_iri(dec)?),
            other => {
                return Err(OntolithError::Failed(format!(
                    "serialization: unknown rule iri tag {other}"
                )));
            }
        };
        let label = decode_str(dec)?;
        Ok(Self {
            header,
            rule_iri,
            label,
        })
    }
}

impl KoCodec for VersionObject {
    fn encode_into(&self, enc: &mut Encoder) {
        self.header.encode_into(enc);
        self.target_id.encode_into(enc);
        self.target_version.encode_into(enc);
        match self.parent_version {
            Some(version) => {
                enc.write_tag(1);
                version.encode_into(enc);
            }
            None => enc.write_tag(0),
        }
    }

    fn decode_from(dec: &mut Decoder<'_>) -> Result<Self, OntolithError> {
        let header = KnowledgeObjectHeader::decode_from(dec)?;
        let target_id = ObjectId::decode_from(dec)?;
        let target_version = ObjectVersion::decode_from(dec)?;
        let parent_version = match dec.read_tag()? {
            0 => None,
            1 => Some(ObjectVersion::decode_from(dec)?),
            other => {
                return Err(OntolithError::Failed(format!(
                    "serialization: unknown parent version tag {other}"
                )));
            }
        };
        Ok(Self {
            header,
            target_id,
            target_version,
            parent_version,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::identity::{ObjectId, ObjectState};
    use crate::domain::knowledge::{
        DatasetObject, GraphObject, OntologyObject, RuleObject, VersionObject,
    };

    fn sample_header() -> KnowledgeObjectHeader {
        let mut header = KnowledgeObjectHeader::new(
            ObjectId::new("urn:ontolith:test:1").expect("valid id"),
            ObjectType::Graph,
            1000,
        );
        header
            .transition_to(ObjectState::Persisted, 2000)
            .expect("transition");
        header
    }

    fn sample_graph() -> GraphObject {
        let mut graph = GraphObject::new_default(
            ObjectId::new("urn:ontolith:test:g").expect("valid id"),
            1000,
        );
        graph.statistics.triple_count = 42;
        graph.statistics.distinct_subjects = 7;
        graph.metadata.insert("owner", "ontolith");
        graph
    }

    #[test]
    fn header_round_trips() {
        let header = sample_header();
        let bytes = encode_ko(&header);
        let decoded = decode_ko::<KnowledgeObjectHeader>(&bytes).expect("decode");
        assert_eq!(header, decoded);
    }

    #[test]
    fn graph_object_round_trips() {
        let graph = sample_graph();
        let bytes = encode_ko(&graph);
        let decoded = decode_ko::<GraphObject>(&bytes).expect("decode");
        assert_eq!(graph, decoded);
    }

    #[test]
    fn dataset_with_named_graphs_round_trips() {
        let id = ObjectId::new("urn:ontolith:test:ds").expect("valid id");
        let mut dataset = DatasetObject::new(id, 1000).expect("new dataset");
        let named = GraphObject::new_named(
            ObjectId::new("urn:ontolith:test:ds/graph/1").expect("valid id"),
            Iri::parse("urn:graph:1").expect("valid iri"),
            1000,
        );
        dataset.add_named_graph(named).expect("add named graph");
        dataset.metadata.insert("kind", "dataset");
        let bytes = encode_ko(&dataset);
        let decoded = decode_ko::<DatasetObject>(&bytes).expect("decode");
        assert_eq!(dataset, decoded);
        assert_eq!(decoded.graph_count(), 2);
    }

    #[test]
    fn ontology_round_trips() {
        let id = ObjectId::new("urn:ontolith:test:onto").expect("valid id");
        let mut ontology = OntologyObject::new(id, 1000).expect("new ontology");
        ontology.tbox_graph = Some(GraphId::Named(Iri::parse("urn:tbox").expect("valid iri")));
        let bytes = encode_ko(&ontology);
        let decoded = decode_ko::<OntologyObject>(&bytes).expect("decode");
        assert_eq!(ontology, decoded);
    }

    #[test]
    fn rule_and_version_round_trip() {
        let rule = RuleObject::new(
            ObjectId::new("urn:ontolith:test:rule").expect("valid id"),
            "rdfs11",
            1000,
        );
        let bytes = encode_ko(&rule);
        assert_eq!(rule, decode_ko::<RuleObject>(&bytes).expect("decode"));

        let version = VersionObject::new(
            ObjectId::new("urn:ontolith:test:v").expect("valid id"),
            ObjectId::new("urn:ontolith:test:target").expect("valid id"),
            ObjectVersion::new(3),
            Some(ObjectVersion::new(2)),
            1000,
        );
        let bytes = encode_ko(&version);
        assert_eq!(version, decode_ko::<VersionObject>(&bytes).expect("decode"));
    }

    #[test]
    fn encoding_is_deterministic() {
        let graph = sample_graph();
        assert_eq!(encode_ko(&graph), encode_ko(&graph));
        let mut other = sample_graph();
        other.metadata.insert("owner", "ontolith");
        other.metadata.insert("b", "1");
        let mut sorted = sample_graph();
        sorted.metadata.labels.clear();
        sorted.metadata.insert("b", "1");
        sorted.metadata.insert("owner", "ontolith");
        assert_eq!(encode_ko(&sorted), encode_ko(&other));
    }

    #[test]
    fn corrupted_input_is_rejected() {
        let graph = sample_graph();
        let mut bytes = encode_ko(&graph);
        assert!(decode_ko::<GraphObject>(&bytes).is_ok());
        bytes.pop();
        assert!(decode_ko::<GraphObject>(&bytes).is_err());

        let mut truncated = encode_ko(&graph);
        truncated.truncate(truncated.len() / 2);
        assert!(decode_ko::<GraphObject>(&truncated).is_err());

        // Flipping the id-length prefix must be rejected by bounds checks.
        let mut tampered = encode_ko(&graph);
        tampered[0] ^= 0x80;
        assert!(decode_ko::<GraphObject>(&tampered).is_err());
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        let graph = sample_graph();
        let mut bytes = encode_ko(&graph);
        bytes.push(0x00);
        assert!(decode_ko::<GraphObject>(&bytes).is_err());
    }
}
