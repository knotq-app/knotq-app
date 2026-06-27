//! Persistent WebSocket sync transport (online, poll-free).
//!
//! `frames` is the JSON wire protocol (mirrors the backend `socket.ts`); `client`
//! is the platform-agnostic connection core (reconnect, request multiplexing,
//! keepalive, server-push callbacks) generic over a [`client::RawSocket`]. The
//! `SyncTransport` adapter that maps [`client::WsRequestError`] onto a platform's
//! error contract lives in the platform crate.
pub mod client;
pub mod frames;

pub use client::{
    PresenceEvent, RawSocket, RawSocketFactory, WsCallbacks, WsClient, WsConfig, WsRequestError,
};
pub use frames::{ServerFrame, KEEPALIVE_PING, KEEPALIVE_PONG};
