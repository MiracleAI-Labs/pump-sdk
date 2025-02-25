use std::str::FromStr;

use anyhow::{anyhow, Result};
use rpc_data::TipAccountResult;
use rand::seq::IteratorRandom;
use solana_sdk::{
    pubkey::Pubkey,
    transaction::{Transaction, VersionedTransaction},
};
use tokio::sync::RwLock;
use tracing::error;
use tonic::transport::{Channel, Uri};
use std::time::Duration;
use solana_transaction_status::UiTransactionEncoding;

pub mod rpc_data;
pub mod client_error;
pub mod http_sender;
pub mod request;
pub mod rpc_client;
pub mod rpc_sender;
pub mod api;

use crate::priority::rpc_client::RpcClient;
use crate::priority::api::api_client::ApiClient;
use crate::common::structs::{Cluster, FeeType};

// 定义交易发送接口
#[async_trait::async_trait]
pub trait TraderTrait: Send + Sync {
    async fn send_transaction(&self, transaction: &Transaction) -> Result<String, anyhow::Error>;
    async fn send_transactions(&self, transactions: &Vec<Transaction>) -> Result<String, anyhow::Error>;
    async fn get_tip_accounts(&self) -> Result<TipAccountResult>;
}

// Jito实现
pub struct JitoClient {
    client: RpcClient,
}

#[async_trait::async_trait]
impl TraderTrait for JitoClient {
    async fn send_transaction(&self, transaction: &Transaction) -> Result<String, anyhow::Error> {
        let bundles = vec![VersionedTransaction::from(transaction.clone())];
        Ok(self.client.send_bundle(&bundles).await?)
    }

    async fn send_transactions(&self, transactions: &Vec<Transaction>) -> Result<String, anyhow::Error> {
        let bundles: Vec<VersionedTransaction> = transactions.iter()
            .map(|t| VersionedTransaction::from(t.clone()))
            .collect();
        Ok(self.client.send_bundle(&bundles).await?)
    }

    async fn get_tip_accounts(&self) -> Result<TipAccountResult> {
        let result = self.client.get_tip_accounts().await?;
        TipAccountResult::from(result).map_err(|e| anyhow!(e))
    }
}

// NextBlock实现
pub struct NextBlockClient {
    client: ApiClient<Channel>,
}

#[async_trait::async_trait]
impl TraderTrait for NextBlockClient {
    async fn send_transaction(&self, transaction: &Transaction) -> Result<String, anyhow::Error> {
        let mut client = self.client.clone();
        let versioned_transaction = VersionedTransaction::from(transaction.clone());
        let encoding = UiTransactionEncoding::Base58;
        let content = serialize_and_encode(&versioned_transaction, encoding)?;
        
        let res = client.post_submit_v2(api::PostSubmitRequest {
            transaction: Some(api::TransactionMessage {
                content,
                is_cleanup: false,
            }),
            skip_pre_flight: false,
            front_running_protection: Some(false),
            experimental_front_running_protection: Some(false),
            snipe_transaction: Some(false),
        }).await?;

        Ok(res.into_inner().signature)
    }

    async fn send_transactions(&self, transactions: &Vec<Transaction>) -> Result<String, anyhow::Error> {
        let mut client = self.client.clone();
        let mut entries = Vec::new();
        let encoding = UiTransactionEncoding::Base58;
        
        for transaction in transactions {
            let versioned_transaction = VersionedTransaction::from(transaction.clone());
            let content = serialize_and_encode(&versioned_transaction, encoding)?;
            entries.push(api::PostSubmitRequestEntry {
                transaction: Some(api::TransactionMessage {
                    content,
                    is_cleanup: false,
                }),
                skip_pre_flight: false,
            });
        }

        let res = client.post_submit_batch_v2(api::PostSubmitBatchRequest {
            entries,
            submit_strategy: api::SubmitStrategy::PSubmitAll as i32,
            use_bundle: Some(false),
            front_running_protection: Some(false),
        }).await?;

        let resp = res.into_inner().transactions[0].clone();
        Ok(resp.signature)
    }

