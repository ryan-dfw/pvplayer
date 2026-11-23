//! The playback engine: decides when to join, preload, start, switch,
//! pause and resume videos, and carries those decisions out.
//!
//! # Structure
//!
//! The module follows a *functional core, imperative shell* design:
//!
//! - [`PlaybackController`] is the core. It receives each MPD [`Status`]
//!   snapshot, remembers which video is *active* and which is *upcoming*,
//!   and returns a [`PlaybackAction`]. It performs no I/O, so its behavior
//!   is unit-tested with hand-built statuses (see the tests below).
//! - [`run`] is the shell. It fetches status, asks the controller what to
//!   do, and executes the answer against [`Mpv`] and [`MpdClient`].
//! - [`start_mpd_event_listener`] wakes the shell when MPD changes. MPD's
//!   `idle` command blocks its connection until something happens, so the
//!   listener uses a second connection on its own thread.
//!
//! # Wake-up schedule
//!
//! Between events the loop sleeps for [`PlaybackController::next_check_delay`]:
//! half the time remaining until the next video's start, clamped to
//! [`START_TIMING_RESOLUTION`]..=[`MAX_RECHECK_INTERVAL`]. Each wake re-reads
//! MPD's elapsed time, so timer error is corrected as the start approaches:
//!
//! ```text
//! 8 s left → sleep 4 s → 4 s left → sleep 2 s → … → 20 ms ticks → start
//! ```

use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use crate::mpd::{MpdClient, PlaybackState, Status};
use crate::mpv::Mpv;
use crate::pv_link::PvLink;
use crate::timing::seconds_until_pv;

use anyhow::{Context, Result};

/// Shortest wait between checks as a video's start time approaches.
///
/// 20 ms is the strictest commonly cited detection threshold for audio
/// leading video (ITU, 1993; ITU-R BT.1359 later relaxed it to about
/// 45 ms). Here, a late start means audio leads, the direction viewers
/// notice soonest.
const START_TIMING_RESOLUTION: Duration = Duration::from_millis(20);

/// Longest wait between checks, used when nothing is staged or MPD is not
/// playing. Also a safety recheck in case an MPD event is missed.
///
/// Keeps the query connection alive, too: MPD closes clients that stay
/// silent for its `connection_timeout` (60 s by default), and [`run`] asks
/// for status at least this often.
///
/// # Future work
///
/// Considered: cap this at a third of the upcoming track's length, for
/// tracks under 15 s. Probably unnecessary: once a video is staged the
/// wait is already half the remaining time, and every song change wakes
/// the loop through [`start_mpd_event_listener`]. Revisit if a short
/// track's video is ever observed starting late.
const MAX_RECHECK_INTERVAL: Duration = Duration::from_secs(15);

/// Whether mpv is currently showing the active video as playing or paused.
///
/// Tracked so a pause or resume is sent to mpv only once per MPD state change.
#[derive(Debug, PartialEq, Eq)]
enum VideoState {
    /// mpv is unpaused.
    Playing,
    /// mpv is paused.
    Paused,
}

/// The next song's video, staged and waiting for its start time.
#[derive(Debug)]
struct UpcomingPv {
    /// MPD `songid` of the song this video belongs to.
    target_song_id: u32,
    /// The video file and its offset.
    link: PvLink,
    /// `true` if the video is already loaded paused in mpv (*preloaded*),
    /// so starting it only needs a seek and play ([`PlaybackAction::Start`]).
    /// `false` if mpv was busy with the active video (*queued*), so it must
    /// be loaded at start time ([`PlaybackAction::Switch`]).
    prepared: bool,
}

/// The video mpv is currently showing.
///
/// Holds no path: the file is already loaded, and is never reloaded.
#[derive(Debug)]
struct ActivePv {
    /// MPD `songid` of the song this video belongs to. The video is dropped
    /// once this ID is neither the current nor the next song.
    target_song_id: u32,
    /// The video's `pv_in_ms`, used to re-align mpv when MPD pauses.
    start_offset_ms: i64,
    /// Whether mpv is playing or paused.
    video_state: VideoState,
}

