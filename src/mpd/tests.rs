use crate::mpd::song::Song;
use crate::mpd::status::Status;

#[test]
fn parses_song_details() {
    let lines = vec![
        String::from("file: music/example.mp3\n"),
        String::from("Title: Example Song\n"),
        String::from("Id: 2\n"),
    ];

    let song = Song::parse(&lines)
        .expect("valid song details should parse");

    assert_eq!(song.file, "music/example.mp3");
    assert_eq!(song.title.as_deref(), Some("Example Song"));
}

#[test]
fn parses_status_fields() {
    let lines = vec![
        String::from("volume: 100\n"),
        String::from("audio: 44100:16:2\n"),
        String::from("nextsongid: 2\n"),
    ];

    let status = Status::parse(&lines).expect("valid status should parse");

    assert_eq!(status.sample_rate_hz, Some(44100));
    assert_eq!(status.next_song_id, Some(2));
}

#[test]
fn missing_fields_remain_none() {
    let lines = vec![String::from("state: stop\n")];

    let status = Status::parse(&lines).expect("missing fields are allowed");

    assert_eq!(status.sample_rate_hz, None);
    assert_eq!(status.next_song_id, None);
}

#[test]
fn rejects_invalid_sample_rate() {
    let lines = vec![String::from("audio: nope:16:2\n")];

    assert!(Status::parse(&lines).is_err());
}

#[test]
fn rejects_invalid_next_song_id() {
    let lines = vec![String::from("nextsongid: nope\n")];

    assert!(Status::parse(&lines).is_err());
}

#[test]
fn song_without_title_is_valid() {
    let lines = vec![
        String::from("file: music/untitled.mp3\n"),
    ];

    let song = Song::parse(&lines)
        .expect("a song without a title should parse");

    assert_eq!(song.file, "music/untitled.mp3");
    assert_eq!(song.title, None);
}

#[test]
fn song_without_file_is_rejected() {
    let lines = vec![
        String::from("Title: Example Song\n"),
    ];

    assert!(Song::parse(&lines).is_err());
}