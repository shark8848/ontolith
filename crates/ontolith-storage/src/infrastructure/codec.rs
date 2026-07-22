//! Backend-agnostic binary codecs for durable storage records.
//!
//! Used by the RocksDB adapter; intentionally free of vendor types.

use crate::domain::{StorageKey, WalPhase, WalRecord, WriteOperation};
use ontolith_core::domain::{Iri, LiteralValue, NodeId};
use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::{Quad, Term, Triple};
use ontolith_transaction::domain::TxnId;

pub fn encode_u64(v: u64) -> [u8; 8] {
    v.to_be_bytes()
}

pub fn decode_u64(bytes: &[u8]) -> Result<u64, OntolithError> {
    if bytes.len() < 8 {
        return Err(OntolithError::Storage("u64 truncated"));
    }
    let mut arr = [0u8; 8];
    arr.copy_from_slice(&bytes[..8]);
    Ok(u64::from_be_bytes(arr))
}

pub fn encode_u128(v: u128) -> [u8; 16] {
    v.to_be_bytes()
}

pub fn decode_u128(bytes: &[u8]) -> Result<u128, OntolithError> {
    if bytes.len() < 16 {
        return Err(OntolithError::Storage("u128 truncated"));
    }
    let mut arr = [0u8; 16];
    arr.copy_from_slice(&bytes[..16]);
    Ok(u128::from_be_bytes(arr))
}

fn put_bytes(buf: &mut Vec<u8>, data: &[u8]) {
    buf.extend_from_slice(&(data.len() as u32).to_be_bytes());
    buf.extend_from_slice(data);
}

fn take_bytes<'a>(input: &'a [u8], off: &mut usize) -> Result<&'a [u8], OntolithError> {
    if *off + 4 > input.len() {
        return Err(OntolithError::Storage("length prefix truncated"));
    }
    let mut len_bytes = [0u8; 4];
    len_bytes.copy_from_slice(&input[*off..*off + 4]);
    let len = u32::from_be_bytes(len_bytes) as usize;
    *off += 4;
    if *off + len > input.len() {
        return Err(OntolithError::Storage("payload truncated"));
    }
    let slice = &input[*off..*off + len];
    *off += len;
    Ok(slice)
}

fn put_str(buf: &mut Vec<u8>, s: &str) {
    put_bytes(buf, s.as_bytes());
}

fn take_str(input: &[u8], off: &mut usize) -> Result<String, OntolithError> {
    let bytes = take_bytes(input, off)?;
    String::from_utf8(bytes.to_vec()).map_err(|_| OntolithError::Storage("utf8"))
}

pub fn encode_term(term: &Term) -> Vec<u8> {
    let mut buf = Vec::new();
    match term {
        Term::Iri(i) => {
            buf.push(1);
            put_str(&mut buf, i.as_str());
        }
        Term::BlankNode(n) => {
            buf.push(2);
            buf.extend_from_slice(&encode_u64(n.get()));
        }
        Term::Literal(LiteralValue::String(s)) => {
            buf.push(3);
            put_str(&mut buf, s);
        }
        Term::Literal(LiteralValue::Integer(v)) => {
            buf.push(4);
            buf.extend_from_slice(&v.to_be_bytes());
        }
        Term::Literal(LiteralValue::Decimal(v)) => {
            buf.push(5);
            buf.extend_from_slice(&v.to_bits().to_be_bytes());
        }
        Term::Literal(LiteralValue::Boolean(v)) => {
            buf.push(6);
            buf.push(u8::from(*v));
        }
    }
    buf
}

