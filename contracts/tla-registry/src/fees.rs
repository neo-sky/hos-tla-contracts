use crate::types::{total_name_length, FeeConfig, PremiumCategory, TlaEntry, TlaType, ONE_NEAR};
use near_sdk::json_types::{U128, U64};
use near_sdk::AccountId;

pub fn base_rent(total_len: u8, config: &FeeConfig) -> u128 {
    match total_len {
        0..=5 => config.rent_tier_5.0,
        6..=8 => config.rent_tier_8.0,
        9..=10 => config.rent_tier_10.0,
        _ => config.rent_tier_12plus.0,
    }
}

pub fn sub_account_rent(total_len: u8, premium: &PremiumCategory, config: &FeeConfig) -> u128 {
    let base = base_rent(total_len, config);
    let (num, den) = premium.multiplier();
    base.saturating_mul(num) / den
}

pub fn split_resale(price: u128, commission_bps: u16) -> (u128, u128) {
    let commission = (price.saturating_mul(u128::from(commission_bps)) / 10_000).min(price);
    (commission, price.saturating_sub(commission))
}

pub fn calculate_rent(tla: &TlaEntry, tla_id: &AccountId, name: &str, config: &FeeConfig) -> u128 {
    match tla.tla_type {
        TlaType::Business => config.sub_fee_per_account.0,
        TlaType::Open => {
            let total_len = total_name_length(tla_id, name);
            sub_account_rent(total_len, &tla.premium_category, config)
        }
    }
}

pub fn default_fee_config() -> FeeConfig {
    FeeConfig {
        tla_allocation_fee: U128(1000 * ONE_NEAR),
        rent_tier_5: U128(50 * ONE_NEAR),
        rent_tier_8: U128(20 * ONE_NEAR),
        rent_tier_10: U128(10 * ONE_NEAR),
        rent_tier_12plus: U128(5 * ONE_NEAR),
        sub_fee_per_account: U128(ONE_NEAR / 2),
        account_creation_deposit: U128(2 * ONE_NEAR),
        business_max_subs: 1000,
        retraction_notice_ns: U64(7 * 24 * 60 * 60 * 1_000_000_000),
        resale_commission_bps: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_resale_conserves_value() {
        let prices = [0u128, 1, 7, 100, 10_000, ONE_NEAR, u128::MAX / 2, u128::MAX];
        let rates = [0u16, 1, 250, 500, 5_000, 9_999, 10_000, u16::MAX];
        for &price in &prices {
            for &bps in &rates {
                let (commission, proceeds) = split_resale(price, bps);
                assert!(
                    commission <= price,
                    "commission {commission} exceeds price {price}"
                );
                assert_eq!(
                    commission.checked_add(proceeds),
                    Some(price),
                    "commission+proceeds must equal price (price={price}, bps={bps})"
                );
            }
        }
    }

    #[test]
    fn base_rent_non_increasing_and_boundaries() {
        let config = default_fee_config();
        let mut prev = u128::MAX;
        for len in 0u8..=64 {
            let rent = base_rent(len, &config);
            assert!(
                rent <= prev,
                "base_rent must not increase with name length (len={len})"
            );
            prev = rent;
        }
        assert_eq!(base_rent(0, &config), config.rent_tier_5.0);
        assert_eq!(base_rent(5, &config), config.rent_tier_5.0);
        assert_eq!(base_rent(6, &config), config.rent_tier_8.0);
        assert_eq!(base_rent(8, &config), config.rent_tier_8.0);
        assert_eq!(base_rent(9, &config), config.rent_tier_10.0);
        assert_eq!(base_rent(10, &config), config.rent_tier_10.0);
        assert_eq!(base_rent(11, &config), config.rent_tier_12plus.0);
        assert_eq!(base_rent(64, &config), config.rent_tier_12plus.0);
    }
}
