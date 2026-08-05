use argon2::{
    Algorithm, Argon2, Params, Version,
    password_hash::{PasswordHash, PasswordHasher as _, PasswordVerifier as _, SaltString},
};
use secrecy::{ExposeSecret, SecretString};
use thiserror::Error;

use super::RandomSource;

const MEMORY_KIB: u32 = 19 * 1024;
const ITERATIONS: u32 = 2;
const PARALLELISM: u32 = 1;
const DUMMY_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHQxMjM0NTY3OA$8HOMJHMYfVc83GmOFzeWk1JI9iw9nV0sH0qewvPPY7k";

#[derive(Debug, Error)]
#[error("password hashing failed")]
pub struct PasswordHashError;

pub trait PasswordHasher: Send + Sync {
    /// Produces a versioned password hash suitable for durable storage.
    ///
    /// # Errors
    ///
    /// Returns [`PasswordHashError`] when parameters, salt encoding, or hashing fails.
    fn hash(&self, password: &SecretString) -> Result<String, PasswordHashError>;
    fn verify(&self, password: &SecretString, hash: &str) -> bool;
    fn verify_dummy(&self, password: &SecretString);
}

pub struct Argon2PasswordHasher<R> {
    random: R,
}

impl<R> Argon2PasswordHasher<R> {
    #[must_use]
    pub const fn new(random: R) -> Self {
        Self { random }
    }

    fn algorithm() -> Result<Argon2<'static>, PasswordHashError> {
        let params = Params::new(MEMORY_KIB, ITERATIONS, PARALLELISM, None)
            .map_err(|_| PasswordHashError)?;
        Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
    }
}

impl<R: RandomSource> PasswordHasher for Argon2PasswordHasher<R> {
    fn hash(&self, password: &SecretString) -> Result<String, PasswordHashError> {
        let mut salt = [0_u8; 16];
        self.random.fill(&mut salt);
        let salt = SaltString::encode_b64(&salt).map_err(|_| PasswordHashError)?;
        Self::algorithm()?
            .hash_password(password.expose_secret().as_bytes(), &salt)
            .map(|hash| hash.to_string())
            .map_err(|_| PasswordHashError)
    }

    fn verify(&self, password: &SecretString, hash: &str) -> bool {
        let Ok(hash) = PasswordHash::new(hash) else {
            return false;
        };
        Self::algorithm().is_ok_and(|argon| {
            argon
                .verify_password(password.expose_secret().as_bytes(), &hash)
                .is_ok()
        })
    }

    fn verify_dummy(&self, password: &SecretString) {
        let _ = self.verify(password, DUMMY_HASH);
    }
}
