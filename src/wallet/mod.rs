pub mod evm;
pub mod keyring_store;
pub mod solana;
pub mod tron;

// Wallet management: loading keys from config/env/keyring, signing transactions.
// Implementation details in evm.rs and solana.rs.
