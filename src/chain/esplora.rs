use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use crossbeam_channel::{Receiver, Sender};
use elements::{Block, Transaction};
use log::{error, info, trace, warn};
use ratelimit::Ratelimiter;
use reqwest::{RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;
use tokio::{task, time};

use crate::boltz::api::Client;
use crate::chain::client::RpcError;
use crate::chain::types::{ChainBackend, NetworkInfo, TransactionBroadcastError};

#[derive(Clone)]
pub struct EsploraClient {
    endpoint: String,
    poll_interval: u64,

    rate_limit: Option<Arc<Ratelimiter>>,

    // To keep the channel alive
    // This is just a stub; streaming transactions is not supported with Esplora
    #[allow(dead_code)]
    tx_sender: Sender<Transaction>,
    tx_receiver: Receiver<Transaction>,

    block_sender: Sender<Block>,
    block_receiver: Receiver<Block>,

    boltz_client: Option<Client>,
}

impl EsploraClient {
    pub fn new(
        endpoint: String,
        poll_interval: u64,
        max_reqs_per_second: u64,
        boltz_endpoint: String,
    ) -> Result<Self, Box<dyn Error>> {
        let (tx_sender, tx_receiver) = crossbeam_channel::bounded::<Transaction>(1);
        let (block_sender, block_receiver) = crossbeam_channel::unbounded::<Block>();

        let rate_limit = if max_reqs_per_second > 0 {
            info!(
                "Rate limiting requests to {} requests/second",
                max_reqs_per_second
            );
            Some(Arc::new(
                Ratelimiter::builder(max_reqs_per_second, Duration::from_secs(1))
                    .max_tokens(max_reqs_per_second)
                    .build()?,
            ))
        } else {
            info!("Not rate limiting");
            None
        };

        let boltz_client = if !boltz_endpoint.is_empty() {
            info!("Broadcasting transactions with Boltz API");
            Some(Client::new(boltz_endpoint))
        } else {
            info!("Broadcasting transactions with Esplora");
            None
        };

        Ok(EsploraClient {
            tx_sender,
            rate_limit,
            tx_receiver,
            block_sender,
            boltz_client,
            poll_interval,
            block_receiver,
            endpoint: crate::utils::string::trim_suffix(endpoint, '/'),
        })
    }

    pub fn connect(&self) {
        let clone = self.clone();

        task::spawn(async move {
            info!(
                "Polling for new blocks every {} seconds",
                clone.poll_interval
            );
            let mut interval = time::interval(Duration::from_secs(clone.poll_interval));

            let mut last_known_block = match clone.get_block_count().await {
                Ok(res) => res,
                Err(err) => {
                    error!("Could not get latest block: {}", err);
                    return;
                }
            };

            loop {
                interval.tick().await;

                let latest_block = match clone.get_block_count().await {
                    Ok(res) => res,
                    Err(err) => {
                        warn!("Could not get latest block: {}", err);
                        continue;
                    }
                };

                for height in last_known_block + 1..latest_block + 1 {
                    let block_hash = match clone.get_block_hash(height).await {
                        Ok(hash) => hash,
                        Err(err) => {
                            warn!("Could not get block hash for height {}: {}", height, err);
                            continue;
                        }
                    };
                    let block = match clone.get_block(block_hash.clone()).await {
                        Ok(block) => block,
                        Err(err) => {
                            warn!("Could not get block with hash {}: {}", block_hash, err);
                            continue;
                        }
                    };
                    trace!(
                        "Got block {} ({})",
                        block.header.height,
                        block.header.block_hash()
                    );
                    match clone.block_sender.send(block) {
                        Ok(_) => {}
                        Err(err) => {
                            warn!("Could not send block update: {}", err);
                            continue;
                        }
                    };
                }

                last_known_block = latest_block;
            }
        });
    }

    async fn request<T: DeserializeOwned>(
        &self,
        is_post: bool,
        method: &str,
        body: Option<String>,
    ) -> Result<T, Box<dyn Error>> {
        let req = self.prepare_request(is_post, method, body);

        self.wait_rate_limit();
        let res = req.send().await?;

        if Self::is_failed_status(res.status()) {
            return Err(Self::handle_error(res).await);
        }

        Ok(res.json::<T>().await?)
    }

    async fn request_string(
        &self,
        is_post: bool,
        method: &str,
        body: Option<String>,
    ) -> Result<String, Box<dyn Error>> {
        let req = self.prepare_request(is_post, method, body);

        self.wait_rate_limit();
        let res = req.send().await?;

        if Self::is_failed_status(res.status()) {
            return Err(Self::handle_error(res).await);
        }

        Ok(res.text().await?)
    }

    async fn request_bytes(
        &self,
        is_post: bool,
        method: &str,
        body: Option<String>,
    ) -> Result<Vec<u8>, Box<dyn Error>> {
        let req = self.prepare_request(is_post, method, body);

        self.wait_rate_limit();
        let res = req.send().await?;

        if Self::is_failed_status(res.status()) {
            return Err(Self::handle_error(res).await);
        }

        Ok(res.bytes().await?.to_vec())
    }

    fn prepare_request(&self, is_post: bool, method: &str, body: Option<String>) -> RequestBuilder {
        let url = format!("{}/{}", self.endpoint, method);

        let client = reqwest::Client::new();
        let mut req = match is_post {
            true => client.post(url),
            false => client.get(url),
        };

        if body.is_some() {
            req = req.body(body.unwrap())
        }

        req
    }

    fn is_failed_status(status: StatusCode) -> bool {
        status.is_client_error() || status.is_server_error()
    }

    async fn handle_error(res: Response) -> Box<dyn Error> {
        let status_code = res.status();
        let res_text = match res.text().await {
            Ok(res) => res,
            Err(err) => return Box::new(err),
        };

        match res_text.find('{') {
            Some(start) => {
                let trimmed_msg = &res_text[start..];
                let parsed_error = match serde_json::from_str::<RpcError>(trimmed_msg) {
                    Ok(res) => res,
                    Err(err) => return Box::new(err),
                };
                parsed_error.message.into()
            }
            None => format!("HTTP status code {:?}", status_code).into(),
        }
    }

    fn wait_rate_limit(&self) {
        if self.rate_limit.is_none() {
            return;
        }

        loop {
            match self.rate_limit.clone().unwrap().try_wait() {
                Ok(_) => {
                    break;
                }
                Err(time) => {
                    std::thread::sleep(time);
                }
            }
        }
    }
}

#[async_trait]
impl ChainBackend for EsploraClient {
    async fn get_network_info(&self) -> Result<NetworkInfo, Box<dyn Error>> {
        // Send some request to make the endpoint is valid
        self.get_block_count().await?;

        return Ok(NetworkInfo {
            subversion: "Esplora".to_string(),
        });
    }

    async fn get_block_count(&self) -> Result<u64, Box<dyn Error>> {
        self.request::<u64>(false, "blocks/tip/height", None).await
    }

    async fn get_block_hash(&self, height: u64) -> Result<String, Box<dyn Error>> {
        self.request_string(false, format!("block-height/{}", height).as_str(), None)
            .await
    }

    async fn get_block(&self, hash: String) -> Result<Block, Box<dyn Error>> {
        let block_hex = self
            .request_bytes(false, format!("block/{}/raw", hash).as_str(), None)
            .await?;
        Ok(elements::encode::deserialize(&block_hex)?)
    }

    async fn send_raw_transaction(&self, hex: String) -> Result<String, TransactionBroadcastError> {
        if self.boltz_client.is_some() {
            self.boltz_client
                .clone()
                .unwrap()
                .send_raw_transaction(hex)
                .await
        } else {
            match self.request_string(true, "tx", Some(hex)).await {
                Ok(res) => Ok(res),
                Err(err) => Err(err.into()),
            }
        }
    }

    async fn get_transaction(&self, hash: String) -> Result<Transaction, Box<dyn Error>> {
        let tx_hex = self
            .request_bytes(false, format!("tx/{}/raw", hash).as_str(), None)
            .await?;
        Ok(elements::encode::deserialize(&tx_hex)?)
    }

    fn get_tx_receiver(&self) -> Receiver<Transaction> {
        self.tx_receiver.clone()
    }

    fn get_block_receiver(&self) -> Receiver<Block> {
        self.block_receiver.clone()
    }
}

#[cfg(test)]
mod esplora_client_test {
    use crate::chain::esplora::EsploraClient;
    use crate::chain::types::ChainBackend;

    const ENDPOINT: &str = "https://blockstream.info/liquid/api/";

    #[test]
    fn test_trim_suffix() {
        assert_eq!(
            EsploraClient::new(ENDPOINT.to_string(), 0, 0, "".to_string())
                .unwrap()
                .endpoint,
            "https://blockstream.info/liquid/api"
        );
        assert_eq!(
            EsploraClient::new(
                "https://blockstream.info/liquid/api".to_string(),
                0,
                0,
                "".to_string(),
            )
            .unwrap()
            .endpoint,
            "https://blockstream.info/liquid/api"
        );
    }

    #[tokio::test]
    async fn test_new() {
        let client = EsploraClient::new(ENDPOINT.to_string(), 0, 0, "".to_string()).unwrap();

        let info = client.get_network_info().await.unwrap();
        assert_eq!(info.subversion, "Esplora");
    }

    #[tokio::test]
    async fn test_get_block_count() {
        let client = EsploraClient::new(ENDPOINT.to_string(), 0, 0, "".to_string()).unwrap();

        let count = client.get_block_count().await.unwrap();
        assert!(count > 2920403);
    }

    #[tokio::test]
    async fn test_get_block_hash() {
        let client = EsploraClient::new(ENDPOINT.to_string(), 0, 0, "".to_string()).unwrap();

        let hash = client.get_block_hash(2920407).await.unwrap();
        assert_eq!(
            hash,
            "4a1f9addaea74ef4132005fa1411abebb3f968b8cb766af93547c77b6207d619"
        );
    }

    #[tokio::test]
    async fn test_get_block() {
        let client = EsploraClient::new(ENDPOINT.to_string(), 0, 0, "".to_string()).unwrap();

        let block_hash = "5510868513d80a64371cdedfef49327dc2cd452b32b93cbcd70ddeddcc7bef66";
        let block = client.get_block(block_hash.to_string()).await.unwrap();
        assert_eq!(block.block_hash().to_string(), block_hash);
        assert_eq!(block.txdata.len(), 3);
    }

    #[tokio::test]
    async fn test_get_transaction() {
        let client = EsploraClient::new(ENDPOINT.to_string(), 0, 0, "".to_string()).unwrap();

        let tx_hash = "dc2505641c10af5fe0ffd8f1bfc14e9608e73137009c69b6ee0d1fe8ce9784d6";
        let block = client.get_transaction(tx_hash.to_string()).await.unwrap();
        assert_eq!(block.txid().to_string(), tx_hash);
    }

    #[tokio::test]
    async fn test_get_transaction_not_found() {
        let client = EsploraClient::new(ENDPOINT.to_string(), 0, 0, "".to_string()).unwrap();

        let tx_hash = "not found";
        let block = client.get_transaction(tx_hash.to_string()).await;
        assert_eq!(format!("{}", block.err().unwrap()), "HTTP status code 400");
    }

    #[tokio::test]
    async fn test_is_transaction_included_error() {
        let client = EsploraClient::new(ENDPOINT.to_string(), 0, 0, "".to_string()).unwrap();

        let res = client
            .send_raw_transaction("02000000010184deca029bfafa9814557c8483964bac14011e1b6ac2115726c9e19c3cf810c00100000017160014b6e8026a881fcbe566aa2517af4a9191a12615a9fdffffff030bb65f0e91c8c79aa139f528ac9a73c5140c7fc0045918d168d360ce9d8a332ded09656b38ef77c0b56cf13866634350c0c0f91b6ef37da9f41b4f17c87f2285b6390347b1c45e3e29b6fc51624c730a6cbff25b3ee9b5566196eeaf47ceff1496b70117a91481f667e593a96d198ed042b9f56a6e58224b4e13870b568534e07c55f250ea1afcf3aab0118562fe4903c422db1050889412e735c448099921851818c2dc362a733ff785ac7401bf3f40c3091ace6ee7ccbecb0fa3f71f0274b927e7609dd41e32f0991dc79a7d645914802b2351e9391e08543676aec92b17a9142e16837ee6f74386fa652296a944c471a5a54fca8701499a818545f6bae39fc03b637f2a4e1e64e590cac1bc3a6f6d71aa4443654c140100000000000000fc00005176160000000247304402206b5f44962a8916599f51b766ba21084af153ac523327875bbe6c4de6196823f0022012130fad3021f866b05327f220f6baab3f50435be4b50cfa9c5edf12feaf4bd8012103dd2ad2412f8780085fec6e23a5406e9bb7a2cd79de02373d9de6d5da81cf4b280043010001e49bcbbaa5d8479235ba526b61fe87c9ec95af0c146ccc73b2b5203be8f00277929a82a9ea035bfa58e3e0bba8c2ac33852c7e93994444908f2f22095c8c8b27fd4e1060330000000000000001399b19007e2e3410bbd174b977c34fab4bd6702cf48e089957d50473b0c50e483f1e0bc7220b33a8498811de53fe9d307a88b37af9715aebeab7da69cec50abe1d4af2e76712844ea265ccf938c68b3d96aeb67659afb0960b5327a0898d5f02985431c46c978f598ff92b0eeedf5aa190c1e03a0ebd9d8f00b19091dcf8adf4d6bc43e5b3f63aa4214a21d69d0fbb61bcc24bd2382d360d3fcc72286c2ac5f811b5d0601e892789fbf74e4903b12bea3cc32826f126b3dfb973dcb83765b0606687caf0b5bb32b7c867f555557b55018fe23c0c56781f77a0eaca5b11e247684910c83493084053ab98ef144f36f8e11438a2ab2779bf2111bf96e158a20a6a54d1c21dfff49dcbc591ce88bf775c7522ba1bd14619abcc44e71bedeec8d19d1bd984e944451ca2a397103c7c7d12e59ad56378f073140772d39beaad8314a9ab903f16e748f02e215b985fc1be758d2384e6bca652a1b1988414d337e09399104e917fdf87e52b83fe018c45a9c072ce41afe260bc54b51ea61669a250296655bb4fd1bf721eda49045c5b28175cfac98a4834e81ede2c4653b1f2d5e5a64661772e88b262459147606e0eed8df9e9e2ecca5c8e7f93e9c84115ba28c1addb78c76fd4f1529aa59d80d9aa9776e75cee94549b952b9aca131a9e3f823618f50d82843ec1afa051ef4052f8683f6201420ab151145e6a9742059c157a6ac6cf71050efd50a1625113d681cae0a1e33f93372e94d03bdf8d7489c53d5ebf0c99ef12568d37d112aeca76c11562b0a6de275f14665674df7c3ac4eab73ba349f9f22ae205f78e41e064b980fb1ff5187cc34b7f5b0703a6008af3e12e394e87a2c634c4f0022e6b81c5645d3f28830d018a4109fd68a7061dd99526dc2abeaab56fbfbaff8f9e622c9d1891bebb121ac393f53528b4ce1702495c2eebf88054fb4693b03dcacdd16bb1bf44763311ad1c430b3b375d0f1cb78d16e0f4977cc4f7c7ca5ab0cf06709e9c2dbb757bd51880ebf36b8b6d7859a736d5bf735adcb090cab7834036d4f8bddaf85f6d95249c1982ef409ec28de1443c12cc2240faa5264d4ab87019cdc37d906f6912c5ec0916395e46f2588ad075a0bf6c563451248f4d4c2bc32eff4de7e8a35121bb997fc1ee68095ed0dfd10b63314203be9aaea4927f729bcb034bdc8123fb5cd69bb5cff47ae5079b68668c7d27876aa34e0b57020c1bf48a86e4e89638ca4bd791909e979d14bf248aabb95988eb36f5c1878d182f4abeb72781911cf53f9c36a65e10c2e0d641109fb51fdc2100af3f70e28b47b2245e0a190a33cfe7e5215c31a18691a056919b159dfe8aab87ff64f9e6abe8c7ddb9f04c27396ad35d6d6cabe876d40426d6aa2c8216e91d497ac7bf3d8c8344c3ec9dcc438ca1c4c84c3777df949f6fdf02c510007d4e3b57523e51e22d468bf852a1190fdc0a64c81317bc10b913f8c59c0fb08f58d9d002f448e9c19290564e1387487cfda4683f3e4856205fb6c909b53a362f17ae9ba30b45911ea19f648a2abd7c9094caabc3067caeea7bdcab46c41a330ec835bf3ea0a913458fbfcf18a1292afd77591d6d14c3a65da69bd1570152d4259ba8fde40fed99d2a958baa38abf4da26cc8ee18644733bb75e48973c7a21b007d0ce6d4fe81a84656977732bfd96d23b586aa526fd436bfb3d4d5b99c6afb8d8615acacb1c6ec9205291dd1bc48a918f4b3fbd06db17dec6c9b32154903a423a1db4bd52e4a1f6fa9e7541ff0c82aa674b61331725c081dd3ff5834b3ddc58f14e10ff474e6116955646ebbb942121e50c733b82be61dbdab50205c76abc2e01efb9927f268aff8d35c4fc705e915fb93f93af618b9416e78ba1761901e31d2c4fd4bb4a55e17f5843592f1055901c6f8cfe1285c6b190a193afd0f7fed9496795a239baa2cab15f16f0dd184ab43ac9e3674f269017a467781ca6216147ea4863a0ea6db256193683d5e72865244f8876ea8641576b8f9bc0fbfed3e10bdc8bac2ea44196c7f1a1ebb00de1192c31d450de81f312cda6257dc04dd9b72c617f1dc966b4e22a4ef938bf6fbc39fbdc12e3471525238bbd11f51dbb57add1d6a41a54cc1ee572b502a0ee08fd51676ce3a4c6d4dc42fae6d7713039dfd4888599e3416b04b245d5cbd9ebf1d0b7981d624ff577bee97828e06f6c8867d2f76dc991df896f9a3baf9d14171b34e83e6f103b5f7c4f3afe9329972ea7c97fc2e5c5098779dfbc3ec2e3ef3fe431e528e4636ea64a450b118461678fb82e99cdbe8435ebac4d8b395396aeb6442d72c3f6936b796f499da9db4b4cb350c94d46523e5c52ed55a6a17c9e10a5013b0114e109ea0aeeefbbde5e7ca5cd6314712157f36a3f3c76ac3b3e280aefef07b29b594e44b0cd8b7c90f4cfcb6ff83deafbc199351178a15aabd9a73d3e948e8b314fcdd30284698fafcce37a9038153d80aedccc5c077b634d65fe69adba4f89d5bcebcd624aaa54a12dd965a4f796932b77d95ca36911f1eb8fe51f675d024690716e7de3c4d855333d1195c2db5eaf0578a51267951c4c912b50412a80a2decbabf076e0d16850a4c968bafecfd52c4f08245094bdb7ce50695fe5acc7280cc422069633dc77f6f0a7aa7f90d114948fed1d035722cdf86ff1c3eca788eaa84cf2821a586dc3180bf0575eb88dd7159dca56a2ec8afdf583d8a1ad19dbe3664150f215ebf39494b26caa01121df91af5a2230365aa43f24000ce9583118f64d6bef69d3041560e7caf34ab9cf01bce13b12c48a24d45232e88eb1c648f0d71ab37384bc985f8028b390bb90614bcc7a08733cb1603b107d330a5132dd32ab4575a5eb1aa9ba8d87701ee63a807fc20c37d4df8a362f66ca4be014839d966c0dc4b2be60a1d7e7ff8989200762c5d00c691e1f30c20c1704eaa07eef15cde90745922dd3c9e3094b2af081c2f35bae58f776e9f30fc0d34de3c693c0ccabe1d908502ddfe34a42169ecddb6284a8c093c01dce2b903ff33f9cbe3a1222399e5c7c42f3613989fab500d7cacc96d7f396d44545f5063ab0122e854f7dd5685ee8d3ee905f596bb44e2214f40542c3bfa393192a5679d8e4f556171c18f6a7abc69f436dc671b222cfc047289619d5075a1c2af7e852c229eb9396357916bac491bac3420068901a73ffe4193460d3e6845d70bc1ef02aad12b96941b4cd7490a8eb6bbd0c6b838aec8f40cdca6608d2aabfca791f92c65ecc3ec7b60b797f00a953ec2c02f1a26e1e08cbed8b1b090059fde76f98c27109e0811f52939d2778bcfc0e1b476e9ea83fbadbd3404f2a7c86d15467aa60de06ebe22111309107bbe7b78407ca7408028296a90f1e34a538ee47597c5e6f2e96c5d78e68dd6a2965ce3c40e2534b9d9a657c665c9a43c3e19d2a2df2b1f5c9d7bcd7ddb8ef4bafa7d87fa3f6d21b74df4f19aaa663aff8bbed69da402eceb8e5b6a661b15c2a1b005ee9621ce696c4b8f2611dfcb801a1f8859e99920bcdbdca957418b90931baf1d9660bde68e73d691b4eb1b565d334c2d9048eae5e5ffb42b4caa7b82a5c334d3849c57e7c37caf6e864016f2297101ea5916575d3e913476e486ecf63b84e238d4f0e9238a604cea3f0d430aa053277352419832dd2bd177bca008186eaf6765537590ced018d6b458291ee5a8825abe6c367b472be30cc62e10da57175167b9879284adea27b16b49bd65f24702ef1f97ec5e1c0a7bf803dc6718f4cd9ebab9ce965bff2f224b72ad4972927070b99d01e5e218423e6895698a62dddf90cff00ee1a90a8defb7dfb77015f3d2d61a3e6f63e3fe0c2c9d483abdf950f210d14d85db6f5e557a4f508588fc3091b397a3837c6ea0ae3c50c7b4242a402d623ee93a972abb240767fcdb50f9aabdd7970e07104699048c6fd24b31aec29d23c9ef407a02cb00f0e91660c71e06cc3680b57d03a6d1052abfb3682eca38aeda03ca422fc8539371e64dc7062d25ba9496305225c543f26dab8431c41788b614c735c691c5156ca95718f1a8ad25acfaa4dd28babd3da9dfaabde3d87b5c17456a1793a6df2507bd6b3d56cfd6cbe1100fe5c34b1ed8d3be0c34bd90d986536196564cca21cb318eec978212471c1e7f6579b049b7f119abb3e422adc27da701bf9e8e746b15553c75c9491cf8a6fc290844db659ef78ac1c0a1d5126350d6c537a1a58cad1a231fbdf9aefb63c2a4c1bce2483c5edefecee758108ddaacc0c206460c4b2c41ff5b87c7fbf737f46d1c54b22e2ac8eed24beca4cda61ab6717f1ddfd16be6abbbfacc6ab9981de1b5560ceebdecf325e82d3cf648831cc9c5ec3d27dd4322968df543b23197acb6eff9afa545214347dc352220becb4a2488ad26202454f4034bb6bb1bcae5024587df865cb218dd6f3852ee5d111094b538d8bf8be9a9c6fbb5b2c26f97fb7d032b6fe74b130b0fc3362b4e1844d790de3ec97e87997b938a3e743f60b12ae54338e84f544b4bcd157aecdfe56f81fb7a473e9c7d0302edc398510ebd5c2d5b444b8880328b686c512c0462394acc662cf64a167c0f684ada15d02af46ef120de5d93a659ff62e963bc0cbce111b1202c59888938755c63440f71663fee1d0ffed673f2d0a9dce73cb3b2de1f70757fca0b994f12cb4d1d622bd36b1514b499c1dabf7f1865cc5f29e5a6d3bb098d0fef88969a7f2a5a176cc6c93ff5bc0bc66acc437240d42eb3250f3be5580b548c6d880acb4194c703b5e298639e5886df03f2140f375dae3979b9b270db513356da114d44e6cbf74dfdd6077ee5db8c3ad829d0197e43a3d27b002816c6b6e231c77cf34b6f0266b66fc986fc55de88f96bda121c0a489cb538d6397f61c0c08113bcdfed316955ef38a5f2e1c4d16efe7679a41194ba1fe12302ea6ff03b526775080ad2458f9e5f06be7b411c0c6132427e14b5e2b837ba4048bb2cf4a86c95944d4cb2b1fb16094085baaf31e33b1e7259d3263200c23a68035d66682db2f5f8b5b2d29119a7755b08e2ada58b5578025a07821694092db7a924e98d9ed2d5348ecd9e4eaa9a3d9f8f4e06406af500179973cb2e9906d972f951e232caac744e1fad797acd05924b755e2545af9c5c018b9a64afbcf31fe6a14349aef07fa47dfbc28ad67179b71e4eb9bf956a3e15bf11a0e6a93bc2acb3ab1b1525878c0a1ee5397d959f1c42bc5b6cc0f5e008d929dd8807c3e48c9ada539cc16bcfa8fe1a40d232331bd7974d16a261b369f3743bb687593ea4b9cb836d15d0835e5c80167125329c2b44f49c0a833270bd20f52dd7d6a39857cd0740a066490051659e12e65fb9407f1c3818e9551fc6ef7adfad85e530297e69d8c28d68f4f17443aa818ba60e51c38f4cda31d689b9b2eee806bcdc4163e4a8cde700d311bebca0c2bb531dc16d2ded1724bba41f70a4ab75b6adc54d134359a41eb5a54122d64d5f37d9d7ded55837205a9ae04927e22d55eea513a33228affb54fc6f70cd389648ed45f4fcea1ec9b4e8140e2f2ef74b76135d631e75fa2a32bb31a0d29b879e1e1810d4455f7f79d14c41cd117a859b5845edc99ac235166796963be286eeda5a37cf4ee412c1c1a6c08b0c7779f99d6ab439c3e6b97c57b67c50bc2cbdceba31e6e1b37cf142c568c1a63e1d02dfcfa945107c41352f795bf81f5f91e1d869061b7cde258f3cafd0d1bc22f861f156c00acb3c89df752ab14b9f06300e28fe23ad2f38aca9f9ff73ee72a0f56bf1befd592bb75b10641de1efd224f769a7f413c247c4953b35ac36f2d83147ca0e39f542f89015f68c73392823930e5304ce6ca44a5c914775985d663549c31878222c7fca20aa1ffbd2af468f53a2e994098a15721dc7443010001c2fda0ca474470e56a95138d1e6ef8c4d1da2a8e76fc07b8fa32dc9e6c6dfff86d932a88866210f88c4e97c3beefc60c5f7c7b959ecc56eb7b04ccc9951158e1fd4e1060330000000000000001d6976e019badcfc39246c6e131f746b86565ca7771d46a5861f81ad39119a462501b2626e64bfc94177e09115f3682a6c022459b692978f40333cd9ab8f55ea562d536f4de852af14a3d08fef8538821825bf1b56d3fa5cd63954bae80d4a5e98f81a00dda45de33f1c66af4b9874286e0f7462de004866c740a00c6a508c1c1cd3821cd976bdae5cc03de12b15aa9a145664109cb9244341c0d6e8edd2737d49e5d80c7e8bfe19687156e4b6931a61b242e8e31eaa3d3df930b4964a1d17a19bbfd8cfd27f4ce9e61adfc2b456ecd463fe173466a0b1413241d9cbde0218ffc6d8dd2184a75a933f057b474e4b3e38d608a54d18266ccbd18a94609a4c4c93de958c49df258bcf9cc1afd8871b667313ab2981671246b17cbd71469b9dd94b3148bd1294e141927587d8869c681fee7bbe93b8d16660e957d8cbf1b1b05aa67c5b599e9cdc4d67b5a558ecbcdf8aa000cdf21250f9865a45291dfdf1c57af94f3fab8efef1f033357c1841a2c17194ea4de306b186bb0f3ce36159cf5b40fc6eb6ee407fe7c93efd3f9c423496fabcb7b557c090c9fe098c59b5b6fb98cc89193d79815e042555cfce51fb2add6ff306f234c70ccbe02298c7c642c16dfc61b8e1a3ee3ac6067248ba262c3e73abcf5c72b79e7c382c1dfb15da2242dee19d0a35b9ccfc91b94d8b0c3bd69a367cc052fb6d4b55be5ed401e7b9ec88f42796e9839faab426ddc571ba09660fbb52f8c9ecf5805f59fc32474a567ce931e20e987b86123fb0926238ac7009f1d1b9e86fa6a36107a306a4fd02702ed52d120298127a6e598836db72ad659c3cd916f4d481d096d03940cd079f1d9aeea7e3f71ac363c834836af4eba469c946c34032f95f579a2295f8fe9d3d29677a7687536d337db4758e8a2811f21d75066ec263970e6f46e98041a533c2e70229c9ec6f0c73139a8bc8542b2142f13415ed6fea6ba9b19706c450ed6bab5aa9f74c3accda3f385963a60e01d3c43e6128e1ecd68443afe827b8cbdc7c34c6b60aef91b0f920c6568f4b84aa929a923a7f11317ea27c74c3286ac9f83f288268a99af2b521cc7adc2e6e6012a004cb15b9f4e32ad92bfd5694df67ada4cb64cbd56cf5c506668d5d7449032626f7f2d380c37856cd61587f214d284e4e62279a765d2d37f6929629a74be133bf282ad09979a749022c3ba35969725d42da667583b810e52a7ab3eb33aa6af5ab63872728eb126192701a0d565f88b703e8ca4c03be97d62e5df216ac2e2f1feb0670eb26c3e15678ed8967b293e1929ee20013335deb8ca576beda339711a5e2d22acf19f04f1973f3b5509df88af98642708dd028a2eecd46cd3c96b7c3ad4819523d0e4f6eab65429fda16e4fd67e06e422ce50f922e2e2824d7540fd3f9d2773af858eb5b936e5c33b97a0855375250021e1aa4a73e055ca2f7f0be9e8e2022a388b59dce0f66d14964ee66b4e140e493edab19eb2be96a68467ab92ab7cbffe6d916545142234a954e4a718d2001fb319ccb0246fef152e7874058b9f50495413a0ece194e55dcc957fa031e0059a89ce070241058006370a2852d5a8d2149decd0b58b90233eb4fdd13d162e6f8bd481d53758f49f34b5f3b4913ecd8818522eaaefe96371233daabb7d247ae3e4341ee08cf716d77c6c8ed15a57e6db82d9e2287ef073fd9a70e22085008eb6587c478f8d073f6b29f1d3a16cdb30d644fc915ed718faeb9d9033eef14299a6176329890b396541743fbb7e9e10e72f5b58e5d4ec0de1e8a53b2afa83ff212dad3b75c060a34df4acc037377f20e92a2172dfc4041402de9dcc7e82a6deeb4fb6f2eaabd3e43f9053d21868bfa29b861d7ef979e9df5840b7ea629485cfb50441065a9f48016b2ffa06659cc468e7e3867b41c59ce3260dd9474679444f3dcf1a8916c7380ae34aeae6925e16427f367c0f4d935fbbbb811be559a88f32cde88a00531a679018b13ba701ade5d87e1cfd8c71c5a9f476d201d0f4710ca33ea80f11b73e846833cc0d8b5bbcd134fcfcd3d45533d6befa8170ef46a56a9b9f3a65b80ba57be42e8585e8be6deb679a67bfe83cfcda51d937c7837386b22d5930cf8f2b1b781c9cbd90cbd73b4d53395e32ad8f37213884e77d0a11d5f843d0da3c40143c3a5cb85ff62f8c6285667033134a88e8659630a7b08ff5eea67e75c17f407eea23c8940770b753db002f2afc0a4b73f1c00a132c194d562a934768155629fcbdd4a757d9fcaf0f0f366acd52ffde0c02fc9cfd812bbbbd63d4b9398b9fd4e0f1f70161921110ecd002e8ef0baab01b657a04a9bc5951033400624e003b4fdb28393b30fc4e216b0bd7643c2a8082f8824c0fa3f42c4a1dd6541f010d8559edc26f3ca44747fbb43ff90a468b4c33e9bee72d5214bf4b622d2ce29837eb643aa3a3c4040a062c386c1f89efd7c80e80c82642585f554ea5a3446c15f4ac293f76b879463047fd77244586be9bca77d2c42a17d5471ed514387f7d045085e12ffce37a99ac3e3d40213b23aeeed50044439df88c7c11903677fec5443043d5f9d6eb9c802d79e6bffc4175ae08649e8016b9b6d006478bf6a94860afce9a5ef2df551cb66dee740f0b5e3a0b8d070c251ad8d2719f1545ecdc1c16d48394777f4c425d3b5c2e4bc028a25ee1efabbda703f7731e37baf2e15f07614ce95ea7b51da7f4e7b574a4b08ccae3ae6b2f663ce0e5e3862281de1930cb1b4f37543e880f8f210ade31c4109aa4532446c369fbf058788acc86500acfe6fb2ee26453ed50d052c319ac841e2d6978761a7008f558f163ba0cae0d8227c2124dc1e1774db755b0cbb63b3aa4f94505f328b4ca5da7aba55bd13b556035f39ff64bccfc75c33fd42cb0d9427990ad1494e20208c9adc7e117facb782481eb2594d2244997825b3e5235643b0c84d7d7112114f8a78e68c11b36ea86b56fc61f925aac502383a1b8581e9e129918265bece2cd56afcce8f785d98d8c55216a4b3a63259e09376cdfb7f059ab5d24462d3d1e5514c0290feda07f8e57929d98a853493ec3a726024636dcaaa55ca1b09419466c1d43e8afa11cbf396391191acbd3cfd82035aa2850e3143977cd0afd3db9f471e2900005fc8eeb7c1084b2592e48030b80973c1f10ccc5c568e8262c8963f9906643dac7d3edb60f110d3b8aea55caa369bd9bb2b8a79801a19be1203347386918e1b2ef56ce32e4d3a344b963ff0482cbfbfbfd758be2a852aacdf4b9619585757914ad397a012356d89078d24a7b8ec4b1fc46dc80defeebc8b7b9d98f74020e93fbbabdcd8f4e2f32125b395c32bda054a8cabbcae80945ebe2369579bde0b5c7b998566a18514a4e55e12019aa82589b940b0cad097bba7f775770c9be751b07185c68d65d9cf46ac6d6f238560c5a4ead0c41189a59cf4bbf401588b2a5d63d6008dece3d948d7e189dada7f0ab62f0e49f8d646310cc85fb9403669b0e249a2cce508cca1d03b6cff26d45275d4a510a3aa3a03b8806737ef2ab6947f9e9c01bc59378dedec96bbba4f5127f1a330e6e50ca4ec85069f2b08ba4a19751cb541558f21b69bad2484bc98885f10713369bcc4cfb2c776471b69d2cfa871f84c2b154ffc73ea433f211c098370476bc0c974426a4bdea73fb2812a228fc9d775a9f107abc8749ad842ab393914f4cb832cb0f373ccf069452bd37316b48bacecc9ea5f01a4d15433fdb904ac3b2118a2099ec26d4c6a90e3fd2e0418d1f448082706ff7127ab72782e7d10deb76e96d88efcc068bad8631d190d92318f798c9add70eba586625af62f787b867aad162c2b96878184142bf1984ab72ff36f40e943c0d23b25c469cdbb9e75fefc6be0695eff0790a65adc2ba800f395d1f732a4b5469c98c44826a47806e5f45a03f558af99f5a9353dc3b87867cd6b8bcdfa9251665ffb86158f4eea8e75fc00274ee0534b3a1fb7a6056b3a15046556cedad29e7d9e45959abf13fb159fa401deeae5f1198fd71af3fe499379698dde6927f0ec9d57d6dc3334a1426ec1620b05f515868c8f05160dcfa40f75b3643e7c2ed84ba0ac6dcf2a5607c94965f364c92ae18e5240099d5dd2ae2690cae15aee5591d4ea019641e7da6b46e5acb77ba3f44bebb9944c7c75e3975f085cc749ad95b2bb2506fdb18f4496adbfb4e4f42817e67e848b0287b62eb511a70f6256cc5658c72f1dddab975477246377945fefb819432d7f485d6cadcd0934c791fa1a6dbdb719726478fd33cf0b51e8efa51439a09d111b90c1c525ec0b4c9d1dc1167d30fe36d938fa67c1f9bab1675c883ec637e829643662e0793082fc2e51a6834163f548b50b927a62f4344e22ccc64e03bd4046c0a4e6308b74d1feeef2c46b4b9cba1dd2be006daa1382b34edc489404cd249c60d1241352ada7063ea207f3bd3716e27b8092a42058131034eba64dea8236644dc1811ff654740f96b6bc598b21d1a87547ab9c66583798e85984f3956aee41201749fd54a849ab6858fd7e1a74dc6632a62cc1a27b92dfd3a52fa77225f4446e3438a32aa2694321120f250d543a0bb3e8ec83fadba68d706db8ab2d1aa34576cc1852acadeab456a5c14f606f014c765fdc3ed45002053eb48be5097666dfd076d9f6dd4fd6c948d0a40775e8b41d8b2666f3f7a77f53d093d23c0225abd79dae4cdf65fda8d81150e7f66dccef3f5acf3bae2284fc94ca952f41aeb2822fb0d10d61493615cb3ada9319389efb58360e32a051ef4874f78d6f688ef4ee8429cb13fd1ed99f37af0c49bec0ce4d90d6de17657ac3ff7a8e6a21fc3b4a6cf0794b408316c4310c778eea65fee261658f2750b06f353771ddd1f47a1ba561f1cf3a42a18936181b01ae1d0412f8c169af4263deff5762cf8e3b346095d6d84755d09bb4f0145ff99e5353440fa1a8a4a2b1c6f6fda7782dc46b308a3f085440e452617c5845b85b9b3dd182ca53a7bfc494bb23b0173be4a4b8784e60d0670d7d65f7e4bfe9297c2e8a0db366c2f4793b2f8a9b57253cf59627a723ca46b5f1201051a13b53041746d15173e9da82768a7b7c0160b74591b4f8ea7fae18eda171824640da4e6532f7c472ebe60fbee726e9cb3b8ee7398db84cea5278ca07253de35375c36f473d846347763e589be9812a6214c21d814655c1a3b9c8d2b201a43d8b42232e7832151e84ce07d86f520cacc1a60194f1f257e0839af62c20b5d5425119d3c69374975b292008879a6a804b8cc6f8a8f2d5c1b155ea091e244b5ab7df02c88263b9fc432461308c321c4d957fdbf85db08fcc95c2a31916145c6426f331fae097380b5b1a7b5f4fad0f8e967598cd9cdbc96ba0cd94861d5ecffeb11086ed7b2afed8401ba4f1daa74e1e93f0f51e7f52a48db03cc80e7c32b55f6a05ce8fd06b8f2102f0d3cf52b6626fc67d55ea33488ff2156076f45c8993f6112cd6d7fa8045c79b92c29fb46594aa1c293ff66bd3c604a6e08aa2652c874e6a0961b9599d38a80a6a3cade428f58fc8fda382f8c7af485161c4a4e852d047279fbd5df00cadbf46bb229f8c8600c04bafdad38e9117c239b6ce73ec8dce24f2ed0a759bb8a4352d5379ccf01425b46600d829b9a34400d39c12e4c67dd743ea5c7a198ff44edc91e05d9a88e66fd4f23b192369f4c51378692a6e1f5f85db7856d95fd12234ad33fe2f91ad158aebd4647d5a4bba990cb0afd4a8e0215968576ac704d44a15bff3e5293337707fe621bbe5270d312b19eda13ffdd04323c5082acacda3bc35b7692868307794257e7d07c70e49638a931e95472203449aa90817fb710f363bd0c54d3c3914bbf74539e1e6694d3ccc1eef14d12aa87ad4edcab2a85aaf4910000".to_string())
            .await;
        assert!(res.err().unwrap().is_already_included());
    }

    #[tokio::test]
    async fn test_is_transaction_included_error_missing_or_spent() {
        let client = EsploraClient::new(ENDPOINT.to_string(), 0, 0, "".to_string()).unwrap();

        let res = client
            .send_raw_transaction("020000000101958ffdbb9b9f2ac3fd7e11283719912e8d3e3733e214c8eb17af3cac072acc3c0100000000fdffffff03016d521c38ec1ea15734ae22b7c46064412829c0d0579f0a713d1c04ede979026f0100000000000003bc0016001426367eb832f544e85b5bbb744eafbf73b3ed21ce0bb12a99864f9c109d678d0ced77b6b6ff8f0605602f9a1924a4670f428d80e7dc086fff1661e71c42759172736b2b8268aaac71e5a1f027cf5d57f5e2660ad9818c0270aa9faa2af9ff6e106320dd015aeb32f2d9e9fd4a545c29da279fcf3c32b8ca016a016d521c38ec1ea15734ae22b7c46064412829c0d0579f0a713d1c04ede979026f01000000000000000e0000000000000000032008c4f4b5ab016edda96efaa5540b77994a991c1d3f07feeed4f9f0d6706be5246882012088a91448e29f061c35a5ea60a377f1caf64535e7eda5a48800d100881426367eb832f544e85b5bbb744eafbf73b3ed21ce8800ce5188206d521c38ec1ea15734ae22b7c46064412829c0d0579f0a713d1c04ede979026f8800cf7508bc030000000000008741c4c5d4066e23e32abf9451a1719c46b928314cf498110371819fecbcaaa77326cd851e5ed89296ac05ddc9d6966e319d1541e58b3e6d8f3d1eace9b24c9770c09d000000430100018dd2b979f7e3c27f924511270389fcacf4f88c0f54bfaec198d8666631f4ecd9de0ab767606a7a0c238e74c358862798d21829595845cf0868a9d436e07b347afd4e106033000000000000000104282200a7d1114d140791383077de1603d82d6ef65c451705d6ac7b9a5a802ce38249fcf06fcdec9fc343ed9adad9d2e2f1bad6344f0b7fae3e48e32d3a760a563f1da99ca93db6c579d7ba8fab793236d23f534c3d61c6684d4145919c3bdacd5d677895ff17c994816847e9ba2f0f10f508d782b751538b72d139f804ddcf966aeef8c5e34b5f6afeb681a650434efaa841ebe2240d26f13d53e5c30e0a2b6a4539e7ac4150376676725e97f9a5ac51e10c0d9790684e4a0a2dc90ce047bcbd7abfd3a93b6ef5317ed5771aedfc388587cb4adc3c2eb12dc16ced33525194617780848cd3b82099a2db1beea2c982479f185b9c549fc2f76c9aaeef645b61dfff5d123baeb00fae4c1f19a2bd0b6e7dce05a5c74906822131eeb5d5db229a561f452ef0ee91443fd9ecf5399f1e2d25c1b539e13403f23bc53ca96111d971ba7291aca34bbc605ebdfa53c42416b12ff3d3edc5b16008fa880112d32036429cf710cdc3635cad6d465c7fa24ffc18cda441656eb74193285e56a7b50b46539ae1c8106ed397ed3f927e967ac6499e3541f504dedaa72e73b84470871f22fa67c5db1869f977854e12351b460e77f4ca5595e04aee3c82e950c1f4eb3e834ffd529bd9aa195ee3e705f09e74af8438d82d4f245ae8b81df1c947d37d2b3cfb6a96142587418ae1564872ae8d670b97163cf9e7fb41d663452cd83fbf5af355bf97098411e4749c1282fd2e86733fa7f64461a28d674f274c805f4e997fdbe921dd1aa7c2d6500e8cd5f796bbc26e7522eb49846c6ba27a5290978aaaafc38726a981a6db98508ac839fdf317bc504e5af5902e904d0ad112600acc1622d7233a7c9d41a4081a75542a31424a0d1a1d4edb91a05a029469730b1670583baf32295a0b453c0f83f7ada6a5927e1dc3f16674ca0754b925d8cd5f3164527fb591766e97df78a8e3c0f34bf0fbe04daf49fc3242481f616be0193d5562eaeddcdbc9ec7c1f348897ee879f9d58a84ac604307b271929019c627617d206d54ef2d47bfb87296d70fa6570f7dc035105ed72024195567c8567600272fe91e44a355a6c46d0845473884a9742b0b1c9bb74dd623a3c7b1b7bdfdad920e335d956ca62641f8cc562cee1592a0964e64c7c33e48444d92cf14bbfb35bb4c981f79561ee7cecf28fa25afd09345bdf4ecbb8c7ae4947367ce2431e52674c09995d29de81f9caaa807d19d3c5ff83e1c75e1d4e4936b23cad87934e2bf51b9f3f890f963306c44199384f67280efe08c67942ee62b883c7c0b9e618ad5f0f30909135e22776e6a67642698d5758385d6eb9b633eac51c37259987a878bde1c14d4a0436fc9ad43d500253b8108b3bbb559166cfd159b752236bb38cde493d1476a728e5996aab27193f62e1ab80df1db02c3be7fb3fafc0f7cdfc091bedca57f737a4163f823e2f92024b21f4cdc6d125ff882e2e609cf0d5acc9fbe63d692bfe89b051ced61d5654cd50a7ec9fd42cd3c73c4c18d426bccf036752a548e0c6263df3b38d5e0098251a000e553d80cc09cfcddc7a1bc73f41a1073c9096aaa52fd3f5c60d5ed9e289424d71f2df3387387fb54a1812e40cc318cc9387fe1978568f5f6bc80584fdc74a7a0a1d2fa25acfabbf6b8104fadac7922c3d2b2c302626cc4912623f9aa1473487a42d29227c95e99095f07fe473d626619fd32cf25c6aedf5ab36b2f844340b490db000989ff16fc89bfbebab87f7d812fe6069c6ff06033d8eeaab69308cdc83a683614c28d863537626966b70720de6242ed0499eb4c876d8dfed6696ee63cf02d4b1324b3413b7559aef5222108a2031b8317990eefe64f2c7a3624a9892d5cbb2edc01a7d00cd6c2f98acc4fb70c0b5b833cb87bb50b19727f8a3b30517a6846a0e9a5c9073dae837564a9d750d40bad695d1b54cd27cffc68eb599818ac135f4cbdefb5a82bacadc24983ffabcd5f19e85217fdb7c9ffdfe491f8743032b5789583bf940059bee9484a84101d2a9048f475883e6b00c9b73bf82015c5decfdd25cbfe680fc7aa5ea40b04c35b4c41dbbf61672f0da3a67492af8c7a640ab5e4173642edce124e83fdcc5a103d832cc11f61880a9930dd80cea184116db8f1bcf56aeb1391cdcd2e77c4590f32dc22162cdabb9e611133d4da45309d2eb984ca355873913080e172667939d79765a27d71bbf8bedc92820e1a8c46cfcd367315de846ba80a9434f36cad80e4a1a425371c2d62a544fd4692df501ff3475465d76d0287667a6230afaee8c208630e0a7be1160731e026a334bdd7a7fc3f3efbdace9aa44e57f5769f53a6ee7fbc68a28bfc4845eba6a95d79613e09b98f4aec76ef6864c4e3c982cdb425474ede61d4882a17235add7e1185cc861c8514deba83f56c85d152c8b6034397294360b424f7ea5b5da3b6f1d5d0c1582e091e3e7f36a6d078d74de2738a26aff4462a7360fefd675e71e6d457669793407db039eab62d895ef6504219dc99968a0aa76b2888874d12bc6f54fd6a4bbfe295af3280c640b3ce94ce12e9d86314b2ba2e03a46cdedfc084c3d37b773fbbbc1dd59105d1cb61ba1c65293b3674f264b64ff49a9df623ddcd51a31987bf5ccc27579ea573ce83ddec1c79bd7a3167acf19c9eed24a823e286d3c12c71e04bd8b478dae3abe5cab6ae55e05accb41f30eaacd8c0ce2d806ac89319d0d915699f071129b5896ba0e9250a8487b6e3393a08a8b3137bf6f29673af85c7ba556d4e51d4b8c4adcd50d755560748b595a100aeb093b6d223be4a17f0cd1d87f7ed7eb9b2e9861d46823a4ced10f7c18598b921ac610671c9630ec6fbd5d4eb87160364066b5d8d63a15a8f1b30e2d2f7d86d1d86e31fec7d026ee7b40f385e1d3ebda159c57f5d73a8d2130dc2cb75bdaaaf2dd294017e622ab3218dd54a7c78caa00fc5051d04c9342589b98b2467b4f9e91e7e30f76b584894c96100bc38548bd1240416aec87a4c6d17bc2357df5173d6fe6921be3988d771fd5ae4c6192e3a43f0bafff4cb2b1fd381868a193b1ee80857e93f9628f128fa4086a27c455c9a85bd6164b6d8e15ab59817af39e8439da7c57ace5c221b3106709bbb0e653d6ff8d86706d46d812778d12f978e827534a8c59348c493cc4a486a6e93a99a8759e451ea8a4111c6a6f846afef483b142ec1505b721b68e3a418393de3d3538a854a309d13bcc1fbaff22cd1b6528919750be1aaef31affb2d0547296effabc1b0d40afdae32d9e176b9579862ed730b71aba36ba46bed5d43a25c96eeb58be45f18045f91611034f87cc5a19808a867b6548aae08724629ad666f5381a38ce9c3e31ea309aabd2a4ad62bbcd20b125f2c422cdbf5cc9b1c97c83de1d0400fe021d998357cd74b9c7d487244d366bf6fa5ccfe500a50a5e68adf6e3939228994e6e44b8f11ecaf79398a7c5521a67a09af3506a446ceed282d3961e97999696d26f87061e8d96328d9031db04f096bf374f28db595416ae06c00146b1130e90eb5d5ee2bde0f73dd61027460b916410104c87e8885f61d8dd562cc0ea05c24285e6b297fe0d936475769c721b114ba8f3a9bae57f385610c61fc2098293fe63861f7de971e0e0045056cc1116ebffaa2f591fc34e96622dfb4066fe669ac8f77a83ddc81a919962783e0836bda7ad6e4bac4440d5634ccf48d63a50f43a4e1166e3188e93733f65b168bb23c5e7fb1289076c0ee9d08dbb9678785ce3f4c121e1a3b8ced59235c1897f566ed771e2ab0b8050d5cb699c97cb45c284f220c03c2f6b26195e2978941fd506caab2741d90f914489b967f4dcd15f9138220fcffdbced53dc36d5d3973df3b21780989316749327bb63041d49cc769719b8d5f30826b6bccf669a3dc56564c37f7512256f73b4b5e823650b08c43f12d6f6c2f9fcc786c012db345de292141e167681849ff4ac4c286cdd5e68e4409ce93579ef85245c7aceeb198037a39da39dc3aac765ee75e71ff19c30a87f16d0ca4aee1a6041b1be90a8db711775f2faa44f986337f6ff2f0295f6bc891407ad460de39ae1ef2dfeff8d7526385524c810f8f3c546b3ad299146c6cd0c4750ea8adffa35cd210ca7c1a00747b1cf2be0963ddd146b8ba0e223f7ff4eeaf8bc8f5d75cd9c0a96830ecefcda3b0678f61e75d3ecb5ef342d918d332c5e669f1fe90b071c8f73199cd3aa8415d505773ce2601df6ce0dd0374b0f0a8d797b73cb6e7b1f839d53f62feedbd287941ffc63bc346535b8bb891ac3ecc5c2070b253c9a032f4c7368597f3d3d68fe1d3d1d06cebb817a2fcb90edd0d8b71b7e691cf645a615aae7da2f90d9130c7ac8c99150ff6f39674f0db9da596324403593aafc65f1c6596bba842d5be60345ca9ff56a909a59f69db2db23b1428d850b29bc0f33084073203e72a06a627915ea070a6435469a41aa5e34d305da185d038386b52e9f275221144bf995d289054d9985904c3fade004ba547539a13f28005b898b5d154b4c98ab267083b8c389afa52c3de99aecc9ee47a34a45c4facfec177021875812280af97341f54b58acf87b3f094cf981255b47173706caf24a8bbd063e0a95a80e8c3a531f7e415338c258e8e4c9ff34735d1469e404445f535b4101635df3198152e234217db77e2400bd8d740324292f77fffab7bb1900bd19b15508bdb74d4da3a44fe088f2706ca2a43328db40273b9500429c553dc92445125912d07fc748dbfb8e62cc19f9a70eb8e2138af1f4efed5a5758f9443262fc310838b199db4145e9f86ab421a262210094f2872ab0b34997306dd1a4467e2877c0d3df2ffa10b2fe99d6da09db2e9771873b3fee06a76b54d62a7a11aef20839b5e10f378e39bb7c1d4c428d7f628778a16660c47c22a8d0291e00fc1d0eaf62e557ebc16e4515ddca419da6704f8912465e910f7892f777d04cddb5ac4d547bc967dff4e49a0d690a62361ddaab025396952602000137996620b4db287ba49d85923b8b55697b6630a7bdcb7ceef056662143e91b4a257f6004b3331d39fe6da2c0f0ef61c1373f2e102bbc0d708a4c686a0825a2c95a03685d8ce4eea1b33929552099030f0bb8f59c3302aecc4ee789d810cdb803e9e4d541a31ded5772dd6a6e4a49fbe844721da73db4c924cc5a7dfd63948a022281b1c6053b253b1605f3fb47315d35ba3b00fe9efb3aca71a7d17d54fcaf155a6310e4ed3dbb1b834aaeb849939a5da1197ffe75b9df7e06aff8f5867d814f4e6652f44bb929d104c623c65767df404ac663b681c30432ce20679bf9fde04ccbfab770387e24e5924874301584a2fe68a6ae04cfeb8b630fd3158165c438b6ef6a4c652f4ba9aa45ca4cd95e1c475fe6ad2c8c56a476928ae248212f3cdebe59e3b80a869b7fdbf215cc280f58990434d729d036436ffa574742c89cb587fb5446bd494c8417e2afde8b9dc7bc8ab5538f7006881c8bd5c5f5ef281f86d3ad17f83230446bb58a45cae32e244c32fe77f38e28fb707485f856cbb3cfda21608ac0a916ae5e2dbbaa45f7d4a2e1da86a7fb243208c0abb0d28c0990c9773985b45a21a7bc37ae36090f74f4cade687308c16e779d5c8da3911e63d6b5b8cf301d1362951d1dfff236ad441b52304e5bd501f1bd074e83bc95e868b961fab607c86f40f649bc9e661e6555f87244956e29b4b2bfe215a1f73a5e3b619a018a760892e5cd44ee786d9853a1963724518d362ab8507802272bbc67abc9604f59b917d68d80988ec2ecfa9c18ad5bc401ee850b447f6399e04a47281909257f59ec34d3d5c77868092563b95b9d503f71669ee6df57e46066615ca52509145bf78251c48484840c556c714a25104b4ee2855aadfbfc47c30000".to_string())
            .await;
        assert!(res.err().unwrap().is_already_included());
    }
}
