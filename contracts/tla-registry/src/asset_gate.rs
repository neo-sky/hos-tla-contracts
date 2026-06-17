use near_sdk::json_types::U128;
use near_sdk::{env, AccountId, PromiseError};

const FT_BALANCE_MAX_LEN: usize = 256;

pub enum BalanceGate {
    Clear,
    Blocked { token: String, reason: String },
}

pub fn ft_balances_clear(allowlist: &[AccountId]) -> BalanceGate {
    if env::promise_results_count() != allowlist.len() as u64 {
        return BalanceGate::Blocked {
            token: String::new(),
            reason: String::from("result_count_mismatch"),
        };
    }
    for (index, token) in allowlist.iter().enumerate() {
        if let Some(reason) = block_reason(index as u64) {
            return BalanceGate::Blocked {
                token: token.as_str().to_string(),
                reason,
            };
        }
    }
    BalanceGate::Clear
}

fn block_reason(index: u64) -> Option<String> {
    match env::promise_result_checked(index, FT_BALANCE_MAX_LEN) {
        Ok(bytes) => match near_sdk::serde_json::from_slice::<U128>(&bytes) {
            Ok(balance) if balance.0 > 0 => Some(balance.0.to_string()),
            Ok(_) => None,
            Err(_) => Some(String::from("balance_unverifiable")),
        },
        Err(PromiseError::Failed) => Some(String::from("balance_query_failed")),
        Err(_) => Some(String::from("balance_query_unverifiable")),
    }
}
