use crate::asset_gate::{ft_balance_fanout, ft_balances_clear, BalanceGate};
use crate::error::ContractError;
use crate::events::Event;
use crate::fees;
use crate::interfaces::{ext_active_signer, ext_hos_extension};
use crate::mother::effective_sub_lifecycle;
use crate::types::*;
use crate::{TlaRegistry, TlaRegistryExt};
use near_sdk::json_types::U128;
use near_sdk::serde::{Deserialize, Serialize};
use near_sdk::{env, near, AccountId, Gas, Promise, PromiseOrValue, PublicKey};

const GAS_FOR_FORCE_TRANSFER: Gas = Gas::from_tgas(45);
const GAS_FOR_SOLD_CALLBACK: Gas = Gas::from_tgas(20);
const GAS_FOR_BUY_BALANCES_CB: Gas = Gas::from_tgas(85);
const GAS_FOR_SIGNER_QUERY: Gas = Gas::from_tgas(5);
const GAS_FOR_VERIFY_CALLBACK: Gas = Gas::from_tgas(10);

#[derive(Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub struct PendingBuy {
    pub tla_id: AccountId,
    pub name: String,
    pub buyer: AccountId,
    pub new_owner_key: PublicKey,
    pub price: U128,
    pub deposit: U128,
    pub owner_key: PublicKey,
    pub sub_account: AccountId,
}

