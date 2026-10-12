# Installer benchmarks

The installer acceptance harness can compare ARM64 KVM hardware acceleration with the existing QEMU TCG software emulation. Both modes use the same 2-vCPU, 4096-MiB Ubuntu 24.04 guest, published Ember 1.0.0 artifacts, and all 14 candidate acceptance gates. This measures installation and lifecycle verification, including browser setup, chat, uploads, WebSockets, push enrollment, repeat installation, backup/restore, reboot, update, and injected recovery failures. It does not measure application throughput or change the release workflow.

Run it only on a fresh, disposable native ARM64 Linux host with root access. The harness changes networking, `/etc/hosts`, local trust stores, and Docker state. KVM mode fails unless `/dev/kvm` supports API version 12 and VM creation; QEMU's QMP response must also confirm that KVM is enabled before guest acceptance begins. There is no automatic TCG fallback.

## Optional Depot CI experiment

[`.depot/workflows/arm-installer-benchmark.yml`](../../.depot/workflows/arm-installer-benchmark.yml) is manual-only (`workflow_dispatch`), with one `depot-ubuntu-24.04-arm-4` sandbox and a 30-minute job timeout. It checks native ARM64/KVM and a working rootful Docker daemon before installing packages and Chromium. It anonymously downloads all four versioned release assets and checks their pinned SHA256 values, source commit, image digest, and bundle digest. It needs repository read access, outbound package/registry access, sudo, and a fresh sandbox; it does not use release credentials or publish anything.

Depot describes nested virtualization as available on every **Depot CI** sandbox. That provider claim has not yet been measured by this benchmark on ARM64. Depot's separate **GitHub Actions runners** do not expose `/dev/kvm`; changing an existing GitHub workflow to those runner labels would not provide this experiment. [Nested virtualization announcement](https://depot.dev/changelog/2026-05-20-nested-virtualization-on-depot-ci), [GitHub Actions runner limitation](https://depot.dev/docs/github-actions/troubleshooting).

Running this optional workflow requires a Depot account and the Depot Code Access GitHub app scoped to this repository. Review the account's plan and usage controls before starting a run. After that setup, the Depot CLI can submit this one workflow, including local changes, without first merging it:

```sh
depot ci run --workflow .depot/workflows/arm-installer-benchmark.yml
```

