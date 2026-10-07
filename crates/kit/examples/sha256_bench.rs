//! SHA-256 throughput at the sizes the app hashes: a cookie's HMAC input, a message fragment
//! (`PageParts`' fragment digests), a room page's text parts (`text_part`) and a whole page
//! (`rack_etag` for a page without parts). Per size: the median and range of 7
//! runs, each hashing about 64 MB. First it prints a digest over every length from 0 to 2 KB, which
//! must come out the same from both backends.
//!
//!   cargo run --release -p matchbox_kit --example sha256_bench
//!   cargo run --release -p matchbox_kit --example sha256_bench --features sha2/force-soft
//!
//! The second is the software backend alone: on aarch64 it's what the app ran before sha2's `asm`
//! feature was turned on there (crates/kit/Cargo.toml), so the two runs are the before and after.

use std::hint::black_box;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

const SIZES: [(&str, usize); 4] =
    [("cookie HMAC input", 64), ("message fragment", 1024), ("room page text", 34 * 1024), ("whole room page", 416 * 1024)];
const BYTES_PER_RUN: usize = 64 << 20;
const RUNS: usize = 7;

fn main() {
    assert_eq!(hex::encode(Sha256::digest(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    println!("{}, CPU has SHA-256 instructions: {}", std::env::consts::ARCH, cpu_has_sha256_instructions());
    println!("digest of every length to 2 KB: {}", digest_of_every_length(2048));
    for (name, size) in SIZES {
        let input = page_like_bytes(size);
        let iterations = (BYTES_PER_RUN / size).max(1);
        let mut runs: Vec<Duration> = (0..RUNS).map(|_| time_per_hash(&input, iterations)).collect();
        runs.sort();
        let median = runs[RUNS / 2];
        let gb_per_s = size as f64 / median.as_secs_f64() / 1e9;
        println!("{name} ({size} B): median {median:?} per hash ({gb_per_s:.2} GB/s), range {:?}..{:?}", runs[0], runs[RUNS - 1]);
    }
}

fn time_per_hash(input: &[u8], iterations: usize) -> Duration {
    let started = Instant::now();
    for _ in 0..iterations {
        black_box(Sha256::digest(black_box(input)));
    }
    started.elapsed() / iterations as u32
}

/// One SHA-256 over the digests of each prefix of a page-like input, block boundaries and padding
/// included.
fn digest_of_every_length(max: usize) -> String {
    let input = page_like_bytes(max);
    let mut all = Sha256::new();
    for len in 0..=max {
        all.update(Sha256::digest(&input[..len]));
    }
    hex::encode(all.finalize())
}

/// HTML-ish bytes (SHA-256 doesn't care, but it keeps the input honest).
fn page_like_bytes(size: usize) -> Vec<u8> {
    b"<div class=\"message\" data-message-id=\"42\"><p>Hello there, see you at 10:30</p></div>\n"
        .iter()
        .copied()
        .cycle()
        .take(size)
        .collect()
}

#[cfg(target_arch = "aarch64")]
fn cpu_has_sha256_instructions() -> bool {
    std::arch::is_aarch64_feature_detected!("sha2")
}

#[cfg(target_arch = "x86_64")]
fn cpu_has_sha256_instructions() -> bool {
    std::arch::is_x86_feature_detected!("sha")
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
fn cpu_has_sha256_instructions() -> bool {
    false
}
