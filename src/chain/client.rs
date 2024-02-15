use std::error::Error;
use std::fs;

use base64::prelude::*;
use crossbeam_channel::Receiver;
use elements::{Block, Transaction};
use log::{debug, trace};
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::json;

use crate::chain::types::{NetworkInfo, ZmqNotification};
use crate::chain::zmq::ZmqClient;

enum StringOrU64 {
    Str(String),
    Num(u64),
}

impl Serialize for StringOrU64 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match *self {
            StringOrU64::Str(ref s) => serializer.serialize_str(s),
            StringOrU64::Num(n) => serializer.serialize_u64(n),
        }
    }
}

#[derive(Deserialize)]
struct RpcError {
    message: String,
}

#[derive(Deserialize)]
struct RpcResponse<T> {
    result: Option<T>,
    error: Option<RpcError>,
}

#[derive(Clone)]
pub struct ChainClient {
    url: String,
    cookie_file_path: String,
    zmq_client: ZmqClient,

    cookie: Option<String>,
}

impl ChainClient {
    pub fn new(host: String, port: u32, cookie_file_path: String) -> ChainClient {
        let client = ChainClient {
            cookie_file_path,
            cookie: None,
            zmq_client: ZmqClient::new(),
            url: format!("http://{}:{}", host, port),
        };
        trace!("Using Elements endpoint: {}", client.url);

        return client;
    }

    pub async fn connect(mut self) -> Result<ChainClient, Box<dyn Error>> {
        let file = fs::read(self.cookie_file_path.clone())?;
        debug!("Read Elements cookie file: {}", self.cookie_file_path);
        self.cookie = Some(format!("Basic {}", BASE64_STANDARD.encode(file)));

        let notifications = self.clone().get_zmq_notifications().await?;

        match self.zmq_client.clone().connect(notifications).await {
            Some(e) => return Err(e),
            None => {}
        };

        Ok(self)
    }

    pub fn get_tx_receiver(self) -> Receiver<Transaction> {
        self.zmq_client.tx_receiver.clone()
    }

    pub fn get_block_receiver(self) -> Receiver<Block> {
        self.zmq_client.block_receiver.clone()
    }

    pub async fn get_block_count(self) -> Result<u64, Box<dyn Error>> {
        self.request::<u64>("getblockcount").await
    }

    pub async fn get_block_hash(self, height: u64) -> Result<String, Box<dyn Error>> {
        self.request_params::<String>("getblockhash", vec![height])
            .await
    }

    pub async fn get_block(self, hash: String) -> Result<Block, Box<dyn Error>> {
        let mut params = Vec::<StringOrU64>::new();
        params.push(StringOrU64::Str(hash));
        params.push(StringOrU64::Num(0));

        let block_hex = self.request_params::<String>("getblock", params).await?;

        Self::parse_hex(block_hex)
    }

    pub async fn get_transaction(self, hash: String) -> Result<Transaction, Box<dyn Error>> {
        let tx_hex = self
            .request_params::<String>("getrawtransaction", vec![hash])
            .await?;

        Self::parse_hex(tx_hex)
    }

    pub async fn get_network_info(self) -> Result<NetworkInfo, Box<dyn Error>> {
        self.request::<NetworkInfo>("getnetworkinfo").await
    }

    pub async fn get_zmq_notifications(self) -> Result<Vec<ZmqNotification>, Box<dyn Error>> {
        self.request::<Vec<ZmqNotification>>("getzmqnotifications")
            .await
    }

    pub async fn send_raw_transaction(self, hex: String) -> Result<String, Box<dyn Error>> {
        self.request_params::<String>("sendrawtransaction", vec![hex])
            .await
    }

    async fn request<T: DeserializeOwned>(self, method: &str) -> Result<T, Box<dyn Error>> {
        self.request_params(method, Vec::<String>::new()).await
    }

    async fn request_params<T: DeserializeOwned>(
        self,
        method: &str,
        params: Vec<impl Serialize>,
    ) -> Result<T, Box<dyn Error>> {
        if self.cookie.is_none() {
            return Err("client not connected".into());
        }

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            "Authorization",
            HeaderValue::from_str(self.cookie.unwrap().as_str())?,
        );

        let data = json!({
            "method": method,
            "params": params,
        });

        let client = reqwest::Client::new();

        let response = client
            .post(self.url)
            .headers(headers)
            .json(&data)
            .send()
            .await?;

        let res = response.json::<RpcResponse<T>>().await?;
        if res.error.is_some() {
            return Err(res.error.unwrap().message.into());
        }

        Ok(res.result.unwrap())
    }

    fn parse_hex<T: elements::encode::Decodable>(hex_str: String) -> Result<T, Box<dyn Error>> {
        match elements::encode::deserialize(
            match hex::decode(hex_str) {
                Ok(res) => res,
                Err(err) => return Err(Box::new(err)),
            }
            .as_ref(),
        ) {
            Ok(block) => Ok(block),
            Err(e) => Err(Box::new(e)),
        }
    }
}
