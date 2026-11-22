"""Publication tests use in-memory GitHub/registry boundaries, never real tokens."""
import copy
import io
import itertools
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

from packages import assemble, digest
from publisher import Publisher
from release_policy import (WORKFLOW, check_advancement, latest_validation, publication_context,
                            release_state, verify_summary)
from remote import APIError, SafeRedirect, unzip
import test_packages
from versions import ROOT

SHA = 'a' * 40
REPO = 'owner/project'


def trusted_run():
    return {'id': 11, 'run_number': 7, 'run_attempt': 1, 'head_sha': SHA, 'head_branch': 'main',
            'event': 'push', 'path': WORKFLOW, 'repository': {'full_name': REPO},
            'head_repository': {'full_name': REPO}, 'status': 'completed', 'conclusion': 'success'}


def summary(run, policy):
    return {'schema_version': 1, 'workflow': WORKFLOW, 'success': True, 'errors': [],
            'ci': {'commit': SHA, 'repository': REPO, 'ref': 'refs/heads/main', 'event': 'push',
                   'run_id': str(run['id']), 'run_attempt': str(run['run_attempt'])},
            'jobs': {key: {'description': desc, 'result': 'success', 'required': True}
                     for key, desc in policy['required_jobs'].items()}}


class FakeGitHub:
    repository, prefix = REPO, '/repos/' + REPO

    def __init__(self):
        self.tags, self.releases, self.payloads, self.artifacts = {}, {}, {}, []
        self.writes, self.uploads = [], []
        self.runs = [trusted_run()]
        self.ancestor, self.main = True, SHA
        self.history = [{'tag_name': 'v0.4.0', 'draft': False, 'prerelease': False}]
        self.fail_upload = None
        self.hide_drafts_by_tag = False
        self.job = {'name': 'Validation summary', 'head_sha': SHA, 'run_id': 11, 'status': 'completed',
                    'conclusion': 'success', 'check_run_url': 'https://api.github.com' + self.prefix + '/check-runs/9'}

    def call(self, path, method='GET', payload=None, missing=False):
        tail = path.removeprefix(self.prefix)
        if method != 'GET':
            self.writes.append((method, tail, copy.deepcopy(payload)))
        if tail == '/git/ref/heads/main':
            return {'object': {'sha': self.main}}
        if tail.startswith('/compare/'):
            return {'merge_base_commit': {'sha': SHA if self.ancestor else 'b' * 40}, 'status': 'identical'}
        if tail.startswith('/git/ref/tags/'):
            value = self.tags.get(tail.removeprefix('/git/ref/tags/'))
            return {'object': {'sha': value, 'type': 'commit'}} if value else None
        if tail == '/git/refs' and method == 'POST':
            self.tags[payload['ref'].removeprefix('refs/tags/')] = payload['sha']
            return {}
        if tail.startswith('/releases/tags/'):
            release = self.releases.get(tail.removeprefix('/releases/tags/'))
            if release and release['draft'] and self.hide_drafts_by_tag:
                return None
            return copy.deepcopy(release)
        if tail == '/releases' and method == 'POST':
            release = {**payload, 'id': 40 + len(self.releases), 'assets': []}
            self.releases[payload['tag_name']] = release
            return copy.deepcopy(release)
        if tail.startswith('/releases/') and method == 'PATCH':
            release = next(item for item in self.releases.values() if item['id'] == int(tail.split('/')[-1]))
            release.update(payload)
            return copy.deepcopy(release)
        if tail == '/check-runs/9':
            return {'head_sha': SHA, 'app': {'slug': 'github-actions'}, 'status': 'completed', 'conclusion': 'success'}
        raise AssertionError((method, tail))

    def pages(self, path, key=None):
        tail = path.removeprefix(self.prefix)
        if tail == '/releases':
            return copy.deepcopy([*self.history, *self.releases.values()])
        if tail.startswith('/actions/workflows/'):
            return copy.deepcopy(self.runs)
        if tail.endswith('/jobs'):
            return [copy.deepcopy(self.job)]
        if tail.endswith('/artifacts'):
            return copy.deepcopy(self.artifacts)
        if tail.endswith('/assets'):
            release = next(item for item in self.releases.values() if item['id'] == int(tail.split('/')[-2]))
            return copy.deepcopy(release['assets'])
        raise AssertionError(tail)

    def download(self, path, destination):
        destination.write_bytes(self.payloads[path])

    def upload(self, release, path):
        if self.fail_upload == len(self.uploads):
            raise APIError(503, 'fixture upload interruption')
        identifier = 100 + len(self.uploads)
        self.uploads.append(path.name)
        self.payloads[self.prefix + f'/releases/assets/{identifier}'] = path.read_bytes()
        self.releases[release['tag_name']]['assets'].append({'id': identifier, 'name': path.name, 'state': 'uploaded'})

    def add_artifact(self, name, files, attempt=1):
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, 'w') as output:
            for filename, data in files.items():
                output.writestr(filename, data)
        identifier = len(self.artifacts) + 1
        self.artifacts.append({'id': identifier, 'name': name, 'expired': False,
                               'workflow_run': {'id': 11, 'head_sha': SHA}})
        self.payloads[self.prefix + f'/actions/artifacts/{identifier}/zip'] = buffer.getvalue()


