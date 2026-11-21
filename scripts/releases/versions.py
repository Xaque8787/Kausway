"""Shared manifest, component registration and surgical version synchronization."""
import json
from pathlib import Path
import re
import sys
import tomllib

sys.path.insert(0, str(Path(__file__).resolve().parent / '_vendor'))
from semver import Version  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
TRACKS = ('suite', 'client', 'container')


def parse_version(value):
    if not isinstance(value, str):
        raise ValueError('Release versions must be strings')
    parsed = Version.parse(value)
    if parsed.build is not None:
        raise ValueError('Release versions cannot contain build metadata; record build identity separately')
    return parsed


def load_release(root=ROOT):
    data = tomllib.loads((root / 'releases.toml').read_text())
    if set(data) != {'schema_version', *TRACKS} or data['schema_version'] != 1:
        raise ValueError('Unknown or missing release tracks/schema')
    for track in TRACKS:
        expected = {'version', 'suite_version'} if track == 'container' else {'version'}
        if set(data[track]) != expected:
            raise ValueError(f'Unknown or missing fields in release track {track}')
        parse_version(data[track]['version'])
    parse_version(data['container']['suite_version'])
    # The selected suite can be an older published release; do not force equality.
    return data


def version(track='suite', root=ROOT):
    return load_release(root)[track]['version']


def plugin_version(release):
    parsed = parse_version(release['suite']['version'])
    if any(number > 65534 for number in (parsed.major, parsed.minor, parsed.patch)):
        raise ValueError('Suite version exceeds the Jellyfin/.NET assembly component range (0..65534)')
    return f'{parsed.major}.{parsed.minor}.{parsed.patch}.0'


def plugin_distributable(release):
    return parse_version(release['suite']['version']).prerelease is None


def inside(root, relative):
    path = root / relative
    if not path.resolve().is_relative_to(root.resolve()):
        raise ValueError(f'Component path escapes repository: {relative}')
    return path


def registration(root=ROOT):
    registry = tomllib.loads((root / 'scripts/releases/components.toml').read_text())
    if set(registry) != {'schema_version', 'rust_packages', 'binaries', 'jellyfin', 'sdk_versions'} or registry['schema_version'] != 1:
        raise ValueError('Unknown or missing component registration fields/schema')
    for group in ['rust_packages', 'binaries']:
        if not registry[group] or any(track not in ('suite', 'client') for track in registry[group].values()):
            raise ValueError(f'Invalid track in {group}')
    binaries = {'kausway': 'suite', 'kausway-host': 'suite', 'kausway-admin': 'suite', 'kausway-test-client': 'client'}
    if set(registry['binaries']) != set(binaries):
        raise ValueError('Missing or unknown binary registration')
    for name, track in binaries.items():
        if registry['binaries'].get(name) != track:
            raise ValueError(f'Missing or incorrect binary registration: {name}')
    for name, track in {'kausway-host': 'suite', 'kausway-admin': 'suite', 'kausway-manager': 'suite',
                        'kausway-client': 'client', 'kausway-client-android': 'client'}.items():
        if registry['rust_packages'].get(name) != track:
            raise ValueError(f'Missing or incorrect package registration: {name}')
    if set(registry['jellyfin']) != {'project', 'metadata'}:
        raise ValueError('Invalid Jellyfin registration')
    names, paths = set(), set()
    for sdk in registry['sdk_versions']:
        if set(sdk) != {'name', 'track', 'format', 'path', 'key'} or sdk['track'] != 'client' or sdk['format'] != 'properties':
            raise ValueError('Invalid SDK registration or unsupported metadata format')
        if sdk['name'] in names or sdk['path'] in paths:
            raise ValueError('Duplicate SDK registration')
        names.add(sdk['name'])
        paths.add(sdk['path'])
        inside(root, sdk['path'])
    if 'android' not in names:
        raise ValueError('Missing Android SDK registration')
    return registry


def binary_versions(root=ROOT):
    releases, registry = load_release(root), registration(root)
    return {name: releases[track]['version'] for name, track in registry['binaries'].items()}


def check_metadata(metadata, root=ROOT):
    if Path(metadata['workspace_root']).resolve() != (root / 'rust').resolve():
        raise ValueError('Metadata belongs to another workspace')
    releases, tracks = load_release(root), registration(root)['rust_packages']
    found = set()
    for package in metadata['packages']:
        name = package['name']
        if name in tracks and package['source'] is None:
            found.add(name)
            if package['version'] != releases[tracks[name]]['version']:
                raise ValueError(f'Package metadata version disagrees with releases.toml: {name}')
    if found != set(tracks):
        raise ValueError('Metadata is missing registered workspace packages')


