#!/usr/bin/env bash
# Runs the ALSA backend's tests that need the kernel's sequencer in a VM
# booted with Ubuntu's generic kernel, for a machine whose own kernel has no
# sequencer: a GitHub runner's Azure kernel is built with no sound at all.
#
# The VM is virtme-ng's, over this machine's own files, so the test binary
# built here runs there as it is. It runs as root there, which loads the
# sequencer. Under `cargo llvm-cov show-env` the binary is instrumented, and
# its coverage is written back into the target directory, which the VM may
# write.
#
# Needs virtme-ng, qemu-system-x86 and a generic kernel in /boot that this
# user may read: linux-image-virtual-hwe-24.04 on Ubuntu 24.04. Uses KVM
# where this user may open /dev/kvm, and QEMU's far slower emulation where
# not.
#
#   scripts/run-sequencer-tests.sh
set -euo pipefail

kernel=$(ls /boot/vmlinuz-*-generic 2>/dev/null | sort -V | tail -n 1 || true)
if [[ -z $kernel || ! -r $kernel ]]; then
    echo "no generic kernel in /boot that this user may read" >&2
    exit 1
fi

binary=$(cargo test -p hapto-midi-devices --lib --no-run --locked --message-format=json |
    python3 -c '
import json, sys
for line in sys.stdin:
    message = json.loads(line)
    if (
        message.get("reason") == "compiler-artifact"
        and message["target"]["name"] == "hapto_midi_devices"
        and message.get("executable")
    ):
        print(message["executable"])
')
target=$(cargo metadata --format-version 1 --no-deps |
    python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')

emulate=()
if [[ ! -r /dev/kvm || ! -w /dev/kvm ]]; then
    echo "no KVM for this user: the VM is emulated, and slow" >&2
    emulate=(--disable-kvm --cpus 2)
fi

# virtme-ng hands the VM a command but not all of this environment, and not
# a command with quotes in it whole, so what the VM runs is a script of its
# own, written where it can read it.
inside="$target/sequencer-tests-in-vm.sh"
{
    echo 'set -e'
    echo 'modprobe snd-seq'
    if [[ -n ${LLVM_PROFILE_FILE:-} ]]; then
        printf 'export LLVM_PROFILE_FILE=%q\n' "$LLVM_PROFILE_FILE"
    fi
    printf 'exec %q alsa::tests\n' "$binary"
} > "$inside"

echo "running $binary in a VM on $kernel"
vng "${emulate[@]}" --run "$kernel" --rwdir "$target" -- bash "$inside"
