"""Expected release contents, independent of publication and GitHub APIs."""
from versions import parse_version

TARGETS = ('x86_64-unknown-linux-musl', 'aarch64-unknown-linux-musl',
           'aarch64-linux-android', 'armv7-linux-androideabi')
SUITE = ['kausway', 'kausway-host', 'kausway-admin']
CLIENT = ['kausway-test-client']


def groups(target):
    if target not in (*TARGETS, 'x86_64-unknown-linux-gnu'):
        raise ValueError('Unsupported binary target')
    if 'android' in target:
        return [('suite', 'host', ['kausway-host']), ('client', 'test-client', CLIENT)]
    return [('suite', 'suite', SUITE), ('suite', 'manager', ['kausway']),
            ('suite', 'host', ['kausway-host']), ('suite', 'admin', ['kausway-admin']),
            ('client', 'test-client', CLIENT)]


def archive_name(component, version, target):
    return f'kausway-{component}-{version}-{target}.tar.gz'


def assets(track, release):
    version = release[track]['version']
    parse_version(version)
    if track == 'container':
        return [f'kausway-container-{version}.json']
    result = [archive_name(component, version, target) for target in TARGETS
              for owner, component, _ in groups(target) if owner == track]
    if track == 'suite' and parse_version(version).prerelease is None:
        from versions import plugin_version
        result.append(f'JellyfinKausway_{plugin_version(release)}.zip')
    if track == 'client':
        base = f'kausway-client-android-{version}'
        result += [base + extension for extension in ['.aar', '.pom', '-sources.jar', '-notices.tar.gz', '-README.md']]
    return sorted(result)
