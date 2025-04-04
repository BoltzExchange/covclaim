use crossbeam_channel::Receiver;
use elements::{AddressParams, Transaction};
use log::{debug, error, info, trace, warn};
use rayon::iter::IntoParallelRefIterator;
use rayon::iter::ParallelIterator;
use std::cmp;
use std::error::Error;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::runtime::Builder;

use crate::chain::types::ChainBackend;
use crate::claimer::constructor::Constructor;
use crate::db;
use crate::db::helpers::get_pending_covenant_for_output;
use crate::kafka::KafkaClient;

pub mod constructor;
pub mod tree;

const MAX_PARALLEL_REQUESTS: usize = 15;

#[derive(Clone)]
pub struct Claimer {
    db: db::Pool,
    chain_client: Arc<Box<dyn ChainBackend + Send + Sync>>,
    constructor: Constructor,
}

impl Claimer {
    pub fn new(
        db: db::Pool,
        chain_client: Arc<Box<dyn ChainBackend + Send + Sync>>,
        sweep_time: u64,
        sweep_interval: u64,
        address_param: &'static AddressParams,
        kafka_client: Option<KafkaClient>,
    ) -> Claimer {
        Claimer {
            constructor: Constructor::new(
                db.clone(),
                chain_client.clone(),
                sweep_time,
                sweep_interval,
                address_param,
                kafka_client,
            ),
            db,
            chain_client,
        }
    }

    pub fn start(self) {
        debug!("Starting claimer");
        let constructor_clone = self.constructor.clone();
        tokio::spawn(async move {
            constructor_clone.start_interval().await;
        });

        let tx_clone = self.clone();
        let tx_receiver = self.clone().chain_client.get_tx_receiver();
        tokio::spawn(async move {
            loop {
                match tx_receiver.recv() {
                    Ok(tx) => {
                        tx_clone.clone().handle_tx(tx).await;
                    }
                    Err(e) => {
                        warn!("Could not read from transaction channel: {}", e);
                    }
                }
            }
        });

        let block_clone = self.clone();
        let block_receiver = self.clone().chain_client.get_block_receiver();
        tokio::spawn(async move {
            match self.rescan().await {
                Ok(height) => {
                    info!("Rescanned to height: {}", height);
                }
                Err(err) => {
                    error!("Rescanning failed: {}", err);
                }
            };

            loop {
                match block_receiver.recv() {
                    Ok(block) => {
                        for tx in block.txdata {
                            block_clone.clone().handle_tx(tx).await;
                        }

                        match db::helpers::upsert_block_height(
                            block_clone.clone().db,
                            block.header.height as u64,
                        ) {
                            Ok(_) => {
                                debug!(
                                    "Updated block height {} ({})",
                                    block.header.height,
                                    block.header.block_hash()
                                );
                            }
                            Err(err) => {
                                warn!("Could not update block height: {}", err);
                                continue;
                            }
                        };
                    }
                    Err(e) => {
                        warn!("Could not read from block channel: {}", e);
                    }
                }
            }
        });
    }

    async fn rescan(self) -> Result<u64, Box<dyn Error>> {
        let block_count = self.chain_client.get_block_count().await?;
        trace!("Current block height: {}", block_count);

        let rescan_height = match db::helpers::get_block_height(self.db.clone()) {
            Some(res) => res,
            None => {
                db::helpers::upsert_block_height(self.db, block_count)?;
                info!("No block height in database");
                debug!("Not rescanning");
                return Ok(block_count);
            }
        };

        info!("Found block height in database: {}", rescan_height);

        let block_range: Vec<u64> = (rescan_height..block_count + 1).collect();

        let (sender, receiver) = crossbeam_channel::bounded(block_range.len());
        for task in IntoIterator::into_iter(block_range.clone()) {
            sender.send(task).unwrap();
        }

        drop(sender);

        let rescan_threads = cmp::min(MAX_PARALLEL_REQUESTS, num_cpus::get());
        trace!("Rescanning with {} threads", rescan_threads);

        let runtime = Builder::new_multi_thread()
            .worker_threads(rescan_threads)
            .enable_all()
            .build()
            .unwrap();

        let processed_blocks = AtomicU64::new(0);
        let blocks_to_rescan = block_range.len();

        (0..rescan_threads)
            .map(|_| receiver.clone())
            .collect::<Vec<Receiver<u64>>>()
            .par_iter()
            .for_each(|receiver| {
                let self_clone = self.clone();

                while let Ok(height) = receiver.recv() {
                    let self_clone = self_clone.clone();
                    runtime.block_on(async move {
                        let block_hash = match self_clone.chain_client.get_block_hash(height).await
                        {
                            Ok(res) => res,
                            Err(err) => {
                                error!("Could not get block hash of {}: {}", height, err);
                                return;
                            }
                        };
                        let block =
                            match self_clone.chain_client.get_block(block_hash.clone()).await {
                                Ok(res) => res,
                                Err(err) => {
                                    error!("Could not get block {}: {}", block_hash, err);
                                    return;
                                }
                            };

                        debug!(
                            "Rescanning block {} ({}) with {} transactions",
                            block.header.height,
                            hex::encode(block.header.block_hash()),
                            block.txdata.len()
                        );

                        for tx in block.txdata {
                            self_clone.clone().handle_tx(tx).await;
                        }
                    });

                    let processed = processed_blocks.fetch_add(1, Ordering::SeqCst) + 1;

                    if processed % 10 == 0 {
                        let processed_perc = processed as f64 / blocks_to_rescan as f64;
                        info!("Rescan progress: {:.2}%", processed_perc * 100.0);
                    }
                }
            });

        runtime.shutdown_background();

        db::helpers::upsert_block_height(self.db, block_count)?;
        debug!("Finished rescanning");

        Ok(block_count)
    }

    async fn handle_tx(self, tx: Transaction) {
        trace!(
            "Checking {} outputs of transaction: {}",
            tx.output.len(),
            tx.txid().to_string()
        );

        for vout in 0..tx.output.len() {
            let out = &tx.output[vout];

            if let Some(covenant) =
                get_pending_covenant_for_output(self.db.clone(), out.script_pubkey.as_bytes())
            {
                info!(
                    "Found covenant {} to claim in {}:{}",
                    hex::encode(covenant.clone().output_script),
                    tx.txid().to_string(),
                    vout
                );

                self.clone()
                    .constructor
                    .schedule_broadcast(covenant, tx.clone())
                    .await;
            }
        }
    }
}
