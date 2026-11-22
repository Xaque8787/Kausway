#!/usr/bin/env python3
"""Normalize AGP's source JAR; deduplicate identical entries, reject conflicts."""
import argparse
from pathlib import Path, PurePosixPath
import zipfile
import warnings

ROOT = Path(__file__).resolve().parents[2]


def normalize(source, output):
    entries = {}
    with zipfile.ZipFile(source) as archive:
        for entry in archive.infolist():
            path = PurePosixPath(entry.filename)
            if path.is_absolute() or '..' in path.parts or '\\' in entry.filename or entry.file_size > 16 * 1024 * 1024:
                raise ValueError('Unsafe/oversized source JAR path')
            if entry.is_dir():
                continue
            # AGP can emit repeated central-directory records pointing to identical
            # local data. Validate CRC/content below while suppressing that known warning.
            with warnings.catch_warnings():
                warnings.filterwarnings('ignore', message='Overlapped entries:.*', category=UserWarning)
                data = archive.read(entry)
            if entry.filename in entries and entries[entry.filename] != data:
                raise ValueError('Conflicting duplicate source entries')
            entries[entry.filename] = data
    for name in ['LICENSE-MIT', 'LICENSE-APACHE']:
        entries['META-INF/' + name] = (ROOT / name).read_bytes()
    if not any(name.endswith(('.kt', '.java')) for name in entries):
        raise ValueError('No SDK API sources')
    with zipfile.ZipFile(output, 'w', compression=zipfile.ZIP_DEFLATED, compresslevel=6) as archive:
        for name, data in sorted(entries.items()):
            info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            info.create_system = 3
            info.external_attr = 0o100644 << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info, data)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    normalize(args.input, args.output)


if __name__ == '__main__':
    main()