/// What changed about MPD's next song, from [`PlaybackController::observe_next_song`].
#[derive(Debug, PartialEq, Eq)]
enum NextSongUpdate {
    /// The next song is the same as last time; nothing to do.
    Unchanged,
    /// A new next song (by `songid`); [`run`] should look up and stage its video.
    Load(u32),
    /// MPD reports no next song (e.g. the end of the queue).
    NoNextSong,
}

/// A decision from [`PlaybackController`], carried out by [`run`].
///
/// | Action          | When                                              | mpv does                  |
/// |-----------------|---------------------------------------------------|---------------------------|
/// | `JoinCurrent`   | A song with a video is already playing, unstaged  | load → seek → play/pause  |
/// | `Start`         | Start time reached; video preloaded               | seek → play               |
/// | `Switch`        | Start time reached; another video was playing     | load → seek → play        |
/// | `Pause`         | MPD paused while the video played                 | pause → seek to MPD       |
/// | `Resume`        | MPD resumed while the video was paused            | play                      |
#[derive(Debug, PartialEq)]
enum PlaybackAction {
    /// Join a video partway through, for a song that is already playing.
    JoinCurrent {
        /// The video to load.
        path: PathBuf,
        /// MPD `songid` of the playing song.
        target_song_id: u32,
        /// The video's `pv_in_ms`.
        start_offset_ms: i64,
        /// `false` if MPD is paused: load and seek, but leave mpv paused.
        should_play: bool,
    },
    /// Play the preloaded video from its corrected position.
    Start {
        /// MPD `songid` of the song the video belongs to.
        target_song_id: u32,
        /// The video's `pv_in_ms`.
        start_offset_ms: i64,
    },
    /// Load the queued video at its start time, replacing the active one.
    Switch {
        /// The video to load.
        path: PathBuf,
        /// MPD `songid` of the song the video belongs to.
        target_song_id: u32,
        /// The video's `pv_in_ms`.
        start_offset_ms: i64,
    },
    /// Pause mpv, then seek it to MPD's position.
    Pause {
        /// Video position in seconds matching MPD, or `None` if it can't be
        /// computed (then mpv is paused without seeking).
        seek_to: Option<f64>,
    },
    /// Unpause mpv. No seek; the pause already aligned it.
    Resume,
}

/// A message from the [`start_mpd_event_listener`] thread.
enum MpdEvent {
    /// MPD's player or queue changed; the loop should re-check status.
    Changed,
    /// The listener's connection failed. [`run`] returns this error and
    /// the program exits.
    Failed(anyhow::Error),
}

/// The decision-making core of playback. See the [module docs](self).
///
/// Holds what pvplayer remembers between MPD snapshots and turns each new
/// [`Status`] into at most one [`PlaybackAction`]. Performs no I/O.
#[derive(Debug)]
struct PlaybackController {
    /// `songid` of the current song at the last check. Used to notice when
    /// the current song changes (by playback, skip, or jump).
    previous_song_id: Option<u32>,

    /// `songid` of the next song at the last check. Used to notice when
    /// the next song changes, so its video can be staged.
    previous_next_id: Option<u32>,

    /// The last status line printed, so identical lines are not repeated.
    previous_display: String,

    /// The video mpv is showing, if any.
    active: Option<ActivePv>,

    /// The next song's video, staged and waiting for its start time, if any.
    upcoming: Option<UpcomingPv>,
}

impl PlaybackController {
    /// Creates a controller that has seen nothing yet: no songs, no videos.
    fn new() -> Self {
        Self {
            previous_song_id: None,
            previous_next_id: None,
            previous_display: String::new(),
            active: None,
            upcoming: None,
        }
    }

