use soroban_sdk::contracterror;

/// Contract-specific errors. OpenZeppelin's NFT errors live in the 200 range
/// and its access-control errors in the 2000 range, so these start at 3000 to
/// keep error codes unambiguous when reading a failed transaction.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum CardError {
    /// The chip has never been registered by the issuer.
    CardNotRegistered = 3000,
    /// The chip is already registered (pending sale or already minted).
    CardAlreadyRegistered = 3001,
    /// The card has already been sold and minted.
    CardAlreadyMinted = 3002,
    /// Prices must be strictly positive.
    InvalidPrice = 3003,
    /// The token has no chip paired with it.
    UnknownToken = 3004,
    /// No minted token is paired with this chip (as ERC-5791's `NoMappedTokenForChip`).
    NoMappedTokenForChip = 3005,
    /// The chip signature doesn't recover to the chip's key.
    InvalidSignature = 3006,
    /// The recovery id is not 0 or 1.
    InvalidSignatureFormat = 3007,
    /// The signature timestamp is later than the ledger's close time.
    SignatureTimestampInFuture = 3008,
    /// The signature timestamp is older than `max_duration_window`.
    SignatureTimestampTooOld = 3009,
    /// Tokens only move with a chip signature (`transfer_token`), as in
    /// ERC-5791; wallet-signed `transfer` / `transfer_from` are disabled.
    TransferRequiresChip = 3010,
    /// A card must be registered with an `http://` or `https://` music URL.
    InvalidMusicUrl = 3011,
    /// `max_duration_window` must be greater than zero.
    InvalidWindow = 3012,
    /// Data for `is_chip_signature_for_token` must start with `pbt:verify:`.
    InvalidVerificationData = 3013,
}
