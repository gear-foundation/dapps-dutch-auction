use crate::state::{State, StateReply};
use auction_io::auction::Auction;
use auction_io::io::*;
use gmeta::Metadata;
use gstd::{errors::Result as GstdResult, msg, prelude::*, MessageId};

static mut AUCTION: Option<Auction> = None;

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

fn common_state() -> <AuctionMetadata as Metadata>::State {
    static_mut_state().clone()
}

fn static_mut_state() -> &'static mut Auction {
    unsafe { AUCTION.get_or_insert(Default::default()) }
}

#[no_mangle]
extern "C" fn state() {
    reply(common_state()).expect(
        "Failed to encode or reply with `<AuctionMetadata as Metadata>::State` from `state()`",
    );
}

#[no_mangle]
extern "C" fn metahash() {
    reply(include!("../.metahash"))
        .expect("Failed to encode or reply with `[u8; 32]` from `metahash()`");
}

fn reply(payload: impl Encode) -> GstdResult<MessageId> {
    msg::reply(payload, 0)
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
