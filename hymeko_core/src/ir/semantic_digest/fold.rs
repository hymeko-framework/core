//! A virtual view of the canonical aggregate bytes and its retained BLAKE3 tree.
//! This changes evaluation/storage only; offsets, tree shape and root flags are
//! those of the ordinary BLAKE3 hash of the same byte stream.

use super::{
    DOMAIN_ARC, DOMAIN_DECL, DOMAIN_EDGE, DOMAIN_MODEL, DOMAIN_NODE, DigestWriter, HashId, MAGIC,
    RecordHashes, SEMANTIC_DIGEST_ALGO_VERSION, SEMANTIC_DIGEST_SCHEMA_VERSION,
    SemanticModelDigest, TargetScheme,
};
use blake3::hazmat::{
    ChainingValue, HasherExt, Mode, left_subtree_len, merge_subtrees_non_root, merge_subtrees_root,
};
use std::{
    ops::Range,
    sync::{Arc, OnceLock},
};

// Hash several BLAKE3 chunks together, retaining SIMD opportunities. This is
// cache granularity, not a new hash format or a measured optimum.
const BLOCK_BYTES: usize = 16 * blake3::CHUNK_LEN;
const HEADER_BYTES: usize = 8 + 8 + 2 + 2 + 1;
const DECLARATIONS_START: usize = HEADER_BYTES + 8 + 8;

trait HashInput {
    fn len(&self) -> usize;
    /// Fill exactly this in-bounds range without reading unrelated payload.
    fn read(&self, range: Range<usize>, output: &mut [u8]);
}

/// One encoder for both complete hashing and random-access aggregate blocks.
pub(super) struct ModelHashInput<'a> {
    scheme: TargetScheme,
    declarations: &'a [HashId],
    records: &'a RecordHashes,
}

impl<'a> ModelHashInput<'a> {
    pub(super) fn new(
        scheme: TargetScheme,
        declarations: &'a [HashId],
        records: &'a RecordHashes,
    ) -> Self {
        Self {
            scheme,
            declarations,
            records,
        }
    }

    fn write_range(&self, range: Range<usize>, write: impl FnMut(&[u8])) {
        assert!(range.start <= range.end && range.end <= self.len());
        let mut output = RangeWriter {
            range,
            cursor: 0,
            write,
        };
        output.field(MAGIC);
        output.field(DOMAIN_MODEL);
        output.field(&SEMANTIC_DIGEST_SCHEMA_VERSION.to_le_bytes());
        output.field(&SEMANTIC_DIGEST_ALGO_VERSION.to_le_bytes());
        output.field(&[self.scheme.tag()]);
        for (domain, hashes) in [
            (DOMAIN_DECL, self.declarations),
            (DOMAIN_NODE, self.records.nodes.as_slice()),
            (DOMAIN_EDGE, self.records.edges.as_slice()),
            (DOMAIN_ARC, self.records.arcs.as_slice()),
        ] {
            output.field(domain);
            output.field(&(hashes.len() as u64).to_le_bytes());
            output.hashes(hashes);
        }
        debug_assert_eq!(output.cursor, self.len());
    }

    pub(super) fn digest(&self) -> SemanticModelDigest {
        let mut writer = DigestWriter::new();
        self.write_range(0..self.len(), |bytes| {
            writer.update(bytes);
        });
        SemanticModelDigest(HashId(*writer.finalize().as_bytes()))
    }
}

impl HashInput for ModelHashInput<'_> {
    fn len(&self) -> usize {
        HEADER_BYTES
            + 4 * 16
            + 32 * (self.declarations.len()
                + self.records.nodes.len()
                + self.records.edges.len()
                + self.records.arcs.len())
    }

    fn read(&self, range: Range<usize>, output: &mut [u8]) {
        assert_eq!(range.len(), output.len());
        let mut written = 0;
        self.write_range(range, |bytes| {
            output[written..written + bytes.len()].copy_from_slice(bytes);
            written += bytes.len();
        });
        debug_assert_eq!(written, output.len());
    }
}

struct RangeWriter<F> {
    range: Range<usize>,
    cursor: usize,
    write: F,
}

impl<F: FnMut(&[u8])> RangeWriter<F> {
    fn field(&mut self, bytes: &[u8]) {
        let start = self
            .range
            .start
            .saturating_sub(self.cursor)
            .min(bytes.len());
        let end = self.range.end.saturating_sub(self.cursor).min(bytes.len());
        if start < end {
            (self.write)(&bytes[start..end]);
        }
        self.cursor += bytes.len();
    }

