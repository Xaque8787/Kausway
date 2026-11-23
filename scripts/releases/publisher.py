"""Release operations. Callers inject the HTTP boundary for tests; no import-time IO."""
import copy
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import urllib.parse

from packages import digest, verify_candidate
from release_policy import (WORKFLOW, check_advancement, latest_validation, publication_context,
                            release_state, suite_latest_eligible, verify_summary)
from remote import observed_write, unzip, wait_visible
from versions import ROOT, load_release, planned_updates


def assert_context(commit):
    publication_context(os.environ, commit)
    head = subprocess.check_output(['git', '-C', str(ROOT), 'rev-parse', 'HEAD'], text=True).strip()
    dirty = subprocess.check_output(['git', '-C', str(ROOT), 'status', '--porcelain'], text=True).strip()
    if head != commit or dirty:
        raise ValueError('Publisher checkout must be clean and pinned to the dispatch SHA')


class Publisher:
    def __init__(self, api, work, context_check=assert_context):
        self.api, self.work, self.context_check = api, work, context_check
        self.work.mkdir(parents=True, exist_ok=True)
        self.prefix = api.prefix
        self.policy = json.loads((ROOT / 'scripts/ci/validation-policy.json').read_text())
        self.main_checked = set()

    def guard(self, commit):
        self.context_check(commit)
        head = self.api.call(self.prefix + '/git/ref/heads/main')['object']['sha']
        if head not in self.main_checked:
            comparison = self.api.call(self.prefix + f'/compare/{commit}...{head}')
            if comparison['merge_base_commit']['sha'] != commit or comparison['status'] not in ('ahead', 'identical'):
                raise ValueError('Pinned commit is no longer in main history')
            self.main_checked.add(head)

    def tag_commit(self, tag):
        ref = self.api.call(self.prefix + '/git/ref/tags/' + urllib.parse.quote(tag, safe=''), missing=True)
        if ref is None:
            return None
        obj = ref['object']
        for _ in range(8):
            if obj['type'] == 'commit':
                return obj['sha']
            if obj['type'] != 'tag':
                break
            obj = self.api.call(self.prefix + '/git/tags/' + obj['sha'])['object']
        raise ValueError('Unsupported/cyclic release tag')

    def release(self, tag):
        release = self.api.call(self.prefix + '/releases/tags/' + urllib.parse.quote(tag, safe=''), missing=True)
        if release is not None:
            return release
        # GitHub's by-tag endpoint is for published releases; drafts can require
        # authenticated listing. Never mistake a recoverable draft for absence.
        matches = [item for item in self.api.pages(self.prefix + '/releases') if item['tag_name'] == tag]
        if len(matches) > 1:
            raise ValueError('Multiple releases share this tag; inspect before retrying')
        return matches[0] if matches else None

    def checked_tag(self, tag, commit):
        def read():
            actual = self.tag_commit(tag)
            if actual is not None and actual != commit:
                raise ValueError(f'Tag {tag} points to another commit: expected {commit}, observed {actual}; never overwrite it')
            return actual
        return wait_visible(read, f'Tag {tag} at commit {commit}')

    def checked_release(self, release, tag, commit, prerelease, draft=None):
        identifier = release['id']
        def read():
            current = self.api.call(self.prefix + f'/releases/{identifier}', missing=True)
            if current is None:
                return None
            if not isinstance(current.get('draft'), bool):
                raise ValueError(f'Release {identifier} has invalid draft state; stop and inspect')
            expected = {'id': identifier, 'tag_name': tag, 'prerelease': prerelease}
            if draft is not None:
                expected['draft'] = draft
            for field, wanted in expected.items():
                if current.get(field) != wanted:
                    raise ValueError(f'Release {identifier} changed: {field} expected {wanted!r}, '
                                     f'observed {current.get(field)!r}; stop and inspect')
            return current
        current = wait_visible(read, f'Release {identifier} ({tag})')
        self.checked_tag(tag, commit)
        return current

    def visible_assets(self, release, names):
        def read():
            found = self.release_assets(release)
            actual = [asset['name'] for asset in found]
            if len(actual) != len(set(actual)) or set(actual) - names:
                raise ValueError(f'Release {release["id"]} contains duplicate/unexpected assets; stop and inspect')
            if set(actual) != names or any(asset['state'] != 'uploaded' for asset in found):
                return None
            return found
        return wait_visible(read, f'Uploaded asset inventory for release {release["id"]}')

    def promote_suite_latest(self, plan, track, release):
        version = plan['versions'][track]['version']
        history = self.api.pages(self.prefix + '/releases') if track == 'suite' else []
        if not suite_latest_eligible(track, version, history):
            return
        tag, commit = release['tag_name'], plan['commit']
        self.checked_release(release, tag, commit, False, draft=False)
        current = self.api.call(self.prefix + '/releases/latest', missing=True)
        if current is not None:
            if current['id'] == release['id']:
                return
            # Even if history listing lagged, an observed newer suite must win.
            if not suite_latest_eligible(track, version, [current]):
                return
        self.validate_plan(plan)
        self.guard(commit)
        self.checked_release(release, tag, commit, False, draft=False)
        if not suite_latest_eligible(track, version, self.api.pages(self.prefix + '/releases')):
            return
        def promoted():
            latest = self.api.call(self.prefix + '/releases/latest', missing=True)
            return latest if latest is not None and latest['id'] == release['id'] else None
        observed_write(lambda: self.api.call(self.prefix + f'/releases/{release["id"]}', 'PATCH',
                                             {'make_latest': 'true'}), promoted, f'Latest promotion for {tag}')
        wait_visible(promoted, f'GitHub Latest promotion for {tag}')

    def artifact(self, run, name):
        matches = [item for item in self.api.pages(self.prefix + f'/actions/runs/{run["id"]}/artifacts', 'artifacts')
                   if item['name'] == name and not item['expired']]
        if len(matches) != 1 or matches[0]['workflow_run']['id'] != run['id'] or matches[0]['workflow_run']['head_sha'] != run['head_sha']:
            raise ValueError(f'Missing/ambiguous/expired validation artifact: {name}')
        return matches[0]

    def download_artifact(self, artifact):
        directory = Path(tempfile.mkdtemp(prefix='artifact-', dir=self.work))
        archive = directory / 'download.zip'
        self.api.download(self.prefix + f'/actions/artifacts/{artifact["id"]}/zip', archive)
        if artifact.get('digest') and artifact['digest'] != 'sha256:' + digest(archive):
            raise ValueError('Actions artifact ZIP digest mismatch')
        payload = directory / 'payload'
        unzip(archive, payload)
        archive.unlink()
        return payload

    def validation(self, commit, expected=None):
        self.guard(commit)
        runs = self.api.pages(self.prefix + '/actions/workflows/build-validation.yml/runs?' +
                              urllib.parse.urlencode({'head_sha': commit, 'branch': 'main'}), 'workflow_runs')
        run = latest_validation(runs, self.api.repository, commit)
        if expected and (run['id'], run['run_attempt']) != (expected['id'], expected['attempt']):
            raise ValueError('Validation evidence changed; rerun the whole publisher to select it explicitly')
        summaries, summary_attempt = [], None
        for number in range(run['run_attempt'], 0, -1):
            jobs = self.api.pages(self.prefix + f'/actions/runs/{run["id"]}/attempts/{number}/jobs', 'jobs')
            summaries = [job for job in jobs if job['name'] == 'Validation summary' and job['run_id'] == run['id']]
            if summaries:
                summary_attempt = number
                break
        if not summaries or len(summaries) != 1:
            raise ValueError('Missing trusted Validation summary job')
        job = summaries[0]
        if job['head_sha'] != commit or job['status'] != 'completed' or job['conclusion'] != 'success':
            raise ValueError('Latest summary check did not succeed')
        url = job['check_run_url']
        expected_prefix = 'https://api.github.com' + self.prefix + '/check-runs/'
        if not url.startswith(expected_prefix):
            raise ValueError('Unexpected summary check identity')
        check = self.api.call(url.removeprefix('https://api.github.com'))
        if check['head_sha'] != commit or check['app']['slug'] != 'github-actions' or check['status'] != 'completed' or check['conclusion'] != 'success':
            raise ValueError('Summary is not a successful GitHub Actions check for this SHA')
        name = f'validation-summary-{commit}-run{run["id"]}-attempt{summary_attempt}'
        report = json.loads((self.download_artifact(self.artifact(run, name)) / 'validation-summary.json').read_text())
        verify_summary(report, self.policy, run, summary_attempt)
        return run

    def candidate(self, run, track, release, commit, expected_digest=None):
        prefix = f'release-candidate-{track}-{commit}-run{run["id"]}-attempt'
        matches = [item for item in self.api.pages(self.prefix + f'/actions/runs/{run["id"]}/artifacts', 'artifacts')
                   if item['name'].startswith(prefix) and item['name'][len(prefix):].isdigit() and not item['expired']
                   and 0 < int(item['name'][len(prefix):]) <= run['run_attempt']]
        if not matches:
            raise ValueError(f'No retained {track} candidate; rerun main validation')
        artifact = max(matches, key=lambda item: int(item['name'][len(prefix):]))
        artifact = self.artifact(run, artifact['name'])
        path = self.download_artifact(artifact)
        info = verify_candidate(path, track, release, commit)
        self.check_clean(info, run)
        checksum = digest(path / 'RELEASE-MANIFEST.json')
        if expected_digest and checksum != expected_digest:
            raise ValueError('Candidate changed since preflight')
        return path, checksum

    def check_clean(self, info, run=None):
        if info['source_dirty'] is not False or not info['sources']:
            raise ValueError('Release must have clean, recorded build sources')
        if run:
            for source in info['sources']:
                ci = source['ci']
                if (source['source_dirty'] is not False or ci['commit'] != run['head_sha']
                        or ci['repository'] != self.api.repository or ci['ref'] != 'refs/heads/main'
                        or ci['event'] != run['event'] or ci['run_id'] != str(run['id'])
                        or not re.fullmatch('[1-9][0-9]*', str(ci['run_attempt']))
                        or int(ci['run_attempt']) > run['run_attempt']):
                    raise ValueError('Candidate source provenance does not match trusted validation')

    def release_assets(self, release):
        return self.api.pages(self.prefix + f'/releases/{release["id"]}/assets')

    def download_release(self, release, track, versions, commit=None, *, assets=None):
        directory = Path(tempfile.mkdtemp(prefix='published-', dir=self.work))
        seen = set()
        for asset in self.release_assets(release) if assets is None else assets:
            name = asset['name']
            if Path(name).name != name or name in seen or asset['state'] != 'uploaded':
                raise ValueError('Invalid/incomplete/duplicate release asset')
            seen.add(name)
            self.api.download(self.prefix + f'/releases/assets/{asset["id"]}', directory / name)
        info = verify_candidate(directory, track, versions, commit)
        self.check_clean(info)
        self.checked_tag(release['tag_name'], info['commit'])
        if release['prerelease'] != info['prerelease']:
            raise ValueError('Published asset/tag identity mismatch')
        return directory, info

    def selected_suite(self, versions):
        selected = copy.deepcopy(versions)
        selected['suite']['version'] = versions['container']['suite_version']
        tag = 'suite-v' + selected['suite']['version']
        release = self.release(tag)
        if release is None or release['draft']:
            raise ValueError(f'Selected suite {tag} must be published first (new-layout assets required)')
        return self.download_release(release, 'suite', selected)

    def preflight(self, commit, selected):
        if not selected or set(selected) - {'suite', 'client', 'container'}:
            raise ValueError('Select at least one valid release track')
        self.guard(commit)
        if planned_updates():
            raise ValueError('Version synchronization check failed')
        versions = load_release()
        run = self.validation(commit)
        history = self.api.pages(self.prefix + '/releases')
        plan = {'schema_version': 1, 'commit': commit, 'repository': self.api.repository, 'versions': versions,
                'publisher_run_id': os.environ['GITHUB_RUN_ID'], 'validation': {'id': run['id'], 'attempt': run['run_attempt']},
                'tracks': {}}
        for track in selected:
            version = versions[track]['version']
            tag = f'{track}-v{version}'
            release = self.release(tag)
            tagged = self.checked_tag(tag, commit) if release is not None else self.tag_commit(tag)
            state = release_state(release, tagged, commit, version)
            check_advancement(track, version, history, complete=state == 'complete')
            item = {'state': state, 'tag': tag, 'version': version}
            if state == 'complete':
                _, info = self.download_release(release, track, versions, commit)
                item['manifest_sha256'] = None
            elif track != 'container':
                _, checksum = self.candidate(run, track, versions, commit)
                item['manifest_sha256'] = checksum
            plan['tracks'][track] = item
        if 'container' in selected:
            if 'suite' not in selected or versions['suite']['version'] != versions['container']['suite_version']:
                self.selected_suite(versions)
        return plan

    def validate_plan(self, plan):
        if (plan['schema_version'] != 1 or plan['repository'] != self.api.repository
                or plan['publisher_run_id'] != os.environ['GITHUB_RUN_ID'] or plan['versions'] != load_release()):
            raise ValueError('Release plan identity changed')
        return self.validation(plan['commit'], plan['validation'])

    def partial_assets(self, release, directory):
        expected = {path.name: path for path in directory.iterdir()}
        found = {}
        for asset in self.release_assets(release):
            if asset['name'] not in expected or asset['name'] in found or asset['state'] != 'uploaded':
                raise ValueError('Draft contains unexpected/unfinished assets; inspect it before retrying')
            path = expected[asset['name']]
            copy_path = Path(tempfile.mkdtemp(prefix='asset-check-', dir=self.work)) / path.name
            self.api.download(self.prefix + f'/releases/assets/{asset["id"]}', copy_path)
            if digest(copy_path) != digest(path):
                raise ValueError('Draft asset conflicts with this validated candidate; never overwrite it')
            found[path.name] = True
        return found

    def publish_directory(self, plan, track, directory):
        commit, versions = plan['commit'], plan['versions']
        self.validate_plan(plan)
        info = verify_candidate(directory, track, versions, commit)
        self.check_clean(info)
        tag, version = plan['tracks'][track]['tag'], versions[track]['version']
        release = self.release(tag)
        tagged = self.checked_tag(tag, commit) if release is not None else self.tag_commit(tag)
        state = release_state(release, tagged, commit, version)
        check_advancement(track, version, self.api.pages(self.prefix + '/releases'), complete=state == 'complete')
        if state == 'complete':
            self.download_release(release, track, versions, commit)
            self.promote_suite_latest(plan, track, release)
            return 'already complete'
        if tagged is None:
            self.guard(commit)
            observed_write(lambda: self.api.call(self.prefix + '/git/refs', 'POST',
                                                 {'ref': 'refs/tags/' + tag, 'sha': commit}),
                           lambda: self.tag_commit(tag), f'Tag creation for {tag}')
        self.checked_tag(tag, commit)
        if release is None:
            self.guard(commit)
            release = observed_write(lambda: self.api.call(self.prefix + '/releases', 'POST',
                                      {'tag_name': tag, 'target_commitish': commit, 'name': f'{track.title()} {version}',
                                       'draft': True, 'prerelease': info['prerelease'], 'make_latest': 'false',
                                       'body': f'Source: `{commit}`\n\nVerified {track} release files. See RELEASE-MANIFEST.json and SHA256SUMS.\n'}),
                                     lambda: self.release(tag), f'Draft creation for {tag}')
        release = self.checked_release(release, tag, commit, info['prerelease'], draft=True)
        found = self.partial_assets(release, directory)
        for path in sorted(directory.iterdir()):
            if path.name in found:
                continue
            self.guard(commit)
            current = self.checked_release(release, tag, commit, info['prerelease'], draft=True)
            def uploaded():
                # A lost response can follow a successful upload. Reuse it only
                # after checking the actual bytes; never repeat an uncertain POST.
                matching = [asset for asset in self.release_assets(current) if asset['name'] == path.name]
                if len(matching) > 1:
                    raise ValueError(f'Duplicate uploaded asset {path.name}; stop and inspect')
                if not matching or matching[0]['state'] != 'uploaded':
                    return None
                self.partial_assets(current, directory)
                return matching[0]
            observed_write(lambda: self.api.upload(current, path), uploaded, f'Upload of {path.name}')
        assets = self.visible_assets(release, {path.name for path in directory.iterdir()})
        self.download_release(release, track, versions, commit, assets=assets)
        self.validate_plan(plan)
        self.guard(commit)
        self.checked_release(release, tag, commit, info['prerelease'], draft=True)
        def published():
            current = self.checked_release(release, tag, commit, info['prerelease'])
            return current if not current['draft'] else None
        observed_write(lambda: self.api.call(self.prefix + f'/releases/{release["id"]}', 'PATCH',
                                             {'draft': False, 'make_latest': 'false'}),
                       published, f'Publication of {tag}')
        release = wait_visible(published, f'Published state for {tag}')
        self.promote_suite_latest(plan, track, release)
        return 'published'