Do not migrate the production workflows or import their secrets for this experiment. No Depot account, app installation, or paid compute is provisioned by adding these files. [Depot CI quickstart](https://depot.dev/docs/ci/quickstart).

As checked on 2026-10-11, the 4-CPU/16-GB ARM64 sandbox costs $0.0002/second ($0.012/minute) in extra usage: 10 minutes is $0.12, 20 minutes $0.24, and 30 minutes $0.36 in usage-equivalent compute. The Developer plan is $20/month with 2,000 base-size minutes; this sandbox consumes those minutes at 2×, equivalent to 1,000 actual minutes. Depot advertises a seven-day trial without a credit card; existing-account eligibility must be checked. Included minutes are not a spending cap. Setup and downloads consume compute time too, and the job timeout is not an account billing cap or a guarantee about provisioning, cleanup, artifact, storage, or plan charges. [Current sandbox sizes and pricing](https://depot.dev/docs/ci/overview#pricing), [Plans and usage controls](https://depot.dev/pricing).

## Evidence and comparisons

The artifact upload allowlist contains only:

- `result.json`: the unchanged complete acceptance result; a candidate success requires all 14 unique gates.
- `benchmark.json`: the unchanged harness-generated accelerator/QEMU metadata, guest resources, stage timings, and hashes of the harness, release manifest, and result.
- `job-timing.json`: provider setup and dependency-install durations, time from the first workflow step through evidence collection, and the SHA256 of the original benchmark bytes. Missing timings or a missing benchmark are represented by `null` after an earlier failure.

`total_job_seconds` excludes queueing, sandbox provisioning, and the later artifact upload; use provider billing records for invoiced duration. The harness's `elapsed_seconds` excludes workflow dependency setup. Compare stage durations as well as totals, retain both result and benchmark hashes, and require `success: true`, `gate_count: 14`, `accelerator: "kvm"`, and `kvm_enabled: true` before calling a KVM run successful. A timeout or missing result is a failed measurement, not a partial pass. Raw guest logs, SSH keys, test credentials, and private setup links are never uploaded by this workflow.

## Local M5 reproduction

The local setup uses an Apple M5 Max running macOS 27.0.1, with Lima 2.2.1 using Virtualization.framework (`vmType: vz`, `nestedVirtualization: true`). The disposable outer VM is Ubuntu 26.04 ARM64 with 4 vCPUs, 16 GiB memory, kernel `7.0.0-34-generic`, QEMU 10.2.1, Docker 29.1.3, Node 24.21.0, and Playwright 1.63.0. Its [Ubuntu 26.04 release-20260927 image](https://cloud-images.ubuntu.com/releases/resolute/release-20260927/ubuntu-26.04-server-cloudimg-arm64.img) SHA256 is `63a93bd5a8d76e33b15ceb5daa3657bd79be804748051ab178e643b0f5da22e7`. Both fresh cases are cloned from the same prepared base; runner provisioning is excluded from harness timings.

The inner guest stays at 2 vCPUs and 4096 MiB using the [pinned Ubuntu 24.04 ARM64 image](guests.lock.json), SHA256 `1d6bffe64b848468ac97f821d369a4846d983de1800ccf6b5ec8853e85cefc55`, and the same published Ember 1.0.0 assets. The harness runs inside Linux, not directly on macOS. Local Mac timings do not establish Depot performance.

The Ubuntu 26.04 host firmware, `qemu-efi-aarch64` version `2025.11-3ubuntu7.3`, had SHA256 `e264b5d66d05ad3abad9ec0b63dcf0c956007a3af9345dd03f79c5246ffcb392`. The TCG comparison stalled at its UEFI banner before Linux started. An [upstream EDK2 report](https://github.com/tianocore/edk2/issues/11962) documents the same early-banner symptom with 2025.11 firmware, QEMU 10.2, and `-cpu max`; its [LPA2 page-table fix](https://github.com/tianocore/edk2/pull/12545) is absent from this Ubuntu package. This is a matching upstream failure, not proof of the local stall's cause.

For the paired reruns, pin the same official Ubuntu 24.04 firmware on the unused prepared outer VM **before cloning either case**. The following extracts the [pinned `2024.02-2ubuntu0.10` package](https://packages.ubuntu.com/noble-updates/all/qemu-efi-aarch64/download) without installing or downgrading a host package, checks both package and firmware hashes, and replaces only the firmware file used by the harness. Run this only in the disposable benchmark base; the harness and stable release defaults remain unchanged.

```sh
set -eu
firmware_dir=$(mktemp -d)
trap 'rm -rf "$firmware_dir"' EXIT
curl -fsSL --output "$firmware_dir/firmware.deb" \
  https://archive.ubuntu.com/ubuntu/pool/main/e/edk2/qemu-efi-aarch64_2024.02-2ubuntu0.10_all.deb
printf '%s  %s\n' \
  fad8e80e929cd00dae8c5c8e5dd3ec85f37d9a832eea776957e5e2cf86ef2ab5 \
  "$firmware_dir/firmware.deb" | sha256sum --check --status
dpkg-deb --extract "$firmware_dir/firmware.deb" "$firmware_dir/extracted"
firmware_source="$firmware_dir/extracted/usr/share/qemu-efi-aarch64/QEMU_EFI.fd"
printf '%s  %s\n' \
  d2d1228a664adbaed2148c2661e30c930af9bf09f03b6d5aa2a8bbc9cb5cab43 \
  "$firmware_source" | sha256sum --check --status
sudo install -o root -g root -m 0644 "$firmware_source" \
  /usr/share/qemu-efi-aarch64/QEMU_EFI.fd
printf '%s  %s\n' \
  d2d1228a664adbaed2148c2661e30c930af9bf09f03b6d5aa2a8bbc9cb5cab43 \
  /usr/share/qemu-efi-aarch64/QEMU_EFI.fd | sha256sum --check --status
```

On this host, the inner guest boot required `kvm-arm.vgic_v3_group1_trap=1` on the **outer Linux kernel command line**. Configure the outer VM's bootloader, reboot it, then clone both KVM and TCG cases from that base. This documented KVM option traps guest accesses to GICv3 group-1 system registers; Linux reports reduced performance when it is enabled. [Linux 7 parameter definition](https://github.com/torvalds/linux/blob/028ef9c96e96197026887c0f092424679298aae8/Documentation/admin-guide/kernel-parameters.txt#L3257-L3259), [activation message](https://github.com/torvalds/linux/blob/028ef9c96e96197026887c0f092424679298aae8/arch/arm64/kvm/vgic/vgic-v3.c#L884-L888).

Verify activation inside each outer VM:

```sh
grep -oF 'kvm-arm.vgic_v3_group1_trap=1' /proc/cmdline
sudo dmesg | grep -E 'GICv3 sysreg trapping enabled.*\[G1\].*reduced performance'
```

The local setup has passed `/dev/kvm` API 12/VM creation and QMP `kvm_enabled: true` checks. These identify the accelerator; complete acceptance evidence is recorded separately below. No hosted compute account or charges were created. After installing the same tools and verifying the same four release assets as the workflow, run inside the disposable Linux VM:

```sh
sudo --preserve-env=PATH,PLAYWRIGHT_BROWSERS_PATH \
  env GUEST_OS=ubuntu-24.04 GUEST_ARCH=arm64 PUBLIC_INSTALLER=false \
  python3 deploy/verification/guests.py --assets release-assets \
  --benchmark-disposable-runner --accel kvm
```

Use `--accel tcg` only for the separate software-emulation comparison. Each invocation requires a clean guest workspace and disposable host; all acceptance checks remain enabled in both modes.


### Local measurements (2026-10-11)

Both fresh cases passed all 14 candidate gates, with QMP confirming KVM enabled for the KVM case and disabled for TCG. TCG took **3.80 times the KVM elapsed time** in this single paired trial (1578.8 / 415.5). The cases ran sequentially on fresh clones of the same prepared outer VM, with matching G1 trapping boot option, pinned firmware, public artifacts, tools, and inner guest resources.

| Measurement | KVM | TCG |
| --- | --- | --- |
| Complete candidate gates | 14/14 passed | 14/14 passed |
| Harness elapsed time | 415.5 s (6 min 55.5 s) | 1578.8 s (26 min 18.8 s) |
| First-install phase | 39.0 s | 275.4 s |
| Reboot SSH readiness phase | 16.2 s | 135.9 s |
| Update with failure injection phase | 189.6 s | 235.5 s |

The measured harness is from [commit `c8677af`](https://github.com/NYLLON-SOFTWARE/ember/commit/c8677afb4161ed4ce75a7c3477f0cc1e100463b1), SHA256 `e87af393659ffb3cf3ec77f4bce971af6cda81c591de82aa218563876f519ea2`, before the later QMP robustness changes. The separate `benchmark.json` file SHA256 values are:

- KVM: `4fd14b0f6d32cb98bbdc32ba7ac9e46cb2b5d9e0396cea5f8c643b6c7e0e7b14`.
- TCG: `0235f5174268fba4f74dce58f4ff3ed857c22b14a5f06835f97df9509ea33353`.

Both complete result files have SHA256 `cce113202ff868c8da1660a60c770d178dbacf028766911d79adf91215980274`: their platform and gate lists are identical, while the benchmark files contain the distinct timings. Raw benchmark, result, and case-provenance JSON remain outside version control.

This experiment used one run per mode; package and image downloads can vary between sequential runs. TCG uses `-cpu max` with the default `virt` machine, while KVM uses `-cpu host` and `virt,gic-version=host`. It compares the practical harness modes, not an isolated accelerator change. The update phase includes an intentional approximately 180-second failed-start health deadline; it is not a normal update-time measurement. Timings include guest boot and package/download work; outer runner provisioning and dependency setup were not timed. Local cloud compute charges were $0. These figures do not measure managed-provider performance or application throughput.
