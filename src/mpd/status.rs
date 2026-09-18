use anyhow::Context;

#[derive(Debug, Default)]
pub struct Status {
    pub sample_rate_hz: Option<u32>,
    pub next_song_id: Option<u32>
}

impl Status {
    pub(super) fn parse(lines: &[String]) -> anyhow::Result<Self> {
        let mut status = Self::default();

        for line in lines{
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