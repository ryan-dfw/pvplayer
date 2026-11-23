//! Turns a song's MPD stickers into a playable video link.
//!
//! See [Sticker schema](crate#sticker-schema) for the stickers involved.

mod paths;

use self::paths::resolve_pv_path;
use std::path::PathBuf;

/// A song's linked video: where the file is and how it lines up.
#[derive(Debug)]
pub struct PvLink {
    /// Full path to the video file.
    pub path: PathBuf,
    /// The `pv_in_ms` sticker: the video position, in milliseconds, that
    /// lines up with the start of the song's audio file. See
    /// [Timing model](crate#timing-model).
    pub start_offset_ms: i64,
    /// Not currently used; always `None`.
    pub video_cutoff_ms: Option<u64>,
}

impl PvLink {
    /// Builds a link from a song's raw `sticker: name=value` lines, as
    /// returned by [`MpdClient::get_stickers`](crate::mpd::MpdClient::get_stickers).
    ///
    /// `song_uri` is the song's path relative to the music library, used to
    /// find its artist's `[pv]` folder.
    ///
    /// Returns `None` (the song has no playable link) if:
    /// - there is no `pv` sticker, or it is blank
    /// - there is no `pv_in_ms` sticker, or it isn't a whole number
    /// - the video's path can't be worked out from `song_uri`
    ///   (see [`paths`])
    ///
    /// Does not check that the video file exists.
    pub fn from_stickers(song_uri: &str, stickers: &[String]) -> Option<Self> {
        let mut filename = None;
        let mut start_offset_ms = None;

        for line in stickers {
            let Some(sticker) = line.strip_prefix("sticker: ") else {
                continue;
            };

            let Some((name, value)) = sticker.trim_end_matches('\n').split_once('=') else {
                continue;
            };

            match name {
                "pv" => filename = Some(value),
                "pv_in_ms" => {
                    start_offset_ms = value.parse::<i64>().ok();
                }
                _ => {}
            }
        }

        let filename = filename?;
        if filename.trim().is_empty() {
            return None;
        }

        let start_offset_ms = start_offset_ms?;

        let path = resolve_pv_path(song_uri, filename)?;

        Some(Self {
            path,
            start_offset_ms,
            video_cutoff_ms: None,
        })
    }
}
