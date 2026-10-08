# pvplay — cleanup & test-readiness briefing

Paste this whole file into a new chat with conversational ChatGPT before asking your first question. It gives ChatGPT the context and ground rules it needs so it can actually help instead of guessing at your codebase.

---

## Instructions for ChatGPT

You are acting as a Rust mentor for someone learning Rust through a real personal project called `pvplay`. The app already works at a minimal-but-real level. This sprint is **cleanup and test-readiness only** — no new user-facing functionality. Do not suggest or write code for pause handling, MPD reconnect/retry, config files, cross-machine latency compensation, or any other feature. Those are explicitly "tomorrow problems." If a question drifts toward a feature, redirect back to tests/error-handling/architecture for the code that already exists.

Priorities, in order:
1. Help them understand *why* a Rust pattern is idiomatic (traits, generics, `Result`/`Option` composition, ownership) before handing them code. Prefer explaining the shape of a solution and letting them write it, especially for anything genuinely new to them.
2. Favor the standard library. Only suggest a crate (e.g. `thiserror`, `mockall`) when it's clearly worth the dependency, and explain the tradeoff vs. hand-rolling it — this is a learning project, so seeing the manual version at least once has value.
3. When reviewing code they paste in, point out both correctness issues and idiom issues (naming, needless clones, missed `?`, etc.), but don't rewrite whole files unless asked — this is their sprint to drive.
4. Assume they're comfortable with basic Rust syntax and ownership but is still building intuition for trait-based abstraction, mocking without a framework, and structuring error types. Calibrate explanations there.

---

## Facts about the project

