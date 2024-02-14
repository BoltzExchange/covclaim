use std::error::Error;
use std::str::FromStr;
use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use elements::hashes::Hash;
use elements::secp256k1_zkp::SecretKey;
use elements::{hashes, Address};
use log::debug;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::api::types::RouterState;
use crate::claimer::tree::SwapTree;
use crate::db::helpers::insert_covenant;
use crate::db::models::PendingCovenant;

#[derive(Serialize, Deserialize)]
struct EmptyResponse {}

#[derive(Serialize, Deserialize)]
struct ErrorResponse {
    pub error: String,
}

#[derive(Deserialize)]
pub struct CovenantClaimRequest {
    #[serde(with = "hex::serde")]
    #[serde(rename = "internalKey")]
    pub internal_key: Vec<u8>,

    #[serde(with = "hex::serde")]
    pub preimage: Vec<u8>,

    #[serde(rename = "blindingKey")]
    pub blinding_key: Option<String>,

    pub address: String,
    pub tree: SwapTree,
}

#[derive(Serialize)]
enum CovenantClaimResponse {
    Error(ErrorResponse),
    Success(EmptyResponse),
}

impl IntoResponse for CovenantClaimResponse {
    fn into_response(self) -> axum::response::Response {
        match self {
            CovenantClaimResponse::Success(resp) => {
                (StatusCode::CREATED, Json(resp)).into_response()
            }
            CovenantClaimResponse::Error(err) => {
                (StatusCode::BAD_REQUEST, Json(err)).into_response()
            }
        }
    }
}

pub async fn post_covenant_claim(
    Extension(state): Extension<Arc<RouterState>>,
    Json(body): Json<CovenantClaimRequest>,
) -> impl IntoResponse {
    let address_script = match Address::from_str(body.address.as_str()) {
        Ok(res) => res,
        Err(err) => {
            return CovenantClaimResponse::Error(ErrorResponse {
                error: format!("could not parse address: {}", err.to_string()),
            })
        }
    };

    let blinding_key: Result<Option<Vec<u8>>, Box<dyn Error>> = match body.blinding_key {
        Some(res) => match hex::decode(res) {
            Ok(res) => match SecretKey::from_slice(res.as_slice()) {
                Ok(_) => Ok(Some(res)),
                Err(err) => Err(err.into()),
            },
            Err(err) => Err(err.into()),
        },
        None => Ok(None),
    };

    if blinding_key.is_err() {
        return CovenantClaimResponse::Error(ErrorResponse {
            error: format!(
                "could not parse blinding key: {}",
                blinding_key.err().unwrap().to_string()
            ),
        });
    }

    let covenant_details = match body.tree.clone().covenant_details() {
        Ok(res) => res,
        Err(err) => {
            return CovenantClaimResponse::Error(ErrorResponse {
                error: format!("could not parse swap tree: {}", err.to_string()),
            })
        }
    };

    let preimage_hash: hashes::hash160::Hash = Hash::hash(body.preimage.clone().as_ref());
    if Vec::from(preimage_hash.as_byte_array()) != covenant_details.preimage_hash {
        return CovenantClaimResponse::Error(ErrorResponse {
            error: "invalid preimage".to_string(),
        });
    }

    match insert_covenant(
        state.db.clone(),
        PendingCovenant {
            preimage: body.preimage,
            blinding_key: blinding_key.unwrap(),
            swap_tree: json!(body.tree).to_string(),
            internal_key: body.internal_key.clone(),
            address: elements::pset::serialize::Serialize::serialize(
                &address_script.script_pubkey(),
            ),
            output_script: elements::pset::serialize::Serialize::serialize(
                &body.tree.clone().address(body.internal_key).script_pubkey(),
            ),
        },
    ) {
        Ok(_) => {
            debug!("Inserted new covenant to claim");
            CovenantClaimResponse::Success(EmptyResponse {})
        }
        Err(e) => CovenantClaimResponse::Error(ErrorResponse {
            error: e.to_string(),
        }),
    }
}
