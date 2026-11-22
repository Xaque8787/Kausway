"""Verified OCI transport: test exported bytes once, upload those exact bytes by digest."""
import hashlib
import gzip
import io
import json
from pathlib import PurePosixPath
import re
import tarfile


def checksum(data):
    return 'sha256:' + hashlib.sha256(data).hexdigest()


def read_image(path):
    files = {}
    with tarfile.open(path) as archive:
        for entry in archive:
            name = PurePosixPath(entry.name)
            if name.is_absolute() or '..' in name.parts or '\\' in entry.name or entry.name in files:
                raise ValueError('Unsafe/duplicate OCI path')
            if entry.isdir():
                continue
            if not entry.isfile() or entry.size > 512 * 1024 * 1024:
                raise ValueError('Unsupported OCI member')
            if entry.name not in ('index.json', 'oci-layout') and not re.fullmatch('blobs/sha256/[0-9a-f]{64}', entry.name):
                raise ValueError('Unexpected OCI layout member')
            files[entry.name] = archive.extractfile(entry).read()
    if json.loads(files['oci-layout'])['imageLayoutVersion'] != '1.0.0':
        raise ValueError('Unsupported OCI layout')
    for name, data in files.items():
        if name.startswith('blobs/') and name.split('/')[-1] != hashlib.sha256(data).hexdigest():
            raise ValueError('OCI blob checksum mismatch')
    def blob(descriptor):
        value = descriptor['digest']
        if not re.fullmatch('sha256:[0-9a-f]{64}', value):
            raise ValueError('Invalid OCI digest')
        raw = files['blobs/sha256/' + value.removeprefix('sha256:')]
        if len(raw) != descriptor['size']:
            raise ValueError('OCI blob size mismatch')
        return raw
    index = json.loads(files['index.json'])
    if len(index['manifests']) != 1:
        raise ValueError('Expected one platform, with provenance/attestations disabled')
    descriptor = index['manifests'][0]
    manifest_raw = blob(descriptor)
    manifest = json.loads(manifest_raw)
    if 'config' not in manifest:
        raise ValueError('Expected an OCI image manifest, not an index')
    config = json.loads(blob(manifest['config']))
    for layer in manifest['layers']:
        blob(layer)
    return {'descriptor': descriptor, 'manifest': manifest, 'raw': manifest_raw, 'config': config,
            'config_digest': manifest['config']['digest'], 'files': files}


def verify_config(config, platform, commit, version, suite_version, suite_commit, suite_manifest):
    if config['os'] + '/' + config['architecture'] != platform:
        raise ValueError('Image architecture mismatch')
    labels = config['config']['Labels']
    wanted = {'org.opencontainers.image.revision': commit, 'org.opencontainers.image.version': version,
              'io.kausway.suite.version': suite_version, 'io.kausway.suite.commit': suite_commit,
              'io.kausway.suite.manifest': suite_manifest}
    if any(labels.get(key) != value for key, value in wanted.items()):
        raise ValueError('Image labels do not identify the pinned release/suite')


def docker_archive(image, destination):
    """Docker's classic store needs manifest.json and uncompressed, verified layers.

    Preserve the original config/image ID and check every layer against its diff ID.
    The OCI archive remains the publication source; this is only a load transport.
    """
    layers = image['manifest']['layers']
    diff_ids = image['config']['rootfs']['diff_ids']
    if len(layers) != len(diff_ids):
        raise ValueError('Image config/layer count differs')
    with tarfile.open(destination, 'w') as output:
        def write(name, data):
            member = tarfile.TarInfo(name)
            member.size = len(data)
            member.mode = 0o644
            output.addfile(member, io.BytesIO(data))
        config = image['config_digest'].removeprefix('sha256:') + '.json'
        write(config, image['files']['blobs/sha256/' + image['config_digest'].removeprefix('sha256:')])
        names = []
        for number, (layer, diff_id) in enumerate(zip(layers, diff_ids)):
            data = image['files']['blobs/sha256/' + layer['digest'].removeprefix('sha256:')]
            if layer['mediaType'] in ('application/vnd.oci.image.layer.v1.tar+gzip',
                                      'application/vnd.docker.image.rootfs.diff.tar.gzip'):
                with gzip.GzipFile(fileobj=io.BytesIO(data)) as stream:
                    data = stream.read(512 * 1024 * 1024 + 1)
            elif layer['mediaType'] != 'application/vnd.oci.image.layer.v1.tar':
                raise ValueError('Unsupported image layer compression')
            if len(data) > 512 * 1024 * 1024 or checksum(data) != diff_id:
                raise ValueError('Uncompressed layer does not match the tested config')
            name = f'{number}/layer.tar'
            names.append(name)
            write(name, data)
        write('manifest.json', json.dumps([{'Config': config, 'RepoTags': ['kausway-suite:release'],
                                          'Layers': names}]).encode())


def push_image(registry, image, guard):
    for descriptor in [image['manifest']['config'], *image['manifest']['layers']]:
        guard()
        data = image['files']['blobs/sha256/' + descriptor['digest'].removeprefix('sha256:')]
        registry.put_blob(descriptor['digest'], data)
    value = checksum(image['raw'])
    existing = registry.manifest(value, missing=True)
    if existing is not None and existing['raw'] != image['raw']:
        raise ValueError('Registry digest conflict')
    if existing is None:
        guard()
        registry.put_manifest(value, image['raw'], image['descriptor']['mediaType'])
    if registry.manifest(value)['digest'] != value:
        raise ValueError('Registry did not retain the tested image bytes')
    return value


def index_bytes(records):
    if set(records) != {'linux/amd64', 'linux/arm64'}:
        raise ValueError('Both native platform records are required')
    manifests = []
    for platform, record in sorted(records.items()):
        os_name, architecture = platform.split('/')
        manifests.append({'mediaType': record['media_type'], 'digest': record['digest'], 'size': record['size'],
                          'platform': {'os': os_name, 'architecture': architecture}})
    return json.dumps({'schemaVersion': 2, 'mediaType': 'application/vnd.oci.image.index.v1+json',
                       'manifests': manifests}, sort_keys=True, separators=(',', ':')).encode()


def ensure_tag(registry, tag, raw, guard):
    value = checksum(raw)
    current = registry.manifest(tag, missing=True)
    if current is not None and current['digest'] != value:
        raise ValueError(f'Immutable image tag {tag} already identifies other bytes')
    if current is None:
        guard()
        registry.put_manifest(tag, raw, 'application/vnd.oci.image.index.v1+json')
    if registry.manifest(tag)['digest'] != value:
        raise ValueError('Image tag verification failed')
