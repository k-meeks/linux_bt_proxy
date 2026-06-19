use chrono::Utc;
use std::{fs, path::PathBuf};

fn main() {
    // Build time for device info
    println!("cargo:rustc-env=BUILD_TIME={}", Utc::now());

    // Proto input dir
    let proto_dir = PathBuf::from("proto");

    // Where to put generated Rust
    let out_proto_dir = PathBuf::from("src/api");

    fs::create_dir_all(&out_proto_dir).expect("Failed to create src/generated");

    // Generate Rust .rs files using rust-protobuf's pure-Rust parser, so building
    // this project doesn't require a system `protoc` binary.
    protobuf_codegen::Codegen::new()
        .pure()
        .out_dir(&out_proto_dir)
        .inputs(&[
            proto_dir.join("api.proto"),
            proto_dir.join("api_options.proto"),
        ])
        .include(&proto_dir)
        .run()
        .expect("protobuf codegen failed");

    // Tell cargo when to rerun
    println!("cargo:rerun-if-changed=proto/api.proto");
    println!("cargo:rerun-if-changed=proto/api_options.proto");
}
