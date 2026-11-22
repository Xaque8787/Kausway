#!/usr/bin/env python3
"""Assemble or verify release files locally. Never create tags/releases or push images."""
import argparse
import os
from pathlib import Path
import subprocess

from packages import assemble, verify_candidate
from versions import ROOT, load_release


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--track', choices=['suite', 'client'], required=True)
    parser.add_argument('--inputs', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--commit', required=True)
    parser.add_argument('--validation-run-id')
    parser.add_argument('--validation-attempt', type=int)
    parser.add_argument('--allow-dirty', action='store_true', help='Local testing only, forbidden in Actions')
    parser.add_argument('--verify-only', action='store_true')
    args = parser.parse_args()
    release = load_release()
    if args.verify_only:
        verify_candidate(args.output, args.track, release, args.commit)
    else:
        if subprocess.check_output(['git', '-C', str(ROOT), 'rev-parse', 'HEAD'], text=True).strip() != args.commit:
            parser.error('Candidate commit must be this source checkout HEAD')
        if args.inputs is None or (args.validation_run_id is None) != (args.validation_attempt is None):
            parser.error('Inputs required; validation run ID and attempt must be provided together')
        if os.environ.get('GITHUB_ACTIONS') == 'true' and args.validation_run_id is None:
            parser.error('Actions candidate assembly requires explicit validation run identity')
        assemble(args.inputs, args.output, args.track, release, args.commit,
                 args.validation_run_id, args.validation_attempt, args.allow_dirty)
    print(f'Verified {args.track} {release[args.track]["version"]} candidate: {args.output}')


if __name__ == '__main__':
    main()
