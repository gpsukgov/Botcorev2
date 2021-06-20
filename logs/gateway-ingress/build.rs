use anyhow::{Context, Result};

fn main() -> Result<()> {
    // Compile the feature-gate protobuf definitions into the client code
    tonic_build::configure()
        .build_client(true)
        .build_server(false)
        .compile(&["feature-gate.proto"], &["../../lib/ipc/proto"])
        .context("compiling feature-gate.proto definitions")?;
    // Compile the logs/uptime protobuf definitions into the client code
    tonic_build::configure()
        .build_client(true)
        .build_server(false)
        .compile(&["logs/uptime.proto"], &["../../lib/ipc/proto"])
        .context("compiling logs/uptime.proto definitions")?;

    Ok(())
}