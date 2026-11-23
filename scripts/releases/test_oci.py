"""Test image transport, immutable tags and interrupted container publication."""
import argparse
import contextlib
import copy
import gzip
import importlib.util
import io
import json
import os
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

from oci import checksum, docker_archive, ensure_tag, index_bytes, push_image, read_image, verify_config
from packages import digest
from remote import APIError
import test_publisher
from versions import ROOT


def image_fixture(path, architecture='amd64', manifest_hash='f' * 64):
    files = {}
    def blob(data, media_type):
        value = checksum(data)
        files['blobs/sha256/' + value.removeprefix('sha256:')] = data
        return {'mediaType': media_type, 'digest': value, 'size': len(data)}
    data = io.BytesIO()
    with tarfile.open(fileobj=data, mode='w') as layer:
        item = tarfile.TarInfo('fixture')
        item.size = 7
        layer.addfile(item, io.BytesIO(b'fixture'))
    raw_layer = data.getvalue()
    labels = {'org.opencontainers.image.revision': test_publisher.SHA, 'org.opencontainers.image.version': '0.2.0',
              'io.kausway.suite.version': '0.5.0', 'io.kausway.suite.commit': test_publisher.SHA,
              'io.kausway.suite.manifest': manifest_hash}
    config = {'os': 'linux', 'architecture': architecture, 'config': {'Labels': labels},
              'rootfs': {'type': 'layers', 'diff_ids': [checksum(raw_layer)]}}
    config_ref = blob(json.dumps(config).encode(), 'application/vnd.oci.image.config.v1+json')
    layer_ref = blob(gzip.compress(raw_layer, mtime=0), 'application/vnd.oci.image.layer.v1.tar+gzip')
    manifest = {'schemaVersion': 2, 'mediaType': 'application/vnd.oci.image.manifest.v1+json',
                'config': config_ref, 'layers': [layer_ref]}
    descriptor = blob(json.dumps(manifest).encode(), manifest['mediaType'])
    files['index.json'] = json.dumps({'schemaVersion': 2, 'manifests': [descriptor]}).encode()
    files['oci-layout'] = b'{"imageLayoutVersion":"1.0.0"}'
    with tarfile.open(path, 'w') as output:
        for name, value in files.items():
            item = tarfile.TarInfo(name)
            item.size = len(value)
            output.addfile(item, io.BytesIO(value))
    return read_image(path)


class FakeRegistry:
    image = 'ghcr.io/owner/project'

    def __init__(self):
        self.manifests, self.blobs, self.writes = {}, {}, []
        self.fail_latest = False

    def manifest(self, reference, missing=False):
        if reference not in self.manifests:
            if missing:
                return None
            raise APIError(404, 'fixture manifest')
        raw = self.manifests[reference]
        return {'digest': checksum(raw), 'raw': raw, 'data': json.loads(raw)}

    def put_manifest(self, reference, raw, media_type):
        if reference == 'latest' and self.fail_latest:
            raise APIError(503, 'fixture latest promotion interruption')
        self.writes.append(reference)
        self.manifests[reference] = raw
        self.manifests[checksum(raw)] = raw

    def blob(self, value):
        return self.blobs[value]

    def put_blob(self, value, data):
        if checksum(data) != value:
            raise ValueError('fixture checksum mismatch')
        self.blobs[value] = data


