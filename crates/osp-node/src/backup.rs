//! Identity backup and recovery (ADR 0009).
//!
//! The identity key is written as 24 recovery words from the BIP39 English
//! list. The words encode the key's 32 bytes directly (BIP39's entropy
//! encoding, not its seed derivation), so every existing key has words, and
//! the last word carries a checksum that catches a mistyped word.

use std::path::Path;

use anyhow::{Result, anyhow, bail};
use bip39::{Language, Mnemonic};
use iroh::SecretKey;

use crate::{home::Home, keys};

/// 256 bits of key and an 8-bit checksum, at 11 bits per word.
pub const WORD_COUNT: usize = 24;

/// The recovery words of the identity of the node in `home`.
pub fn recovery_words(home: &Path) -> Result<Vec<&'static str>> {
    Ok(words(&keys::load(&Home::new(home).identity_key())?))
}

fn words(key: &SecretKey) -> Vec<&'static str> {
    Mnemonic::from_entropy_in(Language::English, &key.to_bytes())
        .expect("32 bytes is a valid BIP39 entropy length")
        .words()
        .collect()
}

/// The identity key that recovery words stand for. Case, extra spaces and
/// line breaks don't matter.
pub(crate) fn identity_from_words(text: &str) -> Result<SecretKey> {
    let typed: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
    if typed.len() != WORD_COUNT {
        bail!("expected {WORD_COUNT} recovery words, got {}", typed.len());
    }
    let mnemonic = Mnemonic::parse_in_normalized(Language::English, &typed.join(" ")).map_err(
        |e| match e {
            bip39::Error::UnknownWord(i) => {
                anyhow!("word {} ({:?}) isn't a recovery word", i + 1, typed[i])
            }
            bip39::Error::InvalidChecksum => {
                anyhow!("the words don't add up; check for a mistyped or swapped word")
            }
            other => anyhow!("not valid recovery words: {other}"),
        },
    )?;
    let (entropy, len) = mnemonic.to_entropy_array();
    let bytes: [u8; 32] = entropy[..len]
        .try_into()
        .map_err(|_| anyhow!("expected {WORD_COUNT} recovery words"))?;
    Ok(SecretKey::from_bytes(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_and_key_round_trip() {
        let key = SecretKey::generate();
        let words = words(&key);
        assert_eq!(words.len(), WORD_COUNT);
        let back = identity_from_words(&words.join(" ")).unwrap();
        assert_eq!(back.to_bytes(), key.to_bytes());
    }

    #[test]
    fn words_are_the_bip39_encoding_of_the_key_bytes() {
        // Test vectors from BIP39: the words for 32 bytes of 0x00 and of 0xff.
        let zeros = words(&SecretKey::from_bytes(&[0; 32])).join(" ");
        assert_eq!(zeros, format!("{}art", "abandon ".repeat(23)));
        let ones = words(&SecretKey::from_bytes(&[0xff; 32])).join(" ");
        assert_eq!(ones, format!("{}vote", "zoo ".repeat(23)));
    }

    #[test]
    fn case_and_spacing_dont_matter() {
        let key = SecretKey::generate();
        let words = words(&key);
        let messy = format!(
            "  {}\n{}  ",
            words[..12].join("  ").to_uppercase(),
            words[12..].join("\t")
        );
        let back = identity_from_words(&messy).unwrap();
        assert_eq!(back.to_bytes(), key.to_bytes());
    }

    #[test]
    fn wrong_words_are_refused() {
        let mut words = words(&SecretKey::from_bytes(&[7; 32]));
        let err = identity_from_words(&words[..23].join(" ")).unwrap_err();
        assert!(err.to_string().contains("got 23"), "{err}");

        let mut typo = words.clone();
        typo[4] = "abandonn";
        let err = identity_from_words(&typo.join(" ")).unwrap_err();
        assert!(err.to_string().contains("word 5"), "{err}");

        // Every word is on the list, but two are swapped: the checksum catches it.
        assert_ne!(words[3], words[9]);
        words.swap(3, 9);
        let err = identity_from_words(&words.join(" ")).unwrap_err();
        assert!(err.to_string().contains("don't add up"), "{err}");
    }
}
