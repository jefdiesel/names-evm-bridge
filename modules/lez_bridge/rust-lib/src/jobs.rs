//! Background jobs: the CLI runs as its own process group, one wallet job at a time.
//!
//! A proof outlasts any module call (about 11 minutes for a private claim), so every send is a
//! job. Each job has `<id>.json` (its record, 0600) and `<id>.log` (the CLI's output, 0600) in
//! the jobs folder. A watcher thread follows the process: it notes each phase as the log reaches
//! it (fetch → simulate → prove → submit → confirm), samples the group's memory, and writes how
//! it ended. A job keeps its record across a module restart: one whose process died unwatched
//! is read from its log (`done` only if the log says it landed, otherwise `unknown`, never a
//! guessed failure); one whose process still runs is followed again by `adopt`.
//!
//! Pause and resume stop and continue the whole group (SIGSTOP/SIGCONT). They are refused for
//! a job wrapped in the memory guard, whose own pause a module SIGCONT could undo
//! (CLI-CONTRACT.md §3). Cancel sends SIGTERM to the group, then SIGKILL after five seconds.

use crate::text;
use crate::util::{create_private, now_ms, private_dir, read_json, write_private};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

/// Jobs a watcher thread of this process follows.
static WATCHED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
/// Record files are read-modified-written by the watcher and by pause/resume/cancel.
static RECORDS: Mutex<()> = Mutex::new(());

const TICK: Duration = Duration::from_millis(250);
const MEMORY_EVERY_MS: u64 = 2_000;
/// Nothing the module starts runs longer (the slowest proof measured is 7,075 s, an RFP-021
/// update the module never runs; a claim is about 11 min).
const JOB_LIMIT_MS: u64 = 60 * 60 * 1000;
const KEEP_FINISHED_MS: u64 = 14 * 24 * 60 * 60 * 1000;
const TAIL_LINES: usize = 24;
/// The phases in order (CLI-CONTRACT.md §3).
const PHASES: [&str; 6] = ["fetch", "simulate", "prove", "submit", "confirm", "done"];

pub struct Jobs {
    pub dir: PathBuf,
}

/// What a job runs: one or more CLI commands in order (a redeem burns several pieces).
pub struct Spec {
    pub kind: String,
    pub subject: String,
    pub steps: Vec<Command>,
    pub dry_run: bool,
    pub guarded: bool,
    /// Seconds the `prove` phase is expected to take (0: no proof).
    pub prove_estimate_s: u64,
    pub peak_estimate_mb: u64,
    pub meta: Value,
    /// What each step reads on stdin, by step (missing or `None`: nothing). How a secret reaches
    /// the CLI (`--secret-file -`) without argv or a file on disk; never written to the record or
    /// the log.
    pub inputs: Vec<Option<Vec<u8>>>,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 96 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

pub fn alive(pid: u64) -> bool {
    pid > 1
        && Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
}

fn signal_group(pgid: u64, signal: &str) -> bool {
    pgid > 1
        && Command::new("/bin/kill")
            .args([&format!("-{signal}"), "--", &format!("-{pgid}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
}

/// Process ids in group `pgid`.
pub fn group_members(pgid: u64) -> Vec<u64> {
    let out = Command::new("/bin/ps").args(["-A", "-o", "pid=,pgid="]).stdin(Stdio::null()).stderr(Stdio::null()).output();
    let Ok(out) = out else { return Vec::new() };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            let pid: u64 = w.next()?.parse().ok()?;
            let g: u64 = w.next()?.parse().ok()?;
            (g == pgid).then_some(pid)
        })
        .collect()
}

/// Resident memory of group `pgid` in MB. RSS under-reads once macOS compresses pages (a 9.4 GB
/// footprint showed as about 6.5 GB RSS), so the record calls it RSS, not footprint.
fn group_rss_mb(pgid: u64) -> u64 {
    let out = Command::new("/bin/ps").args(["-A", "-o", "pgid=,rss="]).stdin(Stdio::null()).stderr(Stdio::null()).output();
    let Ok(out) = out else { return 0 };
    let kb: u64 = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            let g: u64 = w.next()?.parse().ok()?;
            let rss: u64 = w.next()?.parse().ok()?;
            (g == pgid).then_some(rss)
        })
        .sum();
    kb / 1024
}

