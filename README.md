# Central Link Of Trust Contract

This NEAR smart contract implements a simple "trust network" model. Users register on-chain, set trust relationships, and block or unblock other users.

## Safe Deposit

To allow decentralisation, users storing data in the contract (trusting/blocking users, public profile, account creation) need to provide a deposit.
This deposit belong to the account and can be redemeed at any time.

When the data shrink, the deposit amount is automatically refunded to the user. On accoutn deletion the total deposit amount is refunded.

### Profit extraction

A mechanism is implemented for the contract owner to extract profits without stealing deposit of the users.
At any time you can verify that the sum of the amount of NEAR deposited by the users is inferior to the smartcontract amount balance using `get_total_users_deposit()`.
The contract owner can execute `extract_profit(to: AccountId, amount: NearToken)` to extract some near token from the contract. This call will fail if it would result in a balance inferior to the total deposited amount, preventing rug pull.


## Overview

- **Hashed User IDs**  
  The contract identifies users by the Base58-encoded SHA-256 hash of their account IDs. 
- **Trust Network**  
  Each user stores a list of `(other_user, level)` pairs, where `level ∈ [0,1]`.
- **Blocking**  
  Users can block others. When blocked, the blocked user cannot trust or interact with the blocker.
- **Storage Costs**  
  Additional data requires attaching enough NEAR for storage. Surplus deposit is refunded.

## Features

- **Registering / Creating**  
  Users appear automatically once they modify their public profile or trust another user.
- **Updating Profile**  
  Users can modify a string-based `public_profile`.
- **Trust / Untrust**  
  A user can set or remove trust levels to other hashed IDs.
- **Block / Unblock**  
  A user can block or unblock specific hashed IDs.
- **Delete Account**  
  Users can delete their on-chain record. Any leftover deposit is refunded.

## Methods

- **`modify_public_profile(profile: String)`**  
  Updates a user's `public_profile`.
- **`trust(user_id: String, level: f32)`**  
  Sets a trust level `[0..1]`; `0.0` effectively untrusts.  
- **`untrust(user_id: String)`**  
  Removes any trust record to `user_id`.
- **`block_user(user_id: String)`**  
  Blocks `user_id`; also removes any trust that `user_id` had to you.
- **`unblock_user(user_id: String)`**  
  Removes a block.
- **`delete_user()`**  
  Permanently removes the user from contract storage, refunding their deposit.

## Building & Testing

1. **Prerequisites**  
   - [Rust and Cargo](https://www.rust-lang.org/tools/install)
   - NEAR CLI (optional for deploying)

2. **Build**  
   ```bash
   cargo build --target wasm32-unknown-unknown --release
