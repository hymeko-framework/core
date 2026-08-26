// CSR x dense SpMM — CUDA C prototype (Phase 6 feasibility, NOT a backend).
//
// ---------------------------------------------------------------------------
// What this is, and what it is deliberately not
// ---------------------------------------------------------------------------
// A standalone CUDA C program that answers one question before any Rust binding
// or dependency is proposed: does a CUDA kernel with the same semantics as
// `shaders/spmm.wgsl` reproduce the CPU oracle, and what does it cost?
//
// It adds NO Rust dependency, NO vendored crate, and touches no library code.
// CLAUDE.md §1 makes adding a dependency a core-level decision requiring written
// justification and explicit approval; this prototype exists so that
// justification can cite measurements instead of expectations.
//
// ---------------------------------------------------------------------------
// Why the workload generator is duplicated here
// ---------------------------------------------------------------------------
// `hymeko_hnn::workloads::{Lcg, SparseSpec}` is the deterministic generator
// every existing benchmark uses. Reimplementing it in C is duplication, and it
// is accepted for exactly one reason: a number measured on a *different* matrix
// is not comparable to the published wgpu figures, and the alternative — a Rust
// binding to dump the matrix — is the very dependency this prototype exists to
// avoid pre-judging.
//
// The port is verified rather than trusted: `--check-nnz` prints the generated
// nnz, which must equal the values the Rust generator produces for the same
// spec. Those are known from the published benchmark report:
//
//     rows=200000 cols=200000 deg=8  uniform   -> nnz 1599965
//     SparseSpec::large()                      -> nnz 5442303
//
// If the port drifts, those numbers disagree and the prototype is invalid. That
// is the discriminating check, not a hope.
//
// ---------------------------------------------------------------------------
// Kernel semantics — matched to spmm.wgsl deliberately
// ---------------------------------------------------------------------------
//   * one thread per (row, feature) pair;
//   * threadIdx.x indexes the FEATURE axis, so adjacent lanes read adjacent
//     f32s of X and write adjacent f32s of Y — the same coalescing argument as
//     the WGSL kernel, where mapping rows to x would stride Y by feature_dim;
//   * the inner loop walks k in increasing order and accumulates into one
//     scalar, matching `TensorCsr::row_axpy`, so a duplicate column index is
//     summed as two terms exactly as the CPU does;
//   * every thread owns its output element exclusively — no atomics, the same
//     disjoint-output-partition property the CPU and WGSL backends rely on.
//
// Bit-exactness with the CPU is NOT promised: the GPU may contract
// `acc + v * x` into an FMA, rounding once instead of twice. That is a per-term
// rounding difference, not a reordering, and it is why the comparison below uses
// a tolerance rather than an equality.
//
// Build (no absolute paths; nvcc is taken from PATH or $CUDA_HOME):
//   ./build.sh
// or explicitly:
//   CUDA_HOME=/opt/cuda ./build.sh
//
// Run:
//   ./spmm_prototype --check-nnz
//   ./spmm_prototype --rows 200000 --deg 8 --fd 32 --verify
//   ./spmm_prototype --rows 200000 --deg 8 --fd 32 --bench

#include <cuda_runtime.h>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cstdint>
#include <vector>
#include <algorithm>
#include <cmath>

// ---------------------------------------------------------------------------
// Faithful port of hymeko_hnn::workloads::Lcg
// ---------------------------------------------------------------------------

struct Lcg {
    uint64_t s;
    explicit Lcg(uint64_t seed) : s(seed) {}

    uint64_t next_u64() {
        // Wrapping arithmetic: uint64_t overflow is defined as modular in C++.
        s = s * 6364136223846793005ULL + 1442695040888963407ULL;
        return s;
    }

    // Uses the high bits; the low bits of an LCG are weak. Matches `below`.
    size_t below(size_t bound) {
        size_t b = bound < 1 ? 1 : bound;
        return (size_t)(next_u64() >> 11) % b;
    }

    float unit_f32() {
        const float SCALE = 1.0f / (float)(1u << 24);
        return (float)(uint32_t)(next_u64() >> 40) * SCALE;
    }

    // [0.5, 1.5) — bounded away from zero, under one octave, so f32
    // accumulation over ~1e6 terms stays well conditioned.
    float weight_f32() { return 0.5f + unit_f32(); }
};

