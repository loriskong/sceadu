//! Optional signed invite tickets.
//!
//! Biography exchange is handled by the node/session layer; there is no DHT
//! discovery or neighbour-search API.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invite error: {0}")]
    Invite(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub mod invite;

pub use invite::InviteTicket;
