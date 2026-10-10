import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('release', Path(__file__).parents[1] / 'release.py')
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class StableIdentity(unittest.TestCase):
    def test_no_prerelease_or_shell_injection(self):
        for value in ['v1.0.0', '01.0.0', '1.0.0-rc1', '1.0.0+build', '1.0.0\n', '1;echo danger', 'latest']:
            with self.assertRaises(ValueError):
                release.version(value)

    def test_no_reuse_including_draft(self):
        with self.assertRaises(ValueError):
            release.newer('1.0.0', [{'tag_name': 'v1.0.0', 'draft': True}])

    def test_monotonic_across_all_releases(self):
        with self.assertRaises(ValueError):
            release.newer('1.9.9', [{'tag_name': 'v2.0.0'}])
        release.newer('2.0.1', [{'tag_name': 'v2.0.0'}, {'tag_name': 'v9.0.0-rc1', 'prerelease': True}])
        release.newer('1.0.0', [])


class PublicationRetries(unittest.TestCase):
    commit = 'a' * 40

    def current(self):
        return {'tag_name': 'v1.2.3', 'target_commitish': self.commit, 'draft': False, 'prerelease': False}

    def test_retry_current_release_is_allowed(self):
        release.promotable('1.2.3', self.commit, [self.current(), {'tag_name': 'v1.2.2'}])

    def test_old_failed_job_cannot_replace_newer_stable(self):
        with self.assertRaisesRegex(ValueError, 'newer stable release'):
            release.promotable('1.2.3', self.commit, [self.current(), {'tag_name': 'v1.2.4'}])

    def test_published_source_must_match_selected_commit(self):
        with self.assertRaisesRegex(ValueError, 'selected source commit'):
            release.promotable('1.2.3', 'b' * 40, [self.current()])

    def test_draft_or_prerelease_cannot_be_promoted(self):
        for key in ('draft', 'prerelease'):
            with self.assertRaises(ValueError):
                release.promotable('1.2.3', self.commit, [{**self.current(), key: True}])

    def test_unpublished_future_versions_do_not_block_current_retry(self):
        release.promotable('1.2.3', self.commit, [self.current(),
                          {'tag_name': 'v2.0.0', 'draft': True},
                          {'tag_name': 'v3.0.0', 'prerelease': True}])


class ImmutableImageTags(unittest.TestCase):
    def test_both_version_aliases_must_be_unused(self):
        seen = []
        release.unused_image_tags('1.2.3', lambda tag: seen.append(tag) or False)
        self.assertEqual(seen, ['v1.2.3', '1.2.3'])
        for existing in ['v1.2.3', '1.2.3']:
            with self.assertRaises(ValueError):
                release.unused_image_tags('1.2.3', lambda tag: tag == existing)

    def test_network_failure_cannot_allow_an_overwrite(self):
        def unavailable(tag):
            raise OSError('registry unavailable')
        with self.assertRaises(OSError):
            release.unused_image_tags('1.2.3', unavailable)


class PublicDistribution(unittest.TestCase):
    def test_repository_must_explicitly_be_public(self):
        release.require_public_repository({'private': False})
        for metadata in ({'private': True}, {}):
            with self.assertRaisesRegex(ValueError, 'public repository'):
                release.require_public_repository(metadata)
