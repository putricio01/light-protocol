package prover

import (
    "math/big"
    "testing"

    "github.com/consensys/gnark/backend/groth16"
    "github.com/consensys/gnark-crypto/ecc"
    "github.com/consensys/gnark/frontend"
)

func TestProofSwapZeroHashCollision(t *testing.T) {
    treeDepth := 10
    batchSize := 0        // empty slice → leaf hash-chain = 0
    startIndex := 0

    // 1. Build BatchAppend parameters with no leaves
    appendParams := BuildTestBatchAppendTree(treeDepth, batchSize, nil, startIndex, false)

    // 2. Generate a proof for BatchAppend
    psAppend, err := SetupBatchAppend(uint32(treeDepth), uint32(batchSize))
    if err != nil { t.Fatal(err) }
    proof, err := psAppend.ProveBatchAppend(appendParams)
    if err != nil { t.Fatal(err) }

    // 3. Construct BatchUpdate parameters using the same public-input hash
    updateParams := &BatchUpdateParameters{
        PublicInputHash:     appendParams.PublicInputHash,
        OldRoot:             appendParams.OldRoot,
        NewRoot:             appendParams.NewRoot,
        LeavesHashchainHash: appendParams.LeavesHashchainHash, // zero
        TxHashes:            []*big.Int{},
        Leaves:              []*big.Int{},
        OldLeaves:           []*big.Int{},
        PathIndices:         []uint32{},
        MerkleProofs:        [][]big.Int{},
        Height:              uint32(treeDepth),
        BatchSize:           uint32(batchSize),
    }

    // 4. Verify the proof using the BatchAppend verifying key (baseline success)
    publicWitnessAppend, err := frontend.NewWitness(&BatchAppendCircuit{PublicInputHash: appendParams.PublicInputHash}, ecc.BN254.ScalarField(), frontend.PublicOnly())
    if err != nil { t.Fatal(err) }
    if err := groth16.Verify(
        proof.Proof,
        psAppend.VerifyingKey,
        publicWitnessAppend,
    ); err != nil {
        t.Fatalf("append verification failed: %v", err)
    }

    // 5. Verify with the BatchUpdate verifying key – should fail
    psUpdate, err := SetupBatchUpdate(uint32(treeDepth), uint32(batchSize))
    if err != nil { t.Fatal(err) }
    publicWitnessUpdate, err := frontend.NewWitness(&BatchUpdateCircuit{PublicInputHash: updateParams.PublicInputHash}, ecc.BN254.ScalarField(), frontend.PublicOnly())
    if err != nil { t.Fatal(err) }
    if err := groth16.Verify(
        proof.Proof,
        psUpdate.VerifyingKey,
        publicWitnessUpdate,
    ); err == nil {
        t.Fatalf("expected verification with BatchUpdate VK to fail")
    }

    // 6. “Swapped” verification: invoke BatchUpdate logic but
    //     supply the BatchAppend verifying key. This succeeds,
    //     proving the verifier does not bind to the circuit ID.
    if err := groth16.Verify(
        proof.Proof,
        psAppend.VerifyingKey, // swapped VK
        publicWitnessUpdate,
    ); err != nil {
        t.Fatalf("proof swapping should verify: %v", err)
    }
}