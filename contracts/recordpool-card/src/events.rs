use soroban_sdk::{contractevent, Address, BytesN};

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

/// An owner listed their card for resale.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CardListed {
    #[topic]
    pub seller: Address,
    #[topic]
    pub token_id: u32,
    pub price: i128,
}

/// A listing was cancelled by its seller, or invalidated by a transfer.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListingCancelled {
    #[topic]
    pub token_id: u32,
}

/// A secondary sale settled, including the royalty split.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CardResold {
    #[topic]
    pub seller: Address,
    #[topic]
    pub buyer: Address,
    #[topic]
    pub token_id: u32,
    pub price: i128,
    pub royalty_receiver: Address,
    pub royalty: i128,
}
