# RFP-021 error codes

Every refusal of the RFP-021 attestation program, its reference probe and the reference
consumers starts with a tag `RFP021-Enn`, then the code's fixed text, then a detail about public
data:

```
RFP021-E02 signature invalid: finality update rejected: invalid sync committee signature
```

On a chain, the tag is in the transaction's revert reason, e.g. a wallet's simulation answers
`Failed to execute program: Guest panicked: RFP021-E02 …`. Off the chain, the dry-run checks in
`eth_attest::check` (`bootstrap`, `update`, `finality_public`, `period_begin`, `period_finish`,
`code_of_inclusion`, and `call` for any handler) return the same `Code` for the same input:
both go through `eth_attest_core::codes::classify` (verifier errors) or are the program's own
handler. `Code::parse` reads the tag back out of any text. Source:
`lez-bridge/attest/core/src/codes.rs`; tests: `lez-bridge/attest/program/tests/error_codes.rs`
(one per code, the program's refusal and the dry run compared). Branch `public-update`; RFP-021
U5 and U2.

| Code | Name | RFP-021 U5 failure mode | Raised when (examples) |
|---|---|---|---|
| E01 | header not finalized | header not finalised | `AssertAnchor` for an anchor this instance never recorded; an update whose finality proof or execution-payload proof fails |
| E02 | signature invalid | signature invalid | the sync-committee signature does not verify (also a wrong key witness, or a period job's Finish) |
| E03 | handoff not verified | handoff not verified | an update signed in a period the store has no committee for; a next committee whose proof fails; a rotation to an unknown committee |
| E04 | checkpoint stale or invalid | checkpoint stale / outside the weak-subjectivity period | a bootstrap that does not verify against the configured checkpoint; an instance past its weak-subjectivity bound |
| E05 | inclusion proof invalid | inclusion proof invalid | the anchor header, account, storage or receipt proof does not verify, a failed receipt, no log at that index |
| E06 | predicate not satisfied | predicate not satisfied | the proven value fails the predicate; a log that is not the transfer a consumer takes |
| E07 | chain id mismatch | chain-ID mismatch | an anchor or instance for another chain id; a store or advance for another chain |
| E08 | malformed input | malformed input | a message, update, bootstrap, committee or state that does not decode; inconsistent fields |
| E09 | stale or replayed | (also: R8 duplicate update) | the store moved since the update was verified; an instance, job or claim that already exists |
| E10 | not newer | (R8) | a valid update that brings no later finalized header and no new committee |
| E11 | participation below policy | (participation) | fewer signers than the instance's policy |
| E12 | not permitted | (structural) | an effect from another program, an instruction not from the transaction's signer, a message to the wrong account |
| E13 | witness mismatch | (malformed witness) | a committee witness, config, instance or deployment that is not the instance's |

The numbers are stable: a code is never reused for another meaning. The bridge program keeps its
own single refusal for claims (`claim refused`, `DESIGN.md` §2), so a claim's failure never says
which secret-dependent check failed.