    fn hashes(&mut self, hashes: &[HashId]) {
        let end = self.cursor + 32 * hashes.len();
        if self.range.start <= self.cursor && end <= self.range.end {
            // The ordinary full encoder keeps its contiguous streaming loop.
            for hash in hashes {
                (self.write)(&hash.0);
            }
        } else {
            let first = (self.range.start.saturating_sub(self.cursor) / 32).min(hashes.len());
            let last = self
                .range
                .end
                .saturating_sub(self.cursor)
                .div_ceil(32)
                .min(hashes.len());
            self.cursor += first * 32;
            for hash in &hashes[first..last] {
                self.field(&hash.0);
            }
        }
        self.cursor = end;
    }
}

/// Immutable pending aggregate. A seed contains CVs only, never an old model or
/// a chain of pending aggregates. Once materialized, readers share the result.
pub(super) struct SemanticFold {
    scheme: TargetScheme,
    declarations: Arc<Vec<HashId>>,
    records: Arc<RecordHashes>,
    seed: Option<Arc<FoldTree>>,
    dirty: Range<usize>,
    materialized: OnceLock<MaterializedFold>,
}

struct MaterializedFold {
    tree: Arc<FoldTree>,
    digest: SemanticModelDigest,
}

impl SemanticFold {
    pub(super) fn new(
        scheme: TargetScheme,
        declarations: Arc<Vec<HashId>>,
        records: Arc<RecordHashes>,
    ) -> Self {
        let len = ModelHashInput::new(scheme, &declarations, &records).len();
        Self {
            scheme,
            declarations,
            records,
            seed: None,
            dirty: 0..len,
            materialized: OnceLock::new(),
        }
    }

    /// Same-length sorted declarations; `changed` covers every changed rank.
    pub(super) fn with_declarations(
        &self,
        declarations: Arc<Vec<HashId>>,
        changed: Range<usize>,
    ) -> Self {
        assert_eq!(declarations.len(), self.declarations.len());
        assert!(changed.start < changed.end && changed.end <= declarations.len());
        let mut dirty =
            DECLARATIONS_START + 32 * changed.start..DECLARATIONS_START + 32 * changed.end;
        let seed = if let Some(ready) = self.materialized.get() {
            Some(Arc::clone(&ready.tree))
        } else {
            // Accumulate invalidations relative to the last materialized tree.
            // Keeping only its tree avoids an unbounded chain of old snapshots.
            dirty.start = dirty.start.min(self.dirty.start);
            dirty.end = dirty.end.max(self.dirty.end);
            self.seed.clone()
        };
        Self {
            scheme: self.scheme,
            declarations,
            records: Arc::clone(&self.records),
            seed,
            dirty,
            materialized: OnceLock::new(),
        }
    }

    pub(super) fn digest(&self) -> SemanticModelDigest {
        self.materialized
            .get_or_init(|| {
                let input = ModelHashInput::new(self.scheme, &self.declarations, &self.records);
                let tree = Arc::new(FoldTree::build(&input, self.seed.as_deref(), &self.dirty));
                let digest = tree.digest();
                MaterializedFold { tree, digest }
            })
            .digest
    }

    #[cfg(test)]
    pub(super) fn is_materialized(&self) -> bool {
        self.materialized.get().is_some()
    }
}

enum FoldTree {
    Direct(SemanticModelDigest),
    Branch {
        left: Arc<HashNode>,
        right: Arc<HashNode>,
    },
}

impl FoldTree {
    fn build(input: &impl HashInput, seed: Option<&Self>, dirty: &Range<usize>) -> Self {
        let len = input.len();
        if len <= BLOCK_BYTES {
            let mut bytes = [0; BLOCK_BYTES];
            input.read(0..len, &mut bytes[..len]);
            return Self::Direct(SemanticModelDigest(HashId(
                *blake3::hash(&bytes[..len]).as_bytes(),
            )));
        }
        let split = left_subtree_len(len as u64) as usize;
        let (left, right) = match seed {
            Some(Self::Branch { left, right }) => (
                HashNode::update(left, input, 0..split, dirty),
                HashNode::update(right, input, split..len, dirty),
            ),
            _ => (
                HashNode::build(input, 0..split),
                HashNode::build(input, split..len),
            ),
        };
        Self::Branch { left, right }
    }

