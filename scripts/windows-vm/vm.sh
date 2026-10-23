#!/usr/bin/env bash
# A local Windows 11 VM (dockur/windows under podman, KVM-accelerated) for
# building the Windows add-on wheels and checking the Windows app code.
#
#   scripts/windows-vm/vm.sh start    create (first run) or start the VM
#   scripts/windows-vm/vm.sh stop     shut Windows down cleanly
#   scripts/windows-vm/vm.sh ssh [command...]
#   scripts/windows-vm/vm.sh status   container state and first-boot setup log
#
# The first start downloads Windows 11 from Microsoft and installs it
# unattended; oem/setup.ps1 then installs the build tools (log:
# C:\OEM\setup.log). Watch progress at http://127.0.0.1:8006. Exchange files
# through $VM_DIR/shared, which Windows sees as \\host.lan\Data (scp/sftp to
# the VM doesn't work with this OpenSSH build). Unpack source copies with
# `tar -m`: the VM clock can run ahead, and cargo treats files whose
# timestamps predate its last build as unchanged. All ports are
# bound to localhost only. The VM has no GPU: it can compile CUDA code but not
# run it. Windows runs unactivated with a generic install key; licensing is
# the user's responsibility.
set -euo pipefail

VM_DIR="${VM_DIR:-$HOME/arctic-winvm}"
NAME="${NAME:-arctic-winvm}"
IMAGE="${IMAGE:-docker.io/dockurr/windows:6.06}"
RAM_SIZE="${RAM_SIZE:-32G}"
CPU_CORES="${CPU_CORES:-16}"
DISK_SIZE="${DISK_SIZE:-200G}"
SSH_PORT="${SSH_PORT:-2222}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

start() {
  if podman container exists "$NAME"; then
    podman start "$NAME" >/dev/null
    echo "Started $NAME."
    return
  fi
  mkdir -p "$VM_DIR"/{storage,oem,shared,ssh}
  if [[ ! -f "$VM_DIR/ssh/id_ed25519" ]]; then
    ssh-keygen -q -t ed25519 -N "" -C "$NAME" -f "$VM_DIR/ssh/id_ed25519"
  fi
  if [[ ! -f "$VM_DIR/password" ]]; then
    (umask 077 && head -c 24 /dev/urandom | base64 | tr -d '/+=' >"$VM_DIR/password")
  fi
  cp "$SCRIPT_DIR"/oem/* "$VM_DIR/oem/"
  cp "$VM_DIR/ssh/id_ed25519.pub" "$VM_DIR/oem/authorized_key.pub"
  # Rootless podman uses user-mode networking, which forwards only the ports
  # listed in USER_PORTS to Windows.
  podman run -d --name "$NAME" \
    -e VERSION=11 -e RAM_SIZE="$RAM_SIZE" -e CPU_CORES="$CPU_CORES" -e DISK_SIZE="$DISK_SIZE" \
    -e USERNAME=arctic -e PASSWORD="$(cat "$VM_DIR/password")" -e USER_PORTS=22 \
    -p 127.0.0.1:8006:8006 -p 127.0.0.1:3389:3389/tcp -p 127.0.0.1:3389:3389/udp \
    -p "127.0.0.1:$SSH_PORT:22" \
    --device /dev/kvm --device /dev/net/tun --cap-add NET_ADMIN \
    -v "$VM_DIR/storage:/storage" -v "$VM_DIR/oem:/oem" -v "$VM_DIR/shared:/shared" \
    --stop-timeout 120 "$IMAGE" >/dev/null
  echo "Created $NAME. Installing Windows; watch http://127.0.0.1:8006"
  echo "RDP: 127.0.0.1:3389, user arctic, password in $VM_DIR/password"
}

vm_ssh() {
  ssh -i "$VM_DIR/ssh/id_ed25519" -p "$SSH_PORT" \
    -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile="$VM_DIR/ssh/known_hosts" \
    -o ConnectTimeout=10 arctic@127.0.0.1 "$@"
}

case "${1:-}" in
  start) start ;;
  stop) podman stop -t 120 "$NAME" ;;
  ssh) shift && vm_ssh "$@" ;;
  status)
    podman ps -a --filter "name=^$NAME\$" --format '{{.Names}} {{.Status}}'
    vm_ssh 'Get-Content C:\OEM\setup.log -Tail 15' 2>/dev/null || echo "(SSH not reachable yet)"
    ;;
  *) echo "usage: $0 start|stop|ssh [command...]|status" >&2; exit 2 ;;
esac
