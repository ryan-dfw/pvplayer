//! Start-time arithmetic, kept apart from I/O so it is easy to test.

use std::time::Duration;

/// Seconds of playback left before the next song's video should start.
///
/// `duration` and `elapsed` are the current song's length and position.
/// `song_zero_in_video_ms` is the next song's `pv_in_ms`: where that song
/// begins inside its video. A video with pre-roll therefore starts before
/// the current song ends. A negative result means the start time has
/// already passed.
pub fn seconds_until_pv(duration: Duration, elapsed: Duration, song_zero_in_video_ms: i64) -> f64 {
    duration.as_secs_f64() - elapsed.as_secs_f64() - song_zero_in_video_ms as f64 / 1000.0
}
