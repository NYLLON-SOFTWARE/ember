"""Kill the real manager process at downtime boundaries, then recover its durable intent."""
import json
import os
import signal
import unittest
from unittest import mock

import test_installer as fixtures

m = fixtures.m


class AvailabilityRecovery(unittest.TestCase):
    setUp = fixtures.Lifecycle.setUp

    def interrupted(self, operation):
        marker = self.manager.data / 'interrupted-container.json'
        pid = os.fork()
        if pid == 0:
            def kill():
                marker.write_text(json.dumps(self.docker.containers))
                os.kill(os.getpid(), signal.SIGKILL)
            try:
                if operation == 'backup':
                    with mock.patch.object(self.manager, 'archive', side_effect=lambda state: kill()):
                        self.manager.backup(self.state)
                else:
                    def stopped(args, **kwargs):
                        result = self.docker(args, **kwargs)
                        if args[1] == 'stop':
                            kill()
                        return result
                    with mock.patch.object(m, 'run', side_effect=stopped):
                        self.manager.restart(self.state)
            except BaseException:
                os._exit(2)
            os._exit(3)
        _, status = os.waitpid(pid, 0)
        self.assertTrue(os.WIFSIGNALED(status))
        self.assertEqual(os.WTERMSIG(status), signal.SIGKILL)
        self.docker.containers = json.loads(marker.read_text())
        marker.unlink()
        self.assertFalse(self.docker.containers['ember']['State']['Running'])
        journal = m.json_file(self.manager.journal_file)
        self.assertEqual(journal['operation'], operation)
        self.assertTrue(journal['was_running'])
        self.assertEqual(journal['state'], self.state)

    def test_killed_backup_can_restore_original_running_state(self):
        self.interrupted('backup')
        self.manager.restart(self.state)
        self.assertTrue(self.docker.containers['ember']['State']['Running'])
        self.assertFalse(self.manager.journal_file.exists())
        self.assertEqual((self.manager.storage / 'files' / 'upload.bin').read_bytes(), b'original upload\x00')

    def test_killed_restart_can_restore_original_running_state(self):
        self.interrupted('restart')
        self.manager.restart(self.state)
        self.assertTrue(self.docker.containers['ember']['State']['Running'])
        self.assertFalse(self.manager.journal_file.exists())

    def test_backup_and_restart_record_intent_before_docker_stop(self):
        def recorded(args, **kwargs):
            if args[1] == 'stop':
                journal = m.json_file(self.manager.journal_file)
                self.assertTrue(journal['was_running'])
                self.assertEqual(journal['state'], self.state)
                self.assertIn(journal['operation'], {'backup', 'restart'})
            return self.docker(args, **kwargs)
        with mock.patch.object(m, 'run', side_effect=recorded):
            self.manager.backup(self.state)
            self.manager.restart(self.state)
        self.assertFalse(self.manager.journal_file.exists())

    def test_backup_preserves_an_already_stopped_installation(self):
        self.docker.containers['ember']['State']['Running'] = False
        self.manager.backup(self.state)
        self.assertFalse(self.docker.containers['ember']['State']['Running'])
        self.assertFalse(self.manager.journal_file.exists())
        self.assertFalse(any(event[1] == 'start' for event in self.docker.events))

    def test_failed_resume_retains_journal_for_retry(self):
        self.docker.fail_start = True
        with self.assertRaisesRegex(m.Failure, 'start failed'):
            self.manager.backup(self.state)
        self.assertEqual(m.json_file(self.manager.journal_file)['operation'], 'backup')
        self.docker.fail_start = False
        self.manager.restart(self.state)
        self.assertTrue(self.docker.containers['ember']['State']['Running'])
        self.assertFalse(self.manager.journal_file.exists())

    def test_retry_restarts_a_running_process_that_failed_readiness(self):
        launches = 0
        def realistic_start(args, **kwargs):
            nonlocal launches
            if args[1] == 'start' and not self.docker.containers['ember']['State']['Running']:
                launches += 1
            return self.docker(args, **kwargs)
        def healthy_after_relaunch(state):
            if launches < 2:
                raise m.Failure('running process requires a restart')
        with mock.patch.object(m, 'run', side_effect=realistic_start), mock.patch.object(self.manager, 'readiness', side_effect=healthy_after_relaunch):
            with self.assertRaisesRegex(m.Failure, 'requires a restart'):
                self.manager.restart(self.state)
            self.assertTrue(self.docker.containers['ember']['State']['Running'])
            self.manager.restart(self.state)
        self.assertEqual(launches, 2)
        self.assertFalse(self.manager.journal_file.exists())

    def test_readiness_failure_does_not_clear_restart_recovery(self):
        with mock.patch.object(self.manager, 'readiness', side_effect=m.Failure('not ready')):
            with self.assertRaisesRegex(m.Failure, 'not ready'):
                self.manager.restart(self.state)
        self.assertEqual(m.json_file(self.manager.journal_file)['operation'], 'restart')
        self.manager.restart(self.state)
        self.assertFalse(self.manager.journal_file.exists())


if __name__ == '__main__':
    unittest.main()
