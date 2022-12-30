#![no_std]

use core::cmp::min;
use gstd::{exec, msg, prelude::*, ActorId};
use nft_io::{NFTAction, NFTEvent};
use primitive_types::U256;

pub mod state;
pub use state::*;

pub use auction_io::*;

#[derive(Debug, Default)]
pub struct NFT {
    pub token_id: U256,
    pub owner: ActorId,
    pub contract_id: ActorId,
}

#[derive(Debug, Copy, Clone)]
pub enum TransactionStage {
    StartAuctionStage(StartAuctionStage),
    BuyStage(BuyStage),
}

#[derive(Debug, Clone, Copy)]
pub enum StartAuctionStage {
    JustReceivedCommand,
    ValidateNftApproveComplete,
    GetNftOwnerComplete,
    ChangeNftOwnerToContractComplete,
}
#[derive(Debug, Clone, Copy)]
pub enum BuyStage {
    JustReceivedCommand,
    RewardToPreviousOwnerComplete { price: u128 },
}

// StartAuction

// Auction                                                 NFT Owner

// 1. Сохраняем что получили команду на запуск аукциона.

// 2. Отправляем сообщение в контракт NFT на смену владельца на Аукцион
// 3. Сохраняем, что отправили команду NFT на смену владельца 
//                                                         4. Отправляет ответ ОК, меняет владельца NFT
// 5. Если приняли ответ сохраняем, что выполнили запуск аукциона


// Buy

// Auction                                                 NFT Owner

// 1. Сохраняем что получили команду на покупку NFT
// 2. Отправляем предыдущему владельцу NFT вознаграждение.
// 3. Сохраняем, что отправили вознаграждение предыдущему владельцу NFT
// 4. Отправляем в контракт NFT сообщение, чтобы поменяли в контракте NFT владельца с Аукциона на покупателя


#[derive(Debug, Default)]
pub struct Auction {
    pub owner: ActorId,
    pub nft: NFT,
    pub starting_price: u128,
    pub discount_rate: u128,
    pub status: Status,
    pub started_at: u64,
    pub expires_at: u64,
    pub transaction_stage: Option<TransactionStage>,
    pub config: CreateConfig,
}

static mut AUCTION: Option<Auction> = None;

impl Auction {
    async fn buy(&mut self) {
        if !matches!(self.status, Status::IsRunning) {
            panic!("already bought or auction expired");
        }

        if exec::block_timestamp() >= self.expires_at {
            panic!("auction expired");
        }

        let transaction_stage = match &self.transaction_stage {
            Some(transaction_stage) => {
                assert!(matches!(transaction_stage, TransactionStage::BuyStage(_)));
                *transaction_stage
            }
            None => {
                assert_eq!(self.owner, self.nft.owner);
                let transaction_stage = TransactionStage::BuyStage(BuyStage::JustReceivedCommand);
                self.transaction_stage = Some(transaction_stage);
                transaction_stage
            }
        };

        self.process_transaction_stage(&transaction_stage, self.config)
            .await;
    }

    async fn renew_contract(&mut self, config: CreateConfig) {
        if matches!(self.status, Status::IsRunning) {
            panic!("already in use")
        }

        let transaction_stage = match self.transaction_stage {
            Some(transaction_stage) => {
                assert!(matches!(
                    transaction_stage,
                    TransactionStage::StartAuctionStage(_)
                ));
                transaction_stage
            }
            None => {
                let transaction_stage =
                    TransactionStage::StartAuctionStage(StartAuctionStage::JustReceivedCommand);
                self.transaction_stage = Some(transaction_stage);
                transaction_stage
            }
        };

        self.process_transaction_stage(&transaction_stage, config)
            .await;
    }