pub fn decode_term(input: &[u8], off: &mut usize) -> Result<Term, OntolithError> {
    if *off >= input.len() {
        return Err(OntolithError::Storage("term tag missing"));
    }
    let tag = input[*off];
    *off += 1;
    match tag {
        1 => Ok(Term::Iri(Iri::new(take_str(input, off)?))),
        2 => {
            let id = decode_u64(&input[*off..])?;
            *off += 8;
            Ok(Term::BlankNode(NodeId::new(id)))
        }
        3 => Ok(Term::Literal(LiteralValue::String(take_str(input, off)?))),
        4 => {
            if *off + 8 > input.len() {
                return Err(OntolithError::Storage("i64 truncated"));
            }
            let mut arr = [0u8; 8];
            arr.copy_from_slice(&input[*off..*off + 8]);
            *off += 8;
            Ok(Term::Literal(LiteralValue::Integer(i64::from_be_bytes(
                arr,
            ))))
        }
        5 => {
            if *off + 8 > input.len() {
                return Err(OntolithError::Storage("f64 truncated"));
            }
            let mut arr = [0u8; 8];
            arr.copy_from_slice(&input[*off..*off + 8]);
            *off += 8;
            Ok(Term::Literal(LiteralValue::Decimal(f64::from_bits(
                u64::from_be_bytes(arr),
            ))))
        }
        6 => {
            if *off >= input.len() {
                return Err(OntolithError::Storage("bool truncated"));
            }
            let v = input[*off] != 0;
            *off += 1;
            Ok(Term::Literal(LiteralValue::Boolean(v)))
        }
        _ => Err(OntolithError::Storage("unknown term tag")),
    }
}

pub fn encode_triple(t: &Triple) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&encode_u64(t.subject.get()));
    put_str(&mut buf, t.predicate.as_str());
    buf.extend_from_slice(&encode_term(&t.object));
    buf
}

pub fn decode_triple(input: &[u8]) -> Result<Triple, OntolithError> {
    let mut off = 0;
    let subject = NodeId::new(decode_u64(&input[off..])?);
    off += 8;
    let predicate = Iri::new(take_str(input, &mut off)?);
    let object = decode_term(input, &mut off)?;
    Ok(Triple {
        subject,
        predicate,
        object,
    })
}

pub fn encode_quad(q: &Quad) -> Vec<u8> {
    let mut buf = encode_triple(&q.triple);
    match &q.graph_name {
        None => buf.push(0),
        Some(g) => {
            buf.push(1);
            put_str(&mut buf, g.as_str());
        }
    }
    buf
}

pub fn decode_quad(input: &[u8]) -> Result<Quad, OntolithError> {
    // Triple encoding is variable length; decode_triple needs full buffer without graph.
    // Re-parse with offset tracking.
    let mut off = 0;
    let subject = NodeId::new(decode_u64(&input[off..])?);
    off += 8;
    let predicate = Iri::new(take_str(input, &mut off)?);
    let object = decode_term(input, &mut off)?;
    if off >= input.len() {
        return Err(OntolithError::Storage("quad graph flag missing"));
    }
    let flag = input[off];
    off += 1;
    let graph_name = match flag {
        0 => None,
        1 => Some(Iri::new(take_str(input, &mut off)?)),
        _ => return Err(OntolithError::Storage("bad graph flag")),
    };
    Ok(Quad {
        triple: Triple {
            subject,
            predicate,
            object,
        },
        graph_name,
    })
}

pub fn encode_write_op(op: &WriteOperation) -> Vec<u8> {
    let mut buf = Vec::new();
    match op {
        WriteOperation::PutTriple(t) => {
            buf.push(1);
            put_bytes(&mut buf, &encode_triple(t));
        }
        WriteOperation::PutQuad(q) => {
            buf.push(2);
            put_bytes(&mut buf, &encode_quad(q));
        }
        WriteOperation::DeleteTriple(t) => {
            buf.push(3);
            put_bytes(&mut buf, &encode_triple(t));
        }
        WriteOperation::DeleteQuad(q) => {
            buf.push(4);
            put_bytes(&mut buf, &encode_quad(q));
        }
        WriteOperation::DeleteKey(k) => {
            buf.push(5);
            put_str(&mut buf, k.index);
            buf.extend_from_slice(&(k.components.len() as u32).to_be_bytes());
            for c in &k.components {
                buf.extend_from_slice(&encode_u64(c.get()));
            }
        }
    }
    buf
}

