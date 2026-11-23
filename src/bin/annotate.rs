//! `annotate`: links songs to their music videos, interactively.
//!
//! Walks you through **Artist → Album → Song** with fuzzy-search pickers,
//! finds each song's video in the artist's `[pv]` folder by title, and
//! writes the `pv` and `pv_in_ms` stickers to MPD once you enter the sync
//! offset. See [Sticker schema](../pvplay/index.html#sticker-schema).
//!
//! # Usage
//!
//! ```sh
//! cargo run --release --bin annotate
//! ```
//!
//! Pick an artist, then an album, then songs one at a time; press Esc in
//! the song list to quit. For a song with a matching video, enter the
//! offset as milliseconds (`4210`, `-350`) or timecode (`[hh:]mm:ss:ff`,
//! read at the video's frame rate via `ffprobe`), then confirm.
//!
//! # Colors
//!
//! | Color  | Song list                                   | Album list                   |
//! |--------|---------------------------------------------|------------------------------|
//! | Orange | One matching video found, not yet linked    | Has at least one orange song |
//! | Blue   | Already linked (offset shown)               | Has linked songs, no orange  |
//! | Red    | Several videos match; nothing will be done  | (not shown)                  |
//! | Plain  | No matching video                           | Nothing to do or done        |
//!
//! # Matching
//!
//! A video matches a song when their titles are equal after lowercasing
//! and dropping everything but letters and digits. The song's title is its
//! ID3 title (or file name); the video's is its file name, minus any
//! leading track number (`03 Song.mp4` → `Song`). Only `.mp3` audio and
//! `.mp4` video are considered; hidden files are skipped.
//!
//! # Notes
//!
//! - Every song shown must be in MPD's database (`mpc update`), or the
//!   sticker lookup fails and `annotate` exits.
//! - A song that is already linked can't be relinked here; change its
//!   stickers with an MPD client instead.

use anyhow::{Context, bail};
use dialoguer::{
    Confirm, FuzzySelect, Input,
    console::{Style, measure_text_width},
};
use id3::{Tag, TagLike};
use pvplay::{mpd::MpdClient, pv_link::PvLink};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The music library root, with a trailing slash so stripping it leaves an
/// MPD-style relative URI.
///
/// > **Note:** machine-specific. Must match `music_directory` in `mpd.conf`.
const MUSIC_ROOT: &str = "/Volumes/Elm_SSD/0/iTunes Media/Music/";
/// Where MPD listens.
const MPD_ADDRESS: &str = "localhost:6600";

/// An artist's videos, keyed by [`match_key`] of their title. A key with
/// more than one path is ambiguous.
type PvCandidates = HashMap<String, Vec<PathBuf>>;

/// How a song relates to the artist's videos.
#[derive(Debug)]
enum PvMatch {
    /// Already linked by stickers (shown blue).
    Associated(PvLink),
    /// Exactly one video matches and it isn't linked yet (shown orange).
    Ready(PathBuf),
    /// No video matches (shown plain).
    Unavailable,
    /// Several videos match; the song is left alone (shown red).
    Ambiguous(Vec<PathBuf>),
}

/// One song in the song picker.
#[derive(Debug)]
struct SongChoice {
    /// Full path to the `.mp3`.
    audio_path: PathBuf,
    /// Path relative to [`MUSIC_ROOT`]: the MPD URI used for stickers.
    uri: String,
    /// ID3 title, or the file name if untagged.
    title: String,
    /// ID3 track number, if tagged.
    track: Option<u32>,
    /// Its video status.
    pv_match: PvMatch,
}

/// An album's color in the album picker, from its songs' [`PvMatch`]es.
#[derive(Debug)]
enum AlbumPvStatus {
    /// At least one song is [`PvMatch::Ready`] (shown orange).
    Ready,
    /// No ready songs, but at least one is linked (shown blue).
    Associated,
    /// Nothing to link and nothing linked (shown plain).
    Unavailable,
}

impl SongChoice {
    /// The song's base label: `NN- Title` when it has a track number.
    fn label(&self) -> String {
        match self.track {
            Some(number) => format!("{number:02}- {}", self.title),
            None => self.title.clone(),
        }
    }

