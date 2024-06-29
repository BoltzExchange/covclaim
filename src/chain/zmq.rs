use std::error::Error;

use crossbeam_channel::{unbounded, Receiver, Sender};
use elements::{Block, Transaction};
use log::{debug, error, trace, warn};
use zeromq::{Socket, SocketRecv, ZmqError, ZmqMessage};

use crate::chain::types::ZmqNotification;

#[derive(Clone)]
pub struct ZmqClient {
    pub block_sender: Sender<Block>,
    pub block_receiver: Receiver<Block>,

    pub tx_sender: Sender<Transaction>,
    pub tx_receiver: Receiver<Transaction>,
}

impl ZmqClient {
    pub fn new() -> ZmqClient {
        let (tx_sender, tx_receiver) = unbounded::<Transaction>();
        let (block_sender, block_receiver) = unbounded::<Block>();

        ZmqClient {
            tx_sender,
            tx_receiver,
            block_sender,
            block_receiver,
        }
    }

    pub async fn connect(self, notifications: Vec<ZmqNotification>) -> Result<(), Box<dyn Error>> {
        let raw_tx = match Self::find_notification("pubrawtx", notifications.clone()) {
            Some(data) => data,
            None => return Err("pubrawtx ZMQ missing".into()),
        };

        let tx_sender = self.tx_sender.clone();

        Self::subscribe(raw_tx, "rawtx", move |msg| {
            let tx: Transaction = match elements::encode::deserialize(msg.get(1).unwrap()) {
                Ok(tx) => tx,
                Err(e) => {
                    warn!("Could not parse transaction: {}", e);
                    return;
                }
            };

            trace!("Got transaction: {}", tx.txid().to_string());
            match tx_sender.send(tx) {
                Ok(_) => {}
                Err(e) => {
                    warn!("Could not send transaction to channel: {}", e);
                }
            };
        })
        .await?;

        let raw_block = match Self::find_notification("pubrawblock", notifications.clone()) {
            Some(data) => data,
            None => return Err("pubrawblock ZMQ missing".into()),
        };

        let block_sender = self.block_sender.clone();
        Self::subscribe(raw_block, "rawblock", move |msg| {
            let block: Block = match elements::encode::deserialize(msg.get(1).unwrap()) {
                Ok(block) => block,
                Err(e) => {
                    warn!("Could not parse block: {}", e);
                    return;
                }
            };

            trace!(
                "Got block {} ({})",
                block.header.height,
                block.header.block_hash()
            );
            match block_sender.send(block) {
                Ok(_) => {}
                Err(e) => {
                    warn!("Could not send block to channel: {}", e);
                }
            };
        })
        .await?;

        Ok(())
    }

    async fn subscribe<F>(
        notification: ZmqNotification,
        subscription: &str,
        handler: F,
    ) -> Result<(), ZmqError>
    where
        F: Fn(ZmqMessage) + Send + 'static,
    {
        debug!(
            "Connecting to {} ZMQ at {}",
            subscription, notification.address
        );

        let mut socket = zeromq::SubSocket::new();
        socket.connect(notification.address.as_str()).await?;

        socket.subscribe(subscription).await?;

        tokio::spawn(async move {
            loop {
                match socket.recv().await {
                    Ok(recv) => {
                        handler(recv);
                    }
                    Err(e) => {
                        error!("Error receiving data: {}", e);
                        break;
                    }
                }
            }
        });

        Ok(())
    }

    fn find_notification(
        to_find: &str,
        notifications: Vec<ZmqNotification>,
    ) -> Option<ZmqNotification> {
        notifications
            .into_iter()
            .find(|elem| elem.notification_type == to_find)
    }
}
