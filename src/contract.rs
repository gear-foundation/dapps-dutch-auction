use crate::state::{State, StateReply};
use auction_io::auction::{
    Action, AuctionInfo, CreateConfig, Error, Event, Status, Transaction, TransactionId,
};
use auction_io::io::AuctionMetadata;
use core::cmp::min;
use gmeta::Metadata;
use gstd::ActorId;
use gstd::{errors::Result as GstdResult, exec, msg, prelude::*, MessageId};
use nft_io::{NFTAction, NFTEvent};
use primitive_types::U256;

static mut AUCTION: Option<Auction> = None;

#[derive(Debug, Clone, Default)]
pub struct Nft {
    pub token_id: U256,
    pub owner: ActorId,
    pub contract_id: ActorId,
}

#[derive(Debug, Clone, Default)]
pub struct Auction {
    pub owner: ActorId,
    pub nft: Nft,
    pub starting_price: u128,
    pub discount_rate: u128,
    pub status: Status,
    pub started_at: u64,
    pub expires_at: u64,

    pub transactions: BTreeMap<ActorId, Transaction<Action>>,
    pub current_tid: TransactionId,
}

impl Auction {
    pub async fn buy(&mut self, transaction_id: TransactionId) -> Result<(Event, u128), Error> {
        if !matches!(self.status, Status::IsRunning) {
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

        if let Err(e) = msg::send_for_reply(
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
        {
            gstd::debug!("{}", e);
            return Err(Error::NftTransferFailed);
        }

        if let Err(e) = msg::send(self.nft.owner, "REWARD", price) {
            gstd::debug!("{}", e);
            return Err(Error::RewardSendFailed);
        }

        Ok((Event::Bought { price }, refund))
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

    pub async fn renew_contract(&mut self, config: CreateConfig) -> Result<Event, Error> {
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

        Ok(Event::AuctionStarted {
            token_owner: self.nft.owner,
            price: self.starting_price,
            token_id: self.nft.token_id,
        })
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
        if matches!(self.status, Status::IsRunning) && exec::block_timestamp() >= self.expires_at {
            self.status = Status::Expired;
        }
    }

    pub fn is_active(&self) -> bool {
        match self.status {
            Status::None | Status::Purchased { .. } | Status::Expired | Status::Stopped => false,
            Status::IsRunning => true,
        }
    }

    pub fn force_stop(&mut self) -> Result<Event, Error> {
        if msg::source() != self.owner {
            return Err(Error::NotOwner);
        }

        self.status = Status::Stopped;

        Ok(Event::AuctionStoped {
            token_owner: self.owner,
            token_id: self.nft.token_id,
        })
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
            expires_at: self.expires_at,
            status: self.status.clone(),
            transactions: self.transactions.clone(),
            current_tid: self.current_tid,
        }
    }
}

#[no_mangle]
extern "C" fn init() {
    let auction = Auction {
        owner: msg::source(),
        ..Default::default()
    };

    unsafe { AUCTION = Some(auction) };
}

#[gstd::async_main]
async fn main() {
    let action: Action = msg::load().expect("Could not load Action");
    let auction: &mut Auction = unsafe { AUCTION.get_or_insert(Auction::default()) };

    auction.stop_if_time_is_over();

    let msg_source = msg::source();

    let r: Result<Action, Error> = Err(Error::PreviousTxMustBeCompleted);
    let transaction_id = if let Some(Transaction {
        id: tid,
        action: pend_action,
    }) = auction.transactions.get(&msg_source)
    {
        if action != *pend_action {
            reply(r, 0).expect(
                "Failed to encode or reply with `Result<NFTPixelboardEvent, NFTPixelboardError>`",
            );
            return;
        }
        *tid
    } else {
        let transaction_id = auction.current_tid;
        auction.transactions.insert(
            msg_source,
            Transaction {
                id: transaction_id,
                action: action.clone(),
            },
        );
        auction.current_tid = auction.current_tid.wrapping_add(1);
        transaction_id
    };

    let (result, value) = match action {
        Action::Buy => {
            let reply = auction.buy(transaction_id).await;
            let result = match reply {
                Ok((event, refund)) => (Ok(event), refund),
                Err(e) => (Err(e), 0),
            };
            auction.transactions.remove(&msg_source);
            result
        }
        Action::Create(config) => {
            let result = (auction.renew_contract(config).await, 0);
            auction.transactions.remove(&msg_source);
            result
        }
        Action::ForceStop => {
            let result = (auction.force_stop(), 0);
            auction.transactions.remove(&msg_source);
            result
        }
    };
    gstd::debug!("refund = {value}, Result = {:?}", result);

    reply(result, value).expect("Failed to encode or reply with `Result<Event, Error>`");
}

fn common_state() -> <AuctionMetadata as Metadata>::State {
    static_mut_state().info()
}

fn static_mut_state() -> &'static mut Auction {
    unsafe { AUCTION.get_or_insert(Default::default()) }
}

#[no_mangle]
extern "C" fn state() {
    reply(common_state(), 0).expect(
        "Failed to encode or reply with `<AuctionMetadata as Metadata>::State` from `state()`",
    );
}

#[no_mangle]
extern "C" fn metahash() {
    let metahash: [u8; 32] = include!("../.metahash");
    reply(metahash, 0).expect("Failed to encode or reply with `[u8; 32]` from `metahash()`");
}

fn reply(payload: impl Encode, value: u128) -> GstdResult<MessageId> {
    msg::reply(payload, value)
}

#[no_mangle]
extern "C" fn meta_state() -> *mut [i32; 2] {
    let query: State = msg::load().expect("failed to decode input argument");
    let auction: &mut Auction = unsafe { AUCTION.get_or_insert(Auction::default()) };

    auction.stop_if_time_is_over();

    let encoded = match query {
        State::Info => StateReply::Info(auction.info()),
    }
    .encode();

    gstd::util::to_leak_ptr(encoded)
}
