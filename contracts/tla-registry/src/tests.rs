use crate::error::ContractError;
use crate::types::*;
use crate::{fees, TlaRegistry};
use near_sdk::json_types::{U128, U64};
use near_sdk::test_utils::VMContextBuilder;
use near_sdk::{testing_env, AccountId, NearToken, PublicKey};
use std::str::FromStr;

const ADMIN: &str = "hos.testnet";
const HOSEXT: &str = "hos-extension.testnet";
const TLA: &str = "mytla";
const ALICE: &str = "alice.testnet";
const BOB: &str = "bob.testnet";
const PARKED_KEY: &str = "ed25519:DcA2MzgpJbrUATQLLceocVckhhAqrkingax4oJ9kZ847";
const GRACE_NS: u64 = 30 * 24 * 60 * 60 * 1_000_000_000;
const DAY_NS: u64 = 24 * 60 * 60 * 1_000_000_000;

fn acc(s: &str) -> AccountId {
    AccountId::from_str(s).unwrap()
}

fn parked_key() -> PublicKey {
    PublicKey::from_str(PARKED_KEY).unwrap()
}

fn ctx(predecessor: &str, deposit: u128, ts: u64) {
    testing_env!(VMContextBuilder::new()
        .current_account_id(acc("registry.testnet"))
        .predecessor_account_id(acc(predecessor))
        .attached_deposit(NearToken::from_yoctonear(deposit))
        .block_timestamp(ts)
        .build());
}

fn ctx_callback(result: near_sdk::PromiseResult) {
    testing_env!(
        VMContextBuilder::new()
            .current_account_id(acc("registry.testnet"))
            .predecessor_account_id(acc("registry.testnet"))
            .build(),
        near_sdk::test_vm_config(),
        near_sdk::RuntimeFeesConfig::test(),
        Default::default(),
        vec![result],
    );
}

fn deploy() -> TlaRegistry {
    ctx(ADMIN, 0, 0);
    TlaRegistry::new(acc(ADMIN), acc(HOSEXT), parked_key(), U64(GRACE_NS))
}

fn deploy_with_open_tla() -> TlaRegistry {
    let mut c = deploy();
    ctx(ADMIN, 0, 0);
    c.register_tla(acc(TLA), TlaType::Open, PremiumCategory::Standard, None)
        .unwrap();
    c.activate_open_tla(acc(TLA)).unwrap();
    c
}

fn rent_total(c: &TlaRegistry, name: &str) -> u128 {
    let price = c.get_rent_price(acc(TLA), name.to_string()).unwrap();
    price.total_yocto.0
}

fn rent_alice_sub(c: &mut TlaRegistry, name: &str) {
    let total = rent_total(c, name);
    ctx(ALICE, total, 1);
    let _ = c
        .rent_sub_account(acc(TLA), name.to_string(), parked_key(), acc(ALICE))
        .unwrap();
    ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
    c.on_sub_account_created(
        acc(TLA),
        name.to_string(),
        acc(ALICE),
        U128(total - c.get_fee_config().account_creation_deposit.0),
        U128(total),
    );
}

mod names {
    use super::*;

    #[test]
    fn valid_names_accepted() {
        assert!(validate_name("alice").is_ok());
        assert!(validate_name("a1-b_c").is_ok());
    }

    #[test]
    fn invalid_names_rejected() {
        assert!(validate_name("").is_err());
        assert!(validate_name(&"a".repeat(61)).is_err());
        assert!(validate_name("Alice").is_err());
        assert!(validate_name("has.dot").is_err());
        assert!(validate_name("-edge").is_err());
        assert!(validate_name("edge_").is_err());
    }
}

mod fee_math {
    use super::*;

    #[test]
    fn base_rent_tiers() {
        let config = fees::default_fee_config();
        assert_eq!(fees::base_rent(5, &config), config.rent_tier_5.0);
        assert_eq!(fees::base_rent(8, &config), config.rent_tier_8.0);
        assert_eq!(fees::base_rent(10, &config), config.rent_tier_10.0);
        assert_eq!(fees::base_rent(20, &config), config.rent_tier_12plus.0);
    }

