#!/usr/bin/env python3
"""Format only rxd workspace members; cargo fmt --all traverses upstream paths."""
import argparse
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    metadata = json.loads(subprocess.check_output(
        ['cargo', 'metadata', '--no-deps', '--format-version', '1'], cwd=ROOT))
    members = set(metadata['workspace_members'])
    command = ['cargo', 'fmt']
    for package in metadata['packages']:
        if package['id'] in members:
            command += ['--package', package['name']]
    if args.check:
        command += ['--', '--check']
    subprocess.run(command, cwd=ROOT, check=True)


if __name__ == '__main__':
    main()
