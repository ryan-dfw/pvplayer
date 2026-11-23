//! A snapshot of MPD's player state: what's playing, what's next, and where.

use anyhow::{Context, Result};
use std::time::Duration;

/// MPD's playback state (`state:` in its status reply).
#[derive(Debug, PartialEq, Eq)]
pub enum PlaybackState {
    /// `play`
    Playing,
    /// `pause`
    Paused,
    /// `stop`: nothing is playing, and positions are not reported.
    Stopped,
}

/// One reply to MPD's `status` command. Fields are `None` when MPD omits
/// them (e.g. when stopped, or at the end of the queue).
#[derive(Debug, Default)]
pub struct Status {
    /// Sample rate of the playing audio, in Hz (from `audio:`). Not
    /// currently used.
    pub sample_rate_hz: Option<u32>,
    /// `songid` of the next song in the queue (`nextsongid:`).
    pub next_song_id: Option<u32>,
    /// `songid` of the current song (`songid:`).
    pub song_id: Option<u32>,
    /// Playback position in the current song (`elapsed:`).
    pub elapsed: Option<Duration>,
    /// Length of the current song (`duration:`).
    pub duration: Option<Duration>,
    /// Whether MPD is playing, paused, or stopped (`state:`).
    pub state: Option<PlaybackState>,
}

impl Status {
    /// Parses `status` reply lines. Fields pvplayer doesn't use are ignored.
    ///
    /// # Errors
    ///
    /// Returns an error if a used field has an unreadable value: a
    /// non-numeric ID or time, a negative time, an unknown state, or an
    /// `audio:` rate that isn't a number (including DSD formats such as
    /// `dsd64:2`).
    pub(super) fn parse(lines: &[String]) -> anyhow::Result<Self> {
        let mut status = Self::default();

        for line in lines {
            if let Some(value) = line.strip_prefix("songid: ") {
                status.song_id = Some(
                    value
                        .trim()
                        .parse::<u32>()
                        .context("MPD returned an invalid current song ID")?,
                );
            }

            if let Some(value) = line.strip_prefix("elapsed: ") {
                status.elapsed = Some(parse_duration(value)?);
            }

            if let Some(value) = line.strip_prefix("duration: ") {
                status.duration = Some(parse_duration(value)?);
            }

            if let Some(value) = line.strip_prefix("state: ") {
                status.state = Some(match value.trim() {
                    "play" => PlaybackState::Playing,
                    "pause" => PlaybackState::Paused,
                    "stop" => PlaybackState::Stopped,
                    other => anyhow::bail!("MPD returned an unknown playback state: {}", other),
                });
            }

            if let Some(value) = line.strip_prefix("nextsongid: ") {
                status.next_song_id = Some(
                    value
                        .trim()
                        .parse::<u32>()
                        .context("MPD returned an invalid next song id")?,
                );
            }

            if let Some(value) = line.strip_prefix("audio: ") {
                let (sample_rate, _) = value
                    .trim()
                    .split_once(':')
                    .context("MPD audio field is missing its separator")?;

                status.sample_rate_hz = Some(
                    sample_rate
                        .parse::<u32>()
                        .context("MPD returned an invalid sample rate")?,
                );
            }
        }

        Ok(status)
    }
}

/// Parses MPD's seconds-with-decimals time format (e.g. `12.345`).
///
/// # Errors
///
/// Returns an error if `value` isn't a number, or is negative or too large.
fn parse_duration(value: &str) -> Result<Duration> {
    let seconds = value
        .trim()
        .parse::<f64>()
        .context("MPD returned an invalid time value")?;

    Duration::try_from_secs_f64(seconds)
        .context("MPD returned a negative or out-of-range time value")
}