    #[test]
    fn premium_multipliers_scale_rent() {
        let config = fees::default_fee_config();
        let standard = fees::sub_account_rent(20, &PremiumCategory::Standard, &config);
        let premium = fees::sub_account_rent(20, &PremiumCategory::Premium, &config);
        let legendary = fees::sub_account_rent(20, &PremiumCategory::Legendary, &config);
        let community = fees::sub_account_rent(20, &PremiumCategory::Community, &config);
        assert_eq!(standard, config.rent_tier_12plus.0 * 3 / 2);
        assert_eq!(premium, config.rent_tier_12plus.0 * 3);
        assert_eq!(legendary, config.rent_tier_12plus.0 * 5);
        assert_eq!(community, 0);
    }
}

mod tla_admin {
    use super::*;

    #[test]
    fn register_and_activate_open_tla() {
        let c = deploy_with_open_tla();
        let view = c.get_tla(acc(TLA)).unwrap();
        assert!(matches!(view.lifecycle, LifecycleStatus::Active));
    }

    #[test]
    fn register_tla_emits_nep297_event() {
        let mut c = deploy();
        ctx(ADMIN, 0, 0);
        c.register_tla(acc(TLA), TlaType::Open, PremiumCategory::Standard, None)
            .unwrap();
        let logs = near_sdk::test_utils::get_logs();
        let entry = logs
            .iter()
            .find(|l| l.starts_with("EVENT_JSON:"))
            .expect("registry emits an EVENT_JSON log");
        let json: near_sdk::serde_json::Value =
            near_sdk::serde_json::from_str(entry.trim_start_matches("EVENT_JSON:")).unwrap();
        assert_eq!(json["standard"], "hos_tla_registry");
        assert_eq!(json["version"], "1.0.0");
        assert_eq!(json["event"], "tla_registered");
        assert_eq!(json["data"]["tla_id"], TLA);
        assert_eq!(json["data"]["tla_type"], "open");
        assert_eq!(json["data"]["premium_category"], "standard");
        assert!(json["data"]["licensee"].is_null());
    }

    #[test]
    fn outsider_cannot_register() {
        let mut c = deploy();
        ctx(ALICE, 0, 0);
        assert!(matches!(
            c.register_tla(acc(TLA), TlaType::Open, PremiumCategory::Standard, None),
            Err(ContractError::OnlyAdmin)
        ));
    }

    #[test]
    fn duplicate_registration_rejected() {
        let mut c = deploy_with_open_tla();
        ctx(ADMIN, 0, 0);
        assert!(matches!(
            c.register_tla(acc(TLA), TlaType::Open, PremiumCategory::Standard, None),
            Err(ContractError::TlaAlreadyRegistered)
        ));
    }

    #[test]
    fn business_tla_requires_licensee() {
        let mut c = deploy();
        ctx(ADMIN, 0, 0);
        assert!(matches!(
            c.register_tla(acc(TLA), TlaType::Business, PremiumCategory::Standard, None),
            Err(ContractError::BusinessTlaRequiresLicensee)
        ));
    }

    #[test]
    fn open_tla_rejects_business_activation_endpoint() {
        let mut c = deploy();
        ctx(ADMIN, 0, 0);
        c.register_tla(acc(TLA), TlaType::Open, PremiumCategory::Standard, None)
            .unwrap();
        let fee = c.get_fee_config().tla_allocation_fee.0 + fees::base_rent(5, &c.get_fee_config());
        ctx(ADMIN, fee, 0);
        assert!(matches!(
            c.activate_tla(acc(TLA)),
            Err(ContractError::WrongActivationEndpoint)
        ));
    }

    #[test]
    fn suspend_blocks_rentals() {
        let mut c = deploy_with_open_tla();
        ctx(ADMIN, 0, 1);
        c.suspend_tla(acc(TLA)).unwrap();
        ctx(ALICE, rent_total(&c, "alice"), 1);
        assert!(matches!(
            c.rent_sub_account(acc(TLA), "alice".to_string(), parked_key(), acc(ALICE)),
            Err(ContractError::TlaNotAcceptingRentals)
        ));
    }
}

mod rental {
    use super::*;

