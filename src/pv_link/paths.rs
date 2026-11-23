//! Resolves a video filename to a full path via the `Artist [pv]` folder.
//!
//! Videos live in a folder beside the artist's albums, named after the
//! artist with ` [pv]` appended:
//!
//! ```text
//! Music/
//! └── Artist/
//!     ├── Album/
//!     │   └── 01 Song.mp3
//!     └── Artist [pv]/
//!         └── Song.mp4
//! ```

use std::path::{Path, PathBuf};

// below is 100% personal convention which must be made universal before shipping,
// kicking that can as i build

/// The music library root: the folder MPD's song URIs are relative to.
///
/// > **Note:** machine-specific. Must match `music_directory` in `mpd.conf`.
const MUSIC_ROOT: &str = "/Volumes/Elm_SSD/0/iTunes Media/Music";

/// Full path of `pv_filename` in the `[pv]` folder of the artist that owns
/// `song_uri` (expected as `Artist/Album/Track`).
///
/// Returns `None` if `song_uri` has too few folder levels to find an
/// artist, or the artist folder's name isn't valid UTF-8.
pub(super) fn resolve_pv_path(song_uri: &str, pv_filename: &str) -> Option<PathBuf> {
    let track_path = Path::new(MUSIC_ROOT).join(song_uri);
    let artist_dir = track_path.parent()?.parent()?;
    let artist_name = artist_dir.file_name()?.to_str()?;

    Some(
        artist_dir
            .join(format!("{} [pv]", artist_name))
            .join(pv_filename),
    )
}
