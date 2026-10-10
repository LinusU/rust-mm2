#!/usr/bin/env python3
"""Replace one DAVE member in a separate copy without relocating other members.

Only replacements that fit the member's old stored range are accepted. The
replacement is stored raw by setting both size fields to its byte length.
"""
import argparse
import hashlib
import json
import pathlib
import struct


def patch(source, member, payload_path, destination):
    if source.resolve() == destination.resolve():
        raise ValueError('destination must differ from the source archive')
    original = source.read_bytes()
    payload = payload_path.read_bytes()
    if original[:4] != b'DAVE':
        raise ValueError('source is not DAVE')
    count, names_offset, names_size = struct.unpack_from('<III', original, 4)
    names_base = 0x800 + names_offset
    names_end = names_base + names_size
    if 0x800 + count * 16 > len(original) or names_end > len(original):
        raise ValueError('archive table is out of bounds')
    matches = []
    for index in range(count):
        record = 0x800 + index * 16
        name_offset, offset, size, stored = struct.unpack_from('<IIII', original, record)
        start = names_base + name_offset
        if not names_base <= start < names_end:
            raise ValueError('archive filename offset is out of bounds')
        end = original.find(b'\0', start, names_end)
        if end < 0:
            raise ValueError('archive filename is unterminated')
        name = original[start:end].decode('ascii').replace('\\', '/').lower()
        if name == member.replace('\\', '/').lower():
            matches.append((record, offset, size, stored))
    if len(matches) != 1:
        raise ValueError(f'expected one {member!r} member, found {len(matches)}')
    record, offset, size, stored = matches[0]
    if offset + stored > len(original) or len(payload) > stored:
        raise ValueError('replacement does not fit the existing member range')
    patched = bytearray(original)
    patched[offset:offset + len(payload)] = payload
    struct.pack_into('<II', patched, record + 8, len(payload), len(payload))
    # Check the complete unchanged ranges, including the unused payload tail.
    ranges = sorted([(record + 8, record + 16), (offset, offset + len(payload))])
    cursor = 0
    for lo, hi in ranges:
        if patched[cursor:lo] != original[cursor:lo]:
            raise ValueError('unexpected changes outside the target member')
        cursor = hi
    if patched[cursor:] != original[cursor:]:
        raise ValueError('unexpected changes after the target member')
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_bytes(patched)
    return dict(source=str(source), destination=str(destination), member=member,
                archive_bytes=len(original), record_offset=record,
                data_offset=offset, old_size=size, old_stored_size=stored,
                replacement_size=len(payload),
                replacement_sha256=hashlib.sha256(payload).hexdigest(),
                archive_sha256=hashlib.sha256(patched).hexdigest())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('archive', type=pathlib.Path)
    parser.add_argument('member')
    parser.add_argument('payload', type=pathlib.Path)
    parser.add_argument('output', type=pathlib.Path)
    args = parser.parse_args()
    print(json.dumps(patch(args.archive, args.member, args.payload, args.output), indent=2))


if __name__ == '__main__':
    main()