    #[test]
    fn rent_happy_path_records_entry() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let view = c.get_sub_account(acc(TLA), "alice".to_string()).unwrap();
        assert_eq!(view.owner, acc(ALICE));
        assert!(matches!(view.lifecycle, LifecycleStatus::Active));
        assert_eq!(c.get_stats().sub_account_count, 1);
    }

    #[test]
    fn name_taken_rejected() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(BOB, rent_total(&c, "alice"), 2);
        assert!(matches!(
            c.rent_sub_account(acc(TLA), "alice".to_string(), parked_key(), acc(BOB)),
            Err(ContractError::SubAccountNameTaken)
        ));
    }

    #[test]
    fn underpayment_rejected() {
        let mut c = deploy_with_open_tla();
        ctx(ALICE, rent_total(&c, "alice") - 1, 1);
        assert!(matches!(
            c.rent_sub_account(acc(TLA), "alice".to_string(), parked_key(), acc(ALICE)),
            Err(ContractError::InsufficientPayment)
        ));
    }

    #[test]
    fn main_wallet_cannot_equal_sub_account() {
        let mut c = deploy_with_open_tla();
        let sub = format!("alice.{TLA}");
        ctx(ALICE, rent_total(&c, "alice"), 1);
        assert!(matches!(
            c.rent_sub_account(acc(TLA), "alice".to_string(), parked_key(), acc(&sub)),
            Err(ContractError::MainWalletEqualsSubAccount)
        ));
    }

    #[test]
    fn failed_mint_refunds_payer_and_frees_name() {
        let mut c = deploy_with_open_tla();
        let total = rent_total(&c, "alice");
        ctx(ALICE, total, 1);
        let _ = c
            .rent_sub_account(acc(TLA), "alice".to_string(), parked_key(), acc(ALICE))
            .unwrap();
        ctx_callback(near_sdk::PromiseResult::Failed);
        c.on_sub_account_created(
            acc(TLA),
            "alice".to_string(),
            acc(ALICE),
            U128(total - c.get_fee_config().account_creation_deposit.0),
            U128(total),
        );
        assert_eq!(c.get_pending_refund(acc(ALICE)).0, total);
        assert!(c.is_name_available(acc(TLA), "alice".to_string()));
        assert_eq!(c.get_stats().sub_account_count, 0);
    }

    #[test]
    fn renewal_extends_expiry() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let before = c
            .get_sub_account(acc(TLA), "alice".to_string())
            .unwrap()
            .expires_at
            .0;
        let rent = c
            .get_rent_price(acc(TLA), "alice".to_string())
            .unwrap()
            .rent_yocto
            .0;
        ctx(ALICE, rent, 2);
        c.renew_sub_account(acc(TLA), "alice".to_string()).unwrap();
        let after = c
            .get_sub_account(acc(TLA), "alice".to_string())
            .unwrap()
            .expires_at
            .0;
        assert_eq!(after, before + ONE_YEAR_NS as u128);
    }

    #[test]
    fn renewal_past_grace_rejected() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let expires = c
            .get_sub_account(acc(TLA), "alice".to_string())
            .unwrap()
            .expires_at
            .0 as u64;
        let rent = c
            .get_rent_price(acc(TLA), "alice".to_string())
            .unwrap()
            .rent_yocto
            .0;
        ctx(ALICE, rent, expires + GRACE_NS + 1);
        assert!(matches!(
            c.renew_sub_account(acc(TLA), "alice".to_string()),
            Err(ContractError::SubAccountPastGracePeriod)
        ));
    }

    #[test]
    fn set_main_wallet_owner_only() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(BOB, 1, 2);
        assert!(matches!(
            c.set_main_wallet(acc(TLA), "alice".to_string(), acc(BOB)),
            Err(ContractError::OnlyOwner)
        ));
        ctx(ALICE, 1, 2);
        c.set_main_wallet(acc(TLA), "alice".to_string(), acc(BOB))
            .unwrap();
        assert_eq!(
            c.get_sub_account(acc(TLA), "alice".to_string())
                .unwrap()
                .main_wallet,
            acc(BOB)
        );
    }
}

mod marketplace {
    use super::*;

    fn list_alice(c: &mut TlaRegistry, price: u128) {
        ctx(ALICE, 1, 2);
        c.list_sub_account(acc(TLA), "alice".to_string(), U128(price))
            .unwrap();
    }