/// Lines a log tail never shows: the wallet's own chatter, and the claim nullifier, which would
/// link a later claim of an unclaimed deposit to whoever saw the tail (screenshots, fixtures).
fn shown(line: &str) -> bool {
    let t = line.trim_start();
    !(t.starts_with("Transaction data") || t.starts_with("Stored") || t.starts_with("claim nullifier"))
}

impl Jobs {
    fn path(&self, id: &str, ext: &str) -> PathBuf {
        self.dir.join(format!("{id}.{ext}"))
    }

    /// The key of job `id` among the watched ones: its record's path, unique across folders.
    fn key(&self, id: &str) -> String {
        self.path(id, "json").display().to_string()
    }

    fn record(&self, id: &str) -> Option<Value> {
        read_json(&self.path(id, "json"))
    }

    fn write(&self, id: &str, record: &Value) {
        let _ = write_private(&self.path(id, "json"), record.to_string().as_bytes());
    }

    /// Changes a record on disk under the records lock.
    fn update(&self, id: &str, change: impl FnOnce(&mut Value)) -> Option<Value> {
        let _guard = RECORDS.lock().ok()?;
        let mut r = self.record(id)?;
        change(&mut r);
        self.write(id, &r);
        Some(r)
    }

    fn ids(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        let mut ids: Vec<String> = entries
            .filter_map(|e| e.ok()?.file_name().into_string().ok()?.strip_suffix(".json").filter(|id| valid_id(id)).map(str::to_owned))
            .collect();
        ids.sort();
        ids
    }

    /// The job that holds the wallet, if one runs (or is paused).
    pub fn busy(&self) -> Option<Value> {
        self.ids().into_iter().filter_map(|id| self.state(&id).ok()).find(|s| matches!(s["state"].as_str(), Some("running" | "paused")))
    }

    fn forget_old(&self) {
        let now = now_ms();
        for id in self.ids() {
            if self.record(&id).and_then(|r| r["finished"].as_u64()).is_some_and(|at| now.saturating_sub(at) > KEEP_FINISHED_MS) {
                for ext in ["json", "log"] {
                    let _ = std::fs::remove_file(self.path(&id, ext));
                }
            }
        }
    }

    /// Starts `spec` and returns at once with the job's state. Refused while another wallet job
    /// runs: two would each save the wallet from the copy they read when they started.
    pub fn start(&self, spec: Spec) -> Result<Value, String> {
        private_dir(&self.dir).map_err(|e| format!("cannot make {}: {e}", self.dir.display()))?;
        if let Some(job) = self.busy() {
            return Err(format!(
                "the wallet is busy with \"{}\" (job {}); one wallet job runs at a time, so wait for it, or cancel it",
                job["kind"].as_str().unwrap_or("a job"),
                job["id"].as_str().unwrap_or_default()
            ));
        }
        if spec.steps.is_empty() {
            return Err("nothing to run".to_owned());
        }
        self.forget_old();
        let mut id = format!("{}-{}", spec.kind.replace('_', "-"), now_ms());
        while self.path(&id, "json").exists() {
            id.push_str("-1");
        }
        if !valid_id(&id) {
            return Err(format!("not a job id: {id}"));
        }
        let log = create_private(&self.path(&id, "log")).map_err(|e| format!("cannot write the job's log: {e}"))?;
        let started = now_ms();
        let record = json!({
            "id": id, "kind": spec.kind, "subject": spec.subject, "state": "running", "phase": "fetch",
            "phases": [{ "phase": "fetch", "at": started }], "started": started, "finished": null,
            "paused_ms": 0, "paused_since": null, "pid": null, "pgid": null, "step": 0, "steps": spec.steps.len(),
            "dry_run": spec.dry_run, "guarded": spec.guarded, "prove_estimate_s": spec.prove_estimate_s,
            "peak_estimate_mb": spec.peak_estimate_mb, "peak_rss_mb": 0, "exit": null, "error": null, "result": null,
            "cancel_requested": false, "meta": spec.meta,
        });
        let mut steps = spec.steps;
        let inputs = spec.inputs;
        let first = spawn(&mut steps[0], &log, inputs.first().cloned().flatten().as_deref())?;
        let mut record = record;
        record["pid"] = Value::from(first.id());
        record["pgid"] = Value::from(first.id());
        self.write(&id, &record);
        if let Ok(mut w) = WATCHED.lock() {
            w.insert(self.key(&id));
        }
        let jobs = Jobs { dir: self.dir.clone() };
        let job = id.clone();
        std::thread::spawn(move || jobs.watch(&job, first, steps, inputs, log));
        self.state(&id)
    }

