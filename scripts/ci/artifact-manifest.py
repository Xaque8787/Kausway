#!/usr/bin/env python3
"""Inventory an explicitly staged development payload and bind it to its source run."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys

from provenance import ci_context

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts/releases'))
from versions import load_release


def inventory(directory, output_name):
    files = {}
    for path in sorted(directory.rglob('*')):
        if path.is_symlink():
            raise ValueError('Artifact payload cannot contain symlinks')
        if path.is_file() and path != directory / output_name:
            with path.open('rb') as source:
                files[path.relative_to(directory).as_posix()] = hashlib.file_digest(source, 'sha256').hexdigest()
    if not files:
        raise ValueError('Artifact payload is empty')
    return files


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory', type=Path, required=True)
    parser.add_argument('--kind', choices=['android-sdk', 'jellyfin-plugin', 'container'], required=True)
    parser.add_argument('--target', required=True)
    parser.add_argument('--commit', required=True)
    args = parser.parse_args()
    head = subprocess.check_output(['git', '-C', str(ROOT), 'rev-parse', 'HEAD'], text=True).strip()
    if head != args.commit:
        raise ValueError('Requested commit is not this checkout HEAD')
    name = f'BUILD-INFO.{args.kind}.json'
    report = {'format_version': 1, 'development': True, 'kind': args.kind, 'target': args.target,
              'ci': ci_context(args.commit), 'release_versions': load_release(ROOT),
              'source_dirty': bool(subprocess.check_output(
                  ['git', '-C', str(ROOT), 'status', '--porcelain', '--untracked-files=normal'], text=True).strip()),
              'files': inventory(args.directory, name)}
    (args.directory / name).write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
