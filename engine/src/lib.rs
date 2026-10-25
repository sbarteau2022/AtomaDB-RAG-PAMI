//! AtomaDB — Stage-2 probe build (Spec 17 §8).
//! Segment store + vector arm (i8 coarse → f32 rescore) + own BM25 arm +
//! RRF fusion + DIA-1 document-graph assembly + role-gated mind store.
//! K-EMBED lives in the ingest lane; the engine stores vectors it is given.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

pub type Res<T> = Result<T, Box<dyn std::error::Error>>;

// ---------------------------------------------------------------- roles
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Role {
    Ingest,
    AgentRt,
    PanelAdmin,
}

impl Role {
    pub fn parse(s: &str) -> Res<Role> {
        match s {
            "ingest" => Ok(Role::Ingest),
            "agent_rt" => Ok(Role::AgentRt),
            "panel_admin" => Ok(Role::PanelAdmin),
            _ => Err(format!("unknown role: {s}").into()),
        }
    }
    pub fn can_write_corpus(&self) -> bool {
        matches!(self, Role::Ingest | Role::PanelAdmin)
    }
    pub fn can_read_mind(&self) -> bool {
        matches!(self, Role::AgentRt | Role::PanelAdmin)
    }
    pub fn can_deposit_mind(&self) -> bool {
        matches!(self, Role::AgentRt | Role::PanelAdmin)
    }
    pub fn can_mutate_mind(&self) -> bool {
        matches!(self, Role::PanelAdmin)
    }
}

