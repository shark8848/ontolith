//! Scoped GeoSPARQL geometry profile (R3, ADR-0005 / L9-geosparql).
//!
//! Zero external dependencies; all parsing and topology are implemented
//! in-tree so behavior is byte-deterministic across processes and versions.
//!
//! Supported geometry: `Point` and axis-aligned `Rect` (Rectangle).
//! Serializations: WKT (`geo:wktLiteral`, CRS84) and GeoJSON
//! (`geo:jsonLiteral`, CRS84). Everything outside the profile produces a
//! deterministic [`GeoError`].

mod geojson;
mod ops;
mod wkt;

pub use geojson::parse_geojson;
pub use ops::{
    haversine_distance, sf_contains, sf_crosses, sf_disjoint, sf_equals, sf_intersects,
    sf_overlaps, sf_touches, sf_within,
};
pub use wkt::parse_wkt;

/// Geometry datatype IRIs (GeoSPARQL 1.1 vocabulary).
pub const WKT_LITERAL_IRI: &str = "http://www.opengis.net/ont/geosparql#wktLiteral";
pub const GEOJSON_LITERAL_IRI: &str = "http://www.opengis.net/ont/geosparql#jsonLiteral";
/// GeoSPARQL property-function predicates (BGP rewrite targets, L9 §5).
pub const GEO_AS_WKT: &str = "http://www.opengis.net/ont/geosparql#asWKT";
pub const GEO_AS_GEOJSON: &str = "http://www.opengis.net/ont/geosparql#asGeoJSON";
pub const GEO_HAS_GEOMETRY: &str = "http://www.opengis.net/ont/geosparql#hasGeometry";
/// CRS84 IRI (default for both serializations).
pub const CRS84_IRI: &str = "http://www.opengis.net/def/crs/OGC/1.3/CRS84";
/// EPSG:4326 (WGS84 lat/lon) — the SRID reported for CRS84.
pub const CRS84_SRID: i64 = 4326;
/// WGS84 mean Earth radius in metres (fixed constant for determinism).
pub const EARTH_RADIUS_M: f64 = 6_371_008.8;
/// Unit-of-measure IRIs accepted by `geof:distance`.
pub const UOM_METRE: &str = "http://www.opengis.net/def/uom/OGC/1.0/metre";
pub const UOM_KILOMETRE: &str = "http://www.opengis.net/def/uom/OGC/1.0/kilometre";

/// Scoped geometry: a point or an axis-aligned rectangle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Geometry {
    Point {
        x: f64,
        y: f64,
    },
    Rect {
        xmin: f64,
        ymin: f64,
        xmax: f64,
        ymax: f64,
    },
}

impl Geometry {
    /// Minimum bounding rectangle: a point maps to its degenerate rectangle;
    /// a rectangle maps to itself (L9 §4.2).
    pub fn envelope(&self) -> Geometry {
        match self {
            Geometry::Point { x, y } => Geometry::Rect {
                xmin: *x,
                ymin: *y,
                xmax: *x,
                ymax: *y,
            },
            rect @ Geometry::Rect { .. } => *rect,
        }
    }

    /// WKT serialization (deterministic, CRS84 implied): `POINT (x y)` or
    /// `ENVELOPE (xmin ymin xmax ymax)` (GeoSPARQL 1.1 ENVELOPE extension).
    pub fn as_wkt(&self) -> String {
        match self {
            Geometry::Point { x, y } => format!("POINT ({} {})", fmt_coord(*x), fmt_coord(*y)),
            Geometry::Rect {
                xmin,
                ymin,
                xmax,
                ymax,
            } => format!(
                "ENVELOPE ({} {} {} {})",
                fmt_coord(*xmin),
                fmt_coord(*ymin),
                fmt_coord(*xmax),
                fmt_coord(*ymax)
            ),
        }
    }

    /// GeoJSON serialization (deterministic, CRS84 implied).
    pub fn as_geojson(&self) -> String {
        match self {
            Geometry::Point { x, y } => {
                format!(
                    "{{\"type\":\"Point\",\"coordinates\":[{},{}]}}",
                    fmt_coord(*x),
                    fmt_coord(*y)
                )
            }
            Geometry::Rect {
                xmin,
                ymin,
                xmax,
                ymax,
            } => format!(
                "{{\"type\":\"Polygon\",\"coordinates\":[[[{},{}],[{},{}],[{},{}],[{},{}],[{},{}]]]}}",
                fmt_coord(*xmin),
                fmt_coord(*ymin),
                fmt_coord(*xmax),
                fmt_coord(*ymin),
                fmt_coord(*xmax),
                fmt_coord(*ymax),
                fmt_coord(*xmin),
                fmt_coord(*ymax),
                fmt_coord(*xmin),
                fmt_coord(*ymin),
            ),
        }
    }