    #[test]
    fn list_requires_owner() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(BOB, 1, 2);
        assert!(matches!(
            c.list_sub_account(acc(TLA), "alice".to_string(), U128(10)),
            Err(ContractError::OnlyOwner)
        ));
    }

    #[test]
    fn list_and_unlist_roundtrip() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        list_alice(&mut c, 10);
        assert_eq!(
            c.get_listing(acc(TLA), "alice".to_string())
                .unwrap()
                .price_yocto
                .0,
            10
        );
        ctx(ALICE, 1, 2);
        c.unlist_sub_account(acc(TLA), "alice".to_string()).unwrap();
        assert!(c.get_listing(acc(TLA), "alice".to_string()).is_none());
    }

    #[test]
    fn zero_price_rejected() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(ALICE, 1, 2);
        assert!(matches!(
            c.list_sub_account(acc(TLA), "alice".to_string(), U128(0)),
            Err(ContractError::InvalidPrice)
        ));
    }

    #[test]
    fn buy_unlisted_rejected() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(BOB, 10, 2);
        assert!(matches!(
            c.buy_sub_account(acc(TLA), "alice".to_string(), parked_key()),
            Err(ContractError::NotListed)
        ));
    }

    #[test]
    fn buy_below_price_rejected() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        list_alice(&mut c, 10);
        ctx(BOB, 9, 2);
        assert!(matches!(
            c.buy_sub_account(acc(TLA), "alice".to_string(), parked_key()),
            Err(ContractError::PriceNotMet)
        ));
    }

    #[test]
    fn buy_locks_sale_against_relist() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        list_alice(&mut c, 10);
        ctx(BOB, 10, 2);
        let _ = c
            .buy_sub_account(acc(TLA), "alice".to_string(), parked_key())
            .unwrap();
        ctx(ALICE, 1, 2);
        assert!(matches!(
            c.unlist_sub_account(acc(TLA), "alice".to_string()),
            Err(ContractError::SaleInProgress)
        ));
    }

    #[test]
    fn sold_callback_pays_seller_and_swaps_owner() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        list_alice(&mut c, 100);
        ctx(ADMIN, 0, 2);
        let mut config = c.get_fee_config();
        config.resale_commission_bps = 500;
        c.update_fee_config(config).unwrap();
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_sub_account_sold(
            acc(TLA),
            "alice".to_string(),
            acc(BOB),
            U128(100),
            U128(120),
        );
        assert_eq!(c.get_pending_refund(acc(ALICE)).0, 95);
        assert_eq!(c.get_pending_refund(acc(BOB)).0, 20);
        let view = c.get_sub_account(acc(TLA), "alice".to_string()).unwrap();
        assert_eq!(view.owner, acc(BOB));
        assert!(c.get_listing(acc(TLA), "alice".to_string()).is_none());
    }

    #[test]
    fn failed_sale_refunds_buyer_and_unlocks() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        list_alice(&mut c, 100);
        ctx(BOB, 100, 2);
        let _ = c
            .buy_sub_account(acc(TLA), "alice".to_string(), parked_key())
            .unwrap();
        ctx_callback(near_sdk::PromiseResult::Failed);
        c.on_sub_account_sold(
            acc(TLA),
            "alice".to_string(),
            acc(BOB),
            U128(100),
            U128(100),
        );
        assert_eq!(c.get_pending_refund(acc(BOB)).0, 100);
        assert_eq!(
            c.get_sub_account(acc(TLA), "alice".to_string())
                .unwrap()
                .owner,
            acc(ALICE)
        );
        ctx(ALICE, 1, 2);
        c.unlist_sub_account(acc(TLA), "alice".to_string()).unwrap();
    }

    #[test]
    fn accepted_offer_settles_at_offer_price() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(ALICE, 1, 2);
        c.accept_offer(acc(TLA), "alice".to_string(), acc(BOB), U128(50))
            .unwrap();
        ctx(BOB, 50, 2);
        let _ = c
            .buy_sub_account(acc(TLA), "alice".to_string(), parked_key())
            .unwrap();
        let offer = c.get_accepted_offer(acc(TLA), "alice".to_string()).unwrap();
        assert!(offer.settling);
    }

    #[test]
    fn mother_account_not_sellable() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let sub = format!("alice.{TLA}");
        ctx(ALICE, 1, 2);
        c.set_mother(acc(&sub)).unwrap();
        ctx(ALICE, 1, 2);
        assert!(matches!(
            c.list_sub_account(acc(TLA), "alice".to_string(), U128(10)),
            Err(ContractError::SubAccountIsMother)
        ));
    }
}

