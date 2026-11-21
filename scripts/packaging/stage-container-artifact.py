#!/usr/bin/env python3
"""Stage a suite from this validation run's verified binary artifact, without rebuilding."""
import argparse
import json
import os
from pathlib import Path
import re
import runpy
import shutil
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts/releases'))
from versions import binary_versions, load_release
sys.path.insert(0, str(ROOT / 'scripts/ci'))
from provenance import ci_context


def check_provenance(manifest, commit, target, allow_dirty=False):
    context = ci_context(commit)
    if manifest['commit'] != commit or manifest['target'] != target or manifest['component'] != 'suite':
        raise ValueError('Suite artifact source/architecture does not match the requested build')
    if allow_dirty and os.environ.get('GITHUB_ACTIONS') == 'true':
        raise ValueError('Dirty source overrides are prohibited in Actions')
    if manifest['source_dirty'] and not allow_dirty:
        raise ValueError('Suite artifact was built from dirty sources')
    if os.environ.get('GITHUB_ACTIONS') == 'true':
        source = manifest['ci']
        if {k: v for k, v in source.items() if k != 'run_attempt'} != {k: v for k, v in context.items() if k != 'run_attempt'}:
            raise ValueError('Suite artifact is not from this exact source and CI run')
        attempt = source.get('run_attempt', '')
        if not re.fullmatch('[1-9][0-9]*', attempt) or int(attempt) > int(context['run_attempt']):
            raise ValueError('Suite artifact has an invalid/future CI attempt')
    if manifest['binary_versions'] != binary_versions(ROOT):
        raise ValueError('Suite artifact versions do not match this checkout')
    expected = {track: value['version'] for track, value in load_release(ROOT).items() if track != 'schema_version'}
    if manifest['versions'] != expected:
        raise ValueError('Suite artifact release manifest does not match this checkout')


def choose_candidate(candidates):
    if not candidates:
        raise ValueError('No verified suite candidates')
    latest = max(candidate[0] for candidate in candidates)
    newest = [candidate for candidate in candidates if candidate[0] == latest]
    if len(newest) != 1:
        raise ValueError('Ambiguous suite artifacts for the same attempt')
    return newest[0]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--artifacts', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--target', choices=['x86_64-unknown-linux-musl', 'aarch64-unknown-linux-musl'], required=True)
    parser.add_argument('--commit', required=True)
    parser.add_argument('--allow-dirty', action='store_true', help='Local uncommitted testing only, forbidden in Actions')
    args = parser.parse_args()
    if args.output.exists():
        parser.error('Output must not already exist')
    archives = list(args.artifacts.rglob(f'kausway-suite-*-{args.target}.tar.gz'))
    if not archives:
        parser.error('No matching suite archive')
    verifier = runpy.run_path(str(ROOT / 'scripts/ci/development-package-smoke.py'))
    package = runpy.run_path(str(ROOT / 'scripts/packaging/package-development.py'))
    with tempfile.TemporaryDirectory(prefix='kausway-suite-artifact-') as temporary:
        candidates = []
        for index, archive in enumerate(archives):
            checksum, filename = archive.with_name(archive.name + '.sha256').read_text().strip().split('  ', 1)
            if filename != archive.name:
                parser.error('Adjacent checksum names another archive')
            tree, manifest = verifier['verify_archive'](archive, checksum, Path(temporary) / str(index))
            check_provenance(manifest, args.commit, args.target, args.allow_dirty)
            candidates.append((int(manifest['ci']['run_attempt'] or '1'), archive, checksum, tree, manifest))
        # A failed-jobs rerun may reuse the successful binary job's earlier artifact.
        # Keep attempts separate and choose the newest verified one, never overwrite by download order.
        _, archive, checksum, tree, manifest = choose_candidate(candidates)
        bindir = args.output / 'bin'
        for binary in manifest['binaries']:
            package['check_binary'](tree / binary, args.target)
        bindir.mkdir(parents=True)
        for binary in manifest['binaries']:
            shutil.copyfile(tree / binary, bindir / binary)
            (bindir / binary).chmod(0o755)
        share = args.output / 'share/kausway'
        share.mkdir(parents=True)
        shutil.copytree(tree / 'licenses', share / 'licenses')
        release = load_release(ROOT)
        info = {'version': release['container']['version'], 'suite_version': release['suite']['version'],
                'selected_suite_version': release['container']['suite_version'], 'revision': args.commit,
                'target': args.target, 'binary_versions': manifest['binary_versions'],
                'binaries': {name: manifest['files'][name] for name in manifest['binaries']},
                'source_artifact': {'name': archive.name, 'sha256': checksum, 'ci': manifest['ci']}}
        (share / 'BUILD-INFO.json').write_text(json.dumps(info, indent=2) + '\n')
        for path in [share, *share.rglob('*')]:
            path.chmod(0o755 if path.is_dir() else 0o644)
    print(f'Staged verified suite artifact {archive.name}')


if __name__ == '__main__':
    main()
