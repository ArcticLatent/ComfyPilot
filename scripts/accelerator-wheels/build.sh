#!/usr/bin/env bash
# Builds the add-on wheels (SageAttention 2, SageAttention 3, FlashAttention,
# Nunchaku) for one torch/CUDA/Python combination inside a CUDA container, so
# the result doesn't depend on the host's compiler or glibc.
#
#   scripts/accelerator-wheels/build.sh [sage2|sage3|flash|nunchaku|all]
#
# Sources are cloned to $WORK_DIR/src and wheels written to
# $WORK_DIR/out/<cuda>-torch<version>-py<python>/. InsightFace is not built
# here: it doesn't link against torch, so one wheel per Python version serves
# every torch build.
set -euo pipefail

TORCH_VERSION="${TORCH_VERSION:-2.14.1}"
CUDA_TAG="${CUDA_TAG:-cu130}"
PYTHON_VERSION="${PYTHON_VERSION:-3.12}"
CUDA_IMAGE="${CUDA_IMAGE:-docker.io/nvidia/cuda:13.0.3-devel-ubuntu22.04}"
WORK_DIR="${WORK_DIR:-$HOME/arctic-wheels}"
# Parallel compile jobs x nvcc threads per job = concurrent cicc processes,
# each 2-4 GB. The container gets a hard memory cap, so a build that
# overshoots is OOM-killed inside it instead of swapping the desktop to a halt.
MAX_JOBS="${MAX_JOBS:-4}"
NVCC_THREADS="${NVCC_THREADS:-2}"
BUILD_MEMORY="${BUILD_MEMORY:-48g}"

SAGE_REF="${SAGE_REF:-d1a57a546c3d395b1ffcbeecc66d81db76f3b4b5}"
FLASH_REF="${FLASH_REF:-v2.8.3.post2}"
NUNCHAKU_REF="${NUNCHAKU_REF:-302e0e97024ebd68688fe890e5df83731edf7b54}"
# SageAttention 3's setup.py clones CUTLASS's default branch; pin it instead.
CUTLASS_REF="${CUTLASS_REF:-91843b71d90737e0a9b86f7d9df213cff5a3526f}"

# GPU targets. SageAttention 2: Ampere, Ada, Hopper, Blackwell consumer.
# FlashAttention 2 runs sm80 code on sm86/sm89. SageAttention 3 is
# Blackwell-only. Nunchaku's ALL mode builds sm75 through sm121a.
SAGE2_ARCHS="${SAGE2_ARCHS:-8.0;8.6;8.9;9.0;12.0}"
SAGE3_CC="${SAGE3_CC:-12.0}"
FLASH_ARCHS="${FLASH_ARCHS:-80;90;120}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TORCH_TAG="torch$(echo "$TORCH_VERSION" | tr -d .)"
PY_TAG="py$(echo "$PYTHON_VERSION" | tr -d .)"
OUT_NAME="${CUDA_TAG}-${TORCH_TAG}-${PY_TAG}"
IMAGE="arctic-accelerator-build:${OUT_NAME}"
SRC="$WORK_DIR/src"
OUT="$WORK_DIR/out/$OUT_NAME"
mkdir -p "$SRC" "$OUT" "$WORK_DIR/logs"

clone() { # path-under-src url ref [--recurse-submodules]
  local dir="$SRC/$1"
  if [[ ! -d "$dir/.git" ]]; then
    git clone --quiet "${@:4}" "$2" "$dir"
  fi
  if ! git -C "$dir" rev-parse --quiet --verify "$3^{commit}" >/dev/null; then
    git -C "$dir" fetch --quiet --tags origin "$3"
  fi
  git -C "$dir" checkout --quiet --force "$3"
  if [[ "${4:-}" == "--recurse-submodules" ]]; then
    git -C "$dir" submodule update --quiet --init --recursive
  fi
}

