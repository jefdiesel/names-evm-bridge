// SPDX-License-Identifier: MIT OR Apache-2.0
//! The scaffold patch (build.rs, scaffold_patch.rs) on a copy of this module's generated scaffold,
//! and the busy answer the patched dispatch gives.

mod patch {
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/scaffold_patch.rs"));
}

const GENERATED: &str = include_str!("../testdata/scaffold/provider_gen-single.rs.txt");

#[test]
fn the_generated_scaffold_takes_its_lock_with_try_lock_at_all_three_sites() {
    assert_eq!(GENERATED.matches("INSTANCE.0.lock()").count(), 3, "the copy is the generator's own output");
    let patched = patch::patch_scaffold(GENERATED).unwrap();
    assert_eq!(patched.matches("INSTANCE.0.lock()").count(), 0);
    assert_eq!(patched.matches("INSTANCE.0.try_lock()").count(), 3);
    assert!(patched.contains("Err(std::sync::TryLockError::WouldBlock) => return Some(crate::scaffold_busy::answer(method))"));
    // A changed generator fails the build instead of building unpatched.
    assert!(patch::patch_scaffold(&GENERATED.replace("fn ensure_impl<", "fn ensure_it<")).unwrap_err().contains("ensure_impl"));
    let twice = format!("{GENERATED}\n{GENERATED}");
    assert!(patch::patch_scaffold(&twice).is_err());
    assert!(patch::patch_scaffold(&patched).is_err(), "patched twice is refused too");
}

/// The busy answer is the envelope every reader of this module already knows: JSON text with
/// `busy: true` (the mini-app's logic.mjs retries it; a host decoding it twice gets the same).
#[test]
fn the_busy_answer_is_the_usual_envelope() {
    let a = super::scaffold_busy::answer("config");
    let text = a.as_str().unwrap();
    let v: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!((v["success"].as_bool(), v["busy"].as_bool()), (Some(false), Some(true)));
    assert!(v["error"].as_str().unwrap().contains("busy"));
    // Basecamp 0.3.2 serialises it once more: still busy once decoded twice.
    let twice: serde_json::Value = serde_json::from_str(&serde_json::Value::String(text.to_owned()).to_string()).unwrap();
    let again: serde_json::Value = serde_json::from_str(twice.as_str().unwrap()).unwrap();
    assert_eq!(again["busy"], true);
    let s: serde_json::Value = serde_json::from_str(super::scaffold_busy::stopped("x").as_str().unwrap()).unwrap();
    assert!(s.get("busy").is_none() && s["success"] == false);
    assert!(super::SCAFFOLD_LINE.starts_with("lez-bridge-scaffold-try-lock="));
}

/// `config().build` tells one module build from another: the commit and date the build script
/// wrote (in unit tests "unknown"; a module build refuses that), and the version.
#[test]
fn config_reports_the_build_and_lists_build_id() {
    let b = super::build_info();
    assert_eq!(b["version"], super::MODULE_VERSION);
    assert!(b["commit"].is_string() && b["built"].is_string());
    assert!(super::BUILD_COMMIT_LINE.starts_with("lez-bridge-build-commit="));
    assert!(super::FEATURES.contains(&"build_id"));
}
