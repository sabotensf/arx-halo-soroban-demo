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
    /// The caller does not own the token.
    NotTokenOwner = 3004,
    /// There is no active listing for this token.
    NotListed = 3005,
    /// A seller cannot buy their own listing.
    SelfPurchase = 3006,
    /// The royalty exceeds the sale price (should be impossible; defensive).
    RoyaltyExceedsPrice = 3007,
    /// The token has no chip bound to it.
    UnknownToken = 3008,
    /// The chip signature doesn't recover to the key bound to this token.
    InvalidChipSignature = 3009,
}
