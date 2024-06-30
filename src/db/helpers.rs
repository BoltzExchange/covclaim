use diesel::internal::derives::multiconnection::chrono;
use diesel::prelude::*;
use diesel::{insert_into, update};

use crate::db;
use crate::db::models::{Parameter, PendingCovenant, PendingCovenantStatus};
use crate::db::schema::parameters;
use crate::db::schema::pending_covenants;

const BLOCK_HEIGHT_NAME: &str = "block_height";

pub fn upsert_block_height(con: db::Pool, height: u64) -> Result<(), diesel::result::Error> {
    let values = Parameter {
        name: BLOCK_HEIGHT_NAME.to_string(),
        value: height.to_string(),
    };

    match parameters::dsl::parameters
        .select(Parameter::as_select())
        .filter(parameters::dsl::name.eq(BLOCK_HEIGHT_NAME.to_string()))
        .limit(1)
        .load(&mut con.get().unwrap())
    {
        Ok(res) => {
            if res.is_empty() {
                match insert_into(parameters::dsl::parameters)
                    .values(&values)
                    .execute(&mut con.get().unwrap())
                {
                    Ok(_) => Ok(()),
                    Err(err) => Err(err),
                }
            } else {
                match update(parameters::dsl::parameters)
                    .filter(parameters::dsl::name.eq(BLOCK_HEIGHT_NAME.to_string()))
                    .set((parameters::dsl::value.eq(height.to_string()),))
                    .execute(&mut con.get().unwrap())
                {
                    Ok(_) => Ok(()),
                    Err(err) => Err(err),
                }
            }
        }
        Err(err) => Err(err),
    }
}

pub fn get_block_height(con: db::Pool) -> Option<u64> {
    match parameters::dsl::parameters
        .select(Parameter::as_select())
        .filter(parameters::dsl::name.eq(BLOCK_HEIGHT_NAME))
        .load(&mut con.get().unwrap())
    {
        Ok(res) => {
            if res.is_empty() {
                return None;
            }

            Some(res[0].value.parse::<u64>().unwrap())
        }
        Err(_) => None,
    }
}

pub fn insert_covenant(con: db::Pool, covenant: PendingCovenant) -> QueryResult<usize> {
    insert_into(pending_covenants::dsl::pending_covenants)
        .values(&covenant)
        .execute(&mut con.get().unwrap())
}

pub fn set_covenant_transaction(
    con: db::Pool,
    output_script: Vec<u8>,
    tx_id: Vec<u8>,
    time: chrono::NaiveDateTime,
) -> QueryResult<usize> {
    update(pending_covenants::dsl::pending_covenants)
        .filter(pending_covenants::dsl::output_script.eq(output_script))
        .set((
            pending_covenants::dsl::status.eq(PendingCovenantStatus::TransactionFound.to_int()),
            pending_covenants::dsl::tx_id.eq(tx_id),
            pending_covenants::dsl::tx_time.eq(time),
        ))
        .execute(&mut con.get().unwrap())
}

pub fn set_covenant_claimed(con: db::Pool, output_script: Vec<u8>) -> QueryResult<usize> {
    update(pending_covenants::dsl::pending_covenants)
        .filter(pending_covenants::dsl::output_script.eq(output_script))
        .set(pending_covenants::dsl::status.eq(PendingCovenantStatus::Claimed.to_int()))
        .execute(&mut con.get().unwrap())
}

pub fn get_covenants_to_claim(
    con: db::Pool,
    max_time: chrono::NaiveDateTime,
) -> QueryResult<Vec<PendingCovenant>> {
    pending_covenants::dsl::pending_covenants
        .select(PendingCovenant::as_select())
        .filter(pending_covenants::dsl::status.eq(PendingCovenantStatus::TransactionFound.to_int()))
        .filter(pending_covenants::dsl::tx_time.le(max_time))
        .load(&mut con.get().unwrap())
}

pub fn get_pending_covenant_for_output(con: db::Pool, script: &[u8]) -> Option<PendingCovenant> {
    match pending_covenants::dsl::pending_covenants
        .select(PendingCovenant::as_select())
        .filter(pending_covenants::dsl::output_script.eq(script))
        .filter(pending_covenants::dsl::status.eq(PendingCovenantStatus::Pending.to_int()))
        .limit(1)
        .load(&mut con.get().unwrap())
    {
        Ok(res) => {
            if res.is_empty() {
                return None;
            }

            Some(res[0].clone())
        }
        Err(_) => None,
    }
}
