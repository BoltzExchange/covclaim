use std::env;
use std::sync::Arc;

use dotenvy::dotenv;
use elements::AddressParams;
use log::{debug, error, info};

use crate::chain::types::ChainBackend;

mod api;
mod chain;
mod claimer;
mod db;

pub mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

#[tokio::main]
async fn main() {
    dotenv().expect("could not read env file");
    env_logger::init();

    info!(
        "Starting covclaim v{}-{}",
        built_info::PKG_VERSION,
        built_info::GIT_VERSION.unwrap_or("")
    );
    debug!(
        "Compiled with {} for {}",
        built_info::RUSTC_VERSION,
        built_info::TARGET
    );

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

    let elements = get_chain_backend().await;

    let connect_res = match elements.get_network_info().await {
        Ok(res) => res,
        Err(err) => {
            error!("Could not connect to chain backend: {}", err);
            std::process::exit(1);
        }
    };

    info!("Connected to chain backend: {}", connect_res.subversion);

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

async fn get_chain_backend() -> Arc<Box<dyn ChainBackend + Send + Sync>> {
    let backend = env::var("CHAIN_BACKEND").unwrap_or("elements".to_string());
    info!("Using {} chain backend", backend);
    let client: Box<dyn ChainBackend + Send + Sync> = match backend.as_str() {
        "elements" => {
            match chain::client::ChainClient::new(
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
                Ok(client) => Box::new(client),
                Err(err) => {
                    error!("Could not connect to Elements client: {}", err);
                    std::process::exit(1);
                }
            }
        }
        "esplora" => {
            let client = chain::esplora::EsploraClient::new(
                env::var("ESPLORA_ENDPOINT").expect("ESPLORA_ENDPOINT must be set"),
                env::var("ESPLORA_POLL_INTERVAL")
                    .expect("ESPLORA_POLL_INTERVAL must be set")
                    .parse::<u64>()
                    .expect("ESPLORA_POLL_INTERVAL invalid"),
            );
            client.connect();
            Box::new(client)
        }
        &_ => {
            error!("Unknown chain backend: {}", backend);
            std::process::exit(1);
        }
    };

    return Arc::new(client);
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
