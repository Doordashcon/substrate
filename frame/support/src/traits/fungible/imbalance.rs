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

//! The imbalance type and it's associates, which handles keeps everything adding up properly with
//! unbalanced operations.

use super::*;
use super::fungibles::Balance;
use super::balanced::Balanced;

pub trait HandleImbalanceDrop<Balance> {
	fn handle(amount: Balance);
}

#[must_use]
pub struct Imbalance<
	B: Balance,
	OnDrop: HandleImbalanceDrop<B>,
	OppositeOnDrop: HandleImbalanceDrop<B>,
> {
	amount: B,
	_phantom: PhantomData<(OnDrop, OppositeOnDrop)>,
}

impl<
	B: Balance,
	OnDrop: HandleImbalanceDrop<B>,
	OppositeOnDrop: HandleImbalanceDrop<B>
> Drop for Imbalance<B, OnDrop, OppositeOnDrop> {
	fn drop(&mut self) {
		if !self.amount.is_zero() {
			OnDrop::handle(self.amount)
		}
	}
}

impl<
	B: Balance,
	OnDrop: HandleImbalanceDrop<B>,
	OppositeOnDrop: HandleImbalanceDrop<B>,
> super::TryDrop for Imbalance<B, OnDrop, OppositeOnDrop> {
	/// Drop an instance cleanly. Only works if its value represents "no-operation".
	fn try_drop(self) -> Result<(), Self> {
		self.drop_zero()
	}
}

impl<
	B: Balance,
	OnDrop: HandleImbalanceDrop<B>,
	OppositeOnDrop: HandleImbalanceDrop<B>,
> Imbalance<B, OnDrop, OppositeOnDrop> {
	pub fn zero() -> Self {
		Self { amount: Zero::zero(), _phantom: PhantomData }
	}

	pub(crate) fn new(amount: B) -> Self {
		Self { amount, _phantom: PhantomData }
	}

	pub fn drop_zero(self) -> Result<(), Self> {
		if self.amount.is_zero() {
			sp_std::mem::forget(self);
			Ok(())
		} else {
			Err(self)
		}
	}

	pub fn split(self, amount: B) -> (Self, Self) {
		let first = self.amount.min(amount);
		let second = self.amount - first;
		sp_std::mem::forget(self);
		(Imbalance::new(first), Imbalance::new(second))
	}
	pub fn merge(mut self, other: Self) -> Self {
		self.amount = self.amount.saturating_add(other.amount);
		sp_std::mem::forget(other);
		self
	}
	pub fn subsume(&mut self, other: Self) {
		self.amount = self.amount.saturating_add(other.amount);
		sp_std::mem::forget(other);
	}
	pub fn offset(self, other: Imbalance<B, OppositeOnDrop, OnDrop>)
		-> UnderOver<Self, Imbalance<B, OppositeOnDrop, OnDrop>>
	{
		let (a, b) = (self.amount, other.amount);
		sp_std::mem::forget((self, other));

		if a == b {
			UnderOver::Exact
		} else if a > b {
			UnderOver::Under(Imbalance::new(a - b))
		} else {
			UnderOver::Over(Imbalance::<B, OppositeOnDrop, OnDrop>::new(b - a))
		}
	}
	pub fn peek(&self) -> B {
		self.amount
	}
}

/// Imbalance implying that the total_issuance value is less than the sum of all account balances.
pub type DebtOf<AccountId, B> = Imbalance<
	<B as Inspect<AccountId>>::Balance,
	// This will generally be implemented by increasing the total_issuance value.
	<B as Balanced<AccountId>>::OnDropDebt,
	<B as Balanced<AccountId>>::OnDropCredit,
>;

/// Imbalance implying that the total_issuance value is greater than the sum of all account balances.
pub type CreditOf<AccountId, B> = Imbalance<
	<B as Inspect<AccountId>>::Balance,
	// This will generally be implemented by decreasing the total_issuance value.
	<B as Balanced<AccountId>>::OnDropCredit,
	<B as Balanced<AccountId>>::OnDropDebt,
>;
