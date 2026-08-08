//! OIDC complete chain (R2+ track): JWKS/JWK (RFC 7517), RS256 signature
//! verification (RFC 7515 / PKCS#1 v1.5 + SHA-256), OIDC discovery document
//! validation (RFC 8414), claim mapping and pluggable JWKS fetching with
//! cached rotation. Dependency-free like the HS256 baseline (`jwt.rs`).
//!
//! The provider transport is injected via [`JwksFetcher`] so the security
//! crate stays dependency-free; the server supplies a plain-HTTP fetcher and
//! operators may pin a static JWKS snapshot for production.

use super::jwt::{
    JwtClaims, base64url_decode, constant_time_eq, hmac_sha256, now_epoch, parse_claims, sha256,
};
use ontolith_core::error::OntolithError;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Public RSA key material for RS256 verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RsaPublicKey {
    /// Modulus, big-endian bytes (JWK `n`, base64url-decoded).
    pub n: Vec<u8>,
    /// Public exponent, big-endian bytes (JWK `e`).
    pub e: Vec<u8>,
}

/// A single JSON Web Key (RFC 7517).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Jwk {
    pub kid: Option<String>,
    pub kty: String,
    pub alg: Option<String>,
    /// `use` ("sig" / "enc"); None when absent.
    pub use_: Option<String>,
    /// RSA modulus (kty=RSA).
    pub n: Option<Vec<u8>>,
    /// RSA public exponent (kty=RSA).
    pub e: Option<Vec<u8>>,
    /// Symmetric key bytes (kty=oct, HS256).
    pub k: Option<Vec<u8>>,
}

impl Jwk {
    /// Parse a single JWK object from its JSON value.
    pub fn from_value(v: &Value) -> Option<Jwk> {
        let kty = v.get("kty")?.as_str()?.to_owned();
        let kid = v.get("kid").and_then(Value::as_str).map(str::to_owned);
        let alg = v.get("alg").and_then(Value::as_str).map(str::to_owned);
        let use_ = v.get("use").and_then(Value::as_str).map(str::to_owned);
        let b64 = |k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .and_then(|s| base64url_decode(s).ok())
        };
        Jwk {
            kty,
            kid,
            alg,
            use_,
            n: b64("n"),
            e: b64("e"),
            k: b64("k"),
        }
        .into()
    }

    pub fn is_signing(&self) -> bool {
        self.use_.as_deref() != Some("enc")
    }

    /// True when this runtime can actually verify with the key: RS256 (RSA
    /// with modulus+exponent) or HS256 (oct with key bytes). Other key types
    /// (EC, OKP, ...) are never usable here.
    pub fn is_usable(&self) -> bool {
        match self.kty.as_str() {
            "RSA" => self.n.is_some() && self.e.is_some(),
            "oct" => self.k.is_some(),
            _ => false,
        }
    }
}

/// JSON Web Key Set (RFC 7517 `{"keys":[...]}`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Jwks {
    pub keys: Vec<Jwk>,
}

impl Jwks {
    /// Parse a JWKS document; entries this runtime cannot use (malformed or
    /// unsupported key types) are skipped, an empty set is a hard error (a
    /// configured key set with no usable keys must not silently allow
    /// anything).
    pub fn from_json(text: &str) -> Result<Jwks, OntolithError> {
        let v: Value = serde_json::from_str(text)
            .map_err(|e| OntolithError::Failed(format!("invalid jwks json: {e}")))?;
        let keys = v
            .get("keys")
            .and_then(Value::as_array)
            .ok_or_else(|| OntolithError::Failed("jwks missing keys array".into()))?
            .iter()
            .filter_map(Jwk::from_value)
            .filter(Jwk::is_usable)
            .collect::<Vec<_>>();
        if keys.is_empty() {
            return Err(OntolithError::Failed("jwks has no usable keys".into()));
        }
        Ok(Jwks { keys })
    }

