//! Lossless storage for prepared numeric incidence coordinates.
use std::sync::OnceLock;

/// Lossless arithmetic runs, discovered from resolved IR IDs, never fixture metadata.
#[derive(Clone, Copy, Debug)]
pub(super) struct TargetRun {
    pub(super) first: usize,
    pub(super) step: isize,
    pub(super) len: usize,
}
impl TargetRun {
    pub(super) fn range_count(self) -> usize {
        if self.step.unsigned_abs() == 1 {
            1
        } else {
            self.len
        }
    }
    pub(super) fn for_each_range(self, mut visit: impl FnMut(std::ops::Range<usize>)) {
        if self.step == 1 {
            visit(self.first..self.first + self.len);
        } else if self.step == -1 {
            visit(self.first + 1 - self.len..self.first + 1);
        } else {
            for target in self.indices() {
                visit(target..target + 1);
            }
        }
    }
    fn difference(first: usize, next: usize) -> Option<isize> {
        if next >= first {
            isize::try_from(next - first).ok()
        } else {
            isize::try_from(first - next).ok().map(|d| -d)
        }
    }
    fn visit(targets: &[usize], mut emit: impl FnMut(Self)) {
        let mut start = 0;
        while start < targets.len() {
            let step = targets
                .get(start + 1)
                .and_then(|&next| Self::difference(targets[start], next));
            let mut end = start + 1;
            if let Some(step) = step {
                // A two-point jump must not consume the first coordinate of
                // the following contiguous run (e.g. hub, leaf, leaf+1, ...).
                // Keep the singleton and the whole run, with identical slots.
                if step.unsigned_abs() != 1
                    && targets.get(start + 2).is_some_and(|&third| {
                        Self::difference(targets[start + 1], third)
                            .is_some_and(|next| next.unsigned_abs() == 1)
                    })
                {
                    emit(Self {
                        first: targets[start],
                        step: 0,
                        len: 1,
                    });
                    start += 1;
                    continue;
                }
                end += 1;
                while end < targets.len()
                    && targets[end - 1].checked_add_signed(step) == Some(targets[end])
                {
                    end += 1;
                }
            }
            emit(Self {
                first: targets[start],
                step: step.unwrap_or(0),
                len: end - start,
            });
            start = end;
        }
    }
    #[inline]
    pub(super) fn indices(self) -> impl ExactSizeIterator<Item = usize> {
        (0..self.len).map(move |position| {
            // Every emitted endpoint was checked against the original target sequence.
            let distance = self.step.unsigned_abs() * position;
            if self.step >= 0 {
                self.first + distance
            } else {
                self.first - distance
            }
        })
    }
    #[inline]
    pub(super) fn values(self, input: &[f64]) -> RunValues<'_> {
        let contiguous = self
            .first
            .checked_add(self.len)
            .filter(|&end| self.step == 1 && end <= input.len());
        if let Some(end) = contiguous {
            RunValues::Contiguous(input[self.first..end].iter().copied())
        } else {
            // Keep lazy bounds checks and coefficient-call order on short inputs.
            RunValues::Indexed {
                run: self,
                input,
                position: 0,
            }
        }
    }
}

pub(super) enum RunValues<'a> {
    Contiguous(std::iter::Copied<std::slice::Iter<'a, f64>>),
    Indexed {
        run: TargetRun,
        input: &'a [f64],
        position: usize,
    },
}
impl Iterator for RunValues<'_> {
    type Item = f64;
    #[inline]
    fn next(&mut self) -> Option<f64> {
        match self {
            Self::Contiguous(values) => values.next(),
            Self::Indexed {
                run,
                input,
                position,
            } => {
                let index = run.indices().nth(*position)?;
                *position += 1;
                Some(input[index])
            }
        }
    }
    #[inline]
    fn fold<B, F>(self, initial: B, mut f: F) -> B
    where
        F: FnMut(B, f64) -> B,
    {
        match self {
            Self::Contiguous(values) => values.fold(initial, f),
            Self::Indexed {
                run,
                input,
                position,
            } => run
                .indices()
                .skip(position)
                .fold(initial, |state, index| f(state, input[index])),
        }
    }
}

