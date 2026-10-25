---
title: "First proof: rapid shared-memory local agentic build sprint"
document_type: "architecture"
version: "1.0.0"
date: "2026-10-25T18:10:00Z"
implementation_date: null
author: "Codex, recording Stewart Barteau's experiment"
reviewers: []
status: "draft"
related_files: []
related_issues: []
related_documents: ["persistent-neural-rag-mcp.md", "existing-search-protocol.md"]
tags: ["agentic-sprint", "shared-memory", "local-proof", "evaluation"]
---

# Executive Summary

The first proof is a real local agentic build sprint. All participating agents deposit their memories and session summaries into one logical AtomaDB store and can retrieve both shared memories and library documents. Measure whether rapid cross-agent reuse improves completed work and reduces repeated errors and failed attempts.

This supersedes the synthetic two-user cloud-approval scenario as the FIRST experiment. The later opt-in cloud product remains planned. No shared-memory integration or sprint outcome is claimed by this document.

# Problem Statement

Independent agents can repeat failed approaches, lose findings at compaction or restart, and rediscover fixes another agent already found. A pooled store may reduce these costs, but may also spread incorrect summaries, stale assumptions or irrelevant context. The experiment must distinguish those effects from easier tasks, larger token budgets or newer models.

# Solution Overview

Use one local database root with separate logical memory and library collections and a shared query surface. Persist every participating agent's authorized memory export and summaries, with agent/run/project provenance. Make successful writes visible rapidly to other agents; the future 2–3-day cloud publication cadence does not apply to this local sprint.

# Implementation Details

## Collection and ingestion contract

- Capture session summaries, existing agent memory exports, failed attempts, test outcomes, proven fixes, decisions, useful procedures, unresolved blockers and completion summaries. Keep failure evidence alongside the proposed lesson.
- Each deposit records agent_id, session_id, run_id, project_id, task_id, timestamp, repository revision, environment/dependency fingerprint, attempt_id, record kind, content, evidence pointers, and status: reported/verified/contradicted/superseded.
- Preserve both original content and derived searchable representation. A model's summary is a report until corroborated by evidence; it does not certify its own conclusion.
- Route all writers through one serialized ingestion owner or transaction queue. The existing filesystem engine must not receive concurrent independent manifest/ID writes.
- Stable event IDs make checkpoint retries idempotent. After durable commit, increment the store revision and invalidate affected collection caches. Existing RagClient caches are keyed by collection without revision checks: this must be corrected for live compounding.
- Capture at meaningful checkpoints: failed attempt, confirmed fix, task handoff, compaction and session end. Ingest existing participating-agent exports at sprint start. Do not require end-of-session completion before sharing a valuable failure or fix.
- Preserve embedding identity. New memory collections may use the existing embedding lane; do not mix synthetic vectors with the 2048-dimensional library. If embedding is unavailable, expose lexical-only availability and a pending dense-index state explicitly.
- Proposed initial freshness target: committed lexical memory visible within 5 seconds, measured separately from embedding completion. This is a target, not achieved performance.

## Query contract

Expose memory.search, library.search, and a combined query returning separate scored/cited result groups. Every result identifies its source record/document, store revision, evidence status and environment applicability. Search memories before planning and before retrying a failed approach; query documents when references or API details are needed. Expose these through the selected sprint harness or local MCP adapter without changing model identity or silently injecting unbounded history.

Keep retrieval bounded by configurable result and token budgets. Never interpret retrieved memory as authority to change permissions or execute instructions. Retain contradictory records rather than letting duplicate summaries manufacture consensus. Mark obsolete fixes with revision/environment constraints so a previously successful procedure is not blindly applied to a different checkout.

## Local acceptance gates