ensure_image() {
  if ! podman image exists "$IMAGE"; then
    echo "Building image $IMAGE (torch $TORCH_VERSION, $CUDA_TAG, Python $PYTHON_VERSION)..."
    podman build \
      --build-arg CUDA_IMAGE="$CUDA_IMAGE" \
      --build-arg PYTHON_VERSION="$PYTHON_VERSION" \
      --build-arg TORCH_VERSION="$TORCH_VERSION" \
      --build-arg TORCH_INDEX="https://download.pytorch.org/whl/$CUDA_TAG" \
      -t "$IMAGE" -f "$SCRIPT_DIR/Containerfile" "$SCRIPT_DIR"
  fi
}

# Builds run a patched copy of setup.py (setup_container.py); see
# patch_setup.py for what changes and why.
patch_setup() { # package dir [extra args...]
  python3 "$SCRIPT_DIR/patch_setup.py" "$1" "$SRC/$2/setup.py" "$SRC/$2/setup_container.py" \
    --threads "$NVCC_THREADS" "${@:3}"
}

in_container() { # workdir command...
  local workdir="$1"
  shift
  podman run --rm \
    --memory="$BUILD_MEMORY" --memory-swap="$BUILD_MEMORY" \
    -v "$SRC:/src" -v "$OUT:/out" \
    -e MAX_JOBS="$MAX_JOBS" -e NVCC_THREADS="$NVCC_THREADS" -e EXT_PARALLEL=1 \
    -w "$workdir" "$IMAGE" bash -euo pipefail -c "$*"
}

build_sage2() {
  clone SageAttention https://github.com/thu-ml/SageAttention.git "$SAGE_REF"
  patch_setup sage2 SageAttention
  in_container /src/SageAttention "
    rm -rf build dist
    TORCH_CUDA_ARCH_LIST='$SAGE2_ARCHS' python setup_container.py bdist_wheel -d /out
    rm -f setup_container.py"
}

build_sage3() {
  clone SageAttention https://github.com/thu-ml/SageAttention.git "$SAGE_REF"
  clone SageAttention/sageattention3_blackwell/csrc/cutlass https://github.com/NVIDIA/cutlass.git "$CUTLASS_REF"
  patch_setup sage3 SageAttention/sageattention3_blackwell --cc "$SAGE3_CC"
  in_container /src/SageAttention/sageattention3_blackwell "
    rm -rf build dist
    python setup_container.py bdist_wheel -d /out
    rm -f setup_container.py"
}

build_flash() {
  clone flash-attention https://github.com/Dao-AILab/flash-attention.git "$FLASH_REF" --recurse-submodules
  patch_setup flash flash-attention
  in_container /src/flash-attention "
    rm -rf build dist
    FLASH_ATTENTION_FORCE_BUILD=TRUE FLASH_ATTN_CUDA_ARCHS='$FLASH_ARCHS' \\
      python setup_container.py bdist_wheel -d /out
    rm -f setup_container.py"
}

build_nunchaku() {
  clone nunchaku https://github.com/nunchaku-tech/nunchaku.git "$NUNCHAKU_REF" --recurse-submodules
  patch_setup nunchaku nunchaku
  # NUNCHAKU_BUILD_WHEELS drops line info, as upstream's release wheels do
  # (about a third of the size).
  in_container /src/nunchaku "
    rm -rf build dist
    NUNCHAKU_INSTALL_MODE=ALL NUNCHAKU_BUILD_WHEELS=1 python setup_container.py bdist_wheel -d /out
    rm -f setup_container.py"
}

target="${1:-all}"
ensure_image
case "$target" in
  sage2) build_sage2 ;;
  sage3) build_sage3 ;;
  flash) build_flash ;;
  nunchaku) build_nunchaku ;;
  all)
    for step in sage2 sage3 nunchaku flash; do
      echo "=== $step ($(date +%T))"
      "build_$step" 2>&1 | tee "$WORK_DIR/logs/$OUT_NAME-$step.log"
    done
    ;;
  *) echo "usage: $0 [sage2|sage3|flash|nunchaku|all]" >&2; exit 2 ;;
esac
echo "Wheels in $OUT:"
ls -la "$OUT"