    fn digest(&self) -> SemanticModelDigest {
        match self {
            Self::Direct(digest) => *digest,
            Self::Branch { left, right } => SemanticModelDigest(HashId(
                *merge_subtrees_root(&left.cv, &right.cv, Mode::Hash).as_bytes(),
            )),
        }
    }
}

struct HashNode {
    cv: ChainingValue,
    children: Option<(Arc<HashNode>, Arc<HashNode>)>,
}

impl HashNode {
    fn build(input: &impl HashInput, span: Range<usize>) -> Arc<Self> {
        if span.len() <= BLOCK_BYTES {
            return Arc::new(Self::leaf(input, span));
        }
        let split = span.start + left_subtree_len(span.len() as u64) as usize;
        Self::parent(
            Self::build(input, span.start..split),
            Self::build(input, split..span.end),
        )
    }

    fn leaf(input: &impl HashInput, span: Range<usize>) -> Self {
        // Recursive BLAKE3 splits make each start chunk-aligned and each length
        // valid for that offset. Only the rightmost subtree may be partial.
        let mut bytes = [0; BLOCK_BYTES];
        input.read(span.clone(), &mut bytes[..span.len()]);
        let cv = blake3::Hasher::new()
            .set_input_offset(span.start as u64)
            .update(&bytes[..span.len()])
            .finalize_non_root();
        Self { cv, children: None }
    }

    fn parent(left: Arc<Self>, right: Arc<Self>) -> Arc<Self> {
        Arc::new(Self {
            cv: merge_subtrees_non_root(&left.cv, &right.cv, Mode::Hash),
            children: Some((left, right)),
        })
    }