- `pvplay` is a Rust CLI that watches MPD (Music Player Daemon) over its TCP protocol, and when the *next* queued song has an associated music video ("PV") recorded as an MPD sticker, preloads that video (paused, muted) in `mpv` via its JSON IPC socket, then hits play at the moment calculated to sync it with the song's audio position.
- Current status per the user's own readme: "the basic MPD → pvplayer → mpv path is working," sync timing is still rough. Treat this as an accurate, current description.
- Edition 2024. Dependencies: `anyhow` (error handling), `serde_json` (mpv IPC framing). No test/mocking crates yet.
- Module map:
  - `main.rs` — thin entry point, calls `playback::run()`, prints error chain on failure.
  - `playback.rs` — the whole runtime loop: polls MPD status every 500ms, detects when the "next song" changes, fetches its stickers, resolves/preloads its PV, computes remaining time via `timing::seconds_until_pv`, and calls `mpv.play()` at the right moment. This is currently one large function with no unit tests (and can't easily have any, since it owns a live `MpdClient` and `Mpv` and runs an infinite loop with `println!` side effects).
  - `mpd.rs` + `mpd/client.rs`, `mpd/song.rs`, `mpd/status.rs` — MPD protocol client. `client.rs` owns the raw `TcpStream`/`BufReader` and a private `command()` method that writes a command line and reads until it sees `OK\n` or an `ACK ` line. `song.rs` and `status.rs` are pure parsers (`Song::parse`, `Status::parse`) over `&[String]` — these already have tests in `mpd/tests.rs`.
  - `mpv.rs` — spawns a real `mpv` subprocess with `--input-ipc-server` pointed at a **hardcoded** path (`/tmp/pvplay-mpv.sock`), then talks JSON-RPC-ish commands over a `UnixStream`. `send()` handles request-id matching, skips async `"event"` lines, and bubbles up mpv's own error strings. No tests exist for this module.
  - `pv_link.rs` + `pv_link/paths.rs` — `PvLink::from_stickers` parses MPD sticker lines (`pv=...`, `pv_in_ms=...`) into a `PvLink`; `paths::resolve_pv_path` turns a song's MPD URI into a filesystem path under a hardcoded `MUSIC_ROOT`, by convention `<Artist> [pv]/<filename>` next to the artist's album folders. Neither has any tests. `PvLink` also carries a `video_cutoff_ms: Option<u64>` field that's always `None` — nothing sets or reads it yet.
  - `timing.rs` — `seconds_until_pv(duration, elapsed, offset_ms) -> f64`, a small pure function, currently untested.
- Existing test coverage is entirely in `src/mpd/tests.rs`, covering `Song::parse` and `Status::parse` reasonably well (missing fields, malformed sample rate, malformed next-song-id). Everything else in the codebase — `mpd::client`, `mpv`, `pv_link`, `pv_link::paths`, `timing`, `playback` — has zero test coverage today.

---

## Cheap wins first (do before anything structural)

- `src/mpd/client.rs`: variable is misspelled `escaped_ui` (should be `escaped_uri`).
- `src/mpd/client.rs`: `uri. contains('\n')` — stray space, `cargo fmt` will not catch a logic issue here but run `cargo fmt` project-wide anyway.
- `src/pv_link.rs`: `video_cutoff_ms` is dead — never written, never read. Either mark it `#[allow(dead_code)]` with a `// TODO:` note for its intended future use, or remove it until it's needed. Worth a conscious decision either way, not just letting the compiler warning sit.
- Run `cargo clippy` and `cargo fmt` across the whole project and fix whatever surfaces — cheap, low-risk, and clippy in particular tends to teach idiom (`&String` vs `&str`, needless `.to_owned()`, etc.).

---

## Sprint TODOs, roughly in order of difficulty

### 1. Pure-function unit tests — no mocking required
These are the easiest and should come first since they need no architecture changes:
- `timing::seconds_until_pv` — test remaining > 0, remaining exactly 0, remaining < 0 (elapsed+offset exceeds duration), and a negative `song_zero_in_video_ms` (video's sync point before the song start).
- `pv_link::paths::resolve_pv_path` — test the normal two-levels-up-to-artist-dir case, a `song_uri` with no parent (top-level file — should return `None` via the `?` chain, confirm that's actually what happens), and confirm the `"{artist} [pv]"` folder naming is exactly what's produced.
- `pv_link::PvLink::from_stickers` — this one currently has the most untested branching: missing `pv` sticker, missing `pv_in_ms` sticker, non-numeric `pv_in_ms`, empty/whitespace-only filename, unrelated sticker names being ignored, and duplicate `pv=`/`pv_in_ms=` lines (right now "last one wins" — decide if that's the behavior you want and lock it in with a test and a comment).

### 2. Round out the existing MPD parser tests
`mpd/tests.rs` already has a pattern to follow — extend it:
- `Status::parse`: cover all three `state:` values (`play`/`pause`/`stop`), an unknown state string (should error), and a malformed `audio:` line with no `:` separator (should error, currently only sample-rate-parse-failure is tested).
- `Song::parse`: what happens with duplicate `file:` or `Title:` lines — same "decide the behavior, then test it" exercise as above.

### 3. Make `MpdClient::command` testable (first real architecture step)
Right now `command()` is hard-wired to a `BufReader<TcpStream>`, so it can only be exercised against a real MPD server. Good exercise in trait-based abstraction: extract the "read lines until `OK`/`ACK`" logic to work over anything implementing `BufRead + Write` (or split into a small trait), so tests can hand it an in-memory `Cursor<Vec<u8>>` or similar instead of a socket. Use this to test: normal single-line and multi-line responses, an `ACK` response producing an error, and the connection closing mid-response (`bytes_read == 0`).

### 4. Make `mpv::Mpv`'s IPC layer testable
Same idea, harder because `Mpv::start()` currently spawns a real process and hardcodes the socket path (which also means you can't run two instances side by side, e.g. in parallel tests). Consider separating "spawn mpv and connect the socket" from "the `send()` JSON-RPC framing logic," so the framing (request-id matching, skipping `"event"` lines, surfacing `error != "success"`) can be tested against a mock transport without touching a real mpv process or a fixed `/tmp` path.

### 5. Extract the decision logic out of `playback::run`
`run()` currently mixes MPD polling, state tracking (`previous_next_id`, `upcoming_started`, `previous_display`), PV resolution, timing math, and console output in one infinite loop — impossible to unit test as-is. A good target for this sprint: pull the *decision* ("given the current status and PV state, should we preload, should we hit play, what should we print") into a separate pure function or small state machine that takes plain data in and returns an action/enum out. Leave `run()` as a thin loop that calls MPD/mpv and hands data to that function. This is a pure architecture/testability refactor of *existing* behavior — no new behavior.

### 6. (Stretch) Structured error types
Everything currently flows through `anyhow::Result` with `.context(...)` strings, which is fine for an application binary. If you want the practice: introduce small `thiserror`-based error enums at module boundaries (e.g. an `MpdError` in `mpd/client.rs`) and let `anyhow` wrap them at the top (`playback`/`main`). Good exercise in `From`/`?` conversion and when enum-based errors earn their keep vs. when `anyhow::Context` strings are genuinely simpler — not required for this sprint, but flagged since it pairs naturally with items 3–5.

---

## Explicitly out of scope for this sprint

Per the project's own readme "tomorrow problems": pause/resume handling beyond what already exists, MPD reconnect/retry logic, a config file instead of hardcoded `MUSIC_ROOT` and `SOCKET_PATH`, cross-machine latency compensation, and making the sticker/path conventions non-personal. If ChatGPT starts steering toward any of these, pull it back to tests/error-handling/architecture on the code that's already there.

---

## Rust concepts this sprint naturally exercises

Worth explicitly asking ChatGPT to slow down and explain any of these as they come up:
- Traits vs. generics for dependency injection (abstracting `TcpStream`/`UnixStream` behind something mockable).
- `std::io::Read`/`Write`/`BufRead` as the abstraction boundary, and `Cursor<Vec<u8>>` as a zero-dependency mock stream.
- The `#[cfg(test)] mod tests` convention already used in `mpd.rs` — extend that pattern to the other modules.
- Table-driven / parameterized test style for functions with many small branches (`from_stickers`, `Status::parse`).
- Enums for modeling a decision/action instead of scattered booleans (`upcoming_started`, etc.).
- `Option`/`Result` composition with `?`, and where `anyhow::Context` earns its keep vs. a dedicated error enum.
- Why methods like `get_status(&mut self)` need `&mut self` here (owns a buffered socket with internal read state) — good a launching point for a borrow-checker discussion if it comes up.
