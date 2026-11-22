#!/usr/bin/env python3
"""Entry points used only by manual Publish release. No command auto-bumps versions."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

from layout import SUITE, assets
from oci import checksum, docker_archive, ensure_tag, index_bytes, push_image, read_image, verify_config
from packages import digest, write_inventory
from publisher import Publisher
from release_policy import history_version, release_state
from remote import GitHub, Registry
from versions import ROOT, parse_version


def output(name, value):
    if os.environ.get('GITHUB_OUTPUT'):
        with open(os.environ['GITHUB_OUTPUT'], 'a') as stream:
            stream.write(f'{name}={value}\n')


def summary(message):
    print(message)
    if os.environ.get('GITHUB_STEP_SUMMARY'):
        with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as stream:
            stream.write(message + '\n\n')


def suite_for_plan(publisher, plan):
    path, info = publisher.selected_suite(plan['versions'])
    return path, info, digest(path / 'RELEASE-MANIFEST.json')


def expected_image(image, plan, info, manifest, platform):
    verify_config(image['config'], platform, plan['commit'], plan['versions']['container']['version'],
                  plan['versions']['container']['suite_version'], info['commit'], manifest)


def test_image(args, publisher, plan):
    publisher.validate_plan(plan)
    _, info, manifest = suite_for_plan(publisher, plan)
    image = read_image(args.oci)
    expected_image(image, plan, info, manifest, args.platform)
    transport = args.work / 'docker-load.tar'
    docker_archive(image, transport)
    subprocess.run(['docker', 'load', '-i', str(transport)], check=True)
    transport.unlink()
    subprocess.run(['docker', 'tag', image['config_digest'], 'kausway-suite:release'], check=True)
    metadata = json.loads(subprocess.check_output(['docker', 'run', '--rm', 'kausway-suite:release',
                                                  'cat', '/usr/share/kausway/BUILD-INFO.json'], text=True))
    wanted = json.loads((args.context / 'share/kausway/BUILD-INFO.json').read_text())
    if metadata != wanted:
        raise ValueError('Loaded image does not contain the staged suite metadata')
    for name in SUITE:
        actual = subprocess.check_output(['docker', 'run', '--rm', 'kausway-suite:release',
                                          'sha256sum', '/usr/local/bin/' + name], text=True).split()[0]
        if actual != digest(args.context / 'bin' / name):
            raise ValueError('Loaded image binary differs from selected release')
    subprocess.run(['docker', 'run', '--rm', 'kausway-suite:release', 'sh', '-c',
                    'test ! -e /usr/local/bin/kausway-test-client'], check=True)
    subprocess.run([sys.executable, str(ROOT / 'scripts/ci/container-suite-smoke.py'), 'kausway-suite:release'], check=True)
    args.receipt.write_text(json.dumps({'commit': plan['commit'], 'platform': args.platform,
                                      'archive_sha256': digest(args.oci), 'suite_commit': info['commit'],
                                      'suite_manifest_sha256': manifest, 'tested': True}, indent=2) + '\n')


def remote_config(registry, descriptor, platform, plan, suite, manifest):
    image = registry.manifest(descriptor['digest'])
    if image['digest'] != descriptor['digest'] or len(image['raw']) != descriptor['size']:
        raise ValueError('Remote platform manifest digest/size differs')
    config_ref = image['data']['config']
    config = registry.blob(config_ref['digest'])
    if checksum(config) != config_ref['digest'] or len(config) != config_ref['size']:
        raise ValueError('Remote config digest differs')
    verify_config(json.loads(config), platform, plan['commit'], plan['versions']['container']['version'],
                  plan['versions']['container']['suite_version'], suite['commit'], manifest)


def finalize_container(args, publisher, plan):
    publisher.validate_plan(plan)
    _, suite, manifest = suite_for_plan(publisher, plan)
    registry = Registry('ghcr.io/' + publisher.api.repository.lower(), os.environ['GITHUB_ACTOR'], os.environ['GH_TOKEN'])
    version = plan['versions']['container']['version']
    tag = 'container-v' + version
    existing_release = publisher.release(tag)
    state = release_state(existing_release, publisher.tag_commit(tag), plan['commit'], version)
    if state == 'complete':
        directory, _ = publisher.download_release(existing_release, 'container', plan['versions'], plan['commit'])
        descriptor = json.loads((directory / assets('container', plan['versions'])[0]).read_text())
        if descriptor['suite_commit'] != suite['commit'] or descriptor['suite_manifest_sha256'] != manifest:
            raise ValueError('Completed image references another selected suite')
        index = registry.manifest(descriptor['index_digest'])
        for name in [version, 'sha-' + plan['commit']]:
            if registry.manifest(name)['digest'] != descriptor['index_digest']:
                raise ValueError('Completed immutable image tags changed; inspect registry, do not overwrite')
    else:
        records = {}
        for path in args.records.rglob('platform-*.json'):
            record = json.loads(path.read_text())
            if (record['commit'] != plan['commit'] or record['publisher_run_id'] != plan['publisher_run_id']
                    or record['suite_commit'] != suite['commit'] or record['suite_manifest_sha256'] != manifest
                    or record['tested'] is not True or record['run_attempt'] > int(os.environ['GITHUB_RUN_ATTEMPT'])):
                raise ValueError('Platform receipt belongs to another source/run/suite')
            records.setdefault(record['platform'], []).append(record)
        old = registry.manifest(version, missing=True)
        chosen = {}
        for platform, options in records.items():
            if old is not None:
                wanted = [item['digest'] for item in old['data']['manifests'] if
                          item['platform']['os'] + '/' + item['platform']['architecture'] == platform]
                options = [option for option in options if option['digest'] in wanted]
            if not options:
                raise ValueError('Existing version index has no matching tested receipt; resume the original run')
            attempt = max(item['run_attempt'] for item in options)
            latest = [item for item in options if item['run_attempt'] == attempt]
            if len(latest) != 1:
                raise ValueError('Ambiguous tested platform receipts for one attempt')
            chosen[platform] = latest[0]
        raw = index_bytes(chosen)
        value = checksum(raw)
        if old is not None and old['digest'] != value:
            raise ValueError('Existing immutable image version conflicts; never overwrite it')
        for platform, record in chosen.items():
            remote_config(registry, {'digest': record['digest'], 'size': record['size']}, platform, plan, suite, manifest)
        publisher.guard(plan['commit'])
        if registry.manifest(value, missing=True) is None:
            registry.put_manifest(value, raw, 'application/vnd.oci.image.index.v1+json')
        if registry.manifest(value)['digest'] != value:
            raise ValueError('Image index verification failed')
        publisher.validate_plan(plan)
        for name in [version, 'sha-' + plan['commit']]:
            ensure_tag(registry, name, raw, lambda: publisher.guard(plan['commit']))
        tags = {version: value, 'sha-' + plan['commit']: value}
        if parse_version(version).prerelease is None:
            tags['latest'] = value
        descriptor = {'format_version': 1, 'image': registry.image, 'version': version, 'commit': plan['commit'],
                      'suite_version': plan['versions']['container']['suite_version'], 'suite_commit': suite['commit'],
                      'suite_manifest_sha256': manifest, 'index_digest': value,
                      'platforms': {key: item['digest'] for key, item in chosen.items()}, 'tags': tags}
        directory = args.work / 'container-release'
        directory.mkdir()
        (directory / assets('container', plan['versions'])[0]).write_text(json.dumps(descriptor, indent=2) + '\n')
        write_inventory(directory, 'container', plan['versions'], plan['commit'],
                        [{'source_dirty': False, 'suite_commit': suite['commit'], 'validation': plan['validation']}])
        publisher.publish_directory(plan, 'container', directory)
        index = registry.manifest(value)
    if index['digest'] != descriptor['index_digest']:
        raise ValueError('Descriptor/index mismatch')
    platforms = {item['platform']['os'] + '/' + item['platform']['architecture']: item for item in index['data']['manifests']}
    if set(platforms) != {'linux/amd64', 'linux/arm64'} or len(index['data']['manifests']) != 2:
        raise ValueError('Published index architecture set differs')
    for platform, image in platforms.items():
        if image['digest'] != descriptor['platforms'][platform]:
            raise ValueError('Published descriptor platform digest differs')
        remote_config(registry, image, platform, plan, suite, manifest)
    if parse_version(version).prerelease is None:
        history = publisher.api.pages(publisher.prefix + '/releases')
        stable = [history_version(item['tag_name'], 'container') for item in history if not item['draft'] and not item['prerelease']]
        stable = [value for value in stable if value is not None]
        if stable and max(stable) > parse_version(version):
            summary('Container already complete; latest not changed because a newer stable release exists.')
        else:
            publisher.validate_plan(plan)
            publisher.guard(plan['commit'])
            current = registry.manifest('latest', missing=True)
            if current is None or current['digest'] != descriptor['index_digest']:
                registry.put_manifest('latest', index['raw'], 'application/vnd.oci.image.index.v1+json')
            if registry.manifest('latest')['digest'] != descriptor['index_digest']:
                raise ValueError('Latest promotion failed; rerun original failed publication job')
    summary(f'Container {version}: verified {descriptor["index_digest"]} with AMD64/ARM64; no rebuild per tag.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=['preflight', 'binary', 'prepare-container', 'test-image', 'push-image', 'container'])
    parser.add_argument('--plan', type=Path)
    parser.add_argument('--work', type=Path, required=True)
    parser.add_argument('--track', choices=['suite', 'client'])
    parser.add_argument('--suite', action='store_true')
    parser.add_argument('--client', action='store_true')
    parser.add_argument('--container', action='store_true')
    parser.add_argument('--output', type=Path)
    parser.add_argument('--oci', type=Path)
    parser.add_argument('--context', type=Path)
    parser.add_argument('--receipt', type=Path)
    parser.add_argument('--platform', choices=['linux/amd64', 'linux/arm64'])
    parser.add_argument('--records', type=Path)
    args = parser.parse_args()
    publisher = Publisher(GitHub(os.environ['GITHUB_REPOSITORY'], os.environ['GH_TOKEN']), args.work)
    if args.operation == 'preflight':
        selected = [track for track in ['suite', 'client', 'container'] if getattr(args, track)]
        plan = publisher.preflight(os.environ['GITHUB_SHA'], selected)
        args.output.write_text(json.dumps(plan, indent=2) + '\n')
        for track in ['suite', 'client', 'container']:
            output(track, str(track in selected).lower())
            output(track + '_version', plan['versions'][track]['version'])
        output('container_state', plan['tracks'].get('container', {}).get('state', 'unselected'))
        output('container_needs_suite', str('suite' in selected and
               plan['versions']['suite']['version'] == plan['versions']['container']['suite_version']).lower())
        summary(f'Pinned main commit `{plan["commit"]}`; validated run {plan["validation"]["id"]}, attempt {plan["validation"]["attempt"]}.')
        for track, item in plan['tracks'].items():
            summary(f'{track} {item["version"]}: {item["state"]}')
        return
    plan = json.loads(args.plan.read_text())
    run = publisher.validate_plan(plan)
    if args.operation == 'binary':
        if args.track not in plan['tracks']:
            raise ValueError('Track was not selected at dispatch')
        release = publisher.release(plan['tracks'][args.track]['tag'])
        if release is not None and not release['draft']:
            directory, _ = publisher.download_release(release, args.track, plan['versions'], plan['commit'])
        else:
            frozen = plan['tracks'][args.track]['manifest_sha256']
            if frozen is None:
                raise ValueError('A completed release disappeared; do not recreate automatically')
            directory, _ = publisher.candidate(run, args.track, plan['versions'], plan['commit'], frozen)
        summary(f'{args.track}: {publisher.publish_directory(plan, args.track, directory)}')
    else:
        if 'container' not in plan['tracks']:
            raise ValueError('Container was not selected at dispatch')
        if args.operation == 'prepare-container':
            directory, info, checksum_value = suite_for_plan(publisher, plan)
            shutil.copytree(directory, args.output)
            output('suite_commit', info['commit'])
            output('suite_version', plan['versions']['container']['suite_version'])
            output('suite_manifest_sha256', checksum_value)
        elif args.operation == 'test-image':
            test_image(args, publisher, plan)
        elif args.operation == 'push-image':
            _, info, manifest = suite_for_plan(publisher, plan)
            image = read_image(args.oci)
            expected_image(image, plan, info, manifest, args.platform)
            receipt = json.loads(args.receipt.read_text())
            expected = {'commit': plan['commit'], 'platform': args.platform, 'archive_sha256': digest(args.oci),
                        'suite_commit': info['commit'], 'suite_manifest_sha256': manifest, 'tested': True}
            if receipt != expected:
                raise ValueError('OCI bytes differ from the tested image receipt')
            registry = Registry('ghcr.io/' + publisher.api.repository.lower(), os.environ['GITHUB_ACTOR'], os.environ['GH_TOKEN'])
            value = push_image(registry, image, lambda: publisher.guard(plan['commit']))
            record = {**receipt, 'digest': value, 'size': len(image['raw']), 'media_type': image['descriptor']['mediaType'],
                      'publisher_run_id': plan['publisher_run_id'], 'run_attempt': int(os.environ['GITHUB_RUN_ATTEMPT'])}
            args.output.write_text(json.dumps(record, indent=2) + '\n')
        elif args.operation == 'container':
            finalize_container(args, publisher, plan)


if __name__ == '__main__':
    try:
        main()
    except (ValueError, KeyError, OSError, RuntimeError) as error:
        print(f'Publication stopped: {error}', file=sys.stderr)
        sys.exit(1)
