//! The one adapter between this module and the `lez-bridge` CLI (CLI-CONTRACT.md).
//!
//! Everything that touches a wallet or proves runs in the CLI, never in this module. The CLI is
//! run as a separate process, never through a shell: each argument is one argv entry. Its
//! environment is cleared and given only `PATH`, `HOME`, `TMPDIR` and the wallet folder
//! (`LEE_WALLET_HOME_DIR`). `--sequencer` is always passed: the CLI's own default port belongs
//! to another chain on the developer's machine.
//!
//! Reads ask for `--json` when the CLI lists it in `--help`, and otherwise parse today's text
//! with the TEMPORARY parsers in `text.rs`.

use crate::text;
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const READ_PATIENCE: Duration = Duration::from_secs(15);
/// `balances` catches the wallet up with the chain first (10.7 s for 77 blocks, measured).
pub const SYNC_PATIENCE: Duration = Duration::from_secs(120);

#[derive(Clone, Debug)]
pub struct Cli {
    pub program: PathBuf,
    pub wallet_dir: PathBuf,
    pub sequencer: String,
    pub tmp_dir: PathBuf,
}

/// Whether the CLI at a path speaks `--json` (asked once per path and modification time).
static JSON_MODE: Mutex<Option<(PathBuf, u64, bool)>> = Mutex::new(None);

