# Architecture

ratune is a Cargo workspace with four crates:

| Crate | Role |
|-------|------|
| `ratune-subsonic` | Subsonic API client — authentication, endpoints, models |
| `ratune-scrobble` | Last.fm / Libre.fm Audioscrobbler client, auth helpers, listen thresholds |
| `ratune-player` | Audio engine — rodio-based playback on a dedicated thread, gapless transitions, sample tap for FFT |
| `ratune` | Binary — TUI, event loop, state management, Kitty graphics, scrobble integration |

## Crate responsibilities

**`ratune-subsonic`** is a pure HTTP client with no TUI or audio dependencies. It handles Subsonic API authentication (MD5 token + salt per request), and exposes endpoints for browsing artists/albums/tracks, streaming URLs, search, cover art, playlist operations, and server-side scrobbling.

**`ratune-scrobble`** implements the Audioscrobbler API v2.0 (Last.fm and Libre.fm): signed POST requests, browser auth (`auth.getToken` / `auth.getSession`), `track.updateNowPlaying`, and `track.scrobble`. Shared threshold logic distinguishes Ratune local listens from Last.fm scrobble rules.

**`ratune-player`** runs the audio engine on its own `std::thread` (not tokio). It communicates with the TUI through two channels:

- `PlayerCommand` (TUI → player): PlayUrl, EnqueueNext, Pause, Resume, Stop, SetVolume, Seek, Quit
- `PlayerEvent` (player → TUI): TrackStarted, Progress, AboutToFinish, TrackAdvanced, TrackEnded, Error

Progress events fire on a ~500ms tick. Gapless playback is handled via `EnqueueNext` — the TUI sends the next track's URL when it receives `AboutToFinish` (~10 seconds before the current track ends), and rodio's `Sink::append()` handles the seamless transition.

A `SampleTap` wrapper copies decoded samples into a shared ring buffer for FFT analysis by the visualizer.

**`ratune`** (binary) owns the `App` struct, which holds all application state. The event loop runs on tokio and uses `select!` to race crossterm key/mouse events, player events, library update channels, and timer ticks.

Scrobbling hooks into `PlayerEvent::TrackStarted` (now playing) and progress ticks (listen thresholds). Failed Audioscrobbler submissions are persisted under `~/.local/share/ratune/scrobble-queue.json` and retried on startup via background tokio tasks, using the same `LibraryUpdate` channel as other network work.

## Key patterns

- **`Action` enum** — every user intent (navigation, playback, queue manipulation) is expressed as an `Action` variant, mapped from key events in the input layer and dispatched through `App::dispatch()`.
- **`LoadingState<T>`** — `NotLoaded | Loading | Loaded(T) | Error(String)`. Used throughout for async data fetches (albums, tracks, playlists) to keep the UI responsive during network calls.
- **Skip cancellation** — `PlayerCommand::PlayUrl` carries a generation counter. Rapid skips drain the command channel and only fetch the last requested track.
- **Kitty graphics** — album art is rendered outside ratatui's layout system using absolute cursor positioning and Kitty escape sequences. Inside tmux, Unicode placeholder mode (`U=1`) avoids pane clipping artifacts.