    /// The label as shown in the picker: colored by [`PvMatch`], with linked
    /// songs' offsets aligned in a column `label_width` wide.
    fn display_name(&self, label_width: usize) -> String {
        let label = self.label();

        match &self.pv_match {
            PvMatch::Associated(link) => {
                let padding = " ".repeat(label_width.saturating_sub(measure_text_width(&label)));
                let offset = format_start_offset(link.start_offset_ms);

                Style::new()
                    .for_stderr()
                    .blue()
                    .apply_to(format!(" {label}{padding}  {offset}"))
                    .to_string()
            }

            PvMatch::Ready(_) => Style::new()
                .for_stderr()
                .color256(208)
                .apply_to(format!(" {label}"))
                .to_string(),

            PvMatch::Unavailable => format!("  {label}"),

            PvMatch::Ambiguous(_) => Style::new()
                .for_stderr()
                .red()
                .apply_to(format!(" {label}"))
                .to_string(),
        }
    }
}

/// Parses offset input as milliseconds, or as timecode if it contains `:`.
///
/// # Errors
///
/// Returns an error if a millisecond value isn't a whole number, or for
/// timecode, if the frame rate can't be probed (see [`probe_frame_rate`])
/// or the timecode is invalid (see [`timecode_to_milliseconds`]).
fn parse_start_offset(input: &str, video_path: &Path) -> anyhow::Result<i64> {
    let input = input.trim();

    if !input.contains(':') {
        return input
            .parse::<i64>()
            .with_context(|| format!("Invalid millisecond value: {input}"));
    }

    let frame_rate = probe_frame_rate(video_path)?;
    timecode_to_milliseconds(input, frame_rate)
}

/// The average frame rate of the video's first video stream, via `ffprobe`.
///
/// # Errors
///
/// Returns an error if `ffprobe` can't be run (e.g. not installed) or
/// fails, or its output isn't a positive `num/den` rate.
fn probe_frame_rate(video_path: &Path) -> anyhow::Result<f64> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=avg_frame_rate",
            "-of",
            "default=nw=1:nk=1",
        ])
        .arg(video_path)
        .output()
        .with_context(|| format!("Could not run ffprobe for {}", video_path.display()))?;

    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);

        bail!(
            "ffprobe failed for {}: {}",
            video_path.display(),
            error.trim()
        );
    }

    let frame_rate = std::str::from_utf8(&output.stdout)?.trim();

    let (numerator, denominator) = frame_rate
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("Unexpected ffprobe frame rate: {frame_rate}"))?;

    let numerator: f64 = numerator.parse()?;
    let denominator: f64 = denominator.parse()?;

    if numerator <= 0.0 || denominator <= 0.0 {
        bail!("Invalid frame rate: {frame_rate}");
    }

    Ok(numerator / denominator)
}

/// Converts `ss:ff`, `mm:ss:ff` or `hh:mm:ss:ff` timecode (optionally
/// negative) to milliseconds.
///
/// Frames are counted at the rounded ("nominal") rate, e.g. 30 for
/// 29.97 fps, then converted using the real rate. That matches
/// non-drop-frame timecode; drop-frame timecode will be slightly off.
///
/// # Errors
///
/// Returns an error if a field isn't a number, there are too few or too
/// many fields, seconds or minutes are 60 or more where a larger unit
/// follows, or the frame number is too high for the rate.
fn timecode_to_milliseconds(input: &str, frame_rate: f64) -> anyhow::Result<i64> {
    let (negative, input) = match input.strip_prefix('-') {
        Some(input) => (true, input),
        None => (false, input),
    };

    let fields: Vec<u64> = input
        .split(':')
        .map(|field| {
            field
                .parse::<u64>()
                .with_context(|| format!("Invalid timecode: {input}"))
        })
        .collect::<anyhow::Result<_>>()?;

    let (hours, minutes, seconds, frames) = match fields.as_slice() {
        [seconds, frames] => (0, 0, *seconds, *frames),
        [minutes, seconds, frames] => (0, *minutes, *seconds, *frames),
        [hours, minutes, seconds, frames] => (*hours, *minutes, *seconds, *frames),

        _ => bail!(
            "Use milliseconds or timecode formatted as \
            ss:ff, mm:ss:ff, or hh:mm:ss:ff"
        ),
    };

    if seconds >= 60 && fields.len() >= 3 {
        bail!("Timecode seconds must be less than 60");
    }

    if minutes >= 60 && fields.len() >= 4 {
        bail!("Timecode minutes must be less than 60");
    }

    let nominal_frame_rate = frame_rate.round() as u64;

    if frames >= nominal_frame_rate {
        bail!(
            "Frame number {frames} is invalid for approximately \
            {nominal_frame_rate} fps"
        );
    }

    let whole_seconds = hours * 3600 + minutes * 60 + seconds;
    let total_frames = whole_seconds * nominal_frame_rate + frames;

    let milliseconds = ((total_frames as f64 / frame_rate) * 1000.0).round() as i64;

    Ok(if negative {
        -milliseconds
    } else {
        milliseconds
    })
}