    /// Returns the current song's ID if it just changed and nothing has been
    /// staged or activated for it, meaning [`run`] should look up a video to
    /// join. Records the ID, so each song is considered once.
    fn current_song_to_join(&mut self, status: &Status) -> Option<u32> {
        if status.song_id == self.previous_song_id {
            return None;
        }

        self.previous_song_id = status.song_id;

        let current_song_id = status.song_id?;

        let already_active = self
            .active
            .as_ref()
            .is_some_and(|active| active.target_song_id == current_song_id);

        let already_upcoming = self
            .upcoming
            .as_ref()
            .is_some_and(|upcoming| upcoming.target_song_id == current_song_id);

        (!already_active && !already_upcoming).then_some(current_song_id)
    }

    /// Prepares to join the current song's video mid-song, with no pre-roll.
    ///
    /// Used when a song with a video is already playing and nothing was staged
    /// for it, e.g. pvplayer started mid-album or the user skipped. Marks the
    /// video active (playing or paused to match MPD), clears any staged video,
    /// and resets next-song tracking so the next song is looked up again.
    ///
    /// Returns `None`, and changes nothing, if the song has not yet reached
    /// its video's start point (negative `pv_in_ms`).
    fn join_current_action(
        &mut self,
        target_song_id: u32,
        link: PvLink,
        status: &Status,
    ) -> Option<PlaybackAction> {
        intended_video_position(status, target_song_id, link.start_offset_ms)?;

        let should_play = matches!(&status.state, Some(PlaybackState::Playing));

        let video_state = if should_play {
            VideoState::Playing
        } else {
            VideoState::Paused
        };

        let start_offset_ms = link.start_offset_ms;

        self.active = Some(ActivePv {
            target_song_id,
            start_offset_ms,
            video_state,
        });

        self.upcoming = None;
        self.previous_next_id = None;
        self.previous_display.clear();

        Some(PlaybackAction::JoinCurrent {
            path: link.path,
            target_song_id,
            start_offset_ms,
            should_play,
        })
    }

    /// Reacts to MPD's next song changing.
    ///
    /// Also drops the active video once its song is neither current nor next.
    /// When the next song changes, clears any staged video and returns
    /// [`NextSongUpdate::Load`] (or [`NextSongUpdate::NoNextSong`]) so [`run`]
    /// can stage the new one.
    fn observe_next_song(&mut self, status: &Status) -> NextSongUpdate {
        let active_is_relevant = self.active.as_ref().is_some_and(|active| {
            Some(active.target_song_id) == status.song_id
                || Some(active.target_song_id) == status.next_song_id
        });

        if self.active.is_some() && !active_is_relevant {
            self.active = None;
        }

        if status.next_song_id == self.previous_next_id {
            return NextSongUpdate::Unchanged;
        }

        self.previous_display.clear();
        self.previous_next_id = status.next_song_id;

        self.upcoming = None;

        match status.next_song_id {
            Some(id) => NextSongUpdate::Load(id),
            None => NextSongUpdate::NoNextSong,
        }
    }

    /// Whether mpv is free to preload the next video, i.e. no video is active.
    /// There is only one mpv window.
    fn can_preload(&self) -> bool {
        self.active.is_none()
    }

    /// Stages `link` as the next song's video.
    ///
    /// Does not touch mpv: [`run`] has already preloaded it when `prepared` is
    /// `true` (see [`UpcomingPv::prepared`]).
    fn stage(&mut self, target_song_id: u32, link: PvLink, prepared: bool) {
        self.upcoming = Some(UpcomingPv {
            target_song_id,
            link,
            prepared,
        });
    }

    /// Seconds of MPD playback until the staged video should start.
    ///
    /// Negative once the start time has passed. `None` if nothing is staged or
    /// MPD has not reported duration and elapsed time.
    fn staged_seconds_until_start(&self, status: &Status) -> Option<f64> {
        let upcoming = self.upcoming.as_ref()?;
        let duration = status.duration?;
        let elapsed = status.elapsed?;

        Some(seconds_until_pv(
            duration,
            elapsed,
            upcoming.link.start_offset_ms,
        ))
    }