class PolicyTests(unittest.TestCase):
    def test_main_dispatch_and_pinned_workflow_are_required(self):
        env = {'GITHUB_ACTIONS': 'true', 'GITHUB_EVENT_NAME': 'workflow_dispatch', 'GITHUB_REF': 'refs/heads/main',
               'GITHUB_SHA': SHA, 'GITHUB_REPOSITORY': REPO,
               'GITHUB_WORKFLOW_REF': REPO + '/.github/workflows/publish-release.yml@refs/heads/main'}
        publication_context(env, SHA)
        for key, value in [('GITHUB_EVENT_NAME', 'push'), ('GITHUB_REF', 'refs/heads/work'),
                           ('GITHUB_SHA', 'b' * 40), ('GITHUB_WORKFLOW_REF', REPO + '/foreign.yml@refs/heads/main')]:
            with self.subTest(key=key), self.assertRaises(ValueError):
                publication_context({**env, key: value}, SHA)

    def test_latest_exact_main_validation_fails_closed(self):
        run = trusted_run()
        self.assertEqual(latest_validation([run], REPO, SHA), run)
        for field, value in [('head_sha', 'b' * 40), ('head_branch', 'topic'), ('event', 'pull_request'),
                             ('path', '.github/workflows/impostor.yml'), ('head_repository', None)]:
            with self.subTest(field=field), self.assertRaises(ValueError):
                latest_validation([{**run, field: value}], REPO, SHA)
        for status, conclusion in [('in_progress', None), ('completed', 'failure'), ('completed', 'cancelled'),
                                   ('completed', 'skipped')]:
            newer = {**run, 'run_number': 8, 'status': status, 'conclusion': conclusion}
            with self.subTest(conclusion=conclusion), self.assertRaises(ValueError):
                latest_validation([run, newer], REPO, SHA)

    def test_summary_binds_exact_run_attempt_and_all_required_jobs(self):
        policy = json.loads((ROOT / 'scripts/ci/validation-policy.json').read_text())
        report = summary(trusted_run(), policy)
        verify_summary(report, policy, trusted_run(), 1)
        for change in ['sha', 'attempt', 'missing', 'skipped', 'false-success']:
            wrong = copy.deepcopy(report)
            if change == 'sha':
                wrong['ci']['commit'] = 'b' * 40
            elif change == 'attempt':
                wrong['ci']['run_attempt'] = '2'
            elif change == 'missing':
                del wrong['jobs']['android_sdk']
            elif change == 'skipped':
                wrong['jobs']['android_sdk']['result'] = 'skipped'
            else:
                wrong['success'] = False
            with self.subTest(change=change), self.assertRaises(ValueError):
                verify_summary(wrong, policy, trusted_run(), 1)

    def test_advancement_legacy_history_prereleases_and_track_independence(self):
        history = [{'tag_name': tag, 'draft': False} for tag in ['v0.4.0', 'suite-v0.5.0-rc.2', 'client-v0.9.0']]
        check_advancement('suite', '0.5.0-rc.10', history)
        check_advancement('suite', '0.5.0', history)
        check_advancement('container', '0.0.1', history)
        for track, version in [('suite', '0.4.0'), ('suite', '0.5.0-rc.1'), ('client', '0.8.0')]:
            with self.subTest(track=track, version=version), self.assertRaises(ValueError):
                check_advancement(track, version, history)
        check_advancement('client', '0.8.0', history, complete=True)
        self.assertEqual(release_state(None, None, SHA, '0.5.0'), 'not-published')
        self.assertEqual(release_state(None, SHA, SHA, '0.5.0'), 'incomplete')
        self.assertEqual(release_state({'draft': True, 'prerelease': False}, SHA, SHA, '0.5.0'), 'incomplete')
        self.assertEqual(release_state({'draft': False, 'prerelease': False}, SHA, SHA, '0.5.0'), 'complete')
        with self.assertRaises(ValueError):
            release_state(None, 'b' * 40, SHA, '0.5.0')


