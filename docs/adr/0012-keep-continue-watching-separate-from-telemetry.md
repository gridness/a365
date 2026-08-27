# Keep Continue Watching separate from Telemetry

a365 stores Continue Watching as product state containing the selected Series and Episode, last Translation and resolution, and the Player's last observed position as whole elapsed seconds. IINA and mpv report position through the same private IPC channel used to monitor playback; a resumed Player receives that position through its native start option. A natural end advances to the next available Episode with its position reset, while stopped or interrupted playback retains the observed position and a playback failure preserves the previously saved one.

Continue Watching is neither derived from nor written to Telemetry, so disabling or clearing Telemetry cannot remove product functionality and the established Telemetry privacy boundary remains intact. State files written before position tracking remain valid and resume from the beginning.
