//! Plays music videos in sync with the audio playing in MPD.
//!
//! pvplayer launches PVs (promotional videos, i.e. music videos) in [mpv],
//! timed against whatever [MPD] is playing, so an album can flow from song
//! into video and back without a gap. MPD stays the only audio source:
//! pvplayer mutes the video and never touches audio playback.
//!
//! It exists to remove a forced choice: either break an album's flow to go
//! put on a video, or never watch the videos at all.
//!
//! # Scope
//!
//! pvplayer is deliberately narrow. It is not a music player and plays no
//! music. Each song's video is linked by hand (see `annotate`), a process
//! meant for the handful of albums you care about enough to do it.
//!
//! > **Alpha.** Built for one person and tested on one macOS laptop. The
//! > music library path is hardcoded in [`pv_link`].
//!
//! # How it works
//!
//! **Before listening:** link each song to its video with the `annotate`
//! binary. You give it the sync offset (`pv_in_ms`), taken from any NLE,
//! as raw milliseconds or timecode, and it stores the link in MPD's
//! sticker database.
//!
//! **While listening:** run `pvplay` in one terminal and any MPD client
//! (e.g. rmpc) in another, then just play albums. On every MPD change,
//! pvplayer:
//!
//! 1. **Joins** the current song's video if it has one, seeking to the
//!    matching position.
//! 2. **Looks ahead** to the next song. If it has a video, that video is
//!    *preloaded* paused (when nothing is playing) or *queued* (when the
//!    current song's video is still playing; there is only one mpv window).
//! 3. **Starts** the next video `pv_in_ms` before the next song begins, so
//!    its lead-in plays over the end of the current song and lands in sync.
//! 4. **Mirrors pauses:** on pause, mpv pauses and seeks to MPD's position.
//!    On resume it simply unpauses.
//!
//! The scheduling lives in [`playback`], and the start-time arithmetic in
//! [`timing`].
//!
//! # Timing model
//!
//! Every link has one number, `pv_in_ms`: the position in the video, in
//! milliseconds, that lines up with the start of the song's audio *file*
//! (its first byte, including any leading silence, not the first audible
//! note). Measure it in any NLE by syncing the two and reading the video's
//! timecode at the head of the audio clip.
//!
//! ```text
//! previous song  ──────────────────────┤ song ────────────────────
//! video                  ├── pre-roll ─┼── in sync ───────────────
//!                        0             pv_in_ms
//! ```
//!
//! - **Positive:** the video has pre-roll. It starts `pv_in_ms` before the
//!   song, playing over the end of the previous track.
//! - **Zero:** video and song file start together.
//! - **Negative:** the song file starts before the video (the audio has an
//!   intro the video lacks). The video should start `|pv_in_ms|` into the
//!   song. See [Limitations](#limitations).
//!
//! The start time is computed by [`timing::seconds_until_pv`]:
//! `duration - elapsed - pv_in_ms`, in seconds of the previous song's playback.
//!
//! # Sticker schema
//!
//! Links live in MPD's sticker database, attached to each song's URI:
//!
//! | Sticker    | Type          | Meaning                                    | Example          |
//! |------------|---------------|--------------------------------------------|------------------|
//! | `pv`       | filename      | Video file in the artist's `[pv]` folder   | `Song Title.mp4` |
//! | `pv_in_ms` | integer (ms)  | See [Timing model](#timing-model)          | `4210`           |
//!
//! A song needs both stickers to count as linked. See [`pv_link::PvLink`].
//!
//! # Requirements
//!
//! | Tool | Why |
//! |------|-----|
//! | Rust 1.85+ | Edition 2024 |
//! | [MPD] on `localhost:6600` | Audio source; address is fixed |
//! | MPD sticker database | Stores `pv` / `pv_in_ms` (`sticker_file` in `mpd.conf`) |
//! | [mpv] on `PATH` | Plays the video |
//! | `ffprobe` on `PATH` | Only for timecode input in `annotate` |
//!
//! On macOS: `brew install mpd mpv ffmpeg`.
//!
//! # Limitations
//!
//! - Unix only (mpv is controlled over a Unix socket). Tested on one macOS laptop.
//! - The music library root is hardcoded in [`pv_link`].
//! - Assumes each video matches the album audio exactly, with no inserted
//!   scenes or cuts. One offset per song.
//! - Seeking mid-song is not re-synced; the video drifts instead of jumping.
//! - Pausing seeks the video to MPD's position, causing a visible jump.
//! - A negative `pv_in_ms` only takes effect when a song is joined partway
//!   through, after its video's start point.
//! - `annotate` handles `.mp3` audio and `.mp4` video only.
//! - Timecode input assumes the rounded frame rate (no drop-frame).
//! - Only one pvplayer can run at a time (mpv's IPC socket path is fixed).
//! - A linked video file that is missing ends pvplayer after about 5 s,
//!   when mpv's reply times out.
//!
//! [mpv]: https://mpv.io
//! [MPD]: https://www.musicpd.org

pub mod mpd;
pub mod mpv;
pub mod playback;
pub mod pv_link;
pub mod timing;
