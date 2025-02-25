use solana_sdk::commitment_config::CommitmentConfig;
use serde::Deserialize;
use crate::constants::trade::{DEFAULT_BUY_TRADER_FEE, DEFAULT_COMPUTE_UNIT_LIMIT, DEFAULT_COMPUTE_UNIT_PRICE, DEFAULT_SELL_TRADER_FEE};

#[derive(Debug, Clone, PartialEq)]
pub enum FeeType {
    Jito,
    NextBlock,
}

#[derive(Debug, Clone)]
pub struct Cluster {
    pub rpc_url: String,
    pub fee_type: FeeType,
    pub fee_endpoint: String,
    pub fee_token: String,
    pub priority_fee: PriorityFee,
    pub commitment: CommitmentConfig,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq)]

pub struct PriorityFee {
    pub unit_limit: u32,
    pub unit_price: u64,
    pub buy_trader_fee: f64,
    pub sell_trader_fee: f64,
}

impl Default for PriorityFee {
    fn default() -> Self {
        Self { 
            unit_limit: DEFAULT_COMPUTE_UNIT_LIMIT, 
            unit_price: DEFAULT_COMPUTE_UNIT_PRICE, 
            buy_trader_fee: DEFAULT_BUY_TRADER_FEE, 
            sell_trader_fee: DEFAULT_SELL_TRADER_FEE 
        }
    }
}