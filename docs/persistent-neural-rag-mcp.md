---
title: "Persistent personal and collective RAG through MCP"
document_type: "architecture"
version: "1.0.0"
date: "2026-10-25T18:00:00Z"
implementation_date: null
author: "Codex, recording Stewart Barteau's product requirements"
reviewers: []
status: "draft"
related_files: []
related_issues: []
related_documents: ["github-backup.md", "existing-data-model.md", "existing-search-protocol.md", "cloudflare-design.md"]
tags: ["mcp", "memory", "consent", "collective-intelligence", "local-proof"]
---

# Executive Summary

Give agents persistent memory and document retrieval through an MCP server. Users explicitly opt into contributing session summaries or selected short-term memories. Contributions enter private quarantine, pass cleaning and review, and only then become eligible for a collective release every two to three days.

This is the user-defined product direction and a proposed local acceptance design. The MCP service, approval workflow, cloud aggregation and edge distribution have not been implemented.

# Problem Statement

Agents lose useful session knowledge, while raw pooled memories can expose personal data or amplify incorrect instructions. AtomaDB has local retrieval, append-only memory and role checks, but does not yet have authenticated user isolation, contribution consent, cleaning approvals, collective release manifests or an MCP adapter.

# Solution Overview

Maintain three separate retrieval scopes: personal memory, user-authorized libraries, and approved collective intelligence. Local testing simulates cloud quarantine and edge subscribers before any new personal-memory upload is enabled. Shared retrieval indexes are updated by explicit release manifests. Growing shared retrieval knowledge is distinct from training or modifying neural model weights; model training is outside the first proof.

# Implementation Details

## Data flow

Session summary / selected memory / library import
→ authenticated owner and explicit opt-in record
→ private tenant-scoped quarantine
→ cleaning and quality review
→ contributor approval of exact cleaned content
→ curator approval for collective use
→ versioned collective release
→ edge replicas and authorized MCP retrieval.

The user's intended upload-before-cleaning path means raw submissions may exist in private cloud quarantine. This requires explicit disclosure and consent. A local secret preflight should block obvious credentials before transport; this does not replace cloud cleaning. No quarantine data is placed in a public index or shared edge cache.

## Record model

Each record carries tenant_id, owner_id, record_id, kind (session_summary, short_term_memory, durable_memory, library_chunk), source/session/project, submitted_at, expires_at where applicable, consent_id/version/scope, provenance, raw_hash, cleaned_hash, cleaner_version, review decisions, confidence/evidence, supersedes and release membership. Session identifiers and personal provenance are not exposed in collective public metadata.

Keep private provenance for audit separately from the minimum publishable provenance. A session summary is a contributed claim, not automatically an established fact. Distinguish direct evidence, tested procedure, opinion, conjecture, outdated knowledge and conflicting accounts. Repeated or paraphrased copies must not count as independent corroboration.

## State machine

private_draft → quarantined → cleaned → contributor_approved → curator_approved → published.

Alternative terminal states: rejected, withdrawn, expired. Cleaning creates a new revision and invalidates prior approvals. Both approvals bind the cleaned hash, consent version and audience. A publisher can select only eligible records with current approvals and non-revoked consent. Author agents cannot grant themselves curator privileges.

Opt-in contribution is a service enrollment condition for the collective tier. Installation or MCP connection alone does not authorize arbitrary transcript collection. Provide an explicit consent screen identifying the submitted fields, private upload/cleaning process, review path and collective audience. Only selected summaries and allocations are collected; full transcripts, tool outputs, secrets and third-party data are not silently swept in. Whether a personal-only tier exists is a later product decision.

## MCP surface, proposed

- memory.put / memory.search / memory.forget: authenticated personal scope, with TTL and durability choices.
- library.ingest / library.search: permitted library scope and source provenance.
- contribution.submit / contribution.status / contribution.withdraw: owner-bound contribution lifecycle.
- contribution.review: separate authenticated contributor/curator authority, bound to a cleaned revision.
- collective.search / collective.release_info: vetted released material with citations and version.
- release.publish: operator-only publisher. Ordinary agent users cannot publish or approve by supplying role arguments.

