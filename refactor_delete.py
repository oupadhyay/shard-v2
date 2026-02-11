import sys

file_path = "src-tauri/src/vector_store.rs"
with open(file_path, "r") as f:
    lines = f.readlines()

# Find delete_by_source
start_idx = -1
for i, line in enumerate(lines):
    if "pub fn delete_by_source(&self, source_type: SourceType, source_name: &str) -> Result<usize, VectorStoreError> {" in line:
        start_idx = i - 1
        break

if start_idx == -1:
    print("Could not find delete_by_source")
    sys.exit(1)

# Find the end of the function
end_idx = -1
for i in range(start_idx + 1, len(lines)):
    if "pub fn knn_search" in lines[i]:
        end_idx = i - 1
        break

if end_idx == -1:
    print("Could not find end of function")
    sys.exit(1)

new_code = [
    "    /// Delete all chunks for a given source (public wrapper with transaction)\n",
    "    pub fn delete_by_source(&self, source_type: SourceType, source_name: &str) -> Result<usize, VectorStoreError> {\n",
    "        let tx = self.conn.unchecked_transaction()?;\n",
    "        let deleted = self.delete_by_source_internal(&tx, source_type, source_name)?;\n",
    "        tx.commit()?;\n",
    "        Ok(deleted)\n",
    "    }\n",
    "\n",
    "    /// Internal logic for deleting chunks by source\n",
    "    fn delete_by_source_internal(&self, conn: &Connection, source_type: SourceType, source_name: &str) -> Result<usize, VectorStoreError> {\n",
    "        let source_type_str = match source_type {\n",
    "            SourceType::Topic => \"topic\",\n",
    "            SourceType::Insight => \"insight\",\n",
    "        };\n",
    "\n",
    "        // Delete from vec0 table using subquery (efficient single-statement delete)\n",
    "        conn.execute(\n",
    "            \"DELETE FROM chunk_embeddings WHERE chunk_id IN (SELECT id FROM chunks WHERE source_type = ? AND source_name = ?)\",\n",
    "            params![source_type_str, source_name],\n",
    "        )?;\n",
    "\n",
    "        // Delete from chunks table\n",
    "        let deleted = conn.execute(\n",
    "            \"DELETE FROM chunks WHERE source_type = ? AND source_name = ?\",\n",
    "            params![source_type_str, source_name],\n",
    "        )?;\n",
    "\n",
    "        Ok(deleted)\n",
    "    }\n"
]

new_lines = lines[:start_idx] + new_code + lines[end_idx:]

with open(file_path, "w") as f:
    f.writelines(new_lines)

print("Successfully refactored delete_by_source")
