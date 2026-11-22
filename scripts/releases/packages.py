"""Local release-candidate verification/assembly. No network, tagging or publishing."""
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import runpy
import shutil
import tarfile
import tempfile
import xml.etree.ElementTree as ET
import zipfile

from layout import SUITE, TARGETS, archive_name, assets, groups
from versions import ROOT, load_release, parse_version, plugin_version


def digest(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def safe_name(name):
    path = PurePosixPath(name)
    if path.is_absolute() or '..' in path.parts or '\\' in name or not path.parts:
        raise ValueError('Unsafe payload path')
    return path


def check_source(info, commit, run_id=None, attempt=None, allow_dirty=False):
    ci = info['ci']
    if ci['commit'] != commit or info.get('commit', commit) != commit:
        raise ValueError('Mixed source commits')
    if allow_dirty and os.environ.get('GITHUB_ACTIONS') == 'true':
        raise ValueError('Dirty-source overrides are forbidden in Actions')
    if info['source_dirty'] and not allow_dirty:
        raise ValueError('Dirty source cannot become a release candidate')
    if run_id is not None:
        if ci['run_id'] != str(run_id) or not re.fullmatch('[1-9][0-9]*', str(ci['run_attempt'])):
            raise ValueError('Artifact is from another validation run')
        if int(ci['run_attempt']) > int(attempt):
            raise ValueError('Artifact is from a future attempt')
    if os.environ.get('GITHUB_REPOSITORY') and ci['repository'] != os.environ['GITHUB_REPOSITORY']:
        raise ValueError('Artifact is from another repository')


def newest(items):
    if not items:
        raise ValueError('Missing required input artifact')
    attempt = max(int(info['ci']['run_attempt'] or 1) for _, info in items)
    selected = [(path, info) for path, info in items if int(info['ci']['run_attempt'] or 1) == attempt]
    if len(selected) != 1:
        raise ValueError('Ambiguous input artifacts for one attempt')
    return selected[0]


def binary_tools():
    return (runpy.run_path(str(ROOT / 'scripts/ci/development-package-smoke.py')),
            runpy.run_path(str(ROOT / 'scripts/packaging/package-development.py')))


def read_sums(path):
    result = {}
    for line in path.read_text().splitlines():
        checksum, name = line.split('  ', 1)
        if not re.fullmatch('[0-9a-f]{64}', checksum) or Path(name).name != name or name in result:
            raise ValueError('Invalid checksum inventory')
        result[name] = checksum
    return result


def verify_generic(path, kind, commit, run_id, attempt, allow_dirty):
    info = json.loads(path.read_text())
    if info['format_version'] != 1 or info['kind'] != kind or info['development'] is not True:
        raise ValueError('Invalid input artifact manifest')
    check_source(info, commit, run_id, attempt, allow_dirty)
    actual = {}
    for item in path.parent.rglob('*'):
        if item.is_symlink():
            raise ValueError('Artifact contains a symlink')
        if item.is_file() and item != path:
            actual[item.relative_to(path.parent).as_posix()] = digest(item)
    if actual != info['files']:
        raise ValueError('Input artifact inventory/hash mismatch')
    return info


def zip_files(path):
    with zipfile.ZipFile(path) as archive:
        names = archive.namelist()
        if len(names) != len(set(names)):
            raise ValueError('Duplicate ZIP paths')
        for entry in archive.infolist():
            safe_name(entry.filename)
            if entry.file_size > 256 * 1024 * 1024 or (entry.external_attr >> 16) & 0o170000 == 0o120000:
                raise ValueError('Oversized or linked ZIP entry')
        if sum(entry.file_size for entry in archive.infolist()) > 512 * 1024 * 1024:
            raise ValueError('Oversized ZIP payload')
        return {name: archive.read(name) for name in names if not name.endswith('/')}


def verify_plugin(path, release):
    if parse_version(release['suite']['version']).prerelease:
        raise ValueError('Jellyfin plugin is not distributed with suite prereleases')
    numeric = plugin_version(release)
    folder = f'JellyfinKausway_{numeric}/'
    files = zip_files(path)
    expected = {folder + name for name in ['meta.json', 'Jellyfin.Plugin.Kausway.dll', 'LICENSE-MIT', 'LICENSE-APACHE']}
    if set(files) != expected or not files[folder + 'Jellyfin.Plugin.Kausway.dll']:
        raise ValueError('Incorrect plugin package contents')
    if json.loads(files[folder + 'meta.json'])['version'] != numeric:
        raise ValueError('Plugin metadata version mismatch')


def verify_sdk(directory, release):
    version = release['client']['version']
    base = f'kausway-client-android-{version}'
    files = zip_files(directory / (base + '.aar'))
    expected = {f'jni/{abi}/libkausway_client.so' for abi in ['arm64-v8a', 'armeabi-v7a', 'x86_64']}
    if {name for name in files if name.startswith('jni/') and name.endswith('.so')} != expected:
        raise ValueError('SDK native ABI inventory mismatch')
    for name in [*expected, 'AndroidManifest.xml', 'classes.jar', 'libs/rustls-platform-verifier.jar']:
        if not files.get(name):
            raise ValueError(f'Missing SDK payload: {name}')
    with zipfile.ZipFile(io.BytesIO(files['classes.jar'])) as classes:
        for name in ['KauswayClient', 'NativeBridge', 'ConnectionStatus', 'KauswayConfig']:
            if f'io/github/xaque8787/kausway/{name}.class' not in classes.namelist():
                raise ValueError('SDK Kotlin API classes are missing')
    with zipfile.ZipFile(io.BytesIO(files['libs/rustls-platform-verifier.jar'])) as verifier:
        if 'org/rustls/platformverifier/CertificateVerifier.class' not in verifier.namelist():
            raise ValueError('SDK platform verifier is missing')
    pom = ET.parse(directory / (base + '.pom')).getroot()
    identity = {element.tag.split('}')[-1]: element.text for element in pom}
    if any(identity.get(key) != value for key, value in {'version': version, 'groupId': 'io.github.xaque8787',
           'artifactId': 'kausway-client-android', 'packaging': 'aar'}.items()):
        raise ValueError('SDK POM identity/version mismatch')
    sources = zip_files(directory / (base + '-sources.jar'))
    if not any(name.endswith(('.kt', '.java')) for name in sources):
        raise ValueError('SDK source archive has no API source')
    with tarfile.open(directory / (base + '-notices.tar.gz')) as archive:
        names = [entry.name for entry in archive]
        if len(names) != len(set(names)):
            raise ValueError('Duplicate SDK notice paths')
        for entry in archive.getmembers():
            safe_name(entry.name)
            if not entry.isfile() and not entry.isdir():
                raise ValueError('SDK notices contain a link/device')
        for suffix in ['LICENSE-MIT', 'LICENSE-APACHE', *[f'{target}/THIRD-PARTY.json' for target in
                       ['aarch64-linux-android', 'armv7-linux-androideabi', 'x86_64-linux-android']]]:
            if not any(name.endswith('/' + suffix) for name in names):
                raise ValueError(f'Missing SDK notices: {suffix}')
    if not (directory / (base + '-README.md')).read_text().strip():
        raise ValueError('Missing SDK usage guide')


def write_inventory(directory, track, release, commit, sources):
    wanted = set(assets(track, release))
    if {path.name for path in directory.iterdir()} != wanted:
        raise ValueError('Unexpected/missing release payloads')
    info = {'format_version': 1, 'state': 'candidate', 'track': track, 'version': release[track]['version'],
            'source_dirty': any(source.get('source_dirty', True) for source in sources),
            'prerelease': parse_version(release[track]['version']).prerelease is not None, 'commit': commit,
            'sources': sources, 'assets': {name: {'sha256': digest(directory / name), 'size': (directory / name).stat().st_size}
                                        for name in sorted(wanted)}}
    (directory / 'RELEASE-MANIFEST.json').write_text(json.dumps(info, indent=2) + '\n')
    for name in sorted(wanted):
        (directory / (name + '.sha256')).write_text(f'{info["assets"][name]["sha256"]}  {name}\n')
    sums = [f'{digest(path)}  {path.name}\n' for path in sorted(directory.iterdir())]
    (directory / 'SHA256SUMS').write_text(''.join(sums))


def verify_inventory(directory, track, release, commit=None):
    info = json.loads((directory / 'RELEASE-MANIFEST.json').read_text())
    if info['format_version'] != 1 or info['state'] != 'candidate' or info['track'] != track or info['version'] != release[track]['version']:
        raise ValueError('Release identity mismatch')
    if not re.fullmatch('[0-9a-f]{40}', info['commit']) or commit is not None and info['commit'] != commit:
        raise ValueError('Release source mismatch')
    if info['prerelease'] != (parse_version(info['version']).prerelease is not None):
        raise ValueError('Incorrect prerelease classification')
    wanted = set(assets(track, release))
    expected = wanted | {name + '.sha256' for name in wanted} | {'SHA256SUMS', 'RELEASE-MANIFEST.json'}
    if {path.name for path in directory.iterdir()} != expected or set(info['assets']) != wanted:
        raise ValueError('Unexpected/missing release assets')
    sums = read_sums(directory / 'SHA256SUMS')
    if set(sums) != expected - {'SHA256SUMS'}:
        raise ValueError('Incomplete global checksums')
    for name, checksum in sums.items():
        path = directory / name
        if path.is_symlink() or not path.is_file() or digest(path) != checksum:
            raise ValueError('Release checksum mismatch')
    for name in wanted:
        entry = info['assets'][name]
        if digest(directory / name) != entry['sha256'] or (directory / name).stat().st_size != entry['size']:
            raise ValueError('Release manifest checksum/size mismatch')
        if (directory / (name + '.sha256')).read_text() != f'{entry["sha256"]}  {name}\n':
            raise ValueError('Adjacent release checksum mismatch')
    return info


def verify_candidate(directory, track, release, commit=None):
    info = verify_inventory(directory, track, release, commit)
    verifier, package = binary_tools()
    binary_hashes = {}
    with tempfile.TemporaryDirectory(prefix='kausway-release-check-') as temporary:
        for target in TARGETS:
            for owner, component, binaries in groups(target):
                if owner != track:
                    continue
                name = archive_name(component, release[track]['version'], target)
                tree, manifest = verifier['verify_archive'](directory / name, info['assets'][name]['sha256'], Path(temporary))
                if (manifest['development'] or manifest['commit'] != info['commit'] or manifest['target'] != target
                        or manifest['component'] != component or manifest['base_version'] != info['version']
                        or manifest['versions'] != {track: info['version']}):
                    raise ValueError('Invalid candidate binary identity')
                for binary in binaries:
                    package['check_binary'](tree / binary, target)
                    key = (target, binary)
                    current = digest(tree / binary)
                    if key in binary_hashes and binary_hashes[key] != current:
                        raise ValueError('Combined/individual binary mismatch')
                    binary_hashes[key] = current
        if track == 'suite' and not info['prerelease']:
            verify_plugin(directory / f'JellyfinKausway_{plugin_version(release)}.zip', release)
        if track == 'client':
            verify_sdk(directory, release)
        if track == 'container':
            descriptor = json.loads((directory / assets('container', release)[0]).read_text())
            verify_container_descriptor(descriptor, release, info['commit'])
    return info


def assemble(inputs, output, track, release, commit, run_id=None, attempt=None, allow_dirty=False):
    if track not in ('suite', 'client') or output.exists():
        raise ValueError('Select suite/client and a new output directory')
    verifier, package = binary_tools()
    with tempfile.TemporaryDirectory(prefix='kausway-release-stage-') as temporary:
        temp = Path(temporary)
        staged = temp / 'payloads'
        staged.mkdir()
        sources = []
        for target in TARGETS:
            for owner, component, _ in groups(target):
                if owner != track:
                    continue
                candidates = []
                for index, path in enumerate(inputs.rglob(f'kausway-{component}-*-{target}.tar.gz')):
                    sums = read_sums(path.with_name(path.name + '.sha256'))
                    if set(sums) != {path.name}:
                        raise ValueError('Wrong adjacent binary checksum')
                    tree, manifest = verifier['verify_archive'](path, sums[path.name], temp / f'{target}-{component}-{index}')
                    check_source(manifest, commit, run_id, attempt, allow_dirty)
                    if not manifest['development'] or manifest['release_track'] != track or manifest['versions'] != {track: release[track]['version']}:
                        raise ValueError('Wrong development binary track/version')
                    manifest['_tree'] = str(tree)
                    candidates.append((path, manifest))
                path, manifest = newest(candidates)
                tree = Path(manifest.pop('_tree'))
                name = archive_name(component, release[track]['version'], target)
                new_tree = tree.with_name(name.removesuffix('.tar.gz'))
                tree.rename(new_tree)
                manifest.update({'development': False, 'release_candidate': True, 'build_id': release[track]['version'],
                                 'source_artifact': {'name': path.name, 'sha256': digest(path)}})
                (new_tree / 'README.md').write_text((ROOT / 'scripts/releases/README-distribution.md').read_text())
                manifest['files'] = {p.relative_to(new_tree).as_posix(): digest(p) for p in new_tree.rglob('*')
                                     if p.is_file() and p != new_tree / 'BUILD-INFO.json'}
                (new_tree / 'BUILD-INFO.json').write_text(json.dumps(manifest, indent=2, sort_keys=True) + '\n')
                with tarfile.open(path) as original:
                    epoch = original.getmembers()[0].mtime
                package['archive'](new_tree, staged / name, epoch)
                sources.append({'asset': name, 'input_sha256': digest(path), 'ci': manifest['ci'], 'source_dirty': manifest['source_dirty']})
        kind = 'jellyfin-plugin' if track == 'suite' else 'android-sdk'
        if track == 'client' or parse_version(release['suite']['version']).prerelease is None:
            candidates = []
            for path in inputs.rglob(f'BUILD-INFO.{kind}.json'):
                info = verify_generic(path, kind, commit, run_id, attempt, allow_dirty)
                candidates.append((path, info))
            source, metadata = newest(candidates)
            expected_extra = [name for name in assets(track, release) if not any(name.endswith(f'-{target}.tar.gz') for target in TARGETS)]
            for name in expected_extra:
                if name not in metadata['files']:
                    raise ValueError(f'Missing companion release payload: {name}')
                shutil.copyfile(source.parent / name, staged / name)
            sources.append({'kind': kind, 'ci': metadata['ci'], 'input_manifest_sha256': digest(source), 'source_dirty': metadata['source_dirty']})
        write_inventory(staged, track, release, commit, sources)
        verify_candidate(staged, track, release, commit)
        output.parent.mkdir(parents=True, exist_ok=True)
        shutil.copytree(staged, output)


def verify_container_descriptor(info, release, commit):
    version = release['container']['version']
    if info['format_version'] != 1 or not re.fullmatch(r'ghcr\.io/[a-z0-9_.-]+/[a-z0-9_.-]+', info['image']):
        raise ValueError('Invalid container image reference/descriptor format')
    if not re.fullmatch('[0-9a-f]{64}', info['suite_manifest_sha256']):
        raise ValueError('Missing exact suite release inventory digest')
    if info['version'] != version or info['suite_version'] != release['container']['suite_version'] or info['commit'] != commit:
        raise ValueError('Container release identity mismatch')
    if not re.fullmatch('[0-9a-f]{40}', info['suite_commit']):
        raise ValueError('Missing suite source identity')
    if not re.fullmatch('sha256:[0-9a-f]{64}', info['index_digest']):
        raise ValueError('Invalid image index digest')
    if set(info['platforms']) != {'linux/amd64', 'linux/arm64'}:
        raise ValueError('Incomplete container platform set')
    if any(not re.fullmatch('sha256:[0-9a-f]{64}', digest) for digest in info['platforms'].values()):
        raise ValueError('Invalid platform image digest')
    wanted = {version, f'sha-{commit}'}
    if parse_version(version).prerelease is None:
        wanted.add('latest')
    if set(info['tags']) != wanted or any(value != info['index_digest'] for value in info['tags'].values()):
        raise ValueError('Container tags must identify one tested index; prereleases cannot move latest')
