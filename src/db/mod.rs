use diesel::r2d2::ConnectionManager;
use diesel::sqlite::Sqlite;
use diesel::{Connection, SqliteConnection};
use diesel_migrations::{embed_migrations, EmbeddedMigrations, MigrationHarness};
use std::error::Error;

pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!("./migrations");

pub mod helpers;

pub mod models;
mod schema;

pub type DatabaseConnection = SqliteConnection;
pub type Pool = r2d2::Pool<ConnectionManager<DatabaseConnection>>;

pub fn establish_connection(url: &str) -> Result<Pool, Box<dyn Error + Send + Sync>> {
    run_migrations(&mut SqliteConnection::establish(url)?)?;

    let manager = ConnectionManager::<DatabaseConnection>::new(url);
    let pool = Pool::builder().build(manager)?;

    Ok(pool)
}

fn run_migrations(
    connection: &mut impl MigrationHarness<Sqlite>,
) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
    connection.run_pending_migrations(MIGRATIONS)?;
    Ok(())
}
