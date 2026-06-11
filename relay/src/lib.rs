//! puppy-relay: the tiny room server behind "Puppy Pack" (Phase B, Tier 1).
//!
//! Members of a pack connect here, join a room (the room code IS the shared
//! secret -- knowing it is membership), and the relay re-broadcasts presence,
//! chat, and activity lines to everyone in the room. The relay is stateless
//! beyond who's currently connected: no accounts, no history, no persistence.
//!
//! Transport: **line-delimited JSON over TCP** -- the same wire pattern as the
//! puppy-home<->sidecar protocol, so both ends reuse familiar plumbing and the
//! crate needs no networking dependencies. (The plan originally said
//! websockets; that only buys HTTP-proxy traversal for a hosted relay, and the
//! protocol is transport-agnostic -- the same JSON lines can ride ws text
//! frames later without changing either side's logic.)

pub mod hub;
pub mod protocol;
pub mod server;
