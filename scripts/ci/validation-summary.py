#!/usr/bin/env python3
"""Fail-closed aggregate of explicitly required jobs; never infer skip reasons."""
import argparse
import json
import os
from pathlib import Path
import sys

from provenance import ci_context

ROOT = Path(__file__).resolve().parents[2]


def evaluate(policy, needs, context):
    if policy['schema_version'] != 1 or not policy['required_jobs']:
        raise ValueError('Invalid validation policy')
    errors = []
    if set(needs) != set(policy['required_jobs']):
        errors.append('Job graph differs from the required validation policy')
    jobs = {}
    for job, description in policy['required_jobs'].items():
        result = needs.get(job, {}).get('result', 'missing')
        jobs[job] = {'description': description, 'result': result, 'required': True}
        if result != 'success':
            errors.append(f'{job}: required success, got {result}')
    return {'schema_version': 1, 'workflow': policy['workflow'], 'ci': context,
            'success': not errors, 'jobs': jobs, 'errors': errors}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--needs', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    context = ci_context(os.environ['GITHUB_SHA'])
    if any(not value for value in context.values()):
        raise ValueError('Validation evidence requires complete CI provenance')
    policy = json.loads((ROOT / 'scripts/ci/validation-policy.json').read_text())
    report = evaluate(policy, json.loads(args.needs.read_text()), context)
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    lines = ['## Validation summary', '', f"Commit: `{context['commit']}`", '',
             '| Required job | Result |', '| --- | --- |']
    lines += [f"| {name} | {entry['result']} |" for name, entry in report['jobs'].items()]
    lines += ['', 'PASS' if report['success'] else 'FAIL', *report['errors'], '']
    if os.environ.get('GITHUB_STEP_SUMMARY'):
        with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as output:
            output.write('\n'.join(lines))
    print('\n'.join(lines))
    return 0 if report['success'] else 1


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (ValueError, KeyError, OSError) as error:
        print(f'Cannot establish validation success: {error}', file=sys.stderr)
        sys.exit(1)
