use axum::async_trait;
use crossbeam_channel::Receiver;
use elements::{Block, Transaction};
use serde::Deserialize;
use std::error::Error;
use std::fmt;

#[derive(Debug)]
pub struct TransactionBroadcastError {
    pub err: Box<dyn Error>,
}

impl TransactionBroadcastError {
    pub fn is_already_included(&self) -> bool {
        matches!(
            format!("{}", self).as_str(),
            "Transaction already in block chain"
                | "bad-txns-inputs-missingorspent"
                | "insufficient fee, rejecting replacement"
        )
    }
}

impl fmt::Display for TransactionBroadcastError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.err)
    }
}

impl From<Box<dyn Error>> for TransactionBroadcastError {
    fn from(value: Box<dyn Error>) -> Self {
        TransactionBroadcastError { err: value }
    }
}

#[async_trait]
pub trait ChainBackend {
    async fn get_network_info(&self) -> Result<NetworkInfo, Box<dyn Error>>;
    async fn get_block_count(&self) -> Result<u64, Box<dyn Error>>;
    async fn get_block_hash(&self, height: u64) -> Result<String, Box<dyn Error>>;
    async fn get_block(&self, hash: String) -> Result<Block, Box<dyn Error>>;
    async fn send_raw_transaction(&self, hex: String) -> Result<String, TransactionBroadcastError>;
    async fn get_transaction(&self, hash: String) -> Result<Transaction, Box<dyn Error>>;

    fn get_tx_receiver(&self) -> Receiver<Transaction>;
    fn get_block_receiver(&self) -> Receiver<Block>;
}

#[derive(Debug, Clone, Deserialize)]
pub struct NetworkInfo {
    pub subversion: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ZmqNotification {
    #[serde(rename = "type")]
    pub notification_type: String,
    pub address: String,
}