    /// Follows job `id` until its last step ends.
    fn watch(&self, id: &str, mut child: Child, mut steps: Vec<Command>, inputs: Vec<Option<Vec<u8>>>, log: std::fs::File) {
        let mut step = 0usize;
        let mut last_sample = 0u64;
        let outcome: Result<Option<i32>, String> = loop {
            let pgid = child.id() as u64;
            let now = now_ms();
            // Phases, from what the log says so far.
            let phase = text::work_phase_of(&self.whole_log(id));
            let rec = self.update(id, |r| {
                advance(r, phase, now);
                if now.saturating_sub(last_sample) >= MEMORY_EVERY_MS {
                    let mb = group_rss_mb(pgid);
                    if mb > r["peak_rss_mb"].as_u64().unwrap_or(0) {
                        r["peak_rss_mb"] = Value::from(mb);
                    }
                }
            });
            if now.saturating_sub(last_sample) >= MEMORY_EVERY_MS {
                last_sample = now;
            }
            let cancelled = rec.as_ref().is_some_and(|r| r["cancel_requested"] == true);
            match child.try_wait() {
                Ok(Some(status)) => {
                    let code = status.code();
                    step += 1;
                    if code == Some(0) && step < steps.len() && !cancelled {
                        match spawn(&mut steps[step], &log, inputs.get(step).cloned().flatten().as_deref()) {
                            Ok(next) => {
                                self.update(id, |r| {
                                    r["step"] = Value::from(step);
                                    r["pid"] = Value::from(next.id());
                                    r["pgid"] = Value::from(next.id());
                                });
                                child = next;
                                continue;
                            }
                            Err(e) => break Err(e),
                        }
                    }
                    break Ok(code);
                }
                Ok(None) => {
                    let started = rec.as_ref().and_then(|r| r["started"].as_u64()).unwrap_or(now);
                    if now.saturating_sub(started) > JOB_LIMIT_MS {
                        signal_group(pgid, "KILL");
                        let _ = child.wait();
                        break Err(format!("stopped after {} minutes", JOB_LIMIT_MS / 60_000));
                    }
                    std::thread::sleep(TICK);
                }
                Err(e) => break Err(format!("lost the job's process: {e}")),
            }
        };
        let log_text = self.whole_log(id);
        let result = text::parse_claim(&log_text);
        self.update(id, |r| {
            let now = now_ms();
            if let Some(since) = r["paused_since"].as_u64() {
                r["paused_ms"] = Value::from(r["paused_ms"].as_u64().unwrap_or(0) + now.saturating_sub(since));
                r["paused_since"] = Value::Null;
            }
            r["finished"] = Value::from(now);
            r["result"] = result.clone();
            // Lines that arrived after the last tick still count.
            advance(r, text::work_phase_of(&log_text), now);
            let cancelled = r["cancel_requested"] == true;
            match &outcome {
                _ if cancelled => {
                    r["state"] = Value::from("cancelled");
                }
                Ok(code) => {
                    r["exit"] = code.map_or(Value::Null, Value::from);
                    let ok = *code == Some(0);
                    r["state"] = Value::from(if ok { "done" } else { "failed" });
                    if ok {
                        r["phase"] = Value::from("done");
                        if let Some(list) = r["phases"].as_array_mut() {
                            list.push(json!({ "phase": "done", "at": now }));
                        }
                    } else {
                        r["error"] = result["error"].clone();
                        if r["error"].is_null() {
                            r["error"] = json!({ "code": "cli_error", "message": crate::cli::cli_failure(&log_text), "local_only": true });
                        }
                    }
                }
                Err(why) => {
                    r["state"] = Value::from("failed");
                    r["error"] = json!({ "code": "job_error", "message": why, "local_only": true });
                }
            }
        });
        if let Ok(mut w) = WATCHED.lock() {
            w.remove(&self.key(id));
        }
    }

