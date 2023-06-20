mod volume;

use redb::Database;
pub use volume::*;

use crate::config::CONFIG;

lazy_static::lazy_static! {
    pub static ref DATABASE: Database = {
        Database::create(&CONFIG.database).expect("failed to load database")
    };
}