// ---------------------------------------------------------------- records
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Collection {
    pub name: String,
    pub dim: usize,
    pub embedder_id: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ChunkRec {
    pub id: u64,
    pub collection: String,
    pub doc_id: String,
    pub section: String,
    pub ordinal: usize,
    pub parent: Option<u64>,
    pub text: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Manifest {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub collections: Vec<Collection>,
    pub segments: Vec<String>,
    pub next_chunk_id: u64,
    #[serde(default)]
    pub segment_checksums: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub segment_terms: std::collections::BTreeMap<String, Vec<String>>,
}

impl Default for Manifest {
    fn default() -> Self {
        Manifest {
            schema_version: 1,
            collections: Vec::new(),
            segments: Vec::new(),
            next_chunk_id: 1,
            segment_checksums: std::collections::BTreeMap::new(),
            segment_terms: std::collections::BTreeMap::new(),
        }
    }
}

pub fn default_schema_version() -> u32 {
    1
}

// ---------------------------------------------------------------- text utils
pub fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

pub fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Deterministic TEST embedder (hashed term-frequency). Stands in for
/// K-EMBED in mechanics smokes; labeled embedder_id "tf-hash-v1" wherever
/// it is used. Real lanes bring their own vectors via ingest-vec.
pub fn tf_embed(text: &str, dim: usize) -> Vec<f32> {
    let mut v = vec![0f32; dim];
    for tok in tokenize(text) {
        let h = fnv1a(&tok);
        v[(h % dim as u64) as usize] += 1.0;
        v[((h >> 16) % dim as u64) as usize] += 0.5;
    }
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        for x in v.iter_mut() {
            *x /= n;
        }
    }
    v
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let (mut d, mut na, mut nb) = (0f32, 0f32, 0f32);
    for i in 0..a.len().min(b.len()) {
        d += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        d / (na.sqrt() * nb.sqrt())
    }
}

// ---------------------------------------------------------------- BM25 (ours, Spec 17 §7.2)
pub struct Bm25 {
    n: usize,
    avgdl: f32,
    df: HashMap<String, usize>,
    doc_len: Vec<usize>,
    tf: Vec<HashMap<String, u32>>,
}

impl Bm25 {
    pub fn build(chunks: &[ChunkRec]) -> Bm25 {
        let mut df: HashMap<String, usize> = HashMap::new();
        let mut tf = Vec::new();
        let mut doc_len = Vec::new();
        let mut total = 0usize;
        for c in chunks {
            let toks = tokenize(&c.text);
            total += toks.len();
            doc_len.push(toks.len());
            let mut m: HashMap<String, u32> = HashMap::new();
            for t in toks {
                *m.entry(t).or_default() += 1;
            }
            for k in m.keys() {
                *df.entry(k.clone()).or_default() += 1;
            }
            tf.push(m);
        }
        let n = chunks.len().max(1);
        Bm25 {
            n,
            avgdl: total as f32 / n as f32,
            df,
            doc_len,
            tf,
        }
    }
    pub fn scores(&self, query: &str) -> Vec<f32> {
        let (k1, b) = (1.5f32, 0.75f32);
        let mut out = vec![0f32; self.tf.len()];
        for term in tokenize(query) {
            let dft = match self.df.get(&term) {
                Some(d) => *d as f32,
                None => continue,
            };
            let idf = ((self.n as f32 - dft + 0.5) / (dft + 0.5) + 1.0).ln();
            for i in 0..self.tf.len() {
                let f = *self.tf[i].get(&term).unwrap_or(&0) as f32;
                if f == 0.0 {
                    continue;
                }
                let dl = self.doc_len[i] as f32;
                out[i] += idf * (f * (k1 + 1.0)) / (f + k1 * (1.0 - b + b * dl / self.avgdl));
            }
        }
        out
    }
}

// ---------------------------------------------------------------- the store
pub struct Db {
    pub root: PathBuf,
    pub manifest: Manifest,
}

impl Db {
    pub fn create(root: &Path) -> Res<Db> {
        fs::create_dir_all(root.join("segments"))?;
        let db = Db {
            root: root.to_path_buf(),
            manifest: Manifest {
                next_chunk_id: 1,
                ..Default::default()
            },
        };
        db.save_manifest()?;
        Ok(db)
    }
    pub fn open(root: &Path) -> Res<Db> {
        let raw = fs::read_to_string(root.join("manifest.json"))?;
        Ok(Db {
            root: root.to_path_buf(),
            manifest: serde_json::from_str(&raw)?,
        })
    }
    pub fn save_manifest(&self) -> Res<()> {
        // Atomic write: temp file + fsync + rename (audit item #9).
        let path = self.root.join("manifest.json");
        let tmp = self.root.join("manifest.json.tmp");
        fs::write(
            &tmp,
            serde_json::to_string_pretty(&self.manifest)?,
        )?;
        use std::io::Write as _;
        if let Ok(f) = fs::OpenOptions::new().write(true).open(&tmp) {
            let _ = f.sync_all();
        }
        fs::rename(&tmp, &path)?;
        Ok(())
    }
    pub fn collection(&self, name: &str) -> Option<&Collection> {
        self.manifest.collections.iter().find(|c| c.name == name)
    }
    pub fn create_collection(&mut self, role: Role, col: Collection) -> Res<()> {
        if !role.can_write_corpus() {
            return Err("role may not write corpus".into());
        }
        if self.collection(&col.name).is_some() {
            return Err(format!("collection {} exists", col.name).into());
        }
        self.manifest.collections.push(col);
        self.save_manifest()
    }

    /// Ingest one batch as a new immutable segment. Identity is enforced
    /// physically (Spec 17 §8.2): embedder id + dimension must match the
    /// collection, or every vector in the batch is rejected.
    pub fn ingest_batch(
        &mut self,
        role: Role,
        collection: &str,
        embedder_id: &str,
        mut chunks: Vec<ChunkRec>,
        vectors: Vec<Vec<f32>>,
    ) -> Res<usize> {
        if !role.can_write_corpus() {
            return Err("role may not write corpus".into());
        }
        let col = self
            .collection(collection)
            .ok_or("no such collection")?
            .clone();
        if col.embedder_id != embedder_id {
            return Err(format!(
                "embedder identity mismatch: collection wants {}, batch has {}",
                col.embedder_id, embedder_id
            )
            .into());
        }
        if chunks.len() != vectors.len() {
            return Err("chunk/vector count mismatch".into());
        }
        for v in &vectors {
            if v.len() != col.dim {
                return Err(format!(
                    "dimension violation: collection dim {}, vector dim {}",
                    col.dim,
                    v.len()
                )
                .into());
            }
        }
        // assign ids + parent chain within the batch per document order
        let mut last_in_doc: HashMap<String, u64> = HashMap::new();
        for c in chunks.iter_mut() {
            c.id = self.manifest.next_chunk_id;
            self.manifest.next_chunk_id += 1;
            c.collection = collection.to_string();
            if c.parent.is_none() {
                c.parent = last_in_doc.get(&c.doc_id).copied();
            }
            last_in_doc.insert(c.doc_id.clone(), c.id);
        }
        let seg_name = format!("seg-{:05}", self.manifest.segments.len() + 1);
        let seg_dir = self.root.join("segments");
        // records
        fs::write(
            seg_dir.join(format!("{seg_name}.json")),
            serde_json::to_string(&chunks)?,
        )?;
        // vectors: u64 count, u32 dim, then per row u64 id + f32[dim]
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&(chunks.len() as u64).to_le_bytes());
        buf.extend_from_slice(&(col.dim as u32).to_le_bytes());
        for (c, v) in chunks.iter().zip(vectors.iter()) {
            buf.extend_from_slice(&c.id.to_le_bytes());
            for x in v {
                buf.extend_from_slice(&x.to_le_bytes());
            }
        }
        fs::write(seg_dir.join(format!("{seg_name}.vec")), buf)?;
        let n = chunks.len();
        self.manifest.segments.push(seg_name.clone());
        // Audit item #10: non-crypto FNV-1a fingerprint of the segment record.
        let json_text = fs::read_to_string(seg_dir.join(format!("{seg_name}.json"))).unwrap_or_default();
        self.manifest.segment_checksums.insert(seg_name.clone(), format!("{:x}", fnv1a(&json_text)));
        // Doc 02 sec 4.5: per-segment top terms to enable targeted (silo) lookup.
        let mut tf: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for c in &chunks {
            for t in tokenize(&c.text) {
                *tf.entry(t).or_default() += 1;
            }
        }
        let mut terms: Vec<(String, usize)> = tf.into_iter().collect();
        terms.sort_by(|a, b| b.1.cmp(&a.1));
        let top_terms: Vec<String> = terms.into_iter().take(64).map(|(t, _)| t).collect();
        self.manifest.segment_terms.insert(seg_name.clone(), top_terms);

        // Graph sidecar (append-only): emit nodes + contains/parent edges.
        use std::io::Write as _;
        let nodes_path = self.root.join("graph_nodes.jsonl");
        let edges_path = self.root.join("graph_edges.jsonl");
        if let Ok(mut nf) = fs::OpenOptions::new().create(true).append(true).open(&nodes_path) {
            if let Ok(mut ef) = fs::OpenOptions::new().create(true).append(true).open(&edges_path) {
                for c in &chunks {
                    let node = serde_json::json!({
                        "id": format!("chunk:{}", c.id),
                        "type": "chunk",
                        "doc_id": c.doc_id,
                        "section": c.section,
                        "ordinal": c.ordinal,
                        "segment": seg_name,
                    });
                    let _ = writeln!(nf, "{}", node);
                    let contains = serde_json::json!({
                        "from": format!("doc:{}", c.doc_id),
                        "to": format!("chunk:{}", c.id),
                        "type": "contains",
                        "source": "structure",
                    });
                    let _ = writeln!(ef, "{}", contains);
                    if let Some(p) = c.parent {
                        let parent_edge = serde_json::json!({
                            "from": format!("chunk:{}", p),
                            "to": format!("chunk:{}", c.id),
                            "type": "parent_of",
                            "source": "structure",
                        });
                        let _ = writeln!(ef, "{}", parent_edge);
                    }
                }
            }
        }

        self.save_manifest()?;
        Ok(n)
    }

    /// Load all (chunk, vector) pairs of a collection, in segment order.
    pub fn load(&self, collection: &str) -> Res<Vec<(ChunkRec, Vec<f32>)>> {
        self.load_segments(collection, None)
    }

    /// Load all chunks plus a map chunk_id -> segment name (for caching).
    pub fn load_with_segment_map(
        &self,
        collection: &str,
    ) -> Res<(Vec<(ChunkRec, Vec<f32>)>, std::collections::BTreeMap<u64, String>)> {
        let col = self.collection(collection).ok_or("no such collection")?.clone();
        let mut out = Vec::new();
        let mut segmap = std::collections::BTreeMap::new();
        for seg in &self.manifest.segments {
            let recs: Vec<ChunkRec> = serde_json::from_str(&fs::read_to_string(
                self.root.join("segments").join(format!("{seg}.json")),
            )?)?;
            let vec_file = fs::File::open(self.root.join("segments").join(format!("{seg}.vec")))?;
            let mmap = unsafe { memmap2::Mmap::map(&vec_file)? };
            let raw = &mmap[..];
            let count = u64::from_le_bytes(raw[0..8].try_into().unwrap()) as usize;
            let dim = u32::from_le_bytes(raw[8..12].try_into().unwrap()) as usize;
            if dim != col.dim {
                return Err(format!("segment {seg} dim {dim} != collection dim {}", col.dim).into());
            }
            let mut off = 12usize;
            let mut vec_by_id: HashMap<u64, Vec<f32>> = HashMap::new();
            for _ in 0..count {
                let id = u64::from_le_bytes(raw[off..off + 8].try_into().unwrap());
                off += 8;
                let mut v = Vec::with_capacity(dim);
                for _ in 0..dim {
                    v.push(f32::from_le_bytes(raw[off..off + 4].try_into().unwrap()));
                    off += 4;
                }
                vec_by_id.insert(id, v);
            }
            for r in recs {
                if r.collection == collection {
                    if let Some(v) = vec_by_id.remove(&r.id) {
                        segmap.insert(r.id, seg.clone());
                        out.push((r, v));
                    }
                }
            }
        }
        Ok((out, segmap))
    }

    /// Targeted load: only decode the named segments (silo routing). `None`
    /// loads every segment (backward compatible / fallback).
    pub fn load_segments(
        &self,
        collection: &str,
        only: Option<&[String]>,
    ) -> Res<Vec<(ChunkRec, Vec<f32>)>> {
        let col = self.collection(collection).ok_or("no such collection")?.clone();
        let mut out = Vec::new();
        let segs: Vec<String> = match only {
            Some(list) => list.to_vec(),
            None => self.manifest.segments.clone(),
        };
        for seg in &segs {
            let recs: Vec<ChunkRec> = serde_json::from_str(&fs::read_to_string(
                self.root.join("segments").join(format!("{seg}.json")),
            )?)?;
            let vec_file = fs::File::open(self.root.join("segments").join(format!("{seg}.vec")))?;
            let mmap = unsafe { memmap2::Mmap::map(&vec_file)? };
            let raw = &mmap[..];
            let count = u64::from_le_bytes(raw[0..8].try_into().unwrap()) as usize;
            let dim = u32::from_le_bytes(raw[8..12].try_into().unwrap()) as usize;
            if dim != col.dim {
                return Err(format!("segment {seg} dim {dim} != collection dim {}", col.dim).into());
            }
            let mut off = 12usize;
            let mut vec_by_id: HashMap<u64, Vec<f32>> = HashMap::new();
            for _ in 0..count {
                let id = u64::from_le_bytes(raw[off..off + 8].try_into().unwrap());
                off += 8;
                let mut v = Vec::with_capacity(dim);
                for _ in 0..dim {
                    v.push(f32::from_le_bytes(raw[off..off + 4].try_into().unwrap()));
                    off += 4;
                }
                vec_by_id.insert(id, v);
            }
            for r in recs {
                if r.collection == collection {
                    if let Some(v) = vec_by_id.remove(&r.id) {
                        out.push((r, v));
                    }
                }
            }
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------- search
#[derive(Serialize, Clone, Debug)]
pub struct Hit {
    pub chunk_id: u64,
    pub doc_id: String,
    pub section: String,
    pub score: f32,
}

#[derive(Serialize)]
pub struct AssembledDoc {
    pub doc_id: String,
    pub sections: Vec<String>,
    pub text: String,
}

/// Dense arm: int8 coarse scan (K-SCAN-I8 shape) → f32 rescore (K-DIST-HALF
/// stands in as exact f32 at probe scale). Returns chunk indices ranked.
pub fn dense_rank(pairs: &[(ChunkRec, Vec<f32>)], q: &[f32], shortlist: usize) -> Vec<usize> {
    // quantize query and store per-vector int8 codes
    let qmax = q.iter().fold(0f32, |a, &b| a.max(b.abs())).max(1e-9);
    let qcodes: Vec<i32> = q.iter().map(|x| (x / qmax * 127.0) as i32).collect();
    let mut coarse: Vec<(usize, i32)> = Vec::with_capacity(pairs.len());
    for (i, (_, v)) in pairs.iter().enumerate() {
        let vmax = v.iter().fold(0f32, |a, &b| a.max(b.abs())).max(1e-9);
        let mut s: i32 = 0;
        for j in 0..v.len().min(q.len()) {
            s += ((v[j] / vmax * 127.0) as i32) * qcodes[j];
        }
        coarse.push((i, s));
    }
    coarse.sort_by(|a, b| b.1.cmp(&a.1));
    let take = shortlist.min(coarse.len());
    let mut rescored: Vec<(usize, f32)> = coarse[..take]
        .iter()
        .map(|(i, _)| (*i, cosine(q, &pairs[*i].1)))
        .collect();
    rescored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    rescored.iter().map(|(i, _)| *i).collect()
}

pub fn rank_of(ranking: &[usize]) -> HashMap<usize, usize> {
    ranking.iter().enumerate().map(|(r, &i)| (i, r + 1)).collect()
}

/// Flat baseline for benchmarking (doc 02.4.5 audit): no document tree,
/// no relation-aware pool, no sub-index. Scans all chunks for BM25 and
/// dense arms. Used only to measure the nested index improvement.
pub fn search_flat(
    db: &Db,
    collection: &str,
    qtext: &str,
    qvec: Option<&[f32]>,
    k: usize,
) -> Res<(Vec<Hit>, Vec<AssembledDoc>)> {
    let pairs = db.load(collection)?;
    if pairs.is_empty() {
        return Ok((vec![], vec![]));
    }
    let chunks: Vec<ChunkRec> = pairs.iter().map(|(c, _)| c.clone()).collect();

    let mut fused: HashMap<usize, f32> = HashMap::new();
    if let Some(q) = qvec {
        let ranking = dense_rank(&pairs, q, 4 * k + 8);
        for (idx, rank) in rank_of(&ranking) {
            *fused.entry(idx).or_default() += 1.0 / (60.0 + rank as f32);
        }
    }
    let bm = Bm25::build(&chunks);
    let all_scores = bm.scores(qtext);
    let mut bm_idx: Vec<usize> = (0..pairs.len()).collect();
    bm_idx.sort_by(|&a, &b| {
        all_scores[b]
            .partial_cmp(&all_scores[a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for (idx, rank) in rank_of(&bm_idx) {
        if all_scores[idx] > 0.0 {
            *fused.entry(idx).or_default() += 1.0 / (60.0 + rank as f32);
        }
    }

    let mut order: Vec<(usize, f32)> = fused.into_iter().collect();
    order.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    order.truncate(k);
    let hits: Vec<Hit> = order
        .iter()
        .map(|(pi, s)| Hit {
            chunk_id: pairs[*pi].0.id,
            doc_id: pairs[*pi].0.doc_id.clone(),
            section: pairs[*pi].0.section.clone(),
            score: *s,
        })
        .collect();

    // assemble whole docs
    let mut seen: Vec<String> = Vec::new();
    let mut assembled = Vec::new();
    for h in &hits {
        if seen.contains(&h.doc_id) {
            continue;
        }
        seen.push(h.doc_id.clone());
        let mut doc_chunks: Vec<&ChunkRec> = chunks.iter().filter(|c| c.doc_id == h.doc_id).collect();
        doc_chunks.sort_by_key(|c| c.ordinal);
        assembled.push(AssembledDoc {
            doc_id: h.doc_id.clone(),
            sections: doc_chunks.iter().map(|c| c.section.clone()).collect(),
            text: doc_chunks.iter().map(|c| c.text.clone()).collect::<Vec<_>>().join("\n"),
        });
    }
    Ok((hits, assembled))
}


/// Targeted silo routing (doc 02 sec 4.5): pick segments whose recorded
/// top-term set intersects the query terms. Returns None to signal
/// "no index / fall back to full scan" (honest).
/// Targeted silo routing (doc 02 sec 4.5, pressure-tested): pick candidate
/// segments using IDF-weighted term overlap, capped to a bounded frontier so a
/// hot query term cannot drag every segment into the working set.
pub fn candidate_segments(manifest: &Manifest, qtext: &str) -> Option<Vec<String>> {
    if manifest.segment_terms.is_empty() || manifest.segments.is_empty() {
        return None;
    }
    // P2: stopword / low-value term suppression
    let stopwords: std::collections::HashSet<&str> = [
        "the","a","an","and","or","of","to","in","on","for","with","by","is","are",
        "was","were","be","been","being","it","its","this","that","these","those",
        "as","at","from","into","than","then","so","if","but","not","no","nor",
    ].iter().copied().collect();
    let qterms: std::collections::HashSet<String> = tokenize(qtext)
        .into_iter()
        .filter(|t| !stopwords.contains(t.as_str()))
        .collect();
    if qterms.is_empty() {
        return None;
    }
    // document frequency of each query term across segments (for IDF)
    let mut df: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for t in &qterms {
        let d = manifest
            .segments
            .iter()
            .filter(|seg| {
                manifest
                    .segment_terms
                    .get(*seg)
                    .map(|v| v.iter().any(|x| x == t))
                    .unwrap_or(false)
            })
            .count();
        df.insert(t.clone(), d.max(1));
    }
    let n = manifest.segments.len() as f32;
    // score each segment by sum of IDF weights of matching terms
    let mut scored: Vec<(String, f32)> = manifest
        .segments
        .iter()
        .map(|seg| {
            let terms: std::collections::HashSet<&str> = manifest
                .segment_terms
                .get(seg)
                .map(|v| v.iter().map(|s| s.as_str()).collect())
                .unwrap_or_default();
            let mut score = 0.0f32;
            for t in &qterms {
                if terms.contains(t.as_str()) {
                    let d = *df.get(t).unwrap_or(&1) as f32;
                    score += (n / d).ln().max(0.0) + 1.0;
                }
            }
            (seg.clone(), score)
        })
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    // bounded frontier: at most 4 segments (or fewer if only a handful exist)
    let cap = 4.min(scored.len());
    let cands: Vec<String> = scored
        .into_iter()
        .take(cap)
        .filter(|(_, s)| *s > 0.0)
        .map(|(n, _)| n)
        .collect();
    if cands.is_empty() {
        None
    } else {
        Some(cands)
    }
}

pub fn search(
    db: &Db,
    collection: &str,
    qtext: &str,
    qvec: Option<&[f32]>,
    k: usize,
) -> Res<(Vec<Hit>, Vec<AssembledDoc>)> {
    let pairs = match candidate_segments(&db.manifest, qtext) {
        Some(segs) => db.load_segments(collection, Some(&segs))?,
        None => db.load(collection)?,
    };
    search_loaded(collection, &pairs, qtext, qvec, k)
}

pub fn search_loaded(
    collection: &str,
    pairs: &[(ChunkRec, Vec<f32>)],
    qtext: &str,
    qvec: Option<&[f32]>,
    k: usize,
) -> Res<(Vec<Hit>, Vec<AssembledDoc>)> {
    if pairs.is_empty() {
        return Ok((vec![], vec![]));
    }
    let chunks: Vec<ChunkRec> = pairs.iter().map(|(c, _)| c.clone()).collect();

    // Nested index stage one (doc 02 sec 4.5): build document-level tree
    // nodes (aggregate/mean vector per doc) for coarse traversal, then keep
    // the top-3 documents as the working pool (relation-aware narrowing).
    let mut doc_pool: Vec<usize> = (0..pairs.len()).collect();
    if let Some(q) = qvec {
        let mut sums: BTreeMap<String, (Vec<f32>, usize)> = BTreeMap::new();
        for (c, v) in pairs {
            let e = sums
                .entry(c.doc_id.clone())
                .or_insert_with(|| (vec![0f32; v.len()], 0));
            for j in 0..v.len() {
                e.0[j] += v[j];
            }
            e.1 += 1;
        }
        let mut doc_rank: Vec<(String, f32)> = sums
            .iter()
            .map(|(d, (s, n))| {
                let mean: Vec<f32> = s.iter().map(|x| x / *n as f32).collect();
                (d.clone(), cosine(q, &mean))
            })
            .collect();
        doc_rank.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let top_docs: Vec<String> = doc_rank.iter().take(3).map(|(d, _)| d.clone()).collect();
        doc_pool.retain(|&i| top_docs.contains(&pairs[i].0.doc_id));
        if doc_pool.is_empty() {
            doc_pool = (0..pairs.len()).collect();
        }
    }
    let pool_pairs: Vec<(ChunkRec, Vec<f32>)> = doc_pool.iter().map(|&i| pairs[i].clone()).collect();

    // arms
    let mut fused: HashMap<usize, f32> = HashMap::new(); // pool idx -> rrf score
    if let Some(q) = qvec {
        let ranking = dense_rank(&pool_pairs, q, 4 * k + 8);
        for (idx, rank) in rank_of(&ranking) {
            *fused.entry(idx).or_default() += 1.0 / (60.0 + rank as f32);
        }
    }
    // Nested index stage: build BM25 only over the selected document pool
    // (relation-aware sub-index), not over the full corpus.
    let pool_chunks: Vec<ChunkRec> = pool_pairs.iter().map(|(c, _)| c.clone()).collect();
    let bm = Bm25::build(&pool_chunks);
    let pool_scores: Vec<f32> = bm.scores(qtext);
    let mut bm_rank_idx: Vec<usize> = (0..pool_pairs.len()).collect();
    bm_rank_idx.sort_by(|&a, &b| {
        pool_scores[b]
            .partial_cmp(&pool_scores[a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for (idx, rank) in rank_of(&bm_rank_idx) {
        if pool_scores[idx] > 0.0 {
            *fused.entry(idx).or_default() += 1.0 / (60.0 + rank as f32);
        }
    }

    let mut order: Vec<(usize, f32)> = fused.into_iter().collect();
    order.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    order.truncate(k);
    let hits: Vec<Hit> = order
        .iter()
        .map(|(pi, s)| Hit {
            chunk_id: pool_pairs[*pi].0.id,
            doc_id: pool_pairs[*pi].0.doc_id.clone(),
            section: pool_pairs[*pi].0.section.clone(),
            score: *s,
        })
        .collect();

    // DIA-1 stage two: assemble each hit document whole, in ordinal order —
    // coherent recall over a split document is a graph traversal, not a
    // bigger top-k.
    let mut seen: Vec<String> = Vec::new();
    let mut assembled = Vec::new();
    for h in &hits {
        if seen.contains(&h.doc_id) {
            continue;
        }
        seen.push(h.doc_id.clone());
        let mut doc_chunks: Vec<&ChunkRec> = chunks.iter().filter(|c| c.doc_id == h.doc_id).collect();
        doc_chunks.sort_by_key(|c| c.ordinal);
        assembled.push(AssembledDoc {
            doc_id: h.doc_id.clone(),
            sections: doc_chunks.iter().map(|c| c.section.clone()).collect(),
            text: doc_chunks.iter().map(|c| c.text.clone()).collect::<Vec<_>>().join("\n"),
        });
    }
    Ok((hits, assembled))
}

// ---------------------------------------------------------------- mind store
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MindLine {
    op: String, // "deposit" | "tombstone"
    id: u64,
    kind: String,
    text: String,
    supersedes: Option<u64>,
    // Canonical run-summary fields (mind-silo scaffold, 2026-10-25).
    // All defaulted: pre-scaffold lines (notes, tombstones) still parse.
    #[serde(default)]
    who: String,
    #[serde(default)]
    what: String,
    #[serde(default, rename = "where")]
    where_: String,
    #[serde(default)]
    when: i64,
    #[serde(default)]
    why: String,
    #[serde(default)]
    run_id: String,
    #[serde(default)]
    lane: String,
    #[serde(default)]
    project: String,
}

/// Canonical 5W run summary, authored by the MODEL as its last step
/// at end of run (or before compaction). `when` 0/absent means
/// "stamp at deposit" — the engine sets it, never the caller guessing.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct FiveW {
    #[serde(default)]
    who: String,
    #[serde(default)]
    what: String,
    #[serde(default, rename = "where")]
    where_: String,
    #[serde(default)]
    when: i64,
    #[serde(default)]
    why: String,
    #[serde(default)]
    run_id: String,
    #[serde(default)]
    lane: String,
    #[serde(default)]
    project: String,
}

impl FiveW {
    /// The four text fields that must be present and non-empty.
    /// `when` is exempt: 0 means "stamp at deposit".
    pub fn missing(&self) -> Vec<&'static str> {
        let mut m = Vec::new();
        if self.who.trim().is_empty() {
            m.push("who");
        }
        if self.what.trim().is_empty() {
            m.push("what");
        }
        if self.where_.trim().is_empty() {
            m.push("where");
        }
        if self.why.trim().is_empty() {
            m.push("why");
        }
        m
    }
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn mind_lines(db: &Db) -> Vec<MindLine> {
    let p = db.root.join("mind.jsonl");
    if !p.exists() {
        return vec![];
    }
    fs::read_to_string(p)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

pub fn mind_append(db: &Db, line: &MindLine) -> Res<()> {
    use std::io::Write;
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(db.root.join("mind.jsonl"))?;
    writeln!(f, "{}", serde_json::to_string(line)?)?;
    Ok(())
}

pub fn mind_deposit(db: &Db, role: Role, kind: &str, text: &str, supersedes: Option<u64>) -> Res<u64> {
    mind_deposit_5w(db, role, kind, text, supersedes, &FiveW::default())
}

/// Full deposit: same role gate and id sequence as mind_deposit,
/// plus the canonical 5W fields. `when` 0 stamps deposit time.
pub fn mind_deposit_5w(
    db: &Db,
    role: Role,
    kind: &str,
    text: &str,
    supersedes: Option<u64>,
    f: &FiveW,
) -> Res<u64> {
    if !role.can_deposit_mind() {
        return Err("role may not deposit to mind".into());
    }
    let id = mind_lines(db).len() as u64 + 1;
    let when = if f.when != 0 { f.when } else { now_unix() };
    mind_append(
        db,
        &MindLine {
            op: "deposit".into(),
            id,
            kind: kind.into(),
            text: text.into(),
            supersedes,
            who: f.who.clone(),
            what: f.what.clone(),
            where_: f.where_.clone(),
            when,
            why: f.why.clone(),
            run_id: f.run_id.clone(),
            lane: f.lane.clone(),
            project: f.project.clone(),
        },
    )?;
    Ok(id)
}

pub fn mind_list(db: &Db, role: Role) -> Res<Vec<MindLine>> {
    if !role.can_read_mind() {
        return Err("role may not read mind".into());
    }
    let lines = mind_lines(db);
    let gone: Vec<u64> = lines
        .iter()
        .filter(|l| l.op == "tombstone")
        .map(|l| l.supersedes.unwrap_or(0))
        .collect();
    let superseded: Vec<u64> = lines
        .iter()
        .filter_map(|l| l.supersedes)
        .collect();
    Ok(lines
        .into_iter()
        .filter(|l| l.op == "deposit" && !gone.contains(&l.id) && !superseded.contains(&l.id))
        .collect())
}

/// The single tail: latest live records of one kind (default
/// "run-summary"), newest-first (id descending). Same read gate as
/// mind_list: ingest may not read. Filters: project matches the
/// record's project OR an unscoped record (project "") — unscoped
/// team records are visible to every frame; lane/run_id match
/// exactly. limit 0 means all.
pub fn mind_tail(
    db: &Db,
    role: Role,
    kind: &str,
    project: Option<&str>,
    lane: Option<&str>,
    run_id: Option<&str>,
    limit: usize,
) -> Res<Vec<MindLine>> {
    let mut rows = mind_list(db, role)?;
    rows.retain(|l| l.kind == kind);
    if let Some(p) = project {
        if !p.is_empty() {
            rows.retain(|l| l.project == p || l.project.is_empty());
        }
    }
    if let Some(l) = lane {
        if !l.is_empty() {
            rows.retain(|r| r.lane == l);
        }
    }
    if let Some(r) = run_id {
        if !r.is_empty() {
            rows.retain(|x| x.run_id == r);
        }
    }
    rows.sort_by_key(|l| std::cmp::Reverse(l.id));
    if limit > 0 && rows.len() > limit {
        rows.truncate(limit);
    }
    Ok(rows)
}

pub fn mind_mutate(db: &Db, role: Role, target: u64) -> Res<()> {
    if !role.can_mutate_mind() {
        return Err("role may not mutate mind (panel_admin only, MEM-1)".into());
    }
    mind_append(
        db,
        &MindLine {
            op: "tombstone".into(),
            id: mind_lines(db).len() as u64 + 1,
            kind: String::new(),
            text: String::new(),
            supersedes: Some(target),
            who: String::new(),
            what: String::new(),
            where_: String::new(),
            when: 0,
            why: String::new(),
            run_id: String::new(),
            lane: String::new(),
            project: String::new(),
        },
    )
}

// ---------------------------------------------------------------- ingest helpers
/// Structure-aware chunking (the CHK-1 dynamic arm in miniature): markdown
/// headings open sections; a section is a chunk; the section path is kept
/// on every chunk so every hit can cite its location (DIA-1).
pub fn chunk_markdown(text: &str) -> Vec<(String, usize, String)> {
    let mut out: Vec<(String, usize, String)> = Vec::new();
    let mut path: Vec<String> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    let mut ord = 0usize;
    let flush = |out: &mut Vec<(String, usize, String)>, path: &[String], cur: &mut Vec<String>, ord: &mut usize| {
        let body = cur.join("\n").trim().to_string();
        // A run containing only heading lines is a document/section node,
        // not a retrievable chunk — it lives in the graph as structure.
        let has_prose = cur
            .iter()
            .any(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'));
        if !body.is_empty() && has_prose {
            let p = if path.is_empty() {
                "(root)".to_string()
            } else {
                path.join(" > ")
            };
            out.push((p, *ord, body));
            *ord += 1;
        }
        cur.clear();
    };
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with('#') {
            let level = t.chars().take_while(|&c| c == '#').count();
            if level >= 1 && t.chars().nth(level) == Some(' ') {
                flush(&mut out, &path, &mut cur, &mut ord);
                let title = t[level..].trim().to_string();
                while path.len() >= level {
                    path.pop();
                }
                while path.len() < level - 1 {
                    path.push(String::new());
                }
                path.push(title);
                cur.push(line.to_string());
                continue;
            }
        }
        cur.push(line.to_string());
    }
    flush(&mut out, &path, &mut cur, &mut ord);
    out
}

#[derive(Deserialize)]
pub struct VecLine {
    doc_id: String,
    section: String,
    ordinal: usize,
    text: String,
    vector: Vec<f32>,
}

// ---------------------------------------------------------------- CLI + smoke
pub fn arg(flags: &[String], name: &str) -> Option<String> {
    flags
        .windows(2)
        .find(|w| w[0] == name)
        .map(|w| w[1].clone())
}

pub fn role_of(flags: &[String]) -> Role {
    Role::parse(&arg(flags, "--role").unwrap_or_else(|| "panel_admin".into()))
        .unwrap_or(Role::PanelAdmin)
}

pub fn run() -> Res<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("atomadb <init|add-collection|ingest-md|ingest-vec|query|mind-deposit|mind-list|mind-tail|mind-mutate|smoke> ...");
        std::process::exit(2);
    }
    let cmd = args[1].as_str();
    let flags = &args[2..];
    match cmd {
        "init" => {
            Db::create(Path::new(&flags[0]))?;
            println!("initialized {}", flags[0]);
        }
        "add-collection" => {
            let mut db = Db::open(Path::new(&flags[0]))?;
            db.create_collection(
                role_of(flags),
                Collection {
                    name: flags[1].clone(),
                    dim: flags[2].parse()?,
                    embedder_id: flags[3].clone(),
                },
            )?;
            println!("collection {} added", flags[1]);
        }
        "ingest-md" => {
            let mut db = Db::open(Path::new(&flags[0]))?;
            let col = db.collection(&flags[1]).ok_or("no such collection")?.clone();
            if col.embedder_id != "tf-hash-v1" {
                return Err("ingest-md serves the tf-hash-v1 test embedder; use ingest-vec for real lanes".into());
            }
            let mut chunks = Vec::new();
            let mut vectors = Vec::new();
            let mut entries: Vec<PathBuf> = fs::read_dir(&flags[2])?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().map(|x| x == "md").unwrap_or(false))
                .collect();
            entries.sort();
            for p in entries {
                let doc_id = p.file_stem().unwrap().to_string_lossy().to_string();
                let text = fs::read_to_string(&p)?;
                for (section, ord, body) in chunk_markdown(&text) {
                    vectors.push(tf_embed(&body, col.dim));
                    chunks.push(ChunkRec {
                        id: 0,
                        collection: String::new(),
                        doc_id: doc_id.clone(),
                        section,
                        ordinal: ord,
                        parent: None,
                        text: body,
                    });
                }
            }
            let n = db.ingest_batch(role_of(flags), &col.name, "tf-hash-v1", chunks, vectors)?;
            println!("ingested {n} chunks into {}", col.name);
        }
        "ingest-vec" => {
            let mut db = Db::open(Path::new(&flags[0]))?;
            let col = db.collection(&flags[1]).ok_or("no such collection")?.clone();
            let raw = fs::read_to_string(&flags[2])?;
            let mut chunks = Vec::new();
            let mut vectors = Vec::new();
            for line in raw.lines().filter(|l| !l.trim().is_empty()) {
                let vl: VecLine = serde_json::from_str(line)?;
                vectors.push(vl.vector);
                chunks.push(ChunkRec {
                    id: 0,
                    collection: String::new(),
                    doc_id: vl.doc_id,
                    section: vl.section,
                    ordinal: vl.ordinal,
                    parent: None,
                    text: vl.text,
                });
            }
            let n = db.ingest_batch(role_of(flags), &col.name, &col.embedder_id.clone(), chunks, vectors)?;
            println!("ingested {n} vectors into {}", col.name);
        }
        "query" => {
            let db = Db::open(Path::new(&flags[0]))?;
            let col = db.collection(&flags[1]).ok_or("no such collection")?.clone();
            let qvec = if col.embedder_id == "tf-hash-v1" {
                Some(tf_embed(&flags[2], col.dim))
            } else {
                None
            };
            let (hits, assembled) = search(&db, &col.name, &flags[2], qvec.as_deref(), 5)?;
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                "hits": hits, "assembled": assembled
            }))?);
        }
        "query-vec" => {
            // query with an externally-produced vector (the real K-EMBED lane)
            let db = Db::open(Path::new(&flags[0]))?;
            let col = db.collection(&flags[1]).ok_or("no such collection")?.clone();
            let qvec: Vec<f32> = serde_json::from_str(&fs::read_to_string(&flags[2])?)?;
            if qvec.len() != col.dim {
                return Err(format!("query vector dim {} != collection dim {}", qvec.len(), col.dim).into());
            }
            let (hits, assembled) = search(&db, &col.name, &flags[3], Some(&qvec), 5)?;
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                "hits": hits, "assembled": assembled
            }))?);
        }
        "mind-deposit" => {
            let db = Db::open(Path::new(&flags[0]))?;
            let raw = &flags[1];
            // JSON 5W record path: only when the argument is a JSON
            // object carrying a 5W key. Anything else — including
            // JSON-looking text that is not a 5W record — keeps the
            // legacy note path exactly, so old callers do not break.
            let is_5w = serde_json::from_str::<serde_json::Value>(raw)
                .map(|v| {
                    v.as_object()
                        .map(|o| {
                            o.contains_key("who")
                                || o.contains_key("what")
                                || o.contains_key("why")
                        })
                        .unwrap_or(false)
                })
                .unwrap_or(false);
            if is_5w {
                let f: FiveW = serde_json::from_str(raw)?;
                let missing = f.missing();
                if !missing.is_empty() {
                    return Err(format!("5W record missing: {}", missing.join(", ")).into());
                }
                let kind = arg(&flags, "--kind").unwrap_or_else(|| "run-summary".into());
                let id = mind_deposit_5w(&db, role_of(flags), &kind, &f.what, None, &f)?;
                println!("deposited mind record {id}");
            } else {
                let kind = arg(&flags, "--kind").unwrap_or_else(|| "note".into());
                let id = mind_deposit(&db, role_of(flags), &kind, raw, None)?;
                println!("deposited mind record {id}");
            }
        }
        "mind-list" => {
            let db = Db::open(Path::new(&flags[0]))?;
            let rows = mind_list(&db, role_of(flags))?;
            println!("{}", serde_json::to_string_pretty(&rows)?);
        }
        "mind-tail" => {
            // The single tail: latest live records of one kind
            // (default run-summary), newest-first. Same read gate as
            // mind-list: ingest may not read. --limit 0 means all.
            let db = Db::open(Path::new(&flags[0]))?;
            let limit = arg(&flags, "--limit")
                .and_then(|v| v.parse().ok())
                .unwrap_or(20);
            let kind = arg(&flags, "--kind").unwrap_or_else(|| "run-summary".into());
            let rows = mind_tail(
                &db,
                role_of(flags),
                &kind,
                arg(&flags, "--project").as_deref(),
                arg(&flags, "--lane").as_deref(),
                arg(&flags, "--run-id").as_deref(),
                limit,
            )?;
            println!("{}", serde_json::to_string_pretty(&rows)?);
        }
        "mind-mutate" => {
            let db = Db::open(Path::new(&flags[0]))?;
            mind_mutate(&db, role_of(flags), flags[1].parse()?)?;
            println!("mutated");
        }
        "smoke" => {
            std::process::exit(smoke(Path::new(&flags[0]), Path::new(&flags[1]))?);
        }
        "key-smoke" => {
            std::process::exit(key_smoke(Path::new(&flags[0]), Path::new(&flags[1]), Path::new(&flags[2]))?);
        }
        "pami-smoke" => {
            std::process::exit(pami_smoke(Path::new(&flags[0]), Path::new(&flags[1]))?);
        }
        "status" => {
            let db = Db::open(Path::new(&flags[0]))?;
            let mc = db.manifest.collections.len();
            let ms = db.manifest.segments.len();
            println!("schema_version: {}", db.manifest.schema_version);
            println!("collections: {}", mc);
            for c in &db.manifest.collections {
                println!("  - {} dim={} embedder={}", c.name, c.dim, c.embedder_id);
            }
            println!("segments: {}", ms);
            println!("next_chunk_id: {}", db.manifest.next_chunk_id);
            let mind = db.root.join("mind.jsonl");
            if mind.exists() {
                let n = fs::read_to_string(&mind).map(|x| x.lines().count()).unwrap_or(0);
                println!("mind_entries: {}", n);
            } else {
                println!("mind_entries: 0");
            }
        }
        "bench" => {
            use std::time::Instant;
            let db = Db::open(Path::new(&flags[0]))?;
            let coll = flags[1].clone();
            let iters: usize = flags.get(2).and_then(|x| x.parse().ok()).unwrap_or(20);
            let mut queries: Vec<String> = flags[3..].to_vec();
            if queries.is_empty() {
                queries = vec![
                    "transformer attention".into(),
                    "tide window lantern code".into(),
                    "quench bath timing".into(),
                ];
            }
            let mut results = Vec::new();
            for q in &queries {
                let c = db.collection(&coll).ok_or("no such collection")?.clone();
                let qvec = if c.embedder_id == "tf-hash-v1" {
                    Some(tf_embed(q, c.dim))
                } else {
                    None
                };
                // warmup
                let _ = search(&db, &coll, q, qvec.as_deref(), 5);
                let _ = search_flat(&db, &coll, q, qvec.as_deref(), 5);

                // load cost (once): how long to read+parse all segments
                let tload = Instant::now();
                let _ = db.load(&coll)?;
                let load_ms = tload.elapsed().as_secs_f64() * 1000.0;

                // top-1 agreement check (one run each)
                let (nh, _) = search(&db, &coll, q, qvec.as_deref(), 5)?;
                let (fh, _) = search_flat(&db, &coll, q, qvec.as_deref(), 5)?;
                let nested_top1 = nh.first().map(|h| h.doc_id.clone());
                let flat_top1 = fh.first().map(|h| h.doc_id.clone());

                let mut nt = Vec::with_capacity(iters);
                let mut ft = Vec::with_capacity(iters);
                for _ in 0..iters {
                    let t = Instant::now();
                    let _ = search(&db, &coll, q, qvec.as_deref(), 5)?;
                    nt.push(t.elapsed().as_secs_f64() * 1000.0);
                    let t = Instant::now();
                    let _ = search_flat(&db, &coll, q, qvec.as_deref(), 5)?;
                    ft.push(t.elapsed().as_secs_f64() * 1000.0);
                }
                let avg = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
                let mn = |v: &[f64]| *v.iter().min_by(|a, b| a.partial_cmp(b).unwrap()).unwrap();
                let mx = |v: &[f64]| *v.iter().max_by(|a, b| a.partial_cmp(b).unwrap()).unwrap();
                results.push(serde_json::json!({
                    "query": q,
                    "iters": iters,
                    "load_ms_once": load_ms,
                    "nested_ms": {"avg": avg(&nt), "min": mn(&nt), "max": mx(&nt)},
                    "flat_ms": {"avg": avg(&ft), "min": mn(&ft), "max": mx(&ft)},
                    "speedup_flat_over_nested": avg(&ft) / avg(&nt).max(1e-9),
                    "nested_top1": nested_top1,
                    "flat_top1": flat_top1,
                    "same_top1": nested_top1 == flat_top1,
                }));
            }
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                "bench": "nested-vs-flat",
                "collection": coll,
                "results": results,
            }))?);
        }
        "rebuild-index" => {
            let mut db = Db::open(Path::new(&flags[0]))?;
            for seg in db.manifest.segments.clone() {
                let recs: Vec<ChunkRec> = serde_json::from_str(&fs::read_to_string(
                    db.root.join("segments").join(format!("{seg}.json")),
                )?)?;
                let mut tf: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
                for r in &recs {
                    for t in tokenize(&r.text) {
                        *tf.entry(t).or_default() += 1;
                    }
                }
                let mut terms: Vec<(String, usize)> = tf.into_iter().collect();
                terms.sort_by(|a, b| b.1.cmp(&a.1));
                let top: Vec<String> = terms.into_iter().take(64).map(|(t, _)| t).collect();
                db.manifest.segment_terms.insert(seg, top);
            }
            db.save_manifest()?;
            println!("rebuilt index: {} segments", db.manifest.segments.len());
        }
        "rebuild-graph" => {
            // Rebuild graph sidecars from all segments (append fresh; for old DBs).
            let db = Db::open(Path::new(&flags[0]))?;
            let nodes_path = db.root.join("graph_nodes.jsonl");
            let edges_path = db.root.join("graph_edges.jsonl");
            let _ = fs::remove_file(&nodes_path);
            let _ = fs::remove_file(&edges_path);
            for seg in db.manifest.segments.clone() {
                let recs: Vec<ChunkRec> = serde_json::from_str(&fs::read_to_string(
                    db.root.join("segments").join(format!("{seg}.json")),
                )?)?;
                use std::io::Write as _;
                if let (Ok(mut nf), Ok(mut ef)) = (
                    fs::OpenOptions::new().create(true).append(true).open(&nodes_path),
                    fs::OpenOptions::new().create(true).append(true).open(&edges_path),
                ) {
                    for c in &recs {
                        let node = serde_json::json!({
                            "id": format!("chunk:{}", c.id),
                            "type": "chunk",
                            "doc_id": c.doc_id,
                            "section": c.section,
                            "ordinal": c.ordinal,
                            "segment": seg,
                        });
                        let _ = writeln!(nf, "{}", node);
                        let contains = serde_json::json!({
                            "from": format!("doc:{}", c.doc_id),
                            "to": format!("chunk:{}", c.id),
                            "type": "contains",
                            "source": "rebuild",
                        });
                        let _ = writeln!(ef, "{}", contains);
                        if let Some(p) = c.parent {
                            let pe = serde_json::json!({
                                "from": format!("chunk:{}", p),
                                "to": format!("chunk:{}", c.id),
                                "type": "parent_of",
                                "source": "rebuild",
                            });
                            let _ = writeln!(ef, "{}", pe);
                        }
                    }
                }
            }

            // Relational coherence (doc graph): add bounded document-level
            // edges based on centroid similarity (top-5 nearest docs per doc).
            // This is append-only and intended as a coherence aid.
            use std::collections::BTreeMap;
            // Build doc centroids from all chunks across segments.
            let mut doc_acc: BTreeMap<String, (Vec<f32>, usize)> = BTreeMap::new();
            for seg in &db.manifest.segments {
                let recs: Vec<ChunkRec> = serde_json::from_str(&fs::read_to_string(
                    db.root.join("segments").join(format!("{seg}.json")),
                )?)?;
                let raw = fs::read(db.root.join("segments").join(format!("{seg}.vec")))?;
                let count = u64::from_le_bytes(raw[0..8].try_into().unwrap()) as usize;
                let dim = u32::from_le_bytes(raw[8..12].try_into().unwrap()) as usize;
                let mut off = 12usize;
                let mut vec_by_id: HashMap<u64, Vec<f32>> = HashMap::new();
                for _ in 0..count {
                    let id = u64::from_le_bytes(raw[off..off + 8].try_into().unwrap());
                    off += 8;
                    let mut v = Vec::with_capacity(dim);
                    for _ in 0..dim {
                        v.push(f32::from_le_bytes(raw[off..off + 4].try_into().unwrap()));
                        off += 4;
                    }
                    vec_by_id.insert(id, v);
                }
                for r in recs {
                    if let Some(v) = vec_by_id.get(&r.id) {
                        let e = doc_acc.entry(r.doc_id.clone()).or_insert_with(|| (vec![0f32; v.len()], 0));
                        for j in 0..v.len() {
                            e.0[j] += v[j];
                        }
                        e.1 += 1;
                    }
                }
            }
            let mut doc_centroids: BTreeMap<String, Vec<f32>> = BTreeMap::new();
            for (d, (s, n)) in &doc_acc {
                if *n == 0 {
                    continue;
                }
                let mean: Vec<f32> = s.iter().map(|x| x / *n as f32).collect();
                let norm = mean.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
                doc_centroids.insert(d.clone(), mean.iter().map(|x| x / norm).collect());
            }
            let doc_ids: Vec<String> = doc_centroids.keys().cloned().collect();
            // Append related edges (top-5 nearest docs per doc, degree capped).
            if let Ok(mut ef) = fs::OpenOptions::new().create(true).append(true).open(&edges_path) {
                use std::io::Write as _;
                for d in &doc_ids {
                    let mut scored: Vec<(String, f32)> = doc_ids
                        .iter()
                        .filter(|o| *o != d)
                        .map(|o| {
                            let c = cosine(&doc_centroids[d], &doc_centroids[o]);
                            (o.clone(), c)
                        })
                        .collect();
                    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                    let mut added = 0usize;
                    for (o, c) in scored {
                        if added >= 3 || c < 0.0 {
                            break;
                        }
                        // Only add once (sorted doc id order) to keep graph tidy.
                        if d < &o {
                            let edge = serde_json::json!({
                                "from": format!("doc:{}", d),
                                "to": format!("doc:{}", o),
                                "type": "related",
                                "weight": c,
                                "source": "centroid-similarity",
                            });
                            let _ = writeln!(ef, "{}", edge);
                            added += 1;
                        }
                    }
                }
            }
            println!("rebuilt graph sidecars in {}", db.root.display());
        }
        _ => {
            eprintln!("unknown command {cmd}");
            std::process::exit(2);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- smoke
pub fn smoke(work: &Path, corpus: &Path) -> Res<i32> {
    fs::create_dir_all(work)?;
    let dbroot = work.join("smoke-db");
    if dbroot.exists() {
        fs::remove_dir_all(&dbroot)?;
    }
    let mut checks: Vec<(String, bool, String)> = Vec::new();
    let mut check = |name: &str, pass: bool, detail: String| {
        checks.push((name.to_string(), pass, detail));
    };

    let mut db = Db::create(&dbroot)?;
    db.create_collection(
        Role::PanelAdmin,
        Collection {
            name: "probe".into(),
            dim: 256,
            embedder_id: "tf-hash-v1".into(),
        },
    )?;
    // ingest the probe corpus through the structure-aware chunker
    let mut chunks = Vec::new();
    let mut vectors = Vec::new();
    let mut entries: Vec<PathBuf> = fs::read_dir(corpus)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|x| x == "md").unwrap_or(false))
        .collect();
    entries.sort();
    for p in &entries {
        let doc_id = p.file_stem().unwrap().to_string_lossy().to_string();
        for (section, ord, body) in chunk_markdown(&fs::read_to_string(p)?) {
            vectors.push(tf_embed(&body, 256));
            chunks.push(ChunkRec {
                id: 0,
                collection: String::new(),
                doc_id: doc_id.clone(),
                section,
                ordinal: ord,
                parent: None,
                text: body,
            });
        }
    }
    let n = db.ingest_batch(Role::Ingest, "probe", "tf-hash-v1", chunks, vectors)?;
    check("ingest probe corpus", n == 12, format!("{n} chunks (want 12)"));

    // Q1 — the split-document test: the answer joins Berthing (§1) and
    // Night Rules (§3) of harbor.md; orchard/ledger contain "lantern" too.
    let q1 = tf_embed("tide window lantern code", 256);
    let (hits, assembled) = search(&db, "probe", "tide window lantern code", Some(&q1), 5)?;
    check(
        "split-doc query tops harbor",
        hits.first().map(|h| h.doc_id == "harbor").unwrap_or(false),
        format!("top: {:?}", hits.first().map(|h| h.doc_id.clone())),
    );
    let harbor_asm = assembled.iter().find(|a| a.doc_id == "harbor");
    check(
        "coherent recall: harbor assembled across the split",
        harbor_asm
            .map(|a| {
                a.sections.iter().any(|s| s.contains("Berthing"))
                    && a.sections.iter().any(|s| s.contains("Night Rules"))
                    && a.text.contains("tide window")
                    && a.text.contains("lantern code")
            })
            .unwrap_or(false),
        format!("sections: {:?}", harbor_asm.map(|a| a.sections.clone())),
    );

    // Q2 — a single-section answer still lands.
    let q2 = tf_embed("quench bath timing", 256);
    let (hits2, _) = search(&db, "probe", "quench bath timing", Some(&q2), 5)?;
    check(
        "quench query tops quench",
        hits2.first().map(|h| h.doc_id == "quench").unwrap_or(false),
        format!("top: {:?}", hits2.first().map(|h| h.doc_id.clone())),
    );

    // roles (MEM-1 shape)
    check(
        "agent_rt may deposit to mind",
        mind_deposit(&db, Role::AgentRt, "note", "smoke deposit", None).is_ok(),
        String::new(),
    );
    check(
        "ingest may not read mind",
        mind_list(&db, Role::Ingest).is_err(),
        String::new(),
    );
    check(
        "agent_rt may not mutate mind",
        mind_mutate(&db, Role::AgentRt, 1).is_err(),
        String::new(),
    );
    check(
        "panel_admin may mutate mind",
        mind_mutate(&db, Role::PanelAdmin, 1).is_ok(),
        String::new(),
    );

    // Canonical 5W run-summary + the single tail (mind scaffold 2026-10-25).
    let five = FiveW {
        who: "smoke-lane".into(),
        what: "smoke run".into(),
        where_: "smoke-project".into(),
        when: 0,
        why: "smoke".into(),
        run_id: "smoke-run".into(),
        lane: "smoke-lane".into(),
        project: "smoke-project".into(),
    };
    let tail_ok = mind_deposit_5w(&db, Role::AgentRt, "run-summary", "smoke run", None, &five)
        .and_then(|id| {
            let rows = mind_tail(&db, Role::AgentRt, "run-summary", Some("smoke-project"), None, None, 20)?;
            Ok(rows.iter().any(|r| r.id == id && r.who == "smoke-lane" && r.when > 0))
        })
        .unwrap_or(false);
    check("5W run-summary deposit + tail", tail_ok, String::new());
    check(
        "ingest may not read tail",
        mind_tail(&db, Role::Ingest, "run-summary", None, None, None, 20).is_err(),
        String::new(),
    );

    // physical identity enforcement (§8.2)
    let one_chunk = || ChunkRec {
        id: 0,
        collection: String::new(),
        doc_id: "bad".into(),
        section: "x".into(),
        ordinal: 0,
        parent: None,
        text: "bad".into(),
    };
    check(
        "wrong-dimension vector rejected",
        db.ingest_batch(Role::Ingest, "probe", "tf-hash-v1", vec![one_chunk()], vec![vec![0.0; 128]])
            .is_err(),
        String::new(),
    );
    check(
        "foreign embedder rejected",
        db.ingest_batch(Role::Ingest, "probe", "someone-else", vec![one_chunk()], vec![vec![0.0; 256]])
            .is_err(),
        String::new(),
    );

    // persistence + a rough latency read (informational, not a gate)
    drop(db);
    let db2 = Db::open(&dbroot)?;
    let t0 = std::time::Instant::now();
    let mut last_top = String::new();
    for _ in 0..200 {
        let (h, _) = search(&db2, "probe", "tide window lantern code", Some(&q1), 5)?;
        last_top = h.first().map(|x| x.doc_id.clone()).unwrap_or_default();
    }
    let avg_ms = t0.elapsed().as_secs_f64() * 1000.0 / 200.0;
    check(
        "reopen: split-doc top hit still harbor",
        last_top == "harbor",
        format!("top after reopen: {last_top}"),
    );

    let passed = checks.iter().filter(|c| c.1).count();
    for (name, pass, detail) in &checks {
        println!("{} {} {}", if *pass { "PASS" } else { "FAIL" }, name, detail);
    }
    println!("avg query latency over 200 runs: {avg_ms:.3} ms (probe scale, informational)");
    println!("SMOKE: {passed}/{} PASS", checks.len());
    let receipt = serde_json::json!({
        "engine": "AtomaDB stage-2 probe build",
        "embedder": "tf-hash-v1 (TEST embedder — mechanics smoke, not a recall measurement)",
        "checks": checks.iter().map(|(n, p, d)| serde_json::json!({"name": n, "pass": p, "detail": d})).collect::<Vec<_>>(),
        "passed": passed,
        "total": checks.len(),
        "avg_query_ms": avg_ms,
    });
    fs::write(work.join("smoke-receipt.json"), serde_json::to_string_pretty(&receipt)?)?;
    Ok(if passed == checks.len() { 0 } else { 1 })
}