    /// Select a key for a token's `kid` + `alg` header. A key with a matching
    /// `kid` wins; otherwise a single unambiguous key (no kid on either side,
    /// or token has no kid and the set has exactly one key) is returned.
    pub fn select(&self, kid: Option<&str>, alg: &str) -> Option<&Jwk> {
        if let Some(kid) = kid {
            return self.keys.iter().find(|k| k.kid.as_deref() == Some(kid));
        }
        match self.keys.as_slice() {
            [single] => Some(single),
            _ => self
                .keys
                .iter()
                .find(|k| k.kid.is_none() && k.alg.as_deref() == Some(alg)),
        }
    }
}

/// OIDC verification policy (token-level).
#[derive(Debug, Clone, Default)]
pub struct OidcConfig {
    /// Required `iss` claim.
    pub issuer: Option<String>,
    /// Required `aud` claim.
    pub audience: Option<String>,
    /// Clock leeway in seconds for `exp`/`nbf` (defaults to 0).
    pub leeway_secs: u64,
}

/// OIDC discovery document (RFC 8414) — the fields this runtime consumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcDiscovery {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
    pub userinfo_endpoint: Option<String>,
}

impl OidcDiscovery {
    /// Parse a discovery document and require `issuer` to match the
    /// configured value (prevents provider-confusion attacks).
    pub fn parse(text: &str, expected_issuer: &str) -> Result<OidcDiscovery, OntolithError> {
        let v: Value = serde_json::from_str(text)
            .map_err(|e| OntolithError::Failed(format!("invalid discovery json: {e}")))?;
        let issuer = v
            .get("issuer")
            .and_then(Value::as_str)
            .ok_or_else(|| OntolithError::Failed("discovery missing issuer".into()))?;
        if issuer != expected_issuer {
            return Err(OntolithError::Failed(format!(
                "discovery issuer mismatch: got {issuer}, expected {expected_issuer}"
            )));
        }
        let jwks_uri = v
            .get("jwks_uri")
            .and_then(Value::as_str)
            .ok_or_else(|| OntolithError::Failed("discovery missing jwks_uri".into()))?
            .to_owned();
        Ok(OidcDiscovery {
            issuer: issuer.to_owned(),
            authorization_endpoint: v
                .get("authorization_endpoint")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            token_endpoint: v
                .get("token_endpoint")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            jwks_uri,
            userinfo_endpoint: v
                .get("userinfo_endpoint")
                .and_then(Value::as_str)
                .map(str::to_owned),
        })
    }
}

