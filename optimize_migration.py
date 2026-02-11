import sys

file_path = "src-tauri/src/vector_store.rs"
with open(file_path, "r") as f:
    content = f.read()

search_text = """    /// Migrate from JSON chunk index to SQLite
    pub fn migrate_from_json(&self, chunk_index: &ChunkIndex) -> Result<usize, VectorStoreError> {
        log::info!("[VectorStore] Migrating {} chunks from JSON", chunk_index.chunks.len());

        let mut count = 0;
        for chunk in &chunk_index.chunks {
            self.upsert_chunk(chunk)?;
            count += 1;
        }

        log::info!("[VectorStore] Migration complete: {} chunks", count);
        Ok(count)
    }"""

replace_text = """    /// Migrate from JSON chunk index to SQLite
    pub fn migrate_from_json(&self, chunk_index: &ChunkIndex) -> Result<usize, VectorStoreError> {
        log::info!("[VectorStore] Migrating {} chunks from JSON", chunk_index.chunks.len());

        let tx = self.conn.unchecked_transaction()?;
        let mut count = 0;
        for chunk in &chunk_index.chunks {
            self.upsert_chunk_internal(&tx, chunk)?;
            count += 1;
        }
        tx.commit()?;

        log::info!("[VectorStore] Migration complete: {} chunks", count);
        Ok(count)
    }"""

if search_text in content:
    new_content = content.replace(search_text, replace_text)
    with open(file_path, "w") as f:
        f.write(new_content)
    print("Successfully optimized migrate_from_json")
else:
    print("Search text not found")
    sys.exit(1)