    async fn get_tip_accounts(&self) -> Result<TipAccountResult> {
        let accounts = vec![
            "NextbLoCkVtMGcV47JzewQdvBpLqT9TxQFozQkN98pE",
            "NexTbLoCkWykbLuB1NkjXgFWkX9oAtcoagQegygXXA2",
            "NeXTBLoCKs9F1y5PJS9CKrFNNLU1keHW71rfh7KgA1X",
            "NexTBLockJYZ7QD7p2byrUa6df8ndV2WSd8GkbWqfbb",
            "neXtBLock1LeC67jYd1QdAa32kbVeubsfPNTJC1V5At",
            "nEXTBLockYgngeRmRrjDV31mGSekVPqZoMGhQEZtPVG",
            "NEXTbLoCkB51HpLBLojQfpyVAMorm3zzKg7w9NFdqid",
            "nextBLoCkPMgmG8ZgJtABeScP35qLa2AMCNKntAP7Xc"
        ];
        Ok(TipAccountResult { accounts: accounts.iter().map(|s| s.to_string()).collect() })
    }
}

pub struct TraderClient {
    cluster: Cluster,
    tip_accounts: RwLock<Vec<String>>,
    sender: Box<dyn TraderTrait>,
}

impl Clone for TraderClient {
    fn clone(&self) -> Self {
        let cluster = self.cluster.clone();
        let sender: Box<dyn TraderTrait> = if cluster.fee_type == FeeType::Jito {
            Box::new(JitoClient {
                client: RpcClient::new(cluster.fee_endpoint.clone())
            })
        } else {
            let endpoint = cluster.fee_endpoint.parse::<Uri>().unwrap();
            let channel = Channel::builder(endpoint)
                .keep_alive_timeout(Duration::from_secs(5))
                .keep_alive_while_idle(true)
                .connect_lazy();
            Box::new(NextBlockClient {
                client: ApiClient::new(channel)
            })
        };

        Self {
            cluster: cluster.clone(),
            tip_accounts: RwLock::new(Vec::new()),
            sender,
        }
    }
}

impl TraderClient {
    pub fn new(cluster: Cluster) -> Self {
        let sender: Box<dyn TraderTrait> = if cluster.clone().fee_type == FeeType::Jito {
            Box::new(JitoClient {
                client: RpcClient::new(cluster.clone().fee_endpoint)
            })
        } else {
            let endpoint = cluster.clone().fee_endpoint.parse::<Uri>().unwrap();
            let channel = Channel::builder(endpoint)
                .keep_alive_timeout(Duration::from_secs(5))
                .keep_alive_while_idle(true)
                .connect_lazy();
            Box::new(NextBlockClient {
                client: ApiClient::new(channel)
            })
        };
        Self {
            cluster: cluster.clone(),
            tip_accounts: RwLock::new(vec![]),
            sender,
        }
    }

    pub async fn get_tip_accounts(&self) -> Result<TipAccountResult> {
        self.sender.get_tip_accounts().await
    }

    pub async fn init_tip_accounts(&self) -> Result<()> {
        let accounts = self.get_tip_accounts().await?;
        let mut tip_accounts = self.tip_accounts.write().await;
        *tip_accounts = accounts.accounts.iter().map(|a| a.to_string()).collect();
        Ok(())
    }

