use std::env;

use dotenvy::dotenv;
use elements::AddressParams;
use log::{debug, error, info};

mod api;
mod chain;
mod claimer;
mod db;

#[tokio::main]
async fn main() {
    dotenv().expect("could not read env file");
    env_logger::init();

    let network_params = get_address_params();

    let db = match db::establish_connection(
        env::var("DATABASE_URL")
            .expect("DATABASE_URL must be set")
            .as_str(),
    ) {
        Ok(res) => res,
        Err(err) => {
            error!("Could not connect to database: {}", err);
            std::process::exit(1);
        }
    };
    info!("Connected to database");

    let elements = match chain::client::ChainClient::new(
        env::var("ELEMENTS_HOST").expect("ELEMENTS_HOST must be set"),
        env::var("ELEMENTS_PORT")
            .expect("ELEMENTS_PORT must be est")
            .parse::<u32>()
            .expect("ELEMENTS_PORT invalid"),
        env::var("ELEMENTS_COOKIE").expect("ELEMENTS_COOKIE must be set"),
    )
    .connect()
    .await
    {
        Ok(client) => client,
        Err(err) => {
            error!("Could not connect to Elements client: {}", err);
            std::process::exit(1);
        }
    };

    let connect_res = match elements.clone().get_network_info().await {
        Ok(res) => res,
        Err(err) => {
            error!("Could not connect to Elements: {}", err);
            std::process::exit(1);
        }
    };

    info!("Connected to Elements: {}", connect_res.subversion);

    let claimer = claimer::Claimer::new(
        db.clone(),
        elements,
        env::var("SWEEP_TIME")
            .expect("SWEEP_TIME must be set")
            .parse::<u64>()
            .expect("SWEEP_TIME invalid"),
        env::var("SWEEP_INTERVAL")
            .expect("SWEEP_INTERVAL must be set")
            .parse::<u64>()
            .expect("SWEEP_INTERVAL invalid"),
        network_params,
    );
    claimer.start();

    let server_host = env::var("API_HOST").expect("API_HOST must be set");
    let server_port = env::var("API_PORT")
        .expect("API_PORT must be set")
        .parse::<u32>()
        .expect("API_PORT invalid");

    let server = api::server::start_server(db, network_params, server_host.as_str(), server_port);
    info!("Started API server on: {}:{}", server_host, server_port);

    server.await.unwrap().expect("could not start server");
}

fn get_address_params() -> &'static AddressParams {
    let network = env::var("NETWORK").expect("NETWORK must be set");
    debug!("Using network: {network}");

    match network.as_str() {
        "mainnet" => &AddressParams::LIQUID,
        "testnet" => &AddressParams::LIQUID_TESTNET,
        "regtest" => &AddressParams::ELEMENTS,
        &_ => {
            error!("Could not parse network: {}", network);
            std::process::exit(1);
        }
    }
}
