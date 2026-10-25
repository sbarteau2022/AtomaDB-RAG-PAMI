# AtomaDB-RAG-PAMI — Search Protocol

## Plain-language overview

When you ask a question, the engine:

1. narrows the search to the most relevant documents,
2. scores passages using two independent methods (meaning similarity and keyword matching),
3. merges the scores,
4. returns the best passages plus whole assembled documents.

## Step 1: Document shortlist

If a query vector is provided:

- The engine computes an average ("mean") vector for each document.
- It compares the query to each document's mean and keeps the top 3 documents.
- Only chunks from those 3 documents are searched.
- If this fails, it falls back to searching everything.

If no query vector is provided, all chunks are searched.

## Step 2: Dense search (meaning similarity)

- Each vector is compressed to small integers (int8) for a fast first pass.
- The query is also compressed.
- A fast integer score ranks all candidates.
- The top candidates are rescored with exact similarity (cosine).
- Result: a ranking by meaning.

## Step 3: Keyword search (BM25)

- BM25 is a classic, reliable keyword scoring method.
- It rewards passages that contain the query words, adjusted for passage length and word rarity.
- Parameters used: k1 = 1.5, b = 0.75.

## Step 4: Merging with RRF

- Both rankings (dense and BM25) are merged using Reciprocal Rank Fusion.
- RRF gives each result a score of `1 / (60 + rank)`.
- Items appearing in both rankings get more weight.
- The merged list is trimmed to the requested size (usually top 5).

## Step 5: Whole-document assembly

- Instead of returning fragments, the engine returns whole documents.
- For each hit document, it collects all chunks, orders them by their original position, and joins the text.
- This gives the user a coherent answer context.

## Output

- `hits`: list of passage matches with scores.
- `assembled`: list of full documents with section paths and text.

## Implemented targeting (2026-10-25)

- Silo routing via persisted `segment_terms` + `candidate_segments()` (IDF-weighted, bounded frontier).
- Tiered loading (`load_segments`) decodes only selected segments; mmap-backed I/O.
- In-process tier-1 cache in Ares with 2GB memory guard.
- Relation-aware: document tree mean-vector top-3 and graph expansion (bounded 1-hop) when sidecars exist.

## Plain-language tips

- Good results need good chunking and good embeddings.
- If results are weak, check embedder identity and dimensions.
- The engine returns empty results honestly; it never fabricates answers.

## Certification (2026-10-25)

- § Plain-language overview — CERTIFIED (VERIFIED).
- § Step 1 document shortlist — CERTIFIED (VERIFIED: mean vectors + top-3 + fallback).
- § Step 2 dense search — CERTIFIED (VERIFIED: int8 coarse → f32 cosine).
- § Step 3 BM25 — CERTIFIED (VERIFIED: k1=1.5, b=0.75).
- § Step 4 RRF — CERTIFIED (VERIFIED: 1/(60+rank)).
- § Step 5 assembly — CERTIFIED (VERIFIED: ordinal whole-doc assembly).
- § Output — CERTIFIED (VERIFIED: hits/assembled JSON).
- § Implemented targeting — CERTIFIED (VERIFIED: segment_terms routing, targeted load, in-process cache, graph expansion, mmap).

## Read next

- `05-pami-phase-indexing.md`
- `06-recall-protocol-dia1.md`
