# Ratune

A terminal music player for Subsonic-compatible servers.
Built in Rust with [Ratatui](https://github.com/ratatui/ratatui), featuring  album art graphics (including in tmux), gapless playback, fuzzy finder support, and a highly configurable UI.

<img src="docs/screenshots/customized_visual.png" alt="Visualizer now playing customization" width="90%" />

**Why Ratune?**

Ratune was built to bring together a combination of features often missing from Subsonic players: fuzzy navigation, a visually rich UI with album art, deep customization, and a fully terminal-based workflow. Many players excel at a few of these. Ratune aims to cover them all.

---

## Table of contents

- [Highlights](#highlights)
- [Screenshots](#screenshots)
- [Requirements](#requirements)
- [Installation](#installation)
- [Configuration](#configuration)
- [Default keybinds](#default-keybinds)
- [tmux](#tmux)
- [Project layout](#project-layout)
- [Data on disk](#data-on-disk)
- [Credits](#credits)
- [Acknowledgements](#acknowledgements)
- [License](#license)

---

## Highlights

- **Playback**: Gapless queue, seek, shuffle/unshuffle, and playlist management.
- **Album Art**: Display using Kitty graphics and [ratatui-image](https://github.com/ratatui/ratatui-image) (see link for compatible terminals)
- **Lyrics**: Synced lyrics via LRCLib when available.
- **Visualizer**: FFT spectrum analyzer.
- **Fuzzy finder**: Optional library index + external picker (fzf/skim) for fast track selection.
- **Folder navigation**: Optional Browse layout that follows server music folders for servers that provide it.
- **Customization**: Keybinds, theme, layout, now-playing lines, queue row template inspired by ncmpcpp.
- **Integration**: Linux MPRIS (media keys, `playerctl`).
- **Scrobbling**: Last.fm and Libre.fm (Audioscrobbler), plus optional Subsonic `/scrobble` for Navidrome play counts — no MPRIS guessing; Ratune owns playback.

---

## Requirements

### Runtime (prebuilt binary or any install)

These apply whenever you run Ratune, including [GitHub Releases](https://github.com/acmagn/ratune/releases) assets.

- **Server**: A Subsonic-compatible music server (Navidrome is a good default).
- **Linux audio**: ALSA userspace library at runtime — e.g. Debian/Ubuntu `libasound2`, Fedora `alsa-lib`, Arch `alsa-lib`. (You do **not** need `-dev` / `*-devel` packages just to run a prebuilt binary.)
- **Linux D-Bus**: `libdbus-1` at runtime — e.g. Debian/Ubuntu `libdbus-1-3`, Fedora `dbus-libs`, Arch `dbus`. Required by the Linux binary (scrobble keyring, etc.); usually already installed on desktop Linux.
- **macOS audio**: Uses Core Audio via the system toolchain; no separate audio library install for typical use.
- **Optional**: `fzf` or `sk` on `PATH` if you use the library fuzzy picker (see `[library]` in the sample config).

Prebuilt archives on [Releases](https://github.com/acmagn/ratune/releases): **Linux x86_64** (`x86_64-unknown-linux-gnu`), **macOS Apple Silicon** (`aarch64-apple-darwin`), and **macOS Intel** (`x86_64-apple-darwin`). Other targets need a local build (or your own packaging).

### Build from source

Everything under **Runtime**, plus:

- **Rust**: Stable toolchain (`rustup` default stable is fine).
- **Linux build deps**: ALSA and D-Bus headers plus `pkg-config` — e.g. Debian/Ubuntu `libasound2-dev` + `libdbus-1-dev` + `pkg-config`, Fedora `alsa-lib-devel` + `dbus-devel`, Arch `alsa-lib` + `dbus`.

---

## Installation

Current options are:

[Binaries](#binary-releases-linux-x86_64-macos)
[AUR](#arch-linux-aur)
[crates.io](#cratesio)
[Homebrew](#macos-homebrew)
[From source](#build-from-source-1).


### Binary Releases (Linux x86_64, macOS)

Download the `.tar.gz` for your platform from [Releases](https://github.com/acmagn/ratune/releases).
Extract and put `ratune` on your `PATH`.

### Arch Linux (AUR)

Install the **`-bin`** package with an AUR helper, e.g.:

```bash
yay -S ratune-bin
# or: paru -S ratune-bin
```

[ratune-bin on AUR](https://aur.archlinux.org/packages/ratune-bin) ships the same Linux binary as GitHub Releases.

### crates.io

```sh
cargo install ratune
```

This **builds** from the published crate. You need a **Rust toolchain**; on **Linux**, install ALSA and D-Bus **development** packages first (same as [Build from source](#build-from-source)).

### macOS (Homebrew)

```bash
brew tap acmagn/tap
brew install ratune
```

### Build from source

Use this when you want the latest git checkout, you’re on an OS/arch without a prebuilt, or you’re developing Ratune.

**Linux:** install ALSA and D-Bus headers and `pkg-config` *before* the first build:

```bash
# Debian / Ubuntu
sudo apt install libasound2-dev libdbus-1-dev pkg-config

# Fedora / RHEL
sudo dnf install alsa-lib-devel dbus-devel pkg-config

# Arch
sudo pacman -S alsa-lib dbus
```

**Clone and build:**

```sh
git clone https://github.com/acmagn/ratune.git
cd ratune
cargo build --release
```

The binary is `target/release/ratune`. Check the build with `ratune --version` (or `-V`).

**Album art in the terminal:** from a source checkout you can run a small [ratatui-image](https://github.com/ratatui/ratatui-image) harness (same capability query as the real UI) to verify your terminal or tmux passthrough. Use any JPEG/PNG (etc.) on disk:

```sh
# Cargo workspace root (this repo layout)
cargo run -p ratune --example art_image_test -- /path/to/cover.jpg

# Or from the ratune/ crate directory only
cargo run --example art_image_test -- /path/to/cover.jpg
```

Press `q` or Esc to exit.

---

## Configuration

On first start, ratune creates a short default file at `~/.config/ratune/config.toml` (server fields plus common UI defaults). For every key with comments, use the sample file and copy the sections you need: [`docs/sample-config.toml`](docs/sample-config.toml).

### Connecting

Set Subsonic **url** and **username**, then choose how to supply the secret (most secure first):

1. **OS keyring (default)** — leave `password = ""` or remove field entirely. On Linux choose the backend with `password_keyring` (see below).
2. **`password_command`** — run a shell command; stdout is the secret (e.g. `secret-tool`, `pass`, KeePassXC CLI).
3. **Plaintext** — `password = "..."` in the file, or env vars (convenient for scripts; avoid in shared configs).

#### Keyring

Leave `password` empty. Ratune uses [`keyring-core`](https://crates.io/crates/keyring-core) with a platform store: on Linux you pick **`keyutils`** (kernel keyring, default) or **`secret-service`** (gnome-keyring / KWallet); **Keychain** on macOS; **Credential Manager** on Windows. On first run you are prompted once ([inquire](https://crates.io/crates/inquire)); the secret is stored under service **`ratune`** and user **`{url}|{username}`** — not in `config.toml`.

- **`password_keyring = "keyutils"`** (default) — [kernel keyutils](https://docs.rs/linux-keyutils-keyring-store/latest/linux_keyutils_keyring_store/). Lightweight and fine for a server password you can re-enter after reboot; keys may not survive reboot ([persistence](https://docs.rs/linux-keyutils-keyring-store/latest/linux_keyutils_keyring_store/#persistence)).
- **`password_keyring = "secret-service"`** — [Secret Service](https://specifications.freedesktop.org/secret-service/) via libsecret (same wallet as `secret-tool`, browser password managers, etc.). Better when you want the password to persist across reboots like other desktop secrets.

If the chosen store is unavailable (e.g. headless container) you get a one-time session prompt, use **`password_command`** or **`SUBSONIC_PASS`** instead.

```toml
[server]
url = "https://your-navidrome.example.com"
username = "you"
password = "" # or remove entirely
# password_keyring = "keyutils"       # default on Linux
# password_keyring = "secret-service" # gnome-keyring / KWallet
```

#### External secret store (`password_command`)

When you already use a wallet (e.g. `secret-tool`, `pass`, KeePassXC):

```toml
[server]
url = "https://your-navidrome.example.com"
username = "you"
password_command = "secret-tool lookup --label=ratune service subsonic user you"
```

The command runs under `/bin/sh -c` on Unix (or `cmd /C` on Windows); only trimmed stdout is used. Plaintext `password` or `SUBSONIC_PASS` take precedence if set.

#### Plaintext

```toml
password = "your_password"
```

Subsonic auth uses a random salt per request and `MD5(secret + salt)` ([Navidrome / Subsonic API](https://www.navidrome.org/docs/developers/subsonic-api/)).

### Environment overrides (optional)

Overrides the config file when set:

```sh
export SUBSONIC_URL="https://your-server.example.com"
export SUBSONIC_USER="admin"
export SUBSONIC_PASS="your_password"
```

`TERMUSIC_SUBSONIC_*` variants are also accepted (see sample config).

### Snippets: player, cache, theme, fzf

```toml
[player]
default_volume = 70
max_bit_rate = 0

[cache]
enabled = true
max_size_gb = 2

[ui]
# Only `album_art_backend` lives here; NP strip/queue/toggles → `[ui.nptab]`, `[ui.row_now_playing]`, …

[theme]
preset = "dynamic"

[library]
# enabled, index path, fzf binary, fzf args, …  → see sample-config.toml
```

Remapping is done in `[keybinds]`; colors in `[theme]`; now-playing strip vs queue are different keys — see the sample and in-app help (`i`).

### Folder navigation (Browse)

When enabled, the Browse tab can switch between the usual artist / album / track columns and a folder layout that mirrors how your server organizes files on disk (or per-library roots). This uses the Subsonic APIs `getMusicFolders`, `getIndexes`, and `getMusicDirectory` (tested with Navidrome and [gonic](https://github.com/sentriz/gonic)).

**Enable in config** (`[ui.browsetab]` in [`docs/sample-config.toml`](docs/sample-config.toml)):

```toml
[ui.browsetab]
folder_navigation = true
# mode = "artists"   # default on startup (default)
# mode = "files"     # start in folder view when folder_navigation is true
```

**Toggle at runtime:** default **`Ctrl+b`** (`toggle_folder_browse` in `[keybinds]`). Switches between folder view and artist browsing and jumps to the Browse tab. If you start in `files` mode, the first toggle to artists loads the artist list if it was not fetched yet.

### Scrobbling

Ratune can scrobble listens to Last.fm or Libre.fm and optionally notify your Subsonic server so Navidrome records play counts. Because Ratune controls playback directly, scrobbles are based on actual listen progress — not MPRIS metadata.

Full reference: [`[scrobble]`](docs/sample-config.toml) in the sample config.

#### Enable

Register an API account at [Last.fm](https://www.last.fm/api/account/create) (or Libre.fm equivalent), then add a `[scrobble]` block. 

```toml
[scrobble]
enabled = true
service = "lastfm"   # or "librefm"
api_key = "your_application_key"
scrobble_to_server = true   # Subsonic /scrobble (default: true; works without Last.fm)
```

Same options for secret handling as Subsonic password are provided.

#### keyring or secret commands

To not store secrets in the file (synced dotfiles, shared machines, etc.), leave `api_secret` / `session_key` empty and use either command in config or ratune functions to save to the keyring.


| Secret | Resolution order |
|--------|------------------|
| `api_secret` | config → `api_secret_command` → OS keyring (`lastfm\|api_secret`) |
| `session_key` | config → `session_key_command` → OS keyring (`lastfm\|session`) |

Keyring entries use service `ratune`. On Linux, scrobble secrets always use Secret Service (gnome-keyring / KWallet). Env vars (`LASTFM_API_SECRET`, `LASTFM_SESSION_KEY`, …) override the file, same as Subsonic.

```sh
ratune scrobble-api-secret --save-keyring
ratune scrobble-auth --save-keyring
```

Without `--save-keyring`, each command prints the value to paste into config instead.

If you previously saved scrobble secrets with an older build (kernel keyutils), re-run the commands above once and they will land in your desktop wallet instead.

#### plaintext

You can optionally store either/both of these as plaintext instead.

```toml
[scrobble]
enabled = true
service = "lastfm"   # or "librefm"
api_key = "your_application_key"
api_secret = "your_shared_secret"
session_key = "your_session_key"   # from `ratune scrobble-auth`
scrobble_to_server = true   # Subsonic /scrobble (default: true; works without Last.fm)
```

Get `session_key` once with `ratune scrobble-auth` (prints the key for config unless you pass `--save-keyring`).

#### Behaviour

- **Now playing** is sent when a track starts.
- **Scrobble** fires at min(`min_percent`% of track length, `max_listen_seconds`). Defaults for Last.fm: 50%, 4 minutes. Tracks ≤ `min_track_seconds` (default 30 s) are skipped.
- **Subsonic scrobble** (if enabled) uses a separate local threshold (default: 50%, 30 s cap).
- Both sets of thresholds are optional under `[scrobble.thresholds.local]` and `[scrobble.thresholds.audioscrobbler]` — see the sample config. Audioscrobbler defaults follow [Last.fm’s scrobbling rules](https://www.last.fm/api/scrobbling); deviating may cause ignored scrobbles.
- Failed Last.fm submissions are queued in `~/.local/share/ratune/scrobble-queue.json` and retried on the next launch (entries older than 14 days are dropped).
- The status bar shows the service name when scrobbling is enabled; a **✓** appears briefly after a successful submit. Pending queue items show as `Last.fm (N)`.

---

## Default keybinds

These are defaults; everything is overridable in `config.toml`. Press `i` in the app for the list that matches your file.

| Key | Action |
| --- | --- |
| `1` / `2` / `3` | Home / Browse / Now playing |
| `Tab` | Next tab (wrap) |
| `j` / `k` | Move selection |
| `h` / `l` | Columns / home album strip |
| `Enter` | Open / play |
| `a` / `A` | Add track / add all |
| `p` / `Space` | Play / pause |
| `n` / `N` | Next / previous |
| `x` / `z` | Shuffle / unshuffle |
| `+` / `-` | Volume |
| `←` / `→` | Seek (Now playing) |
| `/` | Search |
| `L` | Lyrics |
| `V` | Visualizer |
| `P` | Playlist overlay (Browse) |
| `>` | Add to playlist (Browse) |
| `Ctrl+f` | Library fzf picker (if configured) |
| `Ctrl+b` | Toggle folder / artist browse (if `[ui.browsetab] folder_navigation = true`) |
| `t` | Toggle dynamic theme |
| `i` | Help |
| `q` | Quit |

---

## Screenshots


### Main UI

<p align="center">
  <img src="docs/screenshots/now_playing.png" alt="Now Playing" width="49%" />
  <img src="docs/screenshots/home_page_art.png" alt="Home" width="49%" />
</p>
<p align="center">
  <img src="docs/screenshots/browse.png" alt="Browse" width="49%" />
  <img src="docs/screenshots/lyrics.png" alt="Lyrics" width="49%" />
</p>
<p align="center">
  <img src="docs/screenshots/visualizer.png" alt="Visualizer" width="49%" />
  <img src="docs/screenshots/playlists.png" alt="Playlists" width="49%" />
</p>
<p align="center">
  <img src="docs/screenshots/info.png" alt="Track info" width="49%" />
</p>

### Fuzzy finder

Full-library fzf (or sk) flow.

**Library metadata required for fuzzy finding to work properly.** Enable it and configure refresh/arguments under `[library]` in [`docs/sample-config.toml`](docs/sample-config.toml). Ratune will then fill the library metadata. It can take a few minutes and fuzzy finding will be unavailble during that time, please be patient!

<p align="center">
  <img src="docs/screenshots/fzf.png" alt="Fuzzy library picker" width="90%" />
</p>

### Customization

Theme, layout, now-playing lines, queue row template, tab bar, and more are configured in `config.toml` (see the sample config).


<p align="center">
  <img src="docs/screenshots/customization.png" alt="UI customization" width="90%" />
</p>

Album art, fzf, and visualizer features can be disabled for those desiring a minimalist experience:

<p align="center">
  <img src="docs/screenshots/queue-only.png" alt="UI customization now playing" width="90%" />
  <img src="docs/screenshots/home_page_no_art.png" alt="UI customization home" width="90%" />
</p>

---

## tmux

For album art and focus events inside tmux:

```tmux
set -g allow-passthrough on
set -g focus-events on
```

---

## Project layout

This repository is a Cargo workspace with four crates:

| Crate | Role |
| --- | --- |
| [`ratune`](ratune/) | TUI, event loop, state, art, fzf, MPRIS, scrobbling |
| [`ratune-subsonic`](ratune-subsonic/) | Subsonic HTTP client and models |
| [`ratune-scrobble`](ratune-scrobble/) | Last.fm / Libre.fm Audioscrobbler client and play thresholds |
| [`ratune-player`](ratune-player/) | Audio (rodio), gapless, sample tap for the visualizer |

Details: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

---

## Data on disk

| Path | Purpose |
| --- | --- |
| `~/.config/ratune/config.toml` | Config |
| `~/.config/ratune/state.json` | UI state, queue, browser position |
| `~/.local/share/ratune/history.json` | Play history |
| `~/.local/share/ratune/scrobble-queue.json` | Pending Last.fm scrobbles (offline retry) |
| `~/.cache/ratune/` | Track cache, library index JSON, etc. |

---

## Credits

Ratune is based on [playterm](https://github.com/awriterandtheword-rgb/playterm-app) by [awriterandtheword-rgb](https://github.com/awriterandtheword-rgb) (MIT). The original project is licensed under MIT and served as the foundation for this work. Ratune has since diverged significantly with new features, performance improvements, and UI changes.

---

## Acknowledgements

- [ratatui](https://github.com/ratatui/ratatui) — TUI
- [rodio](https://github.com/RustAudio/rodio) — playback
- [Navidrome](https://www.navidrome.org/) — test target server
- [LRCLib](https://lrclib.net) — lyrics
- [rmpc](https://github.com/mierak/rmpc) — ideas for navigation and art

## License

[MIT](https://opensource.org/licenses/MIT)
