use std::fs;
use std::path::{Path, PathBuf};
use dialoguer::FuzzySelect;
use id3::{Tag, TagLike};

const MUSIC_ROOT: &str = "/Volumes/Elm_SSD/0/iTunes Media/Music/";

fn main() -> anyhow::Result<()> {
    let root = Path::new(MUSIC_ROOT);
    let artist = choose_directory(root, "Artist")?;
    let album = choose_directory(&artist, "Album")?;
    let song = choose_song(&album)?;

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

fn choose_song(album: &Path) -> anyhow::Result<PathBuf> {
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
        .map(|path| {
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

            match track {
                Some(number) => format!("{number:02}- {title}"),
                None => title,
            }
        })
        .collect();

    let selection = FuzzySelect::new()
        .with_prompt("Song")
        .items(&names)
        .interact()?;

    Ok(songs[selection].clone())
}