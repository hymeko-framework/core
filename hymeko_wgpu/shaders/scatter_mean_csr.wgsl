// Edge -> node mean scatter, as a NODE-INDEXED GATHER over an inverse-incidence CSR.
//
// One invocation per (node, channel). The invocation walks that node's CSR row -- the incidences
// naming it -- and accumulates in FIXED CSR ORDER. Every output element is therefore written by
// exactly one invocation, so this kernel contains no atomic and no read-modify-write of shared
// state, and its accumulation order is identical from run to run.
//
// This is the device form of the CPU owner's traversal inversion
// (`hymeko_nagare::ops::scatter::CycleIncidence`, "no atomics, no locks"). The rejected alternative
// -- one invocation per incidence with `atomicAdd` into the node row -- is simpler and forbidden
// here: float atomic accumulation order varies per run, which would make bit-stable parity against
// the CPU owner impossible and force the parity gate down to a loose tolerance.
//
// The denominator is NOT recomputed on device. `counts` carries the CPU owner's own per-node
// incidence count, so the degree-zero case resolves exactly as it does on the host (zero, not
// 0/0 -> NaN).

struct Params {
    n_nodes:  u32,   // rows of the output
    n_edges:  u32,   // rows of the input
    hidden:   u32,   // channels, shared by input and output
    nnz:      u32,   // total incidences; row_offsets[n_nodes] must equal this
    count:    u32,   // n_nodes * hidden
    stride:   u32,   // grid stride
};

// Inverse incidence in CSR: for node v, the incidences at row_offsets[v] .. row_offsets[v+1].
@group(0) @binding(0) var<storage, read>       row_offsets: array<u32>;   // n_nodes + 1
@group(0) @binding(1) var<storage, read>       edge_index:  array<u32>;   // nnz, values < n_edges
@group(0) @binding(2) var<storage, read>       counts:      array<f32>;   // n_nodes, from the host
@group(0) @binding(3) var<storage, read>       edge_state:  array<f32>;   // n_edges * hidden
@group(0) @binding(4) var<storage, read_write> node_state:  array<f32>;   // n_nodes * hidden
@group(0) @binding(5) var<uniform>             params:      Params;

@compute @workgroup_size({{WORKGROUP_X}}, {{WORKGROUP_Y}})
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    for (var out_index = gid.x; out_index < params.count; out_index += params.stride) {
        let node    = out_index / params.hidden;
        let channel = out_index % params.hidden;

        let first = row_offsets[node];
        let last  = row_offsets[node + 1u];

        // Fixed CSR order. No atomic: this invocation owns `out_index` alone.
        var acc = 0.0;
        for (var p = first; p < last; p += 1u) {
            let edge = edge_index[p];
            acc += edge_state[edge * params.hidden + channel];
        }

        // OPERATION ORDER, taken from the CPU owner and not re-derived: it accumulates the sum,
        // then multiplies by the RECIPROCAL `inv = 1.0 / here.len()`. A division here would be a
        // different float operation and would break bit-stable parity, so the reciprocal is
        // reproduced deliberately. An empty row leaves the accumulator at 0.0 and skips the
        // multiply, exactly as the CPU owner's `if !here.is_empty()` does.
        let denom = counts[node];
        var value = acc;
        if (denom > 0.0) {
            let inv = 1.0 / denom;
            value = acc * inv;
        }
        node_state[out_index] = value;
    }
}
