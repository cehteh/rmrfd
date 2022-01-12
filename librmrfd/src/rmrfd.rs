use std::io;
use std::fs;
use std::sync::Arc;
use std::ffi::OsStr;
use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};
use dirinventory::{
    openat, openat::metadata_types, Dir, DynResult, Gatherer, GathererBuilder, GathererHandle,
    InternedName, ObjectPath, ProcessEntry,
};

use crate::inventory::{Inventory, ObjectKey};

/// The daemon state
pub struct Rmrfd {
    inventory_gatherer: Arc<Gatherer>,
    rmrf_dirs:          HashMap<Arc<ObjectPath>, metadata_types::dev_t>,
}

impl Rmrfd {
    /// Delegate construction to a builder
    #[must_use = "configure the builder and finally call build()"]
    pub fn build() -> RmrfdBuilder {
        RmrfdBuilder::default()
    }
}

/// Builder for constructing the daemon
pub struct RmrfdBuilder {
    gatherer_builder: GathererBuilder,
    min_blockcount:   metadata_types::blksize_t,
    rmrf_dirs:        HashMap<Arc<ObjectPath>, metadata_types::dev_t>,
    rmrf_armed:       bool,
}

impl Default for RmrfdBuilder {
    /// Create a RmrfdBuilder with reasonable defaults.
    fn default() -> Self {
        RmrfdBuilder {
            gatherer_builder: Gatherer::build(),
            /// Filter for files bigger than 32kb smaller ones would only bloat memory and
            /// give no much benefit when deleting in size order.
            min_blockcount:   64,
            rmrf_dirs:        HashMap::new(),
            rmrf_armed:       false,
        }
    }
}

impl RmrfdBuilder {
    /// How many InventoryEntries can be pending. The consumer that adds InventoryEntries to
    /// the InventoryGatherer should in most cases be much faster than the directory worker
    /// threads. Thus this number can be small.
    pub fn with_inventory_backlog(mut self, n: usize) -> Self {
        self.rmrf_armed = false;
        self.gatherer_builder = self.gatherer_builder.with_inventory_backlog(n);
        self
    }

    /// How many worker threads are used to gather the inventory.
    pub fn with_gather_threads(mut self, n: usize) -> Self {
        self.rmrf_armed = false;
        self.gatherer_builder = self.gatherer_builder.with_gather_threads(n);
        self
    }

    /// The number of threads the inventory uses to process entries.
    pub fn with_inventory_threads(mut self, n: usize) -> Self {
        self.rmrf_armed = false;
        self.gatherer_builder = self.gatherer_builder.with_output_channels(n);
        self
    }

    /// Filter for files only larger than these much (512 byte) blocks.
    pub fn with_min_blockcount(mut self, c: metadata_types::blksize_t) -> Self {
        self.rmrf_armed = false;
        self.min_blockcount = c;
        self
    }

    /// Safety switch, without arming nothing will be deleted, used for testing and do nothing
    /// options. Arming must be the last call before '.start()'.
    pub fn arm(mut self, state: bool) -> Self {
        self.rmrf_armed = state;
        self
    }

    /// register rmrf directories that are watched for deleting entries.
    pub fn add_dir(mut self, dir: &OsStr) -> io::Result<Self> {
        self.rmrf_armed = false;
        let canonical_path = fs::canonicalize(dir)?;
        if !canonical_path.is_dir() {
            return Err(io::Error::from(io::ErrorKind::NotADirectory));
        }
        let dev = canonical_path.metadata()?.dev();
        self.rmrf_dirs.insert(ObjectPath::new(canonical_path), dev);
        Ok(self)
    }

    /// Creates the Rmrfd and starts worker threads.
    pub fn start(self) -> io::Result<Rmrfd> {
        info!("armed: {}", self.rmrf_armed);
        let inventory_gatherer = self.gatherer_builder.start(Box::new(
            move |gatherer: GathererHandle, entry: ProcessEntry, parent_dir: Option<Arc<Dir>>| {
                match entry {
                    ProcessEntry::Result(Ok(entry), parent_path) => match entry.simple_type() {
                        Some(openat::SimpleType::Dir) => {
                            trace!(
                                "gather: subdir: {:?}",
                                parent_path
                                    .clone()
                                    .subobject(InternedName::new(entry.file_name()))
                            );
                            gatherer.traverse_dir(&entry, parent_path, parent_dir);
                        }
                        _ => match parent_dir.unwrap().metadata(entry.file_name()) {
                            Ok(metadata) => {
                                trace!(
                                    "gather: metadata: {:?}",
                                    parent_path
                                        .clone()
                                        .subobject(InternedName::new(entry.file_name()))
                                );
                                if metadata.size().unwrap_or(0) > self.min_blockcount {
                                    gatherer.output_metadata(
                                        ObjectKey::try_from(&metadata)
                                            .map_or(0, |key| key.bucket_hash()),
                                        &entry,
                                        parent_path,
                                        metadata,
                                    );
                                }
                            }
                            Err(err) => {
                                // FIXME: channel
                                gatherer.output_error(0, Box::new(err), parent_path);
                            }
                        },
                    },
                    ProcessEntry::Result(Err(err), parent_path) => {
                        // FIXME: channel
                        gatherer.output_error(0, Box::new(err), parent_path);
                    }
                    ProcessEntry::EndOfDirectory(_) => {}
                }
            },
        ))?;

        let inventory = Inventory::new(inventory_gatherer.channels_as_vec());

        // create fastrmrf instance
        // slowrmrf

        Ok(Rmrfd {
            inventory_gatherer,
            rmrf_dirs: self.rmrf_dirs,
        })
    }

    // directory watcher loop

    #[cfg(test)]
    pub fn delete_dir(&self, dir: ObjectPath) {
        // rmrfd
        //     .inventory_gatherer
        //     .load_dir_recursive(ObjectPath::new("/home/ct/rmrfd_test"));

        // wait for done
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::{OsStr, OsString};

    use crate::Rmrfd;
    use crate::rmrfd::ObjectPath;

    #[test]
    fn smoke() {
        crate::tests::init_env_logging();
        let rmrfd = Rmrfd::build()
            .with_min_blockcount(64)
            .with_inventory_threads(1)
            .start();
    }

    #[test]
    #[ignore]
    fn rmtest() {
        crate::tests::init_env_logging();
        let rmrfd = Rmrfd::build()
            .with_min_blockcount(1024)
            .with_inventory_threads(8)
            .start()
            .unwrap();

        // blocking things:

        // rmrfd.watch_dirs
        // rmrfd.delete_dir()

        rmrfd
            .inventory_gatherer
            .load_dir_recursive(ObjectPath::new("./"));

        std::thread::sleep(std::time::Duration::from_millis(1000));
    }
}
