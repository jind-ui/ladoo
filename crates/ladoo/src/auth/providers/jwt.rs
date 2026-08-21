//! JWT authentication provider.
//!
//! Decodes and validates JSON Web Tokens from the `Authorization: Bearer`
//! header. The claims struct becomes the user type.
//!
//! Requires the `auth-jwt` feature.
//!
//! # Examples
//!
//! ```rust,ignore
//! use ladoo::prelude::*;
//! use serde::Deserialize;
//!
//! #[derive(Clone, Deserialize)]
//! struct Claims { sub: String }
//!
//! let jwt = JwtAuth::<Claims>::hs256(b"my-secret");
//! App::new()
//!     .group("/api", |g| g.guard(jwt).get("/me", handler))
//! ```

use std::marker::PhantomData;

use async_trait::async_trait;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::de::DeserializeOwned;

use crate::auth::{AuthError, AuthProvider};
use crate::request::Request;

/// JWT authentication provider.
///
/// Decodes JSON Web Tokens from the `Authorization: Bearer <token>`
/// header. The generic parameter `C` is the claims type — it becomes
/// the [`AuthProvider::User`] type, so handlers extract it directly
/// via `Auth<C>`.
///
/// # Examples
///
/// ```rust,ignore
/// use ladoo::auth::providers::JwtAuth;
/// use serde::Deserialize;
///
/// #[derive(Clone, Deserialize)]
/// struct Claims { sub: String, roles: Vec<String> }
///
/// let jwt = JwtAuth::<Claims>::hs256(b"secret")
///     .with_issuer("my-app")
///     .with_audience("my-api");
/// ```
pub struct JwtAuth<C: DeserializeOwned + Clone + Send + Sync + 'static> {
    decoding_key: DecodingKey,
    validation: Validation,
    _claims: PhantomData<C>,
}

impl<C: DeserializeOwned + Clone + Send + Sync + 'static> JwtAuth<C> {
    /// Create a JWT authenticator using HMAC-SHA256.
    ///
    /// Tokens must include an `exp` (expiration) claim by default — tokens
    /// without one are rejected. Call [`allow_no_expiry`](Self::allow_no_expiry)
    /// to opt out.
    pub fn hs256(secret: &[u8]) -> Self {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.set_required_spec_claims(&["exp"]);
        Self {
            decoding_key: DecodingKey::from_secret(secret),
            validation,
            _claims: PhantomData,
        }
    }

    /// Create a JWT authenticator using RSA-SHA256 from a PEM-encoded public key.
    ///
    /// Tokens must include an `exp` (expiration) claim by default — tokens
    /// without one are rejected. Call [`allow_no_expiry`](Self::allow_no_expiry)
    /// to opt out.
    pub fn rs256(pem: &[u8]) -> Self {
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_required_spec_claims(&["exp"]);
        Self {
            decoding_key: DecodingKey::from_rsa_pem(pem).expect("invalid RSA PEM key"),
            validation,
            _claims: PhantomData,
        }
    }

    /// Require and validate the `iss` (issuer) claim.
    ///
    /// Tokens missing an `iss` claim, or whose `iss` does not match,
    /// are rejected. Without this call, `iss` is ignored entirely.
    pub fn with_issuer(mut self, issuer: &str) -> Self {
        self.validation.set_issuer(&[issuer]);
        self.validation
            .required_spec_claims
            .insert("iss".to_string());
        self
    }

    /// Require and validate the `aud` (audience) claim.
    ///
    /// Tokens missing an `aud` claim, or whose `aud` does not match,
    /// are rejected. Without this call, `aud` is ignored entirely.
    pub fn with_audience(mut self, audience: &str) -> Self {
        self.validation.set_audience(&[audience]);
        self.validation
            .required_spec_claims
            .insert("aud".to_string());
        self
    }

    /// Allow tokens without an `exp` claim.
    ///
    /// By default, `JwtAuth` requires tokens to include an `exp` (expiration)
    /// claim. Call this method to accept tokens that never expire — useful for
    /// testing or internal-only tokens where expiry is managed externally.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use ladoo::prelude::*;
    ///
    /// # #[derive(Clone, serde::Deserialize)]
    /// # struct Claims { sub: String }
    /// let auth = JwtAuth::<Claims>::hs256(b"secret")
    ///     .allow_no_expiry();
    /// ```
    pub fn allow_no_expiry(mut self) -> Self {
        self.validation.required_spec_claims.remove("exp");
        self.validation.validate_exp = false;
        self
    }
}

