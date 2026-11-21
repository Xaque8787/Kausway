#!/usr/bin/env python3
"""Package already-built development executables without touching release packaging.

Requires Python 3.12+, git, and readelf. Metadata must come from cargo metadata
--locked --format-version 1 --filter-platform TARGET for this checkout.
"""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import tarfile
import tempfile
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'releases'))
from versions import binary_versions, check_metadata, load_release

ROOT = Path(__file__).resolve().parents[2]
TARGETS = {
    'x86_64-unknown-linux-musl': ('linux', 2, 62, 'x86-64', 'static musl'),
    'aarch64-unknown-linux-musl': ('linux', 2, 183, 'ARM64', 'static musl'),
    'aarch64-linux-android': ('android', 2, 183, 'arm64-v8a', 'Android API 24+'),
    'armv7-linux-androideabi': ('android', 1, 40, 'armeabi-v7a', 'Android API 24+'),
    # Local packaging validation only; not a published work-workflow target.
    'x86_64-unknown-linux-gnu': ('linux', 2, 62, 'x86-64', 'glibc (local validation)'),
}
BINARIES = {'linux': ['kausway', 'kausway-host', 'kausway-admin', 'kausway-test-client'],
            'android': ['kausway-host', 'kausway-test-client']}
ROOT_PACKAGES = {'linux': ['kausway-host', 'kausway-manager', 'kausway-admin'], 'android': ['kausway-host']}
DOCS = ['docs/architecture/configuration-and-storage.md', 'docs/android/storage.md',
        'docs/operations/admin.md', 'docs/operations/manager.md', 'docs/operations/host-administration.md',
        'docs/operations/cli-reference.md', 'docs/operations/development-binaries.md',
        'docs/architecture/transport-policies.md', 'docs/integrations/application-suite.md',
        'docs/integrations/docker-suite.md',
        'protocol/control-v1.md', 'protocol/transport-v1.md']
EXAMPLES = ['examples/application-integration/README.md', 'examples/application-integration/application.py',
            'examples/application-integration/kausway.py', 'examples/docker-integration/Dockerfile',
            'examples/docker-integration/entrypoint.sh', 'examples/docker-integration/supervise.py']


