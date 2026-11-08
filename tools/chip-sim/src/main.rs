//! `chip-sim` — a stand-in for the secp256r1 secure element embedded in a
//! RecordPool chipped card (e.g. an NXP NTAG 424 / JavaCard-class chip).
//!
//! ```text
//! chip-sim keygen <card.key>                 # "manufacture" a card
//! chip-sim pubkey <card.key>                 # 65-byte SEC1 public key (hex)
//! chip-sim sign   <card.key> <challenge-hex> # tap: sign a verifier challenge
//! chip-sim challenge                         # 32 random bytes (hex)
//! ```
//!
//! Signatures are ECDSA P-256 over SHA-256, low-S normalised, 64 bytes r||s —
//! exactly what Soroban's `secp256r1_verify` host function expects.

use std::{env, fs, process::exit};

use p256::ecdsa::{signature::Signer, Signature, SigningKey};
use rand_core::{OsRng, RngCore};

fn usage() -> ! {
    eprintln!(
        "usage:\n  chip-sim keygen <card.key>\n  chip-sim pubkey <card.key>\n  chip-sim sign <card.key> <challenge-hex>\n  chip-sim challenge"
    );
    exit(2)
}

fn load(path: &str) -> SigningKey {
    let raw = fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("cannot read {path}: {e}");
        exit(1)
    });
    let bytes = hex::decode(raw.trim()).expect("key file must be hex");
    SigningKey::from_slice(&bytes).expect("invalid P-256 secret key")
}

fn pubkey_hex(key: &SigningKey) -> String {
    hex::encode(key.verifying_key().to_encoded_point(false).as_bytes())
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["keygen", path] => {
            let key = SigningKey::random(&mut OsRng);
            fs::write(path, hex::encode(key.to_bytes())).expect("write key file");
            println!("{}", pubkey_hex(&key));
        }
        ["pubkey", path] => println!("{}", pubkey_hex(&load(path))),
        ["sign", path, challenge] => {
            let msg = hex::decode(challenge).expect("challenge must be hex");
            let sig: Signature = load(path).sign(&msg);
            let sig = sig.normalize_s().unwrap_or(sig);
            println!("{}", hex::encode(sig.to_bytes()));
        }
        ["challenge"] => {
            let mut buf = [0u8; 32];
            OsRng.fill_bytes(&mut buf);
            println!("{}", hex::encode(buf));
        }
        _ => usage(),
    }
}