fn mtime(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

impl Cli {
    /// The process for `lez-bridge --sequencer <s> <args…>` with the cleared environment.
    pub fn command(&self, args: &[String]) -> Command {
        let mut c = Command::new(&self.program);
        c.env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("TMPDIR", &self.tmp_dir)
            .env("LEE_WALLET_HOME_DIR", &self.wallet_dir)
            .stdin(Stdio::null());
        if let Some(home) = std::env::var_os("HOME") {
            c.env("HOME", home);
        }
        c.args(self.argv(args));
        c
    }

    /// The argv after the program name: always `--sequencer` first.
    pub fn argv(&self, args: &[String]) -> Vec<String> {
        let mut v = vec!["--sequencer".to_owned(), self.sequencer.clone()];
        v.extend(args.iter().cloned());
        v
    }

    pub fn check(&self) -> Result<(), String> {
        if !is_executable(&self.program) {
            return Err(format!("there is no lez-bridge program at {} (set it in Settings)", self.program.display()));
        }
        if !self.wallet_dir.join("storage.json").exists() {
            return Err(format!("there is no wallet in {} (set the wallet folder in Settings)", self.wallet_dir.display()));
        }
        Ok(())
    }

    pub fn json_mode(&self) -> bool {
        let stamp = mtime(&self.program);
        if let Ok(guard) = JSON_MODE.lock() {
            if let Some((p, m, yes)) = guard.as_ref() {
                if p == &self.program && *m == stamp {
                    return *yes;
                }
            }
        }
        let yes = run(self.command(&["--help".to_owned()]), READ_PATIENCE).is_ok_and(|(_, out)| out.contains("--json"));
        if let Ok(mut guard) = JSON_MODE.lock() {
            *guard = Some((self.program.clone(), stamp, yes));
        }
        yes
    }

    /// Runs a read: `--json` and one document if the CLI speaks it, else the text and `parse`.
    fn read(&self, args: Vec<String>, patience: Duration, parse: fn(&str) -> Result<Value, String>) -> Result<Value, String> {
        self.check()?;
        let json = self.json_mode();
        let mut a = args;
        if json {
            a.insert(0, "--json".to_owned());
        }
        let (code, out) = run(self.command(&a), patience)?;
        if code != Some(0) {
            return Err(cli_failure(&out));
        }
        if json {
            serde_json::from_str(out.lines().rev().find(|l| l.trim_start().starts_with('{')).unwrap_or("{}"))
                .map_err(|e| format!("the CLI's --json answer is not JSON: {e}"))
        } else {
            parse(&out)
        }
    }

    pub fn show(&self, bridge: &str) -> Result<Value, String> {
        self.read(vec!["show".into(), "--bridge".into(), bridge.into()], READ_PATIENCE, text::parse_show)
    }

    pub fn balances(&self, token_program: &str) -> Result<Value, String> {
        self.read(vec!["balances".into(), "--token-program".into(), token_program.into()], SYNC_PATIENCE, text::parse_balances)
    }

    pub fn attest_show(&self, program: &str, instance: &str, anchor: Option<&str>) -> Result<Value, String> {
        let mut a: Vec<String> = vec!["attest".into(), "show".into(), "--program".into(), program.into(), "--instance".into(), instance.into()];
        if let Some(h) = anchor {
            a.extend(["--anchor-block-hash".into(), h.into()]);
        }
        self.read(a, READ_PATIENCE, text::parse_attest_show)
    }

    /// The wallet's accounts and their kind (`wallet account list`, which reads only the wallet).
    pub fn accounts(&self) -> Result<Value, String> {
        self.read(vec!["wallet".into(), "account".into(), "list".into()], READ_PATIENCE, parse_accounts)
    }

    /// Runs a pure computation that takes a secret: through a 0600 file (`--secret-file`) when
    /// the CLI speaks `--json`, else (an older CLI) on argv, where it is visible to `ps` for the
    /// well under a second the computation takes.
    fn with_secret(&self, mut a: Vec<String>, secret: &str) -> Result<(Option<i32>, String), String> {
        if self.json_mode() {
            let _ = crate::util::private_dir(&self.tmp_dir);
            let path = self.tmp_dir.join(format!("k-{}", &crate::util::to_hex(&crate::util::random32()?)[..16]));
            crate::util::write_private(&path, secret.as_bytes()).map_err(|e| e.to_string())?;
            a.extend(["--secret-file".to_owned(), path.display().to_string()]);
            let ran = run(self.command(&a), READ_PATIENCE);
            let _ = std::fs::remove_file(&path);
            return ran;
        }
        a.extend(["--secret".to_owned(), secret.to_owned()]);
        run(self.command(&a), READ_PATIENCE)
    }

    /// The deposit commitment C.
    pub fn commitment(&self, secret: &str, dest: &str, eth_token: &str, denom: &str, cfg_id: &str) -> Result<String, String> {
        let a: Vec<String> = ["commitment", "--dest", dest, "--eth-token", eth_token, "--denom", denom, "--cfg-id", cfg_id].iter().map(|s| s.to_string()).collect();
        let (code, out) = self.with_secret(a, secret)?;
        if code != Some(0) {
            return Err(cli_failure(&out));
        }
        text::parse_commitment(&out)
    }

    /// The exit commitment E and release nullifier.
    pub fn exit_commitment(&self, secret: &str, eth_dest: &str, eth_token: &str, denom: &str, max_relay_fee: &str, cfg_id: &str) -> Result<Value, String> {
        let a: Vec<String> = ["exit-commitment", "--eth-dest", eth_dest, "--eth-token", eth_token, "--denom", denom, "--max-relay-fee", max_relay_fee, "--cfg-id", cfg_id]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (code, out) = self.with_secret(a, secret)?;
        if code != Some(0) {
            return Err(cli_failure(&out));
        }
        text::parse_exit_commitment(&out)
    }

    /// Fetches the storage proof and header of `isDepositRoot[root]` at `anchor` into `dir`,
    /// checked natively, sending nothing (`attest prove-slot --fetch-only --save-dir`). The
    /// command requires `--probe` and `--from`, which a fetch never uses.
    pub fn fetch_material(&self, d: &crate::config::Deployment, rpc: &str, root: &str, anchor: &str, dir: &Path) -> Result<Value, String> {
        self.check()?;
        let a: Vec<String> = [
            "attest", "prove-slot", "--fetch-only", "--save-dir", &dir.display().to_string(), "--eth-rpc", rpc, "--program", &d.attest_program, "--instance",
            &d.attest_instance, "--probe", &d.probe_program, "--from", &d.bridge_program, "--contract", &crate::eth::addr_hex(&d.vault), "--mapping-slot", "83",
            "--key", root, "--anchor-block-hash", anchor,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let (code, out) = run(self.command(&a), Duration::from_secs(60))?;
        if code != Some(0) {
            return Err(cli_failure(&out));
        }
        Ok(serde_json::json!({ "verified": out.contains("statement verified natively"), "dir": dir.display().to_string() }))
    }
}

/// `wallet account list`: `[/path ]Public/<id>` or `Private/<id>` lines, maybe with a label.
pub fn parse_accounts(out: &str) -> Result<Value, String> {
    let mut list = Vec::new();
    for line in out.lines() {
        for word in line.split_whitespace() {
            let (kind, id) = match (word.strip_prefix("Public/"), word.strip_prefix("Private/")) {
                (Some(id), _) => ("public", id),
                (_, Some(id)) => ("private", id),
                _ => continue,
            };
            if crate::util::lez_account(id).is_ok() {
                let path = line.split_whitespace().next().filter(|w| w.starts_with('/')).unwrap_or("");
                list.push(serde_json::json!({ "account": id, "kind": kind, "path": path }));
            }
        }
    }
    Ok(serde_json::json!({ "accounts": list }))
}

/// The CLI's own error line, else its last line.
pub fn cli_failure(out: &str) -> String {
    let line = out
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with("Error:") || l.trim_start().starts_with("error:"))
        .or_else(|| out.lines().rev().find(|l| !l.trim().is_empty()))
        .unwrap_or("the CLI failed without saying why");
    line.trim().trim_start_matches("Error: ").trim_start_matches("error: ").to_owned()
}

pub fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Runs `process`, its stdout and stderr merged in order of arrival as far as two pipes allow,
/// and stops it after `patience`.
pub fn run(process: Command, patience: Duration) -> Result<(Option<i32>, String), String> {
    run_input(process, None, patience)
}

/// [`run`], writing `input` to the process's stdin first (a secret for `--secret-file -`).
pub fn run_input(mut process: Command, input: Option<&[u8]>, patience: Duration) -> Result<(Option<i32>, String), String> {
    use std::io::Write;
    use std::os::unix::process::CommandExt;
    if input.is_some() {
        process.stdin(Stdio::piped());
    }
    let mut child = process
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("could not run the lez-bridge CLI: {e}"))?;
    if let (Some(bytes), Some(mut pipe)) = (input, child.stdin.take()) {
        let _ = pipe.write_all(bytes);
    }
    let drain = |s: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut t = String::new();
            if let Some(mut s) = s {
                let _ = s.read_to_string(&mut t);
            }
            t
        })
    };
    let out = drain(child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let err = drain(child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let started = Instant::now();
    let code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) if started.elapsed() > patience => {
                let _ = Command::new("/bin/kill").args(["-KILL", "--", &format!("-{}", child.id())]).status();
                let _ = child.wait();
                return Err(format!("the lez-bridge CLI did not answer within {} s", patience.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(40)),
            Err(e) => return Err(format!("lost the lez-bridge CLI: {e}")),
        }
    };
    let (o, e) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
    Ok((code, if e.is_empty() { o } else { format!("{o}{e}") }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli() -> Cli {
        Cli { program: "/bin/echo".into(), wallet_dir: "/tmp/w".into(), sequencer: "http://127.0.0.1:3250".into(), tmp_dir: "/tmp".into() }
    }

    #[test]
    fn every_command_names_the_sequencer_first() {
        let argv = cli().argv(&["show".into(), "--bridge".into(), "B".into()]);
        assert_eq!(argv[..2], ["--sequencer".to_owned(), "http://127.0.0.1:3250".to_owned()]);
        assert_eq!(argv[2], "show");
    }

    #[test]
    fn the_environment_is_cleared_but_the_wallet_folder() {
        let c = cli().command(&["x".into()]);
        let envs: Vec<(String, Option<String>)> =
            c.get_envs().map(|(k, v)| (k.to_string_lossy().into_owned(), v.map(|v| v.to_string_lossy().into_owned()))).collect();
        assert!(envs.contains(&("LEE_WALLET_HOME_DIR".into(), Some("/tmp/w".into()))));
        assert!(!envs.iter().any(|(k, _)| k == "LEZ_BRIDGE_SEQUENCER" || k == "RISC0_PROVER"));
        // `env_clear` was called: nothing else is inherited.
        let shown = format!("{c:?}");
        assert!(shown.contains("env_clear") || !shown.contains("SHELL="));
    }

    #[test]
    fn runs_time_out_and_report() {
        let mut slow = Command::new("/bin/sleep");
        slow.arg("5");
        let e = run(slow, Duration::from_millis(300)).unwrap_err();
        assert!(e.contains("did not answer"));
        let (code, out) = run(Command::new("/bin/echo"), READ_PATIENCE).unwrap();
        assert_eq!(code, Some(0));
        assert_eq!(out, "\n");
    }

    #[test]
    fn accounts_list_reads_both_kinds() {
        let v = parse_accounts(include_str!("../testdata/cli/accounts.txt")).unwrap();
        let a = v["accounts"].as_array().unwrap();
        assert!(a.len() >= 30);
        assert!(a.iter().any(|x| x["account"] == "2if1EJVcZwFT3oXxF4uewB2mk2XDXpmCWwTtBwUQ6HUC" && x["kind"] == "private"));
        assert!(a.iter().any(|x| x["account"] == "6NAoZKXE14LJGeeASmpzpwvsuDLUDB2r6v2a2dXa4f4f" && x["kind"] == "public"));
    }

    #[test]
    fn cli_failures_name_the_error_line() {
        assert_eq!(cli_failure("root 0x1\nError: deposit already claimed: its nullifier is spent; nothing sent\n"), "deposit already claimed: its nullifier is spent; nothing sent");
    }
}
