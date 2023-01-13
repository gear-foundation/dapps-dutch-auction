#![no_std]

use auction_io::{auction::AuctionInfo, io::AuctionMetadata};
use gmeta::{metawasm, Metadata};
use gstd::prelude::*;

#[metawasm]
pub trait Metawasm {
    type State = <AuctionMetadata as Metadata>::State;

    fn info(mut state: Self::State) -> AuctionInfo {
        state.stop_if_time_is_over();
        state.info()
    }
}
