use std::io;
use std::fs;
use std::sync::Arc;
use std::ffi::OsStr;
use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;

use dirinventory::{
    openat, openat::metadata_types, Dir, DynResult, Gatherer, GathererBuilder, GathererHandle,
    ObjectPath,
};

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
        }
    }
}

impl RmrfdBuilder {
    /// How many InventoryEntries can be pending. The consumer that adds InventoryEntries to
    /// the InventoryGatherer should in most cases be much faster than the directory worker
    /// threads. Thus this number can be small.
    pub fn with_inventory_backlog(mut self, n: usize) -> Self {
        self.gatherer_builder = self.gatherer_builder.with_inventory_backlog(n);
        self
    }

    /// How many worker threads are used to gather the inventory
    pub fn with_inventory_threads(mut self, n: usize) -> Self {
        self.gatherer_builder = self.gatherer_builder.with_gather_threads(n);
        self
    }

    /// Filter for files only larger than these much (512 byte) blocks.
    pub fn with_min_blockcount(mut self, c: metadata_types::blksize_t) -> Self {
        self.min_blockcount = c;
        self
    }

    /// register rmrf directories that are watched for deleting entries.
    pub fn add_dir(mut self, dir: &OsStr) -> io::Result<Self> {
        let canonical_path = fs::canonicalize(dir)?;
        if !canonical_path.is_dir() {
            return Err(io::Error::from(io::ErrorKind::NotADirectory));
        }
        let dev = canonical_path.metadata()?.dev();
        self.rmrf_dirs.insert(ObjectPath::new(canonical_path), dev);
        Ok(self)
    }

    /// Creates and starts the Rmrfd.
    pub fn run(self) -> io::Result<Rmrfd> {
        let (inventory_gatherer, receiver) = self.gatherer_builder.start(Box::new(
            move |gatherer: GathererHandle,
                  entry: openat::Entry,
                  parent_path: Arc<ObjectPath>,
                  parent_dir: Arc<Dir>|
                  -> DynResult<()> {
                match entry.simple_type() {
                    // recurse subdirs
                    Some(openat::SimpleType::Dir) => {
                        gatherer.traverse_dir(&entry, parent_path.clone(), parent_dir.clone());
                        Ok(())
                    }
                    // everything else is eligible for deletion
                    _ => match parent_dir.metadata(entry.file_name()) {
                        Ok(metadata) => {
                            if metadata.size().unwrap_or(0) > self.min_blockcount {
                                gatherer.output_metadata(&entry, parent_path, metadata);
                            }
                            Ok(())
                        }
                        Err(e) => Err(Box::new(e)),
                    },
                }
            },
        ))?;

        Ok(Rmrfd {
            inventory_gatherer,
            rmrf_dirs: self.rmrf_dirs,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::{OsStr, OsString};

    use crate::Rmrfd;

    #[test]
    fn smoke() {
        crate::tests::init_env_logging();
        let rmrfd = Rmrfd::build()
            .add_dir(OsStr::new("../"))
            .unwrap()
            .with_min_blockcount(64)
            .run();
    }
}
