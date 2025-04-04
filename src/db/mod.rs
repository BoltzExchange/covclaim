use std::error::Error;

use diesel::prelude::*;
use diesel::r2d2::ConnectionManager;
use diesel::Connection;
use diesel_migrations::{embed_migrations, EmbeddedMigrations, MigrationHarness};
use log::{debug, error, info, warn};

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
    info!("Attempting to connect to database with URL: {}", url);
    
    let is_postgres = is_postgres_connection_url(url);
    info!(
        "Using {} database",
        if is_postgres {
            "PostgreSQL"
        } else {
            "SQLite"
        }
    );

    debug!("Creating connection manager...");
    let manager = match ConnectionManager::new(url) {
        Ok(m) => m,
        Err(e) => {
            error!("Failed to create connection manager: {}", e);
            return Err(e.into());
        }
    };

    debug!("Building connection pool...");
    let pool = match Pool::builder().build(manager) {
        Ok(p) => p,
        Err(e) => {
            error!("Failed to build connection pool: {}", e);
            return Err(e.into());
        }
    };

    debug!("Running migrations...");
    if let Err(e) = run_migrations(is_postgres, &pool) {
        error!("Failed to run migrations: {}", e);
        return Err(e);
    }

    info!("Successfully connected to database");
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
