use diesel::insert_into;
use diesel::prelude::*;

use crate::db;
use crate::db::models::{Parameter, PendingCovenant};
use crate::db::schema::parameters::dsl::*;
use crate::db::schema::pending_covenants::dsl::*;

// TODO: how to clean up covenants in the database?

const BLOCK_HEIGHT_NAME: &str = "block_height";

pub fn upsert_block_height(con: db::Pool, height: u64) -> QueryResult<usize> {
    let values = Parameter {
        name: BLOCK_HEIGHT_NAME.to_string(),
        value: height.to_string(),
    };

    insert_into(parameters)
        .values(&values)
        .on_conflict(name)
        .do_update()
        .set(value.eq(height.to_string()))
        .execute(&mut con.get().unwrap())
}

pub fn get_block_height(con: db::Pool) -> Option<u64> {
    match parameters
        .select(Parameter::as_select())
        .filter(name.eq(BLOCK_HEIGHT_NAME))
        .load(&mut con.get().unwrap())
    {
        Ok(res) => {
            if res.len() == 0 {
                return None;
            }

            Some(res[0].value.parse::<u64>().unwrap())
        }
        Err(_) => None,
    }
}

pub fn insert_covenant(con: db::Pool, covenant: PendingCovenant) -> QueryResult<usize> {
    insert_into(pending_covenants)
        .values(&covenant)
        .execute(&mut con.get().unwrap())
}

pub fn get_covenant_for_output(con: db::Pool, script: &[u8]) -> Option<PendingCovenant> {
    match pending_covenants
        .select(PendingCovenant::as_select())
        .filter(output_script.eq(script))
        .limit(1)
        .load(&mut con.get().unwrap())
    {
        Ok(res) => {
            if res.len() == 0 {
                return None;
            }

            Some(res[0].clone())
        }
        Err(_) => None,
    }
}
