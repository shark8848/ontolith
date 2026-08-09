//! GeoJSON parsing for the scoped geometry profile (CRS84 only).
//!
//! A minimal, dependency-free JSON parser sufficient for the profile subset:
//! `{"type":"Point|Polygon","coordinates":…,"crs":…}`.

use crate::{GeoError, Geometry};

#[derive(Debug, Clone, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

/// Parse a GeoJSON document into a scoped geometry.
pub fn parse_geojson(input: &str) -> Result<Geometry, GeoError> {
    let mut p = Parser {
        input: input.as_bytes(),
        pos: 0,
    };
    p.skip_ws();
    let value = p.parse_value()?;
    p.skip_ws();
    if p.pos != p.input.len() {
        return Err(GeoError::MalformedGeoJson("trailing content"));
    }
    let obj = match value {
        Json::Obj(fields) => fields,
        _ => {
            return Err(GeoError::MalformedGeoJson(
                "top-level value must be an object",
            ));
        }
    };
    let mut geom_type = None;
    let mut coords = None;
    let mut crs = None;
    for (key, value) in obj {
        match key.as_str() {
            "type" => {
                geom_type = match value {
                    Json::Str(s) => Some(s),
                    _ => return Err(GeoError::MalformedGeoJson("\"type\" must be a string")),
                }
            }
            "coordinates" => coords = Some(value),
            "crs" => {
                crs = match value {
                    Json::Obj(fields) => {
                        let mut name = None;
                        for (k, v) in fields {
                            if k == "properties"
                                && let Json::Obj(props) = v
                            {
                                for (pk, pv) in props {
                                    if pk == "name"
                                        && let Json::Str(s) = pv
                                    {
                                        name = Some(s);
                                    }
                                }
                            }
                        }
                        name
                    }
                    _ => return Err(GeoError::MalformedGeoJson("\"crs\" must be an object")),
                }
            }
            _ => {}
        }
    }
    if let Some(crs) = crs
        && crs != "urn:ogc:def:crs:OGC:1.3:CRS84"
    {
        return Err(GeoError::UnsupportedCrs(crs));
    }
    let geom_type = geom_type.ok_or(GeoError::MalformedGeoJson("missing \"type\""))?;
    let coords = coords.ok_or(GeoError::MalformedGeoJson("missing \"coordinates\""))?;
    match geom_type.as_str() {
        "Point" => {
            let (x, y) = coord_pair(coords)?;
            Ok(Geometry::Point { x, y })
        }
        "Polygon" => polygon_rect(coords),
        _ => Err(GeoError::UnsupportedShape(
            "GeoJSON type must be Point or Polygon",
        )),
    }
}

fn coord_pair(value: Json) -> Result<(f64, f64), GeoError> {
    match value {
        Json::Arr(mut items) => {
            if items.len() != 2 {
                return Err(GeoError::MalformedGeoJson(
                    "coordinates must have exactly 2 numbers",
                ));
            }
            let x = json_num(items.remove(0))?;
            let y = json_num(items.remove(0))?;
            Ok((x, y))
        }
        _ => Err(GeoError::MalformedGeoJson(
            "\"coordinates\" must be an array",
        )),
    }
}

fn json_num(value: Json) -> Result<f64, GeoError> {
    match value {
        Json::Num(n) if n.is_finite() => Ok(n),
        _ => Err(GeoError::MalformedGeoJson(
            "coordinate must be a finite number",
        )),
    }
}

