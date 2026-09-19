use std::time::Duration;

pub fn seconds_until_pv(
    duration: Duration,
    elapsed: Duration,
    song_zero_in_video_ms: i64,
) -> f64 {
    duration.as_secs_f64()
    - elapsed.as_secs_f64()
    - song_zero_in_video_ms as f64 / 1000.0
}