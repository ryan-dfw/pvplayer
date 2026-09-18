use std::path::{Path, PathBuf};

// below is 100% personal convention which must be made universal before shipping,
// kicking that can as i build

const MUSIC_ROOT: &str = "/Volumes/Elm_SSD/0/iTunes Media/Music";

pub(super) fn resolve_pv_path(
    song_uri: &str,
    pv_filename: &str,
) -> Option<PathBuf> {
    let track_path = Path::new(MUSIC_ROOT).join(song_uri);
    let artist_dir = track_path.parent()?.parent()?;
    let artist_name = artist_dir.file_name()?.to_str()?;

    Some(
        artist_dir
            .join(format!("{} [pv]", artist_name))
            .join(pv_filename),
    )
}