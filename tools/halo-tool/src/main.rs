//! `halo-tool` — bridges HaLo NFC chips (Arx Research) and Soroban.
//!
//! A RecordPool card carries a HaLo chip. Its key slot 2 holds a secp256k1
//! key that never leaves the chip and can't sign arbitrary digests: it only
//! signs domain-separated challenges via libhalo's `sign_challenge` command:
//!
//! ```text
//! signature = ECDSA-secp256k1( SHA-256( "\x19External chall pk2:\n" || challenge32 ) )   // DER
//! ```
//!
//! Soroban's `secp256k1_recover` wants a 64-byte r‖s signature with a low-S
//! value plus a recovery id, which HaLo doesn't return for this command. The
//! `prepare` subcommand does that conversion for a **real** HaLo response; the
//! `sim-*` subcommands emulate a chip for tests and the testnet demo.
//!
//! ```text
//! halo-tool challenge                                   # fresh 32-byte challenge (hex)
//! halo-tool prepare <pk2-hex> <challenge-hex> <der-hex> # -> "<signature-hex> <recovery-id>"
//! halo-tool sim-keygen <card.key>                       # simulated chip; prints pk2 (65-byte, 04…)
//! halo-tool sim-sign-challenge <card.key> <challenge>   # simulated sign_challenge; prints DER
//! ```

use std::{env, fs, process::exit};

use k256::ecdsa::{RecoveryId, Signature, SigningKey, VerifyingKey};
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};

/// Domain-separation prefix HaLo applies to `sign_challenge` on key slot 2.
pub const CHALLENGE_PREFIX: &[u8] = b"\x19External chall pk2:\n";

/// The 32-byte digest HaLo signs for `sign_challenge` (and the contract recomputes).
pub fn challenge_digest(challenge: &[u8; 32]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(CHALLENGE_PREFIX);
    h.update(challenge);
    h.finalize().into()
}

/// Converts a HaLo DER signature into Soroban form: 64-byte r‖s with low S,
/// plus the recovery id that makes `secp256k1_recover` return `pubkey`.
pub fn prepare(
    pubkey: &VerifyingKey,
    challenge: &[u8; 32],
    der: &[u8],
) -> Result<([u8; 64], u8), String> {
    let sig = Signature::from_der(der).map_err(|e| format!("bad DER signature: {e}"))?;
    let sig = sig.normalize_s().unwrap_or(sig);
    let digest = challenge_digest(challenge);
    for id in 0..=1u8 {
        let rid = RecoveryId::from_byte(id).expect("0/1 are valid");
        if VerifyingKey::recover_from_prehash(&digest, &sig, rid)
            .ok()
            .as_ref()
            == Some(pubkey)
        {
            return Ok((sig.to_bytes().into(), id));
        }
    }
    Err("signature does not match this public key and challenge".into())
}

/// Simulated chip: what a HaLo returns for `sign_challenge` (DER, not normalised).
pub fn sim_sign_challenge(key: &SigningKey, challenge: &[u8; 32]) -> Vec<u8> {
    let (sig, _) = key
        .sign_prehash_recoverable(&challenge_digest(challenge))
        .expect("sign");
    sig.to_der().as_bytes().to_vec()
}

fn pubkey_hex(key: &VerifyingKey) -> String {
    hex::encode(key.to_encoded_point(false).as_bytes())
}

fn hex32(s: &str) -> [u8; 32] {
    let v = hex::decode(s).unwrap_or_else(|_| die("challenge must be hex"));
    v.try_into()
        .unwrap_or_else(|_| die("challenge must be exactly 32 bytes"))
}

fn load(path: &str) -> SigningKey {
    let raw = fs::read_to_string(path).unwrap_or_else(|e| die(&format!("cannot read {path}: {e}")));
    let bytes = hex::decode(raw.trim()).unwrap_or_else(|_| die("key file must be hex"));
    SigningKey::from_slice(&bytes).unwrap_or_else(|_| die("invalid secp256k1 secret key"))
}

fn die(msg: &str) -> ! {
    eprintln!("{msg}");
    exit(1)
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["challenge"] => {
            let mut buf = [0u8; 32];
            OsRng.fill_bytes(&mut buf);
            println!("{}", hex::encode(buf));
        }
        ["prepare", pk, challenge, der] => {
            let pk_bytes = hex::decode(pk).unwrap_or_else(|_| die("pubkey must be hex"));
            let pubkey = VerifyingKey::from_sec1_bytes(&pk_bytes)
                .unwrap_or_else(|_| die("invalid secp256k1 public key"));
            let der = hex::decode(der).unwrap_or_else(|_| die("signature must be hex"));
            match prepare(&pubkey, &hex32(challenge), &der) {
                Ok((sig, id)) => println!("{} {id}", hex::encode(sig)),
                Err(e) => die(&e),
            }
        }
        ["sim-keygen", path] => {
            let key = SigningKey::random(&mut OsRng);
            fs::write(path, hex::encode(key.to_bytes())).expect("write key file");
            println!("{}", pubkey_hex(key.verifying_key()));
        }
        ["sim-pubkey", path] => println!("{}", pubkey_hex(load(path).verifying_key())),
        ["sim-sign-challenge", path, challenge] => {
            println!(
                "{}",
                hex::encode(sim_sign_challenge(&load(path), &hex32(challenge)))
            );
        }
        _ => {
            eprintln!(
                "usage:\n  halo-tool challenge\n  halo-tool prepare <pk2-hex> <challenge-hex> <der-hex>\n  \
                 halo-tool sim-keygen <card.key>\n  halo-tool sim-pubkey <card.key>\n  \
                 halo-tool sim-sign-challenge <card.key> <challenge-hex>"
            );
            exit(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k256::elliptic_curve::{ops::Neg, scalar::IsHigh};

    fn key() -> SigningKey {
        SigningKey::from_slice(&[0x42; 32]).unwrap()
    }

    #[test]
    fn prepare_round_trips_simulated_chip() {
        let k = key();
        let challenge = [7u8; 32];
        let der = sim_sign_challenge(&k, &challenge);
        let (sig, id) = prepare(k.verifying_key(), &challenge, &der).unwrap();
        let sig = Signature::from_slice(&sig).unwrap();
        let rid = RecoveryId::from_byte(id).unwrap();
        let recovered =
            VerifyingKey::recover_from_prehash(&challenge_digest(&challenge), &sig, rid).unwrap();
        assert_eq!(&recovered, k.verifying_key());
    }

    #[test]
    fn prepare_normalises_high_s() {
        // HaLo doesn't promise low-S; Soroban rejects high-S. Flip s and check
        // prepare() still yields a low-S signature that recovers the key.
        let k = key();
        let challenge = [9u8; 32];
        let (sig, _) = k
            .sign_prehash_recoverable(&challenge_digest(&challenge))
            .unwrap();
        let (r, s) = sig.split_scalars();
        let high = Signature::from_scalars(r.to_bytes(), (*s).neg().to_bytes()).unwrap();
        assert!(bool::from(high.s().is_high()));

        let (out, _) = prepare(k.verifying_key(), &challenge, high.to_der().as_bytes()).unwrap();
        assert!(!bool::from(
            Signature::from_slice(&out).unwrap().s().is_high()
        ));
    }

    #[test]
    fn prepare_rejects_wrong_key() {
        let challenge = [1u8; 32];
        let der = sim_sign_challenge(&key(), &challenge);
        let other = SigningKey::from_slice(&[0x43; 32]).unwrap();
        assert!(prepare(other.verifying_key(), &challenge, &der).is_err());
    }
}
