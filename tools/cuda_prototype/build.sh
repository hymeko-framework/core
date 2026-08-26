#!/usr/bin/env sh
# Build the CUDA SpMM feasibility prototype.
#
# No absolute path is hardcoded. `nvcc` is resolved in this order:
#   1. $NVCC              — explicit override
#   2. $CUDA_HOME/bin/nvcc — standard CUDA layout
#   3. nvcc on $PATH
#
# The prototype is a Phase 6 feasibility artifact, not part of the Rust
# workspace: it adds no dependency and is not built by cargo.
set -eu

OUT=${OUT:-spmm_prototype}
ARCH=${ARCH:-native}          # override for cross-compilation, e.g. ARCH=sm_75

if [ -n "${NVCC:-}" ]; then
    CC="$NVCC"
elif [ -n "${CUDA_HOME:-}" ] && [ -x "$CUDA_HOME/bin/nvcc" ]; then
    CC="$CUDA_HOME/bin/nvcc"
elif command -v nvcc >/dev/null 2>&1; then
    CC=nvcc
else
    echo "nvcc not found. Set NVCC=/path/to/nvcc or CUDA_HOME=/path/to/cuda." >&2
    exit 1
fi

cd "$(dirname "$0")"
echo "nvcc: $CC"
"$CC" --version | tail -2
"$CC" -O3 -arch="$ARCH" -o "$OUT" spmm_prototype.cu
echo "built: $(pwd)/$OUT"
