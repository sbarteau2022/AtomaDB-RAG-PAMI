import importlib.util
import json
from pathlib import Path
import struct
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('snapshot', Path(__file__).parents[1] / 'backup_tools/snapshot.py')
b = importlib.util.module_from_spec(spec)
spec.loader.exec_module(b)

class BackupTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.db = self.root / 'db'
        (self.db / 'segments').mkdir(parents=True)
        self.manifest = {'collections': [{'name': 'papers', 'dim': 2, 'embedder_id': 'test'}], 'segments': ['seg-00001'], 'next_chunk_id': 2}
        (self.db / 'manifest.json').write_text(json.dumps(self.manifest))
        records = [{'id': 1, 'collection': 'papers', 'doc_id': 'doc', 'section': 'a', 'ordinal': 0, 'parent': None, 'text': 'hello'}]
        (self.db / 'segments/seg-00001.json').write_text(json.dumps(records))
        self.vec = self.db / 'segments/seg-00001.vec'
        self.vec.write_bytes(struct.pack('<QIQff', 1, 2, 1, 0.5, 1.0))
        (self.db / 'mind.jsonl').write_text('{}\n')
        (self.db / 'key.json').write_text('DO NOT BACK UP')
        self.store = self.root / 'backups'

    def test_roundtrip_and_dedup(self):
        receipt = b.snapshot(self.db, self.store)
        self.assertEqual(receipt, b.snapshot(self.db, self.store))
        b.restore(receipt, self.root / 'restored')
        for name in b.inventory(self.db):
            self.assertEqual((self.db / name).read_bytes(), (self.root / 'restored' / name).read_bytes())
        self.assertFalse((self.root / 'restored/key.json').exists())
        self.assertEqual(len(list((self.store / 'objects').iterdir())), 4)

    def test_existing_restore_rejected(self):
        with self.assertRaises(ValueError):
            b.restore(b.snapshot(self.db, self.store), self.db)

    def test_corrupt_object_rejected_without_partial_restore(self):
        r = b.snapshot(self.db, self.store)
        next((self.store / 'objects').iterdir()).write_bytes(b'bad')
        target = self.root / 'restore'
        with self.assertRaises(ValueError):
            b.restore(r, target)
        self.assertFalse(target.exists())

    def test_corrupt_receipt(self):
        r = b.snapshot(self.db, self.store)
        r.write_bytes(r.read_bytes() + b' ')
        with self.assertRaises(ValueError):
            b.restore(r, self.root / 'restore')

    def test_truncated_and_nonfinite_vectors(self):
        for data in (b'', struct.pack('<QIQff', 1, 2, 1, float('nan'), 1.0), struct.pack('<QIQff', 1, 3, 1, 1.0, 1.0)):
            self.vec.write_bytes(data)
            with self.assertRaises(ValueError):
                b.snapshot(self.db, self.store)
        self.assertFalse((self.store / 'snapshots').exists())

    def test_path_traversal(self):
        self.manifest['segments'] = ['../secret']
        (self.db / 'manifest.json').write_text(json.dumps(self.manifest))
        with self.assertRaises(ValueError):
            b.snapshot(self.db, self.store)

    def test_symlink(self):
        self.vec.unlink()
        self.vec.symlink_to(self.db / 'manifest.json')
        with self.assertRaises(ValueError):
            b.snapshot(self.db, self.store)

    def test_duplicate_ids_and_wrong_next_id(self):
        self.manifest['segments'].append('seg-00002')
        for suffix in ('json', 'vec'):
            (self.db / ('segments/seg-00002.' + suffix)).write_bytes((self.db / ('segments/seg-00001.' + suffix)).read_bytes())
        (self.db / 'manifest.json').write_text(json.dumps(self.manifest))
        with self.assertRaises(ValueError):
            b.snapshot(self.db, self.store)
        self.manifest['segments'].pop()
        self.manifest['next_chunk_id'] = 1
        (self.db / 'manifest.json').write_text(json.dumps(self.manifest))
        with self.assertRaises(ValueError):
            b.snapshot(self.db, self.store)

    def test_hostile_restore_path(self):
        r = b.snapshot(self.db, self.store)
        data = json.loads(r.read_text())
        data['files']['../escape'] = next(iter(data['files'].values()))
        raw = json.dumps(data).encode()
        import hashlib
        hostile = r.parent / (hashlib.sha256(raw).hexdigest() + '.json')
        hostile.write_bytes(raw)
        with self.assertRaises(ValueError):
            b.restore(hostile, self.root / 'restore')
        self.assertFalse((self.root / 'escape').exists())

    def test_missing_and_lfs_pointer_objects(self):
        r = b.snapshot(self.db, self.store)
        obj = next((self.store / 'objects').iterdir())
        original = obj.read_bytes()
        obj.write_bytes(b'version https://git-lfs.github.com/spec/v1\n')
        with self.assertRaises(ValueError):
            b.restore(r, self.root / 'restore')
        obj.unlink()
        with self.assertRaises(FileNotFoundError):
            b.restore(r, self.root / 'restore')
        self.assertFalse((self.root / 'restore').exists())

    def test_source_changed(self):
        original = b.shutil.copyfile
        def mutate(src, dest):
            result = original(src, dest)
            if Path(src).name == 'manifest.json':
                (self.db / 'mind.jsonl').write_text('changed')
            return result
        from unittest.mock import patch
        with patch.object(b.shutil, 'copyfile', mutate):
            with self.assertRaises(ValueError):
                b.snapshot(self.db, self.store)
        self.assertFalse((self.store / 'snapshots').exists())

if __name__ == '__main__':
    unittest.main()
