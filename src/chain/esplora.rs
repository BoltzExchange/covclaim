use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use crate::boltz::api::Client;
use crate::chain::types::{ChainBackend, NetworkInfo};
use async_trait::async_trait;
use crossbeam_channel::{Receiver, Sender};
use elements::{Block, Transaction};
use log::{error, info, trace, warn};
use ratelimit::Ratelimiter;
use reqwest::Response;
use serde::de::DeserializeOwned;
use tokio::{task, time};

#[derive(Clone)]
pub struct EsploraClient {
    endpoint: String,
    poll_interval: u64,

    rate_limit: Option<Arc<Ratelimiter>>,

    // To keep the channel alive
    // This is just a stub; streaming transactions is not supported with Esplora
    #[allow(dead_code)]
    tx_sender: Sender<Transaction>,
    tx_receiver: Receiver<Transaction>,

    block_sender: Sender<Block>,
    block_receiver: Receiver<Block>,

    boltz_client: Option<Client>,
}

impl EsploraClient {
    pub fn new(
        endpoint: String,
        poll_interval: u64,
        max_reqs_per_second: u64,
        boltz_endpoint: String,
    ) -> Result<Self, Box<dyn Error>> {
        // TODO: also do in boltz
        let trimmed_endpoint = match endpoint.strip_suffix('/') {
            Some(s) => s.to_string(),
            None => endpoint,
        };

        let (tx_sender, tx_receiver) = crossbeam_channel::bounded::<Transaction>(1);
        let (block_sender, block_receiver) = crossbeam_channel::unbounded::<Block>();

        let rate_limit = if max_reqs_per_second > 0 {
            info!(
                "Rate limiting requests to {} requests/second",
                max_reqs_per_second
            );
            Some(Arc::new(
                Ratelimiter::builder(max_reqs_per_second, Duration::from_secs(1))
                    .max_tokens(max_reqs_per_second)
                    .build()?,
            ))
        } else {
            info!("Not rate limiting");
            None
        };

        let boltz_client = if !boltz_endpoint.is_empty() {
            info!("Broadcasting transactions with Boltz API");
            Some(Client::new(boltz_endpoint))
        } else {
            info!("Broadcasting transactions with Esplora");
            None
        };

        Ok(EsploraClient {
            tx_sender,
            rate_limit,
            tx_receiver,
            block_sender,
            boltz_client,
            poll_interval,
            block_receiver,
            endpoint: trimmed_endpoint,
        })
    }

    pub fn connect(&self) {
        let clone = self.clone();

        task::spawn(async move {
            info!(
                "Polling for new blocks every {} seconds",
                clone.poll_interval
            );
            let mut interval = time::interval(Duration::from_secs(clone.poll_interval));

            let mut last_known_block = match clone.get_block_count().await {
                Ok(res) => res,
                Err(err) => {
                    error!("Could not get latest block: {}", err);
                    return;
                }
            };

            loop {
                interval.tick().await;

                let latest_block = match clone.get_block_count().await {
                    Ok(res) => res,
                    Err(err) => {
                        warn!("Could not get latest block: {}", err);
                        continue;
                    }
                };

                for height in last_known_block + 1..latest_block + 1 {
                    let block_hash = match clone.get_block_hash(height).await {
                        Ok(hash) => hash,
                        Err(err) => {
                            warn!("Could not get block hash for height {}: {}", height, err);
                            continue;
                        }
                    };
                    let block = match clone.get_block(block_hash.clone()).await {
                        Ok(block) => block,
                        Err(err) => {
                            warn!("Could not get block with hash {}: {}", block_hash, err);
                            continue;
                        }
                    };
                    trace!(
                        "Got block {} ({})",
                        block.header.height,
                        block.header.block_hash()
                    );
                    match clone.block_sender.send(block) {
                        Ok(_) => {}
                        Err(err) => {
                            warn!("Could not send block update: {}", err);
                            continue;
                        }
                    };
                }

                last_known_block = latest_block;
            }
        });
    }

    async fn request<T: DeserializeOwned>(
        &self,
        is_post: bool,
        method: &str,
        body: Option<String>,
    ) -> Result<T, Box<dyn Error>> {
        Ok(self
            .send_request(is_post, method, body)
            .await?
            .json::<T>()
            .await?)
    }

    async fn request_string(
        &self,
        is_post: bool,
        method: &str,
        body: Option<String>,
    ) -> Result<String, Box<dyn Error>> {
        Ok(self
            .send_request(is_post, method, body)
            .await?
            .text()
            .await?)
    }

    async fn request_bytes(
        &self,
        is_post: bool,
        method: &str,
        body: Option<String>,
    ) -> Result<Vec<u8>, Box<dyn Error>> {
        Ok(self
            .send_request(is_post, method, body)
            .await?
            .bytes()
            .await?
            .to_vec())
    }

    async fn send_request(
        &self,
        is_post: bool,
        method: &str,
        body: Option<String>,
    ) -> Result<Response, reqwest::Error> {
        let url = format!("{}/{}", self.endpoint, method);

        let client = reqwest::Client::new();
        let mut req = match is_post {
            true => client.post(url),
            false => client.get(url),
        };

        if body.is_some() {
            req = req.body(body.unwrap())
        }

        self.wait_rate_limit();
        let res = req.send().await?;
        match res.error_for_status() {
            Ok(res) => Ok(res),
            Err(err) => Err(err),
        }
    }

