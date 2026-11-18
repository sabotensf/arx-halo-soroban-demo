//! `halo-tool` — helpers for using HaLo NFC chips (Arx Research) with the
//! Soroban Physical Backed Token contract.
//!
//! The contract uses HaLo key slot 1, the slot libhalo's `sign` command can
//! use for raw digests. Everything follows Stellar conventions:
//!
//! ```text
//! chip_id     = SHA-256(uncompressed_pubkey)
//! digest      = SHA-256("Stellar Signed Message:\n" || message)
//! signature   = ECDSA-secp256k1(digest) → r || s
//! recovery_id = v − 27
//! ```
//!
//! `chip_id` is 32 bytes, `digest` is the SEP-53 digest, and `r`, `s` and `v`
//! come from libhalo's `signature.raw`.
//!
//! The chip signs the SEP-53 digest with its secp256k1 key, because it can't
//! sign Ed25519 like a Stellar account. The `sim-*` subcommands emulate a
//! chip for tests and the testnet demo.
//!
//! Formats follow libhalo's documentation and source (MIT), in
//! <https://github.com/arx-research/libhalo>:
//!
//! - `docs/halo-command-set.md`: the `sign` and `get_pkeys` commands
//! - `docs/key-slots.md`: why key slot 1 (it signs raw digests)
//! - `docs/api-web.md`, `docs/api-utils.md`: `signature.raw` and its `v`
//! - `core/src.ts/halo/commands.ts`: how `sign` builds its result
//!
//! The tap-URL parameters (`pk1`, `static`) are from Arx's HaLo spec:
//! <https://arx-docs.vercel.app/HaLo/params>. Not tested on a physical chip.
//!
//! ```text
//! # chip_id from libhalo get_pkeys publicKeys["1"]
//! halo-tool chip-id <pubkey-hex>
//!
//! # SEP-53 digest to pass to HaLo `sign { keyNo: 1, digest }`
//! halo-tool digest <message-hex>
//!
//! # libhalo signature.raw → "<signature> <recovery_id>"
//! halo-tool from-halo <r> <s> <v>
//!
//! # simulated chip; prints its chip_id
//! halo-tool sim-keygen <card.key>
//!
//! # its uncompressed public key (pk1 in a tap URL)
//! halo-tool sim-pubkey <card.key>
//!
//! # simulated tap → "<signature> <recovery_id>"
//! halo-tool sim-sign <card.key> <message-hex>
//! ```

use std::{env, fs, process::exit};

use k256::ecdsa::{SigningKey, VerifyingKey};
use rand_core::OsRng;
use sha2::{Digest, Sha256};

// ======================================================================
// Stellar conventions
//
// The same two calculations the contract does on-chain.
// ======================================================================

/// SEP-53 ("Sign and Verify Messages") prefix.
pub const SEP53_PREFIX: &[u8] = b"Stellar Signed Message:\n";

/// A chip's id: SHA-256 of its public key in uncompressed form (65 bytes,
/// starting `04`).
///
/// The same key can be written compressed (33 bytes) or uncompressed. Always
/// hashing the uncompressed form means both give the same id.
pub fn chip_id(public_key: &VerifyingKey) -> [u8; 32] {
    let uncompressed = public_key.to_encoded_point(false);
    Sha256::digest(uncompressed.as_bytes()).into()
}

/// The 32 bytes the chip actually signs:
/// `SHA-256("Stellar Signed Message:\n" || message)`.
pub fn sep53_digest(message: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(SEP53_PREFIX);
    hasher.update(message);
    hasher.finalize().into()
}

// ======================================================================
// Converting a real chip's signature
// ======================================================================

/// A signature in the form the contract takes it.
pub struct ChipSignature {
    /// `r` followed by `s`, 32 bytes each.
    pub r_and_s: [u8; 64],
    /// 0 or 1: tells `secp256k1_recover` which of two possible public keys
    /// made the signature.
    pub recovery_id: u8,
}

impl ChipSignature {
    /// As printed on the command line: `<r_and_s hex> <recovery_id>`.
    pub fn to_cli(&self) -> String {
        format!("{} {}", hex::encode(self.r_and_s), self.recovery_id)
    }
}