class OCITests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.image = image_fixture(self.root / 'oci.tar')

    def test_docker_transport_preserves_config_and_checks_uncompressed_layers(self):
        path = self.root / 'docker.tar'
        docker_archive(self.image, path)
        with tarfile.open(path) as archive:
            manifest = json.load(archive.extractfile('manifest.json'))[0]
            self.assertEqual(checksum(archive.extractfile(manifest['Config']).read()), self.image['config_digest'])
            layer = archive.extractfile(manifest['Layers'][0]).read()
            self.assertEqual(checksum(layer), self.image['config']['rootfs']['diff_ids'][0])
        self.image['config']['rootfs']['diff_ids'][0] = 'sha256:' + '0' * 64
        with self.assertRaises(ValueError):
            docker_archive(self.image, path)

    def test_exact_oci_bytes_uploaded_by_digest_then_tags_never_overwrite(self):
        registry, guards = FakeRegistry(), []
        value = push_image(registry, self.image, lambda: guards.append(True))
        self.assertEqual(registry.manifest(value)['raw'], self.image['raw'])
        self.assertEqual(registry.writes, [value])
        self.assertGreaterEqual(len(guards), 2)
        records = {platform: {'media_type': self.image['descriptor']['mediaType'], 'digest': value,
                              'size': len(self.image['raw'])} for platform in ['linux/amd64', 'linux/arm64']}
        raw = index_bytes(records)
        ensure_tag(registry, '0.2.0', raw, lambda: None)
        writes = len(registry.writes)
        ensure_tag(registry, '0.2.0', raw, lambda: None)
        self.assertEqual(len(registry.writes), writes)
        with self.assertRaisesRegex(ValueError, 'Immutable'):
            ensure_tag(registry, '0.2.0', raw + b' ', lambda: None)
        self.assertNotIn('latest', registry.writes)

    def test_wrong_architecture_labels_and_missing_platform_fail(self):
        arguments = ('linux/amd64', test_publisher.SHA, '0.2.0', '0.5.0', test_publisher.SHA, 'f' * 64)
        verify_config(self.image['config'], *arguments)
        wrong = copy.deepcopy(self.image['config'])
        wrong['architecture'] = 'arm64'
        with self.assertRaises(ValueError):
            verify_config(wrong, *arguments)
        wrong = copy.deepcopy(self.image['config'])
        wrong['config']['Labels']['io.kausway.suite.commit'] = 'b' * 40
        with self.assertRaises(ValueError):
            verify_config(wrong, *arguments)
        with self.assertRaises(ValueError):
            index_bytes({'linux/amd64': {}})

    def test_corrupted_oci_blob_is_rejected(self):
        path = self.root / 'oci.tar'
        with tarfile.open(path, 'a') as archive:
            data = b'incorrect'
            item = tarfile.TarInfo('blobs/sha256/' + '0' * 64)
            item.size = len(data)
            archive.addfile(item, io.BytesIO(data))
        with self.assertRaisesRegex(ValueError, 'checksum'):
            read_image(path)


