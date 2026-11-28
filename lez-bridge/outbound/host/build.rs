// SPDX-License-Identifier: MIT OR Apache-2.0
use std::{collections::HashMap, env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=RISC0_USE_DOCKER");
    println!("cargo:rerun-if-env-changed=RISC0_DOCKER_CONTAINER_TAG");

    let use_docker = env::var("RISC0_USE_DOCKER")
        .map(|v| !v.is_empty() && v != "0")
        .unwrap_or(false);
    if !use_docker {
        risc0_build::embed_methods();
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let root_dir = manifest_dir
        .join("../..")
        .canonicalize()
        .expect("canonicalize lez-bridge docker root_dir");

    let mut opts = HashMap::new();
    for pkg in ["outbound_guest", "outbound_guest_trusted_tip", "outbound_guest_failsafe", "release_guest"] {
        let docker = risc0_build::DockerOptionsBuilder::default()
            .root_dir(root_dir.clone())
            .build()
            .expect("DockerOptions");
        let guest = risc0_build::GuestOptionsBuilder::default()
            .use_docker(docker)
            .build()
            .expect("GuestOptions");
        opts.insert(pkg, guest);
    }
    risc0_build::embed_methods_with_options(opts);
}
