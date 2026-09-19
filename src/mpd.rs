pub mod client;
mod song;
mod status;

pub use client::MpdClient;
pub use song::Song;
pub use status::{PlaybackState, Status};

#[cfg(test)]
mod tests;