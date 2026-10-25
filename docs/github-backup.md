---
title: "GitHub canonical backup for local AtomaDB"
document_type: "implementation"
version: "1.0.0"
date: "2026-10-25T17:48:15Z"
implementation_date: null
author: "Codex"
reviewers: []
status: "draft"
related_files:
  - path: ".gitignore"
    lines: "1-22"
    role: "primary"
  - path: ".gitattributes"
    lines: "1-1"
    role: "primary"
  - path: "README.md"
    lines: "1-55"
    role: "primary"
  - path: "backup_tools/snapshot.py"
    lines: "1-149"
    role: "primary"
  - path: "tests/test_backup.py"
    lines: "1-126"
    role: "test"
  - path: "engine/Cargo.toml"
    lines: "1-14"
    role: "supporting"
  - path: "engine/Cargo.lock"
    lines: "1-201"
    role: "supporting"
  - path: "engine/src/lib.rs"
    lines: "1-2028"
    role: "supporting"
  - path: "engine/src/main.rs"
    lines: "1-8"
    role: "supporting"
related_issues: []
related_documents: ["cloudflare-design.md", "../README.md"]
tags: ["backup", "git-lfs", "local-poc"]
---

# Executive Summary

Keep AtomaDB running locally and store recoverable snapshots in a private GitHub repository. Large database files use Git LFS. A snapshot receipt binds the exact file names, sizes and SHA-256 hashes to one revision.

# Problem Statement

The existing 1.4 GB database has files larger than GitHub's ordinary Git file limit. It also shares its parent directory with embedding configuration, scratch data and source material that should not be swept into a repository.

# Solution Overview

Track the existing Rust engine and allowlisted backup tools. Copy a quiescent database to temporary staging, validate its vectors and records, recheck source hashes, and deduplicate backup objects by SHA-256. Publish the receipt last. A clean-clone restore checks hashes and engine structure before creating a new destination.

# Implementation Details

Python standard library implements portable backup and restore, while Rust remains the retrieval engine. Git LFS stores immutable objects; Git stores receipts and source. No engine semantics are changed. Only library-db is selected for initial backup. Optional mind and graph sidecars are preserved byte-for-byte; their domain semantics are not validated.

## Adversarial review

- No writer lock exists: stop all writers. Before/after hashes detect observed changes but cannot prove a cross-file transaction under concurrent writers.
- Source symlinks and untrusted segment names are rejected. Restore paths use a strict allowlist.
- Reject duplicate/mismatched IDs, nonfinite vectors, wrong dimensions/counts, truncated/trailing bytes, and invalid next IDs.
- Restore never overwrites an existing destination and does not expose a partial restored database after validation failure.
- Local interruptions may leave unreferenced backup objects. No receipt is published before all required objects exist. Retry safely; do not blindly delete referenced objects.
- LFS pointers alone are not a backup. Verify downloaded object bytes and restore from a separate clone.
- Unknown auxiliary files are excluded deliberately; new runtime sidecars require an explicit allowlist change and test.
- This is a trusted-operator tool, not a hostile multiuser ingestion service. SHA-256 checks detect corruption, not a malicious maintainer replacing both content and receipts.

# Testing Evidence

Executed: 11/11 backup edge-case tests; engine smoke 13/13; PAMI smoke 3/3. Cargo test passed but contains zero unit tests; the smoke commands supply engine behavioral checks. The real library snapshot validated thirteen segments and all vector records. Local full-snapshot restore succeeded: 29 files, 1,482,333,198 bytes, seven collections and thirteen segments. Remote LFS upload and clean-clone restore are pending; do not label the remote backup verified yet. Engine smoke and PAMI checks use a synthetic built-in test embedder and do not qualify real-model retrieval quality or the user's future concept.

## Was It Proven Necessary?

Yes: existing vector segments exceed 100 MiB, and safe restore needs more than committing a manifest. Git LFS avoids committing huge binaries into ordinary Git history.

# Deployment / Rollout

Create a private GitHub repository and push only explicit tracked files. No automatic backup, Cloudflare resources, billing changes, or production cutover. Rollback means selecting an earlier receipt and restoring to a new directory; local working data stays intact.

# Next Stage of Implementation

Confirm the concept to be proved, add held-out retrieval tests for that behavior, and measure it locally. Add further databases only when explicitly selected. Retain a separate local copy and monitor shared LFS usage.

# What We Learned

Content-addressed immutable files avoid uploading unchanged segments across snapshots. Quiescence and a remote restore drill are separate requirements from hashing.

# Grounding

Not based on new academic literature. Follows the existing segment design and GitHub's documented LFS and ordinary file limits:

- https://docs.github.com/en/billing/concepts/product-billing/git-lfs
- https://docs.github.com/en/repositories/working-with-files/managing-large-files/about-large-files-on-github

# Changelog

| Version | Date | Author | Change |
|---|---|---|---|
| 1.0.0 | 2026-10-25 | Codex | Private repository and verified snapshot workflow |
