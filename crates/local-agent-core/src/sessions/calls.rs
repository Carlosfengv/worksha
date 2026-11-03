//! File-sized immutable call batches, shared by catalog snapshots.
use super::CallSummary;
use serde::{Serialize, Serializer, ser::SerializeSeq};
use std::{
    collections::HashSet,
    ops::{Index, IndexMut},
    sync::{Arc, Weak},
};
type Batch = Arc<Vec<CallSummary>>;

// Weak identities keep addresses unique without retaining old million-row histories.
#[derive(Clone)]
pub(crate) struct CallSnapshotIdentity {
    chunks: Vec<Weak<Vec<CallSummary>>>,
    directory: Weak<crate::agents::AgentDirectory>,
}
impl CallSnapshotIdentity {
    pub(crate) fn is_live(&self) -> bool {
        self.directory.strong_count() > 0
            && self.chunks.iter().all(|chunk| chunk.strong_count() > 0)
    }
}
impl PartialEq for CallSnapshotIdentity {
    fn eq(&self, other: &Self) -> bool {
        self.directory.ptr_eq(&other.directory)
            && self.chunks.len() == other.chunks.len()
            && self
                .chunks
                .iter()
                .zip(&other.chunks)
                .all(|(a, b)| a.ptr_eq(b))
    }
}
impl Eq for CallSnapshotIdentity {}

#[derive(Clone)]
pub struct CallCatalog {
    pub(crate) chunks: Vec<Batch>,
    directory: Arc<crate::agents::AgentDirectory>,
}
impl Default for CallCatalog {
    fn default() -> Self {
        Self {
            chunks: vec![],
            directory: Arc::new(crate::agents::builtin_directory().clone()),
        }
    }
}
impl CallCatalog {
    pub(crate) fn snapshot_identity(&self) -> CallSnapshotIdentity {
        CallSnapshotIdentity {
            chunks: self.chunks.iter().map(Arc::downgrade).collect(),
            directory: Arc::downgrade(&self.directory),
        }
    }
    pub fn directory(&self) -> &crate::agents::AgentDirectory {
        &self.directory
    }
    pub fn with_directory(mut self, directory: Arc<crate::agents::AgentDirectory>) -> Self {
        self.directory = directory;
        self
    }
    pub(crate) fn shared(chunks: impl Iterator<Item = Arc<Vec<CallSummary>>>) -> Self {
        Self {
            chunks: chunks.collect(),
            ..Default::default()
        }
    }
    pub fn len(&self) -> usize {
        self.chunks.iter().map(|c| c.len()).sum()
    }
    pub fn is_empty(&self) -> bool {
        self.chunks.iter().all(|c| c.is_empty())
    }
    pub fn clear(&mut self) {
        self.chunks.clear();
    }
    pub fn iter(&self) -> impl Iterator<Item = &CallSummary> {
        self.chunks.iter().flat_map(|c| c.iter())
    }
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut CallSummary> {
        self.chunks
            .iter_mut()
            .flat_map(|c| Arc::make_mut(c).iter_mut())
    }
    pub fn retain(&mut self, mut keep: impl FnMut(&CallSummary) -> bool) {
        for chunk in &mut self.chunks {
            if let Some(first) = chunk.iter().position(|c| !keep(c)) {
                let mut next = chunk[..first].to_vec();
                next.extend(chunk[first + 1..].iter().filter(|c| keep(c)).cloned());
                *chunk = Arc::new(next);
            }
        }
        self.chunks.retain(|c| !c.is_empty());
    }
    pub(crate) fn delta<'a>(&'a self, previous: &'a Self) -> (Vec<&'a Batch>, Vec<&'a Batch>) {
        let old: HashSet<_> = previous.chunks.iter().map(Arc::as_ptr).collect();
        let new: HashSet<_> = self.chunks.iter().map(Arc::as_ptr).collect();
        (
            previous
                .chunks
                .iter()
                .filter(|c| !new.contains(&Arc::as_ptr(c)))
                .collect(),
            self.chunks
                .iter()
                .filter(|c| !old.contains(&Arc::as_ptr(c)))
                .collect(),
        )
    }
}