    fn update(
        old: &Arc<Self>,
        input: &impl HashInput,
        span: Range<usize>,
        dirty: &Range<usize>,
    ) -> Arc<Self> {
        if span.end <= dirty.start || dirty.end <= span.start {
            return Arc::clone(old);
        }
        if let Some((old_left, old_right)) = &old.children {
            let split = span.start + left_subtree_len(span.len() as u64) as usize;
            let left = Self::update(old_left, input, span.start..split, dirty);
            let right = Self::update(old_right, input, split..span.end, dirty);
            if Arc::ptr_eq(&left, old_left) && Arc::ptr_eq(&right, old_right) {
                Arc::clone(old)
            } else {
                Self::parent(left, right)
            }
        } else {
            let next = Self::leaf(input, span);
            if next.cv == old.cv {
                Arc::clone(old)
            } else {
                Arc::new(next)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct Bytes {
        data: Vec<u8>,
        read_bytes: Cell<usize>,
    }

    impl Bytes {
        fn new(len: usize) -> Self {
            Self {
                data: (0..len).map(|i| (i * 17 + i / 251) as u8).collect(),
                read_bytes: Cell::new(0),
            }
        }

        fn assert_digest(&self, tree: &FoldTree) {
            assert_eq!(tree.digest().0.0, *blake3::hash(&self.data).as_bytes());
        }
    }

    impl HashInput for Bytes {
        fn len(&self) -> usize {
            self.data.len()
        }
        fn read(&self, range: Range<usize>, output: &mut [u8]) {
            self.read_bytes.set(self.read_bytes.get() + range.len());
            output.copy_from_slice(&self.data[range]);
        }
    }

    struct ModelCase {
        declarations: Arc<Vec<HashId>>,
        records: Arc<RecordHashes>,
    }

    impl ModelCase {
        fn new([decls, nodes, edges, arcs]: [usize; 4]) -> Self {
            let hashes = |count| {
                (0..count)
                    .map(|i| {
                        let mut hash = [0; 32];
                        hash[..8].copy_from_slice(&(i as u64).to_be_bytes());
                        hash[31] = (i % 251) as u8;
                        HashId(hash)
                    })
                    .collect()
            };
            Self {
                declarations: Arc::new(hashes(decls)),
                records: Arc::new(RecordHashes {
                    nodes: hashes(nodes),
                    edges: hashes(edges),
                    arcs: hashes(arcs),
                }),
            }
        }

        fn fold(&self, scheme: TargetScheme) -> Arc<SemanticFold> {
            Arc::new(SemanticFold::new(
                scheme,
                Arc::clone(&self.declarations),
                Arc::clone(&self.records),
            ))
        }

        // Independent byte oracle: fixed published header/domain encoding. Do
        // not call ModelHashInput here; this catches mistakes in that encoder.
        fn bytes(&self, declarations: &[HashId], scheme: TargetScheme) -> Vec<u8> {
            let mut bytes = b"HYMEKOCNSEMMODEL".to_vec();
            bytes.extend_from_slice(&[2, 0, 3, 0, scheme.tag()]);
            for (domain, hashes) in [
                (b"SEMDECL\0", declarations),
                (b"SEMNODE\0", self.records.nodes.as_slice()),
                (b"SEMEDGE\0", self.records.edges.as_slice()),
                (b"SEMARC\0\0", self.records.arcs.as_slice()),
            ] {
                bytes.extend_from_slice(domain);
                bytes.extend_from_slice(&(hashes.len() as u64).to_le_bytes());
                for hash in hashes {
                    bytes.extend_from_slice(&hash.0);
                }
            }
            bytes
        }
    }

    #[test]
    fn virtual_ranges_match_independent_encoding_across_domain_and_hash_boundaries() {
        let case = ModelCase::new([47, 5, 2, 33]);
        for scheme in [TargetScheme::Path, TargetScheme::DeclIdFallback] {
            let input = ModelHashInput::new(scheme, &case.declarations, &case.records);
            let bytes = case.bytes(&case.declarations, scheme);
            assert_eq!(input.len(), bytes.len());
            assert_eq!(input.digest().0.0, *blake3::hash(&bytes).as_bytes());
            for start in (0..=bytes.len()).step_by(13).chain([bytes.len()]) {
                for size in [0, 1, 7, 8, 15, 16, 31, 32, 33, 1023, 1024, BLOCK_BYTES] {
                    let end = (start + size).min(bytes.len());
                    let mut output = vec![0xFF; end - start];
                    input.read(start..end, &mut output);
                    assert_eq!(output, bytes[start..end], "range {start}..{end}");
                }
            }
        }
    }

    #[test]
    fn tree_matches_standard_blake3_at_chunk_page_and_tree_boundaries() {
        for len in [0, 1, 63, 64, 65, 1023, 1024, 1025].into_iter().chain(
            [1, 2, 3, 4, 7, 8].into_iter().flat_map(|pages| {
                [
                    pages * BLOCK_BYTES - 1,
                    pages * BLOCK_BYTES,
                    pages * BLOCK_BYTES + 1,
                ]
            }),
        ) {
            let bytes = Bytes::new(len);
            let tree = FoldTree::build(&bytes, None, &(0..len));
            bytes.assert_digest(&tree);
            assert_eq!(
                bytes.read_bytes.get(),
                len,
                "one initial read at length {len}"
            );
        }
    }

    fn leaves(tree: &FoldTree, len: usize) -> Vec<(Range<usize>, usize)> {
        fn collect(node: &Arc<HashNode>, span: Range<usize>, out: &mut Vec<(Range<usize>, usize)>) {
            if let Some((left, right)) = &node.children {
                let split = span.start + left_subtree_len(span.len() as u64) as usize;
                collect(left, span.start..split, out);
                collect(right, split..span.end, out);
            } else {
                out.push((span, Arc::as_ptr(node) as usize));
            }
        }
        let mut out = Vec::new();
        if let FoldTree::Branch { left, right } = tree {
            let split = left_subtree_len(len as u64) as usize;
            collect(left, 0..split, &mut out);
            collect(right, split..len, &mut out);
        }
        out
    }

    #[test]
    fn local_and_spanning_edits_read_only_intersecting_pages() {
        let len = 8 * BLOCK_BYTES + 37;
        let base = Bytes::new(len);
        let tree = FoldTree::build(&base, None, &(0..len));
        let old_leaves = leaves(&tree, len);
        assert_eq!(old_leaves.len(), 9);
        for (changed, expected_read) in [
            (3 * BLOCK_BYTES + 55..3 * BLOCK_BYTES + 56, BLOCK_BYTES),
            (BLOCK_BYTES - 7..BLOCK_BYTES + 41, 2 * BLOCK_BYTES),
            (8 * BLOCK_BYTES + 1..8 * BLOCK_BYTES + 2, 37),
            (0..len, len),
        ] {
            let mut next = Bytes::new(len);
            for value in &mut next.data[changed.clone()] {
                *value ^= 0x80;
            }
            let updated = FoldTree::build(&next, Some(&tree), &changed);
            next.assert_digest(&updated);
            assert_eq!(next.read_bytes.get(), expected_read);
            for ((span, before), (_, after)) in old_leaves.iter().zip(leaves(&updated, len)) {
                let touched = span.start < changed.end && changed.start < span.end;
                assert_eq!(*before == after, !touched, "sharing for {span:?}");
            }
        }
        base.assert_digest(&tree);
    }

    #[test]
    fn pending_edits_union_ranges_without_retaining_parent_folds() {
        let case = ModelCase::new([4096, 8, 17, 65]);
        let base = case.fold(TargetScheme::Path);
        let expected = base.digest();
        let seed = Arc::clone(&base.materialized.get().unwrap().tree);
        let mut values = (*case.declarations).clone();
        let mut pending = Arc::clone(&base);
        for id in [1000, 1001, 3000] {
            values[id].0[31] ^= 0x80;
            let next = Arc::new(pending.with_declarations(Arc::new(values.clone()), id..id + 1));
            assert!(!next.is_materialized());
            assert!(Arc::ptr_eq(next.seed.as_ref().unwrap(), &seed));
            let previous = Arc::downgrade(&pending);
            let was_base = Arc::ptr_eq(&pending, &base);
            pending = next;
            if !was_base {
                assert!(previous.upgrade().is_none());
            }
        }
        assert_eq!(
            pending.dirty,
            DECLARATIONS_START + 1000 * 32..DECLARATIONS_START + 3001 * 32
        );
        // Cancel the whole pending interval before requesting a digest. The
        // recomputed CVs match the seed and its immutable nodes can be reused.
        let cancelled = pending.with_declarations(Arc::clone(&case.declarations), 1000..3001);
        assert_eq!(cancelled.digest(), expected);
        assert_eq!(
            leaves(
                &cancelled.materialized.get().unwrap().tree,
                case.bytes(&case.declarations, TargetScheme::Path).len()
            ),
            leaves(
                &seed,
                case.bytes(&case.declarations, TargetScheme::Path).len()
            )
        );
        assert!(!pending.is_materialized());
        assert_eq!(
            pending.digest().0.0,
            *blake3::hash(&case.bytes(&values, TargetScheme::Path)).as_bytes()
        );
        assert_eq!(base.digest(), expected);
    }

    #[test]
    fn cold_chain_materializes_only_on_demand_and_is_thread_safe() {
        let case = ModelCase::new([1025, 0, 0, 0]);
        let initial = case.fold(TargetScheme::DeclIdFallback);
        let mut values = (*case.declarations).clone();
        values[511].0[31] ^= 0x80;
        let pending = Arc::new(initial.with_declarations(Arc::new(values.clone()), 511..512));
        assert!(!initial.is_materialized());
        assert!(!pending.is_materialized());
        assert!(pending.seed.is_none());
        let expected = *blake3::hash(&case.bytes(&values, TargetScheme::DeclIdFallback)).as_bytes();
        let barrier = std::sync::Barrier::new(8);
        let results = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        let digest = pending.digest();
                        (
                            digest,
                            pending.materialized.get().unwrap() as *const MaterializedFold as usize,
                        )
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>()
        });
        for result in &results {
            assert_eq!(result.0.0.0, expected);
            assert_eq!(result.1, results[0].1);
        }
        assert!(!initial.is_materialized());
        assert_eq!(
            initial.digest().0.0,
            *blake3::hash(&case.bytes(&case.declarations, TargetScheme::DeclIdFallback)).as_bytes()
        );
    }

    #[test]
    fn invalid_virtual_ranges_and_resize_are_rejected() {
        let case = ModelCase::new([2, 0, 0, 0]);
        let input = ModelHashInput::new(TargetScheme::Path, &case.declarations, &case.records);
        assert!(
            std::panic::catch_unwind(|| input.write_range(0..input.len() + 1, |_| {})).is_err()
        );
        let fold = case.fold(TargetScheme::Path);
        assert!(
            std::panic::catch_unwind(|| fold.with_declarations(Arc::new(vec![]), 0..1)).is_err()
        );
        assert!(
            std::panic::catch_unwind(
                || fold.with_declarations(Arc::clone(&case.declarations), 1..1)
            )
            .is_err()
        );
        assert!(!fold.is_materialized());
    }
}
