use near_sdk::{
    borsh::{self, BorshDeserialize, BorshSerialize},
    bs58,
    env::{self, sha256},
    near,
    require,
    store::IterableMap, // <-- Keeping IterableMap for top-level
    AccountId,
    NearToken,
    Promise,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A type alias for hashed user IDs (sha256 of real account).
#[derive(
    BorshDeserialize,
    BorshSerialize,
    Clone,
    Ord,
    PartialEq,
    PartialOrd,
    Eq,
    Serialize,
    Deserialize,
    Debug,
)]
pub struct HashedUserId {
    s_bs58: String,
}

impl HashedUserId {
    pub fn as_bytes(&self) -> Vec<u8> {
        bs58::decode(&self.s_bs58).into_vec().unwrap()
    }

    pub fn len(&self) -> usize {
        self.s_bs58.len()
    }

    pub fn from_account_id(account_id: &AccountId) -> Self {
        Self {
            s_bs58: bs58::encode(sha256(account_id.as_bytes())).into_string(),
        }
    }

    pub fn from_bs58(s: &str) -> Self {
        Self {
            s_bs58: s.to_string(),
        }
    }
}

/// A record for pending trust request
#[derive(BorshDeserialize, BorshSerialize, Clone)]
pub struct TrustRequest {
    pub deposit: NearToken,
    pub expiry: u64,
}

/// A "view-friendly" version of `UserData` suitable for JSON responses.
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(crate = "near_sdk::serde")]
pub struct UserDataView {
    pub hashed_user_id: String,
    pub requested_trust_cost: u128,
    pub public_profile: String,

    // Sub-maps become simple vectors
    pub trust_network: Vec<(String, f32)>,
    pub blocked_users: Vec<String>,
}

/// The user's data stored in the contract, keyed by hashed user ID.
/// *Refactored* to use Vec instead of IterableMap in sub-fields.
#[derive(BorshDeserialize, BorshSerialize, Clone)]
pub struct UserData {
    pub hashed_user_id: HashedUserId,
    pub requested_trust_cost: NearToken,
    pub public_profile: String,

    // Sub-collections become Vec
    pub trust_network: Vec<(String, f32)>,
    pub blocked_users: Vec<String>,
}

impl UserData {
    pub fn new(hashed_id: HashedUserId) -> Self {
        Self {
            hashed_user_id: hashed_id,
            requested_trust_cost: NearToken::from_yoctonear(0),
            public_profile: String::new(),
            trust_network: Vec::new(),
            blocked_users: Vec::new(),
        }
    }

    // -----------------------------
    // Helpers for sub-collections
    // -----------------------------

    // (String, f32)
    fn set_pair_f32(vec: &mut Vec<(String, f32)>, key: String, value: f32) {
        for (k, v) in vec.iter_mut() {
            if *k == key {
                *v = value;
                return;
            }
        }
        vec.push((key, value));
    }

    fn set_key(vec: &mut Vec<String>, key: String) {
        for k in vec.iter_mut() {
            if *k == key {
                return;
            }
        }
        vec.push(key);
    }

    fn remove_key(vec: &mut Vec<String>, key: String) {
        if let Some(idx) = vec.iter().position(|k| *k == key) {
            vec.remove(idx);
        }
    }

    fn remove_pair_f32(vec: &mut Vec<(String, f32)>, key: &str) {
        if let Some(idx) = vec.iter().position(|(k, _)| k == key) {
            vec.remove(idx);
        }
    }

    // Public convenience methods for each sub-vector:

    // --- trust_network
    pub fn insert_trust_network(&mut self, key: String, val: f32) {
        Self::set_pair_f32(&mut self.trust_network, key, val);
    }

    pub fn remove_trust_network(&mut self, key: &str) {
        Self::remove_pair_f32(&mut self.trust_network, key);
    }

    pub fn get_trust_network(&self, key: &str) -> Option<f32> {
        self.trust_network
            .iter()
            .find_map(|(k, v)| if k == key { Some(*v) } else { None })
    }

    // --- blocked_requests
    pub fn block_user(&mut self, key: String) {
        Self::set_key(&mut self.blocked_users, key);
    }

    pub fn unblock_user(&mut self, key: &str) {
        Self::remove_key(&mut self.blocked_users, key.to_owned());
    }

    pub fn is_blocked(&self, key: &str) -> bool {
        self.blocked_users.iter().any(|k| k == key)
    }
}

