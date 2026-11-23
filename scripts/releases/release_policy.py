"""Pure release/validation decisions, independently testable without remote writes."""
from versions import parse_version

WORKFLOW = '.github/workflows/build-validation.yml'


def publication_context(env, commit):
    repo = env.get('GITHUB_REPOSITORY')
    expected = f'{repo}/.github/workflows/publish-release.yml@refs/heads/main'
    if (env.get('GITHUB_ACTIONS') != 'true' or env.get('GITHUB_EVENT_NAME') != 'workflow_dispatch'
            or env.get('GITHUB_REF') != 'refs/heads/main' or env.get('GITHUB_SHA') != commit
            or env.get('GITHUB_WORKFLOW_REF') != expected):
        raise ValueError('Publication requires manual Publish release on its pinned main commit')


def latest_validation(runs, repository, commit):
    applicable = [run for run in runs if run['head_sha'] == commit and run['head_branch'] == 'main'
                  and run['event'] in ('push', 'workflow_dispatch') and run['path'] == WORKFLOW
                  and run['repository']['full_name'] == repository
                  and (run.get('head_repository') or {}).get('full_name') == repository]
    if not applicable:
        raise ValueError('No trusted main validation run for this exact commit')
    run = max(applicable, key=lambda item: (item['run_number'], item['run_attempt']))
    if run['status'] != 'completed' or run['conclusion'] != 'success':
        raise ValueError('Latest applicable main validation is missing/pending/failed/canceled')
    return run


def verify_summary(report, policy, run, summary_attempt):
    ci = report['ci']
    expected = {'commit': run['head_sha'], 'repository': run['repository']['full_name'], 'ref': 'refs/heads/main',
                'event': run['event'], 'run_id': str(run['id']), 'run_attempt': str(summary_attempt)}
    if (report['schema_version'] != 1 or report['workflow'] != WORKFLOW or report['success'] is not True
            or report['errors'] or ci != expected or set(report['jobs']) != set(policy['required_jobs'])):
        raise ValueError('Validation summary identity/policy does not match the trusted main run')
    if any(job['result'] != 'success' or job['required'] is not True for job in report['jobs'].values()):
        raise ValueError('A required validation did not pass')


def history_version(tag, track):
    prefix = track + '-v'
    if tag.startswith(prefix):
        return parse_version(tag[len(prefix):])
    if track in ('suite', 'client') and tag.startswith('v'):
        try:
            return parse_version(tag[1:])
        except ValueError:
            return None
    return None


def check_advancement(track, version, releases, complete=False):
    current = parse_version(version)
    previous = [history_version(item['tag_name'], track) for item in releases if not item['draft']]
    previous = [item for item in previous if item is not None]
    if not complete and previous and current <= max(previous):
        raise ValueError(f'{track} {version} does not advance published history; bump releases.toml and synchronize')


def suite_latest_eligible(track, version, releases):
    """GitHub has one Latest label; only the newest stable suite may own it."""
    current = parse_version(version)
    if track != 'suite' or current.prerelease is not None:
        return False
    for release in releases:
        previous = history_version(release['tag_name'], 'suite')
        if not release['draft'] and previous is not None and previous.prerelease is None and previous > current:
            return False
    return True


def release_state(release, tag_commit, commit, version):
    if tag_commit is not None and tag_commit != commit:
        raise ValueError('Existing release tag points to another commit; never overwrite it')
    if release is None:
        return 'incomplete' if tag_commit else 'not-published'
    if tag_commit != commit or release['prerelease'] != (parse_version(version).prerelease is not None):
        raise ValueError('Existing release identity/prerelease state conflicts')
    return 'incomplete' if release['draft'] else 'complete'