// ---------------------------------------------------------------- Harness Key (§8.12)
#[derive(Deserialize)]
pub struct KeyFile {
    version: String,
    canonical_dim: usize,
    anchors: Vec<String>,
}

#[derive(Deserialize)]
pub struct AnchorVec {
    text: String,
    vector: Vec<f32>,
}

#[derive(Deserialize)]
pub struct KeyQuery {
    text: String,
    expect: String,
    vector: Vec<f32>,
}

/// Relative representation: an item's coordinates in canonical space are
/// its cosine profile against the anchor set, L2-normalized. Any model
/// that can embed the anchors can speak this space.
pub fn key_coords(v: &[f32], anchors: &[Vec<f32>]) -> Vec<f32> {
    let mut c: Vec<f32> = anchors.iter().map(|a| cosine(v, a)).collect();
    let n = c.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        for x in c.iter_mut() {
            *x /= n;
        }
    }
    c
}

pub fn rank_docs(items: &[(String, Vec<f32>)], q: &[f32]) -> Vec<String> {
    let mut scored: Vec<(usize, f32)> = items
        .iter()
        .enumerate()
        .map(|(i, (_, v))| (i, cosine(q, v)))
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.iter().map(|(i, _)| items[*i].0.clone()).collect()
}

pub fn key_smoke(work: &Path, corpus: &Path, data: &Path) -> Res<i32> {
    fs::create_dir_all(work)?;
    let key: KeyFile = serde_json::from_str(&fs::read_to_string(data.join("key.json"))?)?;
    let anchors_qwen: Vec<AnchorVec> = fs::read_to_string(data.join("anchors_qwen.jsonl"))?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let queries_qwen: BTreeMap<String, KeyQuery> =
        serde_json::from_str(&fs::read_to_string(data.join("key_queries_qwen.json"))?)?;
    let corpus_qwen: Vec<VecLine> = fs::read_to_string(data.join("vectors.jsonl"))?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();

    // tf-hash side: a genuinely foreign embedding space (the B model)
    let mut tf_items: Vec<(String, Vec<f32>)> = Vec::new();
    let mut entries: Vec<PathBuf> = fs::read_dir(corpus)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|x| x == "md").unwrap_or(false))
        .collect();
    entries.sort();
    for p in &entries {
        let doc_id = p.file_stem().unwrap().to_string_lossy().to_string();
        for (_, _, body) in chunk_markdown(&fs::read_to_string(p)?) {
            tf_items.push((doc_id.clone(), tf_embed(&body, 256)));
        }
    }
    let anchors_tf: Vec<Vec<f32>> = key.anchors.iter().map(|a| tf_embed(a, 256)).collect();
    let anchors_q: Vec<Vec<f32>> = anchors_qwen.iter().map(|a| a.vector.clone()).collect();

    // canonical-space corpora (doc_id, coords)
    let canon_qwen: Vec<(String, Vec<f32>)> = corpus_qwen
        .iter()
        .map(|vl| (vl.doc_id.clone(), key_coords(&vl.vector, &anchors_q)))
        .collect();
    let canon_tf: Vec<(String, Vec<f32>)> = tf_items
        .iter()
        .map(|(d, v)| (d.clone(), key_coords(v, &anchors_tf)))
        .collect();

    let mut rows = Vec::new();
    let (mut aa, mut bb, mut ab, mut ba) = (0usize, 0usize, 0usize, 0usize);
    let nq = queries_qwen.len();
    for (name, q) in &queries_qwen {
        let cq_qwen = key_coords(&q.vector, &anchors_q);
        let cq_tf = key_coords(&tf_embed(&q.text, 256), &anchors_tf);
        let r_aa = rank_docs(&canon_qwen, &cq_qwen);
        let r_bb = rank_docs(&canon_tf, &cq_tf);
        let r_ab = rank_docs(&canon_qwen, &cq_tf); // Qwen index, tf query
        let r_ba = rank_docs(&canon_tf, &cq_qwen); // tf index, Qwen query
        let hit = |r: &[String]| r.first().map(|d| d == &q.expect).unwrap_or(false);
        aa += hit(&r_aa) as usize;
        bb += hit(&r_bb) as usize;
        ab += hit(&r_ab) as usize;
        ba += hit(&r_ba) as usize;
        // ranking agreement (doc-level overlap@3) same-model vs cross-model
        let top3 = |r: &[String]| {
            let mut s: Vec<String> = Vec::new();
            for d in r {
                if !s.contains(d) {
                    s.push(d.clone());
                }
                if s.len() == 3 {
                    break;
                }
            }
            s
        };
        let (t_aa, t_ab) = (top3(&r_aa), top3(&r_ab));
        let overlap = t_aa.iter().filter(|d| t_ab.contains(d)).count();
        rows.push(serde_json::json!({
            "query": name, "text": q.text, "expect": q.expect,
            "A_qwen_to_A": r_aa.first(), "B_tf_to_B": r_bb.first(),
            "A_index_B_query": r_ab.first(), "B_index_A_query": r_ba.first(),
            "overlap_at3_A_vs_AB": overlap,
        }));
        println!(
            "{name} expect={} | A->A {} | B->B {} | Aidx/Bq {} | Bidx/Aq {} | overlap@3 {}/3",
            q.expect,
            r_aa.first().cloned().unwrap_or_default(),
            r_bb.first().cloned().unwrap_or_default(),
            r_ab.first().cloned().unwrap_or_default(),
            r_ba.first().cloned().unwrap_or_default(),
            overlap
        );
    }
    let acc = |x: usize| x as f32 / nq as f32;
    println!(
        "KEY ACCURACY same-model A {aa}/{nq}, B {bb}/{nq} | cross-model A-idx/B-q {ab}/{nq}, B-idx/A-q {ba}/{nq}"
    );
    let receipt = serde_json::json!({
        "experiment": "Harness Key cross-model retrieval (Spec 17 §8.12)",
        "key_version": key.version,
        "canonical_dim": key.canonical_dim,
        "method": "relative representation (cosine profile vs anchors)",
        "models": {"A": "Qwen3-VL-Embedding-2B-6bit (2048-dim)", "B": "tf-hash-v1 test embedder (256-dim, foreign space)"},
        "accuracy": {"same_A": acc(aa), "same_B": acc(bb), "cross_Aidx_Bq": acc(ab), "cross_Bidx_Aq": acc(ba)},
        "queries": rows,
        "label": "probe scale (12 chunks). The cross-model tax is the gap between same-model and cross-model accuracy."
    });
    fs::write(work.join("key-receipt.json"), serde_json::to_string_pretty(&receipt)?)?;
    // The mechanism passes if cross-model beats chance (1/4 docs) clearly and
    // at least one direction is lossless at probe scale.
    let pass = acc(ab) >= 0.6 && acc(ba) >= 0.6;
    println!("KEY SMOKE: {}", if pass { "PASS" } else { "CHECK NUMBERS" });
    Ok(if pass { 0 } else { 1 })
}

