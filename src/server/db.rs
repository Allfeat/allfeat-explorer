//! Database plumbing — one place to embed the migration set and expose
//! the `Migrator` to bootstrap code and tests.
//!
//! Keeping the `sqlx::migrate!()` macro in a single spot means:
//!   * every caller runs the exact same migration bundle;
//!   * the sanity `#[test]` below forces the macro to expand on every
//!     build, so a malformed `migrations/*.sql` fails the lib build
//!     before it fails a deploy.

use sqlx::migrate::{MigrateError, Migrator};
use sqlx::PgPool;

/// Compiled migration set, read from `migrations/` at build time. Apply
/// with `MIGRATOR.run(&pool).await` once a `PgPool` is available.
pub static MIGRATOR: Migrator = sqlx::migrate!();

/// Apply every pending migration against `pool`, logging the outcome.
///
/// Called once at boot so operators don't need to run `sqlx migrate run`
/// out-of-band: the binary owns its schema and either reaches a known-good
/// state or refuses to start. Idempotent — `_sqlx_migrations` records
/// what's been applied, so a hot restart against an already-migrated DB
/// returns immediately.
pub async fn run_migrations(pool: &PgPool) -> Result<(), MigrateError> {
    let total = MIGRATOR.iter().count();
    tracing::info!(total, "migrations: applying pending set");
    MIGRATOR.run(pool).await?;
    tracing::info!(total, "migrations: schema up to date");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The macro validates the migrations directory at build time; this
    /// test makes the check observable as a runtime assertion too.
    #[test]
    fn migrator_is_non_empty() {
        assert!(
            MIGRATOR.iter().count() > 0,
            "migrations/ must contain at least the initial schema"
        );
        assert!(
            MIGRATOR.iter().any(|m| m.version == 1),
            "expected initial migration with version=1"
        );
    }
}
