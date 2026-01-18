# Gnark Circuit Constraint Coverage Audit Report

## Executive Summary

**All witness values are properly constrained.** After analyzing all circuit files, every field assigned in witness creation functions is properly constrained in the circuit's `Define()` method. No unconstrained witness variables were discovered.

## Audit Scope

This audit examined:
- `prover/server/prover/v2/batch_update_circuit.go`
- `prover/server/prover/v2/batch_append_circuit.go`
- `prover/server/prover/v2/batch_address_append_circuit.go`
- `prover/server/prover/v2/inclusion_circuit.go`
- `prover/server/prover/v2/non_inclusion_circuit.go`
- `prover/server/prover/v2/combined_circuit.go`
- `prover/server/prover/common/circuit_utils.go`

## Circuit-by-Circuit Analysis

### 1. BatchUpdateCircuit

| Witness Field | Constraint Mechanism |
|---------------|---------------------|
| `PublicInputHash` | Public input, constrained via hash chain assertion (line 41) |
| `OldRoot` | Hash chain input (line 37) + MerkleRootUpdateGadget verification (line 66) |
| `NewRoot` | Hash chain input (line 38) + final root assertion (line 75) |
| `LeavesHashchainHash` | Hash chain input (line 39) + nullifier hash chain assertion (line 59) |
| `TxHashes[]` | All elements used in Poseidon3 nullifier hash (line 55) |
| `Leaves[]` | All elements used in Poseidon3 nullifier hash (line 55) |
| `OldLeaves[]` | All elements used in MerkleRootUpdateGadget (line 67) |
| `MerkleProofs[][]` | All elements used in MerkleRootUpdateGadget loop (line 70) |
| `PathIndices[]` | ToBinary (line 64) + nullifier hash (line 55) + MerkleRootUpdateGadget (line 69) |

**Verdict: PROPERLY CONSTRAINED**

### 2. BatchAppendCircuit

| Witness Field | Constraint Mechanism |
|---------------|---------------------|
| `PublicInputHash` | Public input, assertion (line 41) |
| `OldRoot` | Hash chain (line 35) + MerkleRootUpdateGadget (line 61) |
| `NewRoot` | Hash chain (line 36) + final assertion (line 70) |
| `LeavesHashchainHash` | Hash chain (line 37) + assertion (line 54) |
| `StartIndex` | Hash chain (line 38) + ToBinary via Add (line 59) |
| `OldLeaves[]` | IsZero selector (line 49) + MerkleRootUpdateGadget (line 62) |
| `Leaves[]` | Select (line 50) + createHashChain (line 53) |
| `MerkleProofs[][]` | MerkleRootUpdateGadget (line 65) |

**Verdict: PROPERLY CONSTRAINED**

### 3. BatchAddressTreeAppendCircuit

| Witness Field | Constraint Mechanism |
|---------------|---------------------|
| `PublicInputHash` | Public input, assertion (line 89) |
| `OldRoot` | computePublicInputHash (line 96) + initial currentRoot (line 40) |
| `NewRoot` | computePublicInputHash (line 97) + assertion (line 83) |
| `HashchainHash` | computePublicInputHash (line 98) + assertion (line 86) |
| `StartIndex` | computePublicInputHash (line 99) + ToBinary via Add (line 72) |
| `LowElementValues[]` | LeafHashGadget (line 44) + Poseidon2 (line 50) |
| `LowElementNextValues[]` | LeafHashGadget (line 45) + Poseidon2 newLeafHash (line 69) |
| `LowElementIndices[]` | ToBinary + MerkleRootUpdateGadget (lines 54-55) |
| `LowElementProofs[][]` | MerkleRootUpdateGadget (line 60) |
| `NewElementValues[]` | LeafHashGadget (line 46) + Poseidon2 (line 51) + createHashChain (line 85) |
| `NewElementProofs[][]` | MerkleRootUpdateGadget (line 78) |

**Verdict: PROPERLY CONSTRAINED**