def sha256(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def git(*args):
    return subprocess.check_output(['git', '-C', str(ROOT), *args], text=True).strip()


def check_binary(path, target):
    expected = TARGETS[target]
    with path.open('rb') as source:
        header = source.read(64)
    if len(header) < 64 or header[:4] != b'\x7fELF' or header[5] != 1:
        raise ValueError(f'{path.name}: expected a little-endian ELF executable')
    machine = struct.unpack_from('<H', header, 18)[0]
    if struct.unpack_from('<H', header, 16)[0] not in (2, 3) or (header[4], machine) != expected[1:3] or not path.stat().st_mode & 0o111:
        raise ValueError(f'{path.name}: executable architecture/mode does not match {target}')
    report = subprocess.check_output(['readelf', '--program-headers', '--dynamic', str(path)],
                                     text=True, env={**os.environ, 'LC_ALL': 'C'})
    if target.endswith('-musl') and ('INTERP' in report or '(NEEDED)' in report):
        raise ValueError(f'{path.name}: musl distribution requires a static binary')
    if expected[0] == 'android' and '/system/bin/linker' not in report:
        raise ValueError(f'{path.name}: expected an Android executable interpreter')


def notices(metadata, platform, destination):
    packages = {p['id']: p for p in metadata['packages']}
    nodes = {n['id']: n for n in metadata['resolve']['nodes']}
    pending = [key for key, p in packages.items() if p['name'] in ROOT_PACKAGES[platform]]
    if len(pending) != len(ROOT_PACKAGES[platform]):
        raise ValueError('metadata does not contain the required binary packages')
    seen = set()
    while pending:
        key = pending.pop()
        if key in seen:
            continue
        seen.add(key)
        pending += [d['pkg'] for d in nodes[key]['deps'] if any(k['kind'] != 'dev' for k in d['dep_kinds'])]
    inventory = []
    identities = set()
    for package in sorted((packages[key] for key in seen if packages[key]['source']), key=lambda p: (p['name'], p['version'])):
        identity = (package['name'], package['version'])
        if identity in identities:
            raise ValueError('Duplicate dependency name/version from different sources')
        identities.add(identity)
        source = Path(package['manifest_path']).parent.resolve()
        folder = destination / 'third-party' / f"{package['name']}-{package['version']}"
        files = {p for p in source.rglob('*') if p.is_file() and p.name.upper().startswith(('LICENSE', 'LICENCE', 'COPYING', 'COPYRIGHT', 'NOTICE'))}
        if package.get('license_file'):
            files.add(source / package['license_file'])
        recorded = []
        supplement = ROOT / 'scripts/packaging/notices' / f"{package['name']}-{package['version']}"
        if supplement.is_dir():
            folder.mkdir(parents=True, exist_ok=True)
            for notice in sorted(supplement.iterdir()):
                shutil.copyfile(notice, folder / notice.name)
                recorded.append(notice.name)
        for notice in sorted(files):
            if not notice.resolve().is_relative_to(source) or notice.stat().st_size > 2 * 1024 * 1024:
                raise ValueError(f"Unsafe or oversized notice in {package['name']}")
            relative = notice.relative_to(source)
            target = folder / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(notice, target)
            recorded.append(relative.as_posix())
        fallback = None
        if not recorded:
            # These crates publish no notice file but expressly allow Apache-2.0.
            if package['license'] not in ['MIT OR Apache-2.0', 'Apache-2.0 OR MIT', 'Apache-2.0']:
                raise ValueError(f"No bundled notice or supported license fallback: {package['name']}")
            folder.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / 'LICENSE-APACHE', folder / 'LICENSE-APACHE')
            recorded = ['LICENSE-APACHE']
            fallback = 'Apache-2.0 selected from declared license; crate archive supplied no license file'
        record = {key: package.get(key) for key in ['name', 'version', 'license', 'authors', 'repository']}
        record.update({'notice_files': recorded, 'fallback': fallback})
        (folder / 'PACKAGE.json').write_text(json.dumps(record, indent=2) + '\n')
        inventory.append(record)
    destination.mkdir(parents=True, exist_ok=True)
    (destination / 'THIRD-PARTY.json').write_text(json.dumps(inventory, indent=2) + '\n')
    lines = ['# Third-party dependency notices', '',
             'Cargo dependency inventory for the selected target; includes build-time dependencies',
             'conservatively and excludes dev-only dependencies. Upstream notice files are bundled.',
             'For packages with no supplied license file, Apache-2.0 is selected only when explicitly',
             'allowed by their declared license expression; those entries are marked below.', '',
             '| Package | Version | Declared license | Notice selection |', '| --- | --- | --- | --- |']
    for p in inventory:
        lines.append(f"| {p['name']} | {p['version']} | {p['license']} | {'Apache-2.0 fallback' if p['fallback'] else 'Upstream files'} |")
    (destination / 'THIRD-PARTY-NOTICES.md').write_text('\n'.join(lines) + '\n')


