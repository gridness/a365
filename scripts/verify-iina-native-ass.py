#!/usr/bin/env python3
"""Verify that a365's proxied ASS remains visible across IINA seeks."""

from __future__ import annotations

import argparse
import http.server
import json
import re
import shutil
import socket
import subprocess
import tempfile
import threading
import time
import urllib.request
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
PROXY_SOURCE = ROOT / "crates/a365dt-cli/src/playback/proxy.rs"
IINA_FALLBACK = Path("/Applications/IINA.app/Contents/MacOS/iina-cli")
ASS = b"""[Script Info]
ScriptType: v4.00+
PlayResX: 640
PlayResY: 360

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Default,Arial,32,&H00FFFFFF,&H000000FF,&H00000000,&H80000000,-1,0,0,0,100,100,0,0,1,2,1,2,20,20,28,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: 0,0:00:00.00,0:00:03.00,Default,,0,0,0,,a365 cue zero
Dialogue: 0,0:00:03.00,0:00:06.00,Default,,0,0,0,,a365 cue one
Dialogue: 0,0:00:06.00,0:00:09.00,Default,,0,0,0,,a365 cue two
Dialogue: 0,0:00:09.00,0:00:12.00,Default,,0,0,0,,a365 cue three
"""

SEEK_CASES = (
    (1.0, "a365 cue zero"),
    (10.0, "a365 cue three"),
    (4.0, "a365 cue one"),
    (7.0, "a365 cue two"),
)
SEEK_ROUNDS = 25
REAL_CUE_COUNT = 2
REAL_STRESS_ROUNDS = 12
REAL_SEEKS_PER_ROUND = 8


def subtitle_suffix() -> str:
    source = PROXY_SOURCE.read_text(encoding="utf-8")
    match = re.search(
        r'let subtitle_path = format!\("/\{capability\}([^"}]*)"\)', source
    )
    if match is None:
        raise RuntimeError(f"Could not derive the subtitle endpoint from {PROXY_SOURCE}")
    return match.group(1)


class AssetHandler(http.server.BaseHTTPRequestHandler):
    server_version = "a365-ass-verifier"

    def do_HEAD(self) -> None:
        self._serve(include_body=False)

    def do_GET(self) -> None:
        self._serve(include_body=True)

    def _serve(self, *, include_body: bool) -> None:
        server = self.server
        if self.path == server.media_path:
            body = server.media
            content_type = "video/mp4"
        elif self.path == server.subtitle_path:
            body = ASS
            content_type = "application/octet-stream"
        else:
            self.send_error(404)
            return

        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Accept-Ranges", "bytes")
        self.end_headers()
        if include_body:
            self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:
        pass


def query(socket_path: Path, command: list[object]) -> object:
    request_id = 365
    payload = json.dumps({"command": command, "request_id": request_id}).encode() + b"\n"
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.settimeout(2)
        client.connect(str(socket_path))
        client.sendall(payload)
        with client.makefile("rb") as response:
            while line := response.readline():
                message = json.loads(line)
                if message.get("request_id") == request_id:
                    if message.get("error") != "success":
                        raise RuntimeError(f"IINA IPC failed: {message.get('error')}")
                    return message.get("data")
    raise RuntimeError("IINA closed its IPC connection without a response")


def wait_for_socket(socket_path: Path, process: subprocess.Popen[bytes]) -> None:
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if socket_path.is_socket():
            return
        if process.poll() is not None:
            raise RuntimeError(f"iina-cli exited early with status {process.returncode}")
        time.sleep(0.1)
    raise RuntimeError("IINA did not create its mpv IPC socket")


def wait_for_subtitle(socket_path: Path) -> list[dict[str, object]]:
    deadline = time.monotonic() + 8
    tracks: list[dict[str, object]] = []
    while time.monotonic() < deadline:
        result = query(socket_path, ["get_property", "track-list"])
        if isinstance(result, list):
            tracks = result
            if any(track.get("type") == "sub" for track in tracks):
                return tracks
        time.sleep(0.2)
    return tracks


def wait_for_media(socket_path: Path) -> None:
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        result = query(socket_path, ["get_property", "track-list"])
        if isinstance(result, list) and any(
            track.get("type") == "video" for track in result
        ):
            return
        time.sleep(0.2)
    raise RuntimeError("IINA did not finish loading the verification media")