/// Verify an OIDC/JWT access or ID token against a JWKS with the configured
/// issuer/audience policy. Supported algorithms: RS256 (kty=RSA) and HS256
/// (kty=oct). Signature, `exp`, `nbf`, `iss`, `aud` are enforced; the token
/// header `alg`/`kid` select the verification key.
pub fn verify_oidc_token(
    token: &str,
    jwks: &Jwks,
    config: &OidcConfig,
) -> Result<JwtClaims, OntolithError> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(OntolithError::Failed("unauthorized: malformed jwt".into()));
    }
    let header = base64url_decode(parts[0])
        .map_err(|e| OntolithError::Failed(format!("unauthorized: jwt header: {e}")))?;
    let header: Value = serde_json::from_slice(&header)
        .map_err(|e| OntolithError::Failed(format!("unauthorized: jwt header json: {e}")))?;
    let alg = header
        .get("alg")
        .and_then(Value::as_str)
        .ok_or_else(|| OntolithError::Failed("unauthorized: jwt missing alg".into()))?;
    let kid = header.get("kid").and_then(Value::as_str);
    if alg != "RS256" && alg != "HS256" {
        return Err(OntolithError::Failed(format!(
            "unauthorized: jwt alg {alg} not supported"
        )));
    }

    let key = jwks.select(kid, alg).ok_or_else(|| {
        OntolithError::Failed("unauthorized: no matching jwks key for jwt".into())
    })?;
    if !key.is_signing() {
        return Err(OntolithError::Failed(
            "unauthorized: jwks key not intended for signing".into(),
        ));
    }

    let signing_input = format!("{}.{}", parts[0], parts[1]);
    let provided = base64url_decode(parts[2])
        .map_err(|e| OntolithError::Failed(format!("unauthorized: jwt signature: {e}")))?;
    let valid = match (alg, key.kty.as_str()) {
        ("RS256", "RSA") => {
            let Some(n) = &key.n else {
                return Err(OntolithError::Failed(
                    "unauthorized: jwks key missing n".into(),
                ));
            };
            let Some(e) = &key.e else {
                return Err(OntolithError::Failed(
                    "unauthorized: jwks key missing e".into(),
                ));
            };
            rsa_sha256_verify(&provided, n, e, signing_input.as_bytes())
        }
        ("HS256", "oct") => {
            let Some(k) = &key.k else {
                return Err(OntolithError::Failed(
                    "unauthorized: jwks key missing k".into(),
                ));
            };
            constant_time_eq(&hmac_sha256(k, signing_input.as_bytes()), &provided)
        }
        _ => false,
    };
    if !valid {
        return Err(OntolithError::Failed(
            "unauthorized: jwt signature mismatch".into(),
        ));
    }

    let payload = base64url_decode(parts[1])
        .map_err(|e| OntolithError::Failed(format!("unauthorized: jwt payload: {e}")))?;
    let payload: Value = serde_json::from_slice(&payload)
        .map_err(|e| OntolithError::Failed(format!("unauthorized: jwt payload json: {e}")))?;
    let claims = parse_claims(&payload)?;
    validate_oidc_claims(&claims, config)?;
    Ok(claims)
}

fn validate_oidc_claims(claims: &JwtClaims, config: &OidcConfig) -> Result<(), OntolithError> {
    let leeway = config.leeway_secs as i64;
    let now = now_epoch();
    if let Some(exp) = claims.expires_at
        && exp + leeway < now
    {
        return Err(OntolithError::Failed("unauthorized: jwt expired".into()));
    }
    if let Some(nbf) = claims.not_before
        && nbf > now + leeway
    {
        return Err(OntolithError::Failed(
            "unauthorized: jwt not yet valid".into(),
        ));
    }
    if let Some(expected) = &config.issuer
        && claims.issuer.as_deref() != Some(expected.as_str())
    {
        return Err(OntolithError::Failed(
            "unauthorized: jwt issuer mismatch".into(),
        ));
    }
    if let Some(expected) = &config.audience
        && claims.audience.as_deref() != Some(expected.as_str())
    {
        return Err(OntolithError::Failed(
            "unauthorized: jwt audience mismatch".into(),
        ));
    }
    Ok(())
}

/// Transport hook for fetching discovery/JWKS documents. The security crate
/// stays dependency-free; the server injects its HTTP transport.
pub trait JwksFetcher: Send + Sync {
    fn get(&self, url: &str) -> Result<String, String>;
}

/// JWKS cache with TTL-based refresh for key rotation. A failed refresh
/// keeps serving the previous key set (offline tolerance); a fetch that
/// yields a valid key set replaces the cache.
pub struct CachingJwks {
    inner: Mutex<CacheInner>,
    ttl: Duration,
}

struct CacheInner {
    jwks: Jwks,
    fetched_at: Instant,
}

impl CachingJwks {
    pub fn new(jwks: Jwks, ttl: Duration) -> Self {
        Self {
            inner: Mutex::new(CacheInner {
                jwks,
                fetched_at: Instant::now() - ttl,
            }),
            ttl,
        }
    }

    pub fn current(&self) -> Jwks {
        self.inner
            .lock()
            .map(|g| g.jwks.clone())
            .unwrap_or_default()
    }

