# Make Playback the default interactive action

a365 treats Playback as the default action after Series and Episode selection, while `stream` requests the same action explicitly and `--download` selects the persistent download workflow. Playback uses IINA on macOS and mpv on Linux, hands authenticated media and native ASS subtitles through a credential-free loopback boundary, and keeps one Episode active at a time. This changes the product from a downloader into a playback-first client without abandoning verified downloads or exposing access tokens to Player arguments and history.

Automatic continuation is a separate preference that is disabled by default. When enabled, only a natural end-of-file advances to the next whole-number Episode of the same Series with the same Translation track and resolution; continuation stops at missing coverage, fractional Episodes, manual stops, Player closure, interruption, or errors. Moment Playback never continues automatically.