/// Formats an offset for the song picker, e.g. `in: 0420ms` or `in:-0350ms`.
fn format_start_offset(milliseconds: i64) -> String {
    let sign = if milliseconds < 0 { '-' } else { ' ' };
    let magnitude = milliseconds.unsigned_abs();
    format!("in:{sign}{magnitude:04}ms")
}

/// A title reduced for matching: lowercase letters and digits only.
fn match_key(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|character| character.is_alphanumeric())
        .collect()
}

/// A video's title from its file stem, without a leading track number
/// (`03 Song` → `Song`).
fn title_from_pv_stem(stem: &str) -> &str {
    match stem.split_once(' ') {
        Some((prefix, title))
            if !prefix.is_empty() && prefix.chars().all(|character| character.is_ascii_digit()) =>
        {
            title
        }
        _ => stem,
    }
}

/// Collects the `.mp4` files in `artist`'s `[pv]` folder, keyed by title.
/// An artist with no `[pv]` folder has no candidates.
///
/// # Errors
///
/// Returns an error if the `[pv]` folder or an entry in it can't be read.
fn load_pv_candidates(artist: &Path) -> anyhow::Result<PvCandidates> {
    let artist_name = artist
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Artist path has no directory name"))?;

    let pv_directory = artist.join(format!("{} [pv]", artist_name.to_string_lossy()));

    if !pv_directory.is_dir() {
        return Ok(HashMap::new());
    }

    let mut candidates: PvCandidates = HashMap::new();

    for entry in fs::read_dir(pv_directory)? {
        let path = entry?.path();

        if !path.is_file()
            || !path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("mp4"))
            || path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with('.'))
        {
            continue;
        }

        let Some(stem) = path.file_stem() else {
            continue;
        };

        let stem = stem.to_string_lossy();
        let title = title_from_pv_stem(stem.as_ref());
        let key = match_key(title);

        if !key.is_empty() {
            candidates.entry(key).or_default().push(path);
        }
    }

    Ok(candidates)
}

/// Decides a song's [`PvMatch`]: existing stickers win; otherwise its
/// title is looked up among the candidates.
fn match_pv(
    song_uri: &str,
    title: &str,
    stickers: &[String],
    pv_candidates: &PvCandidates,
) -> PvMatch {
    if let Some(link) = PvLink::from_stickers(song_uri, stickers) {
        return PvMatch::Associated(link);
    }

    let Some(candidates) = pv_candidates.get(&match_key(title)) else {
        return PvMatch::Unavailable;
    };

    match candidates.as_slice() {
        [] => PvMatch::Unavailable,
        [candidate] => PvMatch::Ready(candidate.clone()),
        candidates => PvMatch::Ambiguous(candidates.to_vec()),
    }
}

/// Runs the Artist → Album → Song pickers, annotating songs until the
/// song picker is dismissed.
///
/// # Errors
///
/// Returns an error if a folder can't be read, MPD is unreachable or
/// rejects a command, a prompt fails (e.g. no terminal), or writing
/// stickers fails.
fn main() -> anyhow::Result<()> {
    let root = Path::new(MUSIC_ROOT);
    let artist = choose_directory(root, "Artist")?;
    let pv_candidates = load_pv_candidates(&artist)?;

    let mut client = MpdClient::connect(MPD_ADDRESS)?;
    let album = choose_album(&artist, &mut client, &pv_candidates)?;

    loop {
        let song = choose_song(&album, &mut client, &pv_candidates)?;
        let Some(song) = song else {
            break;
        };
        annotate_song(&song)?;
    }

    Ok(())
}