#[near]
impl TlaRegistry {
    #[handle_result]
    #[payable]
    pub fn list_sub_account(
        &mut self,
        tla_id: AccountId,
        name: String,
        price: U128,
        owner_key: PublicKey,
    ) -> Result<Promise, ContractError> {
        let (sub_account, owner) = self.assert_listable(&tla_id, &name, price, &owner_key)?;
        Ok(ext_active_signer::ext(self.active_signer.clone())
            .with_static_gas(GAS_FOR_SIGNER_QUERY)
            .signer_of(sub_account)
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_VERIFY_CALLBACK)
                    .on_listing_verified(tla_id, name, price, owner_key, owner),
            ))
    }

    #[private]
    pub fn on_listing_verified(
        &mut self,
        tla_id: AccountId,
        name: String,
        price: U128,
        owner_key: PublicKey,
        seller: AccountId,
        #[callback_result] current: Result<Option<String>, near_sdk::PromiseError>,
    ) {
        let key = sub_account_key(&tla_id, &name);
        if !current_key_matches(&current, &owner_key) {
            Event::ListingRejected {
                full_name: key,
                seller,
                reason: "owner_key_not_current".to_string(),
            }
            .emit();
            return;
        }
        if self
            .sub_accounts
            .get(&key)
            .map(|s| s.owner.clone())
            .as_ref()
            != Some(&seller)
        {
            Event::ListingRejected {
                full_name: key,
                seller,
                reason: "owner_changed".to_string(),
            }
            .emit();
            return;
        }
        if self.listings.get(&key).map(|l| l.settling).unwrap_or(false) {
            Event::ListingRejected {
                full_name: key,
                seller,
                reason: "settlement_in_progress".to_string(),
            }
            .emit();
            return;
        }
        self.listings.insert(
            key.clone(),
            Listing {
                price: price.0,
                settling: false,
                owner_key,
            },
        );
        Event::SubAccountListed {
            full_name: key,
            price_yocto: price,
            seller,
        }
        .emit();
    }

    #[handle_result]
    #[payable]
    pub fn unlist_sub_account(
        &mut self,
        tla_id: AccountId,
        name: String,
    ) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_not_paused()?;
        validate_name(&name)?;
        let key = sub_account_key(&tla_id, &name);
        self.assert_sale_idle(&key)?;
        if !self.listings.contains_key(&key) {
            return Err(ContractError::NotListed);
        }
        let owner = self.sub_account_owner(&key)?;
        if env::predecessor_account_id() != owner {
            return Err(ContractError::OnlyOwner);
        }
        self.listings.remove(&key);
        Event::SubAccountUnlisted {
            full_name: key,
            by: owner,
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn accept_offer(
        &mut self,
        tla_id: AccountId,
        name: String,
        buyer: AccountId,
        price: U128,
        owner_key: PublicKey,
    ) -> Result<Promise, ContractError> {
        let (sub_account, owner) = self.assert_listable(&tla_id, &name, price, &owner_key)?;
        Ok(ext_active_signer::ext(self.active_signer.clone())
            .with_static_gas(GAS_FOR_SIGNER_QUERY)
            .signer_of(sub_account)
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_VERIFY_CALLBACK)
                    .on_offer_verified(tla_id, name, buyer, price, owner_key, owner),
            ))
    }

    #[allow(clippy::too_many_arguments)]
    #[private]
    pub fn on_offer_verified(
        &mut self,
        tla_id: AccountId,
        name: String,
        buyer: AccountId,
        price: U128,
        owner_key: PublicKey,
        seller: AccountId,
        #[callback_result] current: Result<Option<String>, near_sdk::PromiseError>,
    ) {
        let key = sub_account_key(&tla_id, &name);
        if !current_key_matches(&current, &owner_key) {
            Event::OfferRejected {
                full_name: key,
                buyer,
                seller,
                reason: "owner_key_not_current".to_string(),
            }
            .emit();
            return;
        }
        if self
            .sub_accounts
            .get(&key)
            .map(|s| s.owner.clone())
            .as_ref()
            != Some(&seller)
        {
            Event::OfferRejected {
                full_name: key,
                buyer,
                seller,
                reason: "owner_changed".to_string(),
            }
            .emit();
            return;
        }
        if self
            .accepted_offers
            .get(&key)
            .map(|o| o.settling)
            .unwrap_or(false)
        {
            Event::OfferRejected {
                full_name: key,
                buyer,
                seller,
                reason: "settlement_in_progress".to_string(),
            }
            .emit();
            return;
        }
        self.accepted_offers.insert(
            key.clone(),
            AcceptedOffer {
                buyer: buyer.clone(),
                price: price.0,
                settling: false,
                owner_key,
            },
        );
        Event::OfferAccepted {
            full_name: key,
            buyer,
            price_yocto: price,
            seller,
        }
        .emit();
    }

    #[handle_result]
    #[payable]
    pub fn revoke_offer(&mut self, tla_id: AccountId, name: String) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_not_paused()?;
        validate_name(&name)?;
        let key = sub_account_key(&tla_id, &name);
        self.assert_sale_idle(&key)?;
        if !self.accepted_offers.contains_key(&key) {
            return Err(ContractError::NoAcceptedOffer);
        }
        let owner = self.sub_account_owner(&key)?;
        if env::predecessor_account_id() != owner {
            return Err(ContractError::OnlyOwner);
        }
        self.accepted_offers.remove(&key);
        Event::OfferRevoked {
            full_name: key,
            by: owner,
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn buy_sub_account(
        &mut self,
        tla_id: AccountId,
        name: String,
        new_owner_key: PublicKey,
    ) -> Result<Promise, ContractError> {
        self.assert_not_paused()?;
        validate_name(&name)?;
        if !hos_common::is_ed25519(&new_owner_key) {
            return Err(ContractError::NotEd25519);
        }
        let key = sub_account_key(&tla_id, &name);
        self.assert_sale_idle(&key)?;
        self.assert_sellable(&key, &tla_id)?;
        let buyer = env::predecessor_account_id();
        let deposit = env::attached_deposit().as_yoctonear();
        let (price, owner_key) = self.resolve_and_lock_sale(&key, &buyer, deposit)?;
        let sub_account: AccountId = key
            .parse()
            .map_err(|_| ContractError::InvalidSubAccountId)?;
        let settlement = PendingBuy {
            tla_id,
            name,
            buyer,
            new_owner_key,
            price: U128(price),
            deposit: U128(deposit),
            owner_key,
            sub_account,
        };
        let allowlist: Vec<AccountId> = self.ft_allowlist.iter().cloned().collect();
        let Some(chain) = ft_balance_fanout(&allowlist, &settlement.sub_account) else {
            return Ok(settle_transfer(&self.hos_extension, settlement));
        };
        Ok(chain.then(
            Self::ext(env::current_account_id())
                .with_static_gas(GAS_FOR_BUY_BALANCES_CB)
                .on_buy_balances_checked(settlement, allowlist),
        ))
    }

    #[private]
    pub fn on_sub_account_sold(
        &mut self,
        tla_id: AccountId,
        name: String,
        buyer: AccountId,
        price: U128,
        deposit: U128,
        #[callback_result] swapped: Result<bool, near_sdk::PromiseError>,
    ) {
        let key = sub_account_key(&tla_id, &name);
        if !matches!(swapped, Ok(true)) {
            self.add_pending_refund(&buyer, deposit.0);
            self.clear_settling(&key);
            Event::SubAccountSaleFailed {
                full_name: key,
                buyer,
            }
            .emit();
            return;
        }
        let seller = match self.sub_accounts.get(&key).map(|s| s.owner.clone()) {
            Some(owner) => owner,
            None => {
                self.add_pending_refund(&buyer, deposit.0);
                self.clear_settling(&key);
                return;
            }
        };
        let (commission, seller_proceeds) =
            fees::split_resale(price.0, self.fee_config.resale_commission_bps);
        self.total_revenue = self.total_revenue.saturating_add(commission);
        self.add_pending_refund(&seller, seller_proceeds);
        self.refund_excess(&buyer, deposit.0, price.0);
        if let Some(sub) = self.sub_accounts.get_mut(&key) {
            sub.owner = buyer.clone();
            sub.main_wallet = buyer.clone();
        }
        self.listings.remove(&key);
        self.accepted_offers.remove(&key);
        Event::SubAccountSold {
            full_name: key,
            tla_id,
            seller,
            buyer,
            price_yocto: price,
            commission_yocto: U128(commission),
            seller_proceeds_yocto: U128(seller_proceeds),
        }
        .emit();
    }

    #[private]
    pub fn on_buy_balances_checked(
        &mut self,
        settlement: PendingBuy,
        allowlist: Vec<AccountId>,
    ) -> PromiseOrValue<()> {
        let key = sub_account_key(&settlement.tla_id, &settlement.name);
        if let BalanceGate::Blocked { token, reason } = ft_balances_clear(&allowlist) {
            return self.abort_sale(&key, &settlement, &token, &reason);
        }
        PromiseOrValue::Promise(settle_transfer(&self.hos_extension, settlement))
    }

    pub fn get_listing(&self, tla_id: AccountId, name: String) -> Option<ListingView> {
        let key = sub_account_key(&tla_id, &name);
        self.listings.get(&key).map(|l| ListingView {
            full_name: key.clone(),
            price_yocto: U128(l.price),
            settling: l.settling,
        })
    }

    pub fn get_accepted_offer(&self, tla_id: AccountId, name: String) -> Option<AcceptedOfferView> {
        let key = sub_account_key(&tla_id, &name);
        self.accepted_offers.get(&key).map(|o| AcceptedOfferView {
            full_name: key.clone(),
            buyer: o.buyer.clone(),
            price_yocto: U128(o.price),
            settling: o.settling,
        })
    }
}