class PublisherTests(unittest.TestCase):
    def setUp(self):
        self.env = patch.dict(os.environ, {'GITHUB_RUN_ID': '90', 'GITHUB_REPOSITORY': REPO,
                                          'GITHUB_STEP_SUMMARY': '', 'GITHUB_OUTPUT': ''})
        self.env.start()
        self.addCleanup(self.env.stop)
        # Reuse packaging fixtures without inheriting/rerunning their test methods.
        fixture = test_packages.PackageTests()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        fixture.ci.update(repository=REPO, ref='refs/heads/main')
        fixture.fixtures()
        self.root, self.inputs, self.versions = fixture.root, fixture.inputs, fixture.release
        self.directory = self.root / 'suite'
        assemble(fixture.inputs, self.directory, 'suite', self.versions, SHA, '11', 1)
        self.api = FakeGitHub()
        self.publisher = Publisher(self.api, self.root / 'work', context_check=lambda commit: None)
        self.api.add_artifact(f'validation-summary-{SHA}-run11-attempt1',
                              {'validation-summary.json': json.dumps(summary(trusted_run(), self.publisher.policy)).encode()})
        self.api.add_artifact(f'release-candidate-suite-{SHA}-run11-attempt1',
                              {path.name: path.read_bytes() for path in self.directory.iterdir()})
        self.versions_patch = patch('publisher.load_release', return_value=self.versions)
        self.versions_patch.start()
        self.addCleanup(self.versions_patch.stop)
        sync = patch('publisher.planned_updates', return_value=[])
        sync.start()
        self.addCleanup(sync.stop)

    def plan(self):
        return self.publisher.preflight(SHA, ['suite'])

    def test_preflight_is_readonly_and_freezes_candidate(self):
        plan = self.plan()
        self.assertEqual(plan['commit'], SHA)
        self.assertEqual(plan['tracks']['suite']['manifest_sha256'], digest(self.directory / 'RELEASE-MANIFEST.json'))
        self.assertEqual(plan['validation'], {'id': 11, 'attempt': 1})
        self.assertFalse(self.api.writes)
        with self.assertRaises(ValueError):
            self.publisher.preflight(SHA, [])
        # Test every checkbox combination without requiring unrelated payload fixtures.
        with patch.object(self.publisher, 'candidate', return_value=(self.directory, 'f' * 64)), \
                patch.object(self.publisher, 'selected_suite'):
            for count in [1, 2, 3]:
                for selection in itertools.combinations(['suite', 'client', 'container'], count):
                    self.assertEqual(set(self.publisher.preflight(SHA, selection)['tracks']), set(selection))

    def test_publish_uploads_verified_files_then_completion_is_readonly(self):
        plan = self.plan()
        self.assertEqual(self.publisher.publish_directory(plan, 'suite', self.directory), 'published')
        self.assertFalse(self.api.releases['suite-v0.5.0']['draft'])
        self.assertEqual(set(self.api.uploads), {path.name for path in self.directory.iterdir()})
        writes, uploads = len(self.api.writes), len(self.api.uploads)
        self.assertEqual(self.publisher.publish_directory(plan, 'suite', self.directory), 'already complete')
        self.assertEqual((len(self.api.writes), len(self.api.uploads)), (writes, uploads))

    def test_interrupted_draft_reuses_identical_assets_without_overwrite(self):
        plan = self.plan()
        self.api.hide_drafts_by_tag = True
        self.api.fail_upload = 3
        with self.assertRaises(APIError):
            self.publisher.publish_directory(plan, 'suite', self.directory)
        self.assertTrue(self.api.releases['suite-v0.5.0']['draft'])
        self.api.fail_upload = None
        self.assertEqual(self.publisher.publish_directory(plan, 'suite', self.directory), 'published')
        self.assertEqual(len(self.api.uploads), len(set(self.api.uploads)))
        self.assertEqual(sum(tail == '/git/refs' for _, tail, _ in self.api.writes), 1)
        self.assertEqual(sum(method == 'POST' and tail == '/releases' for method, tail, _ in self.api.writes), 1)

    def test_client_failure_does_not_replace_completed_suite(self):
        client = self.root / 'client'
        assemble(self.inputs, client, 'client', self.versions, SHA, '11', 1)
        self.api.add_artifact(f'release-candidate-client-{SHA}-run11-attempt1',
                              {path.name: path.read_bytes() for path in client.iterdir()})
        plan = self.publisher.preflight(SHA, ['suite', 'client'])
        self.publisher.publish_directory(plan, 'suite', self.directory)
        self.api.fail_upload = len(self.api.uploads) + 1
        with self.assertRaises(APIError):
            self.publisher.publish_directory(plan, 'client', client)
        self.assertFalse(self.api.releases['suite-v0.5.0']['draft'])
        self.assertTrue(self.api.releases['client-v0.6.0']['draft'])
        suite_assets = copy.deepcopy(self.api.releases['suite-v0.5.0']['assets'])
        self.api.fail_upload = None
        self.publisher.publish_directory(plan, 'client', client)
        self.assertEqual(self.api.releases['suite-v0.5.0']['assets'], suite_assets)
        self.assertFalse(self.api.releases['client-v0.6.0']['draft'])

    def test_conflicting_draft_asset_and_tag_fail_without_overwrite(self):
        plan = self.plan()
        self.api.fail_upload = 1
        with self.assertRaises(APIError):
            self.publisher.publish_directory(plan, 'suite', self.directory)
        asset = self.api.releases['suite-v0.5.0']['assets'][0]
        self.api.payloads[self.api.prefix + f'/releases/assets/{asset["id"]}'] = b'wrong bytes'
        self.api.fail_upload = None
        with self.assertRaisesRegex(ValueError, 'conflicts'):
            self.publisher.publish_directory(plan, 'suite', self.directory)
        self.assertEqual(len(self.api.uploads), 1)
        self.api.tags['suite-v0.5.0'] = 'b' * 40
        with self.assertRaisesRegex(ValueError, 'another commit'):
            self.publisher.publish_directory(plan, 'suite', self.directory)

    def test_incomplete_or_superseded_ci_and_rewritten_main_block_writes(self):
        plan = self.plan()
        self.api.runs[0]['conclusion'] = 'failure'
        with self.assertRaises(ValueError):
            self.publisher.publish_directory(plan, 'suite', self.directory)
        self.assertFalse(self.api.writes)
        self.api.runs[0]['conclusion'] = 'success'
        self.api.runs[0]['run_attempt'] = 2
        with self.assertRaisesRegex(ValueError, 'evidence changed'):
            self.publisher.publish_directory(plan, 'suite', self.directory)
        self.api.runs[0]['run_attempt'] = 1
        self.api.main, self.api.ancestor = 'c' * 40, False
        with self.assertRaisesRegex(ValueError, 'main history'):
            self.publisher.publish_directory(plan, 'suite', self.directory)
        self.assertFalse(self.api.writes)

    def test_later_main_commit_is_allowed_without_switching_candidate(self):
        self.api.main = 'c' * 40
        self.assertEqual(self.plan()['commit'], SHA)

    def test_artifact_missing_expired_changed_or_foreign_fails(self):
        plan = self.plan()
        self.api.artifacts[1]['expired'] = True
        with self.assertRaises(ValueError):
            self.publisher.candidate(trusted_run(), 'suite', self.versions, SHA)
        self.api.artifacts[1]['expired'] = False
        with self.assertRaisesRegex(ValueError, 'changed since preflight'):
            self.publisher.candidate(trusted_run(), 'suite', self.versions, SHA, '0' * 64)
        self.api.artifacts[1]['workflow_run']['head_sha'] = 'b' * 40
        with self.assertRaises(ValueError):
            self.publisher.candidate(trusted_run(), 'suite', self.versions, SHA)
        self.assertFalse(self.api.writes)

    def test_summary_check_must_be_trusted_success_not_just_report(self):
        self.api.job['conclusion'] = 'skipped'
        with self.assertRaises(ValueError):
            self.plan()
        self.api.job['conclusion'] = 'success'
        self.api.job['check_run_url'] = 'https://elsewhere.invalid/check-runs/9'
        with self.assertRaises(ValueError):
            self.plan()