/// Converts libhalo's `signature.raw` (`r`, `s`, `v`) into a [`ChipSignature`].
///
/// libhalo reports `v` the Ethereum way, as 27 or 28, so the recovery id is
/// `v - 27`. A `v` of 0 or 1 is already a recovery id and is passed through.
pub fn from_halo(r: &[u8], s: &[u8], v: u8) -> Result<ChipSignature, String> {
    if r.len() != 32 || s.len() != 32 {
        return Err("r and s must be 32 bytes each".into());
    }
    let recovery_id = match v {
        27 | 28 => v - 27,
        0 | 1 => v,
        _ => return Err(format!("unexpected v = {v}")),
    };

    let mut r_and_s = [0u8; 64];
    r_and_s[..32].copy_from_slice(r);
    r_and_s[32..].copy_from_slice(s);
    Ok(ChipSignature {
        r_and_s,
        recovery_id,
    })
}

// ======================================================================
// Simulated chip
//
// Stands in for a HaLo chip's key slot 1 in the tests and the testnet demo.
// The private key is kept in a file as hex; a real chip never reveals it.
// ======================================================================

pub struct SimChip {
    key: SigningKey,
}

impl SimChip {
    /// A new chip with a random key.
    pub fn generate() -> Self {
        Self {
            key: SigningKey::random(&mut OsRng),
        }
    }

    /// Reads a chip's private key from a file written by [`SimChip::save`].
    pub fn load(path: &str) -> Self {
        let contents =
            fs::read_to_string(path).unwrap_or_else(|e| fail(&format!("cannot read {path}: {e}")));
        let key = SigningKey::from_slice(&parse_hex("key file", &contents))
            .unwrap_or_else(|_| fail("invalid secp256k1 secret key"));
        Self { key }
    }

    pub fn save(&self, path: &str) {
        fs::write(path, hex::encode(self.key.to_bytes()))
            .unwrap_or_else(|e| fail(&format!("cannot write {path}: {e}")));
    }

    /// The uncompressed public key: what HaLo's `get_pkeys` returns as
    /// `publicKeys["1"]`, and what a tap URL carries as `pk1`.
    pub fn public_key(&self) -> Vec<u8> {
        self.key
            .verifying_key()
            .to_encoded_point(false)
            .as_bytes()
            .to_vec()
    }

    pub fn chip_id(&self) -> [u8; 32] {
        chip_id(self.key.verifying_key())
    }

    /// What a tap does: sign the SEP-53 digest of `message`, as HaLo's
    /// `sign { keyNo: 1, digest }` would.
    pub fn sign(&self, message: &[u8]) -> ChipSignature {
        let digest = sep53_digest(message);
        let (signature, recovery_id) = self
            .key
            .sign_prehash_recoverable(&digest)
            .expect("signing a 32-byte digest can't fail");
        ChipSignature {
            r_and_s: signature.to_bytes().into(),
            recovery_id: recovery_id.to_byte(),
        }
    }
}

// ======================================================================
// Command line
// ======================================================================

const USAGE: &str = "\
usage:
  halo-tool chip-id <pubkey-hex>
  halo-tool digest <message-hex>
  halo-tool from-halo <r-hex> <s-hex> <v>
  halo-tool sim-keygen <card.key>
  halo-tool sim-pubkey <card.key>
  halo-tool sim-sign <card.key> <message-hex>";

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();

    match args.as_slice() {
        // Working with a real chip
        ["chip-id", pubkey] => cmd_chip_id(pubkey),
        ["digest", message] => cmd_digest(message),
        ["from-halo", r, s, v] => cmd_from_halo(r, s, v),

        // Working with a simulated chip
        ["sim-keygen", path] => cmd_sim_keygen(path),
        ["sim-pubkey", path] => println!("{}", hex::encode(SimChip::load(path).public_key())),
        ["sim-sign", path, message] => {
            let message = parse_hex("message", message);
            println!("{}", SimChip::load(path).sign(&message).to_cli());
        }

        _ => {
            eprintln!("{USAGE}");
            exit(2)
        }
    }
}

