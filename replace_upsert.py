import sys

file_path = "src-tauri/src/vector_store.rs"
with open(file_path, "r") as f:
    lines = f.readlines()

# Find the start of upsert_chunk_internal
start_idx = -1
for i, line in enumerate(lines):
    if "fn upsert_chunk_internal(&self, conn: &Connection, chunk: &Chunk)" in line:
        start_idx = i - 1 # Include the doc comment
        break

if start_idx == -1:
    print("Could not find upsert_chunk_internal")
    sys.exit(1)

# Find the end of the function (assuming it ends at the next pub fn or a specific marker)
end_idx = -1
for i in range(start_idx + 1, len(lines)):
    if "    pub fn delete_by_source" in lines[i]:
        end_idx = i - 1
        break

if end_idx == -1:
    print("Could not find end of function")
    sys.exit(1)

# The new content
new_code = [
    "    /// Insert or update a chunk with its embedding (public wrapper with transaction)\n",
    "    pub fn upsert_chunk(&self, chunk: &Chunk) -> Result<(), VectorStoreError> {\n",
    "        let tx = self.conn.unchecked_transaction()?;\n",
    "        self.upsert_chunk_internal(&tx, chunk)?;\n",
    "        tx.commit()?;\n",
    "        Ok(())\n",
    "    }\n",
    "\n",
    "    /// Internal logic for upserting a chunk, using an existing connection/transaction\n",
    "    fn upsert_chunk_internal(&self, conn: &Connection, chunk: &Chunk) -> Result<(), VectorStoreError> {\n",
    "        let now = Utc::now().to_rfc3339();\n",
    "        let source_type_str = match chunk.source_type {\n",
    "            SourceType::Topic => \"topic\",\n",
    "            SourceType::Insight => \"insight\",\n",
    "        };\n",
    "        let content_hash = Self::content_hash(&chunk.text);\n",
    "\n",
    "        if chunk.embedding.len() != EMBEDDING_DIM {\n",
    "            return Err(VectorStoreError::Migration(\n",
    "                format!(\"Invalid embedding dimension: expected {}, got {}\", EMBEDDING_DIM, chunk.embedding.len()),\n",
    "            ));\n",
    "        }\n",
    "\n",
    "        // Upsert metadata\n",
    "        conn.execute(\n",
    "            r#\"\n",
    "            INSERT INTO chunks (id, source_type, source_name, heading, text, start_line, end_line, content_hash, created_at, updated_at)\n",
    "            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)\n",
    "            ON CONFLICT(id) DO UPDATE SET\n",
    "                source_type = excluded.source_type,\n",
    "                source_name = excluded.source_name,\n",
    "                heading = excluded.heading,\n",
    "                text = excluded.text,\n",
    "                start_line = excluded.start_line,\n",
    "                end_line = excluded.end_line,\n",
    "                content_hash = excluded.content_hash,\n",
    "                updated_at = excluded.updated_at\n",
    "            \"#,\n",
    "            params![\n",
    "                chunk.id,\n",
    "                source_type_str,\n",
    "                chunk.source_name,\n",
    "                chunk.heading,\n",
    "                chunk.text,\n",
    "                chunk.start_line,\n",
    "                chunk.end_line,\n",
    "                content_hash,\n",
    "                now,\n",
    "            ],\n",
    "        )?;\n",
    "\n",
    "        // Upsert embedding into vec0 virtual table\n",
    "        // vec0 doesn't strictly support UPSERT/REPLACE in all versions, so we DELETE then INSERT\n",
    "        let embedding_bytes = f32_vec_to_bytes(&chunk.embedding);\n",
    "\n",
    "        conn.execute(\n",
    "            \"DELETE FROM chunk_embeddings WHERE chunk_id = ?\",\n",
    "            [chunk.id.as_str()],\n",
    "        )?;\n",
    "\n",
    "        conn.execute(\n",
    "            \"INSERT INTO chunk_embeddings (chunk_id, embedding) VALUES (?, ?)\",\n",
    "            params![chunk.id, embedding_bytes],\n",
    "        )?;\n",
    "\n",
    "        // Also cache the embedding\n",
    "        conn.execute(\n",
    "            \"INSERT OR REPLACE INTO embedding_cache (content_hash, embedding, created_at) VALUES (?, ?, ?)\",\n",
    "            params![content_hash, embedding_bytes, now],\n",
    "        )?;\n",
    "\n",
    "        Ok(())\n",
    "    }\n"
]

# Replace the lines
new_lines = lines[:start_idx] + new_code + lines[end_idx:]

with open(file_path, "w") as f:
    f.writelines(new_lines)

print("Successfully refactored upsert_chunk")