    /// How long the loop may sleep before checking MPD again.
    ///
    /// Half the time until the staged video's start, clamped to
    /// [`START_TIMING_RESOLUTION`]..=[`MAX_RECHECK_INTERVAL`]. The maximum when
    /// MPD is not playing or nothing is staged. See [Wake-up schedule](self#wake-up-schedule).
    fn next_check_delay(&self, status: &Status) -> Duration {
        if !matches!(&status.state, Some(PlaybackState::Playing)) {
            return MAX_RECHECK_INTERVAL;
        }

        let Some(remaining) = self.staged_seconds_until_start(status) else {
            return MAX_RECHECK_INTERVAL;
        };

        if remaining <= 0.0 {
            return START_TIMING_RESOLUTION;
        }

        Duration::from_secs_f64(remaining / 2.0)
            .clamp(START_TIMING_RESOLUTION, MAX_RECHECK_INTERVAL)
    }

    /// Builds a one-line status message ("PV begins in N playback seconds")
    /// for [`run`] to print. `None` if nothing is staged, MPD is stopped, or the
    /// line is unchanged since last time.
    fn display_update(&mut self, status: &Status) -> Option<String> {
        let remaining = self.staged_seconds_until_start(status)?;

        let activity = match &status.state {
            Some(PlaybackState::Playing) => "Playing",
            Some(PlaybackState::Paused) => "Paused",
            _ => return None,
        };

        let display = if remaining >= 0.0 {
            format!(
                "{} - PV begins in {:.0} playback seconds",
                activity,
                remaining.ceil()
            )
        } else {
            format!(
                "{} - intended PV position: {:.0} seconds",
                activity,
                (-remaining).floor()
            )
        };

        if display == self.previous_display {
            return None;
        }

        self.previous_display = display.clone();
        Some(display)
    }

    /// Decides the next video action for this status, if any.
    ///
    /// First, if MPD is playing and the staged video's start time has passed,
    /// promotes it to active and returns [`PlaybackAction::Start`] (preloaded)
    /// or [`PlaybackAction::Switch`] (queued). Otherwise, mirrors MPD's
    /// play/pause onto the active video, returning [`PlaybackAction::Pause`] or
    /// [`PlaybackAction::Resume`] only when the state actually changes.
    fn next_action(&mut self, status: &Status) -> Option<PlaybackAction> {
        let should_start = matches!(&status.state, Some(PlaybackState::Playing))
            && self
                .staged_seconds_until_start(status)
                .is_some_and(|remaining| remaining <= 0.0);

        if should_start {
            let upcoming = self.upcoming.take()?;

            let UpcomingPv {
                target_song_id,
                link,
                prepared,
            } = upcoming;

            let start_offset_ms = link.start_offset_ms;

            self.active = Some(ActivePv {
                target_song_id,
                start_offset_ms,
                video_state: VideoState::Playing,
            });

            return Some(if prepared {
                PlaybackAction::Start {
                    target_song_id,
                    start_offset_ms,
                }
            } else {
                PlaybackAction::Switch {
                    path: link.path,
                    target_song_id,
                    start_offset_ms,
                }
            });
        }

        match (self.active.as_mut(), &status.state) {
            (
                Some(ActivePv {
                    target_song_id,
                    start_offset_ms,
                    video_state,
                }),
                Some(PlaybackState::Paused),
            ) if *video_state == VideoState::Playing => {
                let seek_to = intended_video_position(status, *target_song_id, *start_offset_ms);

                *video_state = VideoState::Paused;

                Some(PlaybackAction::Pause { seek_to })
            }

            (Some(ActivePv { video_state, .. }), Some(PlaybackState::Playing))
                if *video_state == VideoState::Paused =>
            {
                *video_state = VideoState::Playing;
                Some(PlaybackAction::Resume)
            }

            _ => None,
        }
    }
}

