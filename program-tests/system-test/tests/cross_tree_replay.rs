#![cfg(feature = "test-sbf")]

use account_compression::processor::initialize_address_merkle_tree::ToAccountMetas;
use account_compression::ID;
use anchor_lang::InstructionData;
use light_batched_merkle_tree::{
    initialize_state_tree::InitStateTreeAccountsInstructionData,
    merkle_tree::{BatchedMerkleTreeAccount, InstructionDataBatchAppendInputs},
    queue::BatchedQueueAccount,
};
use light_compressed_account::{
    instruction_data::{
        compressed_proof::CompressedProof, insert_into_queues::InsertIntoQueuesInstructionDataMut,
    },
    pubkey::Pubkey as CompressedPubkey,
};

use account_compression::instructions::register_program::RegisteredProgram;
use anchor_lang::AnchorSerialize;
use light_program_test::utils::assert::assert_rpc_error;
use light_program_test::{
    accounts::{initialize::get_group_pda, state_tree_v2::create_batched_state_merkle_tree},
    program_test::LightProgramTest,
    ProgramTestConfig, Rpc,
};
use light_registry::{
    account_compression_cpi::sdk::create_batch_append_instruction,
    sdk::create_register_program_instruction,
    utils::{get_cpi_authority_pda, get_protocol_config_pda_address},
};
use light_test_utils::{
    mock_batched_forester::{MockBatchedForester, MockTxEvent},
    RpcError,
};
use solana_sdk::system_instruction;
use solana_sdk::{
    account::WritableAccount,
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::{Keypair, Signature, Signer},
};
use light_program_test::accounts::register_program::register_program_with_registry_program;

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
    // Each tree must use a unique CPI context account. Reusing the same
    // keypair would result in the second `create_account` call attempting to
    // re-initialize an existing account.
    let ctx_a = Keypair::new();
    create_batched_state_merkle_tree(
        &payer,
        false,
        &mut rpc,
        &tree_a,
        &queue_a,
        &ctx_a,
        InitStateTreeAccountsInstructionData::test_default(),
    )
    .await
    .unwrap();

    // Create second batched state merkle tree (Tree B) with its own context
    // account to avoid duplicate account creation.
    let tree_b = Keypair::new();
    let queue_b = Keypair::new();
    let ctx_b = Keypair::new();
    create_batched_state_merkle_tree(
        &payer,
        false,
        &mut rpc,
        &tree_b,
        &queue_b,
        &ctx_b,
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

async fn generate_proof_for_tree_a(
    tree_pubkey: Pubkey,
    output_queue_pubkey: Pubkey,
    context: &mut LightProgramTest,
    mock_indexer: &mut MockBatchedForester<32>,
) -> (InstructionDataBatchAppendInputs, [u8; 32], [u8; 32], u64) {
    let payer = context.get_payer().insecure_clone();
    let mut counter = 0u32;
    perform_insert_into_output_queue(
        context,
        mock_indexer,
        output_queue_pubkey,
        &payer,
        &mut counter,
        10,
    )
    .await
    .unwrap();

    let merkle_tree_account = &mut context.get_account(tree_pubkey).await.unwrap().unwrap();
    let output_queue_account = &mut context
        .get_account(output_queue_pubkey)
        .await
        .unwrap()
        .unwrap();

    let mt_account_data = merkle_tree_account.data_as_mut_slice();
    let output_queue_account_data = output_queue_account.data_as_mut_slice();

    let zero_copy_account = BatchedMerkleTreeAccount::state_from_bytes(
        mt_account_data,
        &CompressedPubkey::new_from_array(tree_pubkey.to_bytes()),
    )
    .unwrap();
    let old_root = zero_copy_account.get_root().unwrap();
    let start_index = zero_copy_account.get_metadata().next_index;

    let output_zero_copy_account =
        BatchedQueueAccount::output_from_bytes(output_queue_account_data).unwrap();
    let next_full_batch = output_zero_copy_account
        .get_metadata()
        .batch_metadata
        .pending_batch_index;
    let batch = output_zero_copy_account
        .batch_metadata
        .batches
        .get(next_full_batch as usize)
        .unwrap();
    let leaves_hash_chain = *output_zero_copy_account
        .hash_chain_stores
        .get(next_full_batch as usize)
        .expect("Failed to get hash_chain_stores for next_full_batch")
        .get(batch.get_num_inserted_zkps() as usize)
        .expect("Failed to get hash_chain for inserted_zkps");

    let bundle =
        create_append_batch_ix_data(mock_indexer, mt_account_data, output_queue_account_data).await;

    (bundle, old_root, leaves_hash_chain, start_index)
}

async fn perform_insert_into_output_queue(
    context: &mut LightProgramTest,
    mock_indexer: &mut MockBatchedForester<32>,
    output_queue_pubkey: Pubkey,
    payer: &Keypair,
    counter: &mut u32,
    num_of_leaves: u32,
) -> Result<Signature, RpcError> {
    let mut bytes = vec![
        0u8;
        InsertIntoQueuesInstructionDataMut::required_size_for_capacity(
            num_of_leaves as u8,
            0,
            0,
            1,
            0,
            0,
        )
    ];
    let (mut ix_data, _) =
        InsertIntoQueuesInstructionDataMut::new_at(&mut bytes, num_of_leaves as u8, 0, 0, 1, 0, 0)
            .unwrap();
    ix_data.num_output_queues = 1;
    for i in 0..num_of_leaves {
        let mut leaf = [0u8; 32];
        leaf[31] = *counter as u8;
        ix_data.leaves[i as usize].leaf = leaf;
        mock_indexer.output_queue_leaves.push(leaf);
        mock_indexer.tx_events.push(MockTxEvent {
            tx_hash: [0u8; 32],
            inputs: vec![],
            outputs: vec![leaf],
        });
        *counter += 1;
    }

    let instruction = account_compression::instruction::InsertIntoQueues { bytes };
    let accounts = account_compression::accounts::GenericInstruction {
        authority: payer.pubkey(),
    };
    let accounts = [
        accounts.to_account_metas(Some(true)),
        vec![AccountMeta {
            pubkey: output_queue_pubkey,
            is_signer: false,
            is_writable: true,
        }],
    ]
    .concat();

    let instruction = Instruction {
        program_id: ID,
        accounts,
        data: instruction.data(),
    };
    context
        .create_and_send_transaction(&[instruction], &payer.pubkey(), &[payer])
        .await
}

async fn create_append_batch_ix_data(
    mock_indexer: &mut MockBatchedForester<32>,
    mt_account_data: &mut [u8],
    output_queue_account_data: &mut [u8],
) -> InstructionDataBatchAppendInputs {
    let zero_copy_account =
        BatchedMerkleTreeAccount::state_from_bytes(mt_account_data, &Pubkey::default().into())
            .unwrap();
    let output_zero_copy_account =
        BatchedQueueAccount::output_from_bytes(output_queue_account_data).unwrap();

    let next_index = zero_copy_account.get_metadata().next_index;
    let next_full_batch = output_zero_copy_account
        .get_metadata()
        .batch_metadata
        .pending_batch_index;
    let batch = output_zero_copy_account
        .batch_metadata
        .batches
        .get(next_full_batch as usize)
        .unwrap();
    let leaves_hash_chain = output_zero_copy_account
        .hash_chain_stores
        .get(next_full_batch as usize)
        .expect("Failed to get hash_chain_stores for next_full_batch")
        .get(batch.get_num_inserted_zkps() as usize)
        .expect("Failed to get hash_chain for inserted_zkps");
    let (proof, new_root) = mock_indexer
        .get_batched_append_proof(
            next_index as usize,
            batch.get_num_inserted_zkps() as u32,
            batch.zkp_batch_size as u32,
            *leaves_hash_chain,
            batch.get_num_zkp_batches() as u32,
        )
        .await
        .expect("mock_indexer.get_batched_append_proof failed");

    InstructionDataBatchAppendInputs {
        new_root,
        compressed_proof: CompressedProof {
            a: proof.a,
            b: proof.b,
            c: proof.c,
        },
    }
}

#[tokio::test]
async fn replay_proof_on_tree_b() {
    // Spin up a LightProgramTest context
    let mut rpc = LightProgramTest::new(ProgramTestConfig::new(false, None))
        .await
        .unwrap();
    let payer = rpc.get_payer().insecure_clone();

    // Create first batched state merkle tree (Tree A)
    let tree_a = Keypair::new();
    let queue_a = Keypair::new();
    // Separate CPI context for Tree A
    let ctx_a = Keypair::new();
    create_batched_state_merkle_tree(
        &payer,
        false,
        &mut rpc,
        &tree_a,
        &queue_a,
        &ctx_a,
        InitStateTreeAccountsInstructionData::test_default(),
    )
    .await
    .unwrap();

    // Create second batched state merkle tree (Tree B) with its own context
    let tree_b = Keypair::new();
    let queue_b = Keypair::new();
    let ctx_b = Keypair::new();
    create_batched_state_merkle_tree(
        &payer,
        false,
        &mut rpc,
        &tree_b,
        &queue_b,
        &ctx_b,
        InitStateTreeAccountsInstructionData::test_default(),
    )
    .await
    .unwrap();

    // Register the same forester program on both trees so that the
    // `BatchAppend` CPI finds the (tree_pubkey, forester_program) PDA.
    let forester_program = Keypair::new();

    let group_a = get_group_pda(tree_a.pubkey());
    let group_b = get_group_pda(tree_b.pubkey());
    register_program_with_registry_program(&mut rpc, &payer, &group_a, &forester_program).await.unwrap();
    register_program_with_registry_program(&mut rpc, &payer, &group_b, &forester_program).await.unwrap();
    // Generate append proof for Tree A
    let mut mock_indexer = MockBatchedForester::<32>::default();
    let (bundle, old_root, _leaves_hash_chain, _start_index) = generate_proof_for_tree_a(
        tree_a.pubkey(),
        queue_a.pubkey(),
        &mut rpc,
        &mut mock_indexer,
    )
    .await;

    // Both trees start from the same deterministic root at genesis.
    // Without mirroring leaves into Tree B, its root should still equal
    // the old_root used for Tree A's proof.
    let account_b = rpc.get_account(tree_b.pubkey()).await.unwrap().unwrap();
    let mut data_b = account_b.data.clone();
    let mt_b = BatchedMerkleTreeAccount::state_from_bytes(
        data_b.as_mut_slice(),
        &CompressedPubkey::new_from_array(tree_b.pubkey().to_bytes()),
    )
    .unwrap();
    assert_eq!(mt_b.get_root().unwrap(), old_root);

    // Craft BatchAppend instruction for Tree B using Tree A's proof. The proof
    // hashes only the old root, new root, leaves hash chain and start index,
    // so it is valid for any tree sharing the same state. Passing the
    // previously registered `forester_program_id` satisfies the on-chain PDA
    // check.
    let ix = create_batch_append_instruction(
        payer.pubkey(),
        forester_program.pubkey(),
        tree_b.pubkey(),
        queue_b.pubkey(),
        0,
        bundle.try_to_vec().unwrap(),
    );

    rpc.create_and_send_transaction(&[ix], &payer.pubkey(), &[&payer])
        .await
        .unwrap();

    // Fetch Tree B after append and verify root updated to new root from the proof.
    let account_b = rpc.get_account(tree_b.pubkey()).await.unwrap().unwrap();
    let mut data_b = account_b.data.clone();
    let mt_b = BatchedMerkleTreeAccount::state_from_bytes(
        data_b.as_mut_slice(),
        &CompressedPubkey::new_from_array(tree_b.pubkey().to_bytes()),
    )
    .unwrap();
    assert_eq!(mt_b.get_root().unwrap(), bundle.new_root);
}
