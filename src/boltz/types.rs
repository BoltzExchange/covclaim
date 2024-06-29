use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct ErrorResponse {
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TransactionPostResponse {
    pub id: String,
}