/// Starts a thread that waits on MPD's `idle` and sends an [`MpdEvent`]
/// whenever the player or queue changes.
///
/// Uses its own connection, because `idle` blocks the connection it runs on.
///
/// # Errors
///
/// Returns an error only if the initial connection to MPD at `address`
/// fails. Later failures are sent as [`MpdEvent::Failed`] instead.
fn start_mpd_event_listener(address: &str) -> Result<mpsc::Receiver<MpdEvent>> {
    let mut event_client = MpdClient::connect_for_events(address)?;
    let (sender, receiver) = mpsc::channel();

    thread::spawn(move || {
        loop {
            match event_client.wait_for_change() {
                Ok(()) => {
                    if sender.send(MpdEvent::Changed).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = sender.send(MpdEvent::Failed(error));
                    break;
                }
            }
        }
    });

    Ok(receiver)
}

/// Where the video for `target_song_id` should be right now, in seconds.
///
/// - If the target song is current: `elapsed + pv_in_ms`.
/// - If it is next (pre-roll): `pv_in_ms − time left in the current song`,
///   i.e. `elapsed + pv_in_ms − duration`.
///
/// Returns `None` if the position would be negative (the video should not
/// have started yet), the song is neither current nor next, or MPD has not
/// reported the needed times.
fn intended_video_position(
    status: &Status,
    target_song_id: u32,
    start_offset_ms: i64,
) -> Option<f64> {
    let offset_seconds = start_offset_ms as f64 / 1000.0;

    let position = if status.song_id == Some(target_song_id) {
        status.elapsed?.as_secs_f64() + offset_seconds
    } else if status.next_song_id == Some(target_song_id) {
        status.elapsed?.as_secs_f64() + offset_seconds - status.duration?.as_secs_f64()
    } else {
        return None;
    };

    (position >= 0.0).then_some(position)
}