Use a Rust MCP adapter over the existing engine and a transactional local SQLite control store for consent, states, tenant ownership and review receipts. Do not force lifecycle metadata into append-only corpus files. Personal/library/collective scope selection is enforced before candidate retrieval, not by filtering results after a global search. MCP identity comes from verified server auth context; self-reported client names, tenant tool parameters and retrieved instructions are not authorization.

Existing FiveW run summaries can seed personal memory records. Existing Role checks are useful building blocks, but are not sufficient for per-user authorization. Existing memory tombstones hide records; they do not physically erase their bytes or Git history.

## Releases and edge distribution

A release contains a parent revision, eligible cleaned record hashes, embedding model/dimension, source versions, index artifact hashes, revocation epoch and signature. Build and validate locally first. Publish the manifest only after referenced artifacts exist and pass checks. Edge readers verify provenance, atomically select one complete version, and retain the last verified version on rollout failure. A two-to-three-day batch cadence is a product requirement, not an assistant automation configured by this work.

Withdrawals and urgent removals must propagate separately from routine two-to-three-day batches. Use immediate origin denylisting and a short, explicit revocation-freshness lease for replicas. A replica with stale revocation state must fail closed for affected shared content. Already downloaded information cannot be recalled from third parties; do not promise otherwise. Define retention and physical erasure separately from visibility tombstones.

## Backup boundary

GitHub holds engine source, schemas, test fixtures and explicitly vetted distributable library snapshots. Future raw session memories, quarantine records, authentication data and user-identifying review records must be excluded from Git/LFS backup: immutable history conflicts with normal private-data retention and removal. The initial existing library snapshot is a separate operator-approved backup, not user opt-in collective publication. Review library contents and rights before any public/shared redistribution.

# Local Proof and Testing Evidence

No new product behavior is implemented by this design document. Required first local vertical slice:

1. Two synthetic users, one curator and a local MCP client; persist and restart personal memory and library retrieval.
2. User A contributes an opted-in session summary. User B cannot retrieve its raw, cleaned or unapproved form.
3. Cleaner flags seeded secrets, personal identifiers and malicious retrieved instructions. Pattern checks alone cannot certify arbitrary text as safe; uncertain records remain quarantined for review.
4. Contributor approves the cleaned version; curator approves independently. A simulated 72-hour release publishes the eligible item, and B retrieves it with publishable provenance.
5. Change one byte after approval: publication must fail until approvals are renewed. Reject a forged tenant/role, duplicate replay, expired consent, poisoned contribution and cross-user query.
6. Revoke the contribution: origin stops serving it immediately and connected simulated edges stop within the declared lease. A stale edge cannot continue serving it indefinitely.
7. Restart publisher during release and recover without partial versions or duplicate records. Restore backup artifacts and repeat retrieval checks.
8. Measure persistence, cross-tenant isolation, cleaning false negatives/positives, relevance, conflicting evidence, latency and storage growth. Passing synthetic tests alone does not prove real-user privacy, collective intelligence improvement or production readiness.

# Deployment / Rollout

Local-only proof first. Continue using the inspected embedding identity and retrieval baseline. Use synthetic memory data until approval and isolation gates pass. Later map private quarantine/artifacts to private R2, lifecycle metadata to D1, existing Rust retrieval to Containers and verified releases to edge caches. No automatic contributor collection or shared edge publication is currently enabled.

# Next Stage of Implementation

Build the local consent/state machine and MCP adapter as one measurable vertical slice. Implement scope isolation before ingestion, revision-bound approval before publishing, and revocation before distributed caching. Defer autonomous cleaning approval, neural training and broad edge replication until the corresponding gates have evidence.

# What We Learned

The contribution/review control plane is the missing product layer. AtomaDB's current durable segments and local retrieval can support it, but append-only storage alone does not supply consent, review, user isolation or truth verification.

# Grounding

Product requirements come directly from Stewart Barteau's 2026-10-25 clarification. Architecture is a proposal, not a measured outcome. MCP authorization decisions should follow official protocol guidance:

- https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/docs/specification/2026-07-28/basic/authorization/security-considerations.mdx
- https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/docs/specification/2026-07-28/basic/index.mdx

# Changelog

| Version | Date | Author | Change |
|---|---|---|---|
| 1.0.0 | 2026-10-25 | Codex | Capture user-defined persistent/collective memory product and local proof gates |
