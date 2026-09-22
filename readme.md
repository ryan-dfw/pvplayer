# pvplayer

> **Work in progress.** pvplayer is under active development and is not yet ready for general use.

pvplayer is a Rust application that plays music videos alongside music managed by [MPD](https://www.musicpd.org/).

MPD remains responsible for audio playback. pvplayer observes the currently playing song, looks for an associated music video in the song's MPD stickers, and opens that video in mpv with its audio disabled. The goal is to keep the video synchronized with MPD so that music can continue to be managed normally while its corresponding PV plays automatically.

The name comes from **PV** ("promotional video"), sounds better to me than mvplayer. If this goes anywhere someone better w/ names than me can give me a better one.

## How it works

The intended flow is:

1. MPD begins playing a song.
2. pvplayer detects the current song and reads its associated PV metadata.
3. pvplayer resolves and launches the video in mpv.
4. mpv plays video only; MPD remains the audio source.
5. pvplayer synchronizes the video's position with MPD playback.

This separation is intentional: pvplayer is a companion to MPD rather than a replacement for it.

## Current status

The basic MPD → pvplayer → mpv path is working. pvplayer can identify the current song, resolve its associated video, and launch it in mpv.

Playback synchronization is currently under development. Timing behavior can vary between machines, so synchronization may require accounting for machine-specific playback latency.

## Goals

- Automatically play the PV associated with the current MPD song.
- Keep video playback synchronized with MPD without replacing MPD as the audio source.
- Build something I personally want to use.
- Learn Rust through a practical systems project.
- Make the project usable by others if development reaches that point.

## Development

This is primarily a personal-use and learning project. Interfaces, configuration, metadata conventions, and implementation details may change while it is under development.

Setup and usage documentation will be added once the application is stable enough for general use.

## AI assistance

This readme was written by chatGPT except for this note. If the project seems viable i'll replace everything with proper docs; for now just looking to save progress while i work.
