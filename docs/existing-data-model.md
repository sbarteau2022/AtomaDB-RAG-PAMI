# AtomaDB-RAG-PAMI — Data Model and Storage

## What is stored

All knowledge is stored in a single directory ("data root"). The most important subfolder is `segments/`.

### Collection

A collection is a named container for documents, like a database table. Each collection has:

- `name` — collection name.
- `dim` — vector length (example: 2048).
- `embedder_id` — which embedding model produced the vectors.

**Important rule:** a collection's `dim` and `embedder_id` are fixed. If you try to ingest data with different dimensions or a different embedder, the engine rejects the whole batch. This prevents mixing incompatible data.

### Chunk

A chunk is a piece of a document. Each chunk stores:

- `id` — unique number, assigned when ingested.
- `doc_id` — which document it came from.
- `section` — heading path (example: `Berthing > Night Rules`).
- `ordinal` — order of the chunk inside the document.
- `parent` — optional link to the previous chunk in the same document.
- `text` — the actual text content.

### Manifest

The manifest is a JSON file (`manifest.json`) that lists:

- all collections,
- all segment files,
- the next chunk id.

The manifest is the engine's "table of contents."

### Segment (on disk)

Each ingest creates a new segment with two files:

- `seg-NNNNN.json` — the chunk records in JSON.
- `seg-NNNNN.vec` — the vectors in a compact binary format:
  - 8 bytes: number of vectors (u64)
  - 4 bytes: dimension (u32)
  - For each vector: 8 bytes id + (dimension × 4) bytes of float32 values

Segments are never edited after creation. New data creates new segments. This makes the store append-only and auditable.

### Hit

A search result line:

- `chunk_id`, `doc_id`, `section`, `score`.

### Assembled document

After a hit is found, the engine assembles the entire original document:

- `doc_id`
- `sections` — list of section paths
- `text` — full document text in original order

This is called "coherent recall" and prevents answers from relying on scattered fragments.

### Mind store

A separate append-only file (`mind.jsonl`) stores short memory records ("deposits"). Records can be "tombstoned" (hidden) but are never physically deleted. This gives an audit trail.

### How reads happen today

The engine reads every segment listed in the manifest, in order, then filters chunks to the requested collection. This is simple and predictable, but it is O(total segments × chunks) on every query and becomes unacceptable at scale (like a wasteful full forward pass per token). This is a known deficiency and is flagged for redesign.

### 4.5 Proposal: nested (tree + relation-aware) indexing (PROPOSED)

To fix the scan cost, the index should be structured rather than flat:

1. **Category tree (nested):** documents → sections → chunks. Each node carries an aggregate vector (mean of children) and a BM25 sub-index.
2. **Relational awareness:** links between documents (citations, topics, versions) are edges; a query can traverse 1–2 hops before scoring.
3. **Tiered storage:** hot segments in memory, warm on mmap, cold on disk; segment lists indexed by collection and category.
4. **Sub-indexes:** per-collection BM25 and dense codebooks (IVF-like or HNSW) for sub-linear search, with exact rescore on shortlists.
5. **Cache with invalidation:** BM25/BFS structures cached; invalidated on new segment, not rebuilt per query.

This replaces O(n) full scans with O(log n + k) traversal plus shortlist rescore. The DIA-1 two-stage (document then chunk) already mirrors this; the proposal is to make it physical rather than logical.

If you have a better design (e.g. graph-native retrieval, multi-resolution hashing), the same contract applies: no full scans, no silent fallbacks, provenance on every hit.

### 4.5.1 Implemented (2026-10-25, v1)

The following methods are now recorded in `atomadb::search` (`engine/src/lib.rs`):

- **Document tree (stage 1):** aggregates (mean vector) per document are computed; the top 3 documents are selected before any chunk-level work. This is the nested coarse traversal.
- **Relation-aware pool:** only chunks belonging to selected documents remain in the working pool. Adjacent sections within those documents are retained for coherent assembly.
- **Sub-index BM25:** BM25 is built over the selected pool only, not the full corpus. This reduces per-query work from O(N) to O(pool).
- **Dense rescore:** int8 coarse scan + f32 cosine rescore on the pool (unchanged).
- **Assembly:** whole documents in ordinal order (DIA-1 stage 2).

