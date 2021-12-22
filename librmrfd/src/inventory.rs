//! Inventory

use std::io;
use std::fs::Metadata;
use std::os::unix::fs::MetadataExt;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::thread;
use std::cmp::Ordering;
use std::sync::atomic::{self, AtomicUsize};

use easy_error::{Error, ResultExt};
use openat_ct as openat;
use openat::{Dir, SimpleType};
#[allow(unused_imports)]
pub use log::{debug, error, info, trace, warn};

use crate::{InternedNames, ObjectPath, PriorityQueue, QueueEntry};

/// Slightly better name for device identifiers returned from metadata.
pub type DeviceId = u64;

/// All names linked to a single file on disk.
// TODO: keep this sorted, removing by bsearch? let idx = s.binary_search(&num).unwrap_or_else(|x| x); s.insert(idx, num);
pub type ObjectList = Vec<std::sync::Arc<ObjectPath>>;

/// Create a space efficient store for file metadata of files larger than a certain
/// min_blocksize.  This is used to find whcih files to delete first for most space efficient
/// deletion.  There should be only one 'Inventory' around as it is used to merge hardlinks
/// and needs to have a global picture of all indexed files.
pub struct Inventory {
    entries: HashMap<DeviceId, BTreeMap<InventoryKey, ObjectList>>,
    names:   InternedNames,

    // thread management
    dirs_queue: PriorityQueue<DirectoryGatherMessage, u64>,

    thread_count: AtomicUsize,

    // output: (Sender<InventoryEntries>, Receiver<InventoryEntries>),

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
    pub fn new(min_blockcount: u64, num_threads: usize) -> io::Result<Arc<Inventory>> {
        let inventory = Arc::new(Inventory {
            entries: HashMap::new(),
            names: InternedNames::new(),
            dirs_queue: PriorityQueue::new(),
            thread_count: AtomicUsize::new(0),
            min_blockcount,
        });

        (0..num_threads).try_for_each(|_| -> io::Result<()> {
            inventory.clone().spawn_thread()?;
            Ok(())
        })?;

        Ok(inventory)
    }

    #[inline(always)]
    fn process_entry(
        &self,
        entry: io::Result<openat::Entry>,
        dir: Arc<Dir>,
        path: Arc<ObjectPath>,
    ) -> Result<(), Error> {
        // FIXME: when iterating at a certain depth (before number of file handles running
        // out) then dont keep sub dir handles open in an Arc, needs a different strategy
        // then. (break parent Dir, start with a fresh Dir handle)
        let entry = entry.context("Invalid directory entry")?;
        match entry.simple_type() {
            Some(SimpleType::Dir) => {
                trace!("dir: {:?}", path.to_pathbuf().join(entry.file_name()));
                let subdir = ObjectPath::subobject(path, self.names.interning(entry.file_name()));

                // The Order of directory traversal is defined by the 64bit priority in the
                // PriorityQueue. This 64bit are composed of the inode number added by high
                // 16bit part for the directory depth (inversed from u64::MAX down). This
                // results in that directories are traversed depth first in inode increasing order.
                let dir_prio = ((u16::MAX - subdir.depth()) as u64) << 48;
                let message = DirectoryGatherMessage::new_dir(subdir);

                self.dirs_queue
                    .send(message.with_parent(dir), dir_prio + entry.inode());
                Ok(()) // TODO: not returing anything?
            }
            // TODO: split here on simple-type
            _ => {
                // handle anything else
                let metadata = dir.metadata(entry.file_name()).with_context(|| {
                    format!(
                        "Failed get metadata on: {:?}",
                        path.to_pathbuf().join(entry.file_name())
                    )
                })?;

                trace!("file: {:?}", path.to_pathbuf().join(entry.file_name()));

                Ok(()) // TODO: not returing anything?
            }
        }
    }

    /// sends error to output channel and returns it
    fn send_error<T>(&self, err: Error) {
        error!("{:?}", err);
        // TODO: send error to output
    }

