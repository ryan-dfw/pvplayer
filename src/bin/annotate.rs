use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use dialoguer::{console::Style, FuzzySelect};
use id3::{Tag, TagLike};
use pvplay::{mpd::MpdClient, pv_link::PvLink};

const MUSIC_ROOT: &str = "/Volumes/Elm_SSD/0/iTunes Media/Music/";

type PvCandidates = HashMap<String, Vec<PathBuf>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PvStatus {
    Associated,
    Ready,
    Unavailable,
    Ambiguous
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
        if !prefix.is_empty()
            && prefix
            .chars()
            .all(|character| character.is_ascii_digit()) =>
            { title }
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

        let Some(stem) = path.file_stem() else { continue; };

        let stem = stem.to_string_lossy();
        let title = title_from_pv_stem(stem.as_ref());
        let key = match_key(title);

        if !key.is_empty() { candidates.entry(key).or_default().push(path); }
    }

    Ok(candidates)
}

fn classify_pv(
    song_uri: &str,
    title: &str,
    stickers: &[String],
    pv_candidates: &PvCandidates,
) -> PvStatus {
    if PvLink::from_stickers(song_uri, stickers).is_some() {
        return PvStatus::Associated;
    }

    let candidate_count = pv_candidates
        .get(&match_key(title))
        .map_or(0, Vec::len);

    match candidate_count {
        0 => PvStatus::Unavailable,
        1 => PvStatus::Ready,
        _ => PvStatus::Ambiguous,
    }
}


fn main() -> anyhow::Result<()> {
    let root = Path::new(MUSIC_ROOT);
    let artist = choose_directory(root, "Artist")?;
    let pv_candidates = load_pv_candidates(&artist)?;
    let album = choose_album(&artist)?;
    let mut client = MpdClient::connect("localhost:6600")?;
    let song = choose_song(&album, &mut client, &pv_candidates)?;

    println!("Selected: {}", song.display());

    Ok(())
}

fn choose_directory(parent: &Path, prompt: &str) -> anyhow::Result<PathBuf> {
    let mut directories: Vec<PathBuf> = fs::read_dir(parent)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && !path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with('.'))
        })
        .collect();

    directories.sort();

    let names: Vec<String> = directories
        .iter()
        .map( |path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();

    let selection = FuzzySelect::new()
        .with_prompt(prompt)
        .items(&names)
        .interact()?;

    Ok(directories[selection].clone())
}

fn choose_album(artist: &Path) -> anyhow::Result<PathBuf> {
    let mut pv_directory_name = artist
        .file_name()
        .unwrap_or_default()
        .to_os_string();
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
                    || !path.extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("mp3"))
                    || path.file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with("._"))
                { return None; }

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
        sort_album.unwrap_or_else(|| {
            album.file_name()
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
    pv_candidates: &PvCandidates
) -> anyhow::Result<PathBuf> {
    let mut songs: Vec<PathBuf> = fs::read_dir(album)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && !path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("."))
                && path.extension()
                .is_some_and(|ext|ext.eq_ignore_ascii_case("mp3"))
        })
        .collect();

    songs.sort();

    let names: Vec<String> = songs
        .iter()
        .map(|path| -> anyhow::Result<String> {
            let tag = Tag::read_from_path(path).ok();

            let title = tag
                .as_ref()
                .and_then(|tag| tag.title())
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    path.file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                });

            let track = tag.as_ref().and_then(|tag| tag.track());

            let uri = path
                .strip_prefix(Path::new(MUSIC_ROOT))?
                .to_string_lossy();

            let stickers = client.get_stickers(&uri)?;
            let pv_status = classify_pv(uri.as_ref(), &title, &stickers, pv_candidates);

            let display = match track {
                Some(number ) => format!("{number:02}- {title}"),
                None=> title,
            };

            let display = match pv_status {
                PvStatus::Associated => Style::new()
                    .for_stderr()
                    .blue()
                    .apply_to(format!(" {display}"))
                    .to_string(),

                PvStatus::Ready => Style::new()
                    .for_stderr()
                    .color256(208)
                    .apply_to(format!(" {display}"))
                    .to_string(),

                PvStatus::Unavailable => format!("  {display}"),

                PvStatus::Ambiguous => Style::new()
                    .for_stderr()
                    .red()
                    .apply_to(format!(" {display}"))
                    .to_string(),
            };

            Ok(display)
        })
        .collect::<anyhow::Result<_>>()?;

    let selection = FuzzySelect::new()
        .with_prompt("Song")
        .items(&names)
        .highlight_matches(false)
        .interact()?;

    Ok(songs[selection].clone())
}