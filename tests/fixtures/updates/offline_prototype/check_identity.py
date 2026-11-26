"""Compare all fixture bytes and verify the recorded byte manifest."""
import argparse
import hashlib
import json
from pathlib import Path


def tree(directory):
    return {str(path.relative_to(directory)): path.read_bytes()
            for path in directory.rglob('*') if path.is_file()}


def compare(first, second):
    a, b = tree(first), tree(second)
    if a.keys() != b.keys():
        raise ValueError('file inventories differ')
    changed = [name for name in a if a[name] != b[name]]
    if changed:
        raise ValueError('bytes differ: ' + ', '.join(sorted(changed)))
    manifest = json.loads(a['byte-manifest.json'])['files']
    if set(manifest) != set(a) - {'byte-manifest.json'}:
        raise ValueError('manifest inventory differs')
    for name, expected in manifest.items():
        data = a[name]
        if len(data) != expected['length'] or hashlib.sha256(data).hexdigest() != expected['sha256']:
            raise ValueError('manifest mismatch: ' + name)
    return {'identity': 'BYTE-FOR-BYTE IDENTICAL', 'files_compared': len(a),
            'manifest_entries_verified': len(manifest),
            'byte_manifest_sha256': hashlib.sha256(a['byte-manifest.json']).hexdigest()}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('first', type=Path)
    parser.add_argument('second', type=Path)
    args = parser.parse_args()
    print(json.dumps(compare(args.first, args.second), indent=2, sort_keys=True))
