// SPDX-License-Identifier: MIT OR Apache-2.0
//! Typed error codes of the RFP-021 programs (U5, U2; branch `public-update`).
//!
//! Every refusal of the attestation program, the reference probe and the reference consumers
//! starts with one tag `RFP021-Enn`, then the code's fixed text, then (for the verifiers) a detail
//! about public beacon-chain data. The same function, [`classify`], turns a verifier's error into
//! its code on chain (the program panics with [`coded`]) and off chain (the dry-run checks in
//! `eth_attest::check`, and [`Code::parse`] of a wallet simulation's panic text), so both report
//! the same code for the same input. The table is in `docs/ERROR-CODES.md`.

/// One failure mode. The numbers are stable: never reuse one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum Code {
    /// The header is not finalized: an anchor this instance never recorded, or an update whose
    /// finality or execution-payload proof fails.
    HeaderNotFinalized = 1,
    /// The sync-committee signature does not verify (or a key witness is wrong).
    SignatureInvalid = 2,
    /// The committee handoff is not verified: signed in a period the store cannot check, a next
    /// committee whose proof fails, or a rotation to an unknown committee.
    HandoffNotVerified = 3,
    /// The checkpoint is stale or does not verify: a bootstrap that does not match the
    /// checkpoint, or an instance past its weak-subjectivity bound.
    CheckpointStale = 4,
    /// An inclusion proof (anchor header, account, storage, receipt) is invalid.
    InclusionProofInvalid = 5,
    /// The proven value does not satisfy the predicate, or is not the transfer a consumer takes.
    PredicateNotSatisfied = 6,
    /// The chain id does not match the instance's or the anchor's.
    ChainIdMismatch = 7,
    /// Malformed input: a message, update, committee or state that does not decode, or
    /// inconsistent fields.
    MalformedInput = 8,
    /// Stale or replayed: the store moved since this was verified, or this was already done
    /// (instance exists, job exists or finished, claim already made).
    StaleOrReplayed = 9,
    /// The update is valid but brings nothing new (not a later finalized header, no new committee).
    NotNewer = 10,
    /// Fewer signers than the instance's participation policy.
    ParticipationBelowPolicy = 11,
    /// Not permitted: the wrong sender, signer or destination account for this message.
    NotPermitted = 12,
    /// A witness or reference that is not the instance's: committee witness, config, instance or
    /// deployment.
    WitnessMismatch = 13,
}

impl Code {
    pub const ALL: [Code; 13] = [
        Code::HeaderNotFinalized,
        Code::SignatureInvalid,
        Code::HandoffNotVerified,
        Code::CheckpointStale,
        Code::InclusionProofInvalid,
        Code::PredicateNotSatisfied,
        Code::ChainIdMismatch,
        Code::MalformedInput,
        Code::StaleOrReplayed,
        Code::NotNewer,
        Code::ParticipationBelowPolicy,
        Code::NotPermitted,
        Code::WitnessMismatch,
    ];

    #[must_use]
    pub const fn number(self) -> u16 {
        self as u16
    }

    /// The tag and fixed text a refusal starts with.
    #[must_use]
    pub const fn prefix(self) -> &'static str {
        match self {
            Code::HeaderNotFinalized => "RFP021-E01 header not finalized",
            Code::SignatureInvalid => "RFP021-E02 signature invalid",
            Code::HandoffNotVerified => "RFP021-E03 handoff not verified",
            Code::CheckpointStale => "RFP021-E04 checkpoint stale or invalid",
            Code::InclusionProofInvalid => "RFP021-E05 inclusion proof invalid",
            Code::PredicateNotSatisfied => "RFP021-E06 predicate not satisfied",
            Code::ChainIdMismatch => "RFP021-E07 chain id mismatch",
            Code::MalformedInput => "RFP021-E08 malformed input",
            Code::StaleOrReplayed => "RFP021-E09 stale or replayed",
            Code::NotNewer => "RFP021-E10 not newer",
            Code::ParticipationBelowPolicy => "RFP021-E11 participation below policy",
            Code::NotPermitted => "RFP021-E12 not permitted",
            Code::WitnessMismatch => "RFP021-E13 witness mismatch",
        }
    }

    /// The code a refusal text carries (anywhere in it, so a wallet's "Guest panicked: …" parses).
    #[must_use]
    pub fn parse(text: &str) -> Option<Code> {
        let at = text.find("RFP021-E")?;
        let n: u16 = text.get(at + 8..at + 10)?.parse().ok()?;
        Code::ALL.into_iter().find(|c| c.number() == n)
    }
}

/// The code of a verifier error (the attestation program's own texts and Helios's
/// `ConsensusError` texts; first match wins). `None` only for a text no rule covers.
#[must_use]
pub fn classify(detail: &str) -> Option<Code> {
    const RULES: &[(&str, Code)] = &[
        ("RFP021-E", Code::MalformedInput), // already coded: handled below
        ("does not decode", Code::MalformedInput),
        ("pre-Capella", Code::MalformedInput),
        ("overflow", Code::MalformedInput),
        ("wrong length", Code::MalformedInput),
        ("invalid timestamp", Code::MalformedInput),
        ("invalid sync committee signature", Code::SignatureInvalid),
        ("below the instance's policy", Code::ParticipationBelowPolicy),
        ("insufficient participation", Code::ParticipationBelowPolicy),
        ("invalid sync committee period", Code::HandoffNotVerified),
        ("invalid next sync committee proof", Code::HandoffNotVerified),
        ("unknown one", Code::HandoffNotVerified),
        ("unknown committee", Code::HandoffNotVerified),
        ("stand-in committee", Code::HandoffNotVerified),
        ("invalid current sync committee proof", Code::CheckpointStale),
        ("invalid header hash", Code::CheckpointStale),
        ("checkpoint is too old", Code::CheckpointStale),
        ("weak-subjectivity", Code::CheckpointStale),
        ("bootstrap rejected", Code::CheckpointStale),
        ("invalid finality proof", Code::HeaderNotFinalized),
        ("invalid execution payload proof", Code::HeaderNotFinalized),
        ("update not relevant", Code::NotNewer),
        ("does not advance", Code::NotNewer),
        ("committee witness does not match", Code::WitnessMismatch),
        ("not this store's instance", Code::WitnessMismatch),
    ];
    if let Some(c) = Code::parse(detail) {
        return Some(c);
    }
    RULES.iter().skip(1).find(|(k, _)| detail.contains(k)).map(|(_, c)| *c)
}

/// A verifier error as the programs refuse with it: `RFP021-Enn <text>: <detail>`.
#[must_use]
pub fn coded(detail: &str) -> String {
    match classify(detail) {
        Some(c) if detail.starts_with("RFP021-E") => {
            let _ = c;
            detail.to_owned()
        }
        Some(c) => format!("{}: {detail}", c.prefix()),
        None => format!("RFP021-E08 malformed input: {detail}"),
    }
}
