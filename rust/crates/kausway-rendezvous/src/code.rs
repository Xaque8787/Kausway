use std::fmt;

use hkdf::Hkdf;
use iroh::{PublicKey, SecretKey};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::Error;

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A bearer secret. Display is deliberately absent; expose only to the requesting user.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct Code(String);

impl fmt::Debug for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Code([REDACTED])")
    }
}

impl Code {
    pub fn generate() -> Self {
        let mut random = [0u8; 18];
        rand::rng().fill_bytes(&mut random);
        let mut value = String::from("K1");
        for byte in &random {
            value.push(ALPHABET[(byte & 31) as usize] as char);
        }
        random.zeroize();
        Self(value)
    }

    pub fn parse(input: &str) -> Result<Self, Error> {
        // Bound work before normalization. Only ASCII spaces and hyphens are separators.
        if input.len() > 64 || !input.is_ascii() {
            return Err(Error::InvalidCode);
        }
        let mut value = String::with_capacity(20);
        for byte in input.bytes().filter(|b| *b != b' ' && *b != b'-') {
            value.push(byte.to_ascii_uppercase() as char);
        }
        if value.len() != 20 || !value.starts_with("K1") {
            return Err(Error::InvalidCode);
        }
        let mut normalized = String::from("K1");
        for byte in value.bytes().skip(2) {
            let byte = match byte {
                b'O' => b'0',
                b'I' | b'L' => b'1',
                other => other,
            };
            if !ALPHABET.contains(&byte) {
                return Err(Error::InvalidCode);
            }
            normalized.push(byte as char);
        }
        value.zeroize();
        Ok(Self(normalized))
    }

    pub fn expose(&self) -> String {
        format!(
            "K1-{}-{}-{}",
            &self.0[2..8],
            &self.0[8..14],
            &self.0[14..20]
        )
    }

    pub fn secrets(&self) -> Secrets {
        let hkdf = Hkdf::<Sha256>::new(Some(b"kausway-rendezvous-v1"), self.0.as_bytes());
        let mut signing_seed = [0; 32];
        let mut encryption_key = [0; 32];
        hkdf.expand(b"pkarr-ed25519-seed", &mut signing_seed)
            .expect("fixed HKDF length");
        hkdf.expand(b"invite-encryption-key", &mut encryption_key)
            .expect("fixed HKDF length");
        Secrets {
            signing_seed,
            encryption_key,
        }
    }
}

/// Private recovery material; possession permits reading and replacing the DHT record.
#[derive(Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields)]
pub struct Secrets {
    signing_seed: [u8; 32],
    pub(crate) encryption_key: [u8; 32],
}

impl fmt::Debug for Secrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secrets([REDACTED])")
    }
}

impl Secrets {
    pub fn public_key(&self) -> PublicKey {
        self.signing_key().public()
    }

    pub(crate) fn signing_key(&self) -> SecretKey {
        SecretKey::from_bytes(&self.signing_seed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_and_key_separation() {
        let a = Code::parse("k1-oooooo-iiiiii-llllll").unwrap();
        assert_eq!(a.expose(), "K1-000000-111111-111111");
        let b = Code::parse("K1000000111111111111").unwrap();
        assert_eq!(a.secrets().public_key(), b.secrets().public_key());
        let secrets = a.secrets();
        assert_ne!(secrets.signing_seed, secrets.encryption_key);
        // RFC 5869 independently reproduced with Python hashlib/hmac.
        let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert_eq!(
            hex(&secrets.signing_seed),
            "face4b29c6746857fc3f81e65adcd2458018bac59a7c8e669d0b7be96fc5371b"
        );
        assert_eq!(
            hex(&secrets.encryption_key),
            "6e1aab5cea20024138ec3d248af020e280ac8e70099795aa17067b2e9e7cbcd5"
        );
        assert!(!format!("{a:?} {secrets:?}").contains("000000"));
    }

    #[test]
    fn strict_version_length_and_alphabet() {
        for invalid in [
            "",
            "123456",
            "K2-000000-111111-111111",
            "K1-000000-111111-11111U",
            "K1-000000-111111-111111\n",
            "Ｋ1-000000-111111-111111",
        ] {
            assert!(Code::parse(invalid).is_err(), "{invalid}");
        }
        let a = Code::generate();
        let b = Code::generate();
        assert_ne!(a.expose(), b.expose());
        assert_eq!(Code::parse(&a.expose()).unwrap().expose(), a.expose());
    }
}
