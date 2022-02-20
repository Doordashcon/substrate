//! # Nomination Pools for Staking Delegation
//!
//! This pallet allows delegators to delegate their stake to nominating pools, each of which acts as
//! a nominator and nominates validators on their behalf.
//!
//! ## Design
//!
//! _Notes_: this section uses pseudo code to explain general design and does not necessarily
//! reflect the exact implementation. Additionally, a strong knowledge of `pallet-staking`'s api is
//! assumed.
//!
//! The delegation pool abstraction is composed of:
//!
//!  * bonded pool: Tracks the distribution of actively staked funds. See [`BondedPool`] and
//! [`BondedPoolPoints`]
//! * reward pool: Tracks rewards earned by actively staked funds. See [`RewardPool`] and
//!   [`RewardPools`].
//! * unbonding sub pools: Collection of pools at different phases of the unbonding lifecycle. See
//!   [`SubPools`] and [`SubPoolsStorage`].
//! * delegators: Accounts that are members of pools. See [`Delegator`] and [`Delegators`].
// In order to maintain scalability, all operations are independent of the number of delegators. To
// do this, we store delegation specific information local to the delegator while the pool data
// structures have bounded datum.
//
// ### Design goals
//
// * Maintain integrity of slashing events, sufficiently penalizing delegators that where in the
//   pool while it was backing a validator that got slashed.
// * Maximize scalability in terms of delegator count.
//!
//! ### Bonded pool
//!
//! A bonded pool nominates with its total balance, excluding that which has been withdrawn for
//! unbonding. The total points of a bonded pool are always equal to the sum of points of the
//! delegation members. A bonded pool tracks its points and reads its bonded balance.
//!
//! When a delegator joins a pool, `amount_transferred` is transferred from the delegators account
//! to the bonded pools account. Then the pool calls `staking::bond_extra(amount_transferred)` and
//! issues new points which are tracked by the delegator and added to the bonded pool's points.
//!
//! When the pool already has some balance, we want the value of a point before the transfer to
//! equal the value of a point after the transfer. So, when a delegator joins a bonded pool with a
//! given `amount_transferred`, we maintain the ratio of bonded balance to points such that:
//!
//! ```text
//! balance_after_transfer / points_after_transfer == balance_before_transfer / points_before_transfer;
//! ```
//!
//! To achieve this, we issue points based on the following:
//!
//! ```text
//! points_issued = (points_before_transfer / balance_before_transfer) * amount_transferred;
//! ```
//!
//! For new bonded pools we can set the points issued per balance arbitrarily. In this
//! implementation we use a 1 points to 1 balance ratio for pool creation (see
//! [`POINTS_TO_BALANCE_INIT_RATIO`]).
//!
//! **Relevant extrinsics:**
//!
//! * [`Call::create`]
//! * [`Call::join`]
//!
//! ### Reward pool
//!
//! When a pool is first bonded it sets up an arbitrary account as its reward destination. To track
//! staking rewards we track how the balance of this reward account changes.
//!
//! The reward pool needs to store:
//!
//! * The pool balance at the time of the last payout: `reward_pool.balance`
//! * The total earnings ever at the time of the last payout: `reward_pool.total_earnings`
//! * The total points in the pool at the time of the last payout: `reward_pool.points`
//!
//! And the delegator needs to store:
//!
//! * The total payouts at the time of the last payout by that delegator:
//!   `delegator.reward_pool_total_earnings`
//!
//! Before the first reward claim is initiated for a pool, all the above variables are set to zero.
//!
//! When a delegator initiates a claim, the following happens:
//!
//! 1) Compute the reward pool's total points and the delegator's virtual points in the reward pool
//!     * First `current_total_earnings` is computed (`current_balance` is the free balance of the
//!       reward pool at the beginning of these operations.)
//!			```text
//!			current_total_earnings =
//!       		current_balance - reward_pool.balance + pool.total_earnings;
//!			```
//!     * Then the `current_points` is computed. Every balance unit that was added to the reward
//!       pool since last time recorded means that the `pool.points` is increased by
//!       `bonding_pool.total_points`. In other words, for every unit of balance that has been
//!       earned by the reward pool, the reward pool points are inflated by `bonded_pool.points`. In
//!       effect this allows each, single unit of balance (e.g. planck) to be divvied up pro-rata
//!       among delegators based on points.
//!			```text
//!			new_earnings = current_total_earnings - reward_pool.total_earnings;
//!       	current_points = reward_pool.points + bonding_pool.points * new_earnings;
//!			```
//!     * Finally, the`delegator_virtual_points` are computed: the product of the delegator's points
//!       in the bonding pool and the total inflow of balance units since the last time the
//!       delegator claimed rewards
//!			```text
//!			new_earnings_since_last_claim = current_total_earnings - delegator.reward_pool_total_earnings;
//!        	delegator_virtual_points = delegator.points * new_earnings_since_last_claim;
//!       	```
//! 2) Compute the `delegator_payout`:
//!     ```text
//!     delegator_pool_point_ratio = delegator_virtual_points / current_points;
//!     delegator_payout = current_balance * delegator_pool_point_ratio;
//!     ```
//! 3) Transfer `delegator_payout` to the delegator
//! 4) For the delegator set:
//!     ```text
//!     delegator.reward_pool_total_earnings = current_total_earnings;
//!     ```
//! 5) For the pool set:
//!     ```text
//!     reward_pool.points = current_points - delegator_virtual_points;
//!     reward_pool.balance = current_balance - delegator_payout;
//!     reward_pool.total_earnings = current_total_earnings;
//!     ```
//!
//! _Note_: One short coming of this design is that new joiners can claim rewards for the era after
//! they join even though their funds did not contribute to the pools vote weight. When a
//! delegator joins, it's `reward_pool_total_earnings` field is set equal to the `total_earnings`
//! of the reward pool at that point in time. At best the reward pool has the rewards up through the
//! previous era. If a delegator joins prior to the election snapshot it will benefit from the
//! rewards for the active era despite not contributing to the pool's vote weight. If it joins
//! after the election snapshot is taken it will benefit from the rewards of the next _2_ eras
//! because it's vote weight will not be counted until the election snapshot in active era + 1.
//!
//! **Relevant extrinsics:**
//!
//! * [`Call::claim_payout`]
//!
//! ### Unbonding sub pools
//!
//! When a delegator unbonds, it's balance is unbonded in the bonded pool's account and tracked in
//! an unbonding pool associated with the active era. If no such pool exists, one is created. To
//! track which unbonding sub pool a delegator belongs too, a delegator tracks it's
//! `unbonding_era`.
//!
//! When a delegator initiates unbonding it's claim on the bonded pool
//! (`balance_to_unbond`) is computed as:
//!
//! ```text
//! balance_to_unbond = (bonded_pool.balance / bonded_pool.points) * delegator.points;
//! ```
//!
//! If this is the first transfer into an unbonding pool arbitrary amount of points can be issued
//! per balance. In this implementation unbonding pools are initialized with a 1 point to 1 balance
//! ratio (see [`POINTS_TO_BALANCE_INIT_RATIO`]). Otherwise, the unbonding pools hold the same
//! points to balance ratio properties as the bonded pool, so delegator points in the
//! unbonding pool are issued based on
//!
//! ```text
//! new_points_issued = (points_before_transfer / balance_before_transfer) * balance_to_unbond;
//! ```
//!
//! For scalability, a bound is maintained on the number of unbonding sub pools (see
//! [`TotalUnbondingPools`]). An unbonding pool is removed once its older than `current_era -
//! TotalUnbondingPools`. An unbonding pool is merged into the unbonded pool with
//!
//! ```text
//! unbounded_pool.balance = unbounded_pool.balance + unbonding_pool.balance;
//! unbounded_pool.points = unbounded_pool.points + unbonding_pool.points;
//! ```
//!
//! This scheme "averages" out the points value in the unbonded pool.
//!
//! Once a delgators `unbonding_era` is older than `current_era -
//! [sp_staking::StakingInterface::bonding_duration]`, it can can cash it's points out of the
//! corresponding unbonding pool. If it's `unbonding_era` is older than `current_era -
//! TotalUnbondingPools`, it can cash it's points from the unbonded pool.
//!
//! **Relevant extrinsics:**
//!
//! * [`Call::unbond_other`]
//! * [`Call::withdraw_unbonded_other`]
//!
//! ### Slashing
//!
//! Slashes are distributed evenly across the bonded pool and the unbonding pools from slash era+1
//! through the slash apply era.
//
// Slashes are computed and executed by:
//
// 1) Balances of the bonded pool and the unbonding pools in range `slash_era +
// 1..=apply_era` are summed and stored in `total_balance_affected`.
// 2) `slash_ratio` is computed as `slash_amount / total_balance_affected`.
// 3) `bonded_pool_balance_after_slash`is computed as `(1- slash_ratio) * bonded_pool_balance`.
// 4) For all `unbonding_pool` in range `slash_era + 1..=apply_era` set their balance to `(1 -
// slash_ratio) * unbonding_pool_balance`.
//
// Unbonding pools need to be slashed to ensure all nominators whom where in the bonded pool
// while it was backing a validator that equivocated are punished. Without these measures a
// nominator could unbond right after a validator equivocated with no consequences.
//
// This strategy is unfair to delegators who joined after the slash, because they get slashed as
// well, but spares delegators who unbond. The latter is much more important for security: if a
// pool's validators are attacking the network, their delegators need to unbond fast! Avoiding
// slashes gives them an incentive to do that if validators get repeatedly slashed.
//
// To be fair to joiners, this implementation also need joining pools, which are actively staking,
// in addition to the unbonding pools. For maintenance simplicity these are not implemented.
//!
//! ### Pool administration
//!
//! To help facilitate pool adminstration the pool has one of three states (see [`PoolState`]):
//!
//! * Open: Anyone can join the pool and no delegators can be permissionlessly removed.
//! * Blocked: No delegators can join and some admin roles can kick delegators.
//! * Destroying: No delegators can join and all delegators can be permissionlessly removed. Once a
//!   pool is destroying state, it cannot be reverted to another state.
//!
//! A pool has 3 administrative positions (see [`BondedPool`]):
//!
//! * Depositor: creates the pool and is the initial delegator. The can only leave pool once all
//!   other delegators have left. Once they fully leave the pool is destroyed.
//! * Nominator: can select which validators the pool nominates.
//! * State-Toggler: can change the pools state and kick delegators if the pool is blocked.
//! * Root: can change the nominator, state-toggler, or itself and can perform any of the actions
//!   the nominator or state-toggler can.
//!
//! Note: if it is desired that any of the admin roles are not accessible, they can be set to an
//! anonymous proxy account that has no proxies (and is thus provably keyless).
//!
//! **Relevant extrinsics:**
//!
//! * [`Call::create`]
//! * [`Call::nominate`]
//! * [`Call::unbond_other`]
//! * [`Call::withdraw_unbonded_other`]
//!
//! ### Limitations
//!
//! * Delegators cannot vote with their staked funds because they are transferred into the pools
//!   account. In the future this can be overcome by allowing the delegators to vote with their
//!   bonded funds via vote splitting.
//! * Delegators cannot quickly transfer to another pool if they do no like nominations, instead
//!   they must wait for the unbonding duration.
//!
//! # Runtime builder warnings
//!
//! * watch out for overflow of [`RewardPoints`] and [`BalanceOf`] types. Consider things like the
//!   chains total issuance, staking reward rate, and burn rate.
//
// Invariants
// * A `delegator.pool` must always be a valid entry in `RewardPools`, and `BondedPoolPoints`.
// * Every entry in `BondedPoolPoints` must have  a corresponding entry in `RewardPools`
// * If a delegator unbonds, the sub pools should always correctly track slashses such that the
//   calculated amount when withdrawing unbonded is a lower bound of the pools free balance.
// * If the depositor is actively unbonding, the pool is in destroying state. To achieve this, once
//   a pool is flipped to a destroying state it cannot change its state.