fn polygon_rect(value: Json) -> Result<Geometry, GeoError> {
    let rings = match value {
        Json::Arr(rings) => rings,
        _ => {
            return Err(GeoError::MalformedGeoJson(
                "\"coordinates\" must be an array",
            ));
        }
    };
    if rings.len() != 1 {
        return Err(GeoError::UnsupportedShape(
            "only a single exterior ring is supported",
        ));
    }
    let ring = match rings.into_iter().next().expect("len == 1") {
        Json::Arr(points) => points,
        _ => {
            return Err(GeoError::MalformedGeoJson(
                "ring must be an array of positions",
            ));
        }
    };
    if ring.len() != 5 {
        return Err(GeoError::MalformedGeoJson(
            "rectangle ring requires exactly 5 positions (closed)",
        ));
    }
    let mut pts = Vec::with_capacity(ring.len());
    for pos in ring {
        pts.push(coord_pair(pos)?);
    }
    if pts[0] != pts[4] {
        return Err(GeoError::MalformedGeoJson(
            "rectangle ring must be closed (first == last)",
        ));
    }
    let (x0, y0) = pts[0];
    let (x1, y1) = pts[1];
    let (x2, y2) = pts[2];
    let (x3, y3) = pts[3];
    let axis_aligned = (y1 == y0 && x2 == x1 && y3 == y2 && x3 == x0)
        || (x1 == x0 && y2 == y1 && x3 == x2 && y3 == y0);
    if !axis_aligned {
        return Err(GeoError::UnsupportedShape(
            "polygon is not an axis-aligned rectangle",
        ));
    }
    Ok(Geometry::Rect {
        xmin: x0.min(x2),
        ymin: y0.min(y2),
        xmax: x0.max(x2),
        ymax: y0.max(y2),
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
            .is_some_and(|c| matches!(c, b' ' | b'\t' | b'\n' | b'\r'))
        {
            self.pos += 1;
        }
    }

    fn parse_value(&mut self) -> Result<Json, GeoError> {
        self.skip_ws();
        match self.peek() {
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b'"') => Ok(Json::Str(self.parse_string()?)),
            Some(b't') => self.parse_literal("true", Json::Bool(true)),
            Some(b'f') => self.parse_literal("false", Json::Bool(false)),
            Some(b'n') => self.parse_literal("null", Json::Null),
            Some(b'-') | Some(b'0'..=b'9') => self.parse_number(),
            _ => Err(GeoError::MalformedGeoJson("unexpected token")),
        }
    }

    fn parse_literal(&mut self, word: &str, value: Json) -> Result<Json, GeoError> {
        if self.input[self.pos..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            Ok(value)
        } else {
            Err(GeoError::MalformedGeoJson("invalid literal"))
        }
    }

    fn parse_object(&mut self) -> Result<Json, GeoError> {
        self.pos += 1; // '{'
        self.skip_ws();
        let mut fields = Vec::new();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Json::Obj(fields));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(GeoError::MalformedGeoJson("object key must be a string"));
            }
            let key = self.parse_string()?;
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(GeoError::MalformedGeoJson("expected ':' after object key"));
            }
            self.pos += 1;
            let value = self.parse_value()?;
            fields.push((key, value));
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Json::Obj(fields));
                }
                _ => return Err(GeoError::MalformedGeoJson("expected ',' or '}'")),
            }
        }
    }

    fn parse_array(&mut self) -> Result<Json, GeoError> {
        self.pos += 1; // '['
        self.skip_ws();
        let mut items = Vec::new();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Json::Arr(items));
        }
        loop {
            items.push(self.parse_value()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Json::Arr(items));
                }
                _ => return Err(GeoError::MalformedGeoJson("expected ',' or ']'")),
            }
        }
    }

    fn parse_string(&mut self) -> Result<String, GeoError> {
        self.pos += 1; // '"'
        let mut out = String::new();
        loop {
            let c = self
                .peek()
                .ok_or(GeoError::MalformedGeoJson("unterminated string"))?;
            self.pos += 1;
            match c {
                b'"' => return Ok(out),
                b'\\' => {
                    let esc = self
                        .peek()
                        .ok_or(GeoError::MalformedGeoJson("unterminated escape"))?;
                    self.pos += 1;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{0008}'),
                        b'f' => out.push('\u{000C}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hex = self.read_hex4()?;
                            let ch = char::from_u32(hex)
                                .ok_or(GeoError::MalformedGeoJson("invalid \\u escape"))?;
                            out.push(ch);
                        }
                        _ => return Err(GeoError::MalformedGeoJson("invalid escape")),
                    }
                }
                c if c < 0x20 => return Err(GeoError::MalformedGeoJson("control char in string")),
                c => {
                    // ASCII-only JSON string body (keys/values we need are ASCII);
                    // multi-byte UTF-8 is copied verbatim.
                    let start = self.pos - 1;
                    let mut len = 1;
                    if c >= 0x80 {
                        while self.pos < self.input.len()
                            && !matches!(self.input[self.pos], b'"' | b'\\')
                            && self.input[self.pos] >= 0x80
                        {
                            self.pos += 1;
                            len += 1;
                        }
                    }
                    let text = std::str::from_utf8(&self.input[start..start + len])
                        .map_err(|_| GeoError::MalformedGeoJson("invalid UTF-8"))?;
                    out.push_str(text);
                }
            }
        }
    }

    fn read_hex4(&mut self) -> Result<u32, GeoError> {
        let mut value = 0u32;
        for _ in 0..4 {
            let c = self
                .peek()
                .ok_or(GeoError::MalformedGeoJson("unterminated \\u escape"))?;
            self.pos += 1;
            let digit = (c as char)
                .to_digit(16)
                .ok_or(GeoError::MalformedGeoJson("invalid \\u escape"))?;
            value = value * 16 + digit;
        }
        Ok(value)
    }

    fn parse_number(&mut self) -> Result<Json, GeoError> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        let mut digits = 0;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
            digits += 1;
        }
        if digits == 0 {
            return Err(GeoError::MalformedGeoJson("invalid number"));
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            let mut frac = 0;
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.pos += 1;
                frac += 1;
            }
            if frac == 0 {
                return Err(GeoError::MalformedGeoJson("invalid number"));
            }
        }
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'-') | Some(b'+')) {
                self.pos += 1;
            }
            let mut exp = 0;
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.pos += 1;
                exp += 1;
            }
            if exp == 0 {
                return Err(GeoError::MalformedGeoJson("invalid number"));
            }
        }
        let text = std::str::from_utf8(&self.input[start..self.pos])
            .map_err(|_| GeoError::MalformedGeoJson("invalid number"))?;
        let value: f64 = text
            .parse()
            .map_err(|_| GeoError::MalformedGeoJson("invalid number"))?;
        Ok(Json::Num(value))
    }
}
