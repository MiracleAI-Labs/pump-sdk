use std::{str::FromStr, sync::Arc};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use anyhow::{anyhow, Result};
use rand::seq::IteratorRandom;
use rpc_data::TipAccountResult;
use solana_sdk::{
    pubkey::Pubkey,
    transaction::{Transaction, VersionedTransaction},
};
use tokio::sync::RwLock;
use tonic::{service::{interceptor::InterceptedService, Interceptor}, transport::{Channel, Uri}, Status};
use tracing::error;
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

static TIP_ACCOUNTS: &[&str] = &[
    "NextbLoCkVtMGcV47JzewQdvBpLqT9TxQFozQkN98pE",
    "NexTbLoCkWykbLuB1NkjXgFWkX9oAtcoagQegygXXA2",
    "NeXTBLoCKs9F1y5PJS9CKrFNNLU1keHW71rfh7KgA1X",
    "NexTBLockJYZ7QD7p2byrUa6df8ndV2WSd8GkbWqfbb",
    "neXtBLock1LeC67jYd1QdAa32kbVeubsfPNTJC1V5At",
    "nEXTBLockYgngeRmRrjDV31mGSekVPqZoMGhQEZtPVG",
    "NEXTbLoCkB51HpLBLojQfpyVAMorm3zzKg7w9NFdqid",
    "nextBLoCkPMgmG8ZgJtABeScP35qLa2AMCNKntAP7Xc"
];

// 定义交易发送接口
#[async_trait::async_trait]
pub trait TraderTrait: Send + Sync {
    async fn send_transaction(&self, transaction: &Transaction) -> Result<String, anyhow::Error>;
    async fn send_transactions(&self, transactions: &Vec<Transaction>) -> Result<String, anyhow::Error>;
    async fn get_tip_accounts(&self) -> Result<TipAccountResult>;
}

// Jito实现
pub struct JitoClient {
    client: Arc<RpcClient>,
}

impl Clone for JitoClient {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone()
        }
    }
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

#[derive(Clone)]
pub struct MyInterceptor {
    auth_token: String,
}

impl MyInterceptor {
    pub fn new(auth_token: String) -> Self {
        Self { auth_token }
    }
}

impl Interceptor for MyInterceptor {
    fn call(&mut self, mut request: tonic::Request<()>) -> Result<tonic::Request<()>, Status> {
        request.metadata_mut().insert(
            "authorization", 
            tonic::metadata::MetadataValue::from_str(&self.auth_token)
                .map_err(|_| Status::invalid_argument("Invalid auth token"))?
        );
        Ok(request)
    }
}

// NextBlock实现
#[derive(Clone)]
pub struct NextBlockClient {
    pub client: ApiClient<InterceptedService<Channel, MyInterceptor>>,
}

#[async_trait::async_trait]
impl TraderTrait for NextBlockClient {
    async fn send_transaction(&self, transaction: &Transaction) -> Result<String, anyhow::Error> {
        let versioned_transaction = VersionedTransaction::from(transaction.clone());
        let encoding = UiTransactionEncoding::Base58;
        let content = serialize_and_encode(&versioned_transaction, encoding).await?;
        
        let res = self.client.clone().post_submit_v2(api::PostSubmitRequest {
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
        let mut entries = Vec::new();
        let encoding = UiTransactionEncoding::Base58;
        
        for transaction in transactions {
            let versioned_transaction = VersionedTransaction::from(transaction.clone());
            let content = serialize_and_encode(&versioned_transaction, encoding).await?;
            entries.push(api::PostSubmitRequestEntry {
                transaction: Some(api::TransactionMessage {
                    content,
                    is_cleanup: false,
                }),
                skip_pre_flight: false,
            });
        }

        let res = self.client.clone().post_submit_batch_v2(api::PostSubmitBatchRequest {
            entries,
            submit_strategy: api::SubmitStrategy::PSubmitAll as i32,
            use_bundle: Some(false),
            front_running_protection: Some(false),
        }).await?;

        let resp = res.into_inner().transactions[0].clone();
        Ok(resp.signature)
    }

    async fn get_tip_accounts(&self) -> Result<TipAccountResult> {
        let accounts = TIP_ACCOUNTS.iter().map(|s| s.to_string()).collect();
        Ok(TipAccountResult { accounts })
    }
}

pub struct TraderClient {
    pub cluster: Cluster,
    pub tip_accounts: Arc<RwLock<Vec<String>>>,
    pub sender: Arc<dyn TraderTrait + Send + Sync>,
}

impl Clone for TraderClient {
    fn clone(&self) -> Self {
        Self {
            cluster: self.cluster.clone(),
            tip_accounts: self.tip_accounts.clone(),
            sender: self.sender.clone(),
        }
    }
}

impl TraderClient {
    pub fn new(cluster: Cluster) -> Self {
        let sender: Arc<dyn TraderTrait + Send + Sync> = if cluster.clone().fee_type == FeeType::Jito {
           Arc::new(JitoClient {
                client: Arc::new(RpcClient::new(cluster.clone().fee_endpoint))
            })
        } else {
            let auth_token = cluster.fee_token.clone();
            let endpoint = cluster.fee_endpoint.parse::<Uri>().unwrap();
            let tls = tonic::transport::ClientTlsConfig::new();
            let channel = Channel::builder(endpoint)
                .tls_config(tls).unwrap()
                .tcp_keepalive(Some(Duration::from_secs(60)))
                .http2_keep_alive_interval(Duration::from_secs(30))
                .keep_alive_while_idle(true)
                .timeout(Duration::from_secs(30))
                .connect_timeout(Duration::from_secs(10))
                .connect_lazy();

            let client: ApiClient<InterceptedService<Channel, MyInterceptor>> =
            ApiClient::with_interceptor(channel, MyInterceptor::new(auth_token));
            
            Arc::new(NextBlockClient {
                client
            })
        };
        Self {
            cluster: cluster.clone(),
            tip_accounts: Arc::new(RwLock::new(vec![])),
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
        if let Some(acc) = accounts.iter().choose(&mut rand::rng()) {
            Pubkey::from_str(acc)
                .map_err(|err| {
                    error!("jito: failed to parse Pubkey: {:?}", err);
                    anyhow!("Invalid pubkey format")
                })
        } else {
            Err(anyhow!("no valid tip accounts found"))
        }
    }
}

async fn serialize_and_encode(
    transaction: &VersionedTransaction,
    encoding: UiTransactionEncoding,
) -> Result<String> {
    let serialized = match encoding {
        UiTransactionEncoding::Base58 => bs58::encode(bincode::serialize(transaction)?).into_string(),
        UiTransactionEncoding::Base64 => STANDARD.encode(bincode::serialize(transaction)?),
        _ => return Err(anyhow!("Unsupported encoding")),
    };
    Ok(serialized)
}