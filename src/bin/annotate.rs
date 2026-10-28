use std::fs;
use std::path::{Path, PathBuf};
use dialoguer::{console::Style, FuzzySelect};
use id3::{Tag, TagLike};
use pvplay::mpd::MpdClient;

const MUSIC_ROOT: &str = "/Volumes/Elm_SSD/0/iTunes Media/Music/";

fn main() -> anyhow::Result<()> {
    let root = Path::new(MUSIC_ROOT);
    let artist = choose_directory(root, "Artist")?;
    let album = choose_album(&artist)?;
    let mut client = MpdClient::connect("localhost:6600")?;
    let song = choose_song(&album, &mut client)?;

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

fn choose_song(album: &Path, client: &mut MpdClient) -> anyhow::Result<PathBuf> {
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

            let track = tag
                .as_ref()
                .and_then(|tag| tag.track());

            let display = match track {
                Some(number) => format!("{number:02}- {title}"),
                None => title,
            };

            let uri = path
                .strip_prefix(Path::new(MUSIC_ROOT))?
                .to_string_lossy();

            let has_stickers = !client.get_stickers(&uri)?.is_empty();

            let display = if has_stickers {
                Style::new()
                    .for_stderr()
                    .blue()
                    .apply_to(format!(" {display}"))
                    .to_string()
            } else {
                format!("  {display}")
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