    async fn get_token_owner(contract_id: ActorId, token_id: U256) -> ActorId {
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

    fn stop_if_time_is_over(&mut self) {
        if matches!(self.status, Status::IsRunning) && exec::block_timestamp() >= self.expires_at {
            self.status = Status::Expired;
        }
    }

    fn force_stop(&mut self) {
        if msg::source() != self.owner {
            panic!("Can't stop if sender is not owner")
        }

        self.status = Status::Stopped;

        msg::reply(
            Event::AuctionStopped {
                token_owner: self.owner,
                token_id: self.nft.token_id,
            },
            0,
        )
        .unwrap();
    }

    fn info(&self) -> AuctionInfo {
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

    async fn process_transaction_stage(
        &mut self,
        transaction_stage: &TransactionStage,
        config: CreateConfig,
    ) {
        match transaction_stage {
            TransactionStage::StartAuctionStage(start_stage) => {
                self.process_start_stage(start_stage, &config).await
            }
            TransactionStage::BuyStage(buy_stage) => self.process_buy_stage(buy_stage).await,
        }
    }

    async fn process_start_stage(
        &mut self,
        start_stage: &StartAuctionStage,
        config: &CreateConfig,
    ) {
        match start_stage {
            StartAuctionStage::JustReceivedCommand => {
                let minutes_count = config.duration.hours * 60 + config.duration.minutes;
                let duration_in_seconds = minutes_count * 60 + config.duration.seconds;

                if config.starting_price < config.discount_rate * (duration_in_seconds as u128) {
                    panic!("starting price < min");
                }
                self.validate_nft_approve(config).await;
                self.get_owner(config).await;
                self.transfer_nft_to_auction(config).await;
                self.send_reply_that_auction_started();
            }
            StartAuctionStage::ValidateNftApproveComplete => {
                self.get_owner(config).await;
                self.transfer_nft_to_auction(config).await;
                self.send_reply_that_auction_started();
            }
            StartAuctionStage::GetNftOwnerComplete => {
                self.transfer_nft_to_auction(config).await;
                self.send_reply_that_auction_started();
            }
            StartAuctionStage::ChangeNftOwnerToContractComplete => {
                self.send_reply_that_auction_started();
            }
        }
    }

    async fn validate_nft_approve(&mut self, config: &CreateConfig) {
        let reply: NFTEvent = msg::send_for_reply_as(
            config.nft_contract_actor_id,
            NFTAction::IsApproved {
                token_id: config.token_id,
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

        self.transaction_stage = Some(TransactionStage::StartAuctionStage(
            StartAuctionStage::ValidateNftApproveComplete,
        ));
    }

    async fn get_owner(&mut self, config: &CreateConfig) {
        self.started_at = exec::block_timestamp();

        let minutes_count = config.duration.hours * 60 + config.duration.minutes;
        let duration_in_seconds = minutes_count * 60 + config.duration.seconds;

        self.expires_at = self.started_at + duration_in_seconds * 1000;
        self.nft.token_id = config.token_id;
        self.nft.contract_id = config.nft_contract_actor_id;
        self.nft.owner = Self::get_token_owner(config.nft_contract_actor_id, config.token_id).await;
        self.transaction_stage = Some(TransactionStage::StartAuctionStage(
            StartAuctionStage::GetNftOwnerComplete,
        ));
    }

    async fn transfer_nft_to_auction(&mut self, config: &CreateConfig) {
        self.discount_rate = config.discount_rate;
        self.starting_price = config.starting_price;

        msg::send_for_reply(
            self.nft.contract_id,
            NFTAction::Transfer {
                to: self.owner,
                token_id: self.nft.token_id,
            },
            0,
        )
        .unwrap()
        .await
        .expect("Error in nft transfer");
        // Start Auction Stage completed
        self.transaction_stage = Some(TransactionStage::StartAuctionStage(
            StartAuctionStage::ChangeNftOwnerToContractComplete,
        ));
    }

    fn send_reply_that_auction_started(&mut self) {
        msg::reply(
            Event::AuctionStarted {
                token_owner: self.nft.owner,
                price: self.starting_price,
                token_id: self.nft.token_id,
            },
            0,
        )
        .unwrap();
        self.transaction_stage = None
    }

    async fn process_buy_stage(&mut self, buy_stage: &BuyStage) {
        match buy_stage {
            BuyStage::JustReceivedCommand => self.send_reward_to_nft_owner(),
            BuyStage::RewardToPreviousOwnerComplete { price } => {
                let refund = msg::value() - price;
                let refund = if refund < 500 { 0 } else { refund };
                msg::send_for_reply(
                    self.nft.contract_id,
                    NFTAction::Transfer {
                        to: msg::source(),
                        token_id: self.nft.token_id,
                    },
                    0,
                )
                .unwrap()
                .await
                .expect("Error in nft transfer");
                msg::reply(Event::Bought { price: *price }, refund)
                    .expect("Can't send refund and reply");
            }
        }
    }

    fn send_reward_to_nft_owner(&mut self) {
        let price = self.token_price();

        if msg::value() < price {
            panic!("value < price, {:?} < {:?}", msg::value(), price);
        }

        self.status = Status::Purchased { price };
        msg::send(self.nft.owner, "REWARD", price).expect("Couldn't send payment for nft owner");

        self.transaction_stage = Some(TransactionStage::BuyStage(
            BuyStage::RewardToPreviousOwnerComplete { price },
        ));
    }

    fn token_price(&self) -> u128 {
        // time_elapsed is in seconds
        let time_elapsed = exec::block_timestamp().saturating_sub(self.started_at) / 1000;
        let discount = min(
            self.discount_rate * (time_elapsed as u128),
            self.starting_price,
        );

        self.starting_price - discount
    }
}

gstd::metadata! {
    title: "Auction",
    handle:
        input: Action,
        output: Event,
    state:
        input: State,
        output: StateReply,
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

    match action {
        Action::Buy => auction.buy().await,
        Action::Create(config) => auction.renew_contract(config).await,
        Action::ForceStop => auction.force_stop(),
    }
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
