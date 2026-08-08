//! Dependency-free JWT verification (P5-02, OIDC-ready HS256 baseline).
//!
//! Implements the RFC 7519 JSON Web Token subset used by the R1 access
//! layer: `alg=HS256` (HMAC-SHA256, RFC 2104), base64url encoding (RFC
//! 4648 §5), `exp`/`iss`/`aud` claim validation and custom `tenant`/`scope`
//! claims mapped to the [`AuthContext`](crate::domain::AuthContext).
//!
//! SHA-256 and HMAC-SHA256 are implemented in-tree (with RFC 4231 / FIPS
//! 180-4 test vectors) to keep the dependency register unchanged; the
//! token is verified with a constant-time signature comparison.

use crate::domain::AuthContext;
use ontolith_core::error::OntolithError;
use serde_json::Value;

/// Verified JWT claims relevant to the access layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JwtClaims {
    /// `sub` — authenticated user id.
    pub sub: String,
    /// Custom `tenant` claim; falls back to the `ontolith_tenant` claim.
    pub tenant: Option<String>,
    /// Space-separated `scope` claim (e.g. `sparql:query health:read`).
    pub scope: Option<String>,
    /// Optional list of roles.
    pub roles: Vec<String>,
    pub issuer: Option<String>,
    pub audience: Option<String>,
    /// `exp` unix seconds (absent = no expiry).
    pub expires_at: Option<i64>,
    /// `nbf` unix seconds (absent = valid immediately).
    pub not_before: Option<i64>,
}

/// Verification policy for [`verify_hs256`].
#[derive(Debug, Clone, Default)]
pub struct JwtVerifyOptions {
    /// When set, the `iss` claim must match exactly.
    pub issuer: Option<String>,
    /// When set, the `aud` claim must match exactly.
    pub audience: Option<String>,
}

// ---------------------------------------------------------------------------
// Base64url (RFC 4648 §5, no padding).
// ---------------------------------------------------------------------------

pub(crate) fn base64url_decode(input: &str) -> Result<Vec<u8>, String> {
    let mut compact = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '-' => compact.push('+'),
            '_' => compact.push('/'),
            '=' => {}
            _ => compact.push(c),
        }
    }
    let padded = match compact.len() % 4 {
        0 => compact,
        2 => format!("{compact}=="),
        3 => format!("{compact}="),
        _ => return Err("invalid base64url length".into()),
    };
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = padded.into_bytes();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for b in bytes {
        if b == b'=' {
            continue;
        }
        let v = match ALPHABET.iter().position(|&a| a == b) {
            Some(v) => v as u32,
            None => return Err("invalid base64url character".into()),
        };
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    // Flush any whole bytes still buffered (valid padded inputs always leave
    // `bits == 0`; unpadded inputs processed with the `=`-replacement path are
    // padded first, so this loop is a no-op for well-formed input).
    while bits >= 8 {
        bits -= 8;
        out.push((acc >> bits) as u8);
    }
    Ok(out)
}

fn base64url_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len() * 4 / 3 + 3);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out.retain(|c| c != '=');
    out
}

// ---------------------------------------------------------------------------
// SHA-256 (FIPS 180-4).
// ---------------------------------------------------------------------------

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