mod reclaim {
    use super::*;

    #[test]
    fn active_sub_not_reclaimable() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(BOB, 0, 2);
        assert!(matches!(
            c.reclaim_finalize(acc(TLA), "alice".to_string()),
            Err(ContractError::SubAccountNotReclaimable)
        ));
    }

    #[test]
    fn reclaim_finalized_parks_name_and_rerent_works() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_reclaim_finalized(acc(TLA), "alice".to_string(), acc(ALICE));
        assert!(c.is_name_re_rentable(acc(TLA), "alice".to_string()));
        assert!(c.is_name_available(acc(TLA), "alice".to_string()));
        assert_eq!(c.get_stats().sub_account_count, 0);

        let rent = c
            .get_rent_price(acc(TLA), "alice".to_string())
            .unwrap()
            .rent_yocto
            .0;
        ctx(BOB, rent, 3);
        let _ = c
            .rent_sub_account(acc(TLA), "alice".to_string(), parked_key(), acc(BOB))
            .unwrap();
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_sub_account_re_rented(
            acc(TLA),
            "alice".to_string(),
            acc(BOB),
            U128(rent),
            U128(rent),
        );
        assert!(!c.is_name_re_rentable(acc(TLA), "alice".to_string()));
        assert_eq!(
            c.get_sub_account(acc(TLA), "alice".to_string())
                .unwrap()
                .owner,
            acc(BOB)
        );
    }

    #[test]
    fn expired_sub_is_reclaimable_lifecycle() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let expires = c
            .get_sub_account(acc(TLA), "alice".to_string())
            .unwrap()
            .expires_at
            .0 as u64;
        ctx(BOB, 0, expires + GRACE_NS + DAY_NS);
        let view = c.get_sub_account(acc(TLA), "alice".to_string()).unwrap();
        assert!(matches!(view.lifecycle, LifecycleStatus::Reclaimable));
    }
}

mod refunds_and_admin {
    use super::*;

    #[test]
    fn claim_refund_requires_pending() {
        let mut c = deploy();
        ctx(ALICE, 0, 1);
        assert!(matches!(
            c.claim_refund(),
            Err(ContractError::NoPendingRefund)
        ));
    }

    #[test]
    fn withdraw_capped_by_revenue() {
        let mut c = deploy();
        ctx(ADMIN, 0, 1);
        assert!(matches!(
            c.withdraw(U128(1), acc(ADMIN)),
            Err(ContractError::InsufficientRevenue)
        ));
    }

    #[test]
    fn pause_blocks_rent() {
        let mut c = deploy_with_open_tla();
        ctx(ADMIN, 0, 1);
        c.pause().unwrap();
        ctx(ALICE, rent_total(&c, "alice"), 1);
        assert!(matches!(
            c.rent_sub_account(acc(TLA), "alice".to_string(), parked_key(), acc(ALICE)),
            Err(ContractError::Paused)
        ));
    }

    #[test]
    fn parked_signer_must_be_ed25519() {
        let mut c = deploy();
        ctx(ADMIN, 0, 1);
        let secp = PublicKey::from_str(
            "secp256k1:qMoRgcoXai4mBPsdbHi1wfyxF9TdbPCF4qSDQTRP3TfescSRoUdSx6nmeQoN3aiwGzwMyGXAb1gUjBTv5AY8DXj",
        )
        .unwrap();
        assert!(matches!(
            c.set_parked_signer_pubkey(secp),
            Err(ContractError::NotEd25519)
        ));
    }