impl TlaRegistry {
    fn assert_listable(
        &self,
        tla_id: &AccountId,
        name: &str,
        price: U128,
        owner_key: &PublicKey,
    ) -> Result<(AccountId, AccountId), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_not_paused()?;
        validate_name(name)?;
        if price.0 == 0 {
            return Err(ContractError::InvalidPrice);
        }
        if !hos_common::is_ed25519(owner_key) {
            return Err(ContractError::NotEd25519);
        }
        let key = sub_account_key(tla_id, name);
        self.assert_sale_idle(&key)?;
        let owner = self.assert_sellable(&key, tla_id)?;
        if env::predecessor_account_id() != owner {
            return Err(ContractError::OnlyOwner);
        }
        let sub_account: AccountId = key
            .parse()
            .map_err(|_| ContractError::InvalidSubAccountId)?;
        Ok((sub_account, owner))
    }

    pub(crate) fn assert_sale_idle(&self, key: &str) -> Result<(), ContractError> {
        if let Some(listing) = self.listings.get(key) {
            if listing.settling {
                return Err(ContractError::SaleInProgress);
            }
        }
        if let Some(offer) = self.accepted_offers.get(key) {
            if offer.settling {
                return Err(ContractError::SaleInProgress);
            }
        }
        Ok(())
    }

    fn assert_sellable(&self, key: &str, tla_id: &AccountId) -> Result<AccountId, ContractError> {
        let sub_account: AccountId = key
            .parse()
            .map_err(|_| ContractError::InvalidSubAccountId)?;
        if self
            .mother_use_count
            .get(&sub_account)
            .copied()
            .unwrap_or(0)
            > 0
        {
            return Err(ContractError::SubAccountIsMother);
        }
        if self.signer_pending.contains_key(key) {
            return Err(ContractError::SubAccountNotSellable);
        }
        let sub = self
            .sub_accounts
            .get(key)
            .ok_or(ContractError::SubAccountNotFound)?;
        if sub.tla_id != *tla_id {
            return Err(ContractError::SubAccountTlaMismatch);
        }
        if sub.retraction_at.is_some() {
            return Err(ContractError::RetractionPending);
        }
        let tla = self.tlas.get(tla_id).ok_or(ContractError::TlaNotFound)?;
        if tla.tla_type == TlaType::Business {
            return Err(ContractError::BusinessSubNotResellable);
        }
        if tla.status != TlaStatus::Active {
            return Err(ContractError::SubAccountNotSellable);
        }
        if !matches!(
            effective_sub_lifecycle(
                sub,
                tla,
                self.fee_config.retraction_notice_ns.0,
                self.grace_period_ns,
            ),
            LifecycleStatus::Active
        ) {
            return Err(ContractError::SubAccountNotSellable);
        }
        Ok(sub.owner.clone())
    }

    fn sub_account_owner(&self, key: &str) -> Result<AccountId, ContractError> {
        self.sub_accounts
            .get(key)
            .map(|s| s.owner.clone())
            .ok_or(ContractError::SubAccountNotFound)
    }

    fn resolve_and_lock_sale(
        &mut self,
        key: &str,
        buyer: &AccountId,
        deposit: u128,
    ) -> Result<(u128, PublicKey), ContractError> {
        let offer_terms = self
            .accepted_offers
            .get(key)
            .map(|o| (o.buyer.clone(), o.price, o.owner_key.clone()));
        if let Some((offer_buyer, offer_price, owner_key)) = offer_terms {
            if &offer_buyer == buyer {
                if deposit < offer_price {
                    return Err(ContractError::PriceNotMet);
                }
                if let Some(offer) = self.accepted_offers.get_mut(key) {
                    offer.settling = true;
                }
                return Ok((offer_price, owner_key));
            }
        }
        let listing_terms = self
            .listings
            .get(key)
            .map(|l| (l.price, l.owner_key.clone()));
        if let Some((listing_price, owner_key)) = listing_terms {
            if deposit < listing_price {
                return Err(ContractError::PriceNotMet);
            }
            if let Some(listing) = self.listings.get_mut(key) {
                listing.settling = true;
            }
            return Ok((listing_price, owner_key));
        }
        Err(ContractError::NotListed)
    }

    pub(crate) fn clear_settling(&mut self, key: &str) {
        if let Some(listing) = self.listings.get_mut(key) {
            listing.settling = false;
        }
        if let Some(offer) = self.accepted_offers.get_mut(key) {
            offer.settling = false;
        }
    }

    fn abort_sale(
        &mut self,
        key: &str,
        settlement: &PendingBuy,
        token: &str,
        reason: &str,
    ) -> PromiseOrValue<()> {
        self.add_pending_refund(&settlement.buyer, settlement.deposit.0);
        self.clear_settling(key);
        Event::SubAccountSaleBlocked {
            full_name: key.to_string(),
            token: token.to_string(),
            reason: reason.to_string(),
        }
        .emit();
        PromiseOrValue::Value(())
    }
}

fn current_key_matches(
    current: &Result<Option<String>, near_sdk::PromiseError>,
    owner_key: &PublicKey,
) -> bool {
    match (current, hos_common::ed25519_base58(owner_key)) {
        (Ok(Some(signer)), Some(expected)) => {
            signer.strip_prefix("ed25519:").unwrap_or(signer) == expected
        }
        _ => false,
    }
}

fn settle_transfer(hos_extension: &AccountId, settlement: PendingBuy) -> Promise {
    ext_hos_extension::ext(hos_extension.clone())
        .with_static_gas(GAS_FOR_FORCE_TRANSFER)
        .force_transfer(
            settlement.sub_account.clone(),
            settlement.new_owner_key,
            Some(settlement.owner_key),
        )
        .then(
            TlaRegistry::ext(env::current_account_id())
                .with_static_gas(GAS_FOR_SOLD_CALLBACK)
                .on_sub_account_sold(
                    settlement.tla_id,
                    settlement.name,
                    settlement.buyer,
                    settlement.price,
                    settlement.deposit,
                ),
        )
}
