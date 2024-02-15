use diesel::internal::derives::multiconnection::chrono;
use diesel::prelude::*;

pub enum PendingCovenantStatus {
    Pending = 0,
    TransactionFound = 1,
    Claimed = 2,
}

impl PendingCovenantStatus {
    pub fn to_int(self) -> i32 {
        self as i32
    }
}

#[derive(Queryable, Selectable, Insertable, AsChangeset)]
#[diesel(table_name = crate::db::schema::parameters)]
pub struct Parameter {
    pub name: String,
    pub value: String,
}

#[derive(Queryable, Selectable, Insertable, AsChangeset, Clone)]
#[diesel(table_name = crate::db::schema::pending_covenants)]
pub struct PendingCovenant {
    pub output_script: Vec<u8>,
    pub status: i32,
    pub internal_key: Vec<u8>,
    pub preimage: Vec<u8>,
    pub swap_tree: String,
    pub address: Vec<u8>,
    pub blinding_key: Option<Vec<u8>>,
    pub tx_id: Option<Vec<u8>>,
    pub tx_time: Option<chrono::NaiveDateTime>,
}