class RemoteBoundaryTests(unittest.TestCase):
    def test_artifact_zip_traversal_is_rejected(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'artifact.zip'
            with zipfile.ZipFile(path, 'w') as archive:
                archive.writestr('../escape', 'bad')
            with self.assertRaises(ValueError):
                unzip(path, Path(folder) / 'payload')

    def test_redirect_never_forwards_credentials_to_artifact_cdn(self):
        import urllib.request
        req = urllib.request.Request('https://api.github.com/start', headers={'Authorization': 'secret'})
        redirected = SafeRedirect().redirect_request(req, None, 302, '', {}, 'https://objects.example.invalid/end')
        self.assertFalse(redirected.has_header('Authorization'))
        with self.assertRaises(ValueError):
            SafeRedirect().redirect_request(req, None, 302, '', {}, 'http://objects.example.invalid/end')

    def test_only_http_404_can_mean_missing(self):
        import urllib.error
        from remote import request
        for code in [403, 404, 500]:
            opener = unittest.mock.Mock()
            opener.open.side_effect = urllib.error.HTTPError('https://api.github.com/path', code, '', {}, None)
            with patch('urllib.request.build_opener', return_value=opener):
                if code == 404:
                    self.assertIsNone(request('https://api.github.com/path', missing=True)[0])
                else:
                    with self.assertRaises(APIError):
                        request('https://api.github.com/path', missing=True)


if __name__ == '__main__':
    unittest.main()