/// Runs pvplayer: connects to MPD and mpv, then follows playback forever.
///
/// Opens two MPD connections (one for queries, one for
/// [`start_mpd_event_listener`]) and one muted mpv process. Each loop pass:
///
/// 1. **Look:** fetches MPD's [`Status`].
/// 2. **Join:** if a new current song has an unstaged video, prepares
///    [`PlaybackAction::JoinCurrent`].
/// 3. **Stage:** if the next song changed and has a video, preloads it into
///    mpv (when free) or queues it.
/// 4. **Show:** prints the status line, if it changed.
/// 5. **Act:** carries out the join, or else [`PlaybackController::next_action`].
///    Each action re-reads MPD just before seeking, for accuracy.
/// 6. **Wait:** until MPD changes or [`PlaybackController::next_check_delay`]
///    elapses.
///
/// Joins and resumes print a `JOIN CHECK` / `RESUME CHECK` line with the
/// measured video-vs-audio error in ms. These are diagnostic only.
///
/// # Errors
///
/// Never returns `Ok`; it runs until something fails, then returns the error
/// and the program exits. Any of these ends it:
/// - MPD is unreachable at `localhost:6600`, or a command to it fails
/// - mpv cannot be started, rejects a command, or closes (e.g. its window is closed)
/// - the event listener's connection fails
/// - the current song changes while its video is being joined
/// - a linked video file is missing (mpv never reports it loaded; see
///   [`Mpv::load_paused`])
pub fn run() -> Result<()> {
    // Two MPD connections: `idle` blocks the one it runs on, so events get their own.
    let mut client = MpdClient::connect("localhost:6600")?;
    let mpd_events = start_mpd_event_listener("localhost:6600")?;
    let mut mpv = Mpv::start()?;
    let mut controller = PlaybackController::new();

    loop {
        // 1. Look: one snapshot drives every decision in this pass.
        let status = client.get_status()?;

        // 2. Join: a song with a video is playing that nothing staged (started
        //    mid-album, or skipped). Held as pending so it runs before next_action.
        let mut pending_action = None;

        if let Some(id) = controller.current_song_to_join(&status) {
            let song = client.get_song(id)?;
            let stickers = client.get_stickers(&song.file)?;

            if let Some(link) = PvLink::from_stickers(&song.file, &stickers) {
                pending_action = controller.join_current_action(id, link, &status);

                if pending_action.is_none() {
                    // Negative pv_in_ms and the song hasn't reached the video's start yet.
                    println!("Current PV has not reached its start time")
                }
            }
        }

        // 3. Stage: look up the next song's video whenever the next song changes.
        match controller.observe_next_song(&status) {
            NextSongUpdate::Unchanged => {}

            NextSongUpdate::Load(id) => {
                let song = client.get_song(id)?;

                println!("Next up: {}", song.title.as_deref().unwrap_or(&song.file));

                let stickers = client.get_stickers(&song.file)?;

                if let Some(link) = PvLink::from_stickers(&song.file, &stickers) {
                    // One mpv window: preload only if no video is showing; otherwise it's
                    // loaded at start time (Switch).
                    let prepared = controller.can_preload();

                    if prepared {
                        println!("Preloading PV: {}", link.path.display());
                        mpv.load_paused(&link.path)?;
                    } else {
                        println!("Queuing PV behind active video: {}", link.path.display());
                    }

                    controller.stage(id, link, prepared);
                } else {
                    println!("No playable PV link");
                }
            }

            NextSongUpdate::NoNextSong => {
                println!("No next song reported");
            }
        }

        // 4. Show: status line, printed only when it changes.
        if let Some(display) = controller.display_update(&status) {
            println!("{}", display);
        }

        // 5. Act: a pending join wins; otherwise ask the controller.
        if let Some(action) = pending_action.or_else(|| controller.next_action(&status)) {
            match action {
                PlaybackAction::JoinCurrent {
                    path,
                    target_song_id,
                    start_offset_ms,
                    should_play,
                } => {
                    mpv.load_paused(&path)?;

                    // Re-read MPD: loading took time, so the snapshot from step 1 is stale.
                    let refreshed_status = client.get_status()?;

                    let position =
                        intended_video_position(&refreshed_status, target_song_id, start_offset_ms)
                            .context("Current song changed while joining its PV")?;

                    mpv.seek_absolute(position)?;

                    if should_play {
                        mpv.play()?;

                        // Diagnostic: let playback settle, then measure video-vs-audio error.
                        thread::sleep(Duration::from_millis(500));

                        let status_after_start = client.get_status()?;

                        // Timestamp the status so the time spent querying mpv is added back in.
                        let status_received_at = Instant::now();
                        let mpv_position = mpv.time_position()?;
                        let mpv_audio_position = mpv.audio_position();

                        if let Some(expected_position) = intended_video_position(
                            &status_after_start,
                            target_song_id,
                            start_offset_ms,
                        ) {
                            let expected_now =
                                expected_position + status_received_at.elapsed().as_secs_f64();

                            let error_ms = (mpv_position - expected_now) * 1000.0;

                            println!(
                                "JOIN CHECK: MPD expects {:.3}s, mpv video {:.3}s, mpv audio {:?}, video error {error_ms:+.1}ms",
                                expected_now, mpv_position, mpv_audio_position,
                            );
                        }
                    }

                    println!("Joined current PV at {position:.3} seconds");
                }

                PlaybackAction::Start {
                    target_song_id,
                    start_offset_ms,
                } => {
                    // Correct the preload's position: time has passed since it was loaded.
                    let refreshed_status = client.get_status()?;

                    if let Some(position) =
                        intended_video_position(&refreshed_status, target_song_id, start_offset_ms)
                    {
                        mpv.seek_absolute(position)?;
                        println!("Correcting initial PV to {position:.3} seconds");
                    }
                    mpv.play()?;
                    println!("Started PV");
                }

                PlaybackAction::Switch {
                    path,
                    target_song_id,
                    start_offset_ms,
                } => {
                    mpv.load_paused(&path)?;

                    // Re-read MPD after the load, which is the slow part of a switch.
                    let refreshed_status = client.get_status()?;

                    if let Some(position) =
                        intended_video_position(&refreshed_status, target_song_id, start_offset_ms)
                    {
                        mpv.seek_absolute(position)?;
                    }

                    mpv.play()?;
                    println!("Switched to consecutive PV");
                }

                PlaybackAction::Pause { seek_to } => {
                    mpv.pause()?;

                    // Seek while paused so resume needs no correction ("jump on pause").
                    if let Some(position) = seek_to {
                        mpv.seek_absolute(position)?;
                    }

                    println!("Paused PV");
                }

                PlaybackAction::Resume => {
                    // Diagnostic only: measures drift before resuming; nothing is corrected.
                    let refreshed_status = client.get_status()?;
                    let status_received_at = Instant::now();

                    let mpv_position = mpv.time_position()?;

                    if let Some(elapsed) = refreshed_status.elapsed {
                        let expected_position =
                            elapsed.as_secs_f64() + status_received_at.elapsed().as_secs_f64();

                        let error_ms = (mpv_position - expected_position) * 1000.0;

                        println!(
                            "RESUME CHECK: MPD expects {:.3}s, mpv is {:.3}s, error {error_ms:+.1}ms",
                            expected_position, mpv_position
                        );
                    }

                    mpv.play()?;
                    println!("Resumed PV");
                }
            }
        }

        // 6. Wait: wake on an MPD change, or when the adaptive delay elapses.
        let next_check_delay = controller.next_check_delay(&status);
        match mpd_events.recv_timeout(next_check_delay) {
            Ok(MpdEvent::Changed) => {}

            Ok(MpdEvent::Failed(err)) => {
                return Err(err);
            }

            Err(RecvTimeoutError::Timeout) => {}

            Err(RecvTimeoutError::Disconnected) => {
                anyhow::bail!("MPD event listener stopped unexpectedly")
            }
        }
    }
}

