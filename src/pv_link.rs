mod paths;

use self::paths::resolve_pv_path;
use std::path::PathBuf;

#[derive(Debug)]
pub struct PvLink {
    pub path: PathBuf,
    pub start_offset_ms: i64,
    pub video_cutoff_ms: Option<u64>,
}

impl PvLink {
    pub fn from_stickers(
        song_uri: &str,
        stickers: &[String],
    ) -> Option<Self> {
        let mut filename = None;
        let mut start_offset_ms = None;

        for line in stickers {
            let Some(sticker) = line.strip_prefix("sticker: ") else {
                continue;
            };

            let Some((name, value)) = sticker
                .trim_end_matches('\n')
                .split_once('=')
            else {
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