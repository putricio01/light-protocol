#![cfg(feature = "test-sbf")]

use light_batched_merkle_tree::{
    initialize_state_tree::InitStateTreeAccountsInstructionData,
    merkle_tree::BatchedMerkleTreeAccount,
};
use light_compressed_account::pubkey::Pubkey as CompressedPubkey;
use light_program_test::{
    accounts::state_tree_v2::create_batched_state_merkle_tree, program_test::LightProgramTest,
    ProgramTestConfig, Rpc,
};
use solana_sdk::signature::{Keypair, Signer};

#[tokio::test]
async fn init_two_batched_trees() {
    // Spin up a LightProgramTest context
    let mut rpc = LightProgramTest::new(ProgramTestConfig::new(false, None))
        .await
        .unwrap();
    let payer = rpc.get_payer().insecure_clone();

    // Create first batched state merkle tree (Tree A)
    let tree_a = Keypair::new();
    let queue_a = Keypair::new();
    let cpi_a = Keypair::new();
    create_batched_state_merkle_tree(
        &payer,
        false,
        &mut rpc,
        &tree_a,
        &queue_a,
        &cpi_a,
        InitStateTreeAccountsInstructionData::test_default(),
    )
    .await
    .unwrap();

    // Create second batched state merkle tree (Tree B)
    let tree_b = Keypair::new();
    let queue_b = Keypair::new();
    let cpi_b = Keypair::new();
    create_batched_state_merkle_tree(
        &payer,
        false,
        &mut rpc,
        &tree_b,
        &queue_b,
        &cpi_b,
        InitStateTreeAccountsInstructionData::test_default(),
    )
    .await
    .unwrap();

    // Fetch both Merkle tree accounts and compare their roots
    let account_a = rpc.get_account(tree_a.pubkey()).await.unwrap().unwrap();
    let account_b = rpc.get_account(tree_b.pubkey()).await.unwrap().unwrap();
    let mut data_a = account_a.data.clone();
    let mut data_b = account_b.data.clone();

    let mt_a = BatchedMerkleTreeAccount::state_from_bytes(
        &mut data_a,
        &CompressedPubkey::new_from_array(tree_a.pubkey().to_bytes()),
    )
    .unwrap();
    let mt_b = BatchedMerkleTreeAccount::state_from_bytes(
        &mut data_b,
        &CompressedPubkey::new_from_array(tree_b.pubkey().to_bytes()),
    )
    .unwrap();

    let root_a = mt_a.get_root().unwrap();
    let root_b = mt_b.get_root().unwrap();

    assert_eq!(root_a, root_b);
}