def selected_subtitle_track(socket_path: Path) -> dict[str, object] | None:
    tracks = query(socket_path, ["get_property", "track-list"])
    if not isinstance(tracks, list):
        return None
    return next(
        (
            track
            for track in tracks
            if track.get("type") == "sub" and track.get("selected") is True
        ),
        None,
    )


def wait_for_seek(socket_path: Path, target: float) -> None:
    deadline = time.monotonic() + 2
    while time.monotonic() < deadline:
        position = query(socket_path, ["get_property", "time-pos"])
        if isinstance(position, (int, float)) and abs(position - target) < 0.25:
            return
        time.sleep(0.02)
    raise RuntimeError(f"IINA did not seek to {target:.1f}s")


def rendered_subtitle_pixels(path: Path) -> int:
    ffmpeg = shutil.which("ffmpeg")
    if ffmpeg is None:
        raise RuntimeError("ffmpeg is required for the IINA verification harness")
    result = subprocess.run(
        [
            ffmpeg,
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            str(path),
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "pipe:1",
        ],
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    pixels = zip(*(iter(result.stdout),) * 3, strict=True)
    return sum(max(pixel) > 96 for pixel in pixels)


def capture_frame(socket_path: Path, path: Path, mode: str) -> None:
    query(socket_path, ["screenshot-to-file", str(path), mode])
    deadline = time.monotonic() + 2
    while time.monotonic() < deadline:
        if path.is_file() and path.stat().st_size > 0:
            return
        time.sleep(0.02)
    raise RuntimeError(f"IINA did not write verification screenshot {path}")


def capture_subtitle_pixels(socket_path: Path, path: Path) -> int:
    capture_frame(socket_path, path, "subtitles")
    return rendered_subtitle_pixels(path)


def rendered_difference_pixels(with_subtitles: Path, video_only: Path) -> int:
    ffmpeg = shutil.which("ffmpeg")
    if ffmpeg is None:
        raise RuntimeError("ffmpeg is required for the IINA verification harness")
    result = subprocess.run(
        [
            ffmpeg,
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            str(with_subtitles),
            "-i",
            str(video_only),
            "-filter_complex",
            "[0:v][1:v]blend=all_mode=difference,format=gray",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "gray",
            "pipe:1",
        ],
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    return sum(value > 16 for value in result.stdout)


def capture_subtitle_difference(
    socket_path: Path, capture_directory: Path, name: str
) -> int:
    with_subtitles = capture_directory / f"{name}-subtitles.png"
    without_subtitles = capture_directory / f"{name}-without-subtitles.png"
    original_visibility = query(socket_path, ["get_property", "sub-visibility"])
    try:
        query(socket_path, ["set_property", "sub-visibility", True])
        capture_frame(socket_path, with_subtitles, "subtitles")
        query(socket_path, ["set_property", "sub-visibility", False])
        capture_frame(socket_path, without_subtitles, "subtitles")
    finally:
        query(
            socket_path,
            ["set_property", "sub-visibility", original_visibility is not False],
        )
    return rendered_difference_pixels(with_subtitles, without_subtitles)


def verify_subtitles_across_seeks(
    socket_path: Path, capture_directory: Path
) -> tuple[bool, dict[str, object]]:
    query(socket_path, ["set_property", "pause", False])
    for round_index in range(SEEK_ROUNDS):
        for case_index, (target, expected) in enumerate(SEEK_CASES):
            overshoot = SEEK_CASES[(case_index + 1) % len(SEEK_CASES)][0]
            seek_mode = (
                "absolute+exact" if round_index % 2 == 0 else "absolute+keyframes"
            )
            query(socket_path, ["seek", overshoot, seek_mode])
            query(socket_path, ["seek", target, seek_mode])
            wait_for_seek(socket_path, target)
            time.sleep(0.08)
            screenshot = capture_directory / f"seek-{round_index}-{case_index}.png"
            subtitle_pixels = capture_subtitle_pixels(socket_path, screenshot)
            track = selected_subtitle_track(socket_path)
            if subtitle_pixels < 100:
                return False, {
                    "round": round_index + 1,
                    "target": target,
                    "expected": expected,
                    "rendered_subtitle_pixels": subtitle_pixels,
                    "track_present_and_selected": track is not None,
                    "subtitle_visibility": query(
                        socket_path, ["get_property", "sub-visibility"]
                    ),
                    "subtitle_id": query(socket_path, ["get_property", "sid"]),
                }
    return True, {"seeks": SEEK_ROUNDS * len(SEEK_CASES)}


def discover_real_subtitle_cues(
    socket_path: Path, capture_directory: Path
) -> list[float]:
    track = selected_subtitle_track(socket_path)
    if track is None:
        raise RuntimeError("IINA has no selected subtitle track")
    source = track.get("external-filename")
    if not isinstance(source, str):
        raise RuntimeError("IINA did not expose the selected subtitle source")
    if source.startswith("http://127.0.0.1:"):
        with urllib.request.urlopen(source, timeout=5) as response:
            payload = response.read()
    else:
        payload = Path(source).read_bytes()
    text = payload.decode("utf-8-sig")
    section = ""
    event_format: list[str] = []
    candidates: list[float] = []
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            section = stripped
            continue
        if section != "[Events]" or ":" not in stripped:
            continue
        kind, value = stripped.split(":", 1)
        if kind == "Format":
            event_format = [field.strip() for field in value.split(",")]
            continue
        if kind != "Dialogue" or not event_format:
            continue
        values = [field.strip() for field in value.split(",", len(event_format) - 1)]
        if len(values) != len(event_format):
            continue
        event = dict(zip(event_format, values, strict=True))
        try:
            start = ass_timestamp(event["Start"])
            end = ass_timestamp(event["End"])
        except (KeyError, ValueError):
            continue
        body = event.get("Text", "")
        plain_text = re.sub(r"\{[^}]*\}", "", body)
        plain_text = re.sub(r"\\[Nnh]", " ", plain_text).strip()
        if (
            end - start < 1.5
            or start > 360
            or not plain_text
            or re.search(r"\\p[1-9]", body)
        ):
            continue
        target = (start + end) / 2
        if all(abs(target - existing) >= 1 for existing in candidates):
            candidates.append(target)
    candidates.sort()
    candidates = candidates[:: len(candidates) - 1] if len(candidates) > 1 else candidates
    query(socket_path, ["set_property", "pause", True])
    cues = []
    for probe_index, target in enumerate(candidates):
        visible = True
        for offset_index, offset in enumerate((-0.25, 0.25)):
            probe = target + offset
            query(socket_path, ["seek", probe, "absolute+exact"])
            wait_for_seek(socket_path, probe)
            difference = capture_subtitle_difference(
                socket_path,
                capture_directory,
                f"discover-{probe_index}-{offset_index}",
            )
            visible = visible and difference >= 100
        if visible:
            cues.append(target)
        if len(cues) == REAL_CUE_COUNT:
            break
    return cues


def verify_real_subtitles_across_seeks(
    socket_path: Path, capture_directory: Path, cues: list[float]
) -> tuple[bool, dict[str, object]]:
    checks = 0
    for round_index in range(REAL_STRESS_ROUNDS):
        query(socket_path, ["set_property", "pause", False])
        target = cues[round_index % len(cues)]
        for seek_index in range(REAL_SEEKS_PER_ROUND):
            target = cues[(round_index + seek_index) % len(cues)]
            query(socket_path, ["seek", target, "absolute+exact"])
            wait_for_seek(socket_path, target)
        time.sleep(0.08)
        query(socket_path, ["set_property", "pause", True])
        difference = capture_subtitle_difference(
            socket_path,
            capture_directory,
            f"real-{round_index}",
        )
        checks += REAL_SEEKS_PER_ROUND
        track = selected_subtitle_track(socket_path)
        if difference < 100:
            return False, {
                "round": round_index + 1,
                "seeks_before_check": checks,
                "target": target,
                "rendered_subtitle_difference_pixels": difference,
                "track_present_and_selected": track is not None,
                "subtitle_visibility": query(
                    socket_path, ["get_property", "sub-visibility"]
                ),
                "subtitle_id": query(socket_path, ["get_property", "sid"]),
            }
    return True, {"seeks": checks, "cue_positions": cues}


def verify_attached_player(socket_path: Path) -> int:
    if not socket_path.is_socket():
        raise RuntimeError(f"IINA IPC socket does not exist: {socket_path}")
    track = selected_subtitle_track(socket_path)
    if track is None:
        print("FAIL: attached IINA playback has no selected subtitle track")
        return 1
    with tempfile.TemporaryDirectory(prefix="a365-iina-real-ass-") as directory:
        temporary = Path(directory)
        cues = discover_real_subtitle_cues(socket_path, temporary)
        print(json.dumps({"discovered_cue_positions": cues}, indent=2))
        if not cues:
            print("FAIL: no rendered subtitle cue was found in the first six minutes")
            return 1
        passed, seek_summary = verify_real_subtitles_across_seeks(
            socket_path, temporary, cues
        )
        print(json.dumps(seek_summary, indent=2))
        if not passed:
            if seek_summary["track_present_and_selected"]:
                print("FAIL: subtitle rendering disappeared while its track stayed selected")
            else:
                print("FAIL: IINA lost the selected subtitle track while seeking")
            return 1
        print("PASS: real proxied ASS rendering survived repeated IINA seeks")
        return 0


def verify_attached_with_local_subtitle(
    socket_path: Path, *, canonicalize: bool
) -> int:
    track = selected_subtitle_track(socket_path)
    if track is None:
        print("FAIL: attached IINA playback has no selected subtitle track")
        return 1
    source = track.get("external-filename")
    track_id = track.get("id")
    if not isinstance(source, str) or not isinstance(track_id, int):
        raise RuntimeError("IINA did not expose the selected external subtitle source")
    with tempfile.TemporaryDirectory(prefix="a365-iina-local-ass-") as directory:
        local_subtitle = Path(directory) / "subtitle.ass"
        with urllib.request.urlopen(source, timeout=5) as response:
            local_subtitle.write_bytes(response.read())
        selected_subtitle = local_subtitle
        if canonicalize:
            ffmpeg = shutil.which("ffmpeg")
            if ffmpeg is None:
                raise RuntimeError("ffmpeg is required to canonicalize the ASS probe")
            selected_subtitle = Path(directory) / "canonical.ass"
            subprocess.run(
                [
                    ffmpeg,
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-y",
                    "-i",
                    str(local_subtitle),
                    str(selected_subtitle),
                ],
                check=True,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.PIPE,
            )
        query(socket_path, ["sub-remove", track_id])
        query(socket_path, ["sub-add", str(selected_subtitle), "select"])
        tracks = wait_for_subtitle(socket_path)
        if not any(
            candidate.get("type") == "sub"
            and candidate.get("selected") is True
            for candidate in tracks
        ):
            print("FAIL: IINA did not select the temporary local ASS copy")
            return 1
        description = "canonicalized" if canonicalize else "byte-identical"
        print(f"Loaded a {description} temporary local ASS copy")
        return verify_second_half_coverage(socket_path)


def find_iina() -> str:
    iina = shutil.which("iina-cli")
    if iina is None and IINA_FALLBACK.is_file():
        iina = str(IINA_FALLBACK)
    if iina is None:
        raise RuntimeError("IINA was not found")
    return iina


def verify_clone_without_monitor(
    source_socket: Path, *, synthetic_subtitle: bool
) -> int:
    track = selected_subtitle_track(source_socket)
    if track is None:
        print("FAIL: source IINA playback has no selected subtitle track")
        return 1
    media = query(source_socket, ["get_property", "path"])
    subtitle = track.get("external-filename")
    if not isinstance(media, str) or not isinstance(subtitle, str):
        raise RuntimeError("IINA did not expose the source Playback assets")
    with tempfile.TemporaryDirectory(prefix="a365-iina-clone-") as directory:
        temporary = Path(directory)
        socket_path = temporary / "iina.sock"
        if synthetic_subtitle:
            subtitle_path = temporary / "synthetic.ass"
            subtitle_path.write_bytes(ASS)
            subtitle = str(subtitle_path)
        process = subprocess.Popen(
            [
                find_iina(),
                "--separate-windows",
                "--no-stdin",
                "--keep-running",
                f"--mpv-input-ipc-server={socket_path}",
                "--mpv-force-media-title=a365 monitor isolation probe",
                media,
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        try:
            wait_for_socket(socket_path, process)
            wait_for_media(socket_path)
            query(socket_path, ["sub-add", subtitle, "select"])
            wait_for_subtitle(socket_path)
            subtitle_description = "synthetic ASS" if synthetic_subtitle else "same proxied ASS"
            print(
                f"Loaded the real media with {subtitle_description} "
                "without the a365 monitor connection"
            )
            return verify_attached_player(socket_path)
        finally:
            try:
                query(socket_path, ["quit"])
            except (OSError, RuntimeError):
                pass
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                process.terminate()
                process.wait(timeout=3)


def observe_subtitle_recovery(socket_path: Path) -> int:
    if selected_subtitle_track(socket_path) is None:
        print("FAIL: attached IINA playback has no selected subtitle track")
        return 1
    query(socket_path, ["set_property", "pause", True])
    started = time.monotonic()
    with tempfile.TemporaryDirectory(prefix="a365-iina-recovery-") as directory:
        temporary = Path(directory)
        for index in range(20):
            difference = capture_subtitle_difference(
                socket_path, temporary, f"recovery-{index}"
            )
            elapsed = time.monotonic() - started
            if difference >= 100:
                print(
                    json.dumps(
                        {
                            "recovered_after_seconds": round(elapsed, 3),
                            "rendered_subtitle_difference_pixels": difference,
                        },
                        indent=2,
                    )
                )
                print("PASS: subtitle rendering recovered while paused")
                return 0
            time.sleep(0.1)
    print(
        json.dumps(
            {
                "recovered_after_seconds": None,
                "observed_seconds": round(time.monotonic() - started, 3),
                "track_present_and_selected": selected_subtitle_track(socket_path)
                is not None,
            },
            indent=2,
        )
    )
    print("FAIL: subtitle rendering remained absent at the failed frame")
    return 1


def ass_timestamp(value: str) -> float:
    hours, minutes, seconds = value.split(":")
    return int(hours) * 3600 + int(minutes) * 60 + float(seconds)


def analyze_real_subtitle(socket_path: Path) -> int:
    track = selected_subtitle_track(socket_path)
    if track is None:
        print("FAIL: attached IINA playback has no selected subtitle track")
        return 1
    source = track.get("external-filename")
    if not isinstance(source, str) or not source.startswith("http://127.0.0.1:"):
        raise RuntimeError("The selected subtitle is not the original loopback asset")
    with urllib.request.urlopen(source, timeout=5) as response:
        payload = response.read()
    text = payload.decode("utf-8-sig")
    section = ""
    section_counts: dict[str, int] = {}
    script_info: dict[str, str] = {}
    event_format: list[str] = []
    events: list[dict[str, object]] = []
    malformed_events = 0
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            section = stripped
            section_counts.setdefault(section, 0)
            continue
        if stripped:
            section_counts[section] = section_counts.get(section, 0) + 1
        if section == "[Script Info]" and ":" in stripped:
            key, value = stripped.split(":", 1)
            if key in {
                "ScriptType",
                "Collisions",
                "PlayResX",
                "PlayResY",
                "ScaledBorderAndShadow",
                "Timer",
                "WrapStyle",
                "YCbCr Matrix",
            }:
                script_info[key] = value.strip()
        if section != "[Events]" or ":" not in stripped:
            continue
        kind, value = stripped.split(":", 1)
        if kind == "Format":
            event_format = [field.strip() for field in value.split(",")]
        elif kind == "Dialogue" and event_format:
            values = [field.strip() for field in value.split(",", len(event_format) - 1)]
            if len(values) != len(event_format):
                malformed_events += 1
                continue
            event = dict(zip(event_format, values, strict=True))
            try:
                start = ass_timestamp(event["Start"])
                end = ass_timestamp(event["End"])
            except (KeyError, ValueError):
                malformed_events += 1
                continue
            body = event.get("Text", "")
            tags = sorted(set(re.findall(r"\\([A-Za-z]+)", body)))
            events.append(
                {
                    "start": start,
                    "end": end,
                    "layer": event.get("Layer"),
                    "style": event.get("Style"),
                    "effect": event.get("Effect"),
                    "text_length": len(body),
                    "override_tags": tags,
                }
            )
    starts = [event["start"] for event in events]
    durations = [event["end"] - event["start"] for event in events]
    active = {
        str(target): [
            event
            for event in events
            if event["start"] <= target < event["end"]
        ]
        for target in (2.25, 7.25)
    }
    print(
        json.dumps(
            {
                "bytes": len(payload),
                "line_endings": "crlf" if b"\r\n" in payload else "lf",
                "sections": section_counts,
                "script_info": script_info,
                "event_format": event_format,
                "dialogue_events": len(events),
                "malformed_events": malformed_events,
                "starts_are_monotonic": starts == sorted(starts),
                "non_positive_durations": sum(duration <= 0 for duration in durations),
                "minimum_duration": min(durations, default=None),
                "maximum_duration": max(durations, default=None),
                "active_events": active,
            },
            indent=2,
        )
    )
    return 0


def selected_subtitle_timeline(socket_path: Path) -> list[dict[str, object]]:
    track = selected_subtitle_track(socket_path)
    if track is None:
        raise RuntimeError("IINA has no selected subtitle track")
    source = track.get("external-filename")
    if not isinstance(source, str):
        raise RuntimeError("IINA did not expose the selected subtitle source")
    if source.startswith("http://127.0.0.1:"):
        with urllib.request.urlopen(source, timeout=5) as response:
            payload = response.read()
    else:
        payload = Path(source).read_bytes()
    events = []
    for line in payload.decode("utf-8-sig").splitlines():
        if not line.startswith("Dialogue:"):
            continue
        fields = line.split(",", 9)
        if len(fields) != 10:
            continue
        try:
            start = ass_timestamp(fields[1].strip())
            end = ass_timestamp(fields[2].strip())
        except ValueError:
            continue
        body = fields[9]
        plain_text = re.sub(r"\{[^}]*\}", "", body)
        plain_text = re.sub(r"\\[Nnh]", " ", plain_text).strip()
        events.append(
            {
                "start": start,
                "end": end,
                "duration": end - start,
                "has_visible_text": bool(plain_text)
                and re.search(r"\\p[1-9]", body) is None,
            }
        )
    return events


def verify_second_half_coverage(socket_path: Path) -> int:
    duration = query(socket_path, ["get_property", "duration"])
    if not isinstance(duration, (int, float)):
        raise RuntimeError("IINA did not report the media duration")
    events = selected_subtitle_timeline(socket_path)
    last_event_end = max((event["end"] for event in events), default=0.0)
    midpoint = float(duration) / 2
    second_half_events = [event for event in events if event["end"] > midpoint]
    summary = {
        "media_duration_seconds": round(float(duration), 3),
        "last_subtitle_event_seconds": round(float(last_event_end), 3),
        "subtitle_timeline_coverage": round(float(last_event_end) / duration, 3),
        "dialogue_events": len(events),
        "second_half_events": len(second_half_events),
    }
    print(json.dumps(summary, indent=2))
    if last_event_end < duration * 0.8:
        print("FAIL: the selected ASS timeline ends before the video's final fifth")
        return 1
    candidate = next(
        (
            event
            for event in second_half_events
            if event["duration"] >= 1.5 and event["has_visible_text"]
        ),
        None,
    )
    if candidate is None:
        print("FAIL: the selected ASS has no stable visible cue in the second half")
        return 1
    target = (candidate["start"] + candidate["end"]) / 2
    query(socket_path, ["set_property", "pause", True])
    query(socket_path, ["seek", target, "absolute+exact"])
    wait_for_seek(socket_path, target)
    with tempfile.TemporaryDirectory(prefix="a365-iina-second-half-") as directory:
        difference = capture_subtitle_difference(
            socket_path, Path(directory), "second-half"
        )
    if difference < 100:
        print(
            json.dumps(
                {
                    "target": round(float(target), 3),
                    "rendered_subtitle_difference_pixels": difference,
                    "track_present_and_selected": selected_subtitle_track(socket_path)
                    is not None,
                },
                indent=2,
            )
        )
        print("FAIL: a real second-half ASS cue did not render")
        return 1
    print("PASS: the selected ASS covers and renders in the video's second half")
    return 0


def generate_media(path: Path) -> None:
    ffmpeg = shutil.which("ffmpeg")
    if ffmpeg is None:
        raise RuntimeError("ffmpeg is required for the IINA verification harness")
    subprocess.run(
        [
            ffmpeg,
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=640x360:r=24:d=12",
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=48000:cl=stereo",
            "-shortest",
            "-c:v",
            "mpeg4",
            "-q:v",
            "5",
            "-c:a",
            "aac",
            str(path),
        ],
        check=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--socket",
        type=Path,
        help="attach to an active a365-launched IINA mpv IPC socket",
    )
    parser.add_argument(
        "--local-copy",
        action="store_true",
        help="replace the selected proxied ASS with a byte-identical temporary file",
    )
    parser.add_argument(
        "--canonicalize",
        action="store_true",
        help="remux the temporary local ASS copy through ffmpeg before loading it",
    )
    parser.add_argument(
        "--clone-without-monitor",
        action="store_true",
        help="open the same proxied assets in an isolated IINA IPC session",
    )
    parser.add_argument(
        "--synthetic-subtitle",
        action="store_true",
        help="use the known-good synthetic ASS in the isolated IINA clone",
    )
    parser.add_argument(
        "--observe-recovery",
        action="store_true",
        help="check whether subtitle rendering recovers at the current paused frame",
    )
    parser.add_argument(
        "--analyze-subtitle",
        action="store_true",
        help="print structural metadata for the selected loopback ASS without text",
    )
    parser.add_argument(
        "--verify-second-half",
        action="store_true",
        help="verify ASS timeline coverage and rendering after the media midpoint",
    )
    arguments = parser.parse_args()
    if arguments.socket is not None:
        if arguments.verify_second_half:
            return verify_second_half_coverage(arguments.socket)
        if arguments.analyze_subtitle:
            return analyze_real_subtitle(arguments.socket)
        if arguments.observe_recovery:
            return observe_subtitle_recovery(arguments.socket)
        if arguments.clone_without_monitor:
            return verify_clone_without_monitor(
                arguments.socket,
                synthetic_subtitle=arguments.synthetic_subtitle,
            )
        if arguments.local_copy:
            return verify_attached_with_local_subtitle(
                arguments.socket, canonicalize=arguments.canonicalize
            )
        return verify_attached_player(arguments.socket)

    iina = find_iina()

    suffix = subtitle_suffix()
    with tempfile.TemporaryDirectory(prefix="a365-iina-ass-") as directory:
        temporary = Path(directory)
        media_path = temporary / "probe.mp4"
        socket_path = temporary / "iina.sock"
        generate_media(media_path)

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), AssetHandler)
        server.media = media_path.read_bytes()
        server.media_path = "/capability/media"
        server.subtitle_path = f"/capability{suffix}"
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()

        origin = f"http://127.0.0.1:{server.server_port}"
        subtitle_argument = f"{origin}{server.subtitle_path}"
        process = subprocess.Popen(
            [
                iina,
                "--separate-windows",
                "--no-stdin",
                "--keep-running",
                f"--mpv-input-ipc-server={socket_path}",
                "--mpv-force-media-title=a365 native ASS verification",
                f"{origin}{server.media_path}",
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        try:
            wait_for_socket(socket_path, process)
            wait_for_media(socket_path)
            query(socket_path, ["sub-add", subtitle_argument, "select"])
            tracks = wait_for_subtitle(socket_path)
            summary = [
                {
                    key: track.get(key)
                    for key in ("type", "codec", "external", "external-filename", "selected")
                    if key in track
                }
                for track in tracks
            ]
            print(f"subtitle endpoint: {server.subtitle_path}")
            print(json.dumps(summary, indent=2))
            if not any(track.get("type") == "sub" for track in tracks):
                print("FAIL: IINA did not load an external subtitle track")
                return 1
            passed, seek_summary = verify_subtitles_across_seeks(
                socket_path, temporary
            )
            print(json.dumps(seek_summary, indent=2))
            if not passed:
                if seek_summary["track_present_and_selected"]:
                    print("FAIL: subtitle text disappeared while its track stayed selected")
                else:
                    print("FAIL: IINA lost the selected subtitle track while seeking")
                return 1
            print("PASS: IINA kept the proxied native ASS visible across repeated seeks")
            return 0
        finally:
            try:
                query(socket_path, ["quit"])
            except (OSError, RuntimeError):
                pass
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                process.terminate()
                process.wait(timeout=3)
            server.shutdown()
            server.server_close()
            thread.join(timeout=3)


if __name__ == "__main__":
    raise SystemExit(main())
