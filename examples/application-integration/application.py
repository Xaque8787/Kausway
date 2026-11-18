#!/usr/bin/env python3
"""Minimal application: own one manager process and delegate settings to its UI."""
import argparse
from pathlib import Path
from kausway import SuiteManager

parser = argparse.ArgumentParser()
parser.add_argument('--manager', type=Path, required=True)
parser.add_argument('--config', type=Path, required=True)
parser.add_argument('--data-root', type=Path, required=True)
args = parser.parse_args()

with SuiteManager(args.manager, config=args.config, data_root=args.data_root,
                  application_name='ExampleApplication') as kausway:
    print('Application and local Kausway administration are ready.')
    while True:
        command = input('Enter configure, status, or quit: ').strip()
        if command == 'configure':
            # A desktop application wires this call to its Configure Kausway button.
            kausway.admin_entry(open_browser=True)
        elif command == 'status':
            status = kausway.request('status')['status']
            print('Host:', status['host']['observed'])
        elif command == 'quit':
            break
