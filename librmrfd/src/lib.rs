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

#[cfg(test)]
mod test {
    use std::{thread, time};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::io::Write;
    use std::sync::Once;

    use env_logger;

    use crate::*;

    pub fn init_env_logging() {
        static LOGGER: Once = Once::new();
        LOGGER.call_once(|| {
            let counter: AtomicU64 = AtomicU64::new(0);
            let seq_num = move || counter.fetch_add(1, Ordering::Relaxed);

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
        });
    }

    #[test]
    #[ignore]
    fn logger_speed() {
        test::init_env_logging();

        #[allow(unused_imports)]
        use log::{debug, error, info, trace, warn};

        let start = std::time::Instant::now();

        while start.elapsed() < std::time::Duration::from_secs(1) {
            error!("error");
            warn!("warn");
            info!("info");
            debug!("debug");
            trace!("trace");
        }
    }
}