### 4. InclusionCircuit

| Witness Field | Constraint Mechanism |
|---------------|---------------------|
| `PublicInputHash` | Public input, assertion (line 28) |
| `Roots[]` | Hash chain (line 27) + MerkleRootGadget assertion (circuit_utils.go:78) |
| `Leaves[]` | Hash chain (line 27) + MerkleRootGadget (circuit_utils.go:73) |
| `InPathIndices[]` | ToBinary (circuit_utils.go:71) + MerkleRootGadget (circuit_utils.go:74) |
| `InPathElements[][]` | MerkleRootGadget (circuit_utils.go:75) |

**Verdict: PROPERLY CONSTRAINED**

### 5. NonInclusionCircuit

| Witness Field | Constraint Mechanism |
|---------------|---------------------|
| `PublicInputHash` | Public input, assertion (line 32) |
| `Roots[]` | Hash chain (line 31) + MerkleRootGadget assertion (circuit_utils.go:114) |
| `Values[]` | Hash chain (line 31) + LeafHashGadget (circuit_utils.go:104) |
| `LeafLowerRangeValues[]` | LeafHashGadget + AssertIsLess (circuit_utils.go:102, 158) |
| `LeafHigherRangeValues[]` | LeafHashGadget + AssertIsLess (circuit_utils.go:103, 160) |
| `InPathIndices[]` | ToBinary + MerkleRootGadget (circuit_utils.go:107, 111) |
| `InPathElements[][]` | MerkleRootGadget (circuit_utils.go:111) |

**Verdict: PROPERLY CONSTRAINED**

## Key Gadget Analysis

### MerkleRootUpdateGadget (circuit_utils.go:201-226)

This gadget is critical for all batch circuits. It properly constrains:

1. **OldRoot** - Verified via assertion against computed root (line 217)
2. **OldLeaf** - Used as input to MerkleRootGadget (line 212)
3. **NewLeaf** - Used as input to MerkleRootGadget (line 219)
4. **PathIndex[]** - ALL elements iterated in MerkleRootGadget loop (lines 191-198)
5. **MerkleProof[]** - ALL elements iterated in MerkleRootGadget loop (lines 191-198)

```go
func (gadget MerkleRootGadget) DefineGadget(api frontend.API) interface{} {
    currentHash := gadget.Hash
    for i := 0; i < gadget.Height; i++ {  // iterates through ALL elements
        currentHash = abstractor.Call(api, ProveParentHash{
            Bit:     gadget.Index[i],      // uses every index bit
            Hash:    currentHash,
            Sibling: gadget.Path[i],       // uses every proof sibling
        })
    }
    return currentHash
}
```

The `Height` parameter ensures ALL array elements are used - there are no unused elements.

## Protection Mechanisms

The circuits employ several sound security patterns:

1. **Public Input Hash Chains**: All semi-public inputs are bound together via Poseidon hash chains, preventing selective manipulation.

2. **Merkle Proof Iteration**: The `MerkleRootGadget` loop iterates through exactly `Height` elements, ensuring:
   - All `PathIndex` bits are used
   - All `MerkleProof` siblings are used
   - No unused proof elements that could be set arbitrarily

3. **ToBinary Constraints**: Path indices are decomposed to binary, adding bit constraints.

4. **Root Assertions**: Computed Merkle roots are always compared against expected roots.

5. **Hash Chain Verification**: The nullifier/leaves hash chains ensure all leaves are processed.

## Conclusion

**No vulnerabilities found.** All witness values assigned in `CreateWitness`, `ProveBatchUpdate`, `ProveBatchAppend`, etc. are properly constrained in their respective circuit `Define()` methods. The array iterations are bounded by `Height` and `BatchSize` parameters which match the allocated array sizes, ensuring no elements are left unconstrained.

The design follows sound practices where:
- Every input flows into at least one constraint
- Array bounds are explicit and enforced
- Merkle proof verification uses all siblings
- Hash chains bind all relevant inputs together
