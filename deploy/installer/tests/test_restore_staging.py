"""Complete restores accept ordinary hardlinks and clean unjournaled failed attempts."""
import os
from pathlib import Path
import tarfile
import unittest
from unittest import mock

import test_installer as fixtures

m = fixtures.m


class RestoreStaging(unittest.TestCase):
    setUp = fixtures.Lifecycle.setUp

    def staging_paths(self):
        return sorted(self.manager.config.parent.glob('.ember-restore-*')) + sorted(self.manager.data.glob('.storage-restore-*'))

    def test_hardlinked_storage_is_archived_as_complete_regular_files(self):
        original = self.manager.storage / 'files' / 'upload.bin'
        linked = self.manager.storage / 'files' / 'upload-copy.bin'
        os.link(original, linked)
        self.assertEqual(original.stat().st_ino, linked.stat().st_ino)
        expected = original.read_bytes()
        identifier = self.manager.backup(self.state)
        with tarfile.open(self.manager.backups / identifier / 'snapshot.tar.gz', 'r:gz') as archive:
            for name in ('storage/files/upload.bin', 'storage/files/upload-copy.bin'):
                member = archive.getmember(name)
                self.assertTrue(member.isfile())
                self.assertFalse(member.islnk() or member.issym())
                self.assertEqual(archive.extractfile(member).read(), expected)
        original.write_bytes(b'changed through shared inode')
        self.assertEqual(linked.read_bytes(), b'changed through shared inode')
        self.manager.restore(self.state, identifier, True)
        for name in ('upload.bin', 'upload-copy.bin'):
            self.assertEqual((self.manager.storage / 'files' / name).read_bytes(), expected)
        self.assertFalse(self.manager.journal_file.exists())

    def repeated_extraction_failure(self, operation):
        identifier = self.manager.backup(self.state)
        details = {'phase': 'candidate-touched-data', 'backup': identifier}
        if operation == 'restore':
            config = self.manager.config.parent / '.ember-restore-older'
            storage = self.manager.data / '.storage-restore-older'
            config.mkdir(mode=0o700)
            storage.mkdir(mode=0o700)
            (config / 'partial-secret').write_bytes(b'old private partial extraction')
            details = {'phase': 'staged', 'backup': identifier, 'staged_config': str(config), 'staged_storage': str(storage)}
        self.manager.journal(operation, self.state, **details)
        original_journal = self.manager.journal_file.read_bytes()
        before_env = (self.manager.config / 'app.env').read_bytes()
        before_upload = (self.manager.storage / 'files' / 'upload.bin').read_bytes()
        self.docker.events.clear()
        original_copy = m.shutil.copyfileobj
        def partial_copy(source, destination):
            destination.write(source.read(4))
            raise OSError('simulated extraction disk failure')
        for _attempt in range(3):
            with mock.patch.object(m.shutil, 'copyfileobj', side_effect=partial_copy):
                with self.assertRaisesRegex(OSError, 'extraction disk failure'):
                    self.manager.restore(self.state, identifier, True)
            self.assertEqual(self.manager.journal_file.read_bytes(), original_journal)
            self.assertEqual(self.staging_paths(), [])
            self.assertEqual((self.manager.config / 'app.env').read_bytes(), before_env)
            self.assertEqual((self.manager.storage / 'files' / 'upload.bin').read_bytes(), before_upload)
            self.assertTrue(self.docker.containers['ember']['State']['Running'])
        self.assertFalse(any(event[1] in {'stop', 'rm', 'create', 'start'} for event in self.docker.events))
        self.assertIs(m.shutil.copyfileobj, original_copy)

    def test_repeated_extraction_failure_cleans_new_stages_with_older_update_journal(self):
        self.repeated_extraction_failure('update')

    def test_repeated_extraction_failure_cleans_new_stages_with_older_restore_journal(self):
        self.repeated_extraction_failure('restore')

    def test_second_stage_mkdir_failure_cleans_first_stage_and_preserves_journal(self):
        identifier = self.manager.backup(self.state)
        self.manager.journal('update', self.state, phase='candidate-touched-data', backup=identifier)
        original_journal = self.manager.journal_file.read_bytes()
        original_mkdir = Path.mkdir
        def failing_mkdir(path, *args, **kwargs):
            if path.name.startswith('.storage-restore-'):
                raise OSError('cannot create restore storage stage')
            return original_mkdir(path, *args, **kwargs)
        with mock.patch.object(Path, 'mkdir', failing_mkdir):
            with self.assertRaisesRegex(OSError, 'cannot create restore storage stage'):
                self.manager.restore(self.state, identifier, True)
        self.assertEqual(self.staging_paths(), [])
        self.assertEqual(self.manager.journal_file.read_bytes(), original_journal)
        self.assertTrue(self.docker.containers['ember']['State']['Running'])

    def test_failure_after_new_restore_journal_preserves_its_stages(self):
        identifier = self.manager.backup(self.state)
        with mock.patch.object(self.manager, 'stop', side_effect=OSError('stop failed')):
            with self.assertRaisesRegex(OSError, 'stop failed'):
                self.manager.restore(self.state, identifier, True)
        journal = m.json_file(self.manager.journal_file)
        self.assertEqual(journal['operation'], 'restore')
        self.assertEqual(journal['phase'], 'staged')
        self.assertEqual(set(self.staging_paths()), {Path(journal['staged_config']), Path(journal['staged_storage'])})
        self.assertEqual((Path(journal['staged_storage']) / 'files' / 'upload.bin').read_bytes(), b'original upload\x00')


if __name__ == '__main__':
    unittest.main()
