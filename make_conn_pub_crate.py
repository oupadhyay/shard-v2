import sys

file_path = "src-tauri/src/vector_store.rs"
with open(file_path, "r") as f:
    content = f.read()

content = content.replace("pub struct VectorStore {\n    conn: Connection,", "pub struct VectorStore {\n    pub(crate) conn: Connection,")

with open(file_path, "w") as f:
    f.write(content)

print("Successfully updated VectorStore.conn visibility to pub(crate)")