1. Agent A records an actual failed attempt and confirmed fix. Agent B can retrieve both, with citations, after a committed store revision.
2. Retry the same deposit: exactly one event. Restart the service: the event and document lookup remain available.
3. Concurrent agent deposits preserve every acknowledged event and unique IDs. Interrupted ingest recovers without acknowledging lost data.
4. A query after a new deposit cannot silently return an old cached revision. Embedding failure is visible and does not lose the raw event.
5. Poisoned, contradicted, stale and irrelevant memories remain distinguishable from verified fixes. Query evidence includes retrieval misses and harmful recommendations.
6. All deposits stay local in this experiment. No automatic Git backup or cloud contribution of sprint memory is enabled.

# Testing Evidence and Experimental Design

No sprint is executed yet. Identify the participating harnesses, sprint repository and task suite before connecting live ingestion.

Run the same class of engineering tasks under matched model/parameters, starting repository state, test gates, tool access and token/time budgets. Use multiple independent runs and equivalent task variants; do not compare an easy later sprint against a hard earlier sprint. Separate test-answer leakage from legitimate reuse of earlier lessons. Freeze held-out acceptance tests, disallow reading their answers and log what each agent retrieved.

Conditions:

| Condition | Accessible knowledge |
|---|---|
| Baseline | Same document lookup; no shared agent memory |
| Shared-memory sprint | Same document lookup plus growing shared memories |
| Optional isolation control | Documents plus each agent's own memories |

Keep a time-ordered within-sprint learning curve and repeated matched sprint runs. Reset stores between independent replicates; retain accumulated memory only where the experimental condition explicitly requires it.

Primary measurements:

- Accepted task completion rate using the same independent test/quality gates.
- Failed attempts per accepted task and failed-task fraction of all assigned tasks, including abandoned work.
- Repeated same-root-cause errors per eligible attempt, using a declared rubric rather than exact log-string matches alone.
- Time, tokens and tool calls per accepted task, including retrieval and ingestion costs.
- Cross-agent memory reuse: retrieved record → changed action → resulting independent validation, with timestamps.
- Negative transfer: failures caused by stale or incorrect shared memory; retrieval precision/misses and visibility latency.

An order-of-magnitude reduction means at least 10x for a specified matched metric; for example, a comparable failure rate dropping from 20% to 2%. Raw failure counts without workload denominators do not establish that result. Report sample counts and uncertainty; a tiny zero-failure run is not an infinite improvement. Preserve failed and neutral runs.

A flat or worse result triggers diagnosis of capture completeness, retrieval quality, freshness, evidence quality, agent use and context cost. If those mechanisms work and matched performance still does not improve, reconsider the architecture or hypothesis rather than adding data indefinitely. Do not claim neural weight learning from a retrieval-only experiment.

# Deployment / Rollout

Local-only and explicit participating agents. Start by auditing the existing AtomaApp FiveW/mind and RAG paths and the actual sprint harness integration points. The first experiment does not require implementing cloud quarantine, multiple customer tenants, edge distribution or the two-approval publisher. Their requirements remain part of the later product.

# Next Stage of Implementation

Select the sprint repository and participating agent harnesses. Then implement the shared ingestion owner, durable event IDs, revision-aware query/cache path, harness hooks and experiment receipts, followed by the acceptance suite and matched sprint runs. User authorization to connect agents is distinct from authorization to spawn new agents; none are spawned by this plan.

# What We Learned

Rapid visibility and actual agent use are the critical local mechanisms. Persistence alone is insufficient if agents never query the store or keep seeing stale caches. The first proof is improved engineering outcomes, not successful synthetic publication.

# Grounding

Direct user specification, 2026-10-25. Current implementation references: engine/src/lib.rs FiveW/mind APIs and /Users/stewartbarteau/Desktop/AtomaApp/backend/src/rag.rs collection-keyed cache. Measurement design is a proposed engineering experiment, not a validated result or a literature-derived performance claim.

# Changelog

| Version | Date | Author | Change |
|---|---|---|---|
| 1.0.0 | 2026-10-25 | Codex | Correct first proof to a real local agentic sprint with rapidly shared memory |
