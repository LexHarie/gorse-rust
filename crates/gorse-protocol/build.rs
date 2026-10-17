// Ports: protocol/generate.go @ add21b5

//! Generates the protobuf messages and gRPC services from the frozen copies of
//! the upstream `.proto` files in `contracts/proto`. `protox` compiles them in
//! pure Rust, so no system `protoc` is needed.

use std::path::PathBuf;

const PROTOS: [&str; 5] = [
    "cache_store.proto",
    "data_store.proto",
    "encoding.proto",
    "protocol.proto",
    "vector_store.proto",
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let proto_dir = manifest_dir.join("../../contracts/proto");
    for proto in PROTOS {
        println!("cargo:rerun-if-changed={}", proto_dir.join(proto).display());
    }
    let descriptors = protox::compile(PROTOS, [&proto_dir])?;
    tonic_prost_build::configure()
        .build_client(true)
        .build_server(true)
        .compile_fds(descriptors)?;
    Ok(())
}