// TODO
// - Write user top level docs and make the design docs internal
// - Refactor staking slashing to always slash unlocking chunks (then back port)
// - backport making ledger generic over ^^ IDEA: maybe staking can slash unlocking chunks, and then
//   pools is passed the updated unlocking chunks and makes updates based on that
// - benchmarks
// - make staking interface current era
// - staking provider current era should not return option, just era index
// - write detailed docs for StakingInterface

#![cfg_attr(not(feature = "std"), no_std)]

use frame_support::{
	ensure,
	pallet_prelude::*,
	storage::bounded_btree_map::BoundedBTreeMap,
	traits::{Currency, DefensiveOption, DefensiveResult, ExistenceRequirement, Get},
	DefaultNoBound, RuntimeDebugNoBound,
};
use scale_info::TypeInfo;
use sp_core::U256;
use sp_io::hashing::blake2_256;
use sp_runtime::traits::{Bounded, Convert, Saturating, StaticLookup, TrailingZeroInput, Zero};
use sp_staking::{EraIndex, PoolsInterface, SlashPoolArgs, SlashPoolOut, StakingInterface};
use sp_std::{collections::btree_map::BTreeMap, ops::Div, vec::Vec};

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking;

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;
pub mod weights;

pub use pallet::*;
pub use weights::WeightInfo;

type BalanceOf<T> =
	<<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;
type SubPoolsWithEra<T> = BoundedBTreeMap<EraIndex, UnbondPool<T>, TotalUnbondingPools<T>>;
// NOTE: this assumes the balance type u128 or smaller.
type RewardPoints = U256;

const POINTS_TO_BALANCE_INIT_RATIO: u32 = 1;

/// Calculate the number of points to issue from a pool as `(current_points / current_balance) *
/// new_funds` except for some zero edge cases; see logic and tests for details.
fn points_to_issue<T: Config>(
	current_balance: BalanceOf<T>,
	current_points: BalanceOf<T>,
	new_funds: BalanceOf<T>,
) -> BalanceOf<T> {
	match (current_balance.is_zero(), current_points.is_zero()) {
		(true, true) | (false, true) =>
			new_funds.saturating_mul(POINTS_TO_BALANCE_INIT_RATIO.into()),
		(true, false) => {
			// The pool was totally slashed.
			// This is the equivalent of `(current_points / 1) * new_funds`.
			new_funds.saturating_mul(current_points)
		},
		(false, false) => {
			// Equivalent to (current_points / current_balance) * new_funds
			current_points
				.saturating_mul(new_funds)
				// We check for zero above
				.div(current_balance)
		},
	}
}

// Calculate the balance of a pool to unbond as `(current_balance / current_points) *
// delegator_points`. Returns zero if any of the inputs are zero.
fn balance_to_unbond<T: Config>(
	current_balance: BalanceOf<T>,
	current_points: BalanceOf<T>,
	delegator_points: BalanceOf<T>,
) -> BalanceOf<T> {
	if current_balance.is_zero() || current_points.is_zero() || delegator_points.is_zero() {
		// There is nothing to unbond
		return Zero::zero()
	}

	// Equivalent of (current_balance / current_points) * delegator_points
	current_balance
		.saturating_mul(delegator_points)
		// We check for zero above
		.div(current_points)
}

