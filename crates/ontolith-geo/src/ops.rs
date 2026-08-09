//! Geometry operations: haversine distance and the scoped `sf:` topology
//! predicates (Point/Rect closed algebra, DE-9IM dimension rules).

use crate::{EARTH_RADIUS_M, Geometry};

/// Great-circle distance between two points (WGS84, CRS84 degrees).
///
/// Deterministic: fixed mean Earth radius [`EARTH_RADIUS_M`]; any non-point
/// geometry is not representable here (callers validate first).
pub fn haversine_distance(a: Geometry, b: Geometry) -> f64 {
    let (Geometry::Point { x: ax, y: ay }, Geometry::Point { x: bx, y: by }) = (a, b) else {
        panic!("haversine_distance requires two points (caller must validate)");
    };
    let to_rad = |deg: f64| deg.to_radians();
    let dlat = to_rad(by - ay);
    let dlon = to_rad(bx - ax);
    let lat1 = to_rad(ay);
    let lat2 = to_rad(by);
    let sin_lat = (dlat / 2.0).sin();
    let sin_lon = (dlon / 2.0).sin();
    let h = sin_lat * sin_lat + lat1.cos() * lat2.cos() * sin_lon * sin_lon;
    let h = h.clamp(0.0, 1.0);
    2.0 * EARTH_RADIUS_M * h.sqrt().asin()
}

/// `sf:equals` — geometries cover the same set of points.
pub fn sf_equals(a: Geometry, b: Geometry) -> bool {
    match (a, b) {
        (Geometry::Point { x: ax, y: ay }, Geometry::Point { x: bx, y: by }) => {
            ax == bx && ay == by
        }
        (
            Geometry::Point { x, y },
            Geometry::Rect {
                xmin,
                ymin,
                xmax,
                ymax,
            },
        )
        | (
            Geometry::Rect {
                xmin,
                ymin,
                xmax,
                ymax,
            },
            Geometry::Point { x, y },
        ) => xmin == xmax && ymin == ymax && x == xmin && y == ymin,
        (
            Geometry::Rect {
                xmin: ax,
                ymin: ay,
                xmax: a2,
                ymax: a3,
            },
            Geometry::Rect {
                xmin: bx,
                ymin: by,
                xmax: b2,
                ymax: b3,
            },
        ) => ax == bx && ay == by && a2 == b2 && a3 == b3,
    }
}

/// `sf:disjoint` — no common point.
pub fn sf_disjoint(a: Geometry, b: Geometry) -> bool {
    !sf_intersects(a, b)
}

/// `sf:intersects` — at least one common point (boundary included).
pub fn sf_intersects(a: Geometry, b: Geometry) -> bool {
    match (a, b) {
        (Geometry::Point { .. }, Geometry::Point { .. }) => sf_equals(a, b),
        (
            Geometry::Point { x, y },
            Geometry::Rect {
                xmin,
                ymin,
                xmax,
                ymax,
            },
        )
        | (
            Geometry::Rect {
                xmin,
                ymin,
                xmax,
                ymax,
            },
            Geometry::Point { x, y },
        ) => x >= xmin && x <= xmax && y >= ymin && y <= ymax,
        (
            Geometry::Rect {
                xmin: ax,
                ymin: ay,
                xmax: a2,
                ymax: a3,
            },
            Geometry::Rect {
                xmin: bx,
                ymin: by,
                xmax: b2,
                ymax: b3,
            },
        ) => ax <= b2 && bx <= a2 && ay <= b3 && by <= a3,
    }
}

/// `sf:touches` — common boundary points, interiors disjoint.
pub fn sf_touches(a: Geometry, b: Geometry) -> bool {
    match (a, b) {
        (Geometry::Point { .. }, Geometry::Point { .. }) => false,
        (
            Geometry::Point { x, y },
            Geometry::Rect {
                xmin,
                ymin,
                xmax,
                ymax,
            },
        )
        | (
            Geometry::Rect {
                xmin,
                ymin,
                xmax,
                ymax,
            },
            Geometry::Point { x, y },
        ) => {
            sf_intersects(a, b)
                && (x == xmin || x == xmax || y == ymin || y == ymax)
                && !(x > xmin && x < xmax && y > ymin && y < ymax)
        }
        (Geometry::Rect { .. }, Geometry::Rect { .. }) => {
            sf_intersects(a, b) && !sf_overlaps(a, b) && !sf_equals(a, b)
        }
    }
}

/// `sf:crosses` — dimension-1 boundary crossing; never true in the profile.
pub fn sf_crosses(_a: Geometry, _b: Geometry) -> bool {
    false
}

/// `sf:within` — A's interior is a subset of B (boundary-inclusive for
/// equal-dimension containment; a point strictly inside a rectangle).
pub fn sf_within(a: Geometry, b: Geometry) -> bool {
    match (a, b) {
        (Geometry::Point { .. }, Geometry::Point { .. }) => sf_equals(a, b),
        (
            Geometry::Point { x, y },
            Geometry::Rect {
                xmin,
                ymin,
                xmax,
                ymax,
            },
        ) => x > xmin && x < xmax && y > ymin && y < ymax,
        (Geometry::Rect { .. }, Geometry::Point { .. }) => false,
        (
            Geometry::Rect {
                xmin: ax,
                ymin: ay,
                xmax: a2,
                ymax: a3,
            },
            Geometry::Rect {
                xmin: bx,
                ymin: by,
                xmax: b2,
                ymax: b3,
            },
        ) => ax >= bx && a2 <= b2 && ay >= by && a3 <= b3,
    }
}

/// `sf:contains` — the inverse of `sf:within`.
pub fn sf_contains(a: Geometry, b: Geometry) -> bool {
    sf_within(b, a)
}

/// `sf:overlaps` — interiors intersect with equal dimension.
pub fn sf_overlaps(a: Geometry, b: Geometry) -> bool {
    match (a, b) {
        (Geometry::Point { .. }, Geometry::Point { .. }) => false,
        (Geometry::Point { .. }, Geometry::Rect { .. })
        | (Geometry::Rect { .. }, Geometry::Point { .. }) => false,
        (
            Geometry::Rect {
                xmin: ax,
                ymin: ay,
                xmax: a2,
                ymax: a3,
            },
            Geometry::Rect {
                xmin: bx,
                ymin: by,
                xmax: b2,
                ymax: b3,
            },
        ) => ax < b2 && bx < a2 && ay < b3 && by < a3,
    }
}
