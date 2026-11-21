#!/usr/bin/env python3
"""Compile deliberately different track versions in a disposable source checkout."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

from versions import ROOT, planned_updates


def main():
    with tempfile.TemporaryDirectory(prefix='kausway-version-build-') as temporary:
        root = Path(temporary)
        shutil.copytree(ROOT / 'rust', root / 'rust', ignore=shutil.ignore_patterns('target'))
        shutil.copytree(ROOT / 'scripts/releases', root / 'scripts/releases', ignore=shutil.ignore_patterns('__pycache__'))
        for relative in ['releases.toml', 'sdks/android/release-version.properties',
                         'integrations/jellyfin/meta.json', 'integrations/jellyfin/plugin/Jellyfin.Plugin.Kausway.csproj']:
            target = root / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / relative, target)
        # Fixture identity is intentional and independent of current release versions.
        (root / 'releases.toml').write_text('schema_version=1\n[suite]\nversion="0.51.0-rc.1"\n'
                                           '[client]\nversion="0.62.0"\n[container]\n'
                                           'version="0.13.0"\nsuite_version="0.4.0"\n')
        for path, text in planned_updates(root).items():
            path.write_text(text)
        env = os.environ.copy()
        # Keep fixture-version executables out of the developer's normal build output.
        target = Path(env.get('CARGO_TARGET_DIR', ROOT / 'rust/target')).resolve() / 'version-acceptance'
        env['CARGO_TARGET_DIR'] = str(target)
        subprocess.run(['cargo', 'build', '--manifest-path', str(root / 'rust/Cargo.toml'), '--locked', '--offline',
                        '-p', 'kausway-host', '-p', 'kausway-admin', '-p', 'kausway-manager', '--bins'], env=env, check=True)
        for binary in ['kausway', 'kausway-host', 'kausway-admin', 'kausway-test-client']:
            wanted = '0.62.0' if binary == 'kausway-test-client' else '0.51.0-rc.1'
            actual = subprocess.check_output([str(target / 'debug' / binary), '--version'], text=True).strip()
            assert actual == f'{binary} {wanted}', actual
        subprocess.run(['cargo', 'metadata', '--manifest-path', str(root / 'rust/Cargo.toml'), '--locked',
                        '--offline', '--no-deps', '--format-version', '1'], env=env, check=True, stdout=subprocess.DEVNULL)
    print('Divergent suite/client versions compile, report correctly, and retain a valid locked workspace')


if __name__ == '__main__':
    main()