def archive(tree, output, epoch):
    with output.open('xb') as raw:
        with gzip.GzipFile(filename='', mode='wb', fileobj=raw, mtime=epoch, compresslevel=6) as compressed:
            with tarfile.open(fileobj=compressed, mode='w', format=tarfile.PAX_FORMAT) as tar:
                for path in [tree, *sorted(tree.rglob('*'))]:
                    if path.is_symlink():
                        raise ValueError('Symlinks are not part of development packages')
                    info = tar.gettarinfo(str(path), arcname=path.relative_to(tree.parent).as_posix())
                    info.uid = info.gid = 0
                    info.uname = info.gname = ''
                    info.mtime = epoch
                    info.mode = 0o755 if path.is_dir() or path.name in BINARIES['linux'] else 0o644
                    if path.is_file():
                        with path.open('rb') as content:
                            tar.addfile(info, content)
                    else:
                        tar.addfile(info)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary-dir', type=Path, required=True)
    parser.add_argument('--metadata', type=Path, required=True)
    parser.add_argument('--target', choices=TARGETS, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--commit', required=True)
    args = parser.parse_args()
    if not re.fullmatch('[0-9a-f]{40}', args.commit) or args.commit != git('rev-parse', 'HEAD'):
        parser.error('--commit must be the exact full HEAD commit')
    releases = load_release(ROOT)
    expected_versions = binary_versions(ROOT)
    base = releases['suite']['version']
    epoch = int(git('show', '-s', '--format=%ct', args.commit))
    metadata = json.loads(args.metadata.read_text())
    if Path(metadata['workspace_root']).resolve() != ROOT / 'rust':
        parser.error('metadata belongs to another workspace')
    platform, _, _, architecture, runtime = TARGETS[args.target]
    binaries = BINARIES[platform]
    for binary in binaries:
        check_binary(args.binary_dir / binary, args.target)
    check_metadata(metadata, ROOT)
    args.output.mkdir(parents=True, exist_ok=True)
    existing = list(args.output.iterdir())
    if existing:
        parser.error('output directory must be empty; existing artifacts are never overwritten')
    # Only named, committed documentation/templates/notices enter the staging tree.
    with tempfile.TemporaryDirectory(prefix='kausway-package-') as temporary:
        temp = Path(temporary)
        common = temp / 'common'
        common.mkdir()
        shutil.copyfile(ROOT / 'scripts/packaging/development/README.md', common / 'README.md')
        shutil.copyfile(ROOT / 'scripts/packaging/development/config.example.toml', common / 'config.example.toml')
        for relative in DOCS + EXAMPLES:
            source = ROOT / relative
            if not source.is_file():
                raise ValueError(f'Missing distribution documentation: {relative}')
            target = common / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, target)
        licenses = common / 'licenses'
        licenses.mkdir()
        for name in ['LICENSE-MIT', 'LICENSE-APACHE']:
            shutil.copyfile(ROOT / name, licenses / name)
        notices(metadata, platform, licenses)
        groups = [('suite' if platform == 'linux' else 'android-cli', binaries)]
        groups += [('manager' if name == 'kausway' else name.removeprefix('kausway-'), [name]) for name in binaries]
        sums = []
        dirty = bool(git('status', '--porcelain', '--untracked-files=normal'))
        for component, included in groups:
            component_version = expected_versions[included[0]] if len(included) == 1 else base
            build_id = f'{component_version}-dev.g{args.commit[:12]}'
            name = f'kausway-{component}-{build_id}-{args.target}'
            tree = temp / name
            shutil.copytree(common, tree)
            for binary in included:
                shutil.copyfile(args.binary_dir / binary, tree / binary)
            files = {p.relative_to(tree).as_posix(): sha256(p) for p in sorted(tree.rglob('*')) if p.is_file()}
            manifest = {'format_version': 1, 'development': True, 'base_version': component_version, 'build_id': build_id,
                        'versions': {track: values['version'] for track, values in releases.items() if track != 'schema_version'},
                        'binary_versions': {name: expected_versions[name] for name in included},
                        'commit': args.commit, 'source_dirty': dirty, 'target': args.target, 'component': component,
                        'platform': platform, 'architecture': architecture, 'runtime': runtime,
                        'minimum_android_api': 24 if platform == 'android' else None,
                        'binaries': included, 'files': files}
            (tree / 'BUILD-INFO.json').write_text(json.dumps(manifest, indent=2, sort_keys=True) + '\n')
            path = args.output / f'{name}.tar.gz'
            archive(tree, path, epoch)
            checksum = f'{sha256(path)}  {path.name}\n'
            path.with_name(path.name + '.sha256').write_text(checksum)
            sums.append(checksum)
        (args.output / 'SHA256SUMS').write_text(''.join(sums))
    print(f'Packaged {len(sums)} development archives for {args.target}: suite {base}, client {releases["client"]["version"]}')


if __name__ == '__main__':
    main()