/// Main contract state: we keep `IterableMap` for the `users` and `user_deposits`.
#[near(contract_state)]
pub struct CentralLinkOfTrustContract {
    // hashedUserId -> UserData
    users: IterableMap<HashedUserId, UserData>,

    // hashedUserId -> total deposit locked for storage
    user_deposits: IterableMap<HashedUserId, NearToken>,

    // Maximum expiry offset in nanoseconds
    timeout_duration: u64,
}

impl Default for CentralLinkOfTrustContract {
    fn default() -> Self {
        Self {
            users: IterableMap::new(b"u".to_vec()),
            user_deposits: IterableMap::new(b"d".to_vec()),
            timeout_duration: 7 * 24 * 60 * 60 * 1_000_000_000, // 7 days
        }
    }
}

#[near]
impl CentralLinkOfTrustContract {
    #[init]
    #[private]
    pub fn new() -> Self {
        Self::default()
    }

    // ----------------------------------
    // STORAGE MEASUREMENT & DEPOSIT LOGIC
    // ----------------------------------
    /// Measure the storage usage manually.  
    /// We iterate over each vector to sum up their lengths, plus overhead.
    fn measure_storage_usage(&self, hashed_id: &HashedUserId, user_data: &UserData) -> u64 {
        let mut total: u64 = hashed_id.len() as u64;
        total += user_data.public_profile.len() as u64;

        // trust_network
        for (k, _) in &user_data.trust_network {
            total += k.len() as u64 + 4; // f32 is 4 bytes
        }

        // blocked_requests
        for k in &user_data.blocked_users {
            total += k.len() as u64 + 16; // near is u128=16 bytes
        }

        // Overhead
        total + 256
    }

    /// Internal helper to get a user, mutate it, and re-insert into `self.users`.
    fn with_user_data<F>(&mut self, hashed_id: &HashedUserId, f: F)
    where
        F: FnOnce(&mut UserData),
    {
        let mut user_data = match self.users.remove(hashed_id) {
            Some(existing) => existing,
            None => UserData::new(hashed_id.clone()),
        };
        f(&mut user_data);
        self.users.insert(hashed_id.clone(), user_data);
    }

    /// Compare the updated storage usage to the user’s deposit.  Refund or require more if needed.
    fn verify_deposit(&mut self, hashed_id: HashedUserId) {
        let new_size = self.measure_storage_usage(&hashed_id, &self.users[&hashed_id]);
        let cost_per_byte = env::storage_byte_cost();
        let required_deposit = (new_size as u128) * cost_per_byte.as_yoctonear();

        let current_deposit = self
            .user_deposits
            .get(&hashed_id)
            .unwrap_or(&NearToken::from_yoctonear(0))
            .as_yoctonear();
        let attached = env::attached_deposit().as_yoctonear();
        let updated_deposit = current_deposit + attached;

        if updated_deposit < required_deposit {
            let deficit = required_deposit - updated_deposit;
            require!(
                false,
                &format!(
                    "Insufficient deposit. Attach at least {} yoctoNEAR.",
                    deficit
                )
            );
        }

        // Refund any excess deposit
        if updated_deposit > required_deposit {
            let excess_deposit = updated_deposit - required_deposit;
            Promise::new(env::predecessor_account_id())
                .transfer(NearToken::from_yoctonear(excess_deposit));
            self.user_deposits.insert(
                hashed_id.clone(),
                NearToken::from_yoctonear(required_deposit),
            );
        } else {
            self.user_deposits.insert(
                hashed_id.clone(),
                NearToken::from_yoctonear(updated_deposit),
            );
        }
    }

    // ----------------
    // BASICS
    // ----------------

    pub fn get_total_users_deposit(&self) -> NearToken {
        NearToken::from_yoctonear(
            self.user_deposits
                .values()
                .fold(0_u128, |acc, x| acc + (*x).as_yoctonear()),
        )
    }

    pub fn extract_profit(&mut self, to: AccountId, amount: NearToken) {
        require!(
            env::predecessor_account_id() == env::current_account_id(),
            "ERR_NOT_ALLOWED"
        );

        let profit_to_extract = NearToken::from_yoctonear(
            env::account_balance().as_yoctonear()
                - self.get_total_users_deposit().as_yoctonear()
                - (2 * 10 ^ 24), //Overhead for contract size
        );

        require!(profit_to_extract >= amount, "ERR_NOT_ENOUGH_BALANCE");
        require!(amount > NearToken::from_yoctonear(0), "ERR_AMOUNT_TOO_LOW");

        Promise::new(to).transfer(amount);
    }

