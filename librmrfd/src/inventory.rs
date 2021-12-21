use std::io;
use std::fs::{read_dir, Metadata};
use std::os::unix::fs::MetadataExt;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::os::unix::fs::DirEntryExt2;
use std::cmp::Ordering;
use std::sync::{
    atomic::{self, AtomicUsize},
    Mutex, RwLock,
};

use easy_error::{bail, ensure, Error, ResultExt, Terminator};
use crossbeam::channel::{bounded, unbounded, Receiver, Sender};
use openat_ct as openat;
use openat::{Dir, SimpleType};
#[allow(unused_imports)]
pub use log::{debug, error, info, trace, warn};

use crate::{InternedNames, ObjectPath};

/// Slightly better name for device identifiers returned from metadata.
pub type DeviceId = u64;

/// All names linked to a single file on disk.
pub type ObjectList = Vec<std::sync::Arc<ObjectPath>>;

/// Create a space efficient store for file metadata of files larger than a certain
/// min_blocksize.  This is used to find whcih files to delete first for most space efficient
/// deletion.  There should be only one 'Inventory' around as it is used to merge hardlinks
/// and needs to have a global picture of all indexed files.
pub struct Inventory {
    entries: HashMap<DeviceId, BTreeMap<InventoryKey, ObjectList>>,
    names:   InternedNames,

    // thread management
    input: (
        Sender<DirectoryGatherMessage>,
        Receiver<DirectoryGatherMessage>,
    ),
    job_count:    AtomicUsize,
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
    pub fn new(min_blockcount: u64, num_threads: usize) -> Arc<Inventory> {
        let inventory = Arc::new(Inventory {
            entries: HashMap::new(),
            names: InternedNames::new(),
            input: unbounded(),
            job_count: AtomicUsize::new(0),
            thread_count: AtomicUsize::new(0),
            min_blockcount,
        });

        (0..num_threads).for_each(|_| {
            inventory.clone().spawn_thread();
        });

        inventory
    }

    #[inline(always)]
    fn process_entry(
        &self,
        entry: io::Result<openat::Entry>,
        dir: &Arc<Dir>,
        path: &Arc<ObjectPath>,
    ) -> Result<(), Error> {
        // FIXME: when iterating at a certain depth (before number of file handles running
        // out) then dont keep sub dir handles open in an Arc, needs a different strategy then.
        let entry = entry.context("Invalid directory entry")?;
        match entry.simple_type() {
            Some(SimpleType::Dir) => {
                let subdir =
                    ObjectPath::subobject(path.clone(), self.names.interning(entry.file_name()));

                let message = DirectoryGatherMessage::new_dir(subdir.clone());

                self.job_count.fetch_add(1, atomic::Ordering::Release);
                Ok(self
                    .input
                    .0
                    .send(message.with_parent(dir.clone()))
                    .with_context(|| {
                        format!("Failed to send message for: {:?}", subdir.to_pathbuf())
                    })
                    .map_err(|err| {
                        self.job_count.fetch_sub(1, atomic::Ordering::Release);
                        err
                    })?)
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

                Ok(()) //todo!()
            }
        }
    }

    /// sends error to output channel and returns it
    fn send_error<T>(&self, err: Error) -> Result<T, Error> {
        error!("{:?}", err);
        // TODO: send error to output
        Err(err)
    }

    fn spawn_thread(self: Arc<Self>) {
        thread::Builder::new()
            .name(format!(
                "inventory_{}",
                self.job_count.fetch_add(1, atomic::Ordering::Relaxed)
            ))
            .spawn(move || {
                loop {
                    let message = self.input.1.recv();

                    trace!("received message: {:?}", message);
                    use DirectoryGatherMessage::*;

                    match message
                        .context("Message receive error")
                        .map_err::<Result<DirectoryGatherMessage, Error>, _>(|e| self.send_error(e))
                        .ok()
                    {
                        Some(TraverseDirectory { path, parent_dir }) => {
                            match &parent_dir {
                                Some(dir) => dir.sub_dir(path.name()),
                                None => Dir::open(&path.to_pathbuf()),
                            }
                            .and_then(|dir| {
                                let dir = Arc::new(dir);
                                Ok(dir
                                    .list_self()
                                    .and_then(|dir_iter| {
                                        trace!("traverse dir: {:?}", path.to_pathbuf());
                                        Ok(dir_iter.for_each(|entry| {
                                            self.process_entry(entry, &dir, &path)
                                                .context("Could not process entry")
                                                .map_err(|e| self.send_error::<()>(e));
                                        }))
                                    })
                                    .with_context(|| {
                                        format!("Could not iterate: {:?}", path.to_pathbuf())
                                    })
                                    .map_err(|e| self.send_error::<()>(e.into())))
                            })
                            .with_context(|| format!("Could not open: {:?}", path.to_pathbuf()))
                            .map_err(|e| self.send_error::<()>(e.into()));

                            if self.job_count.fetch_sub(1, atomic::Ordering::Acquire) == 1 {
                                // TODO: send 'Done' message
                            }
                        }
                        None => { /* just drop the message, it was a Dud */ }
                    }
                }
            });
    }

    /// Adds a directory to the processing queue of the inventory.
    pub fn load_dir_recursive(&self, path: Arc<ObjectPath>) -> Result<(), Error> {
        self.job_count.fetch_add(1, atomic::Ordering::Release);
        Ok(self
            .input
            .0
            .send(DirectoryGatherMessage::new_dir(path))
            .context("Failed to send message")?)
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
    use std::{thread, time};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::io::Write;

    use env_logger;

    use crate::*;

    fn init_logging() {
        let counter: AtomicU64 = AtomicU64::new(0);
        let seq_num = move || counter.fetch_add(1, Ordering::SeqCst);

        let start = std::time::Instant::now();

        env_logger::Builder::from_default_env()
            .format(move |buf, record| {
                let micros = start.elapsed().as_micros() as u64;
                writeln!(
                    buf,
                    "{:0>12}: {:0>8}.{:0>6}: {:>5}: {}:{}: {}: {}",
                    seq_num(),
                    micros / 1000000,
                    micros % 1000000,
                    record.level().as_str(),
                    record.file().unwrap_or(""),
                    record.line().unwrap_or(0),
                    std::thread::current().name().unwrap_or("UNKNOWN"),
                    record.args()
                )
            })
            .try_init()
            .unwrap();
    }

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

        let inventory = Inventory::new(64, 8);
        inventory.load_dir_recursive(ObjectPath::new(".."));

        // FIXME: wait for threads
        thread::sleep(std::time::Duration::from_millis(1000));
    }
}
