//! WKT parsing for the scoped geometry profile (CRS84 only).

use crate::{CRS84_IRI, GeoError, Geometry};

/// Parse a WKT literal (optionally prefixed with `<CRS>`).
///
/// Supported forms (case-insensitive keywords):
/// - `POINT (x y)`
/// - `POLYGON ((xmin ymin, xmax ymin, xmax ymax, xmin ymax, xmin ymin))`
/// - `ENVELOPE (xmin ymin xmax ymax)` (GeoSPARQL 1.1)
pub fn parse_wkt(input: &str) -> Result<Geometry, GeoError> {
    let mut p = Parser {
        input: input.as_bytes(),
        pos: 0,
    };
    p.skip_ws();
    // Optional CRS prefix: `<iri>`.
    if p.peek() == Some(b'<') {
        let crs = p.parse_angle_iri()?;
        if crs != CRS84_IRI {
            return Err(GeoError::UnsupportedCrs(crs));
        }
        p.skip_ws();
    }
    let keyword = p.parse_keyword()?;
    p.skip_ws();
    p.expect(b'(')?;
    p.skip_ws();
    let geom = match keyword {
        "POINT" => {
            let x = p.parse_number()?;
            let y = p.parse_number()?;
            p.skip_ws();
            p.expect(b')')?;
            Geometry::Point { x, y }
        }
        "POLYGON" => {
            p.expect(b'(')?;
            let ring = p.parse_ring()?;
            p.skip_ws();
            p.expect(b')')?;
            rect_from_ring(&ring)?
        }
        "ENVELOPE" => {
            let xmin = p.parse_number()?;
            let ymin = p.parse_number()?;
            let xmax = p.parse_number()?;
            let ymax = p.parse_number()?;
            p.skip_ws();
            p.expect(b')')?;
            rect(xmin, ymin, xmax, ymax)?
        }
        _ => unreachable!("parse_keyword restricts the set"),
    };
    p.skip_ws();
    if p.pos != p.input.len() {
        return Err(GeoError::MalformedWkt("trailing content after geometry"));
    }
    Ok(geom)
}

fn rect_from_ring(ring: &[(f64, f64)]) -> Result<Geometry, GeoError> {
    if ring.len() != 5 {
        return Err(GeoError::MalformedWkt(
            "rectangle polygon requires exactly 5 points (closed)",
        ));
    }
    if ring[0] != ring[4] {
        return Err(GeoError::MalformedWkt(
            "rectangle polygon ring must be closed (first == last)",
        ));
    }
    let (x0, y0) = ring[0];
    let (x1, y1) = ring[1];
    let (x2, y2) = ring[2];
    let (x3, y3) = ring[3];
    // Axis-aligned rectangle: alternate corners differ in exactly one axis.
    let axis_aligned = (y1 == y0 && x2 == x1 && y3 == y2 && x3 == x0)
        || (x1 == x0 && y2 == y1 && x3 == x2 && y3 == y0);
    if !axis_aligned {
        return Err(GeoError::UnsupportedShape(
            "polygon is not an axis-aligned rectangle",
        ));
    }
    let xmin = x0.min(x2);
    let xmax = x0.max(x2);
    let ymin = y0.min(y2);
    let ymax = y0.max(y2);
    rect(xmin, ymin, xmax, ymax)
}

fn rect(xmin: f64, ymin: f64, xmax: f64, ymax: f64) -> Result<Geometry, GeoError> {
    if !(xmin.is_finite() && ymin.is_finite() && xmax.is_finite() && ymax.is_finite()) {
        return Err(GeoError::MalformedWkt("coordinates must be finite"));
    }
    if xmin > xmax || ymin > ymax {
        return Err(GeoError::MalformedWkt(
            "rect requires xmin <= xmax and ymin <= ymax",
        ));
    }
    Ok(Geometry::Rect {
        xmin,
        ymin,
        xmax,
        ymax,
    })
}

struct Parser<'a> {
    input: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while self
            .input
            .get(self.pos)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.pos += 1;
        }
    }

    fn expect(&mut self, b: u8) -> Result<(), GeoError> {
        self.skip_ws();
        if self.peek() == Some(b) {
            self.pos += 1;
            Ok(())
        } else {
            Err(GeoError::MalformedWkt("unexpected token"))
        }
    }

    fn parse_angle_iri(&mut self) -> Result<String, GeoError> {
        // Assumes peek == '<'.
        self.pos += 1;
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c == b'>' {
                let iri = String::from_utf8(self.input[start..self.pos].to_vec())
                    .map_err(|_| GeoError::MalformedWkt("CRS IRI is not valid UTF-8"))?;
                self.pos += 1;
                return Ok(iri);
            }
            self.pos += 1;
        }
        Err(GeoError::MalformedWkt("unterminated CRS IRI"))
    }

    fn parse_keyword(&mut self) -> Result<&'static str, GeoError> {
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
            self.pos += 1;
        }
        let word = std::str::from_utf8(&self.input[start..self.pos])
            .map_err(|_| GeoError::MalformedWkt("keyword is not valid UTF-8"))?
            .to_ascii_uppercase();
        match word.as_str() {
            "POINT" => Ok("POINT"),
            "POLYGON" => Ok("POLYGON"),
            "ENVELOPE" => Ok("ENVELOPE"),
            "LINESTRING" | "MULTIPOINT" | "MULTILINESTRING" | "MULTIPOLYGON"
            | "GEOMETRYCOLLECTION" | "CIRCULARSTRING" | "CURVE" | "SURFACE" | "TIN"
            | "POLYHEDRALSURFACE" => Err(GeoError::UnsupportedShape(
                "WKT type outside the Point/Rect profile",
            )),
            _ => Err(GeoError::MalformedWkt(
                "expected geometry keyword (POINT/POLYGON/ENVELOPE)",
            )),
        }
    }

    fn parse_ring(&mut self) -> Result<Vec<(f64, f64)>, GeoError> {
        let mut ring = Vec::new();
        loop {
            let x = self.parse_number()?;
            let y = self.parse_number()?;
            ring.push((x, y));
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b')') => {
                    self.pos += 1;
                    return Ok(ring);
                }
                _ => return Err(GeoError::MalformedWkt("expected ',' or ')' in ring")),
            }
        }
    }

    fn parse_number(&mut self) -> Result<f64, GeoError> {
        self.skip_ws();
        let start = self.pos;
        if self.peek() == Some(b'-') || self.peek() == Some(b'+') {
            self.pos += 1;
        }
        let mut digits = 0;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
            digits += 1;
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.pos += 1;
                digits += 1;
            }
        }
        if digits == 0 {
            return Err(GeoError::MalformedWkt("expected a number"));
        }
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'-') | Some(b'+')) {
                self.pos += 1;
            }
            let mut exp_digits = 0;
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.pos += 1;
                exp_digits += 1;
            }
            if exp_digits == 0 {
                return Err(GeoError::MalformedWkt("exponent requires digits"));
            }
        }
        let text = std::str::from_utf8(&self.input[start..self.pos])
            .map_err(|_| GeoError::MalformedWkt("number is not valid UTF-8"))?;
        let value: f64 = text
            .parse()
            .map_err(|_| GeoError::MalformedWkt("invalid number"))?;
        if !value.is_finite() {
            return Err(GeoError::MalformedWkt("coordinates must be finite"));
        }
        Ok(value)
    }
}
