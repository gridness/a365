# a365

a365 discovers user-selected Anime365 and H365 releases while keeping episode, translation, Playback, and download choices explicit.

## Language

**Application home**:
The build-specific, user-scoped directory that owns a365's preferences, local cache, and telemetry state: `~/.a365` for release builds and `~/.a365-dev` for development builds. Preferences live in `config.toml`, cache files under `cache/`, and telemetry files under `data/`; downloaded media and OS-managed credentials are outside it.
_Avoid_: OS application directory, data directory

**Preferences**:
User-defined defaults for the output directory, concurrent download count, subtitle muxing, adult content availability, adult telemetry detail, and automatic Episode continuation. Any omitted preference inherits its built-in default. The TUI Config destination persists edits and applies them to the current Interactive session immediately; `a365 config`, `config show`, and `config reset` remain available for line-oriented use. Preferences are stored in the Application home's `config.toml`; explicit Invocation choices take precedence when present.
_Avoid_: Invocation flags, remote account settings

**Interactive session**:
An a365 run in which a person discovers a Series and makes Playback or download choices. Help, version reporting, shell-completion generation, and maintenance commands are not Interactive sessions.
_Avoid_: Launch, invocation

**Invocation**:
One execution of a365, including Interactive sessions and non-interactive or maintenance commands. Telemetry events from the same Invocation share an Invocation ID.
_Avoid_: Session, launch

**Tip**:
A short, single-line piece of a365 guidance shown at the beginning of an Interactive session. Its source text is Markdown.
_Avoid_: Hint, startup message

**Available update**:
A published stable a365 release whose version is semantically higher than the running version.
_Avoid_: Latest version, new version

**Telemetry event**:
An immutable, timestamped local observation of a365 usage or performance. It may identify a selected Series by title and Content source identity when its privacy preferences permit that, but never records search text, remote candidates, URLs, tokens, or file paths.
_Avoid_: Counter, metric snapshot

**Installation channel**:
The distribution route through which the running a365 executable was installed: Homebrew, Cargo, or manual when no managed route can be identified.
_Avoid_: Installation type, package manager

**Content source**:
A remote catalogue boundary that owns Series identities, metadata, Translations, and media access rules.
_Avoid_: Provider, website

**H365**:
The adult Content source hosted by Hentai365 and available only after explicit user opt-in.
_Avoid_: Adult mode, hidden catalogue

**Adult availability preference**:
User consent controlling whether a365 may expose adult Series across H365, the AniList library, the Timetable, and Moments. When consent is absent, content whose adult status cannot be established is also unavailable.
_Avoid_: H365 toggle, adult mode

**Adult telemetry preference**:
User consent controlling whether local Telemetry events may identify selected adult Series. Without consent, adult-content usage is represented only by source-agnostic counts and outcomes.
_Avoid_: Adult telemetry, privacy mode

**Series**:
A title from one Content source that contains Episodes.
_Avoid_: Anime

**Series suggestion**:
A Series proposed as a likely match while the user searches by title.
_Avoid_: Search guess, search result

**Series search alias**:
Content-source-recognized shorthand or an alternative name for a Series that need not appear in its displayed title.
_Avoid_: Acronym, abbreviation

**Series catalogue**:
The collection of Series available for discovery from one or more enabled Content sources.
_Avoid_: Title index, title database

**Catalogue hit**:
A Series selection that reuses a Series already present in the persisted Series catalogue when the search starts. Direct URLs, cancelled searches, and failed searches are neither hits nor misses.
_Avoid_: Cache hit, API cache hit

**Timetable**:
A current-week, user-timezone view of scheduled anime broadcasts, optionally ordered using the user's AniList connection.
_Avoid_: Calendar, schedule

**AniList connection**:
Browser implicit OAuth authorization that lets a365 read the user's AniList library and personalize Timetable ordering. A pending login owns the fixed loopback callback and a one-time local relay nonce; the validated token lives in the operating-system credential store rather than the Application home.
_Avoid_: AniList account, AniList profile