    /// Follows again the jobs a previous run of this module started and that still run (called
    /// when the module starts). A job whose process is gone is settled by `state` from its log.
    pub fn adopt(&self) {
        for id in self.ids() {
            let Some(r) = self.record(&id) else { continue };
            if !matches!(r["state"].as_str(), Some("running" | "paused")) {
                continue;
            }
            let watched = WATCHED.lock().is_ok_and(|w| w.contains(&self.key(&id)));
            let pid = r["pid"].as_u64().unwrap_or(0);
            if watched || !alive(pid) {
                continue;
            }
            if let Ok(mut w) = WATCHED.lock() {
                w.insert(self.key(&id));
            }
            let jobs = Jobs { dir: self.dir.clone() };
            std::thread::spawn(move || {
                while alive(pid) {
                    std::thread::sleep(Duration::from_millis(500));
                }
                let log = jobs.whole_log(&id);
                let (state, why) = ended_unwatched(&log);
                jobs.update(&id, |r| {
                    r["finished"] = Value::from(now_ms());
                    r["result"] = text::parse_claim(&log);
                    r["state"] = Value::from(if r["cancel_requested"] == true { "cancelled" } else { state });
                    r["note"] = Value::from(why);
                });
                if let Ok(mut w) = WATCHED.lock() {
                    w.remove(&jobs.key(&id));
                }
            });
        }
    }

    pub fn whole_log(&self, id: &str) -> String {
        let Ok(mut f) = std::fs::File::open(self.path(id, "log")) else { return String::new() };
        let len = f.metadata().map_or(0, |m| m.len());
        let _ = f.seek(SeekFrom::Start(len.saturating_sub(256 * 1024)));
        let mut b = Vec::new();
        let _ = f.read_to_end(&mut b);
        String::from_utf8_lossy(&b).into_owned()
    }