#[async_trait]
impl<C: DeserializeOwned + Clone + Send + Sync + 'static> AuthProvider for JwtAuth<C> {
    type User = C;

    async fn authenticate(&self, req: &Request) -> Result<C, AuthError> {
        let header_value = req
            .headers()
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .ok_or(AuthError::Missing)?;

        let token = header_value
            .strip_prefix("Bearer ")
            .ok_or_else(|| AuthError::Invalid("Expected 'Bearer <token>' format".into()))?;

        let token_data = jsonwebtoken::decode::<C>(token, &self.decoding_key, &self.validation)
            .map_err(|e| {
                #[cfg(feature = "logging")]
                tracing::debug!(error = %e, "JWT validation failed");
                match e.kind() {
                    jsonwebtoken::errors::ErrorKind::ExpiredSignature => AuthError::Expired,
                    _ => AuthError::Invalid("Invalid token".into()),
                }
            })?;

        Ok(token_data.claims)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::Method;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    struct TestClaims {
        sub: String,
        exp: u64,
    }

    fn future_exp() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600
    }

    fn make_hs256_token(claims: &TestClaims, secret: &[u8]) -> String {
        use jsonwebtoken::{encode, EncodingKey, Header};
        encode(
            &Header::default(),
            claims,
            &EncodingKey::from_secret(secret),
        )
        .unwrap()
    }

    fn make_expired_token(secret: &[u8]) -> String {
        use jsonwebtoken::{encode, EncodingKey, Header};

        #[derive(Serialize)]
        struct ExpiredClaims {
            sub: String,
            exp: u64,
        }
        encode(
            &Header::default(),
            &ExpiredClaims {
                sub: "alice".into(),
                exp: 0,
            },
            &EncodingKey::from_secret(secret),
        )
        .unwrap()
    }

    #[derive(Serialize)]
    struct ClaimsWithIssAud {
        sub: String,
        iss: String,
        aud: String,
        exp: u64,
    }

    fn make_token_with_iss_aud(claims: &ClaimsWithIssAud, secret: &[u8]) -> String {
        use jsonwebtoken::{encode, EncodingKey, Header};
        encode(
            &Header::default(),
            claims,
            &EncodingKey::from_secret(secret),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn valid_token_returns_claims() {
        let secret = b"test-secret-key-256-bits-long!!";
        let claims = TestClaims {
            sub: "alice".into(),
            exp: future_exp(),
        };
        let token = make_hs256_token(&claims, secret);

        let jwt = JwtAuth::<TestClaims>::hs256(secret);
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "Authorization",
            http::HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        let req = Request::test_with_headers(Method::GET, "/", headers);
        let result = jwt.authenticate(&req).await.unwrap();
        assert_eq!(result.sub, "alice");
    }

    #[tokio::test]
    async fn missing_header_returns_missing() {
        let jwt = JwtAuth::<TestClaims>::hs256(b"secret");
        let req = Request::test(Method::GET, "/");
        let result = jwt.authenticate(&req).await;
        assert!(matches!(result.unwrap_err(), AuthError::Missing));
    }

    #[tokio::test]
    async fn missing_bearer_prefix_returns_invalid() {
        let jwt = JwtAuth::<TestClaims>::hs256(b"secret");
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "Authorization",
            http::HeaderValue::from_static("Token abc123"),
        );
        let req = Request::test_with_headers(Method::GET, "/", headers);
        let result = jwt.authenticate(&req).await;
        assert!(matches!(result.unwrap_err(), AuthError::Invalid(_)));
    }

    #[tokio::test]
    async fn invalid_token_returns_invalid() {
        let jwt = JwtAuth::<TestClaims>::hs256(b"secret");
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "Authorization",
            http::HeaderValue::from_static("Bearer not-a-real-jwt"),
        );
        let req = Request::test_with_headers(Method::GET, "/", headers);
        let result = jwt.authenticate(&req).await;
        assert!(matches!(result.unwrap_err(), AuthError::Invalid(_)));
    }

    #[tokio::test]
    async fn expired_token_returns_expired() {
        let secret = b"test-secret-key-256-bits-long!!";
        let token = make_expired_token(secret);

        let jwt = JwtAuth::<TestClaims>::hs256(secret);
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "Authorization",
            http::HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        let req = Request::test_with_headers(Method::GET, "/", headers);
        let result = jwt.authenticate(&req).await;
        assert!(matches!(result.unwrap_err(), AuthError::Expired));
    }

    #[tokio::test]
    async fn wrong_secret_returns_invalid() {
        let claims = TestClaims {
            sub: "alice".into(),
            exp: future_exp(),
        };
        let token = make_hs256_token(&claims, b"correct-secret-key-long-enough!");
        let jwt = JwtAuth::<TestClaims>::hs256(b"wrong-secret-key!!-long-enough!");
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "Authorization",
            http::HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        let req = Request::test_with_headers(Method::GET, "/", headers);
        let result = jwt.authenticate(&req).await;
        assert!(matches!(result.unwrap_err(), AuthError::Invalid(_)));
    }

    #[tokio::test]
    async fn with_issuer_validation() {
        let secret = b"test-secret-key-256-bits-long!!";
        // Token without iss claim should fail when issuer is required
        let claims = TestClaims {
            sub: "alice".into(),
            exp: future_exp(),
        };
        let token = make_hs256_token(&claims, secret);

        let jwt = JwtAuth::<TestClaims>::hs256(secret).with_issuer("my-app");
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "Authorization",
            http::HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        let req = Request::test_with_headers(Method::GET, "/", headers);
        let result = jwt.authenticate(&req).await;
        assert!(matches!(result.unwrap_err(), AuthError::Invalid(_)));
    }

    #[tokio::test]
    async fn with_audience_validation() {
        let secret = b"test-secret-key-256-bits-long!!";
        // Token without aud claim should fail when audience is required
        let claims = TestClaims {
            sub: "alice".into(),
            exp: future_exp(),
        };
        let token = make_hs256_token(&claims, secret);

        let jwt = JwtAuth::<TestClaims>::hs256(secret).with_audience("my-api");
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "Authorization",
            http::HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        let req = Request::test_with_headers(Method::GET, "/", headers);
        let result = jwt.authenticate(&req).await;
        assert!(matches!(result.unwrap_err(), AuthError::Invalid(_)));
    }

    #[tokio::test]
    async fn matching_issuer_and_audience_succeeds() {
        let secret = b"test-secret-key-256-bits-long!!";
        let claims = ClaimsWithIssAud {
            sub: "alice".into(),
            iss: "my-app".into(),
            aud: "my-api".into(),
            exp: future_exp(),
        };
        let token = make_token_with_iss_aud(&claims, secret);

        #[derive(Clone, Debug, PartialEq, Deserialize)]
        struct Claims {
            sub: String,
        }

        let jwt = JwtAuth::<Claims>::hs256(secret)
            .with_issuer("my-app")
            .with_audience("my-api");
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "Authorization",
            http::HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        let req = Request::test_with_headers(Method::GET, "/", headers);
        let result = jwt.authenticate(&req).await.unwrap();
        assert_eq!(result.sub, "alice");
    }

    // Test-only RSA keypair (2048-bit), not used anywhere else.
    const TEST_RSA_PRIVATE_PEM: &[u8] = br#"-----BEGIN PRIVATE KEY-----
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQCzIjsf7krs7j6h
U6IFNs379VhovRyqIUrMc91t+aarij3V4Fveb73ZF14AYiYWDmcfpf+2FUBE4w3L
HekX6Ude9vEvek7ZHjrYGR6CRd9QX8IoxQHLEBzlCL0++PTJaPzt9dfTb3MV07+T
wsXnG+V76sr0jsgS8DCsE/6sSXgHfFLAQigG2Ls36pcP1sTdQUgdqV/gRtMxQvcK
kGof9dv3ZtiwSza4pg3/VP0PlDiIhtMHdXRDeaV4KLpNmy7IwGJVgksLosH28OkQ
qqXv2SeouxcMex2rHFLEdj+YlIe+ork2ovXFwT/Tlkn2HycKhSoEKeffH2QPOSlx
C3rK5TjFAgMBAAECggEAIrdGmBiZYrOFZcSMiNAnOWZB/QJxdLNBCMCHsFGKsIH5
G+MASuqC83io2hAra2jdKXAFT6dsri0Gtk+UpfKqx0e6VEYy07cdFlY/6GVcMvpr
6XTMtSrpPqXj7zlWT1ZOdluHuU1HE3rXDO7ZZcGtRsepD55APhNYi3DQkVknruNg
HP6z76R7jxl5+kQLh3rpkGggrbC766OTqPXcQ4Mfbp5jxMdC6MCo8nafMZzkVk8t
km4idLMUqjtza5JGjLh3WfYE+SYmlM1z9d925e1SNW4mYDis/oaDIkTdlvOZ1EFz
C0N9Ixp76CNsjOmbYu2I5vNGSqANwJVxIaR5u6XVPQKBgQD7kuVrvx2Mv+cIDJkw
ebn7xC1NXFMywRq7nELZOyraaCBrcAf0U7fjeM/sCceC5XvZziIgFto40dKXI15L
hQ1XxVMPzJFgIZqtv23sxyWyDz2VB89XA7plmTUK0z2ellM/tUetq5dgni4lmhCG
zfsBRoo2FV737icehshui8tWpwKBgQC2SQ9mEQUHVaCSTaKvjGAnlCooNPPQ4qcb
g2VOHBfNGV3hYsoHJ+MGpT79ygkjTRFODD4gicrp2hMGdmnAmK0iVqwZOEXad6Gj
wjPRqSFEFOCeShtlHYvbiXafQhSaMIdHvwXaV2ehHa04+Og5EV4Kk+XFivqvejIo
Hoi4HQGOswKBgQDEJ7my1YWY5ViishAP+BnH8SLRmxdUD7Vka2bEMporSd1daDEL
lOtg9iZJCScDLSPwpAV/t9HXU+M77VvszoWk1jr5qqv/pLQSnZx8bps5xyBhP4Gv
ezyvU1JEaok1SkkG97Y39/9EWpHox8PzGFCKohHKMcem0Y63AqjtaRrXKQKBgDbW
m+9Ux3KBbCEXgg3V6Ud+53/ZDlCVHzjDusJY6UAmlXuswKKOeVoSdHTdRwp7sO0N
+dLIIWdg18Bl90Kdq9hcwsGDkGA9BT/CuNwmSX+12C1Glh9BWEXfgPRAaPpKByiq
axRYnzB1QRuWpiYk92mvPLzFJs2LsXMoXHEnKMTJAoGAB3JsYzoXW2se5zSx3i9u
JaEtPAugYHirThnocl1fiarFh9ujieN8yWLzt+4K/MafZHllf94ijiFxai8L/b8n
U90XXzdG/Jz9E3Y06wQ7vfDrx/ZSDe1dsXZKI1cX61LUdwJeK5hc1EK5z7edJIF3
cAd9h5pR1GbfhxLpBpy3RT8=
-----END PRIVATE KEY-----"#;

    const TEST_RSA_PUBLIC_PEM: &[u8] = br#"-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAsyI7H+5K7O4+oVOiBTbN
