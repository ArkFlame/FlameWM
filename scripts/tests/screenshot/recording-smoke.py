#!/usr/bin/env python3
"""Exercise the current recorder on an isolated X server; requires ffmpeg/ffprobe."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("binary", type=Path)
    parser.add_argument("evidence", type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    args.evidence.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="g9-", dir=args.evidence) as tmp:
        root = Path(tmp)
        read_fd, write_fd = os.pipe()
        server = subprocess.Popen(
            ["Xvfb", "-displayfd", str(write_fd), "-screen", "0", "1920x1080x24", "-nolisten", "tcp"],
            pass_fds=(write_fd,), stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
        )
        os.close(write_fd)
        try:
            with os.fdopen(read_fd) as display_pipe:
                display = display_pipe.readline().strip()
            if not display:
                raise RuntimeError("Xvfb failed to allocate display: " + server.stderr.read().decode())
            env = os.environ.copy()
            env["DISPLAY"] = ":" + display
            env.pop("WAYLAND_DISPLAY", None)
            results = []
            for ext in ("mp4", "webm", "gif"):
                output = root / ("recording." + ext)
                command = [str(binary), "--full", "--record", "2", "--fps", "30", "--output", str(output)]
                start = time.monotonic()
                timing = root / (ext + ".time")
                process = subprocess.Popen(
                    ["/usr/bin/time", "-o", str(timing), "-f", "%U %S %e %P", *command],
                    env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                )
                stdout, stderr = process.communicate(timeout=45)
                elapsed = time.monotonic() - start
                row = {"command": command, "exit": process.returncode, "elapsed_s": elapsed,
                       "stdout": stdout, "stderr": stderr,
                       "time_user_sys_elapsed_cpu": timing.read_text().strip()}
                results.append(row)
                if process.returncode:
                    raise AssertionError(row)
                probe = subprocess.run(["ffprobe", "-v", "error", "-show_streams", "-of", "json", str(output)],
                                       capture_output=True, text=True, check=True, timeout=10)
                streams = json.loads(probe.stdout)["streams"]
                video = [s for s in streams if s["codec_type"] == "video"]
                assert len(video) == 1 and (video[0]["width"], video[0]["height"]) == (1920, 1080), streams
                assert not any(s["codec_type"] == "audio" for s in streams), streams
                row["streams"] = streams
            audio_env = dict(env, PULSE_SERVER="unix:" + str(root / "missing-pulse"))
            command = [str(binary), "--full", "--record", "1", "--audio-source", "missing.monitor",
                       "--output", str(root / "audio.mp4")]
            failed = subprocess.run(command, env=audio_env, capture_output=True, text=True, timeout=20)
            results.append({"command": command, "exit": failed.returncode,
                            "stdout": failed.stdout, "stderr": failed.stderr})
            assert failed.returncode != 0 and "recording failed" in failed.stderr, results[-1]
            (args.evidence / "recording-smoke.json").write_text(json.dumps(results, indent=2) + "\n")
            print("recording formats, default audio and unavailable audio source: PASS")
        finally:
            server.terminate()
            try:
                server.wait(timeout=5)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()


if __name__ == "__main__":
    main()