    /// The job as the UI shows it: the record plus `elapsed_s`, `phase_elapsed_s`, the proving
    /// estimate, `log_tail`, and the outcome read from the log.
    pub fn state(&self, id: &str) -> Result<Value, String> {
        if !valid_id(id) {
            return Err(format!("not a job id: {id}"));
        }
        let mut r = self.record(id).ok_or_else(|| format!("there is no job {id}"))?;
        if matches!(r["state"].as_str(), Some("running" | "paused")) {
            let watched = WATCHED.lock().is_ok_and(|w| w.contains(&self.key(id)));
            if !watched && !r["pid"].as_u64().is_some_and(alive) {
                // Read again: a watcher that just finished wrote the outcome before letting go.
                match self.record(id).filter(|again| !matches!(again["state"].as_str(), Some("running" | "paused"))) {
                    Some(ended) => r = ended,
                    None => {
                        let log = self.whole_log(id);
                        let (state, why) = ended_unwatched(&log);
                        let cancelled = r["cancel_requested"] == true;
                        r = self
                            .update(id, |x| {
                                x["state"] = Value::from(if cancelled { "cancelled" } else { state });
                                x["note"] = Value::from(why);
                                x["finished"] = Value::from(now_ms());
                                x["result"] = text::parse_claim(&log);
                            })
                            .unwrap_or(r);
                    }
                }
            } else if !watched {
                r["detached"] = Value::from(true);
            }
        }
        let now = now_ms();
        let end = r["finished"].as_u64().unwrap_or(now);
        let paused_now = r["paused_since"].as_u64().map_or(0, |s| now.saturating_sub(s));
        let paused = r["paused_ms"].as_u64().unwrap_or(0) + paused_now;
        let started = r["started"].as_u64().unwrap_or(now);
        r["elapsed_s"] = Value::from(end.saturating_sub(started).saturating_sub(paused) / 1000);
        let phase_at = r["phases"].as_array().and_then(|p| p.last()).and_then(|p| p["at"].as_u64()).unwrap_or(started);
        r["phase_elapsed_s"] = Value::from(end.saturating_sub(phase_at).saturating_sub(paused_now) / 1000);
        let log = self.whole_log(id);
        let lines: Vec<&str> = log.lines().filter(|l| shown(l)).collect();
        r["log_tail"] = Value::from(lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n"));
        if r["result"].is_null() {
            r["progress"] = text::parse_claim(&log);
        }
        if let Some(why) = r["meta"]["dry_run_forced"].as_str() {
            r["dry_run_forced"] = Value::from(why);
        }
        r["can_pause"] = Value::from(r["state"] == "running" && r["guarded"] != true);
        r["can_resume"] = Value::from(r["state"] == "paused");
        r["can_cancel"] = Value::from(matches!(r["state"].as_str(), Some("running" | "paused")));
        if let Some(o) = r.as_object_mut() {
            o.remove("pid");
        }
        Ok(r)
    }

    pub fn list(&self) -> Vec<Value> {
        let mut all: Vec<Value> = self.ids().iter().filter_map(|id| self.state(id).ok()).collect();
        all.sort_by_key(|j| std::cmp::Reverse(j["started"].as_u64().unwrap_or(0)));
        all
    }

    pub fn pause(&self, id: &str) -> Result<Value, String> {
        let s = self.state(id)?;
        if s["state"] != "running" {
            return Err(format!("job {id} is {}, not running", s["state"].as_str().unwrap_or("?")));
        }
        if s["guarded"] == true {
            return Err("this proof runs under the memory guard, which pauses and resumes it itself when memory runs low; pausing it from here could undo the guard's pause, so it is not offered (CLI-CONTRACT.md §3)".to_owned());
        }
        let pgid = self.record(id).and_then(|r| r["pgid"].as_u64()).unwrap_or(0);
        if !signal_group(pgid, "STOP") {
            return Err("could not pause the job's processes".to_owned());
        }
        self.update(id, |r| {
            r["state"] = Value::from("paused");
            r["paused_since"] = Value::from(now_ms());
        });
        self.state(id)
    }

    pub fn resume(&self, id: &str) -> Result<Value, String> {
        let s = self.state(id)?;
        if s["state"] != "paused" {
            return Err(format!("job {id} is {}, not paused", s["state"].as_str().unwrap_or("?")));
        }
        let pgid = self.record(id).and_then(|r| r["pgid"].as_u64()).unwrap_or(0);
        if !signal_group(pgid, "CONT") {
            return Err("could not resume the job's processes".to_owned());
        }
        self.update(id, |r| {
            let now = now_ms();
            let since = r["paused_since"].as_u64().unwrap_or(now);
            r["paused_ms"] = Value::from(r["paused_ms"].as_u64().unwrap_or(0) + now.saturating_sub(since));
            r["paused_since"] = Value::Null;
            r["state"] = Value::from("running");
        });
        self.state(id)
    }

    /// Stops the job's whole process group. Before `submit` nothing was sent; after it, the
    /// transaction may still land, and the job says so.
    pub fn cancel(&self, id: &str) -> Result<Value, String> {
        let s = self.state(id)?;
        if !matches!(s["state"].as_str(), Some("running" | "paused")) {
            return Err(format!("job {id} already ended ({})", s["state"].as_str().unwrap_or("?")));
        }
        let pgid = self.record(id).and_then(|r| r["pgid"].as_u64()).unwrap_or(0);
        let late = matches!(s["phase"].as_str(), Some("submit" | "confirm"));
        self.update(id, |r| {
            r["cancel_requested"] = Value::from(true);
            if late {
                r["note"] = Value::from("cancelled after the transaction was submitted: it may still be included; check your holdings");
            }
        });
        signal_group(pgid, "CONT");
        signal_group(pgid, "TERM");
        let jobs = Jobs { dir: self.dir.clone() };
        let job = id.to_owned();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(5));
            if !group_members(pgid).is_empty() {
                signal_group(pgid, "KILL");
            }
            // An adopted or unwatched job has no watcher to write the end.
            if !WATCHED.lock().is_ok_and(|w| w.contains(&jobs.key(&job))) {
                jobs.update(&job, |r| {
                    if matches!(r["state"].as_str(), Some("running" | "paused")) {
                        r["state"] = Value::from("cancelled");
                        r["finished"] = Value::from(now_ms());
                    }
                });
            }
        });
        Ok(json!({ "id": id, "cancel_requested": true, "submitted_already": late }))
    }
}

