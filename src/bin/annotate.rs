use anyhow::{bail, Context};
use id3::{Tag, TagLike};
use pvplay::{mpd::MpdClient, pv_link::PvLink};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use dialoguer::{
    Confirm, FuzzySelect, Input,
    console::{Style, measure_text_width}
};

const MUSIC_ROOT: &str = "/Volumes/Elm_SSD/0/iTunes Media/Music/";
const MPD_ADDRESS: &str = "localhost:6600";

type PvCandidates = HashMap<String, Vec<PathBuf>>;

#[derive(Debug)]
enum PvMatch {
    Associated(PvLink),
    Ready(PathBuf),
    Unavailable,
    Ambiguous(Vec<PathBuf>),
}

#[derive(Debug)]
struct SongChoice {
    audio_path: PathBuf,
    uri: String,
    title: String,
    track: Option<u32>,
    pv_match: PvMatch,
}

impl SongChoice {
    fn label(&self) -> String {
        match self.track {
            Some(number) => format!("{number:02}- {}", self.title),
            None => self.title.clone(),
        }
    }

    fn display_name(&self, label_width: usize) -> String {
        let label = self.label();

        match &self.pv_match {
            PvMatch::Associated(link) => {
                let padding =
                    " ".repeat(label_width.saturating_sub(measure_text_width(&label)));
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
        .with_context(|| {
            format!("Could not run ffprobe for {}", video_path.display())
        })?;

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
        .ok_or_else(|| anyhow::anyhow!(
            "Unexpected ffprobe frame rate: {frame_rate}"
        ))?;

    let numerator: f64 = numerator.parse()?;
    let denominator: f64 = denominator.parse()?;

    if numerator <= 0.0 || denominator <= 0.0 {
        bail!("Invalid frame rate: {frame_rate}");
    }

    Ok(numerator / denominator)
}

fn timecode_to_milliseconds(
    input: &str, frame_rate: f64
) -> anyhow::Result<i64> {
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
        [hours, minutes, seconds, frames] => {
            (*hours, *minutes, *seconds, *frames)
        }

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

    let milliseconds =
        ((total_frames as f64 / frame_rate) * 1000.0).round() as i64;

    Ok(if negative { -milliseconds } else { milliseconds })
}

fn format_start_offset(milliseconds: i64) -> String {
    let sign = if milliseconds < 0 { '-' } else { ' ' };
    let magnitude = milliseconds.unsigned_abs();
    format!("in:{sign}{magnitude:04}ms")
}

fn match_key(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|character| character.is_alphanumeric())
        .collect()
}

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

fn main() -> anyhow::Result<()> {
    let root = Path::new(MUSIC_ROOT);
    let artist = choose_directory(root, "Artist")?;
    let pv_candidates = load_pv_candidates(&artist)?;
    let album = choose_album(&artist)?;

    loop {
        let song = {
            let mut client = MpdClient::connect(MPD_ADDRESS)?;
            choose_song(&album, &mut client, &pv_candidates)?
        };

        let Some(song) = song else {
            break;
        };

        annotate_song(&song)?;
    }
    Ok(())
}

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

fn choose_album(artist: &Path) -> anyhow::Result<PathBuf> {
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
        .map(|path| {
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        })
        .collect();

    let selection = FuzzySelect::new()
        .with_prompt("Album")
        .items(&names)
        .interact()?;

    Ok(albums[selection].clone())
}

fn choose_song(
    album: &Path,
    client: &mut MpdClient,
    pv_candidates: &PvCandidates,
) -> anyhow::Result<Option<SongChoice>> {
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

    let mut choices: Vec<SongChoice> = songs
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
        .collect::<anyhow::Result<_>>()?;

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

fn annotate_ready_song(
    song: &SongChoice,
    video_path: &Path
) -> anyhow::Result<()> {
    let pv_filename = video_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("PV filename is not valid UTF-8"))?;

    println!("Song: {}", song.audio_path.display());
    println!("PV:   {}", video_path.display());

    let start_offset_input: String = Input::new()
        .with_prompt("Song start: ms or [hh:]mm:ss:ff")
        .interact_text()?;

    let start_offset_ms =
        parse_start_offset(&start_offset_input, video_path)?;

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

    let mut client = MpdClient::connect(MPD_ADDRESS)
        .context("Couldn't connect to MPD to write stickers")?;

    client.set_sticker(&song.uri, "pv", pv_filename)?;
    client.set_sticker(&song.uri, "pv_in_ms", &start_offset_ms.to_string())?;

    println!("PV association saved.");

    Ok(())
}