+/VYaL0cqiFKzHPdbfmmq4o91eBb3m+92RdeAGImFg5nH6X/thVAROMNyx3pF+lH
XvbxL3pO2R462BkegkXfUF/CKMUByxAc5Qi9Pvj0yWj87fXX029zFdO/k8LF5xvl
e+rK9I7IEvAwrBP+rEl4B3xSwEIoBti7N+qXD9bE3UFIHalf4EbTMUL3CpBqH/Xb
92bYsEs2uKYN/1T9D5Q4iIbTB3V0Q3mleCi6TZsuyMBiVYJLC6LB9vDpEKql79kn
qLsXDHsdqxxSxHY/mJSHvqK5NqL1xcE/05ZJ9h8nCoUqBCnn3x9kDzkpcQt6yuU4
xQIDAQAB
-----END PUBLIC KEY-----"#;

    fn make_rs256_token(claims: &TestClaims) -> String {
        use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
        let key = EncodingKey::from_rsa_pem(TEST_RSA_PRIVATE_PEM).unwrap();
        encode(&Header::new(Algorithm::RS256), claims, &key).unwrap()
    }

    #[tokio::test]
    async fn rs256_valid_token_returns_claims() {
        let claims = TestClaims {
            sub: "alice".into(),
            exp: future_exp(),
        };
        let token = make_rs256_token(&claims);

        let jwt = JwtAuth::<TestClaims>::rs256(TEST_RSA_PUBLIC_PEM);
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "Authorization",
            http::HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        let req = Request::test_with_headers(Method::GET, "/", headers);
        let result = jwt.authenticate(&req).await.unwrap();
        assert_eq!(result.sub, "alice");
    }

    #[tokio::test]
    async fn rs256_wrong_key_returns_invalid() {
        let claims = TestClaims {
            sub: "alice".into(),
            exp: future_exp(),
        };
        let token = make_hs256_token(&claims, b"test-secret-key-256-bits-long!!");

        let jwt = JwtAuth::<TestClaims>::rs256(TEST_RSA_PUBLIC_PEM);
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "Authorization",
            http::HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        let req = Request::test_with_headers(Method::GET, "/", headers);
        let result = jwt.authenticate(&req).await;
        assert!(matches!(result.unwrap_err(), AuthError::Invalid(_)));
    }

    #[tokio::test]
    async fn rejects_token_without_exp_by_default() {
        #[derive(Debug, Clone, Serialize, Deserialize)]
        struct Claims {
            sub: String,
        }

        let secret = b"test-secret-key-for-jwt-testing!";
        let auth = JwtAuth::<Claims>::hs256(secret);

        // Create a token WITHOUT exp claim
        let claims = Claims {
            sub: "user-1".into(),
        };
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(secret),
        )
        .unwrap();

        let mut headers = http::HeaderMap::new();
        headers.insert(
            "Authorization",
            http::HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        let req = Request::test_with_headers(Method::GET, "/", headers);
        let result = auth.authenticate(&req).await;
        assert!(result.is_err(), "Token without exp should be rejected");
    }

    #[tokio::test]
    async fn allows_token_without_exp_when_opted_in() {
        #[derive(Debug, Clone, Serialize, Deserialize)]
        struct Claims {
            sub: String,
        }

        let secret = b"test-secret-key-for-jwt-testing!";
        let auth = JwtAuth::<Claims>::hs256(secret).allow_no_expiry();

        let claims = Claims {
            sub: "user-1".into(),
        };
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(secret),
        )
        .unwrap();

        let mut headers = http::HeaderMap::new();
        headers.insert(
            "Authorization",
            http::HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        let req = Request::test_with_headers(Method::GET, "/", headers);
        let result = auth.authenticate(&req).await;
        assert!(
            result.is_ok(),
            "Token without exp should be accepted with allow_no_expiry"
        );
    }

    #[tokio::test]
    async fn jwt_error_does_not_leak_internals() {
        #[derive(Debug, Clone, Serialize, Deserialize)]
        struct Claims {
            sub: String,
            exp: u64,
        }

        let secret = b"test-secret-key-for-jwt-testing!";
        let auth = JwtAuth::<Claims>::hs256(secret);

        // Create a token signed with a DIFFERENT secret
        let wrong_secret = b"wrong-secret-key-for-jwt-tests!";
        let exp = future_exp();
        let claims = Claims {
            sub: "user-1".into(),
            exp,
        };
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(wrong_secret),
        )
        .unwrap();

        let mut headers = http::HeaderMap::new();
        headers.insert(
            "Authorization",
            http::HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        let req = Request::test_with_headers(Method::GET, "/", headers);
        let result = auth.authenticate(&req).await;
        let err = result.unwrap_err();

        // Must say "Invalid token", NOT "InvalidSignature" or any jsonwebtoken internal
        match err {
            AuthError::Invalid(msg) => assert_eq!(msg, "Invalid token"),
            other => panic!("Expected AuthError::Invalid, got: {:?}", other),
        }
    }
}