**AniList library**:
The user's read-only collection of standard and custom AniList anime lists, including each entry's status, progress, score, priority, and airing information.
_Avoid_: Watch history, AniList profile

**Anime365 profile**:
The user's Anime365 identity and subscription state, optionally accompanied by publicly visible anime-list progress, scores, and Moments.
_Avoid_: Watch history, account settings

**Moment**:
A short, user-created Anime365 video excerpt associated with an Episode and Series. a365 browses categorized, paginated Moment metadata and plays its highest official rendition without participating in creation or social discussion.
_Avoid_: Clip, Episode, Translation

**Episode**:
A selectable installment of a Series, identified within its Content source and displayed by its episode label.
_Avoid_: File, video

**Episode range**:
One or more inclusive numeric intervals requested from a Series. Overlapping intervals form their union. Missing whole-number Episodes require explicit confirmation, and fractional Episodes inside the intervals form an optional subset that is included only by explicit choice.
_Avoid_: Download batch

**Translation**:
One Content source media release for exactly one Episode, characterized by its kind, language, and authors. A RAW release is also a Translation.
_Avoid_: Translation track

**Translation authors**:
The people or group credited for a Translation.
_Avoid_: Translation title

**Subtitle asset**:
A separate styled subtitle file exposed by a Content source for a subtitle Translation. Its absence means the Translation's subtitles are contained in the video.
_Avoid_: Translation, caption

**Translation track**:
A set of Translations with the same kind, language, and authors across an Episode range. Its coverage is the subset of requested Episodes for which it contains exactly one Translation; choosing incomplete coverage explicitly reduces the Download batch.
_Avoid_: Translation, fallback

**Resolution plan**:
A mapping from every selected Episode to a chosen media resolution, consisting of one preferred resolution and any explicitly chosen exceptions.
_Avoid_: Automatic quality, silent fallback

**Download batch**:
The selected Episodes from one Series, paired with one Translation track and one Resolution plan.
_Avoid_: Queue, playlist

**Playback**:
Playing selected Episode or Moment media in an external Player without keeping a completed media file in the output directory.
_Avoid_: Stream, preview

**Automatic continuation preference**:
User consent allowing Playback to move from an Episode that ended naturally to the next whole-number Episode of the same Series when the selected Translation track and resolution remain available. It never continues after a manual stop, Player closure, interruption, playback error, fractional Episode, or Moment.
_Avoid_: Playlist, unconditional autoplay

**Continue Watching**:
Local product state that offers the same Episode at its last observed Player position after stopped or interrupted Playback, preserves the previous position after a playback failure, and offers the next available Episode from its beginning after a natural end. It remembers the last Translation and resolution for revalidation, stores position as whole elapsed seconds, and is independent of Telemetry.
_Avoid_: Playback telemetry, AniList progress, Anime365 profile

**Trending Series**:
A non-adult AniList trend that maps to a currently playable Series from an enabled Content source.
_Avoid_: Popular anime, recommendation

**Trending Moment**:
A non-adult Moment ranked by Anime365's popular ordering.
_Avoid_: Recent Moment, random Moment

**Player**:
The external application that receives selected Episode media and any separate Subtitle asset for Playback.
_Avoid_: Video backend, viewer

**Player handoff**:
The period in which the full-screen TUI remains rendered with a Playing now state but pauses input while an external Player owns Playback. The current TUI state remains part of the same Interactive session and returns to interaction when the Player exits.
_Avoid_: Leaving the TUI, ending the Interactive session

**Verified download**:
Downloaded Episode media that passed its transfer completion checks and was finalized successfully.
_Avoid_: Existing file, finished transfer

**Muxed download**:
A Verified download whose separate video and Subtitle asset are packaged in one container without rendering the subtitles into the video.
_Avoid_: Burned-in subtitles, re-encoded video
