#!/usr/bin/env python3
"""Require the initialized Kuna pin to match the newest formal upstream tag."""

from pathlib import Path
import subprocess
import sys

if __name__ == "__main__":
    sys.exit(subprocess.call([
        sys.executable, str(Path(__file__).with_name("check-upstream.py")),
        "--source", "kuna", "--require-checkout", "--require-current",
    ]))
