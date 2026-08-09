//! R3 exit-criteria gate: GeoSPARQL scoped capability (L9 / ADR-0005).
//!
//! Gate assertions (PLAN §6 R3, L9 §7 R3-04):
//! 1. `geof:distance` — haversine great-circle distance (metre/kilometre);
//! 2. `geof:sf*` topology — the §4.5 Point/Rect table (within/intersects/
//!    contains/disjoint/touches/overlaps/equals);
//! 3. `geof:envelope` / `geof:getSRID` — WKT envelope + SRID 4326;
//! 4. `geo:asWKT` / `geo:asGeoJSON` / `geo:hasGeometry` property functions
//!    (forward direction) and stored-triple fallback non-interference;
//! 5. determinism — the same query executed twice produces identical bytes;
//! 6. deterministic errors — non-geometry inputs yield unbound, not failure.

use ontolith_core::domain::{Iri, LiteralValue};
use ontolith_query::domain::{BoundValue, QueryRequest, QueryResult, Solution};
use ontolith_query::infrastructure::standard_pipeline_with_dictionary;
use ontolith_rdf::domain::{Term, Triple};
use ontolith_storage::application::{DictionaryCodec, StorageEngine, TripleRepository};
use ontolith_storage::infrastructure::{
    InMemoryDictionary, InMemoryStorageEngine, InMemoryTripleRepository,
};
use ontolith_transaction::domain::TxnId;
use std::sync::Arc;

const GEO: &str = "http://www.opengis.net/ont/geosparql#";
const WKT: &str = "http://www.opengis.net/ont/geosparql#wktLiteral";
const FUNC: &str = "http://www.opengis.net/def/function/geosparql/";
const UOM: &str = "http://www.opengis.net/def/uom/OGC/1.0/";

fn seed_repo() -> (
    Arc<dyn TripleRepository>,
    Arc<dyn StorageEngine>,
    Arc<dyn DictionaryCodec>,
) {
    let engine = Arc::new(InMemoryStorageEngine::new());
    let repo: Arc<dyn TripleRepository> =
        Arc::new(InMemoryTripleRepository::new(Arc::clone(&engine)));
    let dict: Arc<dyn DictionaryCodec> = Arc::new(InMemoryDictionary::new());
    let txn = TxnId::new(1);
    let wkt = |v: &str| {
        Term::Literal(LiteralValue::Typed {
            value: v.into(),
            datatype: Iri::new(WKT),
        })
    };
    let put = |s: &str, p: &str, o: Term| {
        let node = dict.encode_node(s);
        repo.insert(txn, Triple::new(node, Iri::new(p), o)).unwrap();
    };
    put(
        "http://ex.org/london",
        &format!("{GEO}hasGeometry"),
        wkt("POINT (-0.1276 51.5074)"),
    );
    put(
        "http://ex.org/paris",
        &format!("{GEO}hasGeometry"),
        wkt("POINT (2.3522 48.8566)"),
    );
    put(
        "http://ex.org/ukbox",
        &format!("{GEO}hasGeometry"),
        wkt("POLYGON((-6 50,2 50,2 53,-6 53,-6 50))"),
    );
    // Explicitly stored geo:asWKT triple — must keep working through the
    // normal store path when the subject is unbound (no rewrite interference).
    put(
        "http://ex.org/stored",
        &format!("{GEO}asWKT"),
        wkt("POINT (9 8)"),
    );
    engine.commit_transaction(txn).unwrap();
    (repo, engine, dict)
}

fn run(repo: Arc<dyn TripleRepository>, dict: Arc<dyn DictionaryCodec>, q: &str) -> QueryResult {
    standard_pipeline_with_dictionary(repo, dict)
        .execute(&QueryRequest::new(q.to_owned()))
        .unwrap_or_else(|e| panic!("query failed: {e}\n---\n{q}"))
}