/// Notes every phase from the record's current one up to `phase`, in order, even when several
/// were reached within one tick (`done` is noted only when the job ends).
fn advance(r: &mut Value, phase: &str, now: u64) {
    let current = r["phase"].as_str().unwrap_or("fetch").to_owned();
    let (Some(from), Some(to)) = (PHASES.iter().position(|p| *p == current), PHASES.iter().position(|p| *p == phase)) else { return };
    for p in PHASES.iter().take(to + 1).skip(from + 1) {
        if *p == "done" {
            break;
        }
        r["phase"] = Value::from(*p);
        if let Some(list) = r["phases"].as_array_mut() {
            list.push(json!({ "phase": p, "at": now }));
        }
    }
}

fn spawn(step: &mut Command, log: &std::fs::File, input: Option<&[u8]>) -> Result<Child, String> {
    use std::io::Write;
    let out = log.try_clone().map_err(|e| format!("cannot write the job's log: {e}"))?;
    let err = log.try_clone().map_err(|e| format!("cannot write the job's log: {e}"))?;
    let stdin = if input.is_some() { Stdio::piped() } else { Stdio::null() };
    let mut child = step.stdout(out).stderr(err).stdin(stdin).process_group(0).spawn().map_err(|e| format!("could not start the lez-bridge CLI: {e}"))?;
    if let (Some(bytes), Some(mut pipe)) = (input, child.stdin.take()) {
        // A few dozen bytes: the pipe takes them at once; closing it ends the CLI's read.
        let _ = pipe.write_all(bytes);
    }
    Ok(child)
}

/// How a job that ended while nothing watched it went, from its log.
pub fn ended_unwatched(log: &str) -> (&'static str, &'static str) {
    let landed = log.lines().map(str::trim).any(|l| l.starts_with("applied in block ") || l.starts_with("dry run: simulated in "));
    if landed {
        ("done", "the module restarted while this ran; its log says it finished")
    } else if log.lines().any(|l| l.trim_start().starts_with("Error:")) {
        ("failed", "the module restarted while this ran; its log ends with an error")
    } else {
        ("unknown", "the module restarted while this ran, and its log does not say how it ended; check your holdings before trying again")
    }
}

