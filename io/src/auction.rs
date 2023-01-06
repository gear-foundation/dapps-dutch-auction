use crate::io::*;
use core::cmp::min;
use gstd::{exec, msg, prelude::*, ActorId};
use nft_io::{NFTAction, NFTEvent};
use primitive_types::U256;

#[derive(Debug, Decode, Default, Encode, TypeInfo, Clone)]
#[codec(crate = gstd::codec)]
#[scale_info(crate = gstd::scale_info)]
pub enum Status {
    #[default]
    None,
    IsRunning,
    Purchased {
        price: u128,
    },
    Expired,
    Stopped,
}

#[derive(Debug, Clone, Default, Encode, Decode, TypeInfo)]
pub struct NFT {
    pub token_id: U256,
    pub owner: ActorId,
    pub contract_id: ActorId,
}

#[derive(Debug, Clone, Default, Encode, Decode, TypeInfo)]
#[codec(crate = gstd::codec)]
#[scale_info(crate = gstd::scale_info)]
pub struct Auction {
    pub owner: ActorId,
    pub nft: NFT,
    pub starting_price: u128,
    pub discount_rate: u128,
    pub status: Status,
    pub started_at: u64,
    pub expires_at: u64,
}

impl Auction {
    pub async fn buy(&mut self) {
        if !self.is_active() {
            panic!("already bought or auction expired");
        }

        if exec::block_timestamp() >= self.expires_at {
            panic!("auction expired");
        }

        let price = self.token_price();

        if msg::value() < price {
            panic!("value < price, {:?} < {:?}", msg::value(), price);
        }

        self.status = Status::Purchased { price };

        let refund = msg::value() - price;
        let refund = if refund < 500 { 0 } else { refund };
        let transaction_id = 0u64;

        msg::send_for_reply(
            self.nft.contract_id,
            NFTAction::Transfer {
                to: msg::source(),
                token_id: self.nft.token_id,
                transaction_id,
            },
            0,
        )
        .unwrap()
        .await
        .expect("Error in nft transfer");

        msg::send(self.nft.owner, "REWARD", price).expect("Couldn't send payment for nft owner");
        msg::reply(Event::Bought { price }, refund).expect("Can't send refund and reply");
    }

    pub fn token_price(&self) -> u128 {
        // time_elapsed is in seconds
        let time_elapsed = exec::block_timestamp().saturating_sub(self.started_at) / 1000;
        let discount = min(
            self.discount_rate * (time_elapsed as u128),
            self.starting_price,
        );

        self.starting_price - discount
    }

    pub async fn renew_contract(&mut self, config: CreateConfig) {
        if self.is_active() {
            panic!("already in use")
        }

        let minutes_count = config.duration.hours * 60 + config.duration.minutes;
        let duration_in_seconds = minutes_count * 60 + config.duration.seconds;

        if config.starting_price < config.discount_rate * (duration_in_seconds as u128) {
            panic!("starting price < min");
        }

        self.validate_nft_approve(config.nft_contract_actor_id, config.token_id)
            .await;

        self.status = Status::IsRunning;
        self.started_at = exec::block_timestamp();
        self.expires_at = self.started_at + duration_in_seconds * 1000;
        self.nft.token_id = config.token_id;
        self.nft.contract_id = config.nft_contract_actor_id;
        self.nft.owner = Self::get_token_owner(config.nft_contract_actor_id, config.token_id).await;

        self.discount_rate = config.discount_rate;
        self.starting_price = config.starting_price;

        msg::reply(
            Event::AuctionStarted {
                token_owner: self.nft.owner,
                price: self.starting_price,
                token_id: self.nft.token_id,
            },
            0,
        )
        .unwrap();
    }

    pub async fn get_token_owner(contract_id: ActorId, token_id: U256) -> ActorId {
        let reply: NFTEvent = msg::send_for_reply_as(contract_id, NFTAction::Owner { token_id }, 0)
            .expect("Can't send message")
            .await
            .expect("Unable to decode `NFTEvent`");

        if let NFTEvent::Owner { owner, .. } = reply {
            owner
        } else {
            panic!("Wrong received message!")
        }
    }

    pub async fn validate_nft_approve(&self, contract_id: ActorId, token_id: U256) {
        let reply: NFTEvent = msg::send_for_reply_as(
            contract_id,
            NFTAction::IsApproved {
                token_id,
                to: exec::program_id(),
            },
            0,
        )
        .expect("Can't send message")
        .await
        .expect("Unable to decode `NFTEvent`");

        if let NFTEvent::IsApproved { approved, .. } = reply {
            if !approved {
                panic!("You must approve your NFT to this contract before")
            }
        } else {
            panic!("Wrong received message!")
        }
    }

    pub fn stop_if_time_is_over(&mut self) {
        if self.is_active() && exec::block_timestamp() >= self.expires_at {
            self.status = Status::Expired;
        }
    }

    pub fn is_active(&self) -> bool {
        match self.status {
            Status::None | Status::Purchased { .. } | Status::Expired | Status::Stopped => false,
            Status::IsRunning => true,
        }
    }

    pub fn force_stop(&mut self) {
        if msg::source() != self.owner {
            panic!("Can't stop if sender is not owner")
        }

        self.status = Status::Stopped;

        msg::reply(
            Event::AuctionStoped {
                token_owner: self.owner,
                token_id: self.nft.token_id,
            },
            0,
        )
        .unwrap();
    }

    pub fn info(&self) -> AuctionInfo {
        AuctionInfo {
            nft_contract_actor_id: self.nft.contract_id,
            token_id: self.nft.token_id,
            token_owner: self.nft.owner,
            auction_owner: self.owner,
            starting_price: self.starting_price,
            current_price: self.token_price(),
            discount_rate: self.discount_rate,
            time_left: self.expires_at.saturating_sub(exec::block_timestamp()),
            status: self.status.clone(),
        }
    }
}

#[derive(Debug, Decode, Encode, TypeInfo)]
#[codec(crate = gstd::codec)]
#[scale_info(crate = gstd::scale_info)]
pub struct AuctionInfo {
    pub nft_contract_actor_id: ActorId,
    pub token_id: U256,
    pub token_owner: ActorId,
    pub auction_owner: ActorId,
    pub starting_price: u128,
    pub current_price: u128,
    pub discount_rate: u128,
    pub time_left: u64,
    pub status: Status,
}
