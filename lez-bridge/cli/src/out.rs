//! Machine-readable output (`--json`, modules/CLI-CONTRACT.md §0.3, §1, §3) and exit codes (§0.6).
//!
//! With `--json`, stdout carries only JSON: one document for a read, or NDJSON events for a job,
//! ending in exactly one `result` or `error` event. Everything else this process prints, the
//! LEZ wallet crate's own lines included, goes to stderr: at start the real stdout is kept on a
//! duplicate descriptor for the JSON, and descriptor 1 is pointed at stderr.

use std::{
    io::Write as _,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use serde_json::{Value, json};

static JSON: AtomicBool = AtomicBool::new(false);
static JSON_OUT: OnceLock<Mutex<std::fs::File>> = OnceLock::new();
/// A job command (§3) has started its event stream.
static JOB: AtomicBool = AtomicBool::new(false);
/// The job reached `submit`: an error after this is not "refused before sending".
static SENT: AtomicBool = AtomicBool::new(false);
/// The job reached `prove`: only then is a failure a proof failure (exit 4).
static PROVING: AtomicBool = AtomicBool::new(false);

/// Turns `--json` on: keeps the real stdout for JSON and sends descriptor 1 to stderr.
pub fn init(json: bool) {
    if !json {
        return;
    }
    use std::os::fd::FromRawFd as _;
    // SAFETY: plain descriptor calls at start-up, before any other thread writes to stdout.
    let kept = unsafe { libc::dup(1) };
    if kept < 0 {
        return;
    }
    let _ = std::io::stdout().flush();
    // SAFETY: as above; 1 and 2 are open for the whole process.
    unsafe {
        libc::dup2(2, 1);
    }
    // SAFETY: `kept` is a fresh descriptor this process owns from here on.
    let file = unsafe { std::fs::File::from_raw_fd(kept) };
    let _ = JSON_OUT.set(Mutex::new(file));
    JSON.store(true, Ordering::Relaxed);
}

#[must_use]
pub fn json() -> bool {
    JSON.load(Ordering::Relaxed)
}

/// One JSON line on the real stdout (only with `--json`).
pub fn emit(v: &Value) {
    if let Some(out) = JSON_OUT.get()
        && let Ok(mut f) = out.lock()
    {
        let _ = writeln!(f, "{v}");
        let _ = f.flush();
    }
}

/// A read's one document: JSON with `--json`, else nothing (the caller prints its text).
pub fn doc(v: &Value) {
    if json() {
        emit(v);
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Starts a job's event stream with `phase: fetch`.
pub fn job_start() {
    JOB.store(true, Ordering::Relaxed);
    phase("fetch", json!({}));
}

#[must_use]
pub fn in_job() -> bool {
    JOB.load(Ordering::Relaxed)
}

/// `{"event":"phase","phase":…,"at_ms":…}` plus `extra`'s fields.
pub fn phase(name: &str, extra: Value) {
    if name == "submit" {
        SENT.store(true, Ordering::Relaxed);
    }
    if name == "prove" {
        PROVING.store(true, Ordering::Relaxed);
    }
    let mut v = json!({ "event": "phase", "phase": name, "at_ms": now_ms() });
    merge(&mut v, extra);
    if json() {
        emit(&v);
    }
}

/// Any other event: `{"event": name, …fields}`.
pub fn event(name: &str, fields: Value) {
    let mut v = json!({ "event": name });
    merge(&mut v, fields);
    if json() {
        emit(&v);
    }
}

fn merge(v: &mut Value, extra: Value) {
    if let (Some(o), Value::Object(e)) = (v.as_object_mut(), extra) {
        o.extend(e);
    }
}

static RESULT: Mutex<Option<Value>> = Mutex::new(None);

/// What the job's final `result` event carries besides `ok` (the last send's, for a job that
/// sends more than once).
pub fn set_result(fields: Value) {
    if let Ok(mut r) = RESULT.lock() {
        *r = Some(fields);
    }
}

/// Ends a successful job's stream with its one `result` event.
pub fn finish() {
    let mut v = json!({ "event": "result", "ok": true });
    if let Some(fields) = RESULT.lock().ok().and_then(|mut r| r.take()) {
        merge(&mut v, fields);
    }
    if json() {
        emit(&v);
    }
}

/// The error codes of CLI-CONTRACT.md §1 and the exit code of §0.6, from the error's text.
#[must_use]
pub fn classify(message: &str) -> (&'static str, i32) {
    const CODES: &[(&str, &str, i32)] = &[
        ("claim for another deployment", "cfg_mismatch", 1),
        ("token not registered", "token_not_registered", 1),
        ("minting is frozen on this bridge", "minting_frozen", 1),
        ("minting is frozen for this token", "minting_frozen_token", 1),
        ("bad denomination", "bad_denomination", 1),
        ("wrong amount", "wrong_amount", 1),
        ("is not this deposit", "wrong_leaf", 1),
        ("the statement does not verify", "proof_invalid", 1),
        ("storage proof", "proof_invalid", 1),
        ("is not attested", "root_not_attested", 1),
        ("insufficient finality", "anchor_not_finalized", 1),
        ("deposit already claimed", "already_claimed", 1),
        ("the bridge float holds", "float_empty", 1),
        ("already burned", "already_burned", 1),
        ("is not loopback", "not_loopback", 1),
        ("neither loopback nor the public demo chain", "not_loopback", 1),
        ("no wallet yet", "wallet_missing", 6),
        ("but reverted", "reverted", 5),
        ("proof", "proof_failed", 4),
        ("error sending request", "unreachable", 3),
        ("connection refused", "unreachable", 3),
        ("Connection refused", "unreachable", 3),
        ("timed out", "unreachable", 3),
    ];
    for (needle, code, exit) in CODES {
        if message.contains(needle) {
            // A proving failure only counts once the job got that far; before it the text
            // "proof" means an Ethereum storage proof (handled above) or a usage problem.
            if *code == "proof_failed" && !PROVING.load(Ordering::Relaxed) {
                continue;
            }
            return (code, *exit);
        }
    }
    ("failed", 1)
}

/// Ends the process on an error: the `error` event (§1) with `--json`, the plain `Error:` line
/// otherwise, and the §0.6 exit code.
pub fn fail(error: &anyhow::Error) -> ! {
    let message = format!("{error:#}");
    let (code, exit) = classify(&message);
    let sent = SENT.load(Ordering::Relaxed);
    eprintln!("Error: {message}");
    if json() {
        emit(&json!({ "event": "error", "code": code, "message": message, "sent": sent, "local_only": !sent && exit == 1 }));
    }
    std::process::exit(exit)
}

#[cfg(test)]
mod tests {
    use super::classify;

    #[test]
    fn texts_map_to_the_contract_codes() {
        assert_eq!(classify("deposit already claimed: its nullifier is spent; nothing sent"), ("already_claimed", 1));
        assert_eq!(classify("leaf 3 is not this deposit (commitment 0x.. for X)"), ("wrong_leaf", 1));
        assert_eq!(classify("abc was included in block 9 but reverted"), ("reverted", 5));
        assert_eq!(classify("no wallet yet: run `lez-bridge wallet check-health` first"), ("wallet_missing", 6));
        assert_eq!(classify("http://x is not loopback; this client only talks to a local chain"), ("not_loopback", 1));
        assert_eq!(classify("http://10.0.0.1:3250 is neither loopback nor the public demo chain https://lez.nameswallet.app/; this client talks to no other sequencer"), ("not_loopback", 1));
        assert_eq!(classify("something else"), ("failed", 1));
    }
}