pub fn jobs_dir(data: &Path) -> PathBuf {
    data.join("jobs")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lez-bridge-jobs-{name}-{}-{}", std::process::id(), now_ms()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// A stand-in CLI: prints the real claim log's lines with pauses between them.
    fn fake(lines: &[&str], gap: f32) -> Command {
        let script: String = lines.iter().map(|l| format!("echo '{l}'; sleep {gap}; ")).collect();
        let mut c = Command::new("/bin/sh");
        c.args(["-c", &script]);
        c
    }

    fn spec(steps: Vec<Command>) -> Spec {
        Spec { kind: "claim".into(), subject: "dep-1".into(), steps, dry_run: true, guarded: false, prove_estimate_s: 671, peak_estimate_mb: 9403, meta: json!({}), inputs: vec![] }
    }

    fn wait_for(jobs: &Jobs, id: &str, states: &[&str], secs: u64) -> Value {
        for _ in 0..secs * 20 {
            let s = jobs.state(id).unwrap();
            if states.contains(&s["state"].as_str().unwrap()) {
                return s;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("job {id} never reached {states:?}: {}", jobs.state(id).unwrap());
    }

    const REAL: [&str; 11] = [
        "root 0xbe1e080d85207b6b7832392daf38ce65347cabe7902bed16e3251ee03a21d3e8",
        "claim nullifier 0x0101",
        "recipient AePC3xbb5yVhcpHfUkwCqCqNtoCsgVQnBCfbz8G4vGkN (one-time address of 2if1EJVcZwFT3oXxF4uewB2mk2XDXpmCWwTtBwUQ6HUC; the opening stays on this machine)",
        "sponsor 20000000 from the bridge float (float 100000000)",
        "pre-check passed: attested root, unspent nullifier, minting open, denomination 0 = 100000000",
        "Transaction data 00ff",
        "privacy-preserving: proving on this computer",
        "proved and submitted in 670.9s",
        "tx f40c24d4909c68b28e4bdb620a8ce7039f78ac28fcc2108cb2f27513a2b2bc61",
        "applied in block 4998",
        "Stored 3 accounts",
    ];

    #[test]
    fn a_job_runs_through_every_phase_in_order() {
        let jobs = Jobs { dir: temp("phases") };
        let s = jobs.start(spec(vec![fake(&REAL, 0.15)])).unwrap();
        let id = s["id"].as_str().unwrap().to_owned();
        assert_eq!(s["state"], "running");
        let done = wait_for(&jobs, &id, &["done", "failed"], 20);
        assert_eq!(done["state"], "done", "{done}");
        let phases: Vec<&str> = done["phases"].as_array().unwrap().iter().map(|p| p["phase"].as_str().unwrap()).collect();
        assert_eq!(phases, ["fetch", "simulate", "prove", "submit", "confirm", "done"]);
        assert_eq!(done["result"]["block"], 4998);
        assert!(!done["log_tail"].as_str().unwrap().contains("Transaction data"));
        assert!(!done["log_tail"].as_str().unwrap().contains("Stored"));
        assert!(!done["log_tail"].as_str().unwrap().contains("claim nullifier"));
        assert!(done.get("pid").is_none());
        let _ = std::fs::remove_dir_all(&jobs.dir);
    }

    #[test]
    fn one_wallet_job_at_a_time() {
        let jobs = Jobs { dir: temp("lock") };
        let s = jobs.start(spec(vec![fake(&["pre-check passed: x"], 2.0)])).unwrap();
        let second = jobs.start(spec(vec![fake(&["x"], 0.1)])).unwrap_err();
        assert!(second.contains("busy") && second.contains(s["id"].as_str().unwrap()), "{second}");
        jobs.cancel(s["id"].as_str().unwrap()).unwrap();
        wait_for(&jobs, s["id"].as_str().unwrap(), &["cancelled"], 10);
        assert!(jobs.start(spec(vec![fake(&["x"], 0.1)])).is_ok());
        let _ = std::fs::remove_dir_all(&jobs.dir);
    }

    #[test]
    fn pause_stops_the_clock_and_resume_continues() {
        let jobs = Jobs { dir: temp("pause") };
        let s = jobs.start(spec(vec![fake(&["pre-check passed: x", "privacy-preserving: proving on this computer", "applied in block 1"], 1.0)])).unwrap();
        let id = s["id"].as_str().unwrap().to_owned();
        std::thread::sleep(Duration::from_millis(400));
        let p = jobs.pause(&id).unwrap();
        assert_eq!(p["state"], "paused");
        assert_eq!(p["can_resume"], true);
        let pgid = jobs.record(&id).unwrap()["pgid"].as_u64().unwrap();
        // Stopped processes stay stopped: the log does not move.
        let before = jobs.whole_log(&id);
        let clock = jobs.state(&id).unwrap()["elapsed_s"].as_u64().unwrap();
        std::thread::sleep(Duration::from_millis(2500));
        assert_eq!(jobs.whole_log(&id), before);
        let paused = jobs.state(&id).unwrap();
        // The clock stands still while paused (to the second it was read in).
        assert!(paused["elapsed_s"].as_u64().unwrap() <= clock + 1, "{paused}");
        assert!(!group_members(pgid).is_empty());
        jobs.resume(&id).unwrap();
        let done = wait_for(&jobs, &id, &["done", "failed"], 20);
        assert_eq!(done["state"], "done");
        assert!(done["paused_ms"].as_u64().unwrap() >= 2000);
        let _ = std::fs::remove_dir_all(&jobs.dir);
    }

    #[test]
    fn cancel_ends_the_whole_group() {
        let jobs = Jobs { dir: temp("cancel") };
        // A child that ignores nothing and a grandchild: both must go.
        let mut c = Command::new("/bin/sh");
        c.args(["-c", "echo 'pre-check passed: x'; /bin/sleep 30 & /bin/sleep 30; wait"]);
        let s = jobs.start(spec(vec![c])).unwrap();
        let id = s["id"].as_str().unwrap().to_owned();
        std::thread::sleep(Duration::from_millis(400));
        let pgid = jobs.record(&id).unwrap()["pgid"].as_u64().unwrap();
        assert!(group_members(pgid).len() >= 2);
        let c = jobs.cancel(&id).unwrap();
        assert_eq!(c["submitted_already"], false);
        let end = wait_for(&jobs, &id, &["cancelled"], 10);
        assert_eq!(end["can_cancel"], false);
        std::thread::sleep(Duration::from_millis(300));
        assert!(group_members(pgid).is_empty(), "processes left in group {pgid}");
        let _ = std::fs::remove_dir_all(&jobs.dir);
    }

    #[test]
    fn a_job_that_ended_while_the_module_was_gone_is_read_from_its_log() {
        let jobs = Jobs { dir: temp("crash") };
        private_dir(&jobs.dir).unwrap();
        // As a previous module process left them: running, its process long gone.
        for (id, log, want) in [
            ("claim-1", REAL.join("\n"), "done"),
            ("claim-2", "root 0x1\nprivacy-preserving: proving on this computer\n".to_owned(), "unknown"),
            ("claim-3", "root 0x1\nError: deposit already claimed: its nullifier is spent; nothing sent\n".to_owned(), "failed"),
        ] {
            jobs.write(id, &json!({ "id": id, "kind": "claim", "state": "running", "pid": 999_999, "pgid": 999_999, "started": now_ms() - 60_000, "phases": [], "paused_ms": 0 }));
            std::fs::write(jobs.path(id, "log"), log).unwrap();
            let s = jobs.state(id).unwrap();
            assert_eq!(s["state"], want, "{id}: {s}");
            assert!(s["note"].as_str().unwrap().contains("restarted"));
        }
        assert!(jobs.busy().is_none());
        let _ = std::fs::remove_dir_all(&jobs.dir);
    }

    #[test]
    fn a_step_reads_its_input_on_stdin_and_the_record_and_log_never_hold_it() {
        let jobs = Jobs { dir: temp("stdin") };
        let mut c = Command::new("/bin/sh");
        c.args(["-c", "read k; [ ${#k} = 66 ] && echo 'dry run: simulated in 1ms; nothing proven or sent'"]);
        let mut sp = spec(vec![c]);
        sp.inputs = vec![Some(format!("0x{}", "ab".repeat(32)).into_bytes())];
        let id = jobs.start(sp).unwrap()["id"].as_str().unwrap().to_owned();
        let end = wait_for(&jobs, &id, &["done", "failed"], 10);
        assert_eq!(end["state"], "done", "{end}");
        for ext in ["json", "log"] {
            assert!(!std::fs::read_to_string(jobs.path(&id, ext)).unwrap().contains(&"ab".repeat(32)));
        }
    }

    #[test]
    fn a_job_still_running_after_a_restart_is_adopted_and_cancellable() {
        let jobs = Jobs { dir: temp("adopt") };
        private_dir(&jobs.dir).unwrap();
        let log = create_private(&jobs.path("claim-9", "log")).unwrap();
        let mut c = Command::new("/bin/sh");
        c.args(["-c", "echo 'pre-check passed: x'; /bin/sleep 30"]);
        let mut child = spawn(&mut c, &log, None).unwrap();
        let pid = child.id() as u64;
        // A process left by a module that died is reparented and reaped by the system; here the
        // test is its parent, so it reaps it.
        std::thread::spawn(move || child.wait());
        jobs.write("claim-9", &json!({ "id": "claim-9", "kind": "claim", "state": "running", "pid": pid, "pgid": pid, "started": now_ms(), "phases": [], "paused_ms": 0 }));
        assert_eq!(jobs.state("claim-9").unwrap()["detached"], true);
        assert!(jobs.busy().is_some());
        jobs.adopt();
        jobs.cancel("claim-9").unwrap();
        let end = wait_for(&jobs, "claim-9", &["cancelled"], 10);
        assert_eq!(end["state"], "cancelled");
        assert!(!alive(pid) || group_members(pid).is_empty());
        let _ = std::fs::remove_dir_all(&jobs.dir);
    }

    #[test]
    fn a_failed_step_stops_the_rest_and_names_the_error() {
        let jobs = Jobs { dir: temp("steps") };
        let mut failing = Command::new("/bin/sh");
        failing.args(["-c", "echo 'Error: burning is frozen for this token; nothing sent'; exit 1"]);
        let s = jobs.start(spec(vec![fake(&["applied in block 7"], 0.05), failing, fake(&["never"], 0.05)])).unwrap();
        let id = s["id"].as_str().unwrap().to_owned();
        let end = wait_for(&jobs, &id, &["done", "failed"], 10);
        assert_eq!(end["state"], "failed");
        assert_eq!(end["error"]["code"], "burning_frozen");
        assert_eq!(end["step"], 1);
        assert!(!jobs.whole_log(&id).contains("never"));
        let _ = std::fs::remove_dir_all(&jobs.dir);
    }
}
