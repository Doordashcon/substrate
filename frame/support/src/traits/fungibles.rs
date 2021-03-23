// This file is part of Substrate.

// Copyright (C) 2019-2021 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! The traits for sets of fungible tokens and any associated types.

use super::*;
mod balanced;
mod imbalance;
pub use balanced::{BalancedFungibles, UnbalancedFungibles};
pub use imbalance::{Imbalance, HandleImbalanceDrop, DebtOf, CreditOf};

pub trait AssetId: FullCodec + Copy + Default + Eq + PartialEq {}
impl<T: FullCodec + Copy + Default + Eq + PartialEq> AssetId for T {}

pub trait Balance: AtLeast32BitUnsigned + FullCodec + Copy + Default {}
impl<T: AtLeast32BitUnsigned + FullCodec + Copy + Default> Balance for T {}

/// Trait for providing balance-inspection access to a set of named fungible assets.
pub trait InspectFungibles<AccountId> {
	/// Means of identifying one asset class from another.
	type AssetId: AssetId;
	/// Scalar type for representing balance of an account.
	type Balance: Balance;
	/// The total amount of issuance in the system.
	fn total_issuance(asset: Self::AssetId) -> Self::Balance;
	/// The minimum balance any single account may have.
	fn minimum_balance(asset: Self::AssetId) -> Self::Balance;
	/// Get the `asset` balance of `who`.
	fn balance(asset: Self::AssetId, who: &AccountId) -> Self::Balance;
	/// Returns `true` if the `asset` balance of `who` may be increased by `amount`.
	fn can_deposit(asset: Self::AssetId, who: &AccountId, amount: Self::Balance) -> bool;
	/// Returns `Failed` if the `asset` balance of `who` may not be decreased by `amount`, otherwise
	/// the consequence.
	fn can_withdraw(
		asset: Self::AssetId,
		who: &AccountId,
		amount: Self::Balance,
	) -> WithdrawConsequence<Self::Balance>;
}

/// Trait for providing a set of named fungible assets which can be created and destroyed.
pub trait Fungibles<AccountId>: InspectFungibles<AccountId> {
	/// Increase the `asset` balance of `who` by `amount`.
	fn deposit(asset: Self::AssetId, who: &AccountId, amount: Self::Balance) -> DispatchResult;
	/// Attempt to reduce the `asset` balance of `who` by `amount`.
	fn withdraw(asset: Self::AssetId, who: &AccountId, amount: Self::Balance) -> DispatchResult;
	/// Transfer funds from one account into another.
	fn transfer(
		asset: Self::AssetId,
		source: &AccountId,
		dest: &AccountId,
		amount: Self::Balance,
	) -> DispatchResult {
		if !Self::can_deposit(asset, &dest, amount) {
			return Err(DispatchError::Other("Cannot deposit"))
		}
		Self::withdraw(asset, source, amount)?;
		let result = Self::deposit(asset, dest, amount);
		debug_assert!(result.is_ok(), "can_deposit returned true for a failing deposit!");
		result
	}
}

/// Trait for providing a set of named fungible assets which can only be transferred.
pub trait TransferFungibles<AccountId>: InspectFungibles<AccountId> {
	/// Transfer funds from one account into another.
	fn transfer(
		asset: Self::AssetId,
		source: &AccountId,
		dest: &AccountId,
		amount: Self::Balance,
	) -> DispatchResult;
}

/// Trait for providing a set of named fungible assets which can be reserved.
pub trait ReserveFungibles<AccountId>: InspectFungibles<AccountId> {
	/// Amount of funds held in reserve.
	fn reserved_balance(asset: Self::AssetId, who: &AccountId) -> Self::Balance;

	/// Amount of funds held in reserve.
	fn total_balance(asset: Self::AssetId, who: &AccountId) -> Self::Balance;

	/// Check to see if some `amount` of `asset` may be reserved on the account of `who`.
	fn can_reserve(asset: Self::AssetId, who: &AccountId, amount: Self::Balance) -> bool;

	/// Reserve some funds in an account.
	fn reserve(asset: Self::AssetId, who: &AccountId, amount: Self::Balance) -> DispatchResult;

	/// Unreserve some funds in an account.
	fn unreserve(asset: Self::AssetId, who: &AccountId, amount: Self::Balance) -> DispatchResult;

	/// Transfer reserved funds into another account.
	fn repatriate_reserved(
		asset: Self::AssetId,
		who: &AccountId,
		amount: Self::Balance,
		status: BalanceStatus,
	) -> DispatchResult;
}