#[derive(Debug)]
pub(super) enum ProjectionTargets {
    Indexed(Box<[usize]>),
    Runs {
        rows: Box<[usize]>,
        runs: Box<[TargetRun]>,
        expanded: OnceLock<Box<[usize]>>,
    },
}
impl ProjectionTargets {
    pub(super) fn range_count(&self) -> usize {
        match self {
            Self::Indexed(targets) => targets.len(),
            Self::Runs { runs, .. } => runs.iter().map(|run| run.range_count()).sum(),
        }
    }
    pub(super) fn build(offsets: &[usize], targets: Vec<usize>) -> Self {
        let row_count = offsets.len() - 1;
        // Pair-heavy inputs retain exactly their original indexed allocation/layout.
        if targets.len() < row_count.saturating_mul(4) {
            return Self::Indexed(targets.into_boxed_slice());
        }
        let mut sampled_slots = 0;
        let mut sampled_runs = 0;
        let mut sampled_rows = 0;
        for bounds in offsets.windows(2).filter(|r| r[0] != r[1]).take(8) {
            sampled_rows += 1;
            sampled_slots += bounds[1] - bounds[0];
            let mut contiguous = true;
            TargetRun::visit(&targets[bounds[0]..bounds[1]], |run| {
                sampled_runs += 1;
                contiguous &= run.len <= 2 || run.step.unsigned_abs() <= 1;
            });
            if !contiguous {
                return Self::Indexed(targets.into_boxed_slice());
            }
        }
        if sampled_runs * size_of::<TargetRun>() + sampled_rows * size_of::<usize>()
            >= sampled_slots * size_of::<usize>()
        {
            return Self::Indexed(targets.into_boxed_slice());
        }
        let mut rows = Vec::with_capacity(offsets.len());
        let mut runs = Vec::new();
        rows.push(0);
        for bounds in offsets.windows(2) {
            TargetRun::visit(&targets[bounds[0]..bounds[1]], |run| runs.push(run));
            rows.push(runs.len());
        }
        if runs
            .iter()
            .any(|run| run.len > 2 && run.step.unsigned_abs() > 1)
            || rows.len() * size_of::<usize>() + runs.len() * size_of::<TargetRun>()
                >= targets.len() * size_of::<usize>()
        {
            return Self::Indexed(targets.into_boxed_slice());
        }
        Self::Runs {
            rows: rows.into_boxed_slice(),
            runs: runs.into_boxed_slice(),
            expanded: OnceLock::new(),
        }
    }
    #[inline]
    pub(super) fn as_slice(&self) -> &[usize] {
        match self {
            Self::Indexed(targets) => targets,
            Self::Runs { runs, expanded, .. } => expanded.get_or_init(|| {
                runs.iter()
                    .flat_map(|run| run.indices())
                    .collect::<Vec<_>>()
                    .into_boxed_slice()
            }),
        }
    }
    pub(super) fn retained_bytes(&self) -> usize {
        match self {
            Self::Indexed(targets) => size_of_val(targets.as_ref()),
            Self::Runs {
                rows,
                runs,
                expanded,
            } => {
                size_of_val(rows.as_ref())
                    + size_of_val(runs.as_ref())
                    + expanded.get().map_or(0, |data| size_of_val(data.as_ref()))
            }
        }
    }
}

#[cfg(test)]
mod run_tests {
    use super::TargetRun;

    #[test]
    fn shared_hub_or_duplicate_does_not_split_the_following_contiguous_run() {
        for targets in [
            vec![0, 10, 11, 12, 13],
            vec![0, 13, 12, 11, 10],
            vec![10, 10, 11, 12, 13],
        ] {
            let mut runs = Vec::new();
            TargetRun::visit(&targets, |r| runs.push(r));
            assert_eq!(runs.iter().map(|r| r.range_count()).sum::<usize>(), 2);
            assert_eq!(
                runs.iter().flat_map(|r| r.indices()).collect::<Vec<_>>(),
                targets
            );
        }
    }

    #[test]
    fn range_view_preserves_multiplicity_and_reports_exact_span_count() {
        for targets in [
            vec![],
            vec![0, 1, 2, 3],
            vec![7, 6, 5, 4],
            vec![3, 3, 3],
            vec![0, 9, 10, 11, 2, 2],
        ] {
            let mut reconstructed = Vec::new();
            let mut count = 0;
            let mut reported = 0;
            TargetRun::visit(&targets, |run| {
                reported += run.range_count();
                run.for_each_range(|range| {
                    count += 1;
                    reconstructed.extend(range);
                });
            });
            let mut expected = targets;
            expected.sort_unstable();
            reconstructed.sort_unstable();
            assert_eq!(expected, reconstructed);
            assert_eq!(count, reported);
        }
    }

    #[test]
    fn run_encoding_preserves_empty_reversed_wide_and_unrepresentable_steps() {
        let mut cases = vec![
            vec![0, usize::MAX, 0, usize::MAX],
            vec![3; 65],
            (0..64).step_by(3).collect(),
        ];
        for base in [0, isize::MAX as usize - 64, usize::MAX - 64] {
            for len in [0, 1, 2, 63, 64] {
                let row: Vec<_> = (base..base + len).collect();
                cases.push(row.iter().rev().copied().collect());
                cases.push(row);
            }
        }
        for original in cases {
            let mut reconstructed = Vec::new();
            TargetRun::visit(&original, |run| reconstructed.extend(run.indices()));
            assert_eq!(reconstructed, original);
        }
    }
}
