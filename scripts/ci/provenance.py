"""Shared, non-secret CI identity for downloadable artifacts and validation evidence."""
import os
import re


def ci_context(commit, env=None):
    env = os.environ if env is None else env
    if not re.fullmatch('[0-9a-f]{40}', commit):
        raise ValueError('Expected an exact full source commit SHA')
    if env.get('GITHUB_SHA') and env['GITHUB_SHA'] != commit:
        raise ValueError('Artifact commit does not match this Actions run')
    result = {'commit': commit, 'repository': env.get('GITHUB_REPOSITORY'),
              'ref': env.get('GITHUB_REF'), 'event': env.get('GITHUB_EVENT_NAME'),
              'run_id': env.get('GITHUB_RUN_ID'), 'run_attempt': env.get('GITHUB_RUN_ATTEMPT')}
    for field in ['run_id', 'run_attempt']:
        if result[field] is not None and not re.fullmatch('[1-9][0-9]*', result[field]):
            raise ValueError(f'Invalid CI {field}')
    if env.get('GITHUB_ACTIONS') == 'true' and any(not value for value in result.values()):
        raise ValueError('Incomplete Actions provenance')
    return result