    pub async fn get_tip_account(&self) -> Result<Pubkey> {
        {
            let accounts = self.tip_accounts.read().await;
            if !accounts.is_empty() {
                if let Some(acc) = accounts.iter().choose(&mut rand::rng()) {
                    return Pubkey::from_str(acc)
                        .map_err(|err| {
                            error!("jito: failed to parse Pubkey: {:?}", err);
                            anyhow!("Invalid pubkey format")
                        });
                }
            }
        }

        self.init_tip_accounts().await?;

        let accounts = self.tip_accounts.read().await;
        accounts
            .iter()
            .choose(&mut rand::rng())
            .ok_or_else(|| anyhow!("jito: no tip accounts available"))
            .and_then(|acc| {
                Pubkey::from_str(acc).map_err(|err| {
                    error!("jito: failed to parse Pubkey: {:?}", err);
                    anyhow!("Invalid pubkey format")
                })
            })
    }

    pub async fn send_transaction(
        &self,
        transaction: &Transaction,
    ) -> Result<String, anyhow::Error> {
        self.sender.send_transaction(transaction).await
    }

    pub async fn send_transactions(
        &self,
        transactions: &Vec<Transaction>,
    ) -> Result<String, anyhow::Error> {
        self.sender.send_transactions(transactions).await
    }
}

fn serialize_and_encode<T>(input: &T, encoding: UiTransactionEncoding) -> Result<String, anyhow::Error>
where
    T: serde::ser::Serialize,
{
    let serialized = bincode::serialize(input)
        .map_err(|e| anyhow::anyhow!("Serialization failed: {e}"))?;
    let encoded = match encoding {
        UiTransactionEncoding::Base58 => bs58::encode(serialized).into_string(),
        _ => {
            return Err(anyhow::anyhow!("unsupported encoding: {encoding:?}. Supported encodings: base58"))
        }
    };
    Ok(encoded)
}

// 示例代码
#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::{
        message::Message,
        signature::{Keypair, Signer},
        system_instruction,
    };

    // Jito发送交易示例
    #[tokio::test]
    async fn test_jito_send_transaction() -> Result<()> {
        // 初始化Jito客户端
        let jito_client = TraderClient::new(Cluster {
            rpc_url: "https://jito-api.mainnet.solana.com".to_string(),
            fee_type: FeeType::Jito,
            fee_endpoint: "https://jito-api.mainnet.solana.com".to_string(),
            priority_fee: PriorityFee::default(),
            commitment: CommitmentConfig::processed(),
        });

        // 创建一个简单的转账交易
        let from = Keypair::new();
        let to = Pubkey::new_unique();
        let recent_blockhash = solana_sdk::hash::Hash::default(); // 实际使用时需要获取最新的blockhash
        
        let instruction = system_instruction::transfer(&from.pubkey(), &to, 1000000);
        let message = Message::new(&[instruction], Some(&from.pubkey()));
        let transaction = Transaction::new(&[&from], message, recent_blockhash);

        // 发送交易
        let signature = jito_client.send_transaction(&transaction).await?;
        println!("Jito交易签名: {}", signature);
        
        Ok(())
    }

    // NextBlock发送交易示例
    #[tokio::test]
    async fn test_nextblock_send_transaction() -> Result<()> {
        // 初始化NextBlock客户端
        let nextblock_client = TraderClient::new(Cluster {
            rpc_url: "http://nextblock-api.mainnet.solana.com".to_string(),
            fee_type: FeeType::NextBlock,
            fee_endpoint: "http://nextblock-api.mainnet.solana.com".to_string(),
            priority_fee: PriorityFee::default(),
            commitment: CommitmentConfig::processed(),
        });

        // 创建一个简单的转账交易
        let from = Keypair::new();
        let to = Pubkey::new_unique();
        let recent_blockhash = solana_sdk::hash::Hash::default(); // 实际使用时需要获取最新的blockhash

        let instruction = system_instruction::transfer(&from.pubkey(), &to, 1000000);
        let message = Message::new(&[instruction], Some(&from.pubkey()));
        let transaction = Transaction::new(&[&from], message, recent_blockhash);

        // 发送交易
        let signature = nextblock_client.send_transaction(&transaction).await?;
        println!("NextBlock交易签名: {}", signature);

        Ok(())
    }
}
