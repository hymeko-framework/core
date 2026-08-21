use iceoryx2::prelude::*;
use std::mem::size_of;

pub const MAX_NNZ: usize = 1_048_576; // 1M non-zero entries baseline

#[repr(C)]
#[derive(Debug, ZeroCopySend)]
pub struct HypergraphWeights {
    pub version: u64,
    pub nnz: usize,
    pub values: [f32; MAX_NNZ],
}

#[repr(u32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ExpansionKind {
    Star3D = 1,
    Clique2D = 2,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ExpansionHeader {
    pub version: u32,
    pub kind: ExpansionKind,
    pub nnz: u64,
    pub etag: [u8; 32],
    pub dim_k: u64,
    pub dim_i: u64,
    pub dim_j: u64,
}

impl ExpansionHeader {
    pub const VERSION: u32 = 1;

    pub fn new(kind: ExpansionKind, nnz: usize, dim_k: usize, dim_i: usize, dim_j: usize) -> Self {
        Self {
            version: Self::VERSION,
            kind,
            nnz: nnz as u64,
            etag: [0u8; 32],
            dim_k: dim_k as u64,
            dim_i: dim_i as u64,
            dim_j: dim_j as u64,
        }
    }

    /// Calculates byte offsets for the contiguous (k, i, j, val) buffers that follow the header.
    pub fn contiguous_offsets(&self) -> ExpansionOffsets {
        let header_bytes = size_of::<ExpansionHeader>();
        let i64_bytes = size_of::<i64>();
        let f32_bytes = size_of::<f32>();
        let nnz = self.nnz as usize;

        let k_offset = header_bytes;
        let i_offset = k_offset + nnz * i64_bytes;
        let j_offset = i_offset + nnz * i64_bytes;
        let values_offset = j_offset + nnz * i64_bytes;

        ExpansionOffsets {
            header_bytes,
            k_offset,
            i_offset,
            j_offset,
            values_offset,
            total_bytes: values_offset + nnz * f32_bytes,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ExpansionOffsets {
    pub header_bytes: usize,
    pub k_offset: usize,
    pub i_offset: usize,
    pub j_offset: usize,
    pub values_offset: usize,
    pub total_bytes: usize,
}

/// A helper function to calculate the exact byte size required for a given NNZ.
/// Memory Layout: [ExpansionHeader] + [k: i64 * nnz] + [i: i64 * nnz] + [j: i64 * nnz] + [val: f32 * nnz]
pub fn calculate_required_bytes(nnz: usize) -> usize {
    let header_size = std::mem::size_of::<ExpansionHeader>();
    let k_size = std::mem::size_of::<i64>() * nnz;
    let i_size = std::mem::size_of::<i64>() * nnz;
    let j_size = std::mem::size_of::<i64>() * nnz;
    let val_size = std::mem::size_of::<f32>() * nnz;

    header_size + k_size + i_size + j_size + val_size
}