/// `chip-id <pubkey>`: chip id from a public key, compressed or uncompressed.
fn cmd_chip_id(pubkey: &str) {
    let key = VerifyingKey::from_sec1_bytes(&parse_hex("pubkey", pubkey))
        .unwrap_or_else(|_| fail("invalid secp256k1 public key"));
    println!("{}", hex::encode(chip_id(&key)));
}

/// `digest <message>`: the SEP-53 digest to pass to HaLo's `sign` command.
fn cmd_digest(message: &str) {
    let digest = sep53_digest(&parse_hex("message", message));
    println!("{}", hex::encode(digest));
}

/// `from-halo <r> <s> <v>`: libhalo's `signature.raw`, ready for the contract.
fn cmd_from_halo(r: &str, s: &str, v: &str) {
    let v = parse_v(v);
    match from_halo(&parse_hex("r", r), &parse_hex("s", s), v) {
        Ok(signature) => println!("{}", signature.to_cli()),
        Err(e) => fail(&e),
    }
}

/// `sim-keygen <path>`: creates a simulated chip and prints its chip id.
fn cmd_sim_keygen(path: &str) {
    let chip = SimChip::generate();
    chip.save(path);
    println!("{}", hex::encode(chip.chip_id()));
}

// ======================================================================
// Helpers
// ======================================================================

/// Hex to bytes, with or without a `0x` prefix. `what` names the argument in
/// the error message.
fn parse_hex(what: &str, text: &str) -> Vec<u8> {
    hex::decode(text.trim().trim_start_matches("0x"))
        .unwrap_or_else(|_| fail(&format!("{what} must be hex")))
}

/// `v` as decimal (`27`), or as hex (`0x1b` or `1b`).
fn parse_v(text: &str) -> u8 {
    let digits = text.trim_start_matches("0x");
    digits
        .parse()
        .or_else(|_| u8::from_str_radix(digits, 16))
        .unwrap_or_else(|_| fail("v must be a number"))
}

/// Prints an error and exits with status 1.
fn fail(message: &str) -> ! {
    eprintln!("{message}");
    exit(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use k256::ecdsa::{RecoveryId, Signature};

    fn test_chip() -> SimChip {
        SimChip {
            key: SigningKey::from_slice(&[0x42; 32]).unwrap(),
        }
    }

    #[test]
    fn sep53_prefix_and_digest() {
        assert_eq!(SEP53_PREFIX, "Stellar Signed Message:\n".as_bytes());
        let mut expected = b"Stellar Signed Message:\n".to_vec();
        expected.extend_from_slice(b"Hello, World!");
        assert_eq!(
            sep53_digest(b"Hello, World!"),
            <[u8; 32]>::from(Sha256::digest(expected))
        );
    }

    #[test]
    fn chip_id_is_sha256_of_uncompressed_key() {
        let chip = test_chip();
        let key = chip.key.verifying_key();

        // Compressed and uncompressed forms of the same key give the same id.
        let compressed = key.to_encoded_point(true);
        let from_compressed = VerifyingKey::from_sec1_bytes(compressed.as_bytes()).unwrap();
        assert_eq!(chip_id(&from_compressed), chip_id(key));

        assert_eq!(
            chip.chip_id(),
            <[u8; 32]>::from(Sha256::digest(chip.public_key()))
        );
    }

    #[test]
    fn sim_sign_and_from_halo_recover_the_chip() {
        let chip = test_chip();
        let message = [7u8; 32];
        let signed = chip.sign(&message);

        // The same signature as libhalo would report it (v = 27 + recovery id).
        let r = &signed.r_and_s[..32];
        let s = &signed.r_and_s[32..];
        let converted = from_halo(r, s, 27 + signed.recovery_id).unwrap();
        assert_eq!(converted.r_and_s, signed.r_and_s);
        assert_eq!(converted.recovery_id, signed.recovery_id);

        // Recovering the key from the signature gives back the chip's id,
        // which is the check the contract makes.
        let recovered = VerifyingKey::recover_from_prehash(
            &sep53_digest(&message),
            &Signature::from_slice(&signed.r_and_s).unwrap(),
            RecoveryId::from_byte(signed.recovery_id).unwrap(),
        )
        .unwrap();
        assert_eq!(chip_id(&recovered), chip.chip_id());
    }
}
