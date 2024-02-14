use std::error::Error;

use elements::bitcoin::Witness;
use elements::confidential::{Asset, AssetBlindingFactor, Nonce, Value, ValueBlindingFactor};
use elements::script::Builder;
use elements::secp256k1_zkp::rand::rngs::OsRng;
use elements::secp256k1_zkp::SecretKey;
use elements::{
    opcodes, LockTime, OutPoint, Script, Sequence, Transaction, TxIn, TxInWitness, TxOut,
    TxOutWitness,
};
use log::{debug, trace};

use crate::chain::client::ChainClient;
use crate::claimer::tree::SwapTree;
use crate::db;
use crate::db::models::PendingCovenant;

#[derive(Clone)]
pub struct Constructor {
    db: db::Pool,
    chain_client: ChainClient,
}

impl Constructor {
    pub fn new(db: db::Pool, chain_client: ChainClient) -> Constructor {
        Constructor { db, chain_client }
    }

    pub async fn broadcast_claim(
        self,
        covenant: PendingCovenant,
        lockup_tx: Transaction,
        vout: u32,
        prevout: &TxOut,
    ) -> Result<Transaction, Box<dyn Error>> {
        debug!(
            "Broadcasting claim for: {}",
            hex::encode(covenant.clone().output_script)
        );

        let tree = serde_json::from_str::<SwapTree>(covenant.swap_tree.as_str()).unwrap();
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

        match self.chain_client.clone().send_raw_transaction(tx_hex).await {
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
                    Err(err)
                }
            }
        }
    }
}
