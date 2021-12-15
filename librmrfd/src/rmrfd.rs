use std::io;
use std::fs;
use std::sync::Arc;
use std::ffi::OsStr;
use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;

use crate::{DeviceId, Inventory, ObjectPath};

/// The daemon state
pub struct Rmrfd {
    inventory: Inventory,
    rmrf_dirs: HashMap<Arc<ObjectPath>, DeviceId>,
}

impl Rmrfd {
    /// Delegate construction to a builder
    #[must_use = "configure the builder and finally call build()"]
    pub fn new() -> RmrfdBuilder {
        RmrfdBuilder::default()
    }
}

/// Builder for constructing the daemon
pub struct RmrfdBuilder {
    min_blockcount:    u64,
    rmrf_dirs:         HashMap<Arc<ObjectPath>, DeviceId>,
    inventory_threads: usize,
}

impl Default for RmrfdBuilder {
    fn default() -> Self {
        RmrfdBuilder {
            min_blockcount:    0,
            rmrf_dirs:         HashMap::new(),
            inventory_threads: 1,
        }
    }
}

impl RmrfdBuilder {
    /// How many threads are used to gather the inventory
    pub fn with_inventory_threads(mut self, n: usize) -> Self {
        self.inventory_threads = n;
        self
    }

    /// Filter for files only larger than these much (512 byte) blocks.
    pub fn with_min_blockcount(mut self, c: u64) -> Self {
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

    /// Creates the Rmrfd.
    pub fn build(self) -> Rmrfd {
        Rmrfd {
            inventory: Inventory::new(self.min_blockcount),
            rmrf_dirs: self.rmrf_dirs,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::{OsStr, OsString};

    use crate::Rmrfd;

    #[test]
    fn smoke() {
        let rmrfd = Rmrfd::new()
            .add_dir(OsStr::new("../"))
            .unwrap()
            .with_min_blockcount(64)
            .build();
        //.unwrap();
    }
}
