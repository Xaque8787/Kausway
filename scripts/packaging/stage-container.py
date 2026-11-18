#!/usr/bin/env python3
"""Stage verified static executables and notices for the suite image."""
import argparse
import json
from pathlib import Path
import runpy
import shutil
import tomllib

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary-dir', type=Path, required=True)
    parser.add_argument('--metadata', type=Path, required=True)
    parser.add_argument('--target', choices=['x86_64-unknown-linux-musl', 'aarch64-unknown-linux-musl'], required=True)
    parser.add_argument('--revision', default='unknown')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        parser.error('output must not already exist')
    package = runpy.run_path(str(ROOT / 'scripts/packaging/package-development.py'))
    for binary in package['BINARIES']['linux']:
        package['check_binary'](args.binary_dir / binary, args.target)
    metadata = json.loads(args.metadata.read_text())
    if Path(metadata['workspace_root']).resolve() != ROOT / 'rust':
        parser.error('metadata belongs to another workspace')
    version = tomllib.loads((ROOT / 'rust/Cargo.toml').read_text())['workspace']['package']['version']
    bindir = args.output / 'bin'
    bindir.mkdir(parents=True)
    for binary in package['BINARIES']['linux']:
        shutil.copyfile(args.binary_dir / binary, bindir / binary)
        (bindir / binary).chmod(0o755)
    share = args.output / 'share/kausway'
    licenses = share / 'licenses'
    licenses.mkdir(parents=True)
    for name in ['LICENSE-MIT', 'LICENSE-APACHE']:
        shutil.copyfile(ROOT / name, licenses / name)
    package['notices'](metadata, 'linux', licenses)
    (share / 'BUILD-INFO.json').write_text(json.dumps({
        'version': version, 'revision': args.revision, 'target': args.target,
        'binaries': {name: package['sha256'](bindir / name) for name in package['BINARIES']['linux']},
    }, indent=2) + '\n')
    for path in share.rglob('*'):
        path.chmod(0o755 if path.is_dir() else 0o644)
    share.chmod(0o755)


if __name__ == '__main__':
    main()
