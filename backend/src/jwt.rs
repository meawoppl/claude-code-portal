//! JWT utilities for proxy token authentication
//!
//! This module provides functions for creating and verifying JWT tokens
//! used by the proxy CLI to authenticate with the backend.

use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use sha2::{Digest, Sha256};
use shared::ProxyTokenClaims;
use uuid::Uuid;

/// Error type for JWT operations
#[derive(Debug, thiserror::Error)]
pub enum JwtError {
    #[error("Failed to encode JWT: {0}")]
    Encode(#[from] jsonwebtoken::errors::Error),

    #[error("Invalid token: {0}")]
    Invalid(String),

    #[error("Token expired")]
    Expired,

    #[error("Missing secret key")]
    MissingSecret,
}

/// Create a new JWT token for proxy authentication
pub fn create_proxy_token(
    secret: &[u8],
    token_id: Uuid,
    user_id: Uuid,
    email: &str,
    expires_in_days: u32,
) -> Result<String, JwtError> {
    let now = Utc::now();
    let exp = now + Duration::days(expires_in_days as i64);

    let claims = ProxyTokenClaims {
        jti: token_id,
        sub: user_id,
        email: email.to_string(),
        iat: now.timestamp(),
        exp: exp.timestamp(),
    };

    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret),
    )?;

    Ok(token)
}

/// Verify and decode a JWT token
pub fn verify_proxy_token(secret: &[u8], token: &str) -> Result<ProxyTokenClaims, JwtError> {
    let mut validation = Validation::default();
    validation.validate_exp = true;

    let token_data = decode::<ProxyTokenClaims>(
        token,
        &DecodingKey::from_secret(secret),
        &validation,
    )
    .map_err(|e| match e.kind() {
        jsonwebtoken::errors::ErrorKind::ExpiredSignature => JwtError::Expired,
        _ => JwtError::Invalid(e.to_string()),
    })?;

    Ok(token_data.claims)
}

/// Compute SHA256 hash of a token for storage
pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_verify_token() {
        let secret = b"test-secret-key-at-least-32-bytes";
        let token_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let email = "test@example.com";

        let token = create_proxy_token(secret, token_id, user_id, email, 30).unwrap();
        let claims = verify_proxy_token(secret, &token).unwrap();

        assert_eq!(claims.jti, token_id);
        assert_eq!(claims.sub, user_id);
        assert_eq!(claims.email, email);
    }

    #[test]
    fn test_invalid_token() {
        let secret = b"test-secret-key-at-least-32-bytes";
        let result = verify_proxy_token(secret, "invalid-token");
        assert!(result.is_err());
    }

    #[test]
    fn test_wrong_secret() {
        let secret1 = b"test-secret-key-at-least-32-bytes";
        let secret2 = b"different-secret-key-32-bytes!!";
        let token_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();

        let token = create_proxy_token(secret1, token_id, user_id, "test@example.com", 30).unwrap();
        let result = verify_proxy_token(secret2, &token);
        assert!(result.is_err());
    }

    #[test]
    fn test_hash_token() {
        let token = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9";
        let hash = hash_token(token);
        assert_eq!(hash.len(), 64); // SHA256 produces 32 bytes = 64 hex chars
    }
}
