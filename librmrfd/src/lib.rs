//! Rust library to provide the functionality for the rmrfd

#![warn(missing_docs)]
#![warn(rustdoc::missing_crate_level_docs)]
#![feature(hash_set_entry)]
#![feature(dir_entry_ext2)]
#![feature(io_error_more)]

mod inventory;
pub use inventory::{DeviceId, Inventory, ObjectList};

mod objectpath;
pub use objectpath::ObjectPath;

mod internednames;
pub use internednames::{InternedName, InternedNames};

mod rmrfd;
pub use rmrfd::Rmrfd;

mod priority_queue;
pub use priority_queue::PriorityQueue;
