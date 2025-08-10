package prover

import (
	"testing"

	"github.com/consensys/gnark-crypto/ecc"
	"github.com/consensys/gnark/backend/groth16"
	"github.com/consensys/gnark/frontend"
	"github.com/consensys/gnark/test"
)

func TestFrozenHeart_MissingStartIndex(t *testing.T) {
    assert := test.NewAssert(t)

    // 1) Setup append prover/verifier
    ps, err := SetupBatchAppend(3, 2)
    assert.NoError(err)

    // 2) Build params with StartIndex = 5
    params := BuildTestBatchAppendTree(3, 2, nil, 5, false)
    proof, err := ps.ProveBatchAppend(params)
    assert.NoError(err)

    // 3) Verify under append circuit (sanity check)
    goodPub := BatchAppendCircuit{
        PublicInputHash: frontend.Variable(params.PublicInputHash),
        Height:          3,
        BatchSize:       2,
        StartIndex:      5,
    }
    goodWit, err := frontend.NewWitness(&goodPub, ecc.BN254.ScalarField(), frontend.PublicOnly())
    assert.NoError(err)
    err = groth16.Verify(proof.Proof, ps.VerifyingKey, goodWit)
    assert.NoError(err, "should verify with correct StartIndex")

    // 4) Now forge a *different* public witness: StartIndex = 0
    badPub := BatchAppendCircuit{
        PublicInputHash: frontend.Variable(params.PublicInputHash),
        Height:          3,
        BatchSize:       2,
        StartIndex:      0,
    }
    badWit, err := frontend.NewWitness(&badPub, ecc.BN254.ScalarField(), frontend.PublicOnly())
    assert.NoError(err)
    err = groth16.Verify(proof.Proof, ps.VerifyingKey, badWit)

    // 5) THIS should *still* pass, because StartIndex was never hashed in
    assert.NoError(err, "Frozen Heart: proof verifies even with wrong StartIndex")
}


