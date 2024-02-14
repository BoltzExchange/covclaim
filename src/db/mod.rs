use diesel::r2d2::ConnectionManager;
use diesel::SqliteConnection;

pub mod helpers;

pub mod models;
mod schema;

pub type DatabaseConnection = SqliteConnection;
pub type Pool = r2d2::Pool<ConnectionManager<DatabaseConnection>>;

pub fn establish_connection(url: &str) -> Result<Pool, r2d2::Error> {
    let manager = ConnectionManager::<DatabaseConnection>::new(url);
    Pool::builder().build(manager)
}