    /// Refresh when stale; returns the serving key set.
    pub fn refresh(&self, url: &str, fetcher: &dyn JwksFetcher) -> Result<Jwks, OntolithError> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| OntolithError::Failed("jwks cache lock poisoned".into()))?;
        if guard.fetched_at.elapsed() < self.ttl {
            return Ok(guard.jwks.clone());
        }
        let text = fetcher
            .get(url)
            .map_err(|e| OntolithError::Failed(format!("jwks fetch failed: {e}")))?;
        match Jwks::from_json(&text) {
            Ok(jwks) => {
                guard.jwks = jwks.clone();
                guard.fetched_at = Instant::now();
                Ok(jwks)
            }
            // Keep serving the last good set on a malformed response.
            Err(_) => Ok(guard.jwks.clone()),
        }
    }
}

/// OIDC JWKS verification source: a TTL cache refreshed through the injected
/// [`JwksFetcher`], so rotated keys are picked up without a restart while a
/// failed refresh keeps serving the last good set.
pub struct JwksVerifier {
    cache: Arc<CachingJwks>,
    url: String,
    fetcher: Arc<dyn JwksFetcher>,
}

impl JwksVerifier {
    pub fn new(cache: CachingJwks, url: impl Into<String>, fetcher: Arc<dyn JwksFetcher>) -> Self {
        Self {
            cache: Arc::new(cache),
            url: url.into(),
            fetcher,
        }
    }

    /// Return the current key set, refreshing from the URL when the TTL has
    /// elapsed. Errors only when the cache lock is poisoned; fetch failures
    /// are tolerated by serving the previous set.
    pub fn jwks(&self) -> Result<Jwks, OntolithError> {
        self.cache.refresh(&self.url, self.fetcher.as_ref())
    }

    pub fn url(&self) -> &str {
        &self.url
    }
}

impl Clone for JwksVerifier {
    fn clone(&self) -> Self {
        Self {
            cache: Arc::clone(&self.cache),
            url: self.url.clone(),
            fetcher: Arc::clone(&self.fetcher),
        }
    }
}

impl std::fmt::Debug for JwksVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JwksVerifier")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// RSA PKCS#1 v1.5 + SHA-256 verification (dependency-free big integers).
// ---------------------------------------------------------------------------

fn big_trim(mut a: Vec<u8>) -> Vec<u8> {
    while a.len() > 1 && a[0] == 0 {
        a.remove(0);
    }
    a
}

fn big_cmp(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    let a = big_trim(a.to_vec());
    let b = big_trim(b.to_vec());
    a.len().cmp(&b.len()).then_with(|| a.cmp(&b))
}

/// `a - b` for a >= b (big-endian), trimmed.
fn big_sub(a: &[u8], b: &[u8]) -> Vec<u8> {
    let a = big_trim(a.to_vec());
    let b = big_trim(b.to_vec());
    let mut out = vec![0u8; a.len()];
    let mut borrow = 0i64;
    for i in (0..a.len()).rev() {
        let b_i = if i < a.len() - b.len() {
            0u8
        } else {
            b[i - (a.len() - b.len())]
        };
        let diff = a[i] as i64 - b_i as i64 - borrow;
        if diff < 0 {
            out[i] = (diff + 256) as u8;
            borrow = 1;
        } else {
            out[i] = diff as u8;
            borrow = 0;
        }
    }
    big_trim(out)
}

/// Schoolbook multiply (big-endian), trimmed. Uses the canonical
/// least-significant-first formulation internally to keep carry propagation
/// exact, then reverses back to big-endian.
fn big_mul(a: &[u8], b: &[u8]) -> Vec<u8> {
    let a = big_trim(a.to_vec());
    let b = big_trim(b.to_vec());
    if a == [0] || b == [0] {
        return vec![0];
    }
    let mut out_le = vec![0u8; a.len() + b.len()];
    for (i, &ai) in a.iter().rev().enumerate() {
        let mut carry: u64 = 0;
        for (j, &bj) in b.iter().rev().enumerate() {
            let idx = i + j;
            let cur = out_le[idx] as u64 + (ai as u64) * (bj as u64) + carry;
            out_le[idx] = cur as u8;
            carry = cur >> 8;
        }
        let mut idx = i + b.len();
        let mut c = carry;
        while c > 0 {
            let cur = out_le[idx] as u64 + c;
            out_le[idx] = cur as u8;
            c = cur >> 8;
            idx += 1;
        }
    }
    out_le.reverse();
    big_trim(out_le)
}