#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, RuntimeDebugNoBound)]
#[cfg_attr(feature = "std", derive(Clone, PartialEq))]
#[codec(mel_bound(T: Config))]
#[scale_info(skip_type_params(T))]
pub struct Delegator<T: Config> {
	pool: T::AccountId,
	/// The quantity of points this delegator has in the bonded pool or in a sub pool if
	/// `Self::unbonding_era` is some.
	points: BalanceOf<T>,
	/// The reward pools total earnings _ever_ the last time this delegator claimed a payout.
	/// Assuming no massive burning events, we expect this value to always be below total issuance.
	/// This value lines up with the `RewardPool::total_earnings` after a delegator claims a
	/// payout.
	reward_pool_total_earnings: BalanceOf<T>,
	/// The era this delegator started unbonding at.
	unbonding_era: Option<EraIndex>,
}

/// All of a pool's possible states.
#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, PartialEq, RuntimeDebugNoBound)]
#[cfg_attr(feature = "std", derive(Clone))]
pub enum PoolState {
	Open = 0,
	Blocked = 1,
	Destroying = 2,
}

/// Pool permissions and state
#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, RuntimeDebugNoBound, PartialEq)]
#[cfg_attr(feature = "std", derive(Clone))]
#[codec(mel_bound(T: Config))]
#[scale_info(skip_type_params(T))]
struct BondedPoolStorage<T: Config> {
	points: BalanceOf<T>,
	/// See [`BondedPool::depositor`].
	depositor: T::AccountId,
	/// See [`BondedPool::admin`].
	root: T::AccountId,
	/// See [`BondedPool::nominator`].
	nominator: T::AccountId,
	/// See [`BondedPool::state_toggler`].
	state_toggler: T::AccountId,
	/// See [`BondedPool::state_toggler`].
	state: PoolState,
}

#[derive(RuntimeDebugNoBound)]
#[cfg_attr(feature = "std", derive(Clone, PartialEq))]
pub struct BondedPool<T: Config> {
	/// Points of the pool.
	points: BalanceOf<T>,
	/// Account that puts down a deposit to create the pool. This account acts a delegator, but can
	/// only unbond if no other delegators belong to the pool.
	depositor: T::AccountId,
	/// Can perform the same actions as [`Self::nominator`] and [`Self::state_toggler`].
	/// Additionally, this account can set the `nominator` and `state_toggler` at any time.
	root: T::AccountId,
	/// Can set the pool's nominations at any time.
	nominator: T::AccountId,
	/// Can toggle the pools state, including setting the pool as blocked or putting the pool into
	/// destruction mode. The state toggle can also "kick" delegators by unbonding them.
	state_toggler: T::AccountId,
	/// State of the pool.
	state: PoolState,
	/// AccountId of the pool.
	account: T::AccountId,
}

impl<T: Config> BondedPool<T> {
	/// Get [`Self`] from storage. Returns `None` if no entry for `pool_account` exists.
	fn get(pool_account: &T::AccountId) -> Option<Self> {
		BondedPools::<T>::try_get(pool_account).ok().map(
			|BondedPoolStorage { points, depositor, root, nominator, state_toggler, state }| Self {
				points,
				depositor,
				root,
				nominator,
				state_toggler,
				state,
				account: pool_account.clone(),
			},
		)
	}

	/// Consume and put [`Self`] into storage.
	fn put(self) {
		let Self { account, points, depositor, root, nominator, state_toggler, state } = self;
		BondedPools::<T>::insert(
			account,
			BondedPoolStorage { points, depositor, root, nominator, state_toggler, state },
		);
	}

	/// Consume and remove [`Self`] from storage.
	fn remove(self) {
		BondedPools::<T>::remove(self.account);
	}

	/// Get the amount of points to issue for some new funds that will be bonded in the pool.
	fn points_to_issue(&self, new_funds: BalanceOf<T>) -> BalanceOf<T> {
		let bonded_balance =
			T::StakingInterface::bonded_balance(&self.account).unwrap_or(Zero::zero());
		points_to_issue::<T>(bonded_balance, self.points, new_funds)
	}

	/// Get the amount of balance to unbond from the pool based on a delegator's points of the pool.
	fn balance_to_unbond(&self, delegator_points: BalanceOf<T>) -> BalanceOf<T> {
		let bonded_balance =
			T::StakingInterface::bonded_balance(&self.account).unwrap_or(Zero::zero());
		balance_to_unbond::<T>(bonded_balance, self.points, delegator_points)
	}

	/// Issue points to [`Self`] for `new_funds`.
	fn issue(&mut self, new_funds: BalanceOf<T>) -> BalanceOf<T> {
		let points_to_issue = self.points_to_issue(new_funds);
		self.points = self.points.saturating_add(points_to_issue);

		points_to_issue
	}

	/// Check that the pool can accept a member with `new_funds`.
	fn ok_to_join_with(&self, new_funds: BalanceOf<T>) -> Result<(), DispatchError> {
		ensure!(self.state == PoolState::Open, Error::<T>::NotOpen);
		let bonded_balance =
			T::StakingInterface::bonded_balance(&self.account).unwrap_or(Zero::zero());
		ensure!(!bonded_balance.is_zero(), Error::<T>::OverflowRisk);

		let points_to_balance_ratio_floor = self
			.points
			// We checked for zero above
			.div(bonded_balance);

		// TODO make sure these checks make sense. Taken from staking design chat with Al

		// Pool points can inflate relative to balance, but only if the pool is slashed.
		//
		// If we cap the ratio of points:balance so one cannot join a pool that has been slashed
		// 90%,
		ensure!(
			points_to_balance_ratio_floor < T::PoolSizeMax::get().into(),
			Error::<T>::OverflowRisk
		);
		// while restricting the balance to 1/10th of max total issuance,
		ensure!(
			new_funds.saturating_add(bonded_balance) <
				BalanceOf::<T>::max_value().div(T::PoolSizeMax::get().into()),
			Error::<T>::OverflowRisk
		);
		// then we can be decently confident the bonding pool points will not overflow
		// `BalanceOf<T>`.
		Ok(())
	}

	fn can_nominate(&self, who: &T::AccountId) -> bool {
		*who == self.root || *who == self.nominator
	}

	fn can_kick(&self, who: &T::AccountId) -> bool {
		(*who == self.root || *who == self.state_toggler) && self.state == PoolState::Blocked
	}

	fn is_destroying(&self) -> bool {
		self.state == PoolState::Destroying
	}

	fn ok_to_unbond_other_with(
		&self,
		caller: &T::AccountId,
		target_account: &T::AccountId,
		target_delegator: &Delegator<T>,
	) -> Result<(), DispatchError> {
		let is_permissioned = caller == target_account;
		let is_depositor = *target_account == self.depositor;
		match (is_permissioned, is_depositor) {
			// If the pool is blocked, then an admin with kicking permissions can remove a
			// delegator. If the pool is being destroyed, anyone can remove a delegator
			(false, false) => {
				ensure!(
					self.can_kick(caller) || self.is_destroying(),
					Error::<T>::NotKickerOrDestroying
				)
			},
			// Any delegator who is not the depositor can always unbond themselves
			(true, false) => (),
			// The depositor can only start unbonding if the pool is already being destroyed and
			// they are the delegator in the pool. Note that an invariant is once the pool is
			// destroying it cannot switch states, so by being in destroying we are guaranteed no
			// other delegators can possibly join.
			(false, true) | (true, true) => {
				ensure!(target_delegator.points == self.points, Error::<T>::NotOnlyDelegator);
				ensure!(self.is_destroying(), Error::<T>::NotDestroying);
			},
		}
		Ok(())
	}

