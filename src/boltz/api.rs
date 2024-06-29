use std::error::Error;

use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::boltz::types::{ErrorResponse, TransactionPostResponse};

#[derive(Debug, Clone)]
pub struct Client {
    endpoint: String,
}

impl Client {
    pub fn new(endpoint: String) -> Self {
        Client {
            endpoint: crate::utils::string::trim_suffix(endpoint, '/'),
        }
    }

    pub async fn send_raw_transaction(&self, hex: String) -> Result<String, Box<dyn Error>> {
        match self
            .send_request::<TransactionPostResponse>(
                "chain/L-BTC/transaction",
                json!({
                    "hex": hex,
                }),
            )
            .await
        {
            Ok(res) => Ok(res.id),
            Err(err) => Err(err),
        }
    }

    async fn send_request<T: DeserializeOwned>(
        &self,
        method: &str,
        data: Value,
    ) -> Result<T, Box<dyn Error>> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let client = reqwest::Client::new();

        let response = client
            .post(format!("{}/{}", self.endpoint, method))
            .headers(headers)
            .json(&data)
            .send()
            .await?;

        let res_body = response.bytes().await?;

        let err_res = serde_json::from_slice::<ErrorResponse>(res_body.as_ref())?;
        if err_res.error.is_some() {
            return Err(err_res.error.unwrap().into());
        }

        Ok(serde_json::from_slice::<T>(res_body.as_ref())?)
    }
}

#[cfg(test)]
mod client_test {
    use crate::boltz::api::Client;

    const ENDPOINT: &str = "https://api.testnet.boltz.exchange/v2";

    #[test]
    fn test_trim_suffix() {
        assert_eq!(Client::new(ENDPOINT.to_string() + "/").endpoint, ENDPOINT);
        assert_eq!(Client::new(ENDPOINT.to_string()).endpoint, ENDPOINT);
    }

    #[tokio::test]
    async fn test_send_raw_transaction_error_handling() {
        let client = Client::new(ENDPOINT.to_string());

        let res = client
            .send_raw_transaction("covclaim test".to_string())
            .await;
        assert!(res.err().is_some());
    }
}