// ---------------------------------------------------------------- PAMI (§8.11)
/// RoPE-family phase rotation: pair i of the vector rotates by
/// rank * (pi/2) * (1 - 2i/d). Same-rank inner products are preserved
/// exactly; cross-rank products are phase-suppressed.
pub fn rotate_rank(v: &[f32], rank: f32) -> Vec<f32> {
    let d = v.len();
    let mut out = v.to_vec();
    if rank == 0.0 {
        return out;
    }
    for i in 0..d / 2 {
        let frac = 1.0 - (2 * i) as f32 / d as f32;
        let ang = rank * std::f32::consts::FRAC_PI_2 * frac;
        let (c, s) = (ang.cos(), ang.sin());
        let (x, y) = (v[2 * i], v[2 * i + 1]);
        out[2 * i] = x * c - y * s;
        out[2 * i + 1] = x * s + y * c;
    }
    out
}

/// Participation-ratio proxy from the Gram matrix — the cheap end of
/// the multi-fractal spectrum (R8 statistic; actuation deferred).
pub fn participation_ratio(vecs: &[Vec<f32>]) -> f32 {
    let n = vecs.len();
    if n == 0 {
        return 0.0;
    }
    let mut trace = 0f32;
    let mut fro = 0f32;
    for i in 0..n {
        for j in 0..n {
            let g: f32 = vecs[i].iter().zip(vecs[j].iter()).map(|(a, b)| a * b).sum();
            if i == j {
                trace += g;
            }
            fro += g * g;
        }
    }
    if fro == 0.0 {
        0.0
    } else {
        trace * trace / fro
    }
}

