# a365

[![CI](https://github.com/Gridness/a365/actions/workflows/ci.yml/badge.svg)](https://github.com/Gridness/a365/actions/workflows/ci.yml)
[![Coverage](https://codecov.io/gh/Gridness/a365/graph/badge.svg?branch=main)](https://codecov.io/gh/Gridness/a365)
[![License](https://img.shields.io/github/license/Gridness/a365)](LICENSE)

`a365` is a keyboard-and-mouse terminal app for finding, playing, and
downloading anime from Anime365. It keeps styled ASS subtitles native, can use
your AniList library to arrange the current week's timetable, and exposes
Anime365 Moments and your Anime365 profile without coupling those community
pages to core playback.

## Requirements

- macOS: [IINA](https://iina.io/) for playback
- Linux: [mpv](https://mpv.io/) for playback and `secret-tool` from libsecret
  for securely stored AniList login
- an Anime365 access token
- optional: FFmpeg for muxing downloaded MP4 and ASS files into MKV

The media player only sees an unguessable loopback URL. Anime365 credentials
stay inside `a365`; they are never written into player arguments, media URLs,
logs, or ordinary application files.

## Install

With Homebrew:

```console
brew install Gridness/oosama/a365
```

To include optional muxing support:

```console
brew install Gridness/oosama/a365 --with-ffmpeg-full
```

You can also download a platform archive from the
[latest release](https://github.com/Gridness/a365/releases/latest), or build
from source:

```console
cargo install --git https://github.com/Gridness/a365 --bin a365
```

Version 3 archives and the Homebrew formula also install an `a365dt`
compatibility executable. It delegates to `a365`, prints a deprecation notice
for interactive legacy use without contaminating version or completion output,
and will be removed in version 4.

Build metadata overrides use `A365_BUILD_PROFILE`, `A365_COMMIT_SHA`, and
`A365_RUSTC`. Their `A365DT_*` spellings remain accepted through version 3,
with `A365_*` taking precedence, and are removed with the other version 4
compatibility surfaces.

## Use

Open the full-screen app:

```console
a365
```

The destinations are Home, Search, Timetable, Moments, AniList, and Profile.
Use the arrow keys, Tab, Enter, and Escape, or click tabs and rows and scroll
with the mouse. The TUI always restores the terminal before handing media to
IINA or mpv. Press `/` in AniList to filter by title, list, or status. Moments
includes category selection and previous/next page navigation.

Start with a search already entered, or explicitly state that playback is
wanted:

```console
a365 "Frieren"
a365 stream "Frieren"
a365 "https://anime365.ru/catalog/road-of-naruto-30887/"
```

Playback is the default. `stream` is an explicit spelling of the same flow.
Add `--download` to select and download episodes instead:

```console
a365 --download --output ~/Videos --jobs 8 "Frieren"
```

Automatic continuation is off by default. When enabled, `a365` advances only
after a natural end-of-file and only to the next whole-numbered episode on the
same translation track and resolution. Closing or stopping the player does not
advance.

Useful direct destinations and account commands:

```console
a365 timetable
a365 moments
a365 profile
a365 anilist login
a365 anilist status
a365 anilist list
a365 anilist logout
```

AniList login opens the system browser and returns automatically to the TUI
through the exact loopback callback
`http://127.0.0.1:43815/anilist/callback`. The integration is read-only: the
client refuses GraphQL mutations. AniList's implicit response does not return
OAuth state, so the callback page instead carries a fresh one-time local relay
nonce; the access token is validated through `Viewer` before it is saved.
Tokens are stored in macOS Keychain or Linux Secret Service;
`ANILIST_ACCESS_TOKEN` remains available for non-interactive environments.
When macOS asks whether `a365` may access the
`anilist-access-token` Keychain item, its password field expects the Mac login
password—not an AniList token.

## Preferences and adult content

Configure preferences interactively or inspect the resolved values:

```console
a365 config
a365 config show
a365 config reset
```

The editable file is `~/.a365/config.toml`. Supported preferences include the
download output, concurrency, mux behavior, the separate `adult`,
`adult_telemetry`, and `auto_play_next_episode` switches. Adult content is off
by default. Enabling it validates H365 independently; an H365 outage never
blocks Anime365.

Adult content is fail-closed in AniList, timetable, and Moments views: entries
whose classification is unknown are hidden. H365 telemetry is aggregate-only
unless `adult_telemetry` is separately enabled.

## Authentication and integration setup

On first ordinary use, `a365` opens Anime365's access-token page. Paste the
token into the hidden prompt. macOS can save it to Keychain;
`ANIME365_ACCESS_TOKEN` is supported in process environments.

Maintainers configuring the AniList OAuth application and least-privilege
release GitHub App should run the resumable guided wizard from the repository
root:

```console
scripts/setup-v3-integrations.sh
```

It explains every browser button and field, verifies the exact AniList
callback, configures short-lived GitHub App release credentials, records the
expected Actions variable and secret names, and offers a reversible source-PR
and tap-push dry run. It does not publish a release or rename a repository.

## Maintenance and privacy

```console
a365 completions zsh
a365 cache prune
a365 doctor
a365 doctor --debug
a365 stats
a365 telemetry show
a365 telemetry disable
a365 telemetry clear --yes
a365 update
a365 purge
```

`doctor` reports the Anime365 API, optional community-page health, H365 when
enabled, player installation, cache, preferences, local telemetry, and build
status as independent checks. Community failures only disable Moments/public
profile enrichment; search and playback continue.

Telemetry is local, optional, queryable SQLite data and is never transmitted.
It records command outcomes, bounded timing samples, playback sessions, and
ordinary source-qualified series identity. It excludes search text, candidates,
episode identity, URLs, filesystem paths, and credentials. Use `telemetry show`
to inspect every collected field.

Release state lives in `~/.a365`; development builds use `~/.a365-dev`.
Version 3 automatically migrates state from `~/.a365dt`/`~/.a365dt-dev`, older
OS-specific directories, and the old credential accounts. Migration is staged,
locked, permission-hardened, and recoverable before the old copy is removed.

## Development

The repository requires the current stable Rust toolchain. Commands are in the
[justfile](justfile):

```console
just fix -p a365dt-cli
just test -p a365dt-cli
just fmt
```

Architecture decisions are recorded in [docs/adr](docs/adr), and the v3 work
log is in [docs/v3-progress.md](docs/v3-progress.md).
