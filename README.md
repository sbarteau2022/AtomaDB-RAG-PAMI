# AtomaDB — local proof of concept and GitHub backup

The `codex/local-poc` branch contains source and test fixtures. Full database snapshots belong to `main`; the initial LFS transfer must finish before remote recovery is verified.

The local Rust engine remains the active database. This private repository holds the engine, design, and canonical versioned backup history. Database files use Git LFS; Git commits hold content-addressed snapshot receipts. A backup is complete only after the Git commit and all referenced LFS objects have uploaded and a clean-clone restore succeeds.

## Design and boundaries

Existing [data model](docs/existing-data-model.md) and [search protocol](docs/existing-search-protocol.md) are preserved as reference snapshots; current source governs behavior.

See [backup design](docs/github-backup.md) and [future Cloudflare migration](docs/cloudflare-design.md). The snapshot tool preserves exact manifest, segment JSON/vector bytes, and known mind/graph sidecars. It does not re-embed or change ranking. GitHub is a backup store, not the live query endpoint. Treat historical design claims as proposals unless accompanied by current evidence.

The default-deny `.gitignore` excludes working data, credentials, source papers, caches, and builds. Only explicitly approved directories are tracked. Backup content itself contains document text and memory: keep this repository private. No license to redistribute source papers is implied.

## Requirements

Python 3.11+, Rust/Cargo, Git and Git LFS. Backup tools use only Python's standard library. Embedding creation is outside this backup workflow and retains the existing local model.

## Create a backup

Stop ingestion and all mind/graph writers first. The tool detects observed file changes, but does not lock engine writers; operator-enforced quiescence is required.

```sh
python3 backup_tools/snapshot.py snapshot data/library-db backups
python3 -m unittest discover -s tests -v
cargo test --manifest-path engine/Cargo.toml
cargo run --manifest-path engine/Cargo.toml -- smoke /tmp/atomadb-smoke tests/corpus
cargo run --manifest-path engine/Cargo.toml -- pami-smoke /tmp/atomadb-pami tests/corpus

git add engine backup_tools tests docs README.md backups .gitignore .gitattributes
git diff --cached --stat
git commit -m "Back up verified AtomaDB snapshot"
git push origin main
```

The printed snapshot path identifies the backup. Identical input reuses the same objects and receipt. Snapshots never include files merely because they share the database directory. The initial import includes only `library-db`; other databases require explicit separate snapshots.

## Restore without overwriting a working database

```sh
git lfs pull
python3 backup_tools/snapshot.py restore backups/snapshots/SNAPSHOT_SHA256.json /absolute/path/to/new-db
python3 backup_tools/snapshot.py validate /absolute/path/to/new-db
cargo build --release --manifest-path engine/Cargo.toml
```

Replace `SNAPSHOT_SHA256.json` with an actual receipt filename. Restore rejects existing destinations, unsafe paths, missing LFS objects, mismatched checksums, invalid vector shapes and duplicate IDs. It stages and validates before publishing the restored directory. Point the application to the new directory only after its retrieval checks pass.

## Cost and retention

GitHub Free currently includes 10 GiB LFS storage and monthly download bandwidth, shared account-wide. Every changed binary version consumes additional storage. Unchanged content-addressed objects are reused. Do not run full-clone restores continuously or assume deleting Git files clears billed LFS history. Monitor account usage; no billing settings are changed by this project.

Source: https://docs.github.com/en/billing/concepts/product-billing/git-lfs

## Validation status

See `docs/github-backup.md` for checks, limits and the initial restore receipt. No cloud runtime or automatic scheduling is enabled. Snapshots and pushes are explicit commands.

## Persistent memory and collective intelligence direction

See [persistent neural-RAG MCP design](docs/persistent-neural-rag-mcp.md). The intended product supports personal memory, libraries, explicit opt-in session-summary contributions, private quarantine, cleaning, revision-bound approval, and vetted releases every 2–3 days. First prove rapid memory sharing during a real local agentic build sprint; see [sprint proof design](docs/local-agentic-sprint-proof.md). This product layer and edge distribution are proposed; the current repository provides the local engine and backup workflow. Future private user memories and quarantine records must not enter Git backups.

## First proof: local agentic build sprint

All participating agents share one local memory/library store. Ingest their memories, summaries, failed attempts and verified fixes at checkpoints; expose memory and document queries during work. Make committed records visible rapidly, including cache invalidation. Compare matched sprint outcomes against document lookup without shared memory. Report repeated errors, failed attempts, accepted tasks, and net time/token costs; magnitude-level improvements remain a hypothesis. No live agent hooks or shared-memory sprint are implemented by these design updates.
