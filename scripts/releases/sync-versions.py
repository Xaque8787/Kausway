#!/usr/bin/env python3
"""Synchronize version declarations from releases.toml; never build or publish."""
import argparse
import sys
from pathlib import Path

sys.dont_write_bytecode = True

from versions import ROOT, TRACKS, planned_updates, version


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=ROOT, help='Repository root (defaults to this checkout)')
    parser.add_argument('--check', action='store_true', help='Report drift without writing any files')
    parser.add_argument('--scope', choices=('all', *TRACKS), default='all',
                        help='Default checks all components; scoped checks support partial Docker stages')
    parser.add_argument('--print-version', choices=TRACKS, help='Print only the selected version on success')
    args = parser.parse_args()
    root = args.root.resolve()
    try:
        updates = planned_updates(root, scope=args.scope)
        if args.check and updates:
            for path in updates:
                print(f'Version drift: {path.relative_to(root)}', file=sys.stderr)
            print('Run python3 scripts/releases/sync-versions.py and commit the changes.', file=sys.stderr)
            return 1
        if not args.check:
            for path, text in updates.items():
                path.write_text(text)
                if not args.print_version:
                    print(f'Updated {path.relative_to(root)}')
        if args.print_version:
            print(version(args.print_version, root))
        return 0
    except (ValueError, KeyError, OSError, TypeError) as error:
        print(f'Version validation failed: {error}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