/// Fuzzy-picks one of `parent`'s visible subfolders, sorted by name.
///
/// # Errors
///
/// Returns an error if `parent` can't be read or the prompt fails.
fn choose_directory(parent: &Path, prompt: &str) -> anyhow::Result<PathBuf> {
    let mut directories: Vec<PathBuf> = fs::read_dir(parent)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && !path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with('.'))
        })
        .collect();

    directories.sort();

    let names: Vec<String> = directories
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();

    let selection = FuzzySelect::new()
        .with_prompt(prompt)
        .items(&names)
        .interact()?;

    Ok(directories[selection].clone())
}

/// Fuzzy-picks one of `artist`'s albums, colored by [`AlbumPvStatus`].
///
/// Albums are sorted by the album sort tag (`TSOA`/`TSA`) of their first
/// readable `.mp3`, falling back to the folder name. Coloring loads every
/// album's songs and stickers up front.
///
/// # Errors
///
/// Returns an error if a folder can't be read, any album's songs fail to
/// load (see [`load_song_choices`]), or the prompt fails.
fn choose_album(
    artist: &Path,
    mpd_client: &mut MpdClient,
    pv_candidates: &PvCandidates,
) -> anyhow::Result<PathBuf> {
    let mut pv_directory_name = artist.file_name().unwrap_or_default().to_os_string();
    pv_directory_name.push(" [pv]");

    let mut albums: Vec<PathBuf> = fs::read_dir(artist)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && path.file_name() != Some(pv_directory_name.as_os_str())
                && !path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with('.'))
        })
        .collect();

    albums.sort_by_cached_key(|album| {
        let sort_album = fs::read_dir(album)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find_map(|path| {
                if !path.is_file()
                    || !path
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("mp3"))
                    // `._` files are macOS metadata twins on non-Apple volumes.
                    || path
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with("._"))
                {
                    return None;
                }

                let tag = match Tag::read_from_path(&path) {
                    Ok(tag) => tag,
                    Err(error) => {
                        eprintln!("could not read {}: {error}", path.display());
                        return None;
                    }
                };

                tag.get("TSOA")
                    .or_else(|| tag.get("TSA"))
                    .and_then(|frame| frame.content().text())
                    .map(str::to_owned)
            });
        sort_album
            .unwrap_or_else(|| {
                album
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            })
            .to_lowercase()
    });

    let names: Vec<String> = albums
        .iter()
        .map(|album| -> anyhow::Result<String> {
            let name = album
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();

            let songs = load_song_choices(album, mpd_client, pv_candidates)?;

            let display_name = match album_pv_status(&songs) {
                AlbumPvStatus::Ready => Style::new()
                    .for_stderr()
                    .color256(208)
                    .apply_to(name)
                    .to_string(),

                AlbumPvStatus::Associated => {
                    Style::new().for_stderr().blue().apply_to(name).to_string()
                }

                AlbumPvStatus::Unavailable => name,
            };

            Ok(display_name)
        })
        .collect::<anyhow::Result<_>>()?;

    let selection = FuzzySelect::new()
        .with_prompt("Album")
        .items(&names)
        .highlight_matches(false)
        .interact()?;

    Ok(albums[selection].clone())
}

/// An album's color: orange beats blue beats plain.
fn album_pv_status(songs: &[SongChoice]) -> AlbumPvStatus {
    if songs
        .iter()
        .any(|song| matches!(&song.pv_match, PvMatch::Ready(_)))
    {
        AlbumPvStatus::Ready
    } else if songs
        .iter()
        .any(|song| matches!(&song.pv_match, PvMatch::Associated(_)))
    {
        AlbumPvStatus::Associated
    } else {
        AlbumPvStatus::Unavailable
    }
}

