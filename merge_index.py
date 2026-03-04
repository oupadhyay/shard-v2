import re

with open("src/ui/index.ts", "r") as f:
    content = f.read()

conflict_pattern = re.compile(r"<<<<<<< HEAD.*?=======(.*?)>>>>>>> origin/main", re.DOTALL)
match = conflict_pattern.search(content)

if match:
    # Combine the exports
    resolved = """export { SETTINGS_MODAL_HTML, initSettingsTabs, populateModelDropdown } from "./settings";\nexport { SESSIONS_MODAL_HTML, renderSessionList } from "./sessions";\n"""
    content = content[:match.start()] + resolved + content[match.end():]
    print("Resolved index conflict")

with open("src/ui/index.ts", "w") as f:
    f.write(content)
