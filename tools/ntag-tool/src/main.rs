//! `ntag-tool` — the off-chain verifier for NXP NTAG 424 DNA cards.
//!
//! An NTAG 424 DNA chip with Secure Dynamic Messaging (SDM) turned on puts a
//! fresh "SUN" message into the URL it emits on every tap:
//!
//! ```text
//! https://…/tap?picc_data=<32 hex>&cmac=<16 hex>
//!
//! picc_data = AES-128-CBC(K_SDMMetaRead, IV = 0,
//!                         0xC7 || UID(7) || SDMReadCtr(3, LSB first) || random(5))
//! K_ses     = AES-CMAC(K_SDMFileRead, 3C C3 00 01 00 80 || UID || SDMReadCtr)
//! cmac      = odd-indexed bytes of AES-CMAC(K_ses, "")
//! ```
//!
//! (NXP application note AN12196, "NTAG 424 DNA and NTAG 424 DNA TagTamper
//! features and hints".) Checking it needs the chip's secret AES keys, which
//! is why the Soroban contract can't do it: the keys would be public on-chain.
//! This tool plays the verifier holding those keys. After a successful
//! `verify`, the verifier account calls the contract's `attest_tap` with the
//! `chip_id` and counter it prints.
//!
//! Keys come from the `NTAG_META_KEY` and `NTAG_FILE_KEY` environment
//! variables (32 hex chars each). They default to all zeros, the factory
//! default on a new chip. A real deployment sets its own keys on every chip
//! and keeps them in a secrets manager or HSM.
//!
//! ```text
//! ntag-tool chip-id <uid-hex>                  # chip_id = SHA-256(UID), for register_ntag_card
//! ntag-tool sim-tap <uid-hex> <counter>        # simulated tap; prints "<picc_data> <cmac>"
//! ntag-tool verify <picc_data> <cmac>          # prints "<chip_id> <counter>", exits 1 if forged
//! ```

use std::{env, process::exit};

use aes::{
    cipher::{BlockDecrypt, BlockEncrypt, KeyInit},
    Aes128,
};
use cmac::{Cmac, Mac};
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};

/// PICCDataTag: UID mirrored (bit 7), read counter mirrored (bit 6), 7-byte UID.
const PICC_DATA_TAG: u8 = 0xC7;
/// Session-vector prefix for the SDM file-read MAC key (AN12196).
const SV2_PREFIX: [u8; 6] = [0x3C, 0xC3, 0x00, 0x01, 0x00, 0x80];
/// SDMReadCtr is a 24-bit counter.
const MAX_COUNTER: u32 = 0xFF_FFFF;

pub type Uid = [u8; 7];

/// The two SDM keys a verifier holds for a chip.
pub struct SdmKeys {
    pub meta_read: [u8; 16],
    pub file_read: [u8; 16],
}

impl SdmKeys {
    /// Factory-default keys (all zeros).
    pub fn factory_default() -> Self {
        Self {
            meta_read: [0; 16],
            file_read: [0; 16],
        }
    }
}

/// What a verified tap tells you.
#[derive(Debug, PartialEq, Eq)]
pub struct Tap {
    pub uid: Uid,
    pub counter: u32,
}

/// `chip_id` the contract uses for an NTAG 424 card: SHA-256 of its UID.
pub fn chip_id(uid: &Uid) -> [u8; 32] {
    Sha256::digest(uid).into()
}

fn cmac(key: &[u8; 16], data: &[u8]) -> [u8; 16] {
    let mut mac = <Cmac<Aes128> as Mac>::new_from_slice(key).expect("16-byte key");
    mac.update(data);
    mac.finalize().into_bytes().into()
}

/// The 8-byte SDM MAC the chip appends for this UID and counter (with an
/// empty MAC input, i.e. the URL carries only `picc_data` and `cmac`).
fn sdm_mac(file_read_key: &[u8; 16], uid: &Uid, counter: u32) -> [u8; 8] {
    let mut sv2 = [0u8; 16];
    sv2[..6].copy_from_slice(&SV2_PREFIX);
    sv2[6..13].copy_from_slice(uid);
    sv2[13..].copy_from_slice(&counter.to_le_bytes()[..3]);
    let session_key = cmac(file_read_key, &sv2);
    let full = cmac(&session_key, &[]);
    let mut out = [0u8; 8];
    for (i, b) in out.iter_mut().enumerate() {
        *b = full[2 * i + 1];
    }
    out
}

/// Checks a SUN message and returns the UID and tap counter inside it.
pub fn verify(keys: &SdmKeys, picc_data: &[u8; 16], mac: &[u8; 8]) -> Result<Tap, String> {
    // A single block with a zero IV: CBC decryption is plain AES decryption.
    let mut block: aes::Block = (*picc_data).into();
    Aes128::new(&keys.meta_read.into()).decrypt_block(&mut block);

    if block[0] != PICC_DATA_TAG {
        return Err("picc_data didn't decrypt to a UID + counter record (wrong meta key?)".into());
    }
    let mut uid = [0u8; 7];
    uid.copy_from_slice(&block[1..8]);
    let counter = u32::from_le_bytes([block[8], block[9], block[10], 0]);

    // Constant-time comparison isn't needed for a demo CLI, but a server
    // checking MACs should use one.
    if &sdm_mac(&keys.file_read, &uid, counter) != mac {
        return Err("cmac doesn't match: forged or altered tap".into());
    }
    Ok(Tap { uid, counter })
}