enum DegreeShape { UNIFORM, IRREGULAR };

// Matches DegreeShape::sample. Always at least 1.
static size_t sample_degree(Lcg& rng, DegreeShape shape, size_t avg) {
    size_t a = avg < 1 ? 1 : avg;
    if (shape == UNIFORM) return a;
    size_t r = rng.below(100);
    size_t d;
    if (r <= 4)        d = a * 8;
    else if (r <= 19)  d = a * 2;
    else if (r <= 79)  d = a;
    else               d = 1 + a / 4;
    return d < 1 ? 1 : d;
}

struct Csr {
    size_t rows, cols;
    std::vector<uint32_t> row_ptr;   // u32: matches the shader's index width
    std::vector<uint32_t> col_ind;
    std::vector<float>    val;
    size_t nnz() const { return col_ind.size(); }
};

// Port of SparseSpec::build. RNG consumption order is load-bearing: degrees are
// drawn first, then columns, then — after sort+dedup — one weight per SURVIVING
// column. Drawing weights before dedup would desynchronise the stream and
// silently produce a different matrix.
static Csr build_csr(size_t rows, size_t cols, size_t avg_deg, DegreeShape shape, uint64_t seed) {
    Lcg rng(seed);
    Csr m;
    m.rows = rows;
    m.cols = cols;
    m.row_ptr.reserve(rows + 1);
    m.col_ind.reserve(rows * avg_deg);
    m.val.reserve(rows * avg_deg);
    std::vector<size_t> scratch;

    m.row_ptr.push_back(0);
    for (size_t r = 0; r < rows; ++r) {
        size_t degree = sample_degree(rng, shape, avg_deg);
        if (degree > cols) degree = cols;

        scratch.clear();
        for (size_t d = 0; d < degree; ++d) scratch.push_back(rng.below(cols));
        std::sort(scratch.begin(), scratch.end());
        scratch.erase(std::unique(scratch.begin(), scratch.end()), scratch.end());

        for (size_t i = 0; i < scratch.size(); ++i) {
            m.col_ind.push_back((uint32_t)scratch[i]);
            m.val.push_back(rng.weight_f32());
        }
        m.row_ptr.push_back((uint32_t)m.col_ind.size());
    }
    return m;
}

// Port of SparseSpec::dense_block — note the distinct seed derivation.
static std::vector<float> dense_block(size_t cols, size_t fd, uint64_t seed) {
    Lcg rng(seed ^ 0xB10CB10CULL);
    std::vector<float> x(cols * fd);
    for (size_t i = 0; i < x.size(); ++i) x[i] = rng.weight_f32();
    return x;
}

// ---------------------------------------------------------------------------
// CPU oracle — port of TensorCsr::row_axpy / spmm_into_serial
// ---------------------------------------------------------------------------

static void spmm_cpu(const Csr& m, const float* x, size_t fd, float* y) {
    for (size_t i = 0; i < m.rows; ++i) {
        float* yrow = y + i * fd;
        for (size_t f = 0; f < fd; ++f) yrow[f] = 0.0f;
        uint32_t start = m.row_ptr[i], end = m.row_ptr[i + 1];
        for (uint32_t k = start; k < end; ++k) {
            uint32_t col = m.col_ind[k];
            float    v   = m.val[k];
            const float* xrow = x + (size_t)col * fd;
            for (size_t f = 0; f < fd; ++f) yrow[f] += v * xrow[f];
        }
    }
}

// ---------------------------------------------------------------------------
// Kernel — one thread per (row, feature); x indexes features (see header)
// ---------------------------------------------------------------------------

__global__ void spmm_kernel(
    const uint32_t* __restrict__ row_ptr,
    const uint32_t* __restrict__ col_ind,
    const float*    __restrict__ values,
    const float*    __restrict__ x,
    float*          __restrict__ y,
    uint32_t num_rows,
    uint32_t feature_dim)
{
    uint32_t feature = blockIdx.x * blockDim.x + threadIdx.x;
    uint32_t row     = blockIdx.y * blockDim.y + threadIdx.y;
    if (feature >= feature_dim || row >= num_rows) return;

    uint32_t start = row_ptr[row];
    uint32_t end   = row_ptr[row + 1];

    float acc = 0.0f;
    for (uint32_t k = start; k < end; ++k) {
        acc += values[k] * x[(size_t)col_ind[k] * feature_dim + feature];
    }
    y[(size_t)row * feature_dim + feature] = acc;
}

