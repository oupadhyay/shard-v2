import sys

file_path = "src-tauri/src/vector_store.rs"
with open(file_path, "r") as f:
    content = f.read()

content = content.replace("    fn upsert_chunk_internal", "    pub(crate) fn upsert_chunk_internal")
content = content.replace("    fn delete_by_source_internal", "    pub(crate) fn delete_by_source_internal")

with open(file_path, "w") as f:
    f.write(content)

print("Successfully updated visibility to pub(crate)")
