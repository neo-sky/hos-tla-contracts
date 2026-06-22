use crate::interfaces::ext_ft;
use near_sdk::json_types::U128;
use near_sdk::{env, AccountId, Gas, Promise, PromiseError};

const FT_BALANCE_MAX_LEN: usize = 256;
const GAS_PER_FT_BALANCE: Gas = Gas::from_tgas(5);

pub enum BalanceGate {
    Clear,
    Blocked { token: String, reason: String },
}

pub fn ft_balance_fanout(allowlist: &[AccountId], sub_account: &AccountId) -> Option<Promise> {
    let (first, rest) = allowlist.split_first()?;
    let mut chain = ext_ft::ext(first.clone())
        .with_static_gas(GAS_PER_FT_BALANCE)
        .ft_balance_of(sub_account.clone());
    for ft in rest {
        chain = chain.and(
            ext_ft::ext(ft.clone())
                .with_static_gas(GAS_PER_FT_BALANCE)
                .ft_balance_of(sub_account.clone()),
        );
    }
    Some(chain)
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