/// What a chip with these keys would emit for a tap with this counter.
pub fn sim_tap(keys: &SdmKeys, uid: &Uid, counter: u32, padding: [u8; 5]) -> ([u8; 16], [u8; 8]) {
    assert!(counter <= MAX_COUNTER, "SDMReadCtr is 24 bits");
    let mut plain = [0u8; 16];
    plain[0] = PICC_DATA_TAG;
    plain[1..8].copy_from_slice(uid);
    plain[8..11].copy_from_slice(&counter.to_le_bytes()[..3]);
    plain[11..].copy_from_slice(&padding);

    let mut block: aes::Block = plain.into();
    Aes128::new(&keys.meta_read.into()).encrypt_block(&mut block);
    (block.into(), sdm_mac(&keys.file_read, uid, counter))
}

// ---------------------------------------------------------------------- CLI

fn usage() -> ! {
    eprintln!(
        "usage:\n  ntag-tool chip-id <uid-hex>\n  ntag-tool sim-tap <uid-hex> <counter>\n  ntag-tool verify <picc_data-hex> <cmac-hex>\n\nkeys: NTAG_META_KEY / NTAG_FILE_KEY (32 hex chars, default all zeros)"
    );
    exit(2)
}

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("error: {msg}");
    exit(1)
}

fn hex_array<const N: usize>(what: &str, s: &str) -> [u8; N] {
    let bytes = hex::decode(s.trim()).unwrap_or_else(|e| fail(format!("{what}: {e}")));
    bytes
        .try_into()
        .unwrap_or_else(|b: Vec<u8>| fail(format!("{what}: expected {N} bytes, got {}", b.len())))
}

fn env_key(var: &str) -> [u8; 16] {
    match env::var(var) {
        Ok(v) => hex_array(var, &v),
        Err(_) => [0; 16],
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let keys = SdmKeys {
        meta_read: env_key("NTAG_META_KEY"),
        file_read: env_key("NTAG_FILE_KEY"),
    };
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["chip-id", uid] => println!("{}", hex::encode(chip_id(&hex_array("uid", uid)))),
        ["sim-tap", uid, counter] => {
            let counter: u32 = counter
                .parse()
                .ok()
                .filter(|c| *c <= MAX_COUNTER)
                .unwrap_or_else(|| fail("counter must be 0..=16777215"));
            let mut padding = [0u8; 5];
            OsRng.fill_bytes(&mut padding);
            let (picc, mac) = sim_tap(&keys, &hex_array("uid", uid), counter, padding);
            println!("{} {}", hex::encode_upper(picc), hex::encode_upper(mac));
        }
        ["verify", picc, mac] => {
            let tap = verify(
                &keys,
                &hex_array("picc_data", picc),
                &hex_array("cmac", mac),
            )
            .unwrap_or_else(|e| fail(e));
            eprintln!(
                "uid {}  counter {}",
                hex::encode_upper(tap.uid),
                tap.counter
            );
            println!("{} {}", hex::encode(chip_id(&tap.uid)), tap.counter);
        }
        _ => usage(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UID: Uid = [0x04, 0xDE, 0x5F, 0x1E, 0xAC, 0xC0, 0x40];

    #[test]
    fn nxp_reference_vector() {
        // SUN message from NXP AN12196 (factory-default keys).
        let tap = verify(
            &SdmKeys::factory_default(),
            &hex_array("picc", "EF963FF7828658A599F3041510671E88"),
            &hex_array("cmac", "94EED9EE65337086"),
        )
        .unwrap();
        assert_eq!(
            tap,
            Tap {
                uid: UID,
                counter: 61
            }
        );
    }

    #[test]
    fn round_trip_with_custom_keys() {
        let keys = SdmKeys {
            meta_read: [0x11; 16],
            file_read: [0x22; 16],
        };
        let (picc, mac) = sim_tap(&keys, &UID, 1234, [9; 5]);
        assert_eq!(
            verify(&keys, &picc, &mac),
            Ok(Tap {
                uid: UID,
                counter: 1234
            })
        );
    }

    #[test]
    fn rejects_tampered_or_foreign_taps() {
        let keys = SdmKeys::factory_default();
        let (picc, mac) = sim_tap(&keys, &UID, 7, [0; 5]);

        let mut bad_mac = mac;
        bad_mac[0] ^= 1;
        assert!(verify(&keys, &picc, &bad_mac).is_err());

        let mut bad_picc = picc;
        bad_picc[3] ^= 1;
        assert!(verify(&keys, &bad_picc, &mac).is_err());

        // Same chip data, but MAC'd with a key the verifier doesn't trust.
        let other = SdmKeys {
            meta_read: [0; 16],
            file_read: [0x33; 16],
        };
        let (picc, mac) = sim_tap(&other, &UID, 7, [0; 5]);
        assert!(verify(&keys, &picc, &mac).is_err());
    }
}
