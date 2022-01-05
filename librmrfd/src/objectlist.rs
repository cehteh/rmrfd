use std::sync::Arc;

use dirinventory::ObjectPath;

/// Stores a sorted list of unique file paths.
#[derive(Debug)]
pub struct ObjectList(Vec<Arc<ObjectPath>>);

impl ObjectList {
    /// Creates a new ObjectList.
    pub fn new() -> ObjectList {
        ObjectList(Vec::new())
    }

    /// Insert an object, only when not already present.
    pub fn insert(&mut self, object: Arc<ObjectPath>) {
        if let Err(idx) = self.0.binary_search(&object) {
            self.0.insert(idx, object);
        }
    }

    /// Removes an object if present.
    pub fn remove(&mut self, object: Arc<ObjectPath>) {
        if let Ok(idx) = self.0.binary_search(&object) {
            self.0.remove(idx);
        }
    }

    /// Insert an object, only when not already present.
    pub fn contains(&self, object: Arc<ObjectPath>) -> bool {
        self.0.binary_search(&object).is_ok()
    }

    /// Iterator over all stored objects in sorted order.
    pub fn iter(&mut self) -> std::slice::Iter<'_, Arc<ObjectPath>> {
        self.0.iter()
    }

    /// Returns 'true' when at least one object is stored.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns the number of stored objects.
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn objectlist_insert_uniq() {
        let mut ol = ObjectList::new();

        ol.insert(ObjectPath::new("foo"));
        ol.insert(ObjectPath::new("bar"));
        ol.insert(ObjectPath::new("baz"));
        ol.insert(ObjectPath::new("foo"));
        eprintln!("{:?}", ol);
        assert_eq!(ol.len(), 3);
    }
}
