#!/usr/bin/env python3
"""Consume a verified selected suite release directory; never rebuild its binaries."""
import argparse
import copy
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts/releases'))
from layout import SUITE, archive_name
from packages import binary_tools, digest, verify_candidate
from versions import load_release


def stage(directory, output, target, container_commit, allow_dirty=False):
    release = load_release()
    selected = copy.deepcopy(release)
    selected['suite']['version'] = release['container']['suite_version']
    info = verify_candidate(directory, 'suite', selected)
    if output.exists():
        raise ValueError('Output directory already exists')
    if allow_dirty and os.environ.get('GITHUB_ACTIONS') == 'true':
        raise ValueError('Dirty override forbidden in Actions')
    if info['source_dirty'] and not allow_dirty:
        raise ValueError('Selected suite was built from dirty sources')
    name = archive_name('suite', selected['suite']['version'], target)
    if name not in info['assets']:
        raise ValueError('Selected suite does not support the requested image target')
    verifier, _ = binary_tools()
    with tempfile.TemporaryDirectory(prefix='kausway-selected-suite-') as temporary:
        tree, binary = verifier['verify_archive'](directory / name, info['assets'][name]['sha256'], Path(temporary))
        bindir, share = output / 'bin', output / 'share/kausway'
        bindir.mkdir(parents=True)
        share.mkdir(parents=True)
        for executable in SUITE:
            shutil.copyfile(tree / executable, bindir / executable)
            (bindir / executable).chmod(0o755)
        shutil.copytree(tree / 'licenses', share / 'licenses')
        metadata = {'version': release['container']['version'], 'suite_version': selected['suite']['version'],
                    'revision': container_commit, 'suite_source_commit': info['commit'], 'target': target,
                    'binary_versions': binary['binary_versions'], 'binaries': {name: binary['files'][name] for name in SUITE},
                    'source_release': {'manifest_sha256': digest(directory / 'RELEASE-MANIFEST.json'),
                                       'asset': name, 'sha256': info['assets'][name]['sha256']}}
        (share / 'BUILD-INFO.json').write_text(json.dumps(metadata, indent=2) + '\n')
        for path in [share, *share.rglob('*')]:
            path.chmod(0o755 if path.is_dir() else 0o644)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--release', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--target', choices=['x86_64-unknown-linux-musl', 'aarch64-unknown-linux-musl'], required=True)
    parser.add_argument('--commit', required=True, help='Exact container source commit')
    parser.add_argument('--allow-dirty', action='store_true', help='Local testing only')
    args = parser.parse_args()
    import re
    if not re.fullmatch('[0-9a-f]{40}', args.commit):
        parser.error('Expected full container commit')
    if subprocess.check_output(['git', '-C', str(ROOT), 'rev-parse', 'HEAD'], text=True).strip() != args.commit:
        parser.error('Container commit must match this source checkout HEAD')
    stage(args.release, args.output, args.target, args.commit, args.allow_dirty)


if __name__ == '__main__':
    main()
