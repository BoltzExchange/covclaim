use diesel::internal::derives::multiconnection::chrono::{TimeDelta, Utc};
use elements::bitcoin::Witness;
use elements::confidential::{Asset, AssetBlindingFactor, Nonce, Value, ValueBlindingFactor};
use elements::script::Builder;
use elements::secp256k1_zkp::rand::rngs::OsRng;
use elements::secp256k1_zkp::SecretKey;
use elements::{
    opcodes, AddressParams, LockTime, OutPoint, Script, Sequence, Transaction, TxIn, TxInWitness,
    TxOut, TxOutWitness,
};
use log::{debug, error, info, trace, warn};
use std::error::Error;
use std::ops::Sub;
use std::sync::Arc;
use tokio::time;

use crate::chain::types::ChainDataProvider;
use crate::claimer::tree::SwapTree;
use crate::db;
use crate::db::models::PendingCovenant;

#[derive(Clone)]
pub struct Constructor {
    db: db::Pool,
    chain_client: Arc<Box<dyn ChainDataProvider + Send + Sync>>,
    sweep_time: u64,
    sweep_interval: u64,
    address_params: &'static AddressParams,
}

impl Constructor {
    pub fn new(
        db: db::Pool,
        chain_client: Arc<Box<dyn ChainDataProvider + Send + Sync>>,
        sweep_time: u64,
        sweep_interval: u64,
        address_params: &'static AddressParams,
    ) -> Constructor {
        Constructor {
            db,
            sweep_time,
            chain_client,
            address_params,
            sweep_interval,
        }
    }

    pub async fn start_interval(self) {
        if self.clone().claim_instantly() {
            info!("Broadcasting sweeps instantly");
            return;
        }

        info!(
            "Broadcasting claims {} seconds after lockup transactions and checking on interval of {} seconds",
            self.sweep_time,
            self.sweep_interval
        );
        let mut interval = time::interval(time::Duration::from_secs(self.sweep_interval));

        self.clone().broadcast().await;

        loop {
            interval.tick().await;

            trace!("Checking for claims to broadcast");
            self.clone().broadcast().await;
        }
    }

    pub async fn schedule_broadcast(self, covenant: PendingCovenant, lockup_tx: Transaction) {
        if self.clone().claim_instantly() {
            self.broadcast_covenant(covenant, lockup_tx).await;
            return;
        }

        debug!(
            "Scheduling claim of {}",
            hex::encode(covenant.output_script.clone())
        );
        match db::helpers::set_covenant_transaction(
            self.db,
            covenant.output_script,
            hex::decode(lockup_tx.txid().to_string()).unwrap(),
            Utc::now().naive_utc(),
        ) {
            Ok(_) => {}
            Err(err) => {
                warn!("Could not schedule covenant claim: {}", err);
                return;
            }
        };
    }

    async fn broadcast(self) {
        let covenants = match db::helpers::get_covenants_to_claim(
            self.clone().db,
            Utc::now()
                .sub(TimeDelta::seconds(self.sweep_time as i64))
                .naive_utc(),
        ) {
            Ok(res) => res,
            Err(err) => {
                warn!("Could not fetch covenants to claim: {}", err);
                return;
            }
        };

        if covenants.len() == 0 {
            return;
        }

        debug!("Broadcasting {} claims", covenants.len());

        let self_clone = self.clone();
        for cov in covenants {
            let tx = match self_clone
                .clone()
                .chain_client
                .get_transaction(hex::encode(cov.tx_id.clone().unwrap()))
                .await
            {
                Ok(res) => res,
                Err(err) => {
                    error!(
                        "Could not fetch transaction for {}: {}",
                        hex::encode(cov.clone().output_script),
                        err
                    );
                    return;
                }
            };

            self_clone.clone().broadcast_covenant(cov, tx).await;
        }
    }

    async fn broadcast_covenant(self, cov: PendingCovenant, tx: Transaction) {
        match self.clone().broadcast_tx(cov.clone(), tx).await {
            Ok(tx) => {
                info!(
                    "Broadcast claim for {}: {}",
                    hex::encode(cov.clone().output_script),
                    tx.txid().to_string(),
                )
            }
            Err(err) => {
                error!(
                    "Could not broadcast claim for {}: {}",
                    hex::encode(cov.clone().output_script),
                    err
                )
            }
        }
    }

