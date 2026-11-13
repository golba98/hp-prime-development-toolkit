"""Run native launch and real interaction checks, with explicit OS evidence."""

import argparse
import json
import os
import platform
import signal
import subprocess
import sys
from pathlib import Path


def main():
    root = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve()
    artifacts = args.artifacts.resolve()
    artifacts.mkdir(parents=True, exist_ok=True)
    environment = os.environ.copy()
    if sys.platform.startswith("linux"):
        environment.pop("WAYLAND_DISPLAY", None)
        environment.pop("WAYLAND_SOCKET", None)
    status = {
        "os": platform.platform(),
        "architecture": platform.machine(),
        "launch": "not tested",
        "interaction": "not tested",
    }
    try:
        result = subprocess.run(
            [
                str(binary),
                "run",
                str(root / "examples/LIVE.hpprgm"),
                "--gui",
                "--json",
                "--exit-on-finish",
            ],
            capture_output=True,
            check=False,
            timeout=20,
            cwd=root,
            env=environment,
        )
        (artifacts / "launch-stdout.json").write_bytes(result.stdout)
        (artifacts / "launch-stderr.txt").write_bytes(result.stderr)
        report = json.loads(result.stdout)
        assert (
            result.returncode == 0
            and report["data"]["viewer"]["desktop"]
            and report["frames_presented"] >= 5
        ), report
        status["launch"] = "passed"
        with (
            (artifacts / "interaction-stdout.txt").open("wb") as stdout,
            (artifacts / "interaction-stderr.txt").open("wb") as stderr,
        ):
            interaction = subprocess.Popen(
                [
                    sys.executable,
                    "-B",
                    str(root / "scripts/native_gui/test_native.py"),
                    "--binary",
                    str(binary),
                    "--artifacts",
                    str(artifacts / "interaction"),
                ],
                cwd=root,
                stdout=stdout,
                stderr=stderr,
                env=environment,
                start_new_session=os.name != "nt",
                creationflags=subprocess.CREATE_NEW_PROCESS_GROUP
                if os.name == "nt"
                else 0,
            )
            try:
                interaction.wait(timeout=180)
            except subprocess.TimeoutExpired:
                if os.name == "nt":
                    subprocess.run(
                        ["taskkill", "/PID", str(interaction.pid), "/T", "/F"],
                        capture_output=True,
                        check=False,
                    )
                else:
                    os.killpg(interaction.pid, signal.SIGKILL)
                interaction.wait()
                raise AssertionError(
                    "Native suite timed out; its process tree was terminated"
                )
        status["interaction"] = (
            "passed"
            if interaction.returncode == 0
            else ("unavailable" if interaction.returncode == 78 else "failed")
        )
        status["interaction_exit"] = interaction.returncode
        assert interaction.returncode == 0, (
            "Native interaction "
            + status["interaction"]
            + "; see interaction-stderr.txt and interaction/status.json"
        )
    except Exception as error:
        if status["launch"] != "passed":
            status["launch"] = "failed"
        status["error"] = str(error)
    finally:
        (artifacts / "status.json").write_text(json.dumps(status, indent=2))
        print(json.dumps(status))
    return 0 if status["launch"] == status["interaction"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
