use std::hash::Hash;

pub trait GraphView {
    type Node: Copy + Eq + Hash;
    type NeighIter<'a>: Iterator<Item = Self::Node> + 'a
    where
        Self: 'a;

    fn neighbors<'a>(&'a self, n: Self::Node) -> Self::NeighIter<'a>;
}