fn literal_of(row: &Solution, var: &str) -> LiteralValue {
    row.bindings
        .get(var)
        .and_then(|b| match b {
            BoundValue::Literal(l) => Some(l.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{var} not a literal in {row:?}"))
}

fn double_of(row: &Solution, var: &str) -> f64 {
    match literal_of(row, var) {
        LiteralValue::Double(v) => v,
        other => panic!("{var} not a double: {other:?}"),
    }
}

fn bool_of(row: &Solution, var: &str) -> bool {
    match literal_of(row, var) {
        LiteralValue::Boolean(v) => v,
        other => panic!("{var} not a boolean: {other:?}"),
    }
}

fn string_of(row: &Solution, var: &str) -> String {
    literal_of(row, var).lexical_form()
}

#[test]
fn geof_distance_haversine() {
    let (repo, _engine, dict) = seed_repo();
    let q = format!(
        "PREFIX geof: <{FUNC}> PREFIX geo: <{GEO}> \
         SELECT ?d WHERE {{ \
           BIND(geof:distance(\"POINT (-0.1276 51.5074)\"^^geo:wktLiteral, \
                \"POINT (2.3522 48.8566)\"^^geo:wktLiteral, <{UOM}metre>) AS ?d) }}"
    );
    let r = run(repo.clone(), dict.clone(), &q);
    assert_eq!(r.solutions.len(), 1);
    let d = double_of(&r.solutions[0], "d");
    assert!(
        (343_000.0..344_500.0).contains(&d),
        "distance {d} out of range"
    );
    // Default units (no third argument) are metres.
    let q2 = format!(
        "PREFIX geof: <{FUNC}> PREFIX geo: <{GEO}> \
         SELECT ?d WHERE {{ \
           BIND(geof:distance(\"POINT (-0.1276 51.5074)\"^^geo:wktLiteral, \
                \"POINT (2.3522 48.8566)\"^^geo:wktLiteral) AS ?d) }}"
    );
    let r2 = run(repo.clone(), dict.clone(), &q2);
    assert!((double_of(&r2.solutions[0], "d") - d).abs() < f64::EPSILON);
    // Kilometres = metres / 1000.
    let q3 = format!(
        "PREFIX geof: <{FUNC}> PREFIX geo: <{GEO}> \
         SELECT ?d WHERE {{ \
           BIND(geof:distance(\"POINT (-0.1276 51.5074)\"^^geo:wktLiteral, \
                \"POINT (2.3522 48.8566)\"^^geo:wktLiteral, <{UOM}kilometre>) AS ?d) }}"
    );
    let r3 = run(repo, dict, &q3);
    assert!((double_of(&r3.solutions[0], "d") * 1000.0 - d).abs() < 1e-6);
}

#[test]
fn geof_topology_point_rect() {
    let (repo, _engine, dict) = seed_repo();
    let inside = "POINT (0 51)";
    let boundary = "POINT (-6 51)";
    let outside = "POINT (-10 51)";
    let rect = "POLYGON((-6 50,2 50,2 53,-6 53,-6 50))";
    for (point, within, intersects, touches) in [
        (inside, true, true, false),
        (boundary, false, true, true),
        (outside, false, false, false),
    ] {
        let q = format!(
            "PREFIX geof: <{FUNC}> PREFIX geo: <{GEO}> \
             SELECT ?w ?i ?t WHERE {{ \
               BIND(geof:sfWithin(\"{point}\"^^geo:wktLiteral, \"{rect}\"^^geo:wktLiteral) AS ?w) \
               BIND(geof:sfIntersects(\"{point}\"^^geo:wktLiteral, \"{rect}\"^^geo:wktLiteral) AS ?i) \
               BIND(geof:sfTouches(\"{point}\"^^geo:wktLiteral, \"{rect}\"^^geo:wktLiteral) AS ?t) }}"
        );
        let r = run(Arc::clone(&repo), Arc::clone(&dict), &q);
        assert_eq!(r.solutions.len(), 1, "point {point}");
        assert_eq!(bool_of(&r.solutions[0], "w"), within, "within {point}");
        assert_eq!(
            bool_of(&r.solutions[0], "i"),
            intersects,
            "intersects {point}"
        );
        assert_eq!(bool_of(&r.solutions[0], "t"), touches, "touches {point}");
    }
    // Contains / disjoint / overlaps / equals / crosses.
    let q = format!(
        "PREFIX geof: <{FUNC}> PREFIX geo: <{GEO}> \
         SELECT ?c ?d ?o ?e ?x WHERE {{ \
           BIND(geof:sfContains(\"{rect}\"^^geo:wktLiteral, \"{inside}\"^^geo:wktLiteral) AS ?c) \
           BIND(geof:sfDisjoint(\"{rect}\"^^geo:wktLiteral, \"{outside}\"^^geo:wktLiteral) AS ?d) \
           BIND(geof:sfOverlaps(\"{rect}\"^^geo:wktLiteral, \
                \"POLYGON((0 49,4 49,4 54,0 54,0 49))\"^^geo:wktLiteral) AS ?o) \
           BIND(geof:sfEquals(\"{rect}\"^^geo:wktLiteral, \"{rect}\"^^geo:wktLiteral) AS ?e) \
           BIND(geof:sfCrosses(\"{rect}\"^^geo:wktLiteral, \"{inside}\"^^geo:wktLiteral) AS ?x) }}"
    );
    let r = run(repo, dict, &q);
    assert_eq!(r.solutions.len(), 1);
    assert!(bool_of(&r.solutions[0], "c"));
    assert!(bool_of(&r.solutions[0], "d"));
    assert!(bool_of(&r.solutions[0], "o"));
    assert!(bool_of(&r.solutions[0], "e"));
    assert!(!bool_of(&r.solutions[0], "x"));
}

#[test]
fn geof_envelope_and_srid() {
    let (repo, _engine, dict) = seed_repo();
    let q = format!(
        "PREFIX geof: <{FUNC}> PREFIX geo: <{GEO}> \
         SELECT ?e ?s ?i ?v WHERE {{ \
           BIND(geof:envelope(\"POINT (1 2)\"^^geo:wktLiteral) AS ?e) \
           BIND(geof:getSRID(\"POINT (1 2)\"^^geo:wktLiteral) AS ?s) \
           BIND(geof:isSimple(\"POINT (1 2)\"^^geo:wktLiteral) AS ?i) \
           BIND(geof:isValid(\"POINT (1 2)\"^^geo:wktLiteral) AS ?v) }}"
    );
    let r = run(repo, dict, &q);
    assert_eq!(r.solutions.len(), 1);
    let e = literal_of(&r.solutions[0], "e");
    match &e {
        LiteralValue::Typed { value, datatype } => {
            assert_eq!(value, "ENVELOPE (1 2 1 2)");
            assert_eq!(datatype.as_str(), WKT);
        }
        other => panic!("envelope not typed literal: {other:?}"),
    }
    assert_eq!(
        literal_of(&r.solutions[0], "s"),
        LiteralValue::Integer(4326)
    );
    assert!(bool_of(&r.solutions[0], "i"));
    assert!(bool_of(&r.solutions[0], "v"));
}

#[test]
fn geo_property_functions_and_store_fallback() {
    let (repo, _engine, dict) = seed_repo();
    // Forward property functions: subject bound via hasGeometry.
    let q = format!(
        "PREFIX geo: <{GEO}> \
         SELECT ?w ?j ?h WHERE {{ \
           <http://ex.org/london> geo:hasGeometry ?g . \
           ?g geo:asWKT ?w . ?g geo:asGeoJSON ?j . ?g geo:hasGeometry ?h }}"
    );
    let r = run(repo.clone(), dict.clone(), &q);
    assert_eq!(r.solutions.len(), 1);
    assert_eq!(string_of(&r.solutions[0], "w"), "POINT (-0.1276 51.5074)");
    assert_eq!(
        string_of(&r.solutions[0], "j"),
        r#"{"type":"Point","coordinates":[-0.1276,51.5074]}"#
    );
    // hasGeometry is the identity mapping on the geometry literal.
    assert_eq!(string_of(&r.solutions[0], "h"), "POINT (-0.1276 51.5074)");
    // Stored geo:asWKT triple still matches via the store when subject unbound.
    let q2 = format!("PREFIX geo: <{GEO}> SELECT ?s ?w WHERE {{ ?s geo:asWKT ?w }}");
    let r2 = run(Arc::clone(&repo), dict, &q2);
    assert_eq!(r2.solutions.len(), 1, "stored asWKT must match exactly one");
    assert_eq!(string_of(&r2.solutions[0], "w"), "POINT (9 8)");
}

#[test]
fn geosparql_determinism_and_errors() {
    let (repo, _engine, dict) = seed_repo();
    let q = format!(
        "PREFIX geof: <{FUNC}> PREFIX geo: <{GEO}> \
         SELECT ?g ?d WHERE {{ \
           <http://ex.org/london> geo:hasGeometry ?g . \
           BIND(geof:distance(?g, ?g) AS ?d) }}"
    );
    let a = run(Arc::clone(&repo), Arc::clone(&dict), &q);
    let b = run(Arc::clone(&repo), Arc::clone(&dict), &q);
    let bytes = |r: &QueryResult| {
        let mut v: Vec<String> = r.solutions.iter().map(|s| format!("{s:?}")).collect();
        v.sort();
        v.join("\n")
    };
    assert_eq!(bytes(&a), bytes(&b), "same query must be byte-identical");
    assert_eq!(a.solutions.len(), 1);
    assert_eq!(double_of(&a.solutions[0], "d"), 0.0);
    // Non-geometry inputs -> unbound (deterministic error semantics).
    let q_err = format!(
        "PREFIX geof: <{FUNC}> PREFIX geo: <{GEO}> \
         SELECT ?d WHERE {{ \
           BIND(geof:distance(\"not a geometry\"^^geo:wktLiteral, \
                \"POINT (1 2)\"^^geo:wktLiteral) AS ?d) }}"
    );
    let r = run(Arc::clone(&repo), Arc::clone(&dict), &q_err);
    assert_eq!(r.solutions.len(), 1);
    assert!(
        !r.solutions[0].bindings.contains_key("d"),
        "malformed WKT must yield unbound, got {:?}",
        r.solutions[0].bindings.get("d")
    );
    // Unsupported CRS -> unbound.
    let q_crs = format!(
        "PREFIX geof: <{FUNC}> PREFIX geo: <{GEO}> \
         SELECT ?d WHERE {{ \
           BIND(geof:distance(\"<urn:ogc:def:crs:EPSG::3857> POINT (1 2)\"^^geo:wktLiteral, \
                \"POINT (1 2)\"^^geo:wktLiteral) AS ?d) }}"
    );
    let r2 = run(Arc::clone(&repo), Arc::clone(&dict), &q_crs);
    assert_eq!(r2.solutions.len(), 1);
    assert!(!r2.solutions[0].bindings.contains_key("d"));
    // FILTER on an errored function eliminates the row (SPARQL error semantics).
    let q_filter = format!(
        "PREFIX geof: <{FUNC}> PREFIX geo: <{GEO}> \
         SELECT * WHERE {{ \
           BIND(geof:distance(\"not a geometry\"^^geo:wktLiteral, \
                \"POINT (1 2)\"^^geo:wktLiteral) AS ?d) \
           FILTER(?d > 0) }}"
    );
    let r3 = run(repo, dict, &q_filter);
    assert_eq!(r3.solutions.len(), 0);
}
