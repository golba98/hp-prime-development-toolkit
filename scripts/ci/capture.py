"""Capture a single CI command without hiding its status or changing its arguments."""

import subprocess
import sys
from pathlib import Path


def main():
    log = Path(sys.argv[1])
    args = sys.argv[2:]
    if args and args[0] == "--":
        args = args[1:]
    log.parent.mkdir(parents=True, exist_ok=True)
    with log.open("wb") as output:
        child = subprocess.Popen(args, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        for line in iter(child.stdout.readline, b""):
            output.write(line)
            output.flush()
            sys.stdout.buffer.write(line)
            sys.stdout.buffer.flush()
        return child.wait()


if __name__ == "__main__":
    sys.exit(main())