    fn spawn_thread(self: Arc<Self>) -> io::Result<thread::JoinHandle<()>> {
        thread::Builder::new()
            .name(format!(
                "inventory_{}",
                self.thread_count.fetch_add(1, atomic::Ordering::Relaxed)
            ))
            .spawn(move || {
                loop {
                    use DirectoryGatherMessage::*;

                    match self.dirs_queue.recv().entry() {
                        QueueEntry::Entry(TraverseDirectory { path, parent_dir }, _prio) => {
                            match &parent_dir {
                                Some(dir) => dir.sub_dir(path.name()),
                                None => openat::Dir::open(&path.to_pathbuf()),
                            }
                            .map(|dir| {
                                trace!(
                                    "opened fd {:?}: for {:?}: depth {}",
                                    dir,
                                    path.to_pathbuf(),
                                    path.depth()
                                );
                                let dir = Arc::new(dir);
                                dir.list_self()
                                    .map(|dir_iter| {
                                        dir_iter.for_each(|entry| {
                                            self.process_entry(entry, dir.clone(), path.clone())
                                                .context("Could not process entry")
                                                .map_err(|e| self.send_error::<()>(e))
                                                .ok();
                                        })
                                    })
                                    .map_err(|err| {
                                        error!("{:?}: {:?}", *dir, err);
                                        err
                                    })
                                    .with_context(|| {
                                        format!(
                                            "{:?}: Could not iterate {:?}",
                                            *dir,
                                            path.to_pathbuf()
                                        )
                                    })
                                    .map_err(|e| self.send_error::<()>(e))
                            })
                            .with_context(|| format!("Could not open: {:?}", path.to_pathbuf()))
                            .map_err(|e| self.send_error::<()>(e))
                            .ok();
                        }
                        QueueEntry::Drained => {
                            trace!("drained!!!");
                        }
                        _ => unreachable!(),
                    }
                }
            })
    }

    /// Adds a directory to the processing queue of the inventory.
    pub fn load_dir_recursive(&self, path: Arc<ObjectPath>) {
        self.dirs_queue.send(
            DirectoryGatherMessage::new_dir(path),
            0, // start message priority instead depth/inode calculation
        );
    }
}

#[derive(Debug)]
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

/// Messages on the input queue, directories to be processed.
#[derive(Debug)]
enum DirectoryGatherMessage {
    /// path and parent handle of a directory to be traversed. The handle to the directory
    /// itself will be opened by the thread processing it.
    TraverseDirectory {
        path:       Arc<ObjectPath>,
        parent_dir: Option<Arc<Dir>>,
    },
}

impl DirectoryGatherMessage {
    /// Create a new 'TraverseDirectory' message.
    pub fn new_dir(path: Arc<ObjectPath>) -> Self {
        DirectoryGatherMessage::TraverseDirectory {
            path,
            parent_dir: None,
        }
    }

    /// Attach a parent handle to a 'TraverseDirectory' message. Must not be used with other messages!
    pub fn with_parent(mut self, parent: Arc<Dir>) -> Self {
        debug_assert!(matches!(
            self,
            DirectoryGatherMessage::TraverseDirectory { .. }
        ));
        if let DirectoryGatherMessage::TraverseDirectory { parent_dir, .. } = &mut self {
            *parent_dir = Some(parent);
        };
        self
    }
}

/// Messages on the output queue, collected entries, 'Done' when the queue becomes empty and errors passed up
#[derive(Debug)]
enum InventoryEntriesMessage {
    InventoryEntry(InventoryKey, Arc<ObjectPath>),
    Err(Error),
    Done,
}

#[cfg(test)]
mod test {
    use crate::*;

    // tests
    #[test]
    fn smoke() {
        test::init_env_logging();
        let inventory = Inventory::new(64, 1);
    }

    #[test]
    #[ignore]
    fn load_dir() {
        test::init_env_logging();

        let inventory = Inventory::new(64, 1).unwrap();
        inventory.load_dir_recursive(ObjectPath::new("."));

        // FIXME: wait for threads
        std::thread::sleep(std::time::Duration::from_millis(10000000));
        // std::thread::sleep(std::time::Duration::from_millis(1000));
    }
}
