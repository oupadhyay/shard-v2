# TODO

## Fix Deep Research?

## BM25 & RAG Reciprocal Rank Fusion

Reciprocal Rank Fusion merges BM25 and embedding results by summing 1/(k + rank).

### Hybrid retrieval with RRF

- Indexing: Keep two indices over the same corpus: BM25 (inverted index) and dense (vector index with embeddings for passages/documents).
- Initial retrieval: For a query, run BM25 to get top-N and dense ANN search to get top-N. Assign each result a rank within its list (1-based).
- Fusion rule: For each unique document across lists, compute an RRF score: $RRF(d)=\sum_{L \in \{BM25,\,Dense\}} \frac{1}{k + rank_L(d)}$ where $k$ is a small constant (e.g., 60) that dampens early-rank dominance.
- Final ranking: Sort by fused score descending. Return top-K to the reader/generator.

### Why RRF works

- Robustness: Rewards documents that appear high in either list without overfitting to one modality.
- Diversity: Combines lexical precision with semantic recall; avoids embedding-only drift and BM25-only brittleness.
- Parameter-light: Only needs N, K, and k; no training, easy to tune.

### Practical choices

- N values: 100–200 from each retriever; more if passages are short.
- k constant: 60 is a common default; raise k if you want shallower fusion that values mid-ranked items more evenly.
- Granularity: Passage-level indexing typically outperforms document-level for RAG.
- De-duplication: If multiple passages from the same doc surface, either keep passage-level or aggregate per document depending on your downstream reader.
- Query routing: Regex/ID/code-like → increase BM25 weight; conversational → increase dense weight by multiplying its RRF contribution.

### Minimal implementation sketch

- Inputs: bm25Results = [(docId, rank)], denseResults = [(docId, rank)]
- Compute: For each docId in union:
  - score = 0
  - if in bm25: score += 1/(k + rankBM25)
  - if in dense: score += 1/(k + rankDense)
- Output: Sort by score desc, take topK.

## Multi-Provider Support

- Adding support for multiple providers (e.g. GROQ, Claude) for chat completion.