def replace_one(text, pattern, replacement, label):
    result, count = re.subn(pattern, lambda match: replacement(match), text, flags=re.MULTILINE)
    if count != 1:
        raise ValueError(f'{label}: expected exactly one version field, found {count}')
    return result


def set_assignment(text, field, value, label):
    return replace_one(text, rf'^{re.escape(field)}\s*=\s*[^\n]+$',
                       lambda _: f'{field} = "{value}"', label)


def planned_updates(root=ROOT, scope='all'):
    release, registry = load_release(root), registration(root)
    updates = {}

    def selected(track):
        return scope == 'all' or scope == track

    def edit(relative, transform):
        path = inside(root, relative)
        original = path.read_text()
        revised = transform(original)
        if original != revised:
            updates[path] = revised

    workspace = root / 'rust/Cargo.toml'
    workspace_data = tomllib.loads(workspace.read_text())
    if selected('suite'):
        def update_workspace(text):
            return replace_one(text, r'(\[workspace\.package\]\s*\n)([\s\S]*?)(?=^\[|\Z)',
                               lambda m: m[1] + set_assignment(m[2], 'version', release['suite']['version'], workspace), workspace)
        edit('rust/Cargo.toml', update_workspace)

    packages = {}
    for member in workspace_data['workspace']['members']:
        path = inside(root, 'rust/' + member + '/Cargo.toml')
        package = tomllib.loads(path.read_text())['package']
        if package['name'] in packages:
            raise ValueError('Duplicate local Rust package')
        packages[package['name']] = path
    if set(packages) != set(registry['rust_packages']):
        raise ValueError('Missing or unknown Rust package registration')
    for name, path in packages.items():
        track = registry['rust_packages'][name]
        if selected(track):
            wanted = release[track]['version']
            edit(str(path.relative_to(root)), lambda text, value=wanted: replace_one(
                text, r'^(?:version\.workspace\s*=\s*true|version\s*=\s*"[^"]+")\s*$',
                lambda _: f'version = "{value}"', path))

    # Only local package version fields change. Third-party packages, checksums,
    # and dependency requirements are left byte-for-byte intact.
    def update_lock(text):
        found = set()

        def package_block(match):
            block = match[0]
            data = tomllib.loads(block)['package'][0]
            name = data['name']
            if name not in registry['rust_packages'] or 'source' in data:
                return block
            if name in found:
                raise ValueError(f'Duplicate local lockfile entry: {name}')
            found.add(name)
            track = registry['rust_packages'][name]
            if not selected(track):
                return block
            return set_assignment(block, 'version', release[track]['version'], name)

        result = re.sub(r'^\[\[package\]\]\n[\s\S]*?(?=^\[\[package\]\]|\Z)', package_block, text, flags=re.MULTILINE)
        if found != set(packages):
            raise ValueError('Missing local lockfile entries; resolve the workspace before synchronizing')
        return result
    edit('rust/Cargo.lock', update_lock)

    if selected('suite'):
        numeric = plugin_version(release)
        def update_project(text):
            text = replace_one(text, r'<Version>[^<]+</Version>',
                               lambda _: f'<Version>{release["suite"]["version"]}</Version>', 'Jellyfin Version')
            for field in ['AssemblyVersion', 'FileVersion']:
                text = replace_one(text, rf'<{field}>[^<]+</{field}>',
                                   lambda _, tag=field: f'<{tag}>{numeric}</{tag}>', field)
            return text
        edit(registry['jellyfin']['project'], update_project)
        def update_metadata(text):
            data = json.loads(text)
            if 'version' not in data:
                raise ValueError('Missing Jellyfin metadata version')
            data['version'] = numeric
            return json.dumps(data, indent=2) + '\n'
        edit(registry['jellyfin']['metadata'], update_metadata)

    if selected('client'):
        for sdk in registry['sdk_versions']:
            edit(sdk['path'], lambda text, sdk=sdk: replace_one(
                text, rf'^{re.escape(sdk["key"])}=[^\n]*$',
                lambda _: f'{sdk["key"]}={release["client"]["version"]}', sdk['name']))
    return updates