/// Multiply a big-endian number by a small scalar `q` (0..=256) with exact
/// carry propagation. `q` may be 256 (single-byte overflow), which is needed
/// by the modular-reduction quotient search.
fn big_mul_small(a: &[u8], q: u64) -> Vec<u8> {
    let a = big_trim(a.to_vec());
    if q == 0 || a == [0] {
        return vec![0];
    }
    // Align the product to the least-significant end (index `8 + i`), with
    // room above for the final carry byte(s).
    let mut out = vec![0u8; a.len() + 8];
    let mut carry: u64 = 0;
    for i in (0..a.len()).rev() {
        let cur = (a[i] as u64) * q + carry;
        out[i + 8] = cur as u8;
        carry = cur >> 8;
    }
    let mut ci = 7usize;
    while carry > 0 && ci > 0 {
        out[ci] = (carry & 0xff) as u8;
        carry >>= 8;
        ci -= 1;
    }
    big_trim(out)
}

/// Remainder of `x / n` (big-endian, n != 0). Per input byte, finds the
/// quotient digit q in [0, 256] with q*n <= r < (q+1)*n by binary search
/// (8 multiplications per byte, far cheaper than up-to-256 subtractions).
fn big_rem(x: &[u8], n: &[u8]) -> Vec<u8> {
    let n = big_trim(n.to_vec());
    debug_assert!(!n.is_empty() && n != [0]);
    let mut r: Vec<u8> = Vec::with_capacity(n.len() + 1);
    for &b in x {
        r.push(b);
        // r < n*256 + 256 after the shift; find q in [0, 256] with q*n <= r.
        let mut lo = 0usize;
        let mut hi = 257usize; // qn(lo) <= r < qn(hi)
        while lo + 1 < hi {
            let mid = (lo + hi) / 2;
            let qn = big_mul_small(&n, mid as u64);
            if matches!(
                big_cmp(&qn, &r),
                std::cmp::Ordering::Less | std::cmp::Ordering::Equal
            ) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        if lo > 0 {
            let qn = big_mul_small(&n, lo as u64);
            r = big_sub(&r, &qn);
        } else {
            r = big_trim(r.clone());
        }
    }
    big_trim(r)
}

/// `base^exp mod modulus` via square-and-multiply (exp is small: 65537).
fn big_mod_pow(base: &[u8], exp: u64, modulus: &[u8]) -> Vec<u8> {
    let mut result = vec![1u8];
    let mut b = big_rem(base, modulus);
    let mut e = exp;
    while e > 0 {
        if e & 1 == 1 {
            result = big_rem(&big_mul(&result, &b), modulus);
        }
        b = big_rem(&big_mul(&b, &b), modulus);
        e >>= 1;
    }
    big_trim(result)
}

/// RSA PKCS#1 v1.5 signature verification with SHA-256 (RFC 8017 §8.2).
/// `message` is the JWS signing input; `signature` the raw signature bytes.
fn rsa_sha256_verify(signature: &[u8], n: &[u8], e: &[u8], message: &[u8]) -> bool {
    let n = big_trim(n.to_vec());
    if n.len() < 32 {
        return false;
    }
    let e = big_trim(e.to_vec());
    if e.is_empty() || e == [0] {
        return false;
    }
    let e_u64 = e.iter().fold(0u64, |acc, &b| {
        acc.checked_mul(256).map_or(0, |a| a) + b as u64
    });
    if e_u64 == 0 {
        return false;
    }
    // Signature must be exactly k bytes (left-pad shorter inputs).
    let k = n.len();
    if signature.len() > k {
        return false;
    }
    let mut sig = vec![0u8; k - signature.len()];
    sig.extend_from_slice(signature);
    let m = big_mod_pow(&sig, e_u64, &n);
    let expected = emsa_pkcs1_v1_5_sha256(message, k);
    constant_time_eq(&pad_to(&m, k), &expected)
}

/// EMSA-PKCS1-v1_5 encoding: `00 01 FF..FF 00 || DigestInfo(SHA-256) || H`.
fn emsa_pkcs1_v1_5_sha256(message: &[u8], k: usize) -> Vec<u8> {
    // RFC 8017 DigestInfo prefix for SHA-256 (19 bytes).
    const DIGEST_INFO: [u8; 19] = [
        0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01,
        0x05, 0x00, 0x04, 0x20,
    ];
    let digest = sha256(message);
    let t = DIGEST_INFO.len() + digest.len(); // 19 + 32
    debug_assert!(k >= t + 3);
    let mut em = vec![0u8; k];
    em[0] = 0x00;
    em[1] = 0x01;
    em[2..k - t - 1].fill(0xff);
    em[k - t - 1] = 0x00;
    em[k - t..k - digest.len()].copy_from_slice(&DIGEST_INFO);
    em[k - digest.len()..].copy_from_slice(&digest);
    em
}

fn pad_to(v: &[u8], k: usize) -> Vec<u8> {
    debug_assert!(v.len() <= k);
    let mut out = vec![0u8; k - v.len()];
    out.extend_from_slice(v);
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bigint_small_probes() {
        assert_eq!(big_trim(vec![0, 0, 5]), vec![5]);
        assert_eq!(big_mul_small(&[3], 85), vec![255]);
        assert_eq!(big_mul_small(&[1], 256), vec![1, 0]);
        assert_eq!(big_mul(&[0xFF], &[0xFF]), vec![0xFE, 0x01]);
        assert_eq!(big_rem(&[0x01, 0x00], &[0x03]), vec![1]); // 256 % 3
        assert_eq!(big_sub(&[1, 0], &[0xFF]), vec![1]); // 256 - 255
        // 2^10 mod 11 == 1
        assert_eq!(big_mod_pow(&[2], 10, &[0x0B]), vec![1]);
        // 7^4 mod 13 == 9  (2401 % 13)
        assert_eq!(big_mod_pow(&[7], 4, &[0x0D]), vec![9]);
    }

    fn b64(s: &str) -> Vec<u8> {
        base64url_decode(s).expect("b64")
    }

    #[test]
    fn rfc7515_a2_rs256_verifies() {
        let n = "ofgWCuLjybRlzo0tZWJjNiuSfb4p4fAkd_wWJcyQoTbji9k0l8W26mPddxHmfHQp-Vaw-4qPCJrcS2mJPMEzP1Pt0Bm4d4QlL-yRT-SFd2lZS-pCgNMsD1W_YpRPEwOWvG6b32690r2jZ47soMZo9wGzjb_7OMg0LOL-bSf63kpaSHSXndS5z5rexMdbBYUsLA9e-KXBdQOS-UTo7WTBEMa2R2CapHg665xsmtdVMTBQY4uDZlxvb3qCo5ZwKh9kG4LT6_I5IhlJH7aGhyxXFvUK-DWNmoudF8NAco9_h9iaGNj8q2ethFkMLs91kzk2PAcDTW9gb54h4FRWyuXpoQ"
            .to_owned();
        let e = "AQAB";
        // RFC 7515 A.2.1 keeps the JSON line breaks inside the payload, so the
        // signing input below is the exact published BASE64URL(JWS Payload).
        let signing_input = "eyJhbGciOiJSUzI1NiJ9.eyJpc3MiOiJqb2UiLA0KICJleHAiOjEzMDA4MTkzODAsDQogImh0dHA6Ly9leGFtcGxlLmNvbS9pc19yb290Ijp0cnVlfQ";
        let sig = "cC4hiUPoj9Eetdgtv3hF80EGrhuB__dzERat0XF9g2VtQgr9PJbu3XOiZj5RZmh7AAuHIm4Bh-0Qc_lF5YKt_O8W2Fp5jujGbds9uJdbF9CUAr7t1dnZcAcQjbKBYNX4BAynRFdiuB--f_nZLgrnbyTyWzO75vRK5h6xBArLIARNPvkSjtQBMHlb1L07Qe7K0GarZRmB_eSN9383LcOLn6_dO--xi12jzDwusC-eOkHWEsqtFZESc6BfI7noOPqvhJ1phCnvWh6IeYI2w9QOYEUipUTI8np6LbgGY9Fs98rqVt5AXLIhWkWywlVmtVrBp0igcN_IoypGlUPQGe77Rw";
        assert!(rsa_sha256_verify(
            &b64(sig),
            &b64(&n),
            &b64(e),
            signing_input.as_bytes()
        ));
        // Tampered signature must fail.
        let mut bad = b64(sig);
        bad[10] ^= 0x01;
        assert!(!rsa_sha256_verify(
            &bad,
            &b64(&n),
            &b64(e),
            signing_input.as_bytes()
        ));
    }

    /// RFC 7517 §3.1 JWK set: parse, kid selection, single-key fallback.
    #[test]
    fn jwks_parse_and_select() {
        let jwks = Jwks::from_json(
            r#"{"keys":[
                {"kty":"RSA","kid":"2011-04-29","n":"0vx7agoebGcQSuuPiLJXZptN9nndrQmbXEps2aiAFbWhM78LhWx4cbbfAAtVT86zwu1RK7aPFFxuhDR1L6tSoc_BJECPebWKRXjBZCiFV4n3oknjhMstn64tZ_2W-5JsGY4Hc5n9yBXArwl93lqt7_RN5w6Cf0h4QyQ5v-65YGjQR0_FDW2QvzqY368QQMicAtaSqzs8KJZgnYb9c7d0zgdAZHzu6qMQvRL5hajrn1n91CbOpbISD08qNLyrdkt-bFTWhAI4vMQFh6WeZu0fM4lFd2NcRwr3XPksINHaQ-G_xBniIqbw0Ls1jF44-csFCur-kEgU8awapJzKnqDKgw","e":"AQAB","alg":"RS256","use":"sig"},
                {"kty":"oct","kid":"sym1","alg":"HS256","k":"AyM1SysPpbyDfgZld3umj1qzKObwVMkoqQ-EstJQLr_T-1qS0gZH75aKtMN3Yj0iPS4hcgUuTwjAzZr1Z9CAow"}
            ]}"#,
        )
        .expect("parse");
        assert_eq!(jwks.keys.len(), 2);
        assert_eq!(jwks.select(Some("2011-04-29"), "RS256").unwrap().kty, "RSA");
        assert_eq!(jwks.select(Some("sym1"), "HS256").unwrap().kty, "oct");
        assert!(jwks.select(Some("nope"), "RS256").is_none());

        let single = Jwks::from_json(
            r#"{"keys":[{"kty":"RSA","n":"0vx7agoebGcQSuuPiLJXZptN9nndrQmbXEps2aiAFbWhM78LhWx4cbbfAAtVT86zwu1RK7aPFFxuhDR1L6tSoc_BJECPebWKRXjBZCiFV4n3oknjhMstn64tZ_2W-5JsGY4Hc5n9yBXArwl93lqt7_RN5w6Cf0h4QyQ5v-65YGjQR0_FDW2QvzqY368QQMicAtaSqzs8KJZgnYb9c7d0zgdAZHzu6qMQvRL5hajrn1n91CbOpbISD08qNLyrdkt-bFTWhAI4vMQFh6WeZu0fM4lFd2NcRwr3XPksINHaQ-G_xBniIqbw0Ls1jF44-csFCur-kEgU8awapJzKnqDKgw","e":"AQAB"}]}"#,
        )
        .expect("parse single");
        assert!(single.select(None, "RS256").is_some());
        assert!(Jwks::from_json(r#"{"keys":[]}"#).is_err());
        assert!(Jwks::from_json(r#"{"keys":[{"kty":"EC"}]}"#).is_err());
    }

    /// End-to-end: build a token with our own HS256 signer over an oct JWK,
    /// then verify through the OIDC path (kid + alg + exp/iss/aud).
    #[test]
    fn verify_oidc_token_oct_roundtrip() {
        // JWK `k` must equal the base64url encoding of the raw HMAC key bytes.
        let secret = "0123456789abcdef"; // 16 raw bytes
        let k_b64 = "MDEyMzQ1Njc4OWFiY2RlZg"; // base64url of the raw bytes
        let jwks = Jwks::from_json(&format!(
            r#"{{"keys":[{{"kty":"oct","kid":"k1","alg":"HS256","k":"{k_b64}"}}]}}"#
        ))
        .expect("jwks");
        let token = super::super::jwt::sign_tenant_token(
            "acme",
            "u-42",
            secret,
            "https://idp.example",
            "ontolith-server",
            3600,
        )
        .expect("sign token");
        let claims = verify_oidc_token(
            &token,
            &jwks,
            &OidcConfig {
                issuer: Some("https://idp.example".into()),
                audience: Some("ontolith-server".into()),
                leeway_secs: 0,
            },
        )
        .expect("verify");
        assert_eq!(claims.sub, "u-42");
        assert_eq!(claims.tenant.as_deref(), Some("acme"));

        // Wrong kid: no matching key.
        let swapped = Jwks::from_json(&format!(
            r#"{{"keys":[{{"kty":"oct","kid":"other","alg":"HS256","k":"{secret}"}}]}}"#
        ))
        .expect("jwks");
        assert!(verify_oidc_token(&token, &swapped, &OidcConfig::default()).is_err());
        // Wrong issuer.
        assert!(
            verify_oidc_token(
                &token,
                &jwks,
                &OidcConfig {
                    issuer: Some("https://evil".into()),
                    ..Default::default()
                }
            )
            .is_err()
        );
        // Expired token (exp in the past).
        let expired = super::super::jwt::sign_tenant_token(
            "acme",
            "u-42",
            secret,
            "https://idp.example",
            "ontolith-server",
            -100,
        )
        .expect("sign expired");
        assert!(
            verify_oidc_token(
                &expired,
                &jwks,
                &OidcConfig {
                    issuer: Some("https://idp.example".into()),
                    audience: Some("ontolith-server".into()),
                    leeway_secs: 0,
                }
            )
            .is_err()
        );
    }

    #[test]
    fn discovery_parse_and_issuer_guard() {
        let doc = r#"{
            "issuer":"https://idp.example",
            "authorization_endpoint":"https://idp.example/auth",
            "token_endpoint":"https://idp.example/token",
            "userinfo_endpoint":"https://idp.example/userinfo",
            "jwks_uri":"https://idp.example/jwks"
        }"#;
        let d = OidcDiscovery::parse(doc, "https://idp.example").expect("parse");
        assert_eq!(d.jwks_uri, "https://idp.example/jwks");
        assert!(OidcDiscovery::parse(doc, "https://evil").is_err());
    }

    #[test]
    fn caching_jwks_refreshes_and_tolerates_bad_refresh() {
        struct Static(String);
        impl JwksFetcher for Static {
            fn get(&self, _url: &str) -> Result<String, String> {
                Ok(self.0.clone())
            }
        }
        let good = r#"{"keys":[{"kty":"oct","kid":"a","alg":"HS256","k":"c2VjcmV0"}]}"#;
        let bad = "not json";
        let cache = CachingJwks::new(Jwks::from_json(good).unwrap(), Duration::from_secs(1));
        // Stale → refresh.
        let refreshed = cache
            .refresh("https://idp/jwks", &Static(good.into()))
            .unwrap();
        assert_eq!(refreshed.keys.len(), 1);
        // Malformed refresh keeps serving the previous set.
        let kept = cache
            .refresh("https://idp/jwks", &Static(bad.into()))
            .unwrap();
        assert_eq!(kept.keys.len(), 1);
    }
}