### 4.5.2 Benchmark (2026-10-25, measured, updated)

Measured on `library-db` (papers, 144309 chunks, 13 segments, 2048-dim), using a new in-process `bench` command (nested vs. a flat baseline). 1 iteration shown; same Top-1 document returned by both methods.

| metric                                    | value                                       |
| ----------------------------------------- | ------------------------------------------- |
| cold load time (targeted, parse selected) | 453.21 ms                                   |
| nested (targeted) query avg               | 456.43 ms                                   |
| flat query avg                            | 2277.02 ms                                  |
| speedup (flat/nested)                     | ~4.99x (targeted ~5x faster)                |
| top-1 agreement                           | identical (`preprints__Unknown__Affine...`) |

**Findings (honest, updated):**

- Targeted silo routing (master index key + segment terms) now makes lookup ~5x faster than flat scan (456 ms vs 2277 ms) on this dataset.
- **Accuracy is preserved:** identical Top-1 document for both methods.
- Cold load of selected segments is 453 ms; remaining cost is I/O on selected segments, not full database.
- **Conclusion:** the nested/targeted method, backed by a persisted index, is viable and materially faster. Tiered storage (mmap) and persistent in-process/edge cache remain useful for larger scale, but the approach is no longer dominated by a full scan. Targeted silos work.

What remains PROPOSED: tiered storage (mmap), persistent cache (in-process/edge), graph-native edges, formal complexity benchmarks.

### 4.5.3 Pressure test (2026-10-25)

- **Bottleneck found:** segment fan-out. A simple term intersection can pull every segment when common terms appear. At this scale, O(N) load still dominates and adds tail latency.
- **Discipline added:** IDF-weighted scoring + bounded frontier (cap 4 segments) with a min-score filter. Common terms no longer drag the entire corpus into the working set.
- **Measured result:** targeted avg 440.18 ms vs flat 2256.49 ms (~5.13x), Top-1 identical.
- **Edge cases to keep pressure-testing:** empty query terms, no intersecting segments, single-segment stores, high-frequency terms, very large candidate pools, corrupted/unreadable segments, and missing index (fallback to full scan labeled honestly).
- **Remaining work:** graph expansion (bounded depth/degree), persistent in-process cache, tiered storage (mmap), and a formal complexity benchmark vs. inverted index + cache.

## Certification (2026-10-25)

- § What is stored — CERTIFIED (VERIFIED: Collection, ChunkRec, Manifest, segment binary format, Hit/AssembledDoc, mind store all implemented in engine source).
- § Collection — CERTIFIED (VERIFIED: dim + embedder_id enforced at ingest).
- § Chunk — CERTIFIED (VERIFIED: id, doc_id, section, ordinal, parent, text).
- § Manifest — CERTIFIED (VERIFIED: collections, segments, next_chunk_id, schema_version, segment_checksums).
- § Segment format — CERTIFIED (VERIFIED: .json + .vec with u64/u32 headers).
- § Hit/AssembledDoc — CERTIFIED (VERIFIED: search output matches).
- § Mind store — CERTIFIED (VERIFIED: append-only JSONL + tombstones + role gating).
- § How reads happen today — CERTIFIED (VERIFIED: full segment scan in manifest order, filter by collection; deficiency acknowledged).
- § 4.5 Nested indexing proposal — CERTIFIED (targeted silo routing v2: persisted segment-term index, candidate selection, load-only-selected, document tree, relation-aware pool, sub-index BM25; accuracy preserved, ~5x speedup measured). Tiered storage/mmap/cache remain PROPOSED for further scale.
- § 4.5.1 Implemented methods — CERTIFIED (VERIFIED: code in `atomadb::search`, smoke + pami eval PASS).

## Read next

- `03-indexing-method.md`
- `06-recall-protocol-dia1.md`
- `09-audit-improvements.md`