/// The album's `.mp3` files, sorted by file name, each with its title,
/// track number, MPD URI and [`PvMatch`].
///
/// # Errors
///
/// Returns an error if the album can't be read, a file is outside
/// [`MUSIC_ROOT`], or MPD's sticker lookup fails (e.g. the file isn't in
/// MPD's database).
fn load_song_choices(
    album: &Path,
    client: &mut MpdClient,
    pv_candidates: &PvCandidates,
) -> anyhow::Result<Vec<SongChoice>> {
    let mut songs: Vec<PathBuf> = fs::read_dir(album)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && !path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with('.'))
                && path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("mp3"))
        })
        .collect();

    songs.sort();

    songs
        .into_iter()
        .map(|audio_path| -> anyhow::Result<SongChoice> {
            let tag = Tag::read_from_path(&audio_path).ok();

            let title = tag
                .as_ref()
                .and_then(TagLike::title)
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    audio_path
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                });

            let track = tag.as_ref().and_then(TagLike::track);

            let uri = audio_path
                .strip_prefix(Path::new(MUSIC_ROOT))?
                .to_string_lossy()
                .into_owned();

            let stickers = client.get_stickers(&uri)?;
            let pv_match = match_pv(&uri, &title, &stickers, pv_candidates);

            Ok(SongChoice {
                audio_path,
                uri,
                title,
                track,
                pv_match,
            })
        })
        .collect()
}

/// Fuzzy-picks a song from `album`. Returns `None` when the picker is
/// dismissed (Esc), which ends the session.
///
/// # Errors
///
/// Returns an error if the songs can't be loaded or the prompt fails.
fn choose_song(
    album: &Path,
    client: &mut MpdClient,
    pv_candidates: &PvCandidates,
) -> anyhow::Result<Option<SongChoice>> {
    let mut choices = load_song_choices(album, client, pv_candidates)?;

    let label_width = choices
        .iter()
        .map(|choice| measure_text_width(&choice.label()))
        .max()
        .unwrap_or(0);

    let names: Vec<String> = choices
        .iter()
        .map(|choice| choice.display_name(label_width))
        .collect();

    let selection = FuzzySelect::new()
        .with_prompt("Song")
        .items(&names)
        .highlight_matches(false)
        .report(false)
        .interact_opt()?;

    let Some(selection) = selection else {
        return Ok(None);
    };

    Ok(Some(choices.remove(selection)))
}

/// Acts on a picked song: links it if [`PvMatch::Ready`], otherwise just
/// reports its status.
///
/// # Errors
///
/// Returns an error if linking fails (see [`annotate_ready_song`]).
fn annotate_song(song: &SongChoice) -> anyhow::Result<()> {
    match &song.pv_match {
        PvMatch::Associated(link) => {
            println!(
                "Already associated with {} at {} ms",
                link.path.display(),
                link.start_offset_ms
            );
        }

        PvMatch::Ready(video_path) => {
            annotate_ready_song(song, video_path)?;
        }

        PvMatch::Unavailable => {
            println!("No matching PV was found for {}", song.audio_path.display());
        }

        PvMatch::Ambiguous(candidates) => {
            println!(
                "Found multiple possible PVs for {}:",
                song.audio_path.display()
            );

            for candidate in candidates {
                println!("  {}", candidate.display());
            }

            println!("No changes made.");
        }
    }

    Ok(())
}

/// Prompts for the song's offset, shows the stickers to be written, and
/// writes `pv` and `pv_in_ms` after confirmation (default: no).
///
/// # Errors
///
/// Returns an error if the video's file name isn't valid UTF-8, the offset
/// can't be parsed, a prompt fails, or MPD can't be reached or rejects the
/// writes.
fn annotate_ready_song(song: &SongChoice, video_path: &Path) -> anyhow::Result<()> {
    let pv_filename = video_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("PV filename is not valid UTF-8"))?;

    println!("Song: {}", song.audio_path.display());
    println!("PV:   {}", video_path.display());

    let start_offset_input: String = Input::new()
        .with_prompt("Song start: ms or [hh:]mm:ss:ff")
        .interact_text()?;

    let start_offset_ms = parse_start_offset(&start_offset_input, video_path)?;

    println!();
    println!("MPD song: {}", song.uri);
    println!("pv:       {pv_filename}");
    println!("pv_in_ms: {start_offset_ms}");

    let confirmed = Confirm::new()
        .with_prompt("Write these stickers?")
        .default(false)
        .interact()?;

    if !confirmed {
        println!("No changes made.");
        return Ok(());
    }

    // Fresh connection: MPD may have dropped the old one (60 s idle
    // timeout) while the prompts were open.
    let mut client =
        MpdClient::connect(MPD_ADDRESS).context("Couldn't connect to MPD to write stickers")?;

    client.set_sticker(&song.uri, "pv", pv_filename)?;
    client.set_sticker(&song.uri, "pv_in_ms", &start_offset_ms.to_string())?;

    println!("PV association saved.");

    Ok(())
}