pub fn decode_write_op(input: &[u8]) -> Result<WriteOperation, OntolithError> {
    if input.is_empty() {
        return Err(OntolithError::Storage("empty write op"));
    }
    let tag = input[0];
    let mut off = 1;
    match tag {
        1 => {
            let raw = take_bytes(input, &mut off)?;
            Ok(WriteOperation::PutTriple(decode_triple(raw)?))
        }
        2 => {
            let raw = take_bytes(input, &mut off)?;
            Ok(WriteOperation::PutQuad(decode_quad(raw)?))
        }
        3 => {
            let raw = take_bytes(input, &mut off)?;
            Ok(WriteOperation::DeleteTriple(decode_triple(raw)?))
        }
        4 => {
            let raw = take_bytes(input, &mut off)?;
            Ok(WriteOperation::DeleteQuad(decode_quad(raw)?))
        }
        5 => {
            let index = take_str(input, &mut off)?;
            // Leak static index names we know; otherwise store as spo fallback via Box leak for tests only.
            let index: &'static str = match index.as_str() {
                "spo" => "spo",
                "pos" => "pos",
                "osp" => "osp",
                "sop" => "sop",
                "pso" => "pso",
                "ops" => "ops",
                other => Box::leak(other.to_owned().into_boxed_str()),
            };
            if off + 4 > input.len() {
                return Err(OntolithError::Storage("components len truncated"));
            }
            let mut lb = [0u8; 4];
            lb.copy_from_slice(&input[off..off + 4]);
            off += 4;
            let n = u32::from_be_bytes(lb) as usize;
            let mut components = Vec::with_capacity(n);
            for _ in 0..n {
                let id = decode_u64(&input[off..])?;
                off += 8;
                components.push(NodeId::new(id));
            }
            Ok(WriteOperation::DeleteKey(StorageKey { index, components }))
        }
        _ => Err(OntolithError::Storage("unknown write op tag")),
    }
}

pub fn encode_wal_record(rec: &WalRecord) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&encode_u128(rec.txn_id.0));
    buf.push(match rec.phase {
        WalPhase::Staged => 1,
        WalPhase::Committed => 2,
        WalPhase::Aborted => 3,
    });
    buf.extend_from_slice(&(rec.operation_count as u32).to_be_bytes());
    buf.extend_from_slice(&(rec.operations.len() as u32).to_be_bytes());
    for op in &rec.operations {
        put_bytes(&mut buf, &encode_write_op(op));
    }
    buf
}

pub fn decode_wal_record(input: &[u8]) -> Result<WalRecord, OntolithError> {
    let mut off = 0;
    let txn = TxnId::new(decode_u128(&input[off..])?);
    off += 16;
    if off >= input.len() {
        return Err(OntolithError::Storage("wal phase missing"));
    }
    let phase = match input[off] {
        1 => WalPhase::Staged,
        2 => WalPhase::Committed,
        3 => WalPhase::Aborted,
        _ => return Err(OntolithError::Storage("bad wal phase")),
    };
    off += 1;
    if off + 8 > input.len() {
        return Err(OntolithError::Storage("wal counts truncated"));
    }
    let mut c1 = [0u8; 4];
    c1.copy_from_slice(&input[off..off + 4]);
    off += 4;
    let operation_count = u32::from_be_bytes(c1) as usize;
    let mut c2 = [0u8; 4];
    c2.copy_from_slice(&input[off..off + 4]);
    off += 4;
    let n = u32::from_be_bytes(c2) as usize;
    let mut operations = Vec::with_capacity(n);
    for _ in 0..n {
        let raw = take_bytes(input, &mut off)?;
        operations.push(decode_write_op(raw)?);
    }
    Ok(WalRecord {
        txn_id: txn,
        phase,
        operation_count,
        operations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ontolith_rdf::domain::Term;

    #[test]
    fn triple_roundtrip() {
        let t = Triple {
            subject: NodeId::new(9),
            predicate: Iri::new("urn:p"),
            object: Term::Literal(LiteralValue::Integer(42)),
        };
        let enc = encode_triple(&t);
        let dec = decode_triple(&enc).unwrap();
        assert_eq!(t, dec);
    }

    #[test]
    fn wal_record_roundtrip() {
        let rec = WalRecord {
            txn_id: TxnId::new(7),
            phase: WalPhase::Staged,
            operation_count: 1,
            operations: vec![WriteOperation::PutTriple(Triple {
                subject: NodeId::new(1),
                predicate: Iri::new("urn:p"),
                object: Term::Iri(Iri::new("urn:o")),
            })],
        };
        let enc = encode_wal_record(&rec);
        let dec = decode_wal_record(&enc).unwrap();
        assert_eq!(rec, dec);
    }
}
