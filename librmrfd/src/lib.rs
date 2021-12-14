//! Rust library to provide the functionality for the rmrfd

#![warn(missing_docs)]
#![warn(rustdoc::missing_crate_level_docs)]
#![feature(hash_set_entry)]
#![feature(dir_entry_ext2)]

mod inventory;
pub use inventory::Inventory;

mod objectpath;
pub use objectpath::ObjectPath;

mod internednames;
pub use internednames::{InternedName, InternedNames};

type ObjectList = Vec<std::sync::Arc<ObjectPath>>;

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        let result = 2 + 2;
        assert_eq!(result, 4);
    }
}