	/// Returns a result indicating if `Call::withdraw_unbonded_other` can be executed.
	fn ok_to_withdraw_unbonded_other_with(
		&self,
		caller: &T::AccountId,
		target_account: &T::AccountId,
		target_delegator: &Delegator<T>,
		sub_pools: &SubPools<T>,
	) -> Result<bool, DispatchError> {
		if *target_account == self.depositor {
			// This is a depositor
			if !sub_pools.no_era.points.is_zero() {
				// Unbonded pool has some points, so if they are the last delegator they must be
				// here
				// Since the depositor is the last to unbond, this should never be possible
				ensure!(sub_pools.with_era.len().is_zero(), Error::<T>::NotOnlyDelegator);
				ensure!(
					sub_pools.no_era.points == target_delegator.points,
					Error::<T>::NotOnlyDelegator
				);
			} else {
				// No points in the `no_era` pool, so they must be in a `with_era` pool
				// If there are no other delegators, this can be the only `with_era` pool since the
				// depositor was the last to withdraw. This assumes with_era sub pools are destroyed
				// whenever their points go to zero.
				ensure!(sub_pools.with_era.len() == 1, Error::<T>::NotOnlyDelegator);
				sub_pools
					.with_era
					.values()
					.next()
					.filter(|only_unbonding_pool| {
						only_unbonding_pool.points == target_delegator.points
					})
					.ok_or(Error::<T>::NotOnlyDelegator)?;
			}
			Ok(true)
		} else {
			// This isn't a depositor
			let is_permissioned = caller == target_account;
			ensure!(
				is_permissioned || self.can_kick(caller) || self.is_destroying(),
				Error::<T>::NotKickerOrDestroying
			);
			Ok(false)
		}
	}
}

#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, RuntimeDebugNoBound)]
#[cfg_attr(feature = "std", derive(Clone, PartialEq))]
#[codec(mel_bound(T: Config))]
#[scale_info(skip_type_params(T))]
pub struct RewardPool<T: Config> {
	/// The reward destination for the pool.
	account: T::AccountId,
	/// The balance of this reward pool after the last claimed payout.
	balance: BalanceOf<T>,
	/// The total earnings _ever_ of this reward pool after the last claimed payout. I.E. the sum
	/// of all incoming balance through the pools life.
	///
	/// NOTE: We assume this will always be less than total issuance and thus can use the runtimes
	/// `Balance` type. However in a chain with a burn rate higher than the rate this increases,
	/// this type should be bigger than `Balance`.
	total_earnings: BalanceOf<T>,
	/// The total points of this reward pool after the last claimed payout.
	points: RewardPoints,
}

impl<T: Config> RewardPool<T> {
	/// Mutate the reward pool by updating the total earnings and current free balance.
	fn update_total_earnings_and_balance(&mut self) {
		let current_balance = T::Currency::free_balance(&self.account);
		// The earnings since the last time it was updated
		let new_earnings = current_balance.saturating_sub(self.balance);
		// The lifetime earnings of the of the reward pool
		self.total_earnings = new_earnings.saturating_add(self.total_earnings);
		self.balance = current_balance;
	}
}

#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, DefaultNoBound, RuntimeDebugNoBound)]
#[cfg_attr(feature = "std", derive(Clone, PartialEq))]
#[codec(mel_bound(T: Config))]
#[scale_info(skip_type_params(T))]
struct UnbondPool<T: Config> {
	points: BalanceOf<T>,
	balance: BalanceOf<T>,
}

impl<T: Config> UnbondPool<T> {
	fn points_to_issue(&self, new_funds: BalanceOf<T>) -> BalanceOf<T> {
		points_to_issue::<T>(self.balance, self.points, new_funds)
	}

	fn balance_to_unbond(&self, delegator_points: BalanceOf<T>) -> BalanceOf<T> {
		balance_to_unbond::<T>(self.balance, self.points, delegator_points)
	}

	/// Issue points and update the balance given `new_balance`.
	fn issue(&mut self, new_funds: BalanceOf<T>) {
		self.points = self.points.saturating_add(self.points_to_issue(new_funds));
		self.balance = self.balance.saturating_add(new_funds);
	}
}

#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, DefaultNoBound, RuntimeDebugNoBound)]
#[cfg_attr(feature = "std", derive(Clone, PartialEq))]
#[codec(mel_bound(T: Config))]
#[scale_info(skip_type_params(T))]
struct SubPools<T: Config> {
	/// A general, era agnostic pool of funds that have fully unbonded. The pools
	/// of `Self::with_era` will lazily be merged into into this pool if they are
	/// older then `current_era - TotalUnbondingPools`.
	no_era: UnbondPool<T>,
	/// Map of era => unbond pools.
	with_era: SubPoolsWithEra<T>,
}

impl<T: Config> SubPools<T> {
	/// Merge the oldest unbonding pool with an era into the general unbond pool with no associated
	/// era.
	fn maybe_merge_pools(mut self, current_era: EraIndex) -> Self {
		if current_era < TotalUnbondingPools::<T>::get().into() {
			// For the first `0..TotalUnbondingPools` eras of the chain we don't need to do
			// anything. I.E. if `TotalUnbondingPools` is 5 and we are in era 4 we can add a pool
			// for this era and have exactly `TotalUnbondingPools` pools.
			return self
		}

		//  I.E. if `TotalUnbondingPools` is 5 and current era is 10, we only want to retain pools
		// 6..=10.
		let newest_era_to_remove = current_era.saturating_sub(TotalUnbondingPools::<T>::get());

		let eras_to_remove: Vec<_> = self
			.with_era
			.keys()
			.cloned()
			.filter(|era| *era <= newest_era_to_remove)
			.collect();
		for era in eras_to_remove {
			if let Some(p) = self.with_era.remove(&era) {
				self.no_era.points = self.no_era.points.saturating_add(p.points);
				self.no_era.balance = self.no_era.balance.saturating_add(p.balance);
			}
		}

		self
	}

	/// Get the unbond pool for `era`. If one does not exist a default entry will be inserted.
	///
	/// The caller must ensure that the `SubPools::with_era` has room for 1 more entry. Calling
	/// [`SubPools::maybe_merge_pools`] with the current era should the sub pools are in an ok state
	/// to call this method.
	fn unchecked_with_era_get_or_make(&mut self, era: EraIndex) -> &mut UnbondPool<T> {
		if !self.with_era.contains_key(&era) {
			self.with_era
				.try_insert(era, UnbondPool::default())
				.expect("caller has checked pre-conditions. qed.");
		}

		self.with_era.get_mut(&era).expect("entry inserted on the line above. qed.")
	}
}

