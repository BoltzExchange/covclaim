use std::error::Error;

use elements::bitcoin::opcodes::all::OP_PUSHNUM_NEG1;
use elements::bitcoin::XOnlyPublicKey;
use elements::script::Instruction;
use elements::secp256k1_zkp::{All, Secp256k1};
use elements::taproot::{LeafVersion, TaprootBuilder};
use elements::{Address, AddressParams, Script, Transaction, TxOut};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Clone)]
pub struct TreeScript {
    #[serde(with = "hex::serde")]
    pub output: Vec<u8>,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct SwapTree {
    #[serde(rename = "claimLeaf")]
    pub claim_leaf: TreeScript,

    #[serde(rename = "refundLeaf")]
    pub refund_leaf: TreeScript,

    #[serde(rename = "covenantClaimLeaf")]
    pub covenant_claim_leaf: TreeScript,
}

#[derive(Debug)]
pub struct CovenantDetails {
    pub expected_amount: u64,
    pub expected_output: Vec<u8>,
    pub preimage_hash: Vec<u8>,
}

impl SwapTree {
    pub fn covenant_details(self) -> Result<CovenantDetails, Box<dyn Error>> {
        let claim_script = Script::from(self.covenant_claim_leaf.output);

        let mut details = CovenantDetails {
            expected_amount: 0,
            preimage_hash: Vec::new(),
            expected_output: Vec::new(),
        };

        let mut position = 0;

        for op in claim_script.instructions() {
            match op {
                Ok(instr) => match instr {
                    Instruction::PushBytes(data) => match position {
                        3 => details.preimage_hash = Vec::from(data),
                        6 => details.expected_output = Vec::from(data),
                        13 => {
                            if let Ok(array) = data.try_into() {
                                details.expected_amount = u64::from_le_bytes(array);
                            } else {
                                return Err("could not parse covenant output amount".into());
                            }
                        }
                        _ => {}
                    },
                    Instruction::Op(op) => {
                        // For SegWit addresses that is a push operation;
                        // we skip incrementing the counter so that we can use the same match statement
                        if op.into_u8() != OP_PUSHNUM_NEG1.to_u8() {
                            position += 1;
                        }
                    }
                },
                Err(err) => {
                    return Err(
                        format!("could not iterate over covenant claim script: {}", err).into(),
                    );
                }
            }
        }

        Ok(details)
    }

    pub fn address(self, internal_key: Vec<u8>, params: &'static AddressParams) -> Address {
        let key = Self::parse_key(internal_key);

        Address::p2tr(
            &Self::secp(),
            key,
            self.tree_builder()
                .finalize(&Self::secp(), key)
                .unwrap()
                .merkle_root(),
            None,
            params,
        )
    }

    pub fn find_output(
        self,
        lockup_tx: Transaction,
        internal_key: Vec<u8>,
        params: &'static AddressParams,
    ) -> Option<(TxOut, u32)> {
        let script_pubkey = self.address(internal_key, params).script_pubkey();

        for (vout, out) in (0_u32..).zip(lockup_tx.output.into_iter()) {
            if out.script_pubkey.eq(&script_pubkey) {
                return Some((out, vout));
            }
        }

        None
    }

    pub fn control_block(self, internal_key: Vec<u8>) -> Vec<u8> {
        let spend_info = self
            .clone()
            .tree_builder()
            .finalize(&Self::secp(), Self::parse_key(internal_key))
            .unwrap();

        spend_info
            .control_block(&(
                Script::from(self.covenant_claim_leaf.output),
                LeafVersion::default(),
            ))
            .unwrap()
            .serialize()
    }

    fn tree_builder(self) -> TaprootBuilder {
        TaprootBuilder::new()
            .add_leaf(1, Script::from(self.covenant_claim_leaf.output))
            .unwrap()
            .add_leaf(2, Script::from(self.claim_leaf.output))
            .unwrap()
            .add_leaf(2, Script::from(self.refund_leaf.output))
            .unwrap()
    }

    pub fn secp() -> Secp256k1<All> {
        Secp256k1::new()
    }

    fn parse_key(internal_key: Vec<u8>) -> XOnlyPublicKey {
        XOnlyPublicKey::from_slice(internal_key.as_slice()).unwrap()
    }
}

#[cfg(test)]
mod swap_tree_tests {
    use elements::pset::serialize::Serialize;
    use elements::AddressParams;

    use crate::claimer::tree::SwapTree;

    const INTERNAL_KEY: &str = "816963af90d4b882ccbcaacc920ba8e4fdd35c083a052a08d5c1732272ffccd8";

