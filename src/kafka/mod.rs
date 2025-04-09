use std::error::Error;
use std::time::Duration;
use rdkafka::config::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct ClaimMessage {
    pub swap_id: String,
    pub claim_tx_id: String,
    pub claim_tx_time: i64,
    pub message_id: String,
}

pub struct KafkaClient {
    producer: FutureProducer,
    topic: String,
}

impl KafkaClient {
    pub fn new(
        brokers: &str,
        topic: &str,
        username: Option<&str>,
        password: Option<&str>,
    ) -> Result<Self, Box<dyn Error>> {
        let mut config = ClientConfig::new();
        config.set("bootstrap.servers", brokers);
        config.set("message.timeout.ms", "5000");

        // Only set up SASL authentication if both username and password are provided
        if let (Some(username), Some(password)) = (username, password) {
            if !username.is_empty() && !password.is_empty() {
                config.set("security.protocol", "SASL_SSL");
                config.set("sasl.mechanisms", "PLAIN");
                config.set("sasl.username", username);
                config.set("sasl.password", password);
            }
        }

        let producer: FutureProducer = config.create()?;

        Ok(KafkaClient {
            producer,
            topic: topic.to_string(),
        })
    }

    pub async fn send_claim_message(
        &self,
        swap_id: String,
        claim_tx_id: String,
        claim_tx_time: i64,
    ) -> Result<(), Box<dyn Error>> {
        let message = ClaimMessage {
            swap_id,
            claim_tx_id,
            claim_tx_time,
            message_id: Uuid::new_v4().to_string(),
        };

        let json_message = serde_json::to_string(&message)?;

        log::info!("Sending message - swap_id: {}, claim_tx_id: {}, claim_tx_time: {}", message.swap_id, message.claim_tx_id, message.claim_tx_time);
        log::debug!("Sending JSON message: {}", json_message);
        
        let record = FutureRecord::to(&self.topic)
            .payload(&json_message)
            .key(&message.message_id);

        match self.producer.send(record, Duration::from_secs(0)).await {
            Ok(_) => {
                log::info!("Successfully sent claim message for swap {}", message.swap_id);
                Ok(())
            }
            Err((e, _)) => {
                log::error!("Failed to send claim message: {}", e);
                Err(Box::new(e))
            }
        }
    }
} 