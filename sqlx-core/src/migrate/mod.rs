mod error;
#[allow(clippy::module_inception)]
mod migrate;
mod migration;
mod migration_type;
mod migrator;
mod source;
mod squash;

pub use error::MigrateError;
pub use migrate::{Migrate, MigrateDatabase};
pub use migration::{AppliedMigration, Migration};
pub use migration_type::MigrationType;
pub use migrator::Migrator;
pub use source::MigrationSource;
pub use squash::{format_squash_epoch_header, parse_squash_epoch};

#[doc(hidden)]
pub use source::resolve_blocking;