/// The maximum amount of eras an unbonding pool can exist prior to being merged with the
/// `no_era	 pool. This is guaranteed to at least be equal to the staking `UnbondingDuration`. For
/// improved UX [`Config::PostUnbondingPoolsWindow`] should be configured to a non-zero value.
struct TotalUnbondingPools<T: Config>(PhantomData<T>);
impl<T: Config> Get<u32> for TotalUnbondingPools<T> {
	fn get() -> u32 {
		// TODO: This may be too dangerous in the scenario bonding_duration gets decreased because
		// we would no longer be able to decode `SubPoolsWithEra`, which uses `TotalUnbondingPools`
		// as the bound
		T::StakingInterface::bonding_duration() + T::PostUnbondingPoolsWindow::get()
	}
}

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_system::{ensure_signed, pallet_prelude::*};

	#[pallet::pallet]
	#[pallet::generate_store(pub(crate) trait Store)]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config {
		/// The overarching event type.
		type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;

		/// Weight information for extrinsics in this pallet.
		type WeightInfo: weights::WeightInfo;

		/// The nominating balance.
		type Currency: Currency<Self::AccountId>;

		// Infallible method for converting `Currency::Balance` to `U256`.
		type BalanceToU256: Convert<BalanceOf<Self>, U256>;

		// Infallible method for converting `U256` to `Currency::Balance`.
		type U256ToBalance: Convert<U256, BalanceOf<Self>>;

		/// The interface for nominating.
		type StakingInterface: StakingInterface<
			Balance = BalanceOf<Self>,
			AccountId = Self::AccountId,
			LookupSource = <Self::Lookup as StaticLookup>::Source,
		>;

		/// The amount of eras a `SubPools::with_era` pool can exist before it gets merged into the
		/// `SubPools::no_era` pool. In other words, this is the amount of eras a delegator will be
		/// able to withdraw from an unbonding pool which is guaranteed to have the correct ratio of
		/// points to balance; once the `with_era` pool is merged into the `no_era` pool, the ratio
		/// can become skewed due to some slashed ratio getting merged in at some point.
		type PostUnbondingPoolsWindow: Get<u32>;

		#[pallet::constant]
		type PoolSizeMax: Get<u32>;
	}

	/// Minimum amount to bond to join a pool.
	#[pallet::storage]
	pub(crate) type MinJoinBond<T: Config> = StorageValue<_, BalanceOf<T>, ValueQuery>;

	/// Minimum bond required to create a pool.
	#[pallet::storage]
	pub(crate) type MinCreateBond<T: Config> = StorageValue<_, BalanceOf<T>, ValueQuery>;

	/// Maximum number of nomination pools that can exist. If `None`, then an unbounded number of
	/// pools can exist.
	#[pallet::storage]
	pub(crate) type MaxPools<T: Config> = StorageValue<_, u32, OptionQuery>;

	/// Active delegators.
	#[pallet::storage]
	pub(crate) type Delegators<T: Config> =
		CountedStorageMap<_, Twox64Concat, T::AccountId, Delegator<T>>;

	/// To get or insert a pool see [`BondedPool::get`] and [`BondedPool::put`]
	#[pallet::storage]
	pub(crate) type BondedPools<T: Config> =
		CountedStorageMap<_, Twox64Concat, T::AccountId, BondedPoolStorage<T>>;

	/// Reward pools. This is where there rewards for each pool accumulate. When a delegators payout
	/// is claimed, the balance comes out fo the reward pool. Keyed by the bonded pools
	/// _Stash_/_Controller_.
	#[pallet::storage]
	pub(crate) type RewardPools<T: Config> =
		CountedStorageMap<_, Twox64Concat, T::AccountId, RewardPool<T>>;

	/// Groups of unbonding pools. Each group of unbonding pools belongs to a bonded pool,
	/// hence the name sub-pools. Keyed by the bonded pools _Stash_/_Controller_.
	#[pallet::storage]
	pub(crate) type SubPoolsStorage<T: Config> =
		CountedStorageMap<_, Twox64Concat, T::AccountId, SubPools<T>>;

	#[pallet::genesis_config]
	pub struct GenesisConfig<T: Config> {
		pub min_join_bond: BalanceOf<T>,
		pub min_create_bond: BalanceOf<T>,
		pub max_pools: Option<u32>,
	}

	#[cfg(feature = "std")]
	impl<T: Config> Default for GenesisConfig<T> {
		fn default() -> Self {
			Self {
				min_join_bond: Zero::zero(),
				min_create_bond: Zero::zero(),
				max_pools: Some(T::PoolSizeMax::get()),
			}
		}
	}

	#[pallet::genesis_build]
	impl<T: Config> GenesisBuild<T> for GenesisConfig<T> {
		fn build(&self) {
			MinJoinBond::<T>::put(self.min_join_bond);
			MinCreateBond::<T>::put(self.min_create_bond);
			if let Some(max_pools) = self.max_pools {
				MaxPools::<T>::put(max_pools);
			}
		}
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(crate) fn deposit_event)]
	pub enum Event<T: Config> {
		Joined { delegator: T::AccountId, pool: T::AccountId, bonded: BalanceOf<T> },
		PaidOut { delegator: T::AccountId, pool: T::AccountId, payout: BalanceOf<T> },
		Unbonded { delegator: T::AccountId, pool: T::AccountId, amount: BalanceOf<T> },
		Withdrawn { delegator: T::AccountId, pool: T::AccountId, amount: BalanceOf<T> },
		DustWithdrawn { delegator: T::AccountId, pool: T::AccountId },
	}

	#[pallet::error]
	#[cfg_attr(test, derive(PartialEq))]
	pub enum Error<T> {
		/// A (bonded) pool id does not exist.
		PoolNotFound,
		/// An account is not a delegator.
		DelegatorNotFound,
		/// A reward pool does not exist. In all cases this is a system logic error.
		RewardPoolNotFound,
		/// A sub pool does not exist.
		SubPoolsNotFound,
		/// An account is already delegating in another pool. An account may only belong to one
		/// pool at a time.
		AccountBelongsToOtherPool,
		/// The pool has insufficient balance to bond as a nominator.
		InsufficientBond,
		/// The delegator is already unbonding.
		AlreadyUnbonding,
		/// The delegator is not unbonding and thus cannot withdraw funds.
		NotUnbonding,
		/// Unbonded funds cannot be withdrawn yet because the bond duration has not passed.
		NotUnbondedYet,
		/// The amount does not meet the minimum bond to either join or create a pool.
		MinimumBondNotMet,
		/// The transaction could not be executed due to overflow risk for the pool.
		OverflowRisk,
		// Likely only an error ever encountered in poorly built tests.
		/// A pool with the generated account id already exists.
		IdInUse,
		/// A pool must be in [`PoolState::Destroying`] in order for the depositor to unbond or for
		/// other delegators to be permissionlessly unbonded.
		NotDestroying,
		/// The depositor must be the only delegator in the bonded pool in order to unbond. And the
		/// depositor must be the only delegator in the sub pools in order to withdraw unbonded.
		NotOnlyDelegator,
		/// The caller does not have nominating permissions for the pool.
		NotNominator,
		/// Either a) the caller cannot make a valid kick or b) the pool is not destroying
		NotKickerOrDestroying,
		/// The pool is not open to join
		NotOpen,
		/// The system is maxed out on pools.
		MaxPools,
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Join a pre-existing pool.
		///
		/// Notes
		/// * an account can only be a member of a single pool.
		/// * this will *not* dust the delegator account, so the delegator must have at least
		///   `existential deposit + amount` in their account.
		/// * Only a pool with [`PoolState::Open`] can be joined
		#[pallet::weight(666)]
		#[frame_support::transactional]
		pub fn join(
			origin: OriginFor<T>,
			amount: BalanceOf<T>,
			pool_account: T::AccountId,
		) -> DispatchResult {
			let who = ensure_signed(origin)?;
			ensure!(amount >= MinJoinBond::<T>::get(), Error::<T>::MinimumBondNotMet);
			// If a delegator already exists that means they already belong to a pool
			ensure!(!Delegators::<T>::contains_key(&who), Error::<T>::AccountBelongsToOtherPool);

			let mut bonded_pool =
				BondedPool::<T>::get(&pool_account).ok_or(Error::<T>::PoolNotFound)?;
			bonded_pool.ok_to_join_with(amount)?;

			// We don't actually care about writing the reward pool, we just need its
			// total earnings at this point in time.
			let mut reward_pool = RewardPools::<T>::get(&pool_account)
				.defensive_ok_or_else(|| Error::<T>::RewardPoolNotFound)?;
			// This is important because we want the most up-to-date total earnings.
			reward_pool.update_total_earnings_and_balance();

			// Transfer the funds to be bonded from `who` to the pools account so the pool can then
			// go bond them.
			T::Currency::transfer(&who, &pool_account, amount, ExistenceRequirement::KeepAlive)?;
			// We must calculate the points to issue *before* we bond `who`'s funds, else the
			// points:balance ratio will be wrong.
			let new_points = bonded_pool.issue(amount);
			// The pool should always be created in such a way its in a state to bond extra, but if
			// the active balance is slashed below the minimum bonded or the account cannot be
			// found, we exit early.
			T::StakingInterface::bond_extra(pool_account.clone(), amount)?;

			Delegators::insert(
				who.clone(),
				Delegator::<T> {
					pool: pool_account.clone(),
					points: new_points,
					// At best the reward pool has the rewards up through the previous era. If the
					// delegator joins prior to the snapshot they will benefit from the rewards of
					// the active era despite not contributing to the pool's vote weight. If they
					// join after the snapshot is taken they will benefit from the rewards of the
					// next 2 eras because their vote weight will not be counted until the
					// snapshot in active era + 1.
					reward_pool_total_earnings: reward_pool.total_earnings,
					unbonding_era: None,
				},
			);
			bonded_pool.put();
			Self::deposit_event(Event::<T>::Joined {
				delegator: who,
				pool: pool_account,
				bonded: amount,
			});

			Ok(())
		}

		/// A bonded delegator can use this to claim their payout based on the rewards that the pool
		/// has accumulated since their last claimed payout (OR since joining if this is there first
		/// time claiming rewards).
		///
		/// Note that the payout will go to the delegator's account.
		#[pallet::weight(T::WeightInfo::claim_payout())]
		pub fn claim_payout(origin: OriginFor<T>) -> DispatchResult {
			let who = ensure_signed(origin)?;
			let delegator = Delegators::<T>::get(&who).ok_or(Error::<T>::DelegatorNotFound)?;
			let bonded_pool = BondedPool::<T>::get(&delegator.pool)
				.defensive_ok_or_else(|| Error::<T>::PoolNotFound)?;

			Self::do_reward_payout(who, delegator, &bonded_pool)?;

			Ok(())
		}

		/// Unbond _all_ of the `target` delegators funds from the pool. Under certain conditions,
		/// this call can be dispatched permissionlessly (i.e. by any account).
		///
		/// Conditions for a permissionless dispatch:
		///
		/// - The pool is blocked and the caller is either the root or state-toggler. This is
		///   refereed to as a kick.
		/// - The pool is destroying and the delegator is not the depositor.
		/// - The pool is destroying, the delegator is the depositor and no other delegators are in
		///   the pool.
		///
		/// Conditions for permissioned dispatch (i.e. the caller is also the target):
		///
		/// - The caller is not the depositor
		/// - The caller is the depositor, the pool is destroying and not other delegators are in
		///   the pool.
		///
		/// Note: If their are too many unlocking chunks to unbond with the pool account,
		/// [`Self::withdraw_unbonded_pool`] can be called to try and minimize unlocking chunks.
		#[pallet::weight(666)]
		pub fn unbond_other(origin: OriginFor<T>, target: T::AccountId) -> DispatchResult {
			let caller = ensure_signed(origin)?;
			let delegator = Delegators::<T>::get(&target).ok_or(Error::<T>::DelegatorNotFound)?;
			let mut bonded_pool = BondedPool::<T>::get(&delegator.pool)
				.defensive_ok_or_else(|| Error::<T>::PoolNotFound)?;
			bonded_pool.ok_to_unbond_other_with(&caller, &target, &delegator)?;

			// Claim the the payout prior to unbonding. Once the user is unbonding their points
			// no longer exist in the bonded pool and thus they can no longer claim their payouts.
			// It is not strictly necessary to claim the rewards, but we do it here for UX.
			Self::do_reward_payout(target.clone(), delegator, &bonded_pool)?;

			// Re-fetch the delegator because they where updated by `do_reward_payout`.
			let mut delegator =
				Delegators::<T>::get(&target).ok_or(Error::<T>::DelegatorNotFound)?;
			// Note that we lazily create the unbonding pools here if they don't already exist
			let sub_pools = SubPoolsStorage::<T>::get(&delegator.pool).unwrap_or_default();
			let current_era = T::StakingInterface::current_era().unwrap_or(Zero::zero());

			let balance_to_unbond = bonded_pool.balance_to_unbond(delegator.points);

			// Update the bonded pool. Note that we must do this *after* calculating the balance
			// to unbond so we have the correct points for the balance:share ratio.
			bonded_pool.points = bonded_pool.points.saturating_sub(delegator.points);

			// T::StakingInterface::withdraw_unbonded(delegator.pool.clone(), num_slashing_spans)?;
			// Unbond in the actual underlying pool
			T::StakingInterface::unbond(delegator.pool.clone(), balance_to_unbond)?;

			// Merge any older pools into the general, era agnostic unbond pool. Note that we do
			// this before inserting to ensure we don't go over the max unbonding pools.
			let mut sub_pools = sub_pools.maybe_merge_pools(current_era);

			// Update the unbond pool associated with the current era with the
			// unbonded funds. Note that we lazily create the unbond pool if it
			// does not yet exist.
			sub_pools.unchecked_with_era_get_or_make(current_era).issue(balance_to_unbond);

			delegator.unbonding_era = Some(current_era);

			Self::deposit_event(Event::<T>::Unbonded {
				delegator: target.clone(),
				pool: delegator.pool.clone(),
				amount: balance_to_unbond,
			});
			// Now that we know everything has worked write the items to storage.
			bonded_pool.put();
			SubPoolsStorage::insert(&delegator.pool, sub_pools);
			Delegators::insert(target, delegator);

			Ok(())
		}

		/// Call `withdraw_unbonded` for the pools account. This call can be made by any account.
		///
		/// This is useful if their are too many unlocking chunks to unbond, and some can be cleared
		/// by withdrawing.
		#[pallet::weight(666)]
		pub fn pool_withdraw_unbonded(
			origin: OriginFor<T>,
			pool_account: T::AccountId,
			num_slashing_spans: u32,
		) -> DispatchResult {
			let _ = ensure_signed(origin)?;
			T::StakingInterface::withdraw_unbonded(pool_account, num_slashing_spans)?;
			Ok(())
		}

		/// Withdraw unbonded funds for the `target` delegator. Under certain conditions,
		/// this call can be dispatched permissionlessly (i.e. by any account).
		///
		/// Conditions for a permissionless dispatch:
		///
		/// - The pool is in destroy mode and the target is not the depositor.
		/// - The target is the depositor and they are the only delegator in the sub pools.
		/// - The pool is blocked and the caller is either the root or state-toggler.
		///
		/// Conditions for permissioned dispatch:
		///
		/// - The caller is the target and they are not the depositor.
		///
		/// Note: If the target is the depositor, the pool will be destroyed.
		#[pallet::weight(666)]
		pub fn withdraw_unbonded_other(
			origin: OriginFor<T>,
			target: T::AccountId,
			num_slashing_spans: u32,
		) -> DispatchResult {
			let caller = ensure_signed(origin)?;
			let delegator = Delegators::<T>::get(&target).ok_or(Error::<T>::DelegatorNotFound)?;
			let unbonding_era = delegator.unbonding_era.ok_or(Error::<T>::NotUnbonding)?;
			let current_era = T::StakingInterface::current_era().unwrap_or(Zero::zero());
			ensure!(
				current_era.saturating_sub(unbonding_era) >=
					T::StakingInterface::bonding_duration(),
				Error::<T>::NotUnbondedYet
			);

			let mut sub_pools = SubPoolsStorage::<T>::get(&delegator.pool)
				.defensive_ok_or_else(|| Error::<T>::SubPoolsNotFound)?;
			let bonded_pool = BondedPool::<T>::get(&delegator.pool)
				.defensive_ok_or_else(|| Error::<T>::PoolNotFound)?;
			let should_remove_pool = bonded_pool
				.ok_to_withdraw_unbonded_other_with(&caller, &target, &delegator, &sub_pools)?;

			let balance_to_unbond = if let Some(pool) = sub_pools.with_era.get_mut(&unbonding_era) {
				let balance_to_unbond = pool.balance_to_unbond(delegator.points);
				pool.points = pool.points.saturating_sub(delegator.points);
				pool.balance = pool.balance.saturating_sub(balance_to_unbond);
				if pool.points.is_zero() {
					// Clean up pool that is no longer used
					sub_pools.with_era.remove(&unbonding_era);
				}

				balance_to_unbond
			} else {
				// A pool does not belong to this era, so it must have been merged to the era-less
				// pool.
				let balance_to_unbond = sub_pools.no_era.balance_to_unbond(delegator.points);
				sub_pools.no_era.points = sub_pools.no_era.points.saturating_sub(delegator.points);
				sub_pools.no_era.balance =
					sub_pools.no_era.balance.saturating_sub(balance_to_unbond);

				balance_to_unbond
			};

			T::StakingInterface::withdraw_unbonded(delegator.pool.clone(), num_slashing_spans)?;
			if T::Currency::free_balance(&delegator.pool) >= balance_to_unbond {
				T::Currency::transfer(
					&delegator.pool,
					&target,
					balance_to_unbond,
					ExistenceRequirement::AllowDeath,
				)
				.defensive_map_err(|e| e)?;
				Self::deposit_event(Event::<T>::Withdrawn {
					delegator: target.clone(),
					pool: delegator.pool.clone(),
					amount: balance_to_unbond,
				});
			} else {
				// This should only happen in the case a previous withdraw put the pools balance
				// below ED and it was dusted. We gracefully carry primarily to ensure the pool can
				// eventually be destroyed
				Self::deposit_event(Event::<T>::DustWithdrawn {
					delegator: target.clone(),
					pool: delegator.pool.clone(),
				});
			}

			if should_remove_pool {
				let reward_pool = RewardPools::<T>::take(&delegator.pool)
					.defensive_ok_or_else(|| Error::<T>::PoolNotFound)?;
				SubPoolsStorage::<T>::remove(&delegator.pool);
				// Kill accounts from storage by making their balance go below ED. We assume that
				// the accounts have no references that would prevent destruction once we get to
				// this point.
				T::Currency::make_free_balance_be(&reward_pool.account, Zero::zero());
				T::Currency::make_free_balance_be(&bonded_pool.account, Zero::zero());
				bonded_pool.remove();
			} else {
				SubPoolsStorage::<T>::insert(&delegator.pool, sub_pools);
			}
			Delegators::<T>::remove(&target);

			Ok(())
		}

		/// Create a pool.
		///
		/// Note that the pool creator will delegate `amount` to the pool and cannot unbond until
		/// every
		/// NOTE: This does not nominate, a pool admin needs to call [`Call::nominate`]
		///
		/// * `amount`: Balance to delegate to the pool. Must meet the minimum bond.
		/// * `index`: Disambiguation index for seeding account generation. Likely only useful when
		///   creating multiple pools in the same extrinsic.
		#[pallet::weight(666)]
		#[frame_support::transactional]
		pub fn create(
			origin: OriginFor<T>,
			amount: BalanceOf<T>,
			index: u16,
			root: T::AccountId,
			nominator: T::AccountId,
			state_toggler: T::AccountId,
		) -> DispatchResult {
			let who = ensure_signed(origin)?;
			ensure!(
				amount >= T::StakingInterface::minimum_bond() &&
					amount >= MinCreateBond::<T>::get(),
				Error::<T>::MinimumBondNotMet
			);
			if let Some(max_pools) = MaxPools::<T>::get() {
				ensure!((BondedPools::<T>::count() as u32) < max_pools, Error::<T>::MaxPools);
			}
			ensure!(!Delegators::<T>::contains_key(&who), Error::<T>::AccountBelongsToOtherPool);

			let (pool_account, reward_account) = Self::create_accounts(index);
			ensure!(!BondedPools::<T>::contains_key(&pool_account), Error::<T>::IdInUse);

			let mut bonded_pool = BondedPool::<T> {
				account: pool_account.clone(),
				points: Zero::zero(),
				depositor: who.clone(),
				root,
				nominator,
				state_toggler,
				state: PoolState::Open,
			};
			// We must calculate the points issued *before* we bond who's funds, else
			// points:balance ratio will be wrong.
			let points_issued = bonded_pool.issue(amount);
			T::Currency::transfer(&who, &pool_account, amount, ExistenceRequirement::AllowDeath)?;
			T::StakingInterface::bond(
				pool_account.clone(),
				// We make the stash and controller the same for simplicity
				pool_account.clone(),
				amount,
				reward_account.clone(),
			)?;

			Delegators::<T>::insert(
				who,
				Delegator::<T> {
					pool: pool_account.clone(),
					points: points_issued,
					reward_pool_total_earnings: Zero::zero(),
					unbonding_era: None,
				},
			);
			bonded_pool.put();
			RewardPools::<T>::insert(
				pool_account,
				RewardPool::<T> {
					balance: Zero::zero(),
					points: U256::zero(),
					total_earnings: Zero::zero(),
					account: reward_account,
				},
			);

			Ok(())
		}

		#[pallet::weight(T::WeightInfo::nominate())]
		pub fn nominate(
			origin: OriginFor<T>,
			pool_account: T::AccountId,
			validators: Vec<<T::Lookup as StaticLookup>::Source>,
		) -> DispatchResult {
			let who = ensure_signed(origin)?;
			let bonded_pool =
				BondedPool::<T>::get(&pool_account).ok_or(Error::<T>::PoolNotFound)?;
			ensure!(bonded_pool.can_nominate(&who), Error::<T>::NotNominator);
			T::StakingInterface::nominate(pool_account.clone(), validators)?;
			Ok(())
		}

		// pub fn set_state_other(origin: OriginFor<T>, pool_account: T::AccountId, state:
		// PoolState) -> DispatchError { 	let who = ensure_signed!(origin);
		// 	BondedPool::<Runtime>::try_mutate(pool_account, |maybe_bonded_pool| {
		// 		maybe_bonded_pool.ok_or(Error::<T>::PoolNotFound).map(|bonded_pool|
		// 			if bonded_pool.is_destroying() {
		// 				// invariant, a destroying pool cannot become non-destroying
		// 				// this is because
		// 				Err(Error::<T>::Err)?
		// 			}

		// 			if bonded_pool.is_spoiled() && state == PoolState::Destroying {
		// 				bonded_pool.state = PoolState::Destroying
		// 			} else if bonded_pool.root == who || bonded_pool.state_toggler == who {
		// 				bonded_pool.state = who
		// 			}
		// 		)
		// 	})
		// }
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn integrity_test() {
			assert!(
				T::StakingInterface::bonding_duration() < TotalUnbondingPools::<T>::get(),
				"There must be more unbonding pools then the bonding duration /
				so a slash can be applied to relevant unboding pools. (We assume /
				the bonding duration > slash deffer duration.",
			);
		}
	}
}

