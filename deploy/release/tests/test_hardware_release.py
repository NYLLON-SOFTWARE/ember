"""Execute release workflow guards and shell branches without provisioning runners."""
import contextlib
import io
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import textwrap
import unittest
from unittest import mock

ROOT = Path(__file__).parents[3]
PUBLISH = ROOT / '.github/workflows/publish-image.yml'
GUESTS = ROOT / '.github/workflows/installer-guests.yml'


def job(source, name):
    jobs = source.split('\njobs:\n', 1)[1]
    return re.search(r'^  ' + re.escape(name) + r':\n(.*?)(?=^  \S|\Z)', jobs, re.M | re.S).group(1)


def step(source, name):
    return re.search(r'^      - name: ' + re.escape(name) + r'\n(.*?)(?=^      - |\Z)', source, re.M | re.S).group(1)


def shell(source):
    return textwrap.dedent(source.split('        run: |\n', 1)[1])


def stub(path):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text('#!' + sys.executable + '\nimport json,os,sys\n'
                    'with open(os.environ["TOOL_LOG"], "a") as log:\n'
                    '    log.write(json.dumps([os.path.basename(sys.argv[0]), *sys.argv[1:]]) + "\\n")\n')
    path.chmod(0o755)


class HardwareRelease(unittest.TestCase):
    def setUp(self):
        self.publish = PUBLISH.read_text()
        self.guests = GUESTS.read_text()
        self.installation = job(self.guests, 'installation')

    def validate(self, **overrides):
        source = shell(step(job(self.guests, 'configuration'), 'Validate hardware release request'))
        python = source.split("python3 - <<'PY'\n", 1)[1].rsplit('\nPY', 1)[0]
        environment = {'HARDWARE_ACCELERATED': 'true', 'ARM_RUNNER_GROUP': 'ember-release-arm',
            'GITHUB_EVENT_NAME': 'workflow_dispatch', 'GITHUB_REPOSITORY': 'NYLLON-SOFTWARE/ember',
            'GITHUB_REF': 'refs/heads/main'}
        environment.update(overrides)
        with mock.patch.dict(os.environ, environment, clear=True), contextlib.redirect_stdout(io.StringIO()):
            exec(compile(python, str(GUESTS), 'exec'), {})

    def test_actual_hosted_guard_accepts_only_trusted_hardware_dispatch_before_runner_scheduling(self):
        self.validate()
        invalid = ({'GITHUB_EVENT_NAME': event} for event in ('pull_request', 'pull_request_target', 'push'))
        for overrides in (*invalid, {'GITHUB_REPOSITORY': 'fork/ember'}, {'GITHUB_REF': 'refs/pull/1/merge'},
                          {'GITHUB_REF': 'refs/tags/v1.1.0'}, {'ARM_RUNNER_GROUP': ''},
                          {'ARM_RUNNER_GROUP': 'private\nrunner'}, {'ARM_RUNNER_GROUP': '{"labels":"private"}'},
                          {'HARDWARE_ACCELERATED': 'auto'}):
            with self.subTest(overrides=overrides), self.assertRaises(SystemExit) as raised:
                self.validate(**overrides)
            self.assertNotIn('private', str(raised.exception))
        self.validate(HARDWARE_ACCELERATED='false', GITHUB_EVENT_NAME='pull_request',
                      GITHUB_REPOSITORY='fork/ember', GITHUB_REF='refs/pull/1/merge')
        configuration = job(self.guests, 'configuration')
        self.assertIn('    runs-on: ubuntu-24.04\n', configuration)
        self.assertIn('    permissions: {}\n', configuration)
        self.assertIn('    needs: configuration\n', self.installation)
        self.assertNotRegex(self.installation, r'(?m)^    if:.*always\(')

    def test_exact_four_platform_matrix_keeps_default_hosted_and_hardware_arm_group_routing(self):
        self.assertIn('    runs-on: ${{ inputs.hardware_accelerated && matrix.hardware_runner || matrix.runner }}\n', self.installation)
        matrix = self.installation.split('        include:\n', 1)[1].split('    steps:\n', 1)[0]
        entries = re.split(r'^          - os: ', matrix, flags=re.M)[1:]
        platforms = []
        for entry in entries:
            os_name = entry.splitlines()[0]
            arch = re.search(r'^            arch: (\S+)$', entry, re.M).group(1)
            platforms.append((os_name, arch))
            if arch == 'arm64':
                self.assertIn('            runner: ubuntu-24.04-arm\n', entry)
                self.assertIn('            hardware_runner:\n              group: ${{ inputs.arm_runner_group }}\n', entry)
                self.assertIn('              labels: [self-hosted, Linux, ARM64, ember-release-kvm]\n', entry)
            else:
                self.assertIn('            runner: ubuntu-24.04\n', entry)
                self.assertIn('            hardware_runner: ubuntu-24.04\n', entry)
                self.assertNotIn('self-hosted', entry)
        self.assertEqual(set(platforms), {('ubuntu-24.04', 'amd64'), ('debian-13', 'amd64'),
                                         ('ubuntu-24.04', 'arm64'), ('debian-13', 'arm64')})
        self.assertEqual(len(platforms), 4)

    def test_candidate_and_public_share_exact_commit_artifact_and_hardware_settings(self):
        for name, public in (('guests', 'false'), ('public-verification', 'true')):
            with self.subTest(job=name):
                call = job(self.publish, name)
                self.assertIn('    uses: ./.github/workflows/installer-guests.yml\n', call)
                self.assertIn('      commit: ${{ needs.identity.outputs.commit }}\n', call)
                self.assertIn('      artifact: stable-candidate\n', call)
                self.assertIn('      public: ' + public + '\n', call)
                self.assertIn('      hardware_accelerated: ${{ inputs.hardware_accelerated || false }}\n', call)
                self.assertIn("      arm_runner_group: ${{ inputs.arm_runner_group || 'ember-release-arm' }}\n", call)
                self.assertNotIn('secrets:', call)
                self.assertNotIn('write', call)
        for source in (self.publish, self.guests):
            self.assertRegex(source, r'      hardware_accelerated:\n(?:        [^\n]+\n)*        default: false\n')
            self.assertRegex(source, r'      arm_runner_group:\n(?:        [^\n]+\n)*        default: ember-release-arm\n')
        self.assertIn('needs: [identity, index, guests]', job(self.publish, 'promote'))
        self.assertIn('needs: [identity, index, promote, public-verification]', job(self.publish, 'website-ready'))

    def run_shell(self, script, environment):
        with tempfile.TemporaryDirectory() as temporary:
            work = Path(temporary)
            for name in ('sudo', 'npm'):
                stub(work / 'bin' / name)
            stub(work / 'parity/node_modules/.bin/playwright')
            log = work / 'tools.jsonl'
            env = {**os.environ, 'PATH': str(work / 'bin') + os.pathsep + os.environ['PATH'],
                   'TOOL_LOG': str(log), **environment}
            subprocess.run(['bash', '-eu', '-c', script], cwd=work, env=env,
                           capture_output=True, text=True, check=True, timeout=5)
            return [json.loads(line) for line in log.read_text().splitlines()]

    def test_actual_invocation_keeps_tcg_default_and_requires_kvm_metrics_in_hardware_mode(self):
        invocation = shell(step(self.installation, 'Run fresh guest with real test-CA ACME validation'))
        for hardware in ('false', 'true'):
            with self.subTest(hardware=hardware):
                commands = self.run_shell(invocation, {'HARDWARE_ACCELERATED': hardware})
                self.assertEqual(len(commands), 1)
                prefix = ['sudo', '--preserve-env=PATH,GITHUB_ACTIONS,GUEST_OS,GUEST_ARCH,PUBLIC_INSTALLER,PLAYWRIGHT_BROWSERS_PATH',
                          'python3', 'deploy/verification/guests.py', '--assets', 'release-assets']
                self.assertEqual(commands[0], prefix + (['--accel', 'kvm', '--benchmark-disposable-runner'] if hardware == 'true' else []))
        preflight = step(self.installation, 'Require native KVM and a fresh disposable host')
        self.assertIn('        if: inputs.hardware_accelerated\n', preflight)
        self.assertIn('guests.require_kvm(os.environ[\'GUEST_ARCH\'])', preflight)
        self.assertIn("assert not guests.WORK.exists()", preflight)
        self.assertLess(self.installation.index('Require native KVM'), self.installation.index('Install guest tools and browser'))

    def test_prepared_arm_preserves_guest_tools_and_rechecks_firmware_after_browser_dependencies(self):
        dependencies = shell(step(self.installation, 'Install guest tools and browser'))
        for prepared in ('true', 'false'):
            with self.subTest(prepared=prepared):
                commands = self.run_shell(dependencies, {'PREPARED_ARM': prepared})
                apt = [command for command in commands if command[:2] == ['sudo', 'apt-get']]
                self.assertEqual(len(apt), 0 if prepared == 'true' else 2)
                self.assertEqual(commands[-2:], [['npm', 'ci', '--ignore-scripts', '--prefix', 'parity'],
                                                ['playwright', 'install', '--with-deps', 'chromium']])
        pin = step(self.installation, 'Reapply and verify the tested ARM firmware after dependencies')
        self.assertIn("        if: inputs.hardware_accelerated && matrix.arch == 'arm64'\n", pin)
        self.assertIn('        run: sudo sh deploy/verification/pin-arm-firmware.sh\n', pin)
        self.assertLess(self.installation.index('Install guest tools and browser'), self.installation.index('Reapply and verify'))
        self.assertLess(self.installation.index('Reapply and verify'), self.installation.index('Run fresh guest'))

    def test_hardware_evidence_and_read_only_guest_boundary_are_preserved(self):
        self.assertIn('ref: ${{ inputs.commit }}', self.installation)
        self.assertIn('name: ${{ inputs.artifact }}', self.installation)
        self.assertIn('persist-credentials: false', self.installation)
        self.assertIn('    permissions:\n      contents: read\n', self.installation)
        for forbidden in ('secrets:', 'environment:', 'id-token:', 'packages:', 'contents: write', 'GH_TOKEN:', 'CLOUDFLARE_'):
            self.assertNotIn(forbidden, self.installation)
        self.assertIn('application.log result.json benchmark.json; do', self.installation)
        self.assertIn('name: guest-${{ matrix.os }}-${{ matrix.arch }}-public-${{ inputs.public }}', self.installation)
        for name in ('identity', 'index', 'bundle', 'promote', 'deploy', 'rollback-installer', 'website-ready'):
            with self.subTest(job=name):
                self.assertIn('    runs-on: ubuntu-24.04\n', job(self.publish, name))


if __name__ == '__main__':
    unittest.main()
