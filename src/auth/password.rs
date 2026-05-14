use anyhow::{Result, anyhow};
use argon2::Argon2;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};

/// Hash a plaintext password with Argon2id and a fresh random salt.
///
/// ### Arguments
/// - `plaintext`: The password to hash. Never logged.
///
/// ### Returns
/// - `Ok(String)`: PHC-encoded Argon2id hash including parameters and salt.
/// - `Err`: The Argon2 backend rejected the input.
pub fn hash(plaintext: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(plaintext.as_bytes(), &salt)
        .map_err(|e| anyhow!("argon2 hashing failed: {e}"))?;
    Ok(hash.to_string())
}

/// Verify a plaintext password against a previously produced hash.
///
/// ### Arguments
/// - `plaintext`: The password supplied by the user. Never logged.
/// - `phc_hash`: A PHC-encoded Argon2id hash as produced by [`hash`].
///
/// ### Returns
/// - `Ok(true)`: The plaintext matches the hash.
/// - `Ok(false)`: The plaintext does not match the hash.
/// - `Err`: The stored hash could not be parsed (corrupted column).
pub fn verify(plaintext: &str, phc_hash: &str) -> Result<bool> {
    let parsed = PasswordHash::new(phc_hash).map_err(|e| anyhow!("invalid password hash: {e}"))?;
    Ok(Argon2::default()
        .verify_password(plaintext.as_bytes(), &parsed)
        .is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify_roundtrip() {
        let h = hash("correct horse battery staple").unwrap();
        assert!(verify("correct horse battery staple", &h).unwrap());
        assert!(!verify("wrong password", &h).unwrap());
    }

    #[test]
    fn each_hash_is_unique() {
        let a = hash("same").unwrap();
        let b = hash("same").unwrap();
        assert_ne!(a, b, "fresh salt must produce different hashes");
        assert!(verify("same", &a).unwrap());
        assert!(verify("same", &b).unwrap());
    }
}
