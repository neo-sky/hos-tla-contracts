use crate::error::ContractError;
use crate::events::Event;
use crate::fees;
use crate::interfaces::{ext_ft, ext_hos_extension};
use crate::mother::effective_sub_lifecycle;
use crate::types::*;
use crate::{TlaRegistry, TlaRegistryExt};
use near_sdk::json_types::U128;
use near_sdk::serde::{Deserialize, Serialize};
use near_sdk::{
    env, is_promise_success, near, AccountId, Gas, Promise, PromiseError, PromiseOrValue, PublicKey,
};

const GAS_FOR_FORCE_TRANSFER: Gas = Gas::from_tgas(20);
const GAS_FOR_SOLD_CALLBACK: Gas = Gas::from_tgas(20);
const GAS_FOR_FT_BALANCE: Gas = Gas::from_tgas(5);
const GAS_FOR_BUY_BALANCES_CB: Gas = Gas::from_tgas(60);

const FT_BALANCE_MAX_LEN: usize = 256;

#[derive(Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub struct PendingBuy {
    pub tla_id: AccountId,
    pub name: String,
    pub buyer: AccountId,
    pub new_owner_key: PublicKey,
    pub price: U128,
    pub deposit: U128,
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
    ) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_not_paused()?;
        if price.0 == 0 {
            return Err(ContractError::InvalidPrice);
        }
        let key = sub_account_key(&tla_id, &name);
        self.assert_sale_idle(&key)?;
        let owner = self.assert_sellable(&key, &tla_id)?;
        if env::predecessor_account_id() != owner {
            return Err(ContractError::OnlyOwner);
        }
        self.listings.insert(
            key.clone(),
            Listing {
                price: price.0,
                settling: false,
            },
        );
        Event::SubAccountListed {
            full_name: key,
            price_yocto: price,
            seller: owner,
        }
        .emit();
        Ok(())
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
    ) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_not_paused()?;
        if price.0 == 0 {
            return Err(ContractError::InvalidPrice);
        }
        let key = sub_account_key(&tla_id, &name);
        self.assert_sale_idle(&key)?;
        let owner = self.assert_sellable(&key, &tla_id)?;
        if env::predecessor_account_id() != owner {
            return Err(ContractError::OnlyOwner);
        }
        self.accepted_offers.insert(
            key.clone(),
            AcceptedOffer {
                buyer: buyer.clone(),
                price: price.0,
                settling: false,
            },
        );
        Event::OfferAccepted {
            full_name: key,
            buyer,
            price_yocto: price,
            seller: owner,
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn revoke_offer(&mut self, tla_id: AccountId, name: String) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_not_paused()?;
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
        let key = sub_account_key(&tla_id, &name);
        self.assert_sale_idle(&key)?;
        self.assert_sellable(&key, &tla_id)?;
        let buyer = env::predecessor_account_id();
        let deposit = env::attached_deposit().as_yoctonear();
        let price = self.resolve_and_lock_sale(&key, &buyer, deposit)?;
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
        };
        let allowlist: Vec<AccountId> = self.ft_allowlist.iter().cloned().collect();
        if allowlist.is_empty() {
            return Ok(settle_transfer(
                &self.hos_extension,
                &sub_account,
                settlement,
            ));
        }
        let mut chain = ext_ft::ext(allowlist[0].clone())
            .with_static_gas(GAS_FOR_FT_BALANCE)
            .ft_balance_of(sub_account.clone());
        for ft in allowlist.iter().skip(1) {
            chain = chain.and(
                ext_ft::ext(ft.clone())
                    .with_static_gas(GAS_FOR_FT_BALANCE)
                    .ft_balance_of(sub_account.clone()),
            );
        }
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
    ) {
        let key = sub_account_key(&tla_id, &name);
        if !is_promise_success() {
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
        let excess = deposit.0.saturating_sub(price.0);
        if excess > 0 {
            self.add_pending_refund(&buyer, excess);
        }
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
        if let Some((token, reason)) = sale_block_reason(&allowlist) {
            return self.abort_sale(&key, &settlement, &token, &reason);
        }
        match key.parse::<AccountId>() {
            Ok(sub_account) => PromiseOrValue::Promise(settle_transfer(
                &self.hos_extension,
                &sub_account,
                settlement,
            )),
            Err(_) => self.abort_sale(&key, &settlement, "", "invalid_sub_account_id"),
        }
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
    fn assert_sale_idle(&self, key: &str) -> Result<(), ContractError> {
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
        let sub = self
            .sub_accounts
            .get(key)
            .ok_or(ContractError::SubAccountNotFound)?;
        if sub.retraction_at.is_some() {
            return Err(ContractError::RetractionPending);
        }
        let tla = self.tlas.get(tla_id).ok_or(ContractError::TlaNotFound)?;
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
    ) -> Result<u128, ContractError> {
        let offer_terms = self
            .accepted_offers
            .get(key)
            .map(|o| (o.buyer.clone(), o.price));
        if let Some((offer_buyer, offer_price)) = offer_terms {
            if &offer_buyer == buyer {
                if deposit < offer_price {
                    return Err(ContractError::PriceNotMet);
                }
                if let Some(offer) = self.accepted_offers.get_mut(key) {
                    offer.settling = true;
                }
                return Ok(offer_price);
            }
        }
        let listing_price = self.listings.get(key).map(|l| l.price);
        if let Some(listing_price) = listing_price {
            if deposit < listing_price {
                return Err(ContractError::PriceNotMet);
            }
            if let Some(listing) = self.listings.get_mut(key) {
                listing.settling = true;
            }
            return Ok(listing_price);
        }
        Err(ContractError::NotListed)
    }

    fn clear_settling(&mut self, key: &str) {
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

fn settle_transfer(
    hos_extension: &AccountId,
    sub_account: &AccountId,
    settlement: PendingBuy,
) -> Promise {
    ext_hos_extension::ext(hos_extension.clone())
        .with_static_gas(GAS_FOR_FORCE_TRANSFER)
        .force_transfer(sub_account.clone(), settlement.new_owner_key)
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

fn sale_block_reason(allowlist: &[AccountId]) -> Option<(String, String)> {
    if env::promise_results_count() != allowlist.len() as u64 {
        return Some((String::new(), String::from("result_count_mismatch")));
    }
    for (index, token) in allowlist.iter().enumerate() {
        if let Some(reason) = ft_balance_block_reason(index as u64) {
            return Some((token.as_str().to_string(), reason));
        }
    }
    None
}

fn ft_balance_block_reason(index: u64) -> Option<String> {
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
