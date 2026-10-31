//! The repository's rules that a machine can check (AGENTS.md says which): the contract is one document in two files,
//! every agent is pointed to it, ADRs are well formed, every platform has its page, and every daemon method is
//! documented. Runs with `cargo test` on every OS.

use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("repo root")
}

fn read(file: &str) -> String {
    fs::read_to_string(root().join(file)).unwrap_or_else(|e| panic!("{file}: {e}")).replace("\r\n", "\n")
}

#[test]
fn the_contract_is_one_document_in_two_files() {
    assert_eq!(read("AGENTS.md"), read("CLAUDE.md"), "make the same change in AGENTS.md and CLAUDE.md");
}

#[test]
fn every_agent_is_pointed_to_the_contract() {
    for file in ["GEMINI.md", ".github/copilot-instructions.md", ".cursor/rules/contract.mdc"] {
        assert!(read(file).contains("AGENTS.md"), "{file} must point to AGENTS.md");
    }
}

#[test]
fn adrs_are_numbered_titled_and_complete() {
    let mut numbers = Vec::new();
    for entry in fs::read_dir(root().join("adr")).expect("adr folder") {
        let name = entry.expect("entry").file_name().to_string_lossy().into_owned();
        if name == "TEMPLATE.md" {
            continue;
        }
        let number = &name[..4];
        assert!(number.chars().all(|c| c.is_ascii_digit()), "{name}: NNNN-short-name.md");
        assert!(
            name[5..].trim_end_matches(".md").chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "{name}"
        );
        let text = read(&format!("adr/{name}"));
        assert!(text.starts_with(&format!("# ADR {number} — ")), "{name}: title");
        assert!(text.contains("Decided by "), "{name}: who and when");
        for section in ["## Context", "## Decision", "## Consequences"] {
            assert!(text.contains(section), "{name}: {section}");
        }
        numbers.push(number.parse::<u32>().expect("number"));
    }
    numbers.sort_unstable();
    assert_eq!(numbers, (1..=numbers.len() as u32).collect::<Vec<_>>(), "ADR numbers run 1, 2, 3 with no gaps");
}

#[test]
fn every_platform_has_its_page() {
    for os in ["windows", "macos", "linux"] {
        assert!(read(&format!("platforms/{os}/README.md")).len() > 200, "{os}");
        assert!(read("platforms/README.md").contains(&format!("{os}/README.md")), "{os} linked from the overview");
    }
}

#[test]
fn every_daemon_method_is_documented() {
    let daemon = read("crates/clipd/src/main.rs");
    let docs = read("docs/rpc.md");
    let methods: Vec<&str> = daemon
        .lines()
        .filter_map(|l| l.trim().strip_prefix('"')?.split_once("\" =>").map(|m| m.0))
        .filter(|m| m.contains('.') || *m == "version")
        .collect();
    assert!(methods.len() > 20, "found {methods:?}");
    for m in methods {
        assert!(docs.contains(&format!("`{m}`")), "{m} is missing from docs/rpc.md");
    }
}