    pub fn is_point(&self) -> bool {
        matches!(self, Geometry::Point { .. })
    }
}

/// Deterministic coordinate formatting: Rust's shortest round-trip decimal
/// representation, with exponent forms normalized to uppercase `E`.
pub fn fmt_coord(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value.is_sign_positive() {
            "INF".to_owned()
        } else {
            "-INF".to_owned()
        };
    }
    let simple = format!("{value}");
    if simple.contains(['e', 'E']) {
        // `1e-7` -> `1E-7` (normalized, still valid JSON/WKT number).
        let (mantissa, exp) = simple.split_once(['e', 'E']).expect("has exponent");
        let exp_val: i64 = exp.parse().expect("exponent is numeric");
        format!("{mantissa}E{exp_val}")
    } else {
        simple
    }
}

/// Deterministic parse/semantic error for anything outside the scoped profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GeoError {
    /// WKT tokenization/grammar error with a stable message.
    MalformedWkt(&'static str),
    /// GeoJSON parse error with a stable message.
    MalformedGeoJson(&'static str),
    /// CRS other than CRS84.
    UnsupportedCrs(String),
    /// Geometry shape outside the Point/Rect profile.
    UnsupportedShape(&'static str),
    /// `geof:distance` unit not metre/kilometre.
    UnsupportedUnits(String),
    /// Function applied to an argument that is not a scoped geometry.
    NotAGeometry(&'static str),
}

impl std::fmt::Display for GeoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MalformedWkt(msg) => write!(f, "malformed WKT: {msg}"),
            Self::MalformedGeoJson(msg) => write!(f, "malformed GeoJSON: {msg}"),
            Self::UnsupportedCrs(crs) => write!(f, "unsupported CRS: {crs} (profile: CRS84 only)"),
            Self::UnsupportedShape(msg) => write!(f, "unsupported geometry shape: {msg}"),
            Self::UnsupportedUnits(units) => {
                write!(
                    f,
                    "unsupported distance unit: {units} (profile: metre/kilometre)"
                )
            }
            Self::NotAGeometry(msg) => write!(f, "not a scoped geometry: {msg}"),
        }
    }
}

impl std::error::Error for GeoError {}

