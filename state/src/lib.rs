use io::AuctionMetadata;
use gmeta::{metawasm, Metadata};
use gstd::{prelude::*, ActorId};

#[metawasm]
pub trait Metawasm {
    type State = <AuctionMetadata as Metadata>::State;
}