    #[test]
    fn allowlist_roundtrip() {
        let mut c = deploy();
        ctx(ADMIN, 0, 1);
        c.add_ft_allowlist(acc("token.testnet")).unwrap();
        assert_eq!(c.get_ft_allowlist(), vec![acc("token.testnet")]);
        c.remove_ft_allowlist(acc("token.testnet")).unwrap();
        assert!(c.get_ft_allowlist().is_empty());
    }

    #[test]
    fn fee_config_guards() {
        let mut c = deploy();
        ctx(ADMIN, 0, 1);
        let mut config = c.get_fee_config();
        config.resale_commission_bps = 10_001;
        assert!(matches!(
            c.update_fee_config(config),
            Err(ContractError::InvalidCommissionRate)
        ));
    }
}

mod business {
    use super::*;

    fn deploy_with_business_tla() -> TlaRegistry {
        let mut c = deploy();
        ctx(ADMIN, 0, 0);
        c.register_tla(
            acc(TLA),
            TlaType::Business,
            PremiumCategory::Standard,
            Some(acc(ALICE)),
        )
        .unwrap();
        let fee = c.get_fee_config().tla_allocation_fee.0 + fees::base_rent(5, &c.get_fee_config());
        ctx(ALICE, fee, 0);
        c.activate_tla(acc(TLA)).unwrap();
        c
    }

    #[test]
    fn only_licensee_rents_business_subs() {
        let mut c = deploy_with_business_tla();
        let total = c.get_fee_config().sub_fee_per_account.0
            + c.get_fee_config().account_creation_deposit.0;
        ctx(BOB, total, 1);
        assert!(matches!(
            c.rent_sub_account(acc(TLA), "staff".to_string(), parked_key(), acc(BOB)),
            Err(ContractError::OnlyLicensee)
        ));
    }

    #[test]
    fn business_cap_enforced() {
        let mut c = deploy_with_business_tla();
        ctx(ADMIN, 0, 1);
        c.set_business_sub_cap(acc(TLA), Some(0)).unwrap();
        let total = c.get_fee_config().sub_fee_per_account.0
            + c.get_fee_config().account_creation_deposit.0;
        ctx(ALICE, total, 1);
        assert!(matches!(
            c.rent_sub_account(acc(TLA), "staff".to_string(), parked_key(), acc(ALICE)),
            Err(ContractError::MaxBusinessSubsReached)
        ));
    }

    #[test]
    fn retraction_schedule_and_cancel() {
        let mut c = deploy_with_business_tla();
        let total = c.get_fee_config().sub_fee_per_account.0
            + c.get_fee_config().account_creation_deposit.0;
        ctx(ALICE, total, 1);
        let _ = c
            .rent_sub_account(acc(TLA), "staff".to_string(), parked_key(), acc(ALICE))
            .unwrap();
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_sub_account_created(
            acc(TLA),
            "staff".to_string(),
            acc(ALICE),
            U128(c.get_fee_config().sub_fee_per_account.0),
            U128(total),
        );
        ctx(ALICE, 1, 2);
        c.schedule_retraction(acc(TLA), "staff".to_string())
            .unwrap();
        assert!(c.get_retraction_at(acc(TLA), "staff".to_string()).is_some());
        ctx(ALICE, 1, 3);
        c.cancel_retraction(acc(TLA), "staff".to_string()).unwrap();
        assert!(c.get_retraction_at(acc(TLA), "staff".to_string()).is_none());
    }
}

mod mothers {
    use super::*;

    #[test]
    fn default_mother_set_on_rent() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        assert_eq!(c.get_mother(acc(ALICE)), Some(acc(ALICE)));
    }

    #[test]
    fn set_mother_to_unowned_sub_rejected() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let sub = format!("alice.{TLA}");
        ctx(BOB, 1, 2);
        assert!(matches!(
            c.set_mother(acc(&sub)),
            Err(ContractError::OnlyOwner)
        ));
    }

    #[test]
    fn admin_clear_mother_releases_lock() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let sub = format!("alice.{TLA}");
        ctx(ALICE, 1, 2);
        c.set_mother(acc(&sub)).unwrap();
        assert!(c.is_mother(acc(&sub)));
        ctx(ADMIN, 0, 2);
        c.admin_clear_mother(acc(ALICE)).unwrap();
        assert!(!c.is_mother(acc(&sub)));
    }
}