    const TREE_JSON: &str = "{
            \"claimLeaf\": {
                \"version\": 196,
                \"output\": \"82012088a914af8b5215948249f6e10adddc531ffe5d4428b9178820812910149e0e71209624487851f80a0cb97652efb0a836205628bc1b0e8e3aa7ac\"
            },
            \"refundLeaf\": {
                \"version\": 196,
                \"output\": \"201ec7adf6f1c40ad340533027d15952c0c5b7aa0dd6c4b38d838e62d32d4d0259ad020b06b1\"
            },
            \"covenantClaimLeaf\": {
                \"version\": 196,
                \"output\": \"82012088a914af8b5215948249f6e10adddc531ffe5d4428b9178800d1008814aff4f5af812e3db39024f2000db7e23091dc06038800ce51882025b251070e29ca19043cf33ccd7324e2ddab03ecc4ae0b5e77c4fc0e5cf6c95a8800cf7508a08601000000000087\"
            }
        }";

    #[test]
    fn parse_swap_tree() {
        let _: SwapTree = serde_json::from_str(TREE_JSON).unwrap();
    }

    #[test]
    fn address() {
        let internal_key = hex::decode(INTERNAL_KEY).unwrap();

        let address = serde_json::from_str::<SwapTree>(TREE_JSON)
            .unwrap()
            .address(internal_key, &AddressParams::ELEMENTS);

        assert_eq!(
            address.to_string(),
            "ert1pephte6qwvmhs74wp9aup4fs0syk6ed233sqtved7grk6qucedj0qksw749"
        );
        assert_eq!(
            hex::encode(address.script_pubkey().serialize()),
            "5120c86ebce80e66ef0f55c12f781aa60f812dacb5518c00b665be40eda073196c9e"
        );
    }

    #[test]
    fn covenant_details() {
        let swap: SwapTree = serde_json::from_str(TREE_JSON).unwrap();
        let details = swap.covenant_details().unwrap();

        assert_eq!(details.expected_amount, 100_000);
        assert_eq!(
            hex::encode(details.expected_output),
            "aff4f5af812e3db39024f2000db7e23091dc0603"
        );
        assert_eq!(
            hex::encode(details.preimage_hash),
            "af8b5215948249f6e10adddc531ffe5d4428b917"
        );
    }

    #[test]
    fn covenant_details_legacy() {
        let swap: SwapTree = serde_json::from_str("{
    \"claimLeaf\": {
      \"version\": 196,
      \"output\": \"82012088a9149eabdcb6a7e19a6a1cf082f8ef261d4c7ce6d25688204f3b8fed02c3eaf785bdcbc45e6e7a811e9062c6a681f1b3d0f51bd8c359206cac\"
    },
    \"refundLeaf\": {
      \"version\": 196,
      \"output\": \"203e2100f5b5f7100a972f21cd17526f3f79e157128323aa0ab124c1baa33f9ee6ad0372fd2ab1\"
    },
    \"covenantClaimLeaf\": {
      \"version\": 196,
      \"output\": \"82012088a9149eabdcb6a7e19a6a1cf082f8ef261d4c7ce6d2568800d14f8820b80f397fe1edcb87e54ce9cd5b4a5896b19e7d577b3bb868c4eb7ff1c3a5bb938800ce5188206d521c38ec1ea15734ae22b7c46064412829c0d0579f0a713d1c04ede979026f8800cf7508542500000000000087\"
    }
  }").unwrap();
        let details = swap.covenant_details().unwrap();

        assert_eq!(details.expected_amount, 9556);
        assert_eq!(
            hex::encode(details.expected_output),
            "b80f397fe1edcb87e54ce9cd5b4a5896b19e7d577b3bb868c4eb7ff1c3a5bb93"
        );
        assert_eq!(
            hex::encode(details.preimage_hash),
            "9eabdcb6a7e19a6a1cf082f8ef261d4c7ce6d256"
        );
    }

    #[test]
    fn control_block() {
        let internal_key = hex::decode(INTERNAL_KEY).unwrap();

        let control_block = serde_json::from_str::<SwapTree>(TREE_JSON)
            .unwrap()
            .control_block(internal_key);

        assert_eq!(hex::encode(control_block), "c4816963af90d4b882ccbcaacc920ba8e4fdd35c083a052a08d5c1732272ffccd8d6350677678c01dd2e3e90f67a0728a81e263d08b36623747ad9c811faf2fc42");
    }
}
