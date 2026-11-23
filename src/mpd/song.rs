//! A song in MPD's queue, as reported by `playlistid`.

use anyhow::Context;

/// A queue entry: its file and, if tagged, its title.
#[derive(Debug)]
pub struct Song {
    /// Path relative to MPD's music directory, also used as the URI for
    /// sticker commands.
    pub file: String,
    /// The `Title` tag, if the file has one.
    pub title: Option<String>,
}

impl Song {
    /// Parses `playlistid` reply lines.
    ///
    /// # Errors
    ///
    /// Returns an error if the reply has no `file:` line.
    pub(super) fn parse(lines: &[String]) -> anyhow::Result<Self> {
        let mut file = None;
        let mut title = None;

        for line in lines {
            let line = line.strip_suffix('\n').unwrap_or(line);

            if let Some(value) = line.strip_prefix("file: ") {
                file = Some(value.to_owned());
            }

            if let Some(value) = line.strip_prefix("Title: ") {
                title = Some(value.to_owned());
            }
        }

        Ok(Self {
            file: file.context("MPD song response is missing its file")?,
            title,
        })
    }
}