pub(crate) fn sha256(message: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let bit_len = (message.len() as u64).wrapping_mul(8);
    let mut padded = message.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    let mut w = [0u32; 64];
    for block in padded.chunks_exact(64) {
        for (i, word) in w.iter_mut().take(16).enumerate() {
            let start = i * 4;
            *word = u32::from_be_bytes([
                block[start],
                block[start + 1],
                block[start + 2],
                block[start + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

// ---------------------------------------------------------------------------
// HMAC-SHA256 (RFC 2104).
// ---------------------------------------------------------------------------

pub(crate) fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut key = key.to_vec();
    if key.len() > 64 {
        key = sha256(&key).to_vec();
    }
    key.resize(64, 0);
    let mut inner = [0u8; 64];
    let mut outer = [0u8; 64];
    for i in 0..64 {
        inner[i] = key[i] ^ 0x36;
        outer[i] = key[i] ^ 0x5c;
    }
    let mut inner_msg = inner.to_vec();
    inner_msg.extend_from_slice(data);
    let inner_hash = sha256(&inner_msg);
    let mut outer_msg = outer.to_vec();
    outer_msg.extend_from_slice(&inner_hash);
    sha256(&outer_msg)
}

/// Constant-time byte comparison.
pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

// ---------------------------------------------------------------------------
// JWT.
// ---------------------------------------------------------------------------

pub(crate) fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub(crate) fn parse_claims(payload: &Value) -> Result<JwtClaims, OntolithError> {
    let sub = payload
        .get("sub")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| OntolithError::Failed("unauthorized: jwt missing sub claim".into()))?
        .to_owned();
    let tenant = payload
        .get("tenant")
        .or_else(|| payload.get("ontolith_tenant"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let scope = payload
        .get("scope")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let roles = payload
        .get("roles")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let issuer = payload
        .get("iss")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let audience = payload
        .get("aud")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let expires_at = payload.get("exp").and_then(Value::as_i64);
    let not_before = payload.get("nbf").and_then(Value::as_i64);
    Ok(JwtClaims {
        sub,
        tenant,
        scope,
        roles,
        issuer,
        audience,
        expires_at,
        not_before,
    })
}

/// Sign a JWT with HS256 (used by tests and by deployments that issue their
/// own tokens with the same shared secret).
pub fn sign_hs256(
    claims: &serde_json::Map<String, Value>,
    secret: &str,
    expires_in_secs: Option<i64>,
) -> Result<String, OntolithError> {
    let mut payload = claims.clone();
    if let Some(ttl) = expires_in_secs {
        payload.insert("exp".into(), Value::from(now_epoch() + ttl));
    }
    let header = br#"{"alg":"HS256","typ":"JWT"}"#;
    let payload_bytes = serde_json::to_vec(&payload)
        .map_err(|e| OntolithError::Failed(format!("jwt payload serialization: {e}")))?;
    let signing_input = format!(
        "{}.{}",
        base64url_encode(header),
        base64url_encode(&payload_bytes)
    );
    let sig = hmac_sha256(secret.as_bytes(), signing_input.as_bytes());
    Ok(format!("{}.{}", signing_input, base64url_encode(&sig)))
}

/// Convenience builder for tenant-scoped HS256 tokens (P5-02). Used by
/// deployments that issue tokens with the same shared secret, and by tests.
pub fn sign_tenant_token(
    tenant: &str,
    user: &str,
    secret: &str,
    issuer: &str,
    audience: &str,
    ttl_secs: i64,
) -> Result<String, OntolithError> {
    let mut claims = serde_json::Map::new();
    claims.insert("sub".into(), Value::from(user));
    claims.insert("tenant".into(), Value::from(tenant));
    claims.insert("iss".into(), Value::from(issuer));
    claims.insert("aud".into(), Value::from(audience));
    sign_hs256(&claims, secret, Some(ttl_secs))
}

/// Verify an HS256 JWT and return its claims. Rejects malformed tokens,
/// signature mismatches, expired tokens and issuer/audience mismatches.
pub fn verify_hs256(
    token: &str,
    secret: &str,
    options: &JwtVerifyOptions,
) -> Result<JwtClaims, OntolithError> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(OntolithError::Failed("unauthorized: malformed jwt".into()));
    }
    let header = base64url_decode(parts[0])
        .map_err(|e| OntolithError::Failed(format!("unauthorized: jwt header: {e}")))?;
    let header: Value = serde_json::from_slice(&header)
        .map_err(|e| OntolithError::Failed(format!("unauthorized: jwt header json: {e}")))?;
    if header.get("alg").and_then(Value::as_str) != Some("HS256") {
        return Err(OntolithError::Failed(
            "unauthorized: jwt alg must be HS256".into(),
        ));
    }

    let signing_input = format!("{}.{}", parts[0], parts[1]);
    let expected = hmac_sha256(secret.as_bytes(), signing_input.as_bytes());
    let provided = base64url_decode(parts[2])
        .map_err(|e| OntolithError::Failed(format!("unauthorized: jwt signature: {e}")))?;
    if !constant_time_eq(&expected, &provided) {
        return Err(OntolithError::Failed(
            "unauthorized: jwt signature mismatch".into(),
        ));
    }

    let payload = base64url_decode(parts[1])
        .map_err(|e| OntolithError::Failed(format!("unauthorized: jwt payload: {e}")))?;
    let payload: Value = serde_json::from_slice(&payload)
        .map_err(|e| OntolithError::Failed(format!("unauthorized: jwt payload json: {e}")))?;
    let claims = parse_claims(&payload)?;

    if let Some(exp) = claims.expires_at
        && exp < now_epoch()
    {
        return Err(OntolithError::Failed("unauthorized: jwt expired".into()));
    }
    if let Some(expected) = &options.issuer
        && claims.issuer.as_deref() != Some(expected.as_str())
    {
        return Err(OntolithError::Failed(
            "unauthorized: jwt issuer mismatch".into(),
        ));
    }
    if let Some(expected) = &options.audience
        && claims.audience.as_deref() != Some(expected.as_str())
    {
        return Err(OntolithError::Failed(
            "unauthorized: jwt audience mismatch".into(),
        ));
    }
    Ok(claims)
}

/// Map verified JWT claims to an [`AuthContext`].
pub fn auth_context_from_claims(
    claims: &JwtClaims,
    header_tenant: Option<&str>,
    header_user: Option<&str>,
    default_permissions: Vec<crate::domain::Permission>,
) -> AuthContext {
    let tenant = claims
        .tenant
        .as_deref()
        .or(header_tenant)
        .filter(|t| !t.is_empty())
        .unwrap_or("system");
    let user = claims.sub.as_str();
    let user = if user.is_empty() {
        header_user.unwrap_or("system")
    } else {
        user
    };
    let permissions = match &claims.scope {
        Some(scope) if !scope.trim().is_empty() => scope
            .split_whitespace()
            .filter_map(|item| {
                let (resource, action) = item.split_once(':')?;
                Some(crate::domain::Permission::new(resource, action))
            })
            .collect(),
        _ => default_permissions,
    };
    AuthContext::tenant_user(tenant, user, permissions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_fips_180_4_vectors() {
        // FIPS 180-4 §B.2 / standard "abc" vector.
        let digest = sha256(b"abc");
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let digest = sha256(b"");
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let digest = sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq");
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn hmac_sha256_matches_rfc4231_vector() {
        // RFC 4231 test case 2 (key = "Jefe", data = "what do ya want for nothing?").
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        let hex: String = mac.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn base64url_roundtrip() {
        let cases = [
            b"".to_vec(),
            b"f".to_vec(),
            b"fo".to_vec(),
            b"foo".to_vec(),
            b"foob".to_vec(),
            b"fooba".to_vec(),
            b"foobar".to_vec(),
        ];
        for c in cases {
            let enc = base64url_encode(&c);
            assert!(!enc.contains('+') && !enc.contains('/') && !enc.contains('='));
            assert_eq!(base64url_decode(&enc).unwrap(), c);
        }
    }

    #[test]
    fn jwt_sign_verify_roundtrip() {
        let mut claims = serde_json::Map::new();
        claims.insert("sub".into(), Value::from("u1"));
        claims.insert("tenant".into(), Value::from("acme"));
        claims.insert("scope".into(), Value::from("sparql:query health:read"));
        let token = sign_hs256(&claims, "s3cret", Some(3600)).unwrap();
        let verified = verify_hs256(&token, "s3cret", &JwtVerifyOptions::default()).unwrap();
        assert_eq!(verified.sub, "u1");
        assert_eq!(verified.tenant.as_deref(), Some("acme"));
        assert_eq!(verified.scope.as_deref(), Some("sparql:query health:read"));
        assert!(verified.expires_at.unwrap() > now_epoch());
    }

    #[test]
    fn jwt_rejects_tampered_and_expired_tokens() {
        let mut claims = serde_json::Map::new();
        claims.insert("sub".into(), Value::from("u1"));
        claims.insert("tenant".into(), Value::from("acme"));
        let token = sign_hs256(&claims, "s3cret", Some(3600)).unwrap();

        // Wrong secret.
        assert!(verify_hs256(&token, "wrong", &JwtVerifyOptions::default()).is_err());

        // Tampered payload.
        let parts: Vec<&str> = token.split('.').collect();
        let mut payload = base64url_decode(parts[1]).unwrap();
        let last = payload.len() - 1;
        payload[last] ^= 1;
        let forged = format!("{}.{}.{}", parts[0], base64url_encode(&payload), parts[2]);
        assert!(verify_hs256(&forged, "s3cret", &JwtVerifyOptions::default()).is_err());

        // Expired.
        let expired = sign_hs256(&claims, "s3cret", Some(-60)).unwrap();
        assert!(verify_hs256(&expired, "s3cret", &JwtVerifyOptions::default()).is_err());

        // Issuer / audience policy.
        claims.insert("iss".into(), Value::from("https://issuer.example"));
        claims.insert("aud".into(), Value::from("ontolith"));
        let token = sign_hs256(&claims, "s3cret", Some(3600)).unwrap();
        assert!(
            verify_hs256(
                &token,
                "s3cret",
                &JwtVerifyOptions {
                    issuer: Some("https://issuer.example".into()),
                    audience: None,
                },
            )
            .is_ok()
        );
        assert!(
            verify_hs256(
                &token,
                "s3cret",
                &JwtVerifyOptions {
                    issuer: Some("https://other.example".into()),
                    audience: None,
                },
            )
            .is_err()
        );
        assert!(
            verify_hs256(
                &token,
                "s3cret",
                &JwtVerifyOptions {
                    issuer: None,
                    audience: Some("ontolith".into()),
                },
            )
            .is_ok()
        );
        assert!(
            verify_hs256(
                &token,
                "s3cret",
                &JwtVerifyOptions {
                    issuer: None,
                    audience: Some("other-app".into()),
                },
            )
            .is_err()
        );
    }
}
