#![cfg(feature = "test-sbf")]

use solana_program_test::tokio;
use solana_sdk::{signature::Keypair, signer::Signer, transaction::Transaction, pubkey::Pubkey};

use light_program_test::{LightProgramTest, ProgramTestConfig, TestRpc};

// These paths are based on typical structure in this repo. Adjust to match existing tests.
// Look at other files in `program-tests/compressed-token-test/tests` and copy their imports.
use light_program_test::accounts::state_tree_v2::create_batched_state_merkle_tree;
use light_test_utils::{
    airdrop_lamports,
    spl::{create_mint_helper, mint_spl_tokens},
};

use light_compressed_token_sdk::{
    compressed_token::transfer2::{
        account_metas::Transfer2AccountsMetaConfig,
        create_transfer2_instruction,
        Transfer2Config,
        Transfer2Inputs,
    },
    ctoken::CreateAssociatedTokenAccount,
    utils::CTokenDefaultAccounts,
};

// If there is a shared helper in this crate that sets up a “standard” CTK environment,
// import it too (e.g. something like `setup_compressed_token_test_env`).

#[tokio::test]
async fn output_leaf_misdirection_spend_from_attacker_tree() {
    // 1) Start Light test environment (all programs: account-compression, system, compressed-token)
    let mut test = LightProgramTest::new(ProgramTestConfig::default());
    let mut ctx = test.start_with_context().await;
    let rpc = TestRpc::new(&mut ctx);

    let payer = Keypair::from_bytes(&ctx.payer.to_bytes()).unwrap();

    // 2) Create two batched state trees: canonical_tree and attacker_tree
    // Use the same helper and params as other state_tree_v2 tests.
    let canonical_tree = create_batched_state_merkle_tree(
        &mut ctx,
        /* height */ 26,
        /* canopy_depth */ 0,
        /* with_queue */ true,
    )
    .await;

    let attacker_tree = create_batched_state_merkle_tree(
        &mut ctx,
        /* height */ 26,
        /* canopy_depth */ 0,
        /* with_queue */ true,
    )
    .await;

    let canonical_tree_pubkey = canonical_tree.state_tree;
    let attacker_tree_pubkey = attacker_tree.state_tree;

    // TODO: if there are helper types for reading tree state (roots / leaf count),
    // import and capture before/after snapshots here.
    // let canonical_before = rpc.get_account_data::<PublicStateMerkleTreeAccount>(&canonical_tree_pubkey).await.unwrap();
    // let attacker_before = rpc.get_account_data::<PublicStateMerkleTreeAccount>(&attacker_tree_pubkey).await.unwrap();

    // 3) Set up SPL mint + initial compressed-token position
    // ---------------------------------------------------------
    // a) Create an SPL mint M and mint some tokens to `owner`.
    let owner = Keypair::new();
    let mint = create_mint_helper(&mut ctx, &rpc, &payer, 6).await; // 6 decimals example

    let owner_spl_ata =
        mint_spl_tokens(&mut ctx, &rpc, &payer, &mint, &owner.pubkey(), 1_000_000).await;

    // b) Create compressed-token ATA (ctoken) for owner, if required by the SDK.
    let ctoken_default_accounts = CTokenDefaultAccounts::new(&payer.pubkey(), &mint);
    let _owner_ctoken_ata = CreateAssociatedTokenAccount::instruction(
        &payer.pubkey(),
        &owner.pubkey(),
        &mint,
        &ctoken_default_accounts,
    )
    .unwrap();
    // (If there is an existing helper in other tests to do this, reuse that instead.)

    // c) Optionally, run a "mint_action" or initial transfer2 to create a compressed position
    // in the canonical_tree for owner. For the misdirection attack, it's enough to have
    // a valid input position to spend from, whichever pattern other tests use.
    //
    // TODO: reuse existing compressed-token-test helper for "create initial compressed balance".

    // 4) Build misdirected Transfer2Inputs
    // ---------------------------------------------------------
    // Goal: build a transfer that should logically credit some recipient for mint `mint`,
    // but whose outputs use a merkle_tree_index pointing at attacker_tree instead of canonical_tree.

    // In existing tests, look at how they construct `Transfer2Inputs` and `Transfer2Config`.
    // We follow that pattern, but ensure:
    //   - outputs[i].merkle_tree_index == index of attacker_tree in remaining accounts
    //   - not the index of canonical_tree.

    let recipient = Keypair::new();

    // Pseudo-code / structure: you'll need to fill these fields using patterns from other tests.
    let transfer_inputs = Transfer2Inputs {
        // token_data: MultiInputTokenDataWithContext { ... },
        // inputs: [... compressed input for owner ...],
        // outputs: [... compressed output for recipient, with output_queue/merkle_tree_index set to attacker_tree_index ...],
        // prove_by_index / proofs / etc.
        // TODO: fill using repo helpers
        ..Default::default()
    };

    // Decide whether you want system CPI or not.
    // For misdirection we *do* want the system program to actually update attacker_tree.
    let transfer_config = Transfer2Config {
        // Use same config as "happy path" transfer2 tests, except for output_queue index.
        // e.g., use_system_program_cpi: true,
        // etc.
        ..Default::default()
    };

    // Build account metas config; important part: include BOTH trees, but make
    // sure attacker_tree is at the index referenced by transfer_inputs.outputs[*].merkle_tree_index.
    let accounts_meta_config = Transfer2AccountsMetaConfig {
        // Fill this according to existing tests. Typical fields:
        // - state_tree_accounts
        // - output_queue_accounts
        // - system_program / account_compression_program / registry
        //
        // IMPORTANT: put `attacker_tree_pubkey` at the "output tree index" used in outputs.
        // TODO: fill using repo-specific types and patterns
        ..Default::default()
    };

    let ix = create_transfer2_instruction(
        &payer.pubkey(),
        &transfer_inputs,
        &transfer_config,
        &accounts_meta_config,
        &ctoken_default_accounts,
    )
    .expect("failed to build transfer2 misdirection instruction");

    let tx = Transaction::new_signed_with_payer(
        &[ix],
        Some(&payer.pubkey()),
        &[&payer, &owner], // include other signers if needed
        ctx.last_blockhash,
    );

    rpc.process_transaction(tx)
        .await
        .expect("misdirected transfer2 should succeed");

    // 5) Assert canonical tree unchanged, attacker tree updated
    // ---------------------------------------------------------
    // Fetch both trees' data and compare before/after.

    // let canonical_after = rpc.get_account_data::<PublicStateMerkleTreeAccount>(&canonical_tree_pubkey).await.unwrap();
    // let attacker_after = rpc.get_account_data::<PublicStateMerkleTreeAccount>(&attacker_tree_pubkey).await.unwrap();

    // TODO:
    // assert_eq!(canonical_before.root, canonical_after.root, "canonical tree root must not change");
    // assert_ne!(attacker_before.root, attacker_after.root, "attacker tree root must change");
    // Optionally: assert leaf_count increased on attacker tree.

    // 6) Spend/withdraw from attacker tree
    // ---------------------------------------------------------
    // Now build a second operation that uses the leaf in attacker_tree as an input:
    //   - either another transfer2 that spends this compressed balance, OR
    //   - a decompress/withdraw instruction that yields SPL tokens.

    // Use the same helpers used in other tests to:
    //   - construct an inclusion proof for the last leaf in attacker_tree,
    //   - create the corresponding input compressed account struct,
    //   - build the instruction.

    // For example (pseudo):
    //
    // let (compressed_input, merkle_context, proof) =
    //     build_last_leaf_input_for_tree(&mut ctx, &attacker_tree_pubkey).await;
    //
    // let spend_inputs = Transfer2Inputs {
    //     // single input using compressed_input + merkle_context + proof,
    //     // outputs to some new recipient or decompression.
    // };
    //
    // let spend_ix = create_transfer2_instruction(
    //     &payer.pubkey(),
    //     &spend_inputs,
    //     &transfer_config,
    //     &accounts_meta_config_for_attacker_tree,
    //     &ctoken_default_accounts,
    // )?;
    //
    // let spend_tx = Transaction::new_signed_with_payer(
    //     &[spend_ix],
    //     Some(&payer.pubkey()),
    //     &[&payer, &recipient],
    //     ctx.last_blockhash,
    // );
    //
    // rpc.process_transaction(spend_tx)
    //     .await
    //     .expect("spend from attacker tree should succeed");

    // Optional: assert that SPL balances / compressed balances reflect that the leaf
    // in attacker_tree was actually spendable.
}
