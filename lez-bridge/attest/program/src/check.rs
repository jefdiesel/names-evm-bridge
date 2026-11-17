// SPDX-License-Identifier: MIT OR Apache-2.0
//! Off-chain dry-run verification with typed codes (RFP-021 U2, U5; branch `public-update`).
//!
//! The same verifiers the program runs, returning [`Code`] instead of panicking: the code is the
//! one the program's refusal carries for the same input, because both go through
//! [`eth_attest_core::codes::classify`] (verifiers) or are the program's own handler ([`call`]).
//! A client checks an update or statement here before it builds a transaction, and parses a
//! wallet simulation's refusal with [`Code::parse`].

pub use eth_attest_core::codes::Code;
use eth_attest_core::{
    InstanceConfig, LcSummary, LcUpdate,
    codes::classify,
    inclusion::InclusionError,
};
use lee_core::account::AccountId;

use crate::lc::{self, Verified};

/// The code of a verifier's error text.
#[must_use]
pub fn code_of(error: &str) -> Code {
    classify(error).unwrap_or(Code::MalformedInput)
}

/// The code of an inclusion error (statements, consumers).
#[must_use]
pub fn code_of_inclusion(e: &InclusionError) -> Code {
    e.class().code()
}

/// `Init`'s verification.
///
/// # Errors
/// The code the program would refuse with.
pub fn bootstrap(program: &AccountId, config: &InstanceConfig, bootstrap: &[u8]) -> Result<LcSummary, Code> {
    lc::verify_bootstrap(program, config, bootstrap).map_err(|e| code_of(&e))
}

/// The private `Update`'s verification.
///
/// # Errors
/// The code the program would refuse with.
pub fn update(
    program: &AccountId,
    config: &InstanceConfig,
    prev: &LcSummary,
    current: &[u8],
    next: Option<&[u8]>,
    update: &LcUpdate,
) -> Result<Verified, Code> {
    lc::verify_update(program, config, prev, current, next, update).map_err(|e| code_of(&e))
}

/// `UpdatePublic`'s verification.
///
/// # Errors
/// The code the program would refuse with.
#[cfg(feature = "public-update")]
pub fn finality_public(
    program: &AccountId,
    config: &InstanceConfig,
    prev: &LcSummary,
    committee: &[u8],
    keys: &eth_attest_core::KeyWitnesses,
    update: &[u8],
) -> Result<Verified, Code> {
    crate::public::verify_finality_public(program, config, prev, committee, keys, update).map_err(|e| code_of(&e))
}

/// `PeriodBegin`'s verification.
///
/// # Errors
/// The code the program would refuse with.
#[cfg(feature = "public-update")]
pub fn period_begin(program: &AccountId, config: &InstanceConfig, prev: &LcSummary, update: &[u8]) -> Result<eth_attest_core::PeriodJob, Code> {
    crate::public::verify_period_begin(program, config, prev, update).map_err(|e| code_of(&e))
}

/// `FinalityBegin`'s verification (the pairing-seam split, up to the Miller loop).
///
/// # Errors
/// The code the program would refuse with.
#[cfg(feature = "public-update")]
pub fn finality_begin(
    program: &AccountId,
    config: &InstanceConfig,
    prev: &LcSummary,
    committee: &[u8],
    keys: &eth_attest_core::KeyWitnesses,
    update: &[u8],
) -> Result<eth_attest_core::PeriodJob, Code> {
    crate::public::verify_finality_begin(program, config, prev, committee, keys, update).map_err(|e| code_of(&e))
}

/// `FinalityFinish`'s final exponentiation.
///
/// # Errors
/// The code the program would refuse with.
#[cfg(feature = "public-update")]
pub fn finality_finish(job: &eth_attest_core::PeriodJob) -> Result<(), Code> {
    crate::public::verify_finality_finish(job).map_err(|e| code_of(&e))
}

/// `PeriodFinish`'s signature check.
///
/// # Errors
/// The code the program would refuse with.
#[cfg(feature = "public-update")]
pub fn period_finish(job: &eth_attest_core::PeriodJob, committee: &[u8], keys: &eth_attest_core::KeyWitnesses) -> Result<(), Code> {
    crate::public::verify_period_finish(job, committee, keys).map_err(|e| code_of(&e))
}

/// Any call of a program handler, natively (the attestation program's, the probe's or a
/// consumer's): its response, or the code its refusal carries. Native only.
///
/// # Errors
/// The refusal's code ([`Code::MalformedInput`] for a panic that carries none).
#[cfg(not(target_os = "zkvm"))]
pub fn call(
    handler: fn(&lee_core::program::ReceiveInput) -> lee_core::program::Response,
    input: &lee_core::program::ReceiveInput,
) -> Result<lee_core::program::Response, Code> {
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handler(input)));
    r.map_err(|e| {
        let text = e
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| e.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_default();
        Code::parse(&text).unwrap_or(Code::MalformedInput)
    })
}
