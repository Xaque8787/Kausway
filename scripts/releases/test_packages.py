"""Release policy and end-to-end packaging fixtures; fixture ELF files are never executed."""
import copy
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import struct
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import zipfile

from layout import SUITE, TARGETS, archive_name, assets, groups
from packages import (assemble, binary_tools, digest, verify_candidate, verify_container_descriptor,
                      verify_inventory)
from versions import ROOT, plugin_version


def jar(entries):
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, 'w') as output:
        for name in entries:
            output.writestr(name, b'fixture')
    return buffer.getvalue()


def elf(target):
    short = target == 'armv7-linux-androideabi'
    android = 'android' in target
    machine = 40 if short else 62 if target.startswith('x86_64') else 183
    ident = b'\x7fELF' + bytes([1 if short else 2, 1, 1]) + bytes(9)
    interpreter = b'/system/bin/linker\0' if short else b'/system/bin/linker64\0'
    if short:
        header = struct.pack('<16sHHIIIIIHHHHHH', ident, 2, machine, 1, 0, 52, 0, 0, 52, 32, 1, 0, 0, 0)
        program = struct.pack('<IIIIIIII', 3, 84, 0, 0, len(interpreter), len(interpreter), 4, 1)
    else:
        header = struct.pack('<16sHHIQQQIHHHHHH', ident, 2, machine, 1, 0, 64 if android else 0, 0, 0, 64, 56, int(android), 0, 0, 0)
        program = struct.pack('<IIQQQQQQ', 3, 4, 120, 0, 0, len(interpreter), len(interpreter), 1)
    return header + (program + interpreter if android else b'')


class PackageTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='kausway-package-tests-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.inputs = self.root / 'inputs'
        self.inputs.mkdir()
        self.release = {'schema_version': 1, 'suite': {'version': '0.5.0'}, 'client': {'version': '0.6.0'},
                        'container': {'version': '0.2.0', 'suite_version': '0.5.0'}}
        self.sha = 'a' * 40
        self.ci = {'commit': self.sha, 'repository': os.environ.get('GITHUB_REPOSITORY'),
                   'ref': 'refs/heads/topic', 'event': 'push', 'run_id': '11', 'run_attempt': '1'}
        self.verifier, self.package = binary_tools()

    def generic(self, directory, kind):
        info = {'format_version': 1, 'kind': kind, 'development': True, 'ci': self.ci,
                'source_dirty': False, 'release_versions': self.release,
                'files': {p.relative_to(directory).as_posix(): digest(p) for p in directory.rglob('*') if p.is_file()}}
        (directory / f'BUILD-INFO.{kind}.json').write_text(json.dumps(info))

    def fixtures(self):
        for target in TARGETS:
            for track, component, binaries in groups(target):
                version = self.release[track]['version']
                build = f'{version}-dev.g{self.sha[:12]}'
                name = archive_name(component, build, target)
                tree = self.root / 'trees' / name.removesuffix('.tar.gz')
                (tree / 'licenses').mkdir(parents=True)
                (tree / 'licenses/THIRD-PARTY.json').write_text('[{"name":"fixture"}]')
                for license in ['LICENSE-MIT', 'LICENSE-APACHE']:
                    (tree / 'licenses' / license).write_text('fixture notice')
                (tree / 'README.md').write_text('fixture')
                (tree / 'config.example.toml').write_text('schema_version=1\n[manager]\nhost_enabled=false\n')
                for binary in binaries:
                    (tree / binary).write_bytes(elf(target))
                    (tree / binary).chmod(0o755)
                info = {'format_version': 1, 'development': True, 'base_version': version, 'build_id': build,
                        'release_track': track, 'versions': {track: version}, 'ci': self.ci, 'commit': self.sha,
                        'source_dirty': False, 'target': target, 'component': component,
                        'platform': 'android' if 'android' in target else 'linux',
                        'minimum_android_api': 24 if 'android' in target else None, 'binaries': binaries,
                        'binary_versions': {name: version for name in binaries},
                        'files': {p.relative_to(tree).as_posix(): digest(p) for p in tree.rglob('*') if p.is_file()}}
                (tree / 'BUILD-INFO.json').write_text(json.dumps(info))
                path = self.inputs / name
                self.package['archive'](tree, path, 123456)
                (self.inputs / (name + '.sha256')).write_text(f'{digest(path)}  {name}\n')
        plugin = self.inputs / 'plugin'
        plugin.mkdir()
        numeric = plugin_version(self.release)
        with zipfile.ZipFile(plugin / f'JellyfinKausway_{numeric}.zip', 'w') as output:
            for name in ['Jellyfin.Plugin.Kausway.dll', 'LICENSE-MIT', 'LICENSE-APACHE']:
                output.writestr(f'JellyfinKausway_{numeric}/{name}', b'fixture')
            output.writestr(f'JellyfinKausway_{numeric}/meta.json', json.dumps({'version': numeric}))
        self.generic(plugin, 'jellyfin-plugin')
        sdk = self.inputs / 'sdk'
        sdk.mkdir()
        base = f'kausway-client-android-{self.release["client"]["version"]}'
        with zipfile.ZipFile(sdk / (base + '.aar'), 'w') as output:
            for abi in ['arm64-v8a', 'armeabi-v7a', 'x86_64']:
                output.writestr(f'jni/{abi}/libkausway_client.so', b'fixture')
            output.writestr('AndroidManifest.xml', b'fixture')
            output.writestr('classes.jar', jar([f'io/github/xaque8787/kausway/{name}.class' for name in
                                              ['KauswayClient', 'NativeBridge', 'ConnectionStatus', 'KauswayConfig']]))
            output.writestr('libs/rustls-platform-verifier.jar', jar(['org/rustls/platformverifier/CertificateVerifier.class']))
        (sdk / (base + '.pom')).write_text(f'<project><groupId>io.github.xaque8787</groupId>'
                                         f'<artifactId>kausway-client-android</artifactId><packaging>aar</packaging>'
                                         f'<version>{self.release["client"]["version"]}</version></project>')
        (sdk / (base + '-sources.jar')).write_bytes(jar(['Client.kt']))
        notices = self.root / 'notices'
        notices.mkdir()
        for suffix in ['LICENSE-MIT', 'LICENSE-APACHE', *[f'{target}/THIRD-PARTY.json' for target in
                       ['aarch64-linux-android', 'armv7-linux-androideabi', 'x86_64-linux-android']]]:
            p = notices / suffix
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text('fixture')
        with tarfile.open(sdk / (base + '-notices.tar.gz'), 'w:gz') as output:
            output.add(notices, arcname='notices')
        (sdk / (base + '-README.md')).write_text('fixture SDK guide')
        self.generic(sdk, 'android-sdk')

    def test_expected_assets_are_independent_and_prerelease_plugin_is_excluded(self):
        self.assertEqual(len(assets('suite', self.release)), 11)
        self.assertEqual(len(assets('client', self.release)), 9)
        self.assertTrue(all('test-client' not in name for name in assets('suite', self.release)))
        self.release['suite']['version'] = '0.5.0-rc.1'
        self.assertEqual(len(assets('suite', self.release)), 10)
        self.assertFalse(any('Jellyfin' in name for name in assets('suite', self.release)))

    def test_complete_suite_and_client_candidates(self):
        self.fixtures()
        for track in ['suite', 'client']:
            output = self.root / track
            assemble(self.inputs, output, track, self.release, self.sha, '11', 1)
            info = verify_candidate(output, track, self.release, self.sha)
            self.assertEqual(set(info['assets']), set(assets(track, self.release)))
            self.assertFalse(info['source_dirty'])

    def test_prerelease_does_not_require_or_distribute_plugin(self):
        self.release['suite']['version'] = '0.5.0-rc.1'
        self.fixtures()
        shutil.rmtree(self.inputs / 'plugin')
        output = self.root / 'suite'
        assemble(self.inputs, output, 'suite', self.release, self.sha, '11', 1)
        self.assertFalse(list(output.glob('*.zip')))
        self.assertTrue(verify_candidate(output, 'suite', self.release)['prerelease'])

    def test_missing_architecture_stable_plugin_and_sdk_companions_fail(self):
        self.fixtures()
        victim = self.inputs / archive_name('host', f'0.5.0-dev.g{self.sha[:12]}', TARGETS[1])
        backup = victim.read_bytes()
        victim.unlink()
        with self.assertRaises(ValueError):
            assemble(self.inputs, self.root / 'missing-target', 'suite', self.release, self.sha, '11', 1)
        victim.write_bytes(backup)
        plugin = self.inputs / 'plugin/BUILD-INFO.jellyfin-plugin.json'
        plugin.unlink()
        with self.assertRaises(ValueError):
            assemble(self.inputs, self.root / 'missing-plugin', 'suite', self.release, self.sha, '11', 1)
        next((self.inputs / 'sdk').glob('*-sources.jar')).unlink()
        with self.assertRaises(ValueError):
            assemble(self.inputs, self.root / 'missing-sdk', 'client', self.release, self.sha, '11', 1)

    def test_wrong_commit_run_and_corruption_fail(self):
        self.fixtures()
        for sha, run in [('b' * 40, '11'), (self.sha, '12')]:
            with self.assertRaises(ValueError):
                assemble(self.inputs, self.root / 'bad-source', 'suite', self.release, sha, run, 1)
        victim = self.inputs / archive_name('suite', f'0.5.0-dev.g{self.sha[:12]}', TARGETS[0])
        victim.write_bytes(victim.read_bytes() + b'changed')
        with self.assertRaises(ValueError):
            assemble(self.inputs, self.root / 'bad-hash', 'suite', self.release, self.sha, '11', 1)

    def test_sdk_pom_identity_is_checked_beyond_hashes(self):
        self.fixtures()
        sdk = self.inputs / 'sdk'
        pom = next(sdk.glob('*.pom'))
        pom.write_text(pom.read_text().replace('kausway-client-android', 'different-sdk'))
        (sdk / 'BUILD-INFO.android-sdk.json').unlink()
        self.generic(sdk, 'android-sdk')
        with self.assertRaises(ValueError):
            assemble(self.inputs, self.root / 'wrong-sdk', 'client', self.release, self.sha, '11', 1)

    def test_release_inventory_rejects_extras_missing_files_and_modified_payloads(self):
        self.fixtures()
        output = self.root / 'suite'
        assemble(self.inputs, output, 'suite', self.release, self.sha, '11', 1)
        extra = output / 'unexpected.txt'
        extra.write_text('extra')
        with self.assertRaises(ValueError):
            verify_inventory(output, 'suite', self.release)
        extra.unlink()
        asset = output / assets('suite', self.release)[0]
        asset.write_bytes(asset.read_bytes() + b'changed')
        with self.assertRaises(ValueError):
            verify_inventory(output, 'suite', self.release)

    def test_selected_suite_preserves_binary_hashes_and_separate_source_commit(self):
        self.fixtures()
        output = self.root / 'suite'
        assemble(self.inputs, output, 'suite', self.release, self.sha, '11', 1)
        spec = importlib.util.spec_from_file_location('stage_release', ROOT / 'scripts/packaging/stage-container-release.py')
        helper = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(helper)
        context = self.root / 'image'
        with patch.object(helper, 'load_release', return_value=self.release):
            helper.stage(output, context, TARGETS[0], 'b' * 40)
        metadata = json.loads((context / 'share/kausway/BUILD-INFO.json').read_text())
        self.assertEqual(metadata['suite_source_commit'], self.sha)
        self.assertEqual(metadata['revision'], 'b' * 40)
        self.assertEqual({p.name for p in (context / 'bin').iterdir()}, set(SUITE))
        for name in SUITE:
            self.assertEqual(digest(context / 'bin' / name), metadata['binaries'][name])
        changed = copy.deepcopy(self.release)
        changed['container']['suite_version'] = '0.4.9'
        with patch.object(helper, 'load_release', return_value=changed), self.assertRaises(ValueError):
            helper.stage(output, self.root / 'wrong-image', TARGETS[0], 'b' * 40)

    def test_container_descriptor_tags_and_platforms(self):
        value = 'sha256:' + 'c' * 64
        info = {'format_version': 1, 'image': 'ghcr.io/owner/project', 'suite_manifest_sha256': 'd' * 64,
                'version': '0.2.0', 'suite_version': '0.5.0', 'commit': self.sha, 'suite_commit': 'b' * 40,
                'index_digest': value, 'platforms': {'linux/amd64': value, 'linux/arm64': value},
                'tags': {'0.2.0': value, f'sha-{self.sha}': value, 'latest': value}}
        verify_container_descriptor(info, self.release, self.sha)
        wrong = copy.deepcopy(info)
        wrong['tags']['latest'] = 'sha256:' + 'd' * 64
        with self.assertRaises(ValueError):
            verify_container_descriptor(wrong, self.release, self.sha)
        release = copy.deepcopy(self.release)
        release['container']['version'] = '0.2.0-rc.1'
        info['version'] = '0.2.0-rc.1'
        info['tags'] = {'0.2.0-rc.1': value, f'sha-{self.sha}': value}
        verify_container_descriptor(info, release, self.sha)
        info['tags']['latest'] = value
        with self.assertRaises(ValueError):
            verify_container_descriptor(info, release, self.sha)

    def test_source_jar_normalization_deduplicates_only_identical_content(self):
        spec = importlib.util.spec_from_file_location('sdk_sources', ROOT / 'scripts/packaging/package-sdk-sources.py')
        helper = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(helper)
        source, output = self.root / 'sources.jar', self.root / 'normalized.jar'
        import warnings
        with warnings.catch_warnings():
            warnings.simplefilter('ignore', UserWarning)
            with zipfile.ZipFile(source, 'w') as jar_file:
                jar_file.writestr('Client.kt', b'API')
                jar_file.writestr('Client.kt', b'API')
        helper.normalize(source, output)
        original = output.read_bytes()
        helper.normalize(source, output)
        self.assertEqual(original, output.read_bytes())
        with zipfile.ZipFile(output) as jar_file:
            self.assertEqual(len(jar_file.namelist()), len(set(jar_file.namelist())))
        with warnings.catch_warnings():
            warnings.simplefilter('ignore', UserWarning)
            with zipfile.ZipFile(source, 'a') as jar_file:
                jar_file.writestr('Client.kt', b'Different API')
        with self.assertRaises(ValueError):
            helper.normalize(source, output)

    def test_source_jar_rejects_agp_shared_local_header_records(self):
        spec = importlib.util.spec_from_file_location('sdk_sources_overlap', ROOT / 'scripts/packaging/package-sdk-sources.py')
        helper = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(helper)
        source, output = self.root / 'overlap.jar', self.root / 'must-not-exist.jar'
        source.write_bytes(jar(['Client.kt']))
        # Reproduce AGP's two central-directory records referencing one local file.
        # Private ZIP writer fields are used only to construct this malformed fixture.
        with zipfile.ZipFile(source, 'a') as archive:
            archive.filelist.append(copy.copy(archive.infolist()[0]))
            archive._didModify = True
        with self.assertRaises(zipfile.BadZipFile):
            helper.normalize(source, output)
        self.assertFalse(output.exists())


if __name__ == '__main__':
    unittest.main()