impl<T: Config> Pallet<T> {
	fn create_accounts(index: u16) -> (T::AccountId, T::AccountId) {
		let parent_hash = frame_system::Pallet::<T>::parent_hash();
		let ext_index = frame_system::Pallet::<T>::extrinsic_index().unwrap_or_default();

		let stash_entropy =
			(b"pools/stash", index, parent_hash, ext_index).using_encoded(blake2_256);
		let reward_entropy =
			(b"pools/rewards", index, parent_hash, ext_index).using_encoded(blake2_256);

		(
			Decode::decode(&mut TrailingZeroInput::new(stash_entropy.as_ref()))
				.expect("infinite length input; no invalid inputs for type; qed"),
			Decode::decode(&mut TrailingZeroInput::new(reward_entropy.as_ref()))
				.expect("infinite length input; no invalid inputs for type; qed"),
		)
	}

	/// Calculate the rewards for `delegator`.
	fn calculate_delegator_payout(
		bonded_pool: &BondedPool<T>,
		mut reward_pool: RewardPool<T>,
		mut delegator: Delegator<T>,
	) -> Result<(RewardPool<T>, Delegator<T>, BalanceOf<T>), DispatchError> {
		// If the delegator is unbonding they cannot claim rewards. Note that when the delagator
		// goes to unbond, the unbond function should claim rewards for the final time.
		ensure!(delegator.unbonding_era.is_none(), Error::<T>::AlreadyUnbonding);

		let last_total_earnings = reward_pool.total_earnings;
		reward_pool.update_total_earnings_and_balance();
		// Notice there is an edge case where total_earnings have not increased and this is zero
		let new_earnings = T::BalanceToU256::convert(
			reward_pool.total_earnings.saturating_sub(last_total_earnings),
		);

		// The new points that will be added to the pool. For every unit of balance that has
		// been earned by the reward pool, we inflate the reward pool points by
		// `bonded_pool.points`. In effect this allows each, single unit of balance (e.g.
		// plank) to be divvied up pro-rata among delegators based on points.
		let new_points = T::BalanceToU256::convert(bonded_pool.points).saturating_mul(new_earnings);

		// The points of the reward pool after taking into account the new earnings. Notice that
		// this only stays even or increases over time except for when we subtract delegator virtual
		// shares.
		let current_points = reward_pool.points.saturating_add(new_points);

		// The rewards pool's earnings since the last time this delegator claimed a payout
		let new_earnings_since_last_claim =
			reward_pool.total_earnings.saturating_sub(delegator.reward_pool_total_earnings);
		// The points of the reward pool that belong to the delegator.
		let delegator_virtual_points = T::BalanceToU256::convert(delegator.points)
			.saturating_mul(T::BalanceToU256::convert(new_earnings_since_last_claim));

		let delegator_payout = if delegator_virtual_points.is_zero() ||
			current_points.is_zero() ||
			reward_pool.balance.is_zero()
		{
			Zero::zero()
		} else {
			// Equivalent to `(delegator_virtual_points / current_points) * reward_pool.balance`
			T::U256ToBalance::convert(
				delegator_virtual_points
					.saturating_mul(T::BalanceToU256::convert(reward_pool.balance))
					// We check for zero above
					.div(current_points),
			)
		};

		// Record updates
		delegator.reward_pool_total_earnings = reward_pool.total_earnings;
		reward_pool.points = current_points.saturating_sub(delegator_virtual_points);
		reward_pool.balance = reward_pool.balance.saturating_sub(delegator_payout);

		Ok((reward_pool, delegator, delegator_payout))
	}