/// Controller and position tests, using hand-built [`Status`] snapshots.
/// No MPD or mpv is involved.
#[cfg(test)]
mod tests {
    use super::{
        ActivePv, NextSongUpdate, PlaybackAction, PlaybackController, VideoState,
        intended_video_position,
    };
    use crate::mpd::{PlaybackState, Status};
    use crate::pv_link::PvLink;
    use std::path::PathBuf;
    use std::time::Duration;

    /// During the current song, the video position is `elapsed + pv_in_ms`.
    #[test]
    fn calculates_position_for_current_song() {
        let status = Status {
            song_id: Some(42),
            elapsed: Some(Duration::from_secs_f64(12.5)),
            ..Status::default()
        };

        assert_eq!(intended_video_position(&status, 42, 3_000), Some(15.5));
    }

    /// During pre-roll, the position is `pv_in_ms` minus the time left in the previous song.
    #[test]
    fn calculates_position_during_preroll() {
        let status = Status {
            next_song_id: Some(42),
            elapsed: Some(Duration::from_secs(238)),
            duration: Some(Duration::from_secs(240)),
            ..Status::default()
        };

        assert_eq!(intended_video_position(&status, 42, 3_000), Some(1.0));
    }

    /// Before pre-roll begins, there is no position yet.
    #[test]
    fn returns_none_before_preroll_begins() {
        let status = Status {
            next_song_id: Some(42),
            elapsed: Some(Duration::from_secs(230)),
            duration: Some(Duration::from_secs(240)),
            ..Status::default()
        };

        assert_eq!(intended_video_position(&status, 42, 3_000), None,);
    }

    /// A song that is neither current nor next has no position.
    #[test]
    fn returns_none_for_unrelated_song() {
        let status = Status {
            song_id: Some(7),
            next_song_id: Some(8),
            elapsed: Some(Duration::from_secs(10)),
            duration: Some(Duration::from_secs(200)),
            ..Status::default()
        };

        assert_eq!(intended_video_position(&status, 42, 3_000), None,);
    }

