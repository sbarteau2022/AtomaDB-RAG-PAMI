---
title: "Cloudflare AtomaDB scale-out migration"
document_type: "architecture"
version: "1.0.0"
date: "2026-10-25T17:24:30Z"
implementation_date: null
author: "Codex"
reviewers: []
status: "draft"
related_files: []
related_issues: []
related_documents: ["11-network-endpoint-and-cloudflare-d1.md", "02-data-model-and-storage.md", "04-search-protocol.md"]
tags: ["cloudflare", "atomadb", "migration", "retrieval-parity"]
---

# Executive Summary

Move the existing AtomaDB system to Cloudflare while retaining its document format and retrieval code. Keep large files in object storage and searchable metadata in SQL. This is an inspected migration design; no resources have been provisioned and no corpus has been uploaded.

# Problem Statement

The earlier D1-only proposal puts whole segments in SQL rows and performs retrieval inside a Worker. The actual library already occupies approximately 1.4 GiB on disk. Its largest vector segment is 564,799,612 bytes, exceeding D1's 2 MB row limit and a Worker's 128 MB memory limit. Splitting rows alone would not address the retrieval working set.

# Solution Overview

Use a Worker for authenticated API routing, D1 for manifests and publication records, private R2 for immutable JSON/vector segments, and Cloudflare Containers for the existing Rust retrieval library. Container disks are disposable caches; durable state must be recoverable from R2 and D1. This revises the proposed storage placement in document 11, without claiming a measured cloud deployment.

# Implementation Details

## Inspected sources

- Engine: `/Users/stewartbarteau/Desktop/AtomaDB-RAG-PAMI/engine/src/lib.rs`.
- Current application client: `/Users/stewartbarteau/Desktop/AtomaApp/backend/src/rag.rs`.
- Data: `/Users/stewartbarteau/Desktop/AtomaDB-RAG-PAMI/data/library-db`.
- Seven collections, thirteen segments, 2048-dimensional vectors; embedder identity `qwen3-vl-embedding-2b-6bit`.
- Manifest schema 1; next chunk ID 144316. The manifest currently has zero segment checksum entries. Migration must compute independent SHA-256 receipts.
- Current client uses `candidate_segments` followed by `search_loaded`; retain that path for application parity. Query embedding failure currently produces lexical retrieval; preserve explicit `dense` disclosure.

## Component mapping

| Component | Target | Contract |
|---|---|---|
| Immutable JSON/vector files | Private R2 | Exact bytes, content hash, size, original name |
| Collection identities and snapshot manifests | D1 | Fixed dimension/embedder, revisioned publication |
| Search and coherent assembly | Rust container | Existing BM25/dense/RRF/DIA-1 code |
| Authentication and request bounds | Worker | Server-resolved roles; collection isolation |
| Async ingestion | Queues plus Rust ingest workers | Retry-safe publication; bounded backlog |
| Write coordination | One publisher per DB, then sharded coordinators | No duplicate global chunk IDs |
| Mind entries/tombstones | Append-only durable records | Preserve role semantics and event order |

Use Rust for retrieval reuse and TypeScript for Worker bindings. Python is suitable for offline inventory/verification tooling, not a replacement retrieval service.

## Snapshot and migration protocol

1. Capture a stable manifest and its referenced immutable files. Detect source changes during capture and restart rather than publishing a mixed snapshot.
2. Validate JSON/vector ID alignment, vector counts and dimensions, collection identity, finite vector values, and manifest references. Do not upload arbitrary data-root files or embedding credentials.
3. Compute SHA-256 for every file; upload to content-addressed R2 keys. Same name plus different content is a conflict, not a successful retry.
4. Validate uploaded sizes and bytes/hashes. Record a staged revision in D1 only after every required object exists.
5. Atomically publish the revision in D1 using compare-and-swap against the previous head. R2 and D1 do not share a transaction; partial uploads stay unreferenced and are safe to retry.
6. Readers pin one revision, verify hashes during cache hydration, and report not-ready on missing/corrupt objects. Never combine generations or silently return incomplete documents.
7. Keep local canonical ownership until parity and restore gates pass. Roll back by selecting the prior immutable revision; never delete the local corpus during migration.

## Scale boundaries

Replicas increase concurrent query capacity but do not make each query sublinear. Bound cache bytes, in-flight hydration, query queues, and response bytes. For larger corpora, collection/segment shards require globally consistent BM25 statistics and global document candidate selection to preserve ordering. Naively merging independently ranked shard results changes RRF semantics and is not qualified parity.

Do not replace the embedding model with Workers AI or the dense search with Vectorize without a separate measured equivalence gate. Accept query vectors with verified embedding identity initially; moving the exact embedding model off the local machine requires a separately selected runtime.

# Testing Evidence

This pass inspected source, manifest, file headers and Cloudflare authentication only. It did not run retrieval parity, load tests, network deployment, or restore drills. No performance or production readiness claim is made.

Required gates before promotion:

- Deterministic local/cloud comparison using identical snapshots, query vectors, ranking parameters, ties, hit IDs, scores and assembled text.
- Empty/no-match queries, malformed dimensions, non-finite vectors, missing/corrupt files, invalid roles, cross-collection requests, oversized bodies and responses.
- Interrupted uploads, duplicate retries, conflicting segment names, concurrent publication, stale caches, cold restarts and bounded overload.
- Warm/cold latency and throughput at the requested storage size and concurrency; report p50/p95/p99 and cost per workload.
- Restore a fresh instance from durable storage, verify hashes, then repeat parity queries.

Adversarial review findings: D1 row sizing, Worker memory, absent source checksums, disposable container disks, cross-service publication atomicity, and ranking changes from naive sharding all need explicit handling.

## Proven necessary

Separating bulk storage is justified by measured file sizes and current platform limits. Container sizing, replica count and sharding are not yet justified by load evidence.

# Deployment / Rollout

No deployment performed. Wrangler authentication is available. Initial capacity, concurrency and budget remain to be specified. Stage an isolated cloud replica first; switch application routing and canonical ownership only after acceptance gates pass. Backups require independent retained copies and a tested restore, not just R2 availability.

# Next Stage of Implementation

| Step | Dependency |
|---|---|
| Select capacity and cost envelope | User workload and budget |
| Build snapshot validator and hash receipts | Stable source snapshot |
| Build R2/D1 staging publisher | Validator and cloud bindings |
| Add Rust HTTP/container adapter and authenticated Worker | API limits and exact engine contract |
| Test parity, failure recovery and load | Running isolated cloud replica |
| Promote cloud ownership | Passing acceptance evidence |

# What We Learned

| Area | Finding |
|---|---|
| Storage | Existing segments are too large for the old whole-row D1 proposal |
| Compatibility | Reuse the current application retrieval path as the baseline |
| Integrity | Checksums must be generated, not assumed from the manifest |
| Scaling | Replication and retrieval sharding solve different problems |

# Grounding

Not grounded in new academic literature; based on local implementation inspection and official platform documentation checked 2026-10-25:

- https://developers.cloudflare.com/d1/platform/limits/
- https://developers.cloudflare.com/workers/platform/limits/
- https://developers.cloudflare.com/containers/faq/
- https://developers.cloudflare.com/r2/objects/

# Changelog

| Version | Date | Author | Change |
|---|---|---|---|
| 1.0.0 | 2026-10-25 | Codex | Source inspection and draft scale-out migration design |
