//! Runs the protocol tests from the kernel's netproto.rs on the build machine.
//! Run it with `make test-net` from the project root.

#![allow(dead_code)]
extern crate alloc;

#[path = "../../../src/netproto.rs"]
mod netproto;

fn main() {
    println!("netproto is built; run `cargo test` to check it");
}
