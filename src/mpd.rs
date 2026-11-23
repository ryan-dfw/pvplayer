//! Talks to MPD over its text protocol.
//!
//! [`MpdClient`] sends commands and returns raw reply lines, which
//! [`Status`] and [`Song`] parse. See MPD's
//! [protocol reference](https://mpd.readthedocs.io/en/latest/protocol.html).

pub mod client;
mod song;
mod status;

pub use client::MpdClient;
pub use song::Song;
pub use status::{PlaybackState, Status};

#[cfg(test)]
mod tests;