    fn wait_rate_limit(&self) {
        if self.rate_limit.is_none() {
            return;
        }

        loop {
            match self.rate_limit.clone().unwrap().try_wait() {
                Ok(_) => {
                    break;
                }
                Err(time) => {
                    std::thread::sleep(time);
                }
            }
        }
    }
}

#[async_trait]
impl ChainBackend for EsploraClient {
    async fn get_network_info(&self) -> Result<NetworkInfo, Box<dyn Error>> {
        // Send some request to make the endpoint is valid
        self.get_block_count().await?;

        return Ok(NetworkInfo {
            subversion: "Esplora".to_string(),
        });
    }

    async fn get_block_count(&self) -> Result<u64, Box<dyn Error>> {
        self.request::<u64>(false, "blocks/tip/height", None).await
    }

    async fn get_block_hash(&self, height: u64) -> Result<String, Box<dyn Error>> {
        self.request_string(false, format!("block-height/{}", height).as_str(), None)
            .await
    }

    async fn get_block(&self, hash: String) -> Result<Block, Box<dyn Error>> {
        let block_hex = self
            .request_bytes(false, format!("block/{}/raw", hash).as_str(), None)
            .await?;
        Ok(elements::encode::deserialize(&block_hex)?)
    }

    async fn send_raw_transaction(&self, hex: String) -> Result<String, Box<dyn Error>> {
        if self.boltz_client.is_some() {
            self.boltz_client
                .clone()
                .unwrap()
                .send_raw_transaction(hex)
                .await
        } else {
            self.request_string(true, "tx", Some(hex)).await
        }
    }

    async fn get_transaction(&self, hash: String) -> Result<Transaction, Box<dyn Error>> {
        let tx_hex = self
            .request_bytes(false, format!("tx/{}/raw", hash).as_str(), None)
            .await?;
        Ok(elements::encode::deserialize(&tx_hex)?)
    }

    fn get_tx_receiver(&self) -> Receiver<Transaction> {
        self.tx_receiver.clone()
    }

    fn get_block_receiver(&self) -> Receiver<Block> {
        self.block_receiver.clone()
    }
}

#[cfg(test)]
mod esplora_client_test {
    use crate::chain::esplora::EsploraClient;
    use crate::chain::types::ChainBackend;

    const ENDPOINT: &str = "https://blockstream.info/liquid/api/";

    #[test]
    fn test_trim_suffix() {
        assert_eq!(
            EsploraClient::new(ENDPOINT.to_string(), 0, 0, "".to_string())
                .unwrap()
                .endpoint,
            "https://blockstream.info/liquid/api"
        );
        assert_eq!(
            EsploraClient::new(
                "https://blockstream.info/liquid/api".to_string(),
                0,
                0,
                "".to_string(),
            )
            .unwrap()
            .endpoint,
            "https://blockstream.info/liquid/api"
        );
    }

    #[tokio::test]
    async fn test_new() {
        let client = EsploraClient::new(ENDPOINT.to_string(), 0, 0, "".to_string()).unwrap();

        let info = client.get_network_info().await.unwrap();
        assert_eq!(info.subversion, "Esplora");
    }

    #[tokio::test]
    async fn test_get_block_count() {
        let client = EsploraClient::new(ENDPOINT.to_string(), 0, 0, "".to_string()).unwrap();

        let count = client.get_block_count().await.unwrap();
        assert!(count > 2920403);
    }

    #[tokio::test]
    async fn test_get_block_hash() {
        let client = EsploraClient::new(ENDPOINT.to_string(), 0, 0, "".to_string()).unwrap();

        let hash = client.get_block_hash(2920407).await.unwrap();
        assert_eq!(
            hash,
            "4a1f9addaea74ef4132005fa1411abebb3f968b8cb766af93547c77b6207d619"
        );
    }

    #[tokio::test]
    async fn test_get_block() {
        let client = EsploraClient::new(ENDPOINT.to_string(), 0, 0, "".to_string()).unwrap();

        let block_hash = "5510868513d80a64371cdedfef49327dc2cd452b32b93cbcd70ddeddcc7bef66";
        let block = client.get_block(block_hash.to_string()).await.unwrap();
        assert_eq!(block.block_hash().to_string(), block_hash);
        assert_eq!(block.txdata.len(), 3);
    }

    #[tokio::test]
    async fn test_get_transaction() {
        let client = EsploraClient::new(ENDPOINT.to_string(), 0, 0, "".to_string()).unwrap();

        let tx_hash = "dc2505641c10af5fe0ffd8f1bfc14e9608e73137009c69b6ee0d1fe8ce9784d6";
        let block = client.get_transaction(tx_hash.to_string()).await.unwrap();
        assert_eq!(block.txid().to_string(), tx_hash);
    }

    #[tokio::test]
    async fn test_get_transaction_not_found() {
        let client = EsploraClient::new(ENDPOINT.to_string(), 0, 0, "".to_string()).unwrap();

        let tx_hash = "not found";
        let block = client.get_transaction(tx_hash.to_string()).await;
        assert!(block.err().is_some());
    }
}