	/// Transfer the delegator their payout from the pool and deposit the corresponding event.
	fn transfer_reward(
		reward_pool: &T::AccountId,
		delegator: T::AccountId,
		pool: T::AccountId,
		payout: BalanceOf<T>,
	) -> Result<(), DispatchError> {
		T::Currency::transfer(reward_pool, &delegator, payout, ExistenceRequirement::AllowDeath)?;
		Self::deposit_event(Event::<T>::PaidOut { delegator, pool, payout });

		Ok(())
	}

	fn do_reward_payout(
		delegator_id: T::AccountId,
		delegator: Delegator<T>,
		bonded_pool: &BondedPool<T>,
	) -> DispatchResult {
		let reward_pool = RewardPools::<T>::get(&delegator.pool)
			.defensive_ok_or_else(|| Error::<T>::RewardPoolNotFound)?;

		let (reward_pool, delegator, delegator_payout) =
			Self::calculate_delegator_payout(bonded_pool, reward_pool, delegator)?;

		// Transfer payout to the delegator.
		Self::transfer_reward(
			&reward_pool.account,
			delegator_id.clone(),
			delegator.pool.clone(),
			delegator_payout,
		)?;

		// Write the updated delegator and reward pool to storage
		RewardPools::insert(&delegator.pool, reward_pool);
		Delegators::insert(delegator_id, delegator);

		Ok(())
	}