/// Normalize a literal into a scoped geometry, dispatching on the datatype IRI.
pub fn geometry_from_literal(value: &str, datatype: &str) -> Result<Geometry, GeoError> {
    match datatype {
        WKT_LITERAL_IRI => parse_wkt(value),
        GEOJSON_LITERAL_IRI => parse_geojson(value),
        _ => Err(GeoError::NotAGeometry(
            "datatype is not geo:wktLiteral/jsonLiteral",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P1: Geometry = Geometry::Point { x: 1.0, y: 2.0 };
    const P2: Geometry = Geometry::Point { x: 3.0, y: 4.0 };
    const R1: Geometry = Geometry::Rect {
        xmin: 0.0,
        ymin: 0.0,
        xmax: 10.0,
        ymax: 10.0,
    };
    const R2: Geometry = Geometry::Rect {
        xmin: 5.0,
        ymin: 5.0,
        xmax: 15.0,
        ymax: 15.0,
    };

    #[test]
    fn wkt_roundtrip() {
        assert_eq!(parse_wkt("POINT (1 2)").unwrap(), P1);
        assert_eq!(parse_wkt("POINT(1 2)").unwrap().as_wkt(), "POINT (1 2)");
        assert_eq!(parse_wkt("POLYGON((0 0,10 0,10 10,0 10,0 0))").unwrap(), R1);
        assert_eq!(
            parse_wkt("ENVELOPE (0 0 10 10)").unwrap().as_wkt(),
            "ENVELOPE (0 0 10 10)"
        );
        // Optional CRS prefix, case-insensitive keywords.
        assert_eq!(parse_wkt(&format!("<{CRS84_IRI}> point(1 2)")).unwrap(), P1);
    }

    #[test]
    fn wkt_deterministic_errors() {
        assert_eq!(
            parse_wkt("POINT (1)").unwrap_err(),
            GeoError::MalformedWkt("expected a number")
        );
        assert_eq!(
            parse_wkt("POLYGON((0 0,5 5,10 0,0 10,0 0))").unwrap_err(),
            GeoError::UnsupportedShape("polygon is not an axis-aligned rectangle")
        );
        assert_eq!(
            parse_wkt("<urn:ogc:def:crs:EPSG::3857> POINT (1 2)").unwrap_err(),
            GeoError::UnsupportedCrs("urn:ogc:def:crs:EPSG::3857".into())
        );
        assert!(matches!(
            parse_wkt("LINESTRING (0 0, 1 1)").unwrap_err(),
            GeoError::UnsupportedShape(_)
        ));
        assert_eq!(
            parse_wkt("not wkt").unwrap_err(),
            GeoError::MalformedWkt("expected geometry keyword (POINT/POLYGON/ENVELOPE)")
        );
    }

    #[test]
    fn geojson_roundtrip() {
        assert_eq!(
            parse_geojson(r#"{"type":"Point","coordinates":[1,2]}"#).unwrap(),
            P1
        );
        assert_eq!(
            parse_geojson(
                r#"{"type":"Polygon","coordinates":[[[0,0],[10,0],[10,10],[0,10],[0,0]]]}"#
            )
            .unwrap(),
            R1
        );
        assert_eq!(
            parse_geojson(r#"{"type":"Point","coordinates":[1,2]}"#)
                .unwrap()
                .as_geojson(),
            r#"{"type":"Point","coordinates":[1,2]}"#
        );
    }

    #[test]
    fn geojson_deterministic_errors() {
        assert_eq!(
            parse_geojson(r#"{"type":"LineString","coordinates":[[0,0],[1,1]]}"#).unwrap_err(),
            GeoError::UnsupportedShape("GeoJSON type must be Point or Polygon")
        );
        assert!(matches!(
            parse_geojson("{not json").unwrap_err(),
            GeoError::MalformedGeoJson(_)
        ));
        assert_eq!(
            parse_geojson(r#"{"type":"Point","coordinates":[1,2],"crs":{"type":"name","properties":{"name":"EPSG:3857"}}}"#).unwrap_err(),
            GeoError::UnsupportedCrs("EPSG:3857".into())
        );
    }

    #[test]
    fn topology_point_point() {
        assert!(sf_equals(P1, P1));
        assert!(!sf_disjoint(P1, P1));
        assert!(sf_intersects(P1, P1));
        assert!(!sf_touches(P1, P1));
        assert!(!sf_crosses(P1, P1));
        assert!(sf_within(P1, P1));
        assert!(sf_contains(P1, P1));
        assert!(!sf_overlaps(P1, P1));
        assert!(sf_disjoint(P1, P2));
    }

    #[test]
    fn topology_point_rect() {
        let inside = Geometry::Point { x: 5.0, y: 5.0 };
        let boundary = Geometry::Point { x: 0.0, y: 5.0 };
        let outside = Geometry::Point { x: -1.0, y: 5.0 };
        assert!(sf_within(inside, R1));
        assert!(sf_intersects(inside, R1));
        assert!(sf_contains(R1, inside));
        assert!(sf_touches(boundary, R1));
        assert!(sf_disjoint(outside, R1));
        assert!(!sf_within(boundary, R1));
    }

    #[test]
    fn topology_rect_rect() {
        assert!(sf_overlaps(R1, R2));
        assert!(sf_intersects(R1, R2));
        assert!(!sf_disjoint(R1, R2));
        assert!(!sf_touches(R1, R2));
        assert!(!sf_within(R2, R1));
        let contained = Geometry::Rect {
            xmin: 1.0,
            ymin: 1.0,
            xmax: 9.0,
            ymax: 9.0,
        };
        assert!(sf_within(contained, R1));
        assert!(sf_contains(R1, contained));
        // Edge-touching rectangles: touches, not overlaps.
        let touching = Geometry::Rect {
            xmin: 10.0,
            ymin: 0.0,
            xmax: 20.0,
            ymax: 10.0,
        };
        assert!(sf_touches(R1, touching));
        assert!(!sf_overlaps(R1, touching));
        assert!(!sf_disjoint(R1, touching));
        assert!(sf_equals(R1, R1));
    }

    #[test]
    fn haversine_known_value() {
        // London -> Paris approximate great-circle distance ~343.5 km.
        let london = Geometry::Point {
            x: -0.1276,
            y: 51.5074,
        };
        let paris = Geometry::Point {
            x: 2.3522,
            y: 48.8566,
        };
        let m = haversine_distance(london, paris);
        assert!((343_000.0..344_500.0).contains(&m), "got {m}");
    }
}