    // ---------------
    // USER APIS
    // ---------------
    #[payable]
    pub fn modify_public_profile(&mut self, profile: String) {
        let caller_id = HashedUserId::from_account_id(&env::predecessor_account_id());
        self.with_user_data(&caller_id, |user_data| {
            user_data.public_profile = profile;
        });
        self.verify_deposit(caller_id);
    }

    pub fn view_users(&self) -> Vec<String> {
        self.users
            .iter()
            .map(|(hashed_id, _)| hashed_id.s_bs58.clone())
            .collect()
    }

    /// Return a `UserDataView` for the given hashed ID.
    pub fn get_user_data(&self, user_id: String) -> Option<UserDataView> {
        let h_user_id = HashedUserId::from_bs58(&user_id);
        if let Some(user) = self.users.get(&h_user_id) {
            Some(UserDataView {
                hashed_user_id: user.hashed_user_id.s_bs58.clone(),
                requested_trust_cost: user.requested_trust_cost.as_yoctonear(),
                public_profile: user.public_profile.clone(),
                trust_network: user.trust_network.clone(),

                blocked_users: user.blocked_users.iter().map(|k| k.clone()).collect(),
            })
        } else {
            None
        }
    }

    /// Return a `UserDataView` for the given hashed ID.
    pub fn get_user_deposit(&self, user_id: String) -> Option<NearToken> {
        let h_user_id = HashedUserId::from_bs58(&user_id);
        if let Some(deposit) = self.user_deposits.get(&h_user_id) {
            Some(deposit.to_owned())
        } else {
            None
        }
    }

    // trust level = 0..1
    #[payable]
    pub fn trust(&mut self, user_id: String, level: f32) {
        require!(level >= 0.0 && level <= 1.0, "Invalid trust level");
        let caller_id = HashedUserId::from_account_id(&env::predecessor_account_id());
        let h_trusted_id = HashedUserId::from_bs58(&user_id);

        // If the target user exists, ensure they are not blocking caller
        if let Some(target_user) = self.users.get(&h_trusted_id) {
            if target_user.is_blocked(&caller_id.s_bs58) {
                env::panic_str("You are blocked");
            }
        }

        self.with_user_data(&caller_id, |user_data| {
            if level == 0.0 {
                user_data.remove_trust_network(&h_trusted_id.s_bs58);
            } else {
                user_data.insert_trust_network(h_trusted_id.s_bs58.clone(), level);
            }
        });
        self.verify_deposit(caller_id);
    }

    #[payable]
    pub fn untrust(&mut self, user_id: String) {
        let caller_id = HashedUserId::from_account_id(&env::predecessor_account_id());
        let h_trusted_id = HashedUserId::from_bs58(&user_id);
        self.with_user_data(&caller_id, |user_data| {
            user_data.remove_trust_network(&h_trusted_id.s_bs58);
        });
        self.verify_deposit(caller_id);
    }

    // block a user
    #[payable]
    pub fn block_user(&mut self, other_id: String) {
        let caller_id = HashedUserId::from_account_id(&env::predecessor_account_id());
        let h_other_id = HashedUserId::from_bs58(&other_id);

        self.with_user_data(&caller_id, |user_data| {
            user_data.block_user(h_other_id.s_bs58.clone());
        });

        // Also ensure that the "other user" no longer trusts caller
        if let Some(_) = self.users.get(&h_other_id) {
            self.with_user_data(&h_other_id, |user_data| {
                user_data.remove_trust_network(&caller_id.s_bs58);
            });
        }
        self.verify_deposit(caller_id);
    }

    #[payable]
    pub fn unblock_user(&mut self, other_id: String) {
        let caller_id = HashedUserId::from_account_id(&env::predecessor_account_id());
        let h_other_id = HashedUserId::from_bs58(&other_id);

        self.with_user_data(&caller_id, |user_data| {
            user_data.unblock_user(&h_other_id.s_bs58);
        });
        self.verify_deposit(caller_id);
    }

