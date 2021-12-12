#![feature(once_cell)]
#![feature(hash_set_entry)]
#![feature(dir_entry_ext2)]
use std::fs::{read_dir, Metadata};
use std::os::unix::fs::MetadataExt;
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::ffi::{OsStr, OsString};
use std::sync::Arc;
use std::io;
use std::borrow::Borrow;
use std::ops::Deref;
use std::os::unix::fs::DirEntryExt2;

struct Inventory {
    entries:      BTreeSet<InventoryEntry>,
    counter:      u64,
    thousand:     u64,
    cached_names: HashSet<CachedName>,
}

impl Inventory {
    pub fn new() -> Inventory {
        Inventory {
            entries:      BTreeSet::new(),
            counter:      1,
            thousand:     1000,
            cached_names: HashSet::new(),
        }
    }

    fn load_dir_recursive_intern(
        &mut self,
        dir: Arc<DirectoryPath>,
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
                            DirectoryPath::subdir(dir.clone(), dirname),
                            path,
                        )?;
                        path.pop();
                    } else {
                        if metadata.blocks() > 32 {
                            let name = self.cache_name(entry.file_name_ref());
                            self.entries
                                .insert(InventoryEntry::new(dir.clone(), name, &metadata));
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
            Arc::new(DirectoryPath::new(&dir)),
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
    dev:    u64,
    nlink:  u64,
    // parent_dir: RmrfDir
    path:   Arc<DirectoryPath>,
    name:   CachedName,
}

impl InventoryEntry {
    fn new(path: Arc<DirectoryPath>, name: CachedName, metadata: &Metadata) -> InventoryEntry {
        InventoryEntry {
            blocks: metadata.blocks(),
            ino: metadata.ino(),
            dev: metadata.dev(),
            nlink: metadata.nlink(),
            // parent_dir: RmrfDir
            path,
            name,
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
                let r = self.dev.cmp(&other.dev);
                if r == Ordering::Equal {
                    self.path.cmp(&other.path)
                } else {
                    r
                }
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
        self.blocks == other.blocks && self.ino == other.ino && self.dev == other.dev
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
struct DirectoryPath {
    parent: Option<Arc<DirectoryPath>>,
    name:   CachedName,
}

impl Eq for DirectoryPath {}

impl DirectoryPath {
    pub fn new<P: AsRef<Path>>(path: P) -> DirectoryPath {
        DirectoryPath {
            parent: None,
            name:   CachedName::new(path.as_ref().as_os_str()),
        }
    }

    pub fn subdir(parent: Arc<DirectoryPath>, name: CachedName) -> Arc<DirectoryPath> {
        Arc::new(DirectoryPath {
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
    assert_eq!(DirectoryPath::new(".").to_pathbuf(), PathBuf::from("."));
}

#[test]
fn directory_path_subdir() {
    let p = Arc::new(DirectoryPath::new("."));
    assert_eq!(
        DirectoryPath::subdir(p, CachedName::new(OsStr::new("foo"))).to_pathbuf(),
        PathBuf::from("./foo")
    );
}

fn main() {
    eprintln!("Hello, world!");

    let mut inventory = Inventory::new();
    inventory.load_dir_recursive(".").unwrap();

    eprintln!("loaded entries: {}", inventory.entries.len());

    for item in inventory.entries {
        println!(
            "{}: {}: {:?}",
            item.blocks,
            item.nlink,
            item.path.to_pathbuf().join(item.name)
        );
    }
}
