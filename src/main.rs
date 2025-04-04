use std::env;
use std::sync::Arc;

use crate::chain::esplora::EsploraClient;
use crate::chain::types::ChainBackend;
use crate::kafka::KafkaClient;
use dotenvy::dotenv;
use elements::AddressParams;
use log::{debug, error, info};

mod api;
mod boltz;
mod chain;
mod claimer;
mod db;
mod kafka;
mod utils;

pub mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

#[tokio::main]
async fn main() {
    match dotenv() {
        Ok(_) => {}
        Err(err) => println!("Could not read .env file: {}", err),
    };
    env_logger::init();

    info!(
        "Starting {} v{}-{}{}",
        built_info::PKG_NAME,
        built_info::PKG_VERSION,
        built_info::GIT_VERSION.unwrap_or(""),
        if built_info::GIT_DIRTY.unwrap_or(false) {
            "-dirty"
        } else {
            ""
        }
    );

    debug!(
        "Compiled {} with {} for {}",
        built_info::PROFILE,
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

    // Initialize Kafka client
    let kafka_client = match KafkaClient::new(
        &env::var("KAFKA_BROKERS").unwrap_or_else(|_| "localhost:9092".to_string()),
        &env::var("KAFKA_TOPIC").unwrap_or_else(|_| "covenant_claims".to_string()),
        env::var("KAFKA_USERNAME").ok().as_deref(),
        env::var("KAFKA_PASSWORD").ok().as_deref(),
    ) {
        Ok(client) => {
            info!("Connected to Kafka");
            Some(client)
        }
        Err(err) => {
            error!("Could not connect to Kafka: {}", err);
            None
        }
    };

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
        kafka_client,
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
                    .expect("ELEMENTS_PORT must be set")
                    .parse::<u32>()
                    .expect("ELEMENTS_PORT invalid"),
                None,
                Some(env::var("ELEMENTS_USER").expect("ELEMENTS_USER must be set")),
                Some(env::var("ELEMENTS_PASSWORD").expect("ELEMENTS_PASSWORD must be set")),
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
            match EsploraClient::new(
                env::var("ESPLORA_ENDPOINT").expect("ESPLORA_ENDPOINT must be set"),
                env::var("ESPLORA_POLL_INTERVAL")
                    .expect("ESPLORA_POLL_INTERVAL must be set")
                    .parse::<u64>()
                    .expect("ESPLORA_POLL_INTERVAL invalid"),
                env::var("ESPLORA_MAX_REQUESTS_PER_SECOND")
                    .expect("ESPLORA_MAX_REQUESTS_PER_SECOND must be set")
                    .parse::<u64>()
                    .expect("ESPLORA_MAX_REQUESTS_PER_SECOND invalid"),
                env::var("BOLTZ_ENDPOINT").expect("BOLTZ_ENDPOINT must be set"),
            ) {
                Ok(client) => {
                    client.connect();
                    Box::new(client)
                }
                Err(err) => {
                    error!("Could not create Esplora client: {}", err);
                    std::process::exit(1);
                }
            }
        }
        &_ => {
            error!("Unknown chain backend: {}", backend);
            std::process::exit(1);
        }
    };

    Arc::new(client)
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