	fn do_slash(
		SlashPoolArgs {
			pool_stash,
			slash_amount,
			slash_era,
			apply_era,
			active_bonded,
		}: SlashPoolArgs::<T::AccountId, BalanceOf<T>>,
	) -> Option<SlashPoolOut<BalanceOf<T>>> {
		// Make sure this is a pool account
		BondedPools::<T>::contains_key(&pool_stash).then(|| ())?;
		let mut sub_pools = SubPoolsStorage::<T>::get(pool_stash).unwrap_or_default();

		let affected_range = (slash_era + 1)..=apply_era;

		// Note that this doesn't count the balance in the `no_era` pool
		let unbonding_affected_balance: BalanceOf<T> =
			affected_range.clone().fold(BalanceOf::<T>::zero(), |balance_sum, era| {
				if let Some(unbond_pool) = sub_pools.with_era.get(&era) {
					balance_sum.saturating_add(unbond_pool.balance)
				} else {
					balance_sum
				}
			});

		// Note that the balances of the bonded pool and its affected sub-pools will saturated at
		// zero if slash_amount > total_affected_balance
		let total_affected_balance = active_bonded.saturating_add(unbonding_affected_balance);
		if total_affected_balance.is_zero() {
			return Some(SlashPoolOut {
				slashed_bonded: Zero::zero(),
				slashed_unlocking: Default::default(),
			})
		}
		let slashed_unlocking: BTreeMap<_, _> = affected_range
			.filter_map(|era| {
				if let Some(mut unbond_pool) = sub_pools.with_era.get_mut(&era) {
					let after_slash_balance = {
						// Equivalent to `(slash_amount / total_affected_balance) *
						// unbond_pool.balance`
						let pool_slash_amount = slash_amount
							.saturating_mul(unbond_pool.balance)
							// We check for zero above
							.div(total_affected_balance);

						unbond_pool.balance.saturating_sub(pool_slash_amount)
					};

					unbond_pool.balance = after_slash_balance;

					Some((era, after_slash_balance))
				} else {
					None
				}
			})
			.collect();
		SubPoolsStorage::<T>::insert(pool_stash, sub_pools);

		// Equivalent to `(slash_amount / total_affected_balance) * active_bonded`
		let slashed_bonded = {
			let bonded_pool_slash_amount = slash_amount
				.saturating_mul(active_bonded)
				// We check for zero above
				.div(total_affected_balance);

			active_bonded.saturating_sub(bonded_pool_slash_amount)
		};
		Some(SlashPoolOut { slashed_bonded, slashed_unlocking })
	}
}

impl<T: Config> PoolsInterface for Pallet<T> {
	type AccountId = T::AccountId;
	type Balance = BalanceOf<T>;

	fn slash_pool(
		args: SlashPoolArgs<Self::AccountId, Self::Balance>,
	) -> Option<SlashPoolOut<Self::Balance>> {
		Self::do_slash(args)
	}
}
