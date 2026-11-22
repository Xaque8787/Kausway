#!/usr/bin/env python3
"""Normalize SDK sources; reject overlaps/conflicts and retain deterministic licenses."""
import argparse
from pathlib import Path, PurePosixPath
import zipfile
import warnings

ROOT = Path(__file__).resolve().parents[2]


def normalize(source, output):
    entries = {}
    with zipfile.ZipFile(source) as archive:
        offsets = [entry.header_offset for entry in archive.infolist()]
        if len(offsets) != len(set(offsets)):
            raise zipfile.BadZipFile('Source entries share a local header (overlapping ZIP records)')
        for entry in archive.infolist():
            path = PurePosixPath(entry.filename)
            if path.is_absolute() or '..' in path.parts or '\\' in entry.filename or entry.file_size > 16 * 1024 * 1024:
                raise ValueError('Unsafe/oversized source JAR path')
            if entry.is_dir():
                continue
            # Some Python builds warn where others raise BadZipFile. Treat both
            # as corruption; never disable the ZIP overlap protection.
            with warnings.catch_warnings():
                warnings.filterwarnings('error', message='Overlapped entries:.*', category=UserWarning)
                try:
                    data = archive.read(entry)
                except UserWarning as error:
                    raise zipfile.BadZipFile('Overlapping source JAR entries') from error
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
