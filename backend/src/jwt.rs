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
        token_type: "proxy".to_string(),
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

    let token_data =
        decode::<ProxyTokenClaims>(token, &DecodingKey::from_secret(secret), &validation).map_err(
            |e| match e.kind() {
                jsonwebtoken::errors::ErrorKind::ExpiredSignature => JwtError::Expired,
                _ => JwtError::Invalid(e.to_string()),
            },
        )?;

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

    #[test]
    fn test_hash_token_consistency() {
        // Same token should always produce same hash
        let token = "test-token-12345";
        let hash1 = hash_token(token);
        let hash2 = hash_token(token);
        assert_eq!(hash1, hash2, "Hash should be deterministic");
    }

    #[test]
    fn test_hash_token_uniqueness() {
        // Different tokens should produce different hashes
        let hash1 = hash_token("token-1");
        let hash2 = hash_token("token-2");
        assert_ne!(
            hash1, hash2,
            "Different tokens should have different hashes"
        );
    }

    #[test]
    fn test_token_expiration_claims() {
        let secret = b"test-secret-key-at-least-32-bytes";
        let token_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let email = "test@example.com";

        // Create token with 30 day expiration
        let token = create_proxy_token(secret, token_id, user_id, email, 30).unwrap();
        let claims = verify_proxy_token(secret, &token).unwrap();

        // Verify expiration is roughly 30 days from now
        let now = Utc::now().timestamp();
        let expected_exp = now + (30 * 24 * 60 * 60); // 30 days in seconds

        // Allow 60 seconds tolerance for test execution time
        assert!(
            (claims.exp - expected_exp).abs() < 60,
            "Expiration should be approximately 30 days from now"
        );

        // Verify iat is close to now
        assert!(
            (claims.iat - now).abs() < 60,
            "Issued-at should be close to now"
        );
    }

    #[test]
    fn test_device_flow_token_scenario() {
        // Simulate the device flow token creation scenario:
        // 1. Generate token ID
        // 2. Create JWT with user info
        // 3. Hash the token for database storage
        // 4. Verify the token can be decoded

        let secret = b"device-flow-secret-at-least-32-bytes";
        let token_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let user_email = "device-user@example.com";
        let expires_in_days = 30u32; // Device tokens valid for 30 days

        // Step 1: Create the JWT token
        let token = create_proxy_token(secret, token_id, user_id, user_email, expires_in_days)
            .expect("Token creation should succeed");

        // Step 2: Hash for database storage
        let token_hash = hash_token(&token);
        assert_eq!(token_hash.len(), 64, "Hash should be 64 hex chars");

        // Step 3: Verify we can decode the token
        let claims = verify_proxy_token(secret, &token).expect("Token verification should succeed");

        // Step 4: Verify all claims match
        assert_eq!(claims.jti, token_id, "Token ID should match");
        assert_eq!(claims.sub, user_id, "User ID should match");
        assert_eq!(claims.email, user_email, "Email should match");

        // Step 5: Verify hash can be used to lookup token
        // (In real code, this would be a database lookup)
        let stored_hash = token_hash.clone();
        let new_hash = hash_token(&token);
        assert_eq!(
            stored_hash, new_hash,
            "Re-hashing token should match stored hash"
        );
    }

    #[test]
    fn test_jwt_error_types() {
        let secret = b"test-secret-key-at-least-32-bytes";

        // Test Invalid error
        let result = verify_proxy_token(secret, "not-a-jwt");
        assert!(matches!(result, Err(JwtError::Invalid(_))));

        // Test wrong secret (also Invalid)
        let token_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let token = create_proxy_token(secret, token_id, user_id, "test@test.com", 30).unwrap();
        let wrong_secret = b"wrong-secret-key-at-least-32-bytes";
        let result = verify_proxy_token(wrong_secret, &token);
        assert!(matches!(result, Err(JwtError::Invalid(_))));
    }

    #[test]
    fn test_multiple_tokens_same_user() {
        // A user might have multiple device flow tokens
        let secret = b"test-secret-key-at-least-32-bytes";
        let user_id = Uuid::new_v4();
        let email = "multi-device@example.com";

        // Create multiple tokens for same user
        let mut tokens = Vec::new();
        let mut hashes = std::collections::HashSet::new();

        for _ in 0..5 {
            let token_id = Uuid::new_v4();
            let token = create_proxy_token(secret, token_id, user_id, email, 30).unwrap();
            let hash = hash_token(&token);

            // Verify token
            let claims = verify_proxy_token(secret, &token).unwrap();
            assert_eq!(claims.sub, user_id);
            assert_eq!(claims.email, email);

            // Each token should have unique hash
            assert!(
                hashes.insert(hash.clone()),
                "Each token should have unique hash"
            );

            tokens.push((token, hash));
        }

        // Verify all tokens are still valid
        for (token, _) in &tokens {
            let claims = verify_proxy_token(secret, token).unwrap();
            assert_eq!(claims.sub, user_id);
        }
    }
}
