import sys

file_path = "src-tauri/src/memories.rs"
with open(file_path, "r") as f:
    content = f.read()

search_text = """    let saved_count = tokio::task::spawn_blocking(move || -> Result<usize, String> {
        let mut count = 0;
        let vs = get_vector_store(&handle)?;

        // Step C: Delete old chunks only for sources where ALL chunks are ready
        // Skip delete if any embedding failed for the source (to avoid data loss)
        for (s_type, s_name) in &sources {
            let key = (s_type.clone(), s_name.clone());
            let expected = expected_per_source.get(&key).copied().unwrap_or(0);
            let actual = actual_chunks_per_source.get(&key).copied().unwrap_or(0);

            if actual < expected {
                log::warn!(
                    "[Chunk] Skipping delete for {} - only {}/{} chunks ready (embedding failures)",
                    s_name, actual, expected
                );
                continue;
            }

            if let Err(e) = vs.delete_by_source(s_type.clone(), s_name) {
                log::warn!("[Chunk] Failed to clear old chunks for {}: {}", s_name, e);
            }
        }

        // Save cached chunks
        for chunk in chunks_to_save {
            if let Err(e) = vs.upsert_chunk(&chunk) {
                log::error!("[Chunk] Failed to save cached chunk {}: {}", chunk.id, e);
            } else {
                count += 1;
            }
        }

        // Save newly generated chunks
        for chunk in gen_chunks {
            if let Err(e) = vs.upsert_chunk(&chunk) {
                log::error!("[Chunk] Failed to save generated chunk {}: {}", chunk.id, e);
            } else {
                count += 1;
            }
        }

        // 4. Cleanup deleted files
        for (s_type, s_name) in known {
            if !processed.contains(&(s_type.clone(), s_name.clone())) {
                 if let Err(e) = vs.delete_by_source(s_type.clone(), &s_name) {
                     log::warn!("[Chunk] Failed to cleanup deleted source {}: {}", s_name, e);
                 } else {
                     log::info!("[Chunk] Removed deleted source: {}", s_name);
                 }
            }
        }

        Ok(count)
    }).await.map_err(|e| format!("Blocking save task failed: {}", e))??;"""

replace_text = """    let saved_count = tokio::task::spawn_blocking(move || -> Result<usize, String> {
        let mut count = 0;
        let vs = get_vector_store(&handle)?;

        // Wrap everything in a single transaction for performance
        let tx = vs.conn.unchecked_transaction()
            .map_err(|e| format!("Failed to start transaction: {}", e))?;

        // Step C: Delete old chunks only for sources where ALL chunks are ready
        // Skip delete if any embedding failed for the source (to avoid data loss)
        for (s_type, s_name) in &sources {
            let key = (s_type.clone(), s_name.clone());
            let expected = expected_per_source.get(&key).copied().unwrap_or(0);
            let actual = actual_chunks_per_source.get(&key).copied().unwrap_or(0);

            if actual < expected {
                log::warn!(
                    "[Chunk] Skipping delete for {} - only {}/{} chunks ready (embedding failures)",
                    s_name, actual, expected
                );
                continue;
            }

            if let Err(e) = vs.delete_by_source_internal(&tx, s_type.clone(), s_name) {
                log::warn!("[Chunk] Failed to clear old chunks for {}: {}", s_name, e);
            }
        }

        // Save cached chunks
        for chunk in chunks_to_save {
            if let Err(e) = vs.upsert_chunk_internal(&tx, &chunk) {
                log::error!("[Chunk] Failed to save cached chunk {}: {}", chunk.id, e);
            } else {
                count += 1;
            }
        }

        // Save newly generated chunks
        for chunk in gen_chunks {
            if let Err(e) = vs.upsert_chunk_internal(&tx, &chunk) {
                log::error!("[Chunk] Failed to save generated chunk {}: {}", chunk.id, e);
            } else {
                count += 1;
            }
        }

        // 4. Cleanup deleted files
        for (s_type, s_name) in known {
            if !processed.contains(&(s_type.clone(), s_name.clone())) {
                 if let Err(e) = vs.delete_by_source_internal(&tx, s_type.clone(), &s_name) {
                     log::warn!("[Chunk] Failed to cleanup deleted source {}: {}", s_name, e);
                 } else {
                     log::info!("[Chunk] Removed deleted source: {}", s_name);
                 }
            }
        }

        tx.commit().map_err(|e| format!("Failed to commit transaction: {}", e))?;

        Ok(count)
    }).await.map_err(|e| format!("Blocking save task failed: {}", e))??;"""

if search_text in content:
    new_content = content.replace(search_text, replace_text)
    with open(file_path, "w") as f:
        f.write(new_content)
    print("Successfully optimized rebuild_chunk_index")
else:
    print("Search text not found")
    sys.exit(1)
