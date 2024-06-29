use std::error::Error;
use std::str::FromStr;
use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use elements::hashes::Hash;
use elements::secp256k1_zkp::{MusigKeyAggCache, PublicKey, SecretKey};
use elements::{hashes, Address, AddressParams};
use log::debug;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::api::types::RouterState;
use crate::claimer::tree::SwapTree;
use crate::db::helpers::insert_covenant;
use crate::db::models::{PendingCovenant, PendingCovenantStatus};

#[derive(Clone, Serialize, Deserialize)]
struct EmptyResponse {}

#[derive(Clone, Serialize, Deserialize)]
struct ErrorResponse {
    pub error: String,
}

#[derive(Deserialize)]
pub struct CovenantClaimRequest {
    #[serde(with = "hex::serde")]
    #[serde(rename = "claimPublicKey")]
    pub claim_public_key: Vec<u8>,

    #[serde(with = "hex::serde")]
    #[serde(rename = "refundPublicKey")]
    pub refund_public_key: Vec<u8>,

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
    let address = match parse_address(state.address_params, body.address) {
        Ok(addr) => addr,
        Err(err) => return CovenantClaimResponse::Error(err),
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

    let aggregate = MusigKeyAggCache::new(
        &SwapTree::secp(),
        &[
            match PublicKey::from_slice(body.refund_public_key.as_ref()) {
                Ok(res) => res,
                Err(err) => {
                    return CovenantClaimResponse::Error(ErrorResponse {
                        error: format!("could not parse refundPublicKey: {}", err.to_string()),
                    })
                }
            },
            match PublicKey::from_slice(body.claim_public_key.as_ref()) {
                Ok(res) => res,
                Err(err) => {
                    return CovenantClaimResponse::Error(ErrorResponse {
                        error: format!("could not parse claimPublicKey: {}", err.to_string()),
                    })
                }
            },
        ],
    );
    let internal_key = Vec::from(aggregate.agg_pk().serialize());

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
            internal_key: internal_key.clone(),
            status: PendingCovenantStatus::Pending.to_int(),
            address: elements::pset::serialize::Serialize::serialize(&address.script_pubkey()),
            output_script: elements::pset::serialize::Serialize::serialize(
                &body
                    .tree
                    .clone()
                    .address(internal_key, &state.address_params)
                    .script_pubkey(),
            ),
            tx_id: None,
            tx_time: None,
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

fn parse_address(
    params: &'static AddressParams,
    address: String,
) -> Result<Address, ErrorResponse> {
    let address = match Address::from_str(address.as_str()) {
        Ok(res) => res,
        Err(err) => {
            return Err(ErrorResponse {
                error: format!("could not parse address: {}", err.to_string()),
            });
        }
    };
    if address.params != params {
        return Err(ErrorResponse {
            error: "address has invalid network".to_string(),
        });
    }

    Ok(address)
}

#[cfg(test)]
mod parse_address_test {
    use crate::api::routes::parse_address;

    macro_rules! address_tests {
        ($($name:ident: $value:expr,)*) => {
            $(
                #[test]
                fn $name() {
                    let (params, address) = $value;
                    let res = parse_address(params, address);
                    assert!(res.is_ok());
                }
            )*
        }
    }

    address_tests! {
        address_regtest_blech32: (&elements::AddressParams::ELEMENTS, "el1qq2kqp5kej5gjfh24scawfxpl3uju5fr9tqv6f78sjauxlakgq8musy544qwhad34768q5t0ppmzr0z9wyn70cx5fkee9lzv4j".to_string()),
        address_regtest_bech32: (&elements::AddressParams::ELEMENTS, "ert1qpf0c8tqm70908xalp9jh4275etnq5lgnet663j".to_string()),
        address_regtest_p2sh_segwit: (&elements::AddressParams::ELEMENTS, "AzpqTBkNPY3XMEzeJTGP6zHjhBU811mAr9QLXJXFBsEaHNztc3zU1f7q2Hb6gsAVaRBKszTqTsgRDooT".to_string()),
        address_regtest_p2sh_segwit_unblinded: (&elements::AddressParams::ELEMENTS, "XCwwMo8ZF6aFssQLkaxpKQ3bpVNkQV8DFZ".to_string()),
        address_regtest_p2pkh: (&elements::AddressParams::ELEMENTS, "CTEuCmh6NCYTpevF6xzneb9M6TvkgdWrKf1r5RxToWjks1s4J94jaFvBnmQgEFZfgLMdZZjUNmJx8v5o".to_string()),
        address_regtest_p2pkh_unblinded: (&elements::AddressParams::ELEMENTS, "2dosihHMFweo6wWaHkXeZ5mMNmp8U4Nkt9B".to_string()),
        address_regtest_blech32_testnet: (&elements::AddressParams::LIQUID_TESTNET, "tlq1qqtfnwp6xac7lxa7nc7s972qm7yjptk46ncwhpymy5303a5pff7peeq5a3s5j4xfgydd3hdns4addsepu0psrk3ewf8ldskw47".to_string()),
        address_regtest_blech32_mainnet: (&elements::AddressParams::LIQUID, "lq1qq0qeqmfff6rcp38qd5muf22fsej5k0e0aaq2y4mv0j4h4sql5ejyxgd4ycpcg0x0f6snpeam4r7nxeyrattgudvyckw36lvkw".to_string()),
    }

    #[test]
    fn test_parse_address_invalid() {
        let res = parse_address(&elements::AddressParams::ELEMENTS, "not valid".to_string());
        assert!(res.clone().err().is_some());
        assert_eq!(
            res.err().unwrap().error,
            "could not parse address: base58 error: invalid base58 character 0x20"
        );
    }

    #[test]
    fn test_parse_address_invalid_network() {
        let res = parse_address(&elements::AddressParams::LIQUID, "el1qq2kqp5kej5gjfh24scawfxpl3uju5fr9tqv6f78sjauxlakgq8musy544qwhad34768q5t0ppmzr0z9wyn70cx5fkee9lzv4j".to_string());
        assert!(res.clone().err().is_some());
        assert_eq!(res.err().unwrap().error, "address has invalid network");
    }
}