#define CHECK(call) do { \
    cudaError_t e_ = (call); \
    if (e_ != cudaSuccess) { \
        fprintf(stderr, "CUDA error %s at %s:%d\n", cudaGetErrorString(e_), __FILE__, __LINE__); \
        exit(2); \
    } \
} while (0)

int main(int argc, char** argv) {
    size_t rows = 200000, cols = 0, deg = 8, fd = 32;
    DegreeShape shape = UNIFORM;
    uint64_t seed = 0x005EEDC0FFEEULL;   // matches crossover_sweep's SEED
    bool verify = false, bench = false, check_nnz = false;
    int bx = 16, by = 8;                 // matches spmm_workgroup_for's 16x8

    for (int i = 1; i < argc; ++i) {
        if (!strcmp(argv[i], "--rows") && i + 1 < argc)      rows = strtoull(argv[++i], 0, 10);
        else if (!strcmp(argv[i], "--cols") && i + 1 < argc) cols = strtoull(argv[++i], 0, 10);
        else if (!strcmp(argv[i], "--deg") && i + 1 < argc)  deg  = strtoull(argv[++i], 0, 10);
        else if (!strcmp(argv[i], "--fd") && i + 1 < argc)   fd   = strtoull(argv[++i], 0, 10);
        else if (!strcmp(argv[i], "--bx") && i + 1 < argc)   bx   = atoi(argv[++i]);
        else if (!strcmp(argv[i], "--by") && i + 1 < argc)   by   = atoi(argv[++i]);
        else if (!strcmp(argv[i], "--irregular"))            shape = IRREGULAR;
        else if (!strcmp(argv[i], "--verify"))               verify = true;
        else if (!strcmp(argv[i], "--bench"))                bench = true;
        else if (!strcmp(argv[i], "--check-nnz"))            check_nnz = true;
    }
    if (cols == 0) cols = rows;

    if (check_nnz) {
        // The port-fidelity check. These must match the Rust generator.
        struct { size_t r, c, d; DegreeShape s; const char* name; } cases[] = {
            {200000, 200000, 8,  UNIFORM,   "uniform deg8 200k  (expect nnz 1599965)"},
            {200000, 200000, 32, UNIFORM,   "uniform deg32 200k"},
            {50000,  50000,  8,  UNIFORM,   "uniform deg8 50k   (expect nnz 399970)"},
        };
        for (auto& c : cases) {
            Csr m = build_csr(c.r, c.c, c.d, c.s, seed);
            printf("  %-42s nnz = %zu\n", c.name, m.nnz());
        }
        return 0;
    }

    fprintf(stderr, "building workload: rows=%zu cols=%zu deg=%zu fd=%zu %s\n",
            rows, cols, deg, fd, shape == UNIFORM ? "uniform" : "irregular");
    Csr m = build_csr(rows, cols, deg, shape, seed);
    std::vector<float> x = dense_block(cols, fd, seed);
    fprintf(stderr, "nnz = %zu\n", m.nnz());

    uint32_t *d_rp, *d_ci; float *d_v, *d_x, *d_y;
    size_t y_elems = rows * fd;
    CHECK(cudaMalloc(&d_rp, m.row_ptr.size() * sizeof(uint32_t)));
    CHECK(cudaMalloc(&d_ci, m.col_ind.size() * sizeof(uint32_t)));
    CHECK(cudaMalloc(&d_v,  m.val.size()     * sizeof(float)));
    CHECK(cudaMalloc(&d_x,  x.size()         * sizeof(float)));
    CHECK(cudaMalloc(&d_y,  y_elems          * sizeof(float)));
    CHECK(cudaMemcpy(d_rp, m.row_ptr.data(), m.row_ptr.size()*sizeof(uint32_t), cudaMemcpyHostToDevice));
    CHECK(cudaMemcpy(d_ci, m.col_ind.data(), m.col_ind.size()*sizeof(uint32_t), cudaMemcpyHostToDevice));
    CHECK(cudaMemcpy(d_v,  m.val.data(),     m.val.size()*sizeof(float),        cudaMemcpyHostToDevice));
    CHECK(cudaMemcpy(d_x,  x.data(),         x.size()*sizeof(float),            cudaMemcpyHostToDevice));

    dim3 block(bx, by);
    dim3 grid((uint32_t)((fd + bx - 1) / bx), (uint32_t)((rows + by - 1) / by));

    if (verify) {
        spmm_kernel<<<grid, block>>>(d_rp, d_ci, d_v, d_x, d_y, (uint32_t)rows, (uint32_t)fd);
        CHECK(cudaGetLastError());
        CHECK(cudaDeviceSynchronize());
        std::vector<float> gpu(y_elems);
        CHECK(cudaMemcpy(gpu.data(), d_y, y_elems*sizeof(float), cudaMemcpyDeviceToHost));

        std::vector<float> cpu(y_elems);
        spmm_cpu(m, x.data(), fd, cpu.data());

        double worst_rel = 0.0; size_t worst_i = 0; size_t bad = 0;
        for (size_t i = 0; i < y_elems; ++i) {
            double a = gpu[i], b = cpu[i];
            double abs = fabs(a - b);
            double rel = fabs(b) > 0.0 ? abs / fabs(b) : abs;
            if (rel > worst_rel) { worst_rel = rel; worst_i = i; }
            if (!(abs <= 1e-4 || rel <= 1e-5)) ++bad;
        }
        printf("verify: elements=%zu  worst_rel=%.3e at %zu (gpu=%.6f cpu=%.6f)  failures=%zu\n",
               y_elems, worst_rel, worst_i, gpu[worst_i], cpu[worst_i], bad);
        printf("verdict: %s\n", bad == 0 ? "PASS (within FMA-contraction tolerance)" : "FAIL");
        if (bad) return 1;
    }

    if (bench) {
        cudaEvent_t t0, t1;
        CHECK(cudaEventCreate(&t0)); CHECK(cudaEventCreate(&t1));
        for (int i = 0; i < 5; ++i)   // warm-up, discarded
            spmm_kernel<<<grid, block>>>(d_rp, d_ci, d_v, d_x, d_y, (uint32_t)rows, (uint32_t)fd);
        CHECK(cudaDeviceSynchronize());

        const int REPS = 50;
        std::vector<float> ms(REPS);
        for (int i = 0; i < REPS; ++i) {
            CHECK(cudaEventRecord(t0));
            spmm_kernel<<<grid, block>>>(d_rp, d_ci, d_v, d_x, d_y, (uint32_t)rows, (uint32_t)fd);
            CHECK(cudaEventRecord(t1));
            CHECK(cudaEventSynchronize(t1));
            CHECK(cudaEventElapsedTime(&ms[i], t0, t1));
        }
        std::sort(ms.begin(), ms.end());
        double med = ms[REPS/2], q1 = ms[REPS/4], q3 = ms[(3*REPS)/4];
        double macs = (double)m.nnz() * (double)fd;
        printf("kernel_only rows=%zu deg=%zu fd=%zu block=%dx%d: median=%.1f us (q1=%.1f q3=%.1f) %.1f GMAC/s\n",
               rows, deg, fd, bx, by, med*1000.0, q1*1000.0, q3*1000.0, macs / (med/1000.0) / 1e9);

        // Readback measured separately: the wgpu report shows device-to-host
        // dominating at 88-95%, and lumping it in would hide that.
        std::vector<float> host(y_elems);
        CHECK(cudaEventRecord(t0));
        for (int i = 0; i < 10; ++i)
            CHECK(cudaMemcpy(host.data(), d_y, y_elems*sizeof(float), cudaMemcpyDeviceToHost));
        CHECK(cudaEventRecord(t1));
        CHECK(cudaEventSynchronize(t1));
        float rb; CHECK(cudaEventElapsedTime(&rb, t0, t1));
        double rb_us = rb * 1000.0 / 10.0;
        printf("readback   %zu MB: %.1f us  (%.2f GB/s)\n",
               y_elems*sizeof(float)/(1024*1024), rb_us,
               (double)(y_elems*sizeof(float)) / (rb_us*1e-6) / 1e9);
    }

    cudaFree(d_rp); cudaFree(d_ci); cudaFree(d_v); cudaFree(d_x); cudaFree(d_y);
    return 0;
}
