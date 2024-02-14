use crate::db::Pool;
use elements::AddressParams;

pub struct RouterState {
    pub db: Pool,
    pub address_params: &'static AddressParams,
}