pub fn pami_smoke(work: &Path, corpus: &Path) -> Res<i32> {
    fs::create_dir_all(work)?;
    // chunks (rank 0) via the structure-aware chunker, tf embedder
    let mut chunks: Vec<ChunkRec> = Vec::new();
    let mut entries: Vec<PathBuf> = fs::read_dir(corpus)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|x| x == "md").unwrap_or(false))
        .collect();
    entries.sort();
    for p in &entries {
        let doc_id = p.file_stem().unwrap().to_string_lossy().to_string();
        for (section, ord, body) in chunk_markdown(&fs::read_to_string(p)?) {
            chunks.push(ChunkRec {
                id: 0,
                collection: String::new(),
                doc_id: doc_id.clone(),
                section,
                ordinal: ord,
                parent: None,
                text: body,
            });
        }
    }
    let chunk_vecs: Vec<Vec<f32>> = chunks.iter().map(|c| tf_embed(&c.text, 256)).collect();
    // document rank (rank 1): partition-and-aggregate — mean of children
    let mut doc_ids: Vec<String> = Vec::new();
    for c in &chunks {
        if !doc_ids.contains(&c.doc_id) {
            doc_ids.push(c.doc_id.clone());
        }
    }
    let mut doc_vecs: Vec<Vec<f32>> = Vec::new();
    for d in &doc_ids {
        let mut acc = vec![0f32; 256];
        let mut n = 0usize;
        for (c, v) in chunks.iter().zip(chunk_vecs.iter()) {
            if &c.doc_id == d {
                for j in 0..256 {
                    acc[j] += v[j];
                }
                n += 1;
            }
        }
        let norm = acc.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
        doc_vecs.push(acc.iter().map(|x| x / norm).collect());
    }
    let pr_chunk = participation_ratio(&chunk_vecs);
    let pr_doc = participation_ratio(&doc_vecs);

    // mixed-rank index entries: (is_doc, doc_id, vector)
    let build_index = |phase: bool| -> Vec<(bool, String, Vec<f32>)> {
        let mut idx = Vec::new();
        for (c, v) in chunks.iter().zip(chunk_vecs.iter()) {
            idx.push((false, c.doc_id.clone(), if phase { rotate_rank(v, 0.0) } else { v.clone() }));
        }
        for (d, v) in doc_ids.iter().zip(doc_vecs.iter()) {
            idx.push((true, d.clone(), if phase { rotate_rank(v, 1.0) } else { v.clone() }));
        }
        idx
    };
    let run = |phase: bool, qtext: &str, target_doc_rank: bool| -> (f32, String) {
        let idx = build_index(phase);
        let q = tf_embed(qtext, 256);
        let q = if phase {
            rotate_rank(&q, if target_doc_rank { 1.0 } else { 0.0 })
        } else {
            q
        };
        let mut scored: Vec<(usize, f32)> = idx
            .iter()
            .enumerate()
            .map(|(i, (_, _, v))| (i, cosine(&q, v)))
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let top5 = &scored[..5.min(scored.len())];
        let pure = top5
            .iter()
            .filter(|(i, _)| idx[*i].0 == target_doc_rank)
            .count() as f32
            / top5.len() as f32;
        (pure, idx[scored[0].0].1.clone())
    };

    let chunk_queries = [
        ("tide window lantern code", "harbor"),
        ("quench bath timing", "quench"),
        ("lantern oil pint ration", "ledger"),
    ];
    let doc_queries = [
        ("harbor port operations rules signals", "harbor"),
        ("kiln firing steel workshop log", "quench"),
    ];
    let mut rows = Vec::new();
    let (mut pur_off, mut pur_on, mut purd_off, mut purd_on) = (0f32, 0f32, 0f32, 0f32);
    let (mut top_off, mut top_on) = (0usize, 0usize);
    for (q, expect) in chunk_queries {
        let (po, t1o) = run(false, q, false);
        let (pn, t1n) = run(true, q, false);
        pur_off += po;
        pur_on += pn;
        top_off += (t1o == expect) as usize;
        top_on += (t1n == expect) as usize;
        rows.push(serde_json::json!({"query": q, "target_rank": "chunk", "expect_doc": expect,
            "purity_off": po, "purity_on": pn, "top1_off": t1o, "top1_on": t1n}));
        println!("chunk-q '{q}': purity off {po:.2} on {pn:.2} | top1 off {t1o} on {t1n}");
    }
    for (q, expect) in doc_queries {
        let (po, _) = run(false, q, true);
        let (pn, t1n) = run(true, q, true);
        purd_off += po;
        purd_on += pn;
        rows.push(serde_json::json!({"query": q, "target_rank": "document", "expect_doc": expect,
            "purity_off": po, "purity_on": pn, "top1_on": t1n}));
        println!("doc-q '{q}': doc-purity off {po:.2} on {pn:.2} | top1 on {t1n}");
    }
    let nc = chunk_queries.len() as f32;
    let nd = doc_queries.len() as f32;
    let (pc_off, pc_on, pd_off, pd_on) = (pur_off / nc, pur_on / nc, purd_off / nd, purd_on / nd);
    println!(
        "PAMI rank purity: chunk off {pc_off:.2} on {pc_on:.2} | doc off {pd_off:.2} on {pd_on:.2} | chunk top1 off {top_off}/3 on {top_on}/3"
    );
    println!("multi-fractal stat (participation-ratio proxy): chunk rank {pr_chunk:.2} of 12, doc rank {pr_doc:.2} of 4");
    let checks = [
        ("phase raises or perfects chunk-rank purity", pc_on >= pc_off),
        ("phase raises or perfects doc-rank purity", pd_on >= pd_off),
        ("phase does not cost chunk top-1 accuracy", top_on >= top_off),
    ];
    let passed = checks.iter().filter(|c| c.1).count();
    for (n, p) in &checks {
        println!("{} {n}", if *p { "PASS" } else { "FAIL" });
    }
    let receipt = serde_json::json!({
        "experiment": "PAMI phase-augmented mixed-rank index (Spec 17 §8.11)",
        "phase_schedule": "pair i rotates by rank * (pi/2) * (1 - 2i/d); ranks: chunk=0, document=1",
        "aggregation": "document rank = mean of child chunk vectors (partition-and-aggregate, primitive form)",
        "purity": {"chunk_off": pc_off, "chunk_on": pc_on, "doc_off": pd_off, "doc_on": pd_on},
        "chunk_top1": {"off": top_off, "on": top_on, "of": 3},
        "multifractal_proxy": {"participation_ratio_chunk_rank": pr_chunk, "participation_ratio_doc_rank": pr_doc},
        "checks": checks.iter().map(|(n, p)| serde_json::json!({"name": n, "pass": p})).collect::<Vec<_>>(),
        "label": "probe scale, tf-hash test embedder for the index mechanics; phase ablation is theta=0."
    });
    fs::write(work.join("pami-receipt.json"), serde_json::to_string_pretty(&receipt)?)?;
    println!("PAMI SMOKE: {passed}/{} PASS", checks.len());
    Ok(if passed == checks.len() { 0 } else { 1 })
}
