use std::io;
use std::fs::{read_dir, Metadata};
use std::os::unix::fs::MetadataExt;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::os::unix::fs::DirEntryExt2;
use std::cmp::Ordering;

use crate::{InternedNames, ObjectPath};

/// Slightly better name for device identifiers returned from metadata.
pub type DeviceId = u64;

/// All names linked to a single file on disk.
pub type ObjectList = Vec<std::sync::Arc<ObjectPath>>;

/// Space efficient store for file metadata of files larger than a certain min_blocksize.
/// This is used to find whcih files to delete first for most space efficient deletion.  There
/// should be only one 'Inventory' around as it is used to merge hardlinks and needs to have a
/// global picture of all indexed files.
pub struct Inventory {
    entries: HashMap<DeviceId, BTreeMap<InventoryKey, ObjectList>>,
    names:   InternedNames,

    // config section
    min_blockcount: u64,
    // stats
}

impl Inventory {
    /// Create an Inventory. The 'min_blockcount' sets a pre-filter for keeping only files
    /// which have more than this much blocks (512 bytes) allocated. Since this inventory is
    /// used for deleting the biggiest files (with all hardlinks to be deleted) first, setting
    /// this blockcount to some reasonably higher number can save a lot memory. Files not
    /// indexed in the directory will get deleted anyway on a later pass.
    pub fn new(min_blockcount: u64) -> Inventory {
        Inventory {
            entries: HashMap::new(),
            names: InternedNames::new(),
            min_blockcount,
        }
    }

    // dir and path reflect the same thing, the Pathbuf is used as mutable state throughout
    // the recursion.
    fn load_dir_recursive_intern(
        &mut self,
        dir: Arc<ObjectPath>,
        path: &mut PathBuf,
    ) -> io::Result<()> {
        for entry in read_dir(&path)? {
            match entry {
                Ok(entry) => {
                    let metadata = entry.metadata()?;
                    if metadata.is_dir() {
                        let dirname = self.names.interning(entry.file_name_ref());
                        path.push(&dirname);
                        self.load_dir_recursive_intern(
                            ObjectPath::subobject(dir.clone(), dirname),
                            path,
                        )?;
                        path.pop();
                    } else if metadata.blocks() > self.min_blockcount {
                        let name = self.names.interning(entry.file_name_ref());
                        self.entries
                            .entry(metadata.dev())
                            .or_default()
                            .entry(InventoryKey::new(&metadata))
                            .or_default()
                            .push(ObjectPath::subobject(dir.clone(), name));
                    }
                }
                Err(err) => return Err(err), /* TODO: log but ignore errors, we'll want to delete anyway */
            }
        }
        Ok(())
    }

    /// Adds a directory to the inventory
    pub fn load_dir_recursive<P: AsRef<Path>>(&mut self, dir: P) -> io::Result<()> {
        self.load_dir_recursive_intern(ObjectPath::new(&dir), &mut PathBuf::from(&dir.as_ref()))
    }
}

struct InventoryKey {
    blocks: u64,
    ino:    u64,
}

impl InventoryKey {
    fn new(metadata: &Metadata) -> InventoryKey {
        InventoryKey {
            blocks: metadata.blocks(),
            ino:    metadata.ino(),
        }
    }
}

impl Ord for InventoryKey {
    fn cmp(&self, other: &Self) -> Ordering {
        let r = self.blocks.cmp(&other.blocks);
        if r == Ordering::Equal {
            self.ino.cmp(&other.ino)
        } else {
            r
        }
    }
}

impl PartialOrd for InventoryKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for InventoryKey {
    fn eq(&self, other: &Self) -> bool {
        self.blocks == other.blocks && self.ino == other.ino
    }
}

impl Eq for InventoryKey {}
