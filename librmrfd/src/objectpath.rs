use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::InternedName;

/// Space efficient storage of paths. Instead storing full pathnames it stores only interned
/// strings of the actual object names and a reference to its parent. Note tat since parents
/// are usually shared between all ObjectPath instances, the API uses Arc<ObjectPath> instead
/// plain objects.
#[derive(Hash, PartialOrd, PartialEq, Ord)]
pub struct ObjectPath {
    parent: Option<Arc<ObjectPath>>,
    name:   InternedName,
}

impl Eq for ObjectPath {}

impl ObjectPath {
    /// Creates a new ObjectPath without a parent.
    pub fn new<P: AsRef<Path>>(path: P) -> Arc<ObjectPath> {
        Arc::new(ObjectPath {
            parent: None,
            name:   InternedName::new(path.as_ref().as_os_str()),
        })
    }

    /// Creates a new ObjectPath as subobject to some existing ObjectPath object.
    pub fn subobject(parent: Arc<ObjectPath>, name: InternedName) -> Arc<ObjectPath> {
        Arc::new(ObjectPath {
            parent: Some(parent),
            name,
        })
    }

    fn pathbuf_push_parent(&self, target: &mut PathBuf, len: usize) {
        if let Some(parent) = &self.parent {
            parent.pathbuf_push_parent(target, len + self.name.len() + 1 /* delimiter char */)
        } else {
            target.reserve(len + self.name.len());
        };
        target.push(&*self.name);
    }

    /// Construct the ObjectPath as String in the given PathBuf.
    pub fn to_pathbuf<'a>(&self, target: &'a mut PathBuf) -> &'a PathBuf {
        target.clear();
        self.pathbuf_push_parent(target, 1 /* maybe for root delimter */);
        target
    }
}

#[test]
fn objectpath_path_smoke() {
    let mut pathbuf = PathBuf::new();
    assert_eq!(
        ObjectPath::new(".").to_pathbuf(&mut pathbuf),
        &PathBuf::from(".")
    );
}

#[test]
fn objectpath_path_subobject() {
    use std::ffi::OsStr;
    let p = ObjectPath::new(".");
    let mut pathbuf = PathBuf::new();
    assert_eq!(
        ObjectPath::subobject(p, InternedName::new(OsStr::new("foo"))).to_pathbuf(&mut pathbuf),
        &PathBuf::from("./foo")
    );
}