    async fn broadcast_tx(
        self,
        covenant: PendingCovenant,
        lockup_tx: Transaction,
    ) -> Result<Transaction, Box<dyn Error + Send + Sync>> {
        let tree = serde_json::from_str::<SwapTree>(covenant.swap_tree.as_str()).unwrap();
        let (prevout, vout) = match tree.clone().find_output(
            lockup_tx.clone(),
            covenant.clone().internal_key,
            self.address_params,
        ) {
            Some(res) => res,
            None => {
                return Err(format!(
                    "could not find swap output for {}",
                    hex::encode(covenant.output_script)
                )
                .into());
            }
        };

        debug!(
            "Broadcasting claim for: {}",
            hex::encode(covenant.clone().output_script)
        );

        let cov_details = tree.clone().covenant_details().unwrap();

        let mut witness = Witness::new();
        witness.push(covenant.clone().preimage);
        witness.push(Script::from(tree.clone().covenant_claim_leaf.output).as_bytes());
        witness.push(tree.control_block(covenant.clone().internal_key));

        let secp = &SwapTree::secp();

        let is_blinded = prevout.asset.is_confidential() && prevout.value.is_confidential();
        let tx_secrets = match is_blinded {
            true => match prevout.unblind(
                &secp,
                match SecretKey::from_slice(
                    match covenant.clone().blinding_key {
                        Some(res) => res,
                        None => return Err("no blinding key for blinded swap".into()),
                    }
                    .as_slice(),
                ) {
                    Ok(res) => res,
                    Err(err) => return Err(err.into()),
                },
            ) {
                Ok(res) => Some(res),
                Err(err) => return Err(err.into()),
            },
            false => None,
        };
        let utxo_value = match tx_secrets {
            // Leave 1 sat for a blinded OP_RETURN
            Some(secrets) => secrets.value - 1,
            None => prevout.value.explicit().unwrap(),
        };
        let utxo_asset = match tx_secrets {
            Some(secrets) => secrets.asset,
            None => prevout.asset.explicit().unwrap(),
        };

        let mut outs = Vec::<TxOut>::new();
        outs.push(TxOut {
            nonce: Nonce::Null,
            asset: Asset::Explicit(utxo_asset),
            value: Value::Explicit(cov_details.expected_amount),
            script_pubkey: Script::from(covenant.clone().address),
            witness: TxOutWitness {
                rangeproof: None,
                surjection_proof: None,
            },
        });

        if is_blinded {
            let mut rng = OsRng::default();

            let op_return_script = Builder::new()
                .push_opcode(opcodes::all::OP_RETURN)
                .into_script();

            let out_abf = AssetBlindingFactor::new(&mut rng);
            let (blinded_asset, surjection_proof) = Asset::Explicit(utxo_asset).blind(
                &mut rng,
                &secp,
                out_abf,
                &[tx_secrets.unwrap()],
            )?;

            let final_vbf = ValueBlindingFactor::last(
                &secp,
                1,
                out_abf,
                &[(
                    tx_secrets.unwrap().value,
                    tx_secrets.unwrap().asset_bf,
                    tx_secrets.unwrap().value_bf,
                )],
                &[
                    (
                        cov_details.expected_amount,
                        AssetBlindingFactor::zero(),
                        ValueBlindingFactor::zero(),
                    ),
                    (
                        utxo_value - cov_details.expected_amount,
                        AssetBlindingFactor::zero(),
                        ValueBlindingFactor::zero(),
                    ),
                ],
            );
            let (blinded_value, nonce, rangeproof) = Value::Explicit(1).blind(
                &secp,
                final_vbf,
                SecretKey::new(&mut rng).public_key(&secp),
                SecretKey::new(&mut rng),
                &op_return_script.clone(),
                &elements::RangeProofMessage {
                    asset: utxo_asset,
                    bf: out_abf,
                },
            )?;

            outs.push(TxOut {
                nonce,
                value: blinded_value,
                asset: blinded_asset,
                script_pubkey: op_return_script,
                witness: TxOutWitness {
                    rangeproof: Some(Box::new(rangeproof)),
                    surjection_proof: Some(Box::new(surjection_proof)),
                },
            });
        }

        outs.push(TxOut::new_fee(
            utxo_value - cov_details.expected_amount,
            utxo_asset,
        ));

        let tx = Transaction {
            version: 2,
            lock_time: LockTime::from_consensus(0),
            input: vec![TxIn {
                previous_output: OutPoint {
                    vout,
                    txid: lockup_tx.txid(),
                },
                is_pegin: false,
                script_sig: Default::default(),
                sequence: Sequence::from_consensus(0xFFFFFFFD),
                witness: TxInWitness {
                    pegin_witness: vec![],
                    amount_rangeproof: None,
                    inflation_keys_rangeproof: None,
                    script_witness: witness.to_vec(),
                },
                asset_issuance: Default::default(),
            }],
            output: outs,
        };

        let tx_hex = hex::encode(elements::pset::serialize::Serialize::serialize(&tx));
        trace!("Broadcasting transaction {}", tx_hex);

        match self.chain_client.send_raw_transaction(tx_hex).await {
            Ok(_) => match db::helpers::set_covenant_claimed(self.db, covenant.output_script) {
                Ok(_) => Ok(tx),
                Err(err) => Err(Box::new(err)),
            },
            Err(err) => {
                let err_str = err.to_string();

                if err_str.starts_with("insufficient fee, rejecting replacement")
                    || err_str.starts_with("bad-txns-inputs-missingorspent")
                {
                    Ok(tx)
                } else {
                    Err(err.to_string().into())
                }
            }
        }
    }

    fn claim_instantly(self) -> bool {
        self.sweep_interval == 0
    }
}
