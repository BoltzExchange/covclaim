use serde::Deserialize;

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