class ContainerPublisherTests(unittest.TestCase):
    def setUp(self):
        helper = test_publisher.PublisherTests()
        helper.setUp()
        self.addCleanup(helper.doCleanups)
        self.root, self.api, self.publisher = helper.root, helper.api, helper.publisher
        self.helper = helper
        self.publisher.publish_directory(helper.plan(), 'suite', helper.directory)
        self.plan = self.publisher.preflight(test_publisher.SHA, ['container'])
        self.registry = FakeRegistry()
        spec = importlib.util.spec_from_file_location('release_cli_tests', ROOT / 'scripts/releases/publish-release.py')
        self.cli = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.cli)
        registry = patch.object(self.cli, 'Registry', return_value=self.registry)
        registry.start()
        self.addCleanup(registry.stop)
        env = patch.dict(os.environ, {'GITHUB_ACTOR': 'fixture', 'GH_TOKEN': 'unused', 'GITHUB_RUN_ATTEMPT': '1'})
        env.start()
        self.addCleanup(env.stop)
        self.records = self.root / 'records'
        self.records.mkdir()
        manifest = digest(helper.directory / 'RELEASE-MANIFEST.json')
        for arch in ['amd64', 'arm64']:
            image = image_fixture(self.root / f'{arch}.tar', arch, manifest)
            value = push_image(self.registry, image, lambda: None)
            record = {'platform': 'linux/' + arch, 'commit': test_publisher.SHA, 'suite_commit': test_publisher.SHA,
                      'suite_manifest_sha256': manifest, 'tested': True, 'run_attempt': 1, 'publisher_run_id': '90',
                      'digest': value, 'size': len(image['raw']), 'media_type': image['descriptor']['mediaType']}
            (self.records / f'platform-{arch}.json').write_text(json.dumps(record))

    def run_container(self):
        work = Path(tempfile.mkdtemp(dir=self.root))
        with contextlib.redirect_stdout(io.StringIO()):
            self.cli.finalize_container(argparse.Namespace(work=work, records=self.records), self.publisher, self.plan)

    def test_latest_failure_after_release_completion_can_be_repaired_without_rebuild(self):
        self.registry.fail_latest = True
        with self.assertRaises(APIError):
            self.run_container()
        self.assertFalse(self.api.releases['container-v0.2.0']['draft'])
        value = self.registry.manifest('0.2.0')['digest']
        self.assertNotIn('latest', self.registry.manifests)
        uploads = len(self.api.uploads)
        self.registry.fail_latest = False
        self.run_container()
        self.assertEqual(self.registry.manifest('latest')['digest'], value)
        self.assertEqual(self.registry.manifest('sha-' + test_publisher.SHA)['digest'], value)
        self.assertEqual(len(self.api.uploads), uploads)
        writes = len(self.registry.writes)
        self.run_container()
        self.assertEqual(len(self.registry.writes), writes)

    def test_interrupted_assets_retry_retains_index_and_identical_draft_assets(self):
        self.api.fail_upload = len(self.api.uploads) + 1
        with self.assertRaises(APIError):
            self.run_container()
        value = self.registry.manifest('0.2.0')['digest']
        self.assertNotIn('latest', self.registry.manifests)
        self.api.fail_upload = None
        self.run_container()
        self.assertEqual(self.registry.manifest('latest')['digest'], value)
        self.assertFalse(self.api.releases['container-v0.2.0']['draft'])

    def test_container_publication_uses_canonical_draft_id_when_tag_lookup_lags(self):
        self.api.hide_drafts_by_tag = True
        original = self.api.pages
        def pages(path, key=None):
            result = original(path, key)
            return [item for item in result if not item['draft']] if path == self.api.prefix + '/releases' else result
        with patch.object(self.api, 'pages', side_effect=pages):
            self.run_container()
        self.assertFalse(self.api.releases['container-v0.2.0']['draft'])
        self.assertEqual(self.registry.manifest('latest')['digest'], self.registry.manifest('0.2.0')['digest'])
        self.assertEqual(self.api.latest['tag_name'], 'suite-v0.5.0')

    def test_registry_conflict_and_untrusted_receipts_prevent_promotion(self):
        record = self.records / 'platform-amd64.json'
        data = json.loads(record.read_text())
        data['publisher_run_id'] = '91'
        record.write_text(json.dumps(data))
        with self.assertRaisesRegex(ValueError, 'another source/run/suite'):
            self.run_container()
        self.assertNotIn('0.2.0', self.registry.manifests)
        self.assertNotIn('container-v0.2.0', self.api.releases)

    def test_rerunning_older_complete_release_does_not_downgrade_latest(self):
        self.run_container()
        old = self.registry.manifest('latest')['digest']
        newer_raw = b'{"schemaVersion":2,"manifests":[]}'
        self.registry.put_manifest('latest', newer_raw, 'application/vnd.oci.image.index.v1+json')
        self.api.history.append({'tag_name': 'container-v0.3.0', 'draft': False, 'prerelease': False})
        self.run_container()
        self.assertEqual(self.registry.manifest('latest')['digest'], checksum(newer_raw))
        self.assertEqual(self.registry.manifest('0.2.0')['digest'], old)

    def test_prerelease_never_moves_latest(self):
        # Rebuild fixture configs with the prerelease container label; suite stays stable.
        self.helper.versions['container']['version'] = '0.2.0-rc.1'
        self.plan = self.publisher.preflight(test_publisher.SHA, ['container'])
        for path in self.records.glob('*.json'):
            record = json.loads(path.read_text())
            image = self.registry.manifest(record['digest'])
            config_ref = image['data']['config']
            config = json.loads(self.registry.blob(config_ref['digest']))
            config['config']['Labels']['org.opencontainers.image.version'] = '0.2.0-rc.1'
            config_raw = json.dumps(config).encode()
            self.registry.put_blob(checksum(config_raw), config_raw)
            config_ref.update(digest=checksum(config_raw), size=len(config_raw))
            raw = json.dumps(image['data']).encode()
            self.registry.put_manifest(checksum(raw), raw, record['media_type'])
            record.update(digest=checksum(raw), size=len(raw))
            path.write_text(json.dumps(record))
        self.run_container()
        self.assertNotIn('latest', self.registry.manifests)
        self.assertTrue(self.api.releases['container-v0.2.0-rc.1']['prerelease'])


if __name__ == '__main__':
    unittest.main()