    // -------------
    // DELETE ACCOUNT
    // -------------
    #[payable]
    pub fn delete_user(&mut self) {
        let caller_id = HashedUserId::from_account_id(&env::predecessor_account_id());
        if self.users.get(&caller_id).is_none() {
            env::panic_str("No record found for this user");
        }
        // The deposit the user had staked
        let user_deposit = self
            .user_deposits
            .get(&caller_id)
            .unwrap_or(&NearToken::from_yoctonear(0))
            .clone();

        // Remove from contract
        self.users.remove(&caller_id);
        self.user_deposits.remove(&caller_id);

        // Full refund
        Promise::new(env::predecessor_account_id()).transfer(user_deposit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_data_trust_network_insertion() {
        let mut user = UserData::new(HashedUserId::from_bs58("alice"));
        user.insert_trust_network("bob".to_string(), 0.5);
        assert_eq!(user.get_trust_network("bob").unwrap(), 0.5);
    }

    #[test]
    fn user_data_trust_network_removal() {
        let mut user = UserData::new(HashedUserId::from_bs58("alice"));
        user.insert_trust_network("bob".to_string(), 0.5);
        user.remove_trust_network("bob");
        assert_eq!(user.get_trust_network("bob"), None);
    }

    #[test]
    fn user_data_cloning() {
        let mut user = UserData::new(HashedUserId::from_bs58("alice"));
        user.insert_trust_network("bob".to_string(), 0.5);
        let user_clone = user.clone();
        assert_eq!(user_clone.get_trust_network("bob").unwrap(), 0.5);
    }

    #[test]
    fn user_data_trust_insertion_after_cloning() {
        let user = UserData::new(HashedUserId::from_bs58("alice"));
        let mut user_clone = user.clone();
        user_clone.insert_trust_network("bob".to_string(), 0.5);

        // The clone has "bob" => 0.5
        assert_eq!(user_clone.get_trust_network("bob").unwrap(), 0.5);
        // The original has none
        assert_eq!(user.get_trust_network("bob"), None);
    }

    #[test]
    fn user_data_block_user() {
        let mut alice = UserData::new(HashedUserId::from_bs58("alice"));
        let bob_id = "bob".to_string();

        // Initially, alice is not blocking bob
        assert!(!alice.is_blocked(&bob_id));

        // Block bob
        alice.block_user(bob_id.clone());
        assert!(alice.is_blocked(&bob_id));

        // Re-blocking bob doesn't cause duplication
        alice.block_user(bob_id.clone());
        // still only blocked once
        assert!(alice.is_blocked(&bob_id));
        assert_eq!(alice.blocked_users.len(), 1);
    }

    #[test]
    fn user_data_unblock_user() {
        let mut alice = UserData::new(HashedUserId::from_bs58("alice"));
        alice.block_user("bob".to_string());
        assert!(alice.is_blocked("bob"));

        // Unblock bob
        alice.unblock_user("bob");
        assert!(!alice.is_blocked("bob"));
        // repeated unblock is safe
        alice.unblock_user("bob");
        assert!(!alice.is_blocked("bob"));
        assert_eq!(alice.blocked_users.len(), 0);
    }

    /// If a user is blocked, the other user cannot remain in trust_network.
    ///
    /// Simulate the logic that "If user2 is blocked by user1, user2's
    /// trust relationship to user1 is removed."
    /// This is partially tested in `block_user` contract call,
    /// but here we do it on the data structure level.
    #[test]
    fn blocking_remove_other_trust() {
        // Suppose we have two users, alice and bob
        let mut alice = UserData::new(HashedUserId::from_bs58("alice"));
        let mut bob = UserData::new(HashedUserId::from_bs58("bob"));

        // bob trusts alice
        bob.insert_trust_network("alice".to_string(), 0.7);
        assert_eq!(bob.get_trust_network("alice").unwrap(), 0.7);

        // alice blocks bob => we remove bob->alice trust if we do that in the contract logic
        // We'll do it manually here to simulate the "block_user" contract call:
        // i.e. alice blocks "bob"
        alice.block_user("bob".to_string());

        // Then "bob" must remove trust of "alice"
        bob.remove_trust_network("alice");

        // Check that bob no longer trusts alice
        assert_eq!(bob.get_trust_network("alice"), None);
    }

    #[test]
    fn trust_changes() {
        let mut alice = UserData::new(HashedUserId::from_bs58("alice"));

        // trust, untrust, trust again
        alice.insert_trust_network("bob".to_string(), 0.3);
        assert_eq!(alice.get_trust_network("bob").unwrap(), 0.3);

        // untrust bob
        alice.remove_trust_network("bob");
        assert_eq!(alice.get_trust_network("bob"), None);

        // re-trust bob at 1.0
        alice.insert_trust_network("bob".to_string(), 1.0);
        assert_eq!(alice.get_trust_network("bob").unwrap(), 1.0);
    }
}
