use std::time::Duration;
use anyhow::{Context, Result};

#[derive(Debug, PartialEq, Eq)]
pub enum PlaybackState {
    Playing,
    Paused,
    Stopped
}

#[derive(Debug, Default)]
pub struct Status {
    pub sample_rate_hz: Option<u32>,
    pub next_song_id: Option<u32>,
    pub song_id: Option<u32>,
    pub elapsed: Option<Duration>,
    pub duration: Option<Duration>,
    pub state: Option<PlaybackState>,
}

impl Status {
    pub(super) fn parse(lines: &[String]) -> anyhow::Result<Self> {
        let mut status = Self::default();

        for line in lines{

            if let Some(value) = line.strip_prefix("songid: ") {
                status.song_id = Some(
                    value.trim().parse::<u32>()
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
                    other => anyhow::bail!(
                        "MPD returned an unknown playback state: {}", other
                    )
                });
            }

            if let Some(value) = line.strip_prefix("nextsongid: ") {
                status.next_song_id = Some(
                    value.trim().parse::<u32>()
                        .context("MPD returned an invalid next song id")?,
                );
            }

            if let Some(value) = line.strip_prefix("audio: ") {
                let (sample_rate, _) = value
                    .trim()
                    .split_once(':')
                    .context("MPD audio field is missing its separator")?;

                status.sample_rate_hz = Some(
                    sample_rate.parse::<u32>()
                        .context("MPD returned an invalid sample rate")?,
                );
            }
        }

        Ok(status)
    }
}

fn parse_duration(value: &str) -> Result<Duration> {
    let seconds = value
        .trim()
        .parse::<f64>()
        .context("MPD returned an invalid time value")?;

    Duration::try_from_secs_f64(seconds)
        .context("MPD returned a negative or out-of-range time value")
}