#![feature(once_cell)]
#![feature(hash_set_entry)]
#![feature(dir_entry_ext2)]
use std::fs::{read_dir, Metadata};
use std::os::unix::fs::MetadataExt;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::ffi::{OsStr, OsString};
use std::sync::Arc;
use std::io;
use std::borrow::Borrow;
use std::ops::Deref;
use std::os::unix::fs::DirEntryExt2;

struct Inventory {
    entries:      HashMap<u64, BTreeSet<InventoryEntry>>,
    counter:      u64,
    thousand:     u64,
    cached_names: HashSet<CachedName>,
}

impl Inventory {
    pub fn new() -> Inventory {
        Inventory {
            entries:      HashMap::new(),
            counter:      1,
            thousand:     1000,
            cached_names: HashSet::new(),
        }
    }

    fn load_dir_recursive_intern(
        &mut self,
        dir: Arc<ObjectPath>,
        path: &mut PathBuf,
    ) -> io::Result<()> {
        self.thousand -= 1;
        if self.thousand == 0 {
            eprintln!("{}: {:?}", self.counter, &path);
            self.thousand = 1000;
        }
        for entry in read_dir(&path)? {
            self.counter += 1;
            match entry {
                Ok(entry) => {
                    let metadata = entry.metadata()?;
                    if metadata.is_dir() {
                        let dirname = self.cache_name(entry.file_name_ref());
                        path.push(&dirname);
                        self.load_dir_recursive_intern(
                            ObjectPath::subobject(dir.clone(), dirname),
                            path,
                        )?;
                        path.pop();
                    } else {
                        if metadata.blocks() > 0 {
                            let name = self.cache_name(entry.file_name_ref());
                            self.entries
                                .entry(metadata.dev())
                                .or_default()
                                .insert(InventoryEntry::new(
                                    ObjectPath::subobject(dir.clone(), name),
                                    &metadata
                                ));
                        }
                    }
                }
                Err(err) => return Err(err), /* TODO: log but ignore errors, we'll want to delete anyway */
            }
        }
        Ok(())
    }

    pub fn load_dir_recursive<P: AsRef<Path>>(&mut self, dir: P) -> io::Result<()> {
        self.load_dir_recursive_intern(
            Arc::new(ObjectPath::new(&dir)),
            &mut PathBuf::from(&dir.as_ref()),
        )
    }

    pub fn cache_name(&mut self, name: &OsStr) -> CachedName {
        self.cached_names
            .get_or_insert_with(name, |name| CachedName(Arc::new(OsString::from(name))))
            .clone()
    }

    pub fn garbage_collect() {
        /* PLANNED: remove all entries with refcount == 1 (drain_filter)*/
    }
}

struct InventoryEntry {
    blocks: u64,
    ino:    u64,
    // parent_dir: RmrfDir
    path:   Arc<ObjectPath>,
}

impl InventoryEntry {
    fn new(path: Arc<ObjectPath>, metadata: &Metadata) -> InventoryEntry {
        InventoryEntry {
            blocks: metadata.blocks(),
            ino: metadata.ino(),
            // parent_dir: RmrfDir
            path,
        }
    }
}

use std::cmp::Ordering;

impl Ord for InventoryEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        let r = self.blocks.cmp(&other.blocks);
        if r == Ordering::Equal {
            let r = self.ino.cmp(&other.ino);
            if r == Ordering::Equal {
                self.path.cmp(&other.path)
            } else {
                r
            }
        } else {
            r
        }
    }
}

impl PartialOrd for InventoryEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for InventoryEntry {
    fn eq(&self, other: &Self) -> bool {
        self.blocks == other.blocks && self.ino == other.ino
    }
}

impl Eq for InventoryEntry {}

#[derive(Debug, Hash, PartialOrd, PartialEq, Eq, Ord)]
struct CachedName(Arc<OsString>);

impl CachedName {
    fn new(s: &OsStr) -> CachedName {
        CachedName(Arc::new(OsString::from(s)))
    }
}

impl Borrow<OsStr> for CachedName {
    fn borrow(&self) -> &OsStr {
        &self.0
    }
}

impl Deref for CachedName {
    type Target = OsStr;

    fn deref(&self) -> &OsStr {
        &self.0
    }
}

impl Clone for CachedName {
    fn clone(&self) -> CachedName {
        CachedName(self.0.clone())
    }
}

impl AsRef<Path> for CachedName {
    fn as_ref(&self) -> &Path {
        Path::new(&*self.0)
    }
}

#[derive(PartialOrd, PartialEq, Ord)]
struct ObjectPath {
    parent: Option<Arc<ObjectPath>>,
    name:   CachedName,
}

impl Eq for ObjectPath {}

impl ObjectPath {
    pub fn new<P: AsRef<Path>>(path: P) -> ObjectPath {
        ObjectPath {
            parent: None,
            name:   CachedName::new(path.as_ref().as_os_str()),
        }
    }

    pub fn subobject(parent: Arc<ObjectPath>, name: CachedName) -> Arc<ObjectPath> {
        Arc::new(ObjectPath {
            parent: Some(parent.clone()),
            name:   name.clone(),
        })
    }

    fn pathbuf_push_parent(&self, pathbuf: &mut PathBuf) {
        if let Some(parent) = &self.parent {
            parent.pathbuf_push_parent(pathbuf)
        };
        pathbuf.push(&*self.name);
    }

    pub fn to_pathbuf(&self) -> PathBuf {
        let mut pathbuf = PathBuf::new();
        self.pathbuf_push_parent(&mut pathbuf);
        pathbuf
    }
}

#[test]
fn directory_path_smoke() {
    assert_eq!(ObjectPath::new(".").to_pathbuf(), PathBuf::from("."));
}

#[test]
fn directory_path_subobject() {
    let p = Arc::new(ObjectPath::new("."));
    assert_eq!(
        ObjectPath::subobject(p, CachedName::new(OsStr::new("foo"))).to_pathbuf(),
        PathBuf::from("./foo")
    );
}

fn main() {
    eprintln!("Hello, world!");

    let mut inventory = Inventory::new();
    inventory.load_dir_recursive(".").unwrap();

    let mut sum = 0;
    inventory.entries
             .iter()
             .for_each(|table: (&u64, &BTreeSet<InventoryEntry>)| {
                 sum += table.1.len();
             });

    eprintln!("loaded entries: {}", sum);

    eprintln!("strings in cache: {}", inventory.cached_names.len());
}
