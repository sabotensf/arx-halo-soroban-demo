use soroban_sdk::{contractevent, Address, BytesN};

/// A chip was paired with a token (as ERC-5791's `ChipSet`).
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChipSet {
    #[topic]
    pub token_id: u32,
    #[topic]
    pub chip_id: BytesN<32>,
}

/// The issuer added a physical card to the sellable inventory.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CardRegistered {
    #[topic]
    pub chip_id: BytesN<32>,
    pub price: i128,
}

/// A card was bought from the issuer and its token minted to the buyer.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CardPurchased {
    #[topic]
    pub buyer: Address,
    #[topic]
    pub token_id: u32,
    pub chip_id: BytesN<32>,
    pub price: i128,
}

/// The owner changed the chip's nonce, canceling every unused signature.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NonceRotated {
    #[topic]
    pub chip_id: BytesN<32>,
}
