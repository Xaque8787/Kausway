#!/usr/bin/env python3
"""Bundle project/target-specific JNI dependency notices for the client SDK."""
import argparse
import json
from pathlib import Path
import runpy
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts/releases'))
from versions import version


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    package = runpy.run_path(str(ROOT / 'scripts/packaging/package-development.py'))
    epoch = int(subprocess.check_output(['git', '-C', str(ROOT), 'show', '-s', '--format=%ct', 'HEAD'], text=True))
    with tempfile.TemporaryDirectory(prefix='kausway-sdk-notices-') as temporary:
        tree = Path(temporary) / f'kausway-client-android-{version("client")}-notices'
        tree.mkdir()
        for name in ['LICENSE-MIT', 'LICENSE-APACHE']:
            shutil.copyfile(ROOT / name, tree / name)
        for target in ['aarch64-linux-android', 'armv7-linux-androideabi', 'x86_64-linux-android']:
            raw = subprocess.check_output(['cargo', 'metadata', '--manifest-path', str(ROOT / 'rust/Cargo.toml'),
                                           '--locked', '--format-version', '1', '--filter-platform', target], text=True)
            package['notices'](json.loads(raw), 'android', tree / target, roots=['kausway-client-android'])
        staged = Path(temporary) / 'notices.tar.gz'
        package['archive'](tree, staged, epoch)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(staged, args.output)


if __name__ == '__main__':
    main()
