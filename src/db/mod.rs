use std::error::Error;

use diesel::prelude::*;
use diesel::r2d2::ConnectionManager;
use diesel::Connection;
use diesel_migrations::{embed_migrations, EmbeddedMigrations, MigrationHarness};
use log::info;

pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!("./migrations");
pub const MIGRATIONS_POSTGRES: EmbeddedMigrations = embed_migrations!("./migrations_postgres");

pub mod helpers;

pub mod models;
mod schema;

#[derive(diesel::MultiConnection)]
pub enum AnyConnection {
    Postgresql(PgConnection),
    Sqlite(SqliteConnection),
}

pub type Pool = r2d2::Pool<ConnectionManager<AnyConnection>>;

pub fn establish_connection(url: &str) -> Result<Pool, Box<dyn Error + Send + Sync>> {
    info!(
        "Using {} database",
        if is_postgres_connection_url(url) {
            "PostgreSQL"
        } else {
            "SQLite"
        }
    );

    let manager = ConnectionManager::new(url);
    let pool = Pool::builder().build(manager)?;

    run_migrations(is_postgres_connection_url(url), &pool)?;

    Ok(pool)
}

fn run_migrations(
    is_postgres: bool,
    pool: &Pool,
) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
    let mut con = pool.get()?;
    con.run_pending_migrations(if is_postgres {
        MIGRATIONS_POSTGRES
    } else {
        MIGRATIONS
    })?;
    Ok(())
}

fn is_postgres_connection_url(url: &str) -> bool {
    url.starts_with("postgresql")
}
