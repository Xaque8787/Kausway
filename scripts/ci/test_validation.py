"""Contract tests for fail-closed aggregation, artifact identity and CI wiring."""
import copy
import importlib.util
import json
import os
from pathlib import Path
import re
import tempfile
import unittest
from unittest.mock import patch

from provenance import ci_context

ROOT = Path(__file__).resolve().parents[2]


def module(relative, name):
    spec = importlib.util.spec_from_file_location(name, ROOT / relative)
    loaded = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(loaded)
    return loaded


summary = module('scripts/ci/validation-summary.py', 'validation_summary')
manifest_tool = module('scripts/ci/artifact-manifest.py', 'artifact_manifest')
container_tool = module('scripts/packaging/stage-container-artifact.py', 'container_artifact')


class ValidationTests(unittest.TestCase):
    def setUp(self):
        self.sha = 'a' * 40
        self.env = {'GITHUB_ACTIONS': 'true', 'GITHUB_SHA': self.sha, 'GITHUB_REPOSITORY': 'owner/project',
                    'GITHUB_REF': 'refs/heads/topic/new-sdk', 'GITHUB_EVENT_NAME': 'push',
                    'GITHUB_RUN_ID': '1234', 'GITHUB_RUN_ATTEMPT': '2'}
        self.context = ci_context(self.sha, self.env)
        self.policy = json.loads((ROOT / 'scripts/ci/validation-policy.json').read_text())
        self.needs = {name: {'result': 'success'} for name in self.policy['required_jobs']}

    def test_success_report_is_bound_to_commit_and_attempt(self):
        result = summary.evaluate(self.policy, self.needs, self.context)
        self.assertTrue(result['success'])
        self.assertEqual(result['ci']['commit'], self.sha)
        self.assertEqual(result['ci']['run_attempt'], '2')
        self.assertEqual(set(result['jobs']), set(self.needs))

    def test_every_non_success_outcome_blocks_publication_evidence(self):
        for job in self.needs:
            for outcome in ['failure', 'cancelled', 'skipped', 'pending', 'unknown']:
                with self.subTest(job=job, outcome=outcome):
                    needs = copy.deepcopy(self.needs)
                    needs[job]['result'] = outcome
                    self.assertFalse(summary.evaluate(self.policy, needs, self.context)['success'])

    def test_missing_or_unregistered_jobs_fail(self):
        missing = copy.deepcopy(self.needs)
        missing.pop('android_sdk')
        self.assertFalse(summary.evaluate(self.policy, missing, self.context)['success'])
        extra = {**self.needs, 'unchecked_new_job': {'result': 'success'}}
        self.assertFalse(summary.evaluate(self.policy, extra, self.context)['success'])

    def test_provenance_rejects_wrong_commit_or_missing_ci_fields(self):
        with self.assertRaises(ValueError):
            ci_context('b' * 40, self.env)
        for field in ['GITHUB_RUN_ID', 'GITHUB_RUN_ATTEMPT', 'GITHUB_REPOSITORY']:
            env = dict(self.env)
            del env[field]
            with self.assertRaises(ValueError):
                ci_context(self.sha, env)
        with self.assertRaises(ValueError):
            ci_context(self.sha, {**self.env, 'GITHUB_RUN_ATTEMPT': 'not-an-attempt'})

    def test_manifest_hashes_payload_and_rejects_links(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            with self.assertRaises(ValueError):
                manifest_tool.inventory(directory, 'BUILD-INFO.json')
            (directory / 'package.aar').write_bytes(b'test payload')
            inventory = manifest_tool.inventory(directory, 'BUILD-INFO.json')
            (directory / 'BUILD-INFO.json').write_text('previous manifest')
            self.assertEqual(inventory, manifest_tool.inventory(directory, 'BUILD-INFO.json'))
            (directory / 'package.aar').write_bytes(b'changed payload')
            self.assertNotEqual(inventory, manifest_tool.inventory(directory, 'BUILD-INFO.json'))
            (directory / 'link').symlink_to(directory / 'package.aar')
            with self.assertRaises(ValueError):
                manifest_tool.inventory(directory, 'BUILD-INFO.json')

    def test_container_accepts_only_matching_run_artifact(self):
        artifact = {'commit': self.sha, 'target': 'x86_64-unknown-linux-musl', 'component': 'suite',
                    'source_dirty': False, 'ci': self.context, 'binary_versions': container_tool.binary_versions(ROOT),
                    'versions': {k: v['version'] for k, v in container_tool.load_release(ROOT).items() if k != 'schema_version'}}
        with patch.dict(os.environ, self.env, clear=True):
            container_tool.check_provenance(artifact, self.sha, artifact['target'])
            for key, value in [('commit', 'b' * 40), ('target', 'aarch64-unknown-linux-musl'),
                               ('source_dirty', True), ('component', 'host'), ('binary_versions', {})]:
                invalid = {**artifact, key: value}
                with self.subTest(key=key), self.assertRaises(ValueError):
                    container_tool.check_provenance(invalid, self.sha, artifact['target'])
            earlier = {**artifact, 'ci': {**self.context, 'run_attempt': '1'}}
            container_tool.check_provenance(earlier, self.sha, artifact['target'])
            for changed in [{'run_attempt': '3'}, {'run_id': '9999'}]:
                invalid = {**artifact, 'ci': {**self.context, **changed}}
                with self.assertRaises(ValueError):
                    container_tool.check_provenance(invalid, self.sha, artifact['target'])
            with self.assertRaises(ValueError):
                container_tool.check_provenance(artifact, self.sha, artifact['target'], allow_dirty=True)

    def test_local_development_can_use_downloaded_ci_archive(self):
        artifact = {'commit': self.sha, 'target': 'x86_64-unknown-linux-musl', 'component': 'suite',
                    'source_dirty': False, 'ci': self.context, 'binary_versions': container_tool.binary_versions(ROOT),
                    'versions': {k: v['version'] for k, v in container_tool.load_release(ROOT).items() if k != 'schema_version'}}
        with patch.dict(os.environ, {}, clear=True):
            container_tool.check_provenance(artifact, self.sha, artifact['target'])

    def test_artifact_attempt_selection_is_order_independent_and_unambiguous(self):
        for candidates in [[(1, 'old'), (2, 'new')], [(2, 'new'), (1, 'old')]]:
            self.assertEqual(container_tool.choose_candidate(candidates), (2, 'new'))
        self.assertEqual(container_tool.choose_candidate([(1, 'reused')]), (1, 'reused'))
        for invalid in [[], [(2, 'a'), (2, 'b')]]:
            with self.assertRaises(ValueError):
                container_tool.choose_candidate(invalid)

    def test_summary_policy_covers_entire_parent_job_graph(self):
        text = (ROOT / '.github/workflows/build-validation.yml').read_text()
        # Event/permission/env fields outside jobs are deliberately excluded.
        jobs_text = text.split('\njobs:\n', 1)[1]
        jobs = set(re.findall(r'^  ([A-Za-z_][A-Za-z0-9_-]*):$', jobs_text, re.MULTILINE))
        required = set(self.policy['required_jobs'])
        self.assertEqual(jobs, required | {'validation_summary'})
        summary_job = jobs_text.split('\n  validation_summary:', 1)[1]
        self.assertIn('if: always()', summary_job)
        needs = re.search(r'^    needs: \[([^\]]+)\]$', summary_job, re.MULTILINE)[1]
        self.assertEqual({name.strip() for name in needs.split(',')}, required)

    def test_development_entrypoints_have_no_branch_or_publication_gate(self):
        parent = (ROOT / '.github/workflows/build-validation.yml').read_text()
        self.assertIn('branches: ["**"]', parent)
        for name in ['development-binaries.yml', 'development-containers.yml', 'android-sdk.yml']:
            text = (ROOT / '.github/workflows' / name).read_text()
            self.assertIn('workflow_call:', text)
            self.assertIn('workflow_dispatch:', text)
            self.assertNotIn('refs/heads/work', text)
            self.assertNotIn('packages: write', text)
            self.assertNotIn('docker/login-action', text)
            self.assertNotIn('push: true', text)
            self.assertNotIn('\n  push:', text)
            self.assertNotIn('\n  pull_request:', text)
            self.assertNotIn('\n    paths:', text)


if __name__ == '__main__':
    unittest.main()
