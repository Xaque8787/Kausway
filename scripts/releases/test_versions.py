"""Acceptance tests use isolated fixtures and never bump this checkout's versions."""
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import tomllib
import unittest

from versions import (ROOT, binary_versions, check_metadata, load_release, parse_version, planned_updates,
                      plugin_distributable, plugin_version, registration)


class VersionTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='kausway-versions-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        paths = ['releases.toml', 'scripts/releases/components.toml', 'rust/Cargo.toml', 'rust/Cargo.lock',
                 'sdks/android/release-version.properties', 'integrations/jellyfin/meta.json',
                 'integrations/jellyfin/plugin/Jellyfin.Plugin.Kausway.csproj']
        paths += [str(path.relative_to(ROOT)) for path in (ROOT / 'rust/crates').glob('*/Cargo.toml')]
        for relative in paths:
            target = self.root / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / relative, target)
        # Fixed, synchronized fixture baseline: bump tests must still perform a
        # real change when the repository itself reaches one of their versions.
        (self.root / 'releases.toml').write_text(
            'schema_version = 1\n[suite]\nversion = "0.4.0"\n'
            '[client]\nversion = "0.4.0"\n[container]\n'
            'version = "0.1.0"\nsuite_version = "0.4.0"\n')
        for path, text in planned_updates(self.root).items():
            path.write_text(text)

    def snapshot(self):
        return {str(p.relative_to(self.root)): hashlib.sha256(p.read_bytes()).hexdigest()
                for p in self.root.rglob('*') if p.is_file()}

    def bump(self, track, value, field='version'):
        path = self.root / 'releases.toml'
        text = path.read_text()
        before = load_release(self.root)[track][field]
        start = text.index(f'[{track}]')
        text = text[:start] + text[start:].replace(f'{field} = "{before}"', f'{field} = "{value}"', 1)
        path.write_text(text)

    def sync(self):
        updates = planned_updates(self.root)
        for path, text in updates.items():
            path.write_text(text)
        return updates

    def cli(self, *args):
        return subprocess.run([sys.executable, str(ROOT / 'scripts/releases/sync-versions.py'),
                               '--root', str(self.root), *args], capture_output=True, text=True)

    def package_versions(self):
        return {p['name']: p['version'] for p in tomllib.loads((self.root / 'rust/Cargo.lock').read_text())['package']
                if 'source' not in p}

    def test_current_checkout_is_synchronized(self):
        self.assertFalse(planned_updates(ROOT))

    def test_repeatable_sync_and_readonly_check(self):
        self.bump('suite', '0.5.0')
        before = self.snapshot()
        result = self.cli('--check')
        self.assertEqual(result.returncode, 1)
        self.assertIn('Version drift:', result.stderr)
        self.assertEqual(before, self.snapshot())
        self.assertEqual(self.cli().returncode, 0)
        after = self.snapshot()
        self.assertEqual(self.cli().returncode, 0)
        self.assertEqual(self.cli('--check').returncode, 0)
        self.assertEqual(after, self.snapshot())

    def test_suite_bump_does_not_bump_client_or_container(self):
        release = load_release(self.root)
        self.bump('suite', '0.5.0')
        self.sync()
        versions = self.package_versions()
        for name, track in registration(self.root)['rust_packages'].items():
            self.assertEqual(versions[name], '0.5.0' if track == 'suite' else release['client']['version'])
        self.assertEqual(load_release(self.root)['container'], release['container'])
        self.assertEqual(json.loads((self.root / 'integrations/jellyfin/meta.json').read_text())['version'], '0.5.0.0')
        self.assertFalse(planned_updates(self.root))

    def test_client_bump_does_not_bump_suite_or_plugin(self):
        previous = self.package_versions()
        plugin = (self.root / 'integrations/jellyfin/meta.json').read_bytes()
        self.bump('client', '0.6.0')
        self.sync()
        versions = self.package_versions()
        for name, track in registration(self.root)['rust_packages'].items():
            self.assertEqual(versions[name], '0.6.0' if track == 'client' else previous[name])
        self.assertEqual((self.root / 'integrations/jellyfin/meta.json').read_bytes(), plugin)
        self.assertIn('version=0.6.0', (self.root / 'sdks/android/release-version.properties').read_text())
        self.assertEqual(binary_versions(self.root)['kausway-test-client'], '0.6.0')

    def test_container_bump_and_suite_selection_do_not_rewrite_binaries(self):
        self.bump('container', '0.2.0')
        self.bump('container', '0.3.0', 'suite_version')
        self.assertFalse(planned_updates(self.root))

    def test_third_party_lock_entries_unchanged(self):
        def external():
            text = (self.root / 'rust/Cargo.lock').read_text()
            return [block for block in text.split('[[package]]') if '\nsource = ' in block]
        before = external()
        self.bump('suite', '0.7.0')
        self.bump('client', '0.8.0')
        self.sync()
        self.assertEqual(before, external())

    def test_semver_order_and_rejections(self):
        self.assertLess(parse_version('0.5.0-rc.2'), parse_version('0.5.0-rc.10'))
        self.assertLess(parse_version('0.5.0-rc.10'), parse_version('0.5.0'))
        self.assertGreater(parse_version('0.10.0'), parse_version('0.9.0'))
        for invalid in ['0.5', '01.5.0', '0.5.0-01', 'v0.5.0', '0.5.0+build.1', '0.5.0.post1']:
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                parse_version(invalid)

    def test_jellyfin_prerelease_mapping(self):
        self.bump('suite', '0.5.0-rc.1')
        release = load_release(self.root)
        self.assertEqual(plugin_version(release), '0.5.0.0')
        self.assertFalse(plugin_distributable(release))
        self.sync()
        self.assertFalse(planned_updates(self.root))
        self.bump('suite', '0.5.0')
        self.assertTrue(plugin_distributable(load_release(self.root)))

    def test_missing_or_unknown_registration_fails(self):
        path = self.root / 'scripts/releases/components.toml'
        original = path.read_text()
        for revised in [original.replace('kausway-client = "client"\n', ''),
                        original.replace('[rust_packages]', '[rust_packages]\nunknown = "suite"'),
                        original.replace('kausway-test-client = "client"', 'kausway-test-client = "suite"')]:
            path.write_text(revised)
            with self.assertRaises(ValueError):
                planned_updates(self.root)

    def test_jellyfin_numeric_mapping_rejects_out_of_range_components(self):
        self.bump('suite', '0.5.65535')
        before = self.snapshot()
        self.assertNotEqual(self.cli().returncode, 0)
        self.assertEqual(before, self.snapshot())

    def test_new_sdk_registration_shares_client_track(self):
        registry = self.root / 'scripts/releases/components.toml'
        with registry.open('a') as output:
            output.write('\n[[sdk_versions]]\nname="future-sdk"\ntrack="client"\nformat="properties"\n'
                         'path="sdks/future/version.properties"\nkey="version"\n')
        sdk = self.root / 'sdks/future/version.properties'
        sdk.parent.mkdir(parents=True)
        sdk.write_text('version=0.0.0\n')
        self.bump('client', '0.9.0')
        self.sync()
        self.assertEqual(sdk.read_text(), 'version=0.9.0\n')
        self.assertIn('version=0.9.0', (self.root / 'sdks/android/release-version.properties').read_text())

    def test_invalid_manifest_leaves_everything_untouched(self):
        self.bump('suite', '0.5')
        before = self.snapshot()
        self.assertNotEqual(self.cli().returncode, 0)
        self.assertEqual(before, self.snapshot())

    def test_lockfile_missing_local_package_fails_before_writes(self):
        path = self.root / 'rust/Cargo.lock'
        path.write_text(path.read_text().replace('name = "kausway-admin"', 'name = "missing-admin"'))
        before = self.snapshot()
        self.assertNotEqual(self.cli().returncode, 0)
        self.assertEqual(before, self.snapshot())

    def test_scope_supports_partial_suite_docker_stage(self):
        (self.root / 'sdks/android/release-version.properties').unlink()
        self.assertFalse(planned_updates(self.root, scope='suite'))
        with self.assertRaises(FileNotFoundError):
            planned_updates(self.root)

    def test_packaging_rejects_stale_or_incomplete_workspace_metadata(self):
        registry, release = registration(self.root), load_release(self.root)
        metadata = {'workspace_root': str(self.root / 'rust'), 'packages': [
            {'name': name, 'source': None, 'version': release[track]['version']}
            for name, track in registry['rust_packages'].items()]}
        check_metadata(metadata, self.root)
        metadata['packages'][0]['version'] = '0.0.0'
        with self.assertRaises(ValueError):
            check_metadata(metadata, self.root)
        metadata['packages'].pop(0)
        with self.assertRaises(ValueError):
            check_metadata(metadata, self.root)


if __name__ == '__main__':
    unittest.main()
