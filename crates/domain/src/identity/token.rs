use std::fmt;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use secrecy::{ExposeSecret, SecretString};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

#[derive(Clone, Copy)]
pub struct TokenHash([u8; 32]);

impl TokenHash {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl PartialEq for TokenHash {
    fn eq(&self, other: &Self) -> bool {
        self.0.ct_eq(&other.0).into()
    }
}

impl Eq for TokenHash {}

impl fmt::Debug for TokenHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TokenHash([REDACTED])")
    }
}

macro_rules! opaque_token {
    ($name:ident) => {
        pub struct $name(SecretString);

        impl $name {
            #[must_use]
            pub fn from_random_bytes(bytes: [u8; 32]) -> Self {
                Self(SecretString::from(URL_SAFE_NO_PAD.encode(bytes)))
            }

            #[must_use]
            pub fn parse(value: SecretString) -> Self {
                Self(value)
            }

            #[must_use]
            pub fn hash_for_storage(&self) -> TokenHash {
                let digest = Sha256::digest(self.0.expose_secret().as_bytes());
                let mut bytes = [0_u8; 32];
                bytes.copy_from_slice(&digest);
                TokenHash(bytes)
            }

            #[must_use]
            pub fn into_secret(self) -> SecretString {
                self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!(stringify!($name), "([REDACTED])"))
            }
        }
    };
}

opaque_token!(EmailVerificationToken);
opaque_token!(PasswordResetToken);
opaque_token!(SessionToken);
opaque_token!(CsrfToken);