    /// A new next song is loaded without dropping the video still playing.
    #[test]
    fn requests_next_pv_while_keeping_current_pv_active() {
        let mut controller = PlaybackController::new();
        controller.previous_next_id = Some(42);
        controller.active = Some(ActivePv {
            target_song_id: 42,
            start_offset_ms: 3_000,
            video_state: VideoState::Playing,
        });

        let status = Status {
            song_id: Some(42),
            next_song_id: Some(99),
            ..Status::default()
        };

        assert_eq!(
            controller.observe_next_song(&status),
            NextSongUpdate::Load(99)
        );

        assert!(matches!(
            controller.active,
            Some(ActivePv {
                target_song_id: 42,
                start_offset_ms,
                ..
            })
        ));
    }

    /// The active video is dropped once its song is neither current nor next.
    #[test]
    fn clears_active_pv_after_target_song_is_no_longer_relevant() {
        let mut controller = PlaybackController::new();
        controller.previous_next_id = Some(100);
        controller.active = Some(ActivePv {
            target_song_id: 42,
            start_offset_ms: 3_000,
            video_state: VideoState::Playing,
        });

        let status = Status {
            song_id: Some(99),
            next_song_id: Some(100),
            ..Status::default()
        };

        assert_eq!(
            controller.observe_next_song(&status),
            NextSongUpdate::Unchanged
        );
        assert!(controller.active.is_none());
    }

    /// A preloaded video starts on time, then pauses (with a seek) and resumes with MPD.
    #[test]
    fn handles_start_pause_resume_flow() {
        let mut controller = PlaybackController::new();

        controller.stage(
            42,
            PvLink {
                path: PathBuf::from("/test/pv.mp4"),
                start_offset_ms: 3_000,
                video_cutoff_ms: None,
            },
            true,
        );

        let ready_to_start = Status {
            duration: Some(Duration::from_secs(240)),
            elapsed: Some(Duration::from_secs(238)),
            state: Some(PlaybackState::Playing),
            ..Status::default()
        };

        assert_eq!(
            controller.next_action(&ready_to_start),
            Some(PlaybackAction::Start {
                target_song_id: 42,
                start_offset_ms: 3_000
            }),
        );

        let paused = Status {
            song_id: Some(42),
            elapsed: Some(Duration::from_secs(10)),
            state: Some(PlaybackState::Paused),
            ..Status::default()
        };

        assert_eq!(
            controller.next_action(&paused),
            Some(PlaybackAction::Pause {
                seek_to: Some(13.0),
            }),
        );

        let resumed = Status {
            song_id: Some(42),
            state: Some(PlaybackState::Playing),
            ..Status::default()
        };

        assert_eq!(
            controller.next_action(&resumed),
            Some(PlaybackAction::Resume),
        );
    }

    /// A queued video is switched in at its start time and becomes the active one.
    #[test]
    fn switches_to_unprepared_upcoming_pv() {
        let mut controller = PlaybackController::new();
        controller.active = Some(ActivePv {
            target_song_id: 42,
            start_offset_ms: 3_000,
            video_state: VideoState::Playing,
        });

        let upcoming_path = PathBuf::from("/test/upcoming.mp4");

        controller.stage(
            99,
            PvLink {
                path: upcoming_path.clone(),
                start_offset_ms: 3_000,
                video_cutoff_ms: None,
            },
            false,
        );

        assert!(controller.active.is_some());
        assert!(controller.upcoming.is_some());

        let ready_to_switch = Status {
            song_id: Some(42),
            next_song_id: Some(99),
            duration: Some(Duration::from_secs(240)),
            elapsed: Some(Duration::from_secs(238)),
            state: Some(PlaybackState::Playing),
            ..Status::default()
        };

        assert_eq!(
            controller.next_action(&ready_to_switch),
            Some(PlaybackAction::Switch {
                path: upcoming_path,
                target_song_id: 99,
                start_offset_ms: 3_000,
            }),
        );

        assert!(controller.upcoming.is_none());
        assert!(matches!(
            controller.active,
            Some(ActivePv {
                target_song_id: 99,
                ..
            })
        ));
    }
}
