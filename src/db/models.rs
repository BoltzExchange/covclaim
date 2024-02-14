use diesel::prelude::*;

pub enum PendingCovenantStatus {
    Pending = 0,
    Claimed = 1,
}

impl PendingCovenantStatus {
    pub fn to_int(self) -> i32 {
        self as i32
    }
}

#[derive(Queryable, Selectable, Insertable, AsChangeset)]
#[diesel(table_name = crate::db::schema::parameters)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Parameter {
    pub name: String,
    pub value: String,
}

#[derive(Queryable, Selectable, Insertable, AsChangeset, Clone)]
#[diesel(table_name = crate::db::schema::pending_covenants)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct PendingCovenant {
    pub output_script: Vec<u8>,
    pub status: i32,
    pub internal_key: Vec<u8>,
    pub preimage: Vec<u8>,
    pub swap_tree: String,
    pub address: Vec<u8>,
    pub blinding_key: Option<Vec<u8>>,
}