impl From<Vec<CallSummary>> for CallCatalog {
    fn from(calls: Vec<CallSummary>) -> Self {
        Self {
            chunks: vec![Arc::new(calls)],
            ..Default::default()
        }
    }
}
impl FromIterator<CallSummary> for CallCatalog {
    fn from_iter<T: IntoIterator<Item = CallSummary>>(calls: T) -> Self {
        calls.into_iter().collect::<Vec<_>>().into()
    }
}
impl Index<usize> for CallCatalog {
    type Output = CallSummary;
    fn index(&self, mut index: usize) -> &Self::Output {
        for chunk in &self.chunks {
            if index < chunk.len() {
                return &chunk[index];
            }
            index -= chunk.len();
        }
        panic!("call index out of bounds")
    }
}
impl IndexMut<usize> for CallCatalog {
    fn index_mut(&mut self, mut index: usize) -> &mut Self::Output {
        for chunk in &mut self.chunks {
            if index < chunk.len() {
                return &mut Arc::make_mut(chunk)[index];
            }
            index -= chunk.len();
        }
        panic!("call index out of bounds")
    }
}
impl Serialize for CallCatalog {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.len()))?;
        for call in self.iter() {
            seq.serialize_element(call)?;
        }
        seq.end()
    }
}
impl<'a> IntoIterator for &'a CallCatalog {
    type Item = &'a CallSummary;
    type IntoIter = Box<dyn Iterator<Item = Self::Item> + 'a>;
    fn into_iter(self) -> Self::IntoIter {
        Box::new(self.iter())
    }
}
impl<'a> IntoIterator for &'a mut CallCatalog {
    type Item = &'a mut CallSummary;
    type IntoIter = Box<dyn Iterator<Item = Self::Item> + 'a>;
    fn into_iter(self) -> Self::IntoIter {
        Box::new(self.iter_mut())
    }
}
impl IntoIterator for CallCatalog {
    type Item = CallSummary;
    type IntoIter = std::iter::FlatMap<
        std::vec::IntoIter<Arc<Vec<CallSummary>>>,
        std::vec::IntoIter<CallSummary>,
        fn(Arc<Vec<CallSummary>>) -> std::vec::IntoIter<CallSummary>,
    >;
    fn into_iter(self) -> Self::IntoIter {
        self.chunks
            .into_iter()
            .flat_map(|c| Arc::unwrap_or_clone(c).into_iter())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshots_share_batches_and_copy_only_the_changed_file() {
        let original = crate::associated_cost_tests::catalog().calls;
        let mut snapshot = original.clone();
        assert!(Arc::ptr_eq(&original.chunks[0], &snapshot.chunks[0]));
        let previous = serde_json::to_value(&original).unwrap();
        snapshot[0].model = Some("changed".into());
        assert!(!Arc::ptr_eq(&original.chunks[0], &snapshot.chunks[0]));
        assert!(Arc::ptr_eq(&original.chunks[1], &snapshot.chunks[1]));
        assert_eq!(serde_json::to_value(&original).unwrap(), previous);
        let (removed, added) = snapshot.delta(&original);
        assert_eq!((removed.len(), added.len()), (1, 1));
        assert_eq!(removed[0][0].id, added[0][0].id);
        assert_eq!(
            serde_json::to_value(&snapshot).unwrap(),
            serde_json::to_value(snapshot.iter().collect::<Vec<_>>()).unwrap()
        );
        let mut examined = 0;
        snapshot.retain(|_| {
            examined += 1;
            true
        });
        assert_eq!(examined, 2);
        assert!(Arc::ptr_eq(&original.chunks[1], &snapshot.chunks[1]));
        let id = snapshot[0].id.clone();
        snapshot.retain(|c| c.id != id);
        assert_eq!(snapshot.len(), 1);
        assert_eq!(original.len(), 2);
        assert!(Arc::ptr_eq(&original.chunks[1], &snapshot.chunks[0]));
    }
}
