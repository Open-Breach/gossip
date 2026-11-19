//! The `gossip` program: all of it is in the library beside this file.

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    gossip::run().await
}
