"""Versioned, allowlisted AtomaDB backups. Stop all writers before snapshotting."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import struct
import tempfile

SIDECARS = ('mind.jsonl', 'graph_nodes.jsonl', 'graph_edges.jsonl')

def digest(path):
    with path.open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()

def validate(root):
    manifest = json.loads((root / 'manifest.json').read_bytes())
    collections = {c['name']: c for c in manifest['collections']}
    if len(collections) != len(manifest['collections']):
        raise ValueError('duplicate collection')
    names, ids = set(), set()
    for name in manifest['segments']:
        if not re.fullmatch(r'seg-[0-9]+', name) or name in names:
            raise ValueError('unsafe or duplicate segment')
        names.add(name)
        records = json.loads((root / 'segments' / (name + '.json')).read_bytes())
        with (root / 'segments' / (name + '.vec')).open('rb') as f:
            header = f.read(12)
            if len(header) != 12:
                raise ValueError('truncated vector header')
            count, dim = struct.unpack('<QI', header)
            if count != len(records) or dim == 0:
                raise ValueError('vector count/dimension mismatch')
            for record in records:
                col = collections[record['collection']]
                if col['dim'] != dim:
                    raise ValueError('collection dimension mismatch')
                row = f.read(8 + dim * 4)
                if len(row) != 8 + dim * 4:
                    raise ValueError('truncated vector')
                chunk_id = struct.unpack_from('<Q', row)[0]
                if chunk_id != record['id'] or chunk_id in ids:
                    raise ValueError('duplicate or mismatched chunk id')
                ids.add(chunk_id)
                import math
                if not all(math.isfinite(v) for v in struct.unpack_from('<' + 'f' * dim, row, 8)):
                    raise ValueError('nonfinite vector')
            if f.read(1):
                raise ValueError('trailing vector bytes')
    if ids and manifest['next_chunk_id'] <= max(ids):
        raise ValueError('invalid next chunk id')
    return manifest

def inventory(root):
    manifest = json.loads((root / 'manifest.json').read_bytes())
    names = ['manifest.json']
    for segment in manifest['segments']:
        if not re.fullmatch(r'seg-[0-9]+', segment):
            raise ValueError('unsafe segment name')
        names += [f'segments/{segment}.json', f'segments/{segment}.vec']
    names += [name for name in SIDECARS if (root / name).exists()]
    for name in names:
        p = root / name
        if p.is_symlink() or not p.is_file() or root.resolve() not in p.resolve().parents:
            raise ValueError('nonregular or escaped source')
    return names

def snapshot(source, store):
    source, store = Path(source), Path(store)
    names = inventory(source)
    before = {n: digest(source / n) for n in names}
    objects = store / 'objects'
    objects.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        staged = Path(tmp)
        receipts = {}
        for name in names:
            dest = staged / name
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source / name, dest)
            sha = digest(dest)
            if sha != before[name]:
                raise ValueError('source changed during snapshot')
            receipts[name] = {'sha256': sha, 'size': dest.stat().st_size}
        validate(staged)
        if inventory(source) != names or any(digest(source / n) != before[n] for n in names):
            raise ValueError('source changed; stop writers and retry')
        for name, receipt in receipts.items():
            obj = objects / (receipt['sha256'] + '.blob')
            if obj.exists():
                if digest(obj) != receipt['sha256']:
                    raise ValueError('existing backup object corrupt')
            else:
                shutil.copyfile(staged / name, obj)
        payload = json.dumps({'version': 1, 'files': receipts}, sort_keys=True, indent=2).encode() + b'\n'
        identity = hashlib.sha256(payload).hexdigest()
        folder = store / 'snapshots'
        folder.mkdir(exist_ok=True)
        output = folder / (identity + '.json')
        output.write_bytes(payload)
        return output

def restore(receipt_path, target):
    receipt_path, target = Path(receipt_path), Path(target)
    if target.exists():
        raise ValueError('restore destination must not exist')
    raw = receipt_path.read_bytes()
    if receipt_path.stem != hashlib.sha256(raw).hexdigest():
        raise ValueError('snapshot receipt hash mismatch')
    receipt = json.loads(raw)
    if receipt['version'] != 1:
        raise ValueError('unsupported backup version')
    target.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=target.parent) as tmp:
        staged = Path(tmp) / 'db'
        staged.mkdir()
        for name, metadata in receipt['files'].items():
            if name not in ('manifest.json', *SIDECARS) and not re.fullmatch(r'segments/seg-[0-9]+\.(json|vec)', name):
                raise ValueError('unsafe backup path')
            sha = metadata['sha256']
            if not re.fullmatch('[0-9a-f]{64}', sha):
                raise ValueError('invalid object hash')
            obj = receipt_path.parent.parent / 'objects' / (sha + '.blob')
            if obj.is_symlink() or obj.stat().st_size != metadata['size'] or digest(obj) != sha:
                raise ValueError('missing/corrupt object; run git lfs pull')
            dest = staged / name
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(obj, dest)
        validate(staged)
        if set(inventory(staged)) != set(receipt['files']):
            raise ValueError('snapshot file list mismatch')
        os.rename(staged, target)
    return target

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=['snapshot', 'restore', 'validate'])
    parser.add_argument('source', type=Path)
    parser.add_argument('destination', nargs='?', type=Path)
    args = parser.parse_args()
    if args.operation == 'validate':
        validate(args.source)
        print('valid')
    else:
        if args.destination is None:
            parser.error('destination required')
        print(globals()[args.operation](args.source, args.destination))
