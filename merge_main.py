import re

with open("src/main.ts", "r") as f:
    content = f.read()

# Resolve first conflict (imports)
content = content.replace("""<<<<<<< HEAD
  renderSessionList,
=======
  populateModelDropdown,
>>>>>>> origin/main""", """  renderSessionList,
  populateModelDropdown,""")

# Resolve second conflict (session list rendering)
# We want to keep the origin/main approach of creating elements (since it added delete buttons)
# BUT we want to apply the XSS fix to it!
# Wait, the XSS fix was to use textContent anyway!
# In origin/main: titleEl.textContent = s.title; This ALREADY fixes the XSS because textContent doesn't parse HTML.
# So the XSS was actually fixed by the DOM creation refactor in origin/main!
# We can just adopt origin/main's implementation entirely.
# Let's verify origin/main's implementation.

conflict2_pattern = re.compile(r"<<<<<<< HEAD.*?=======(.*?)>>>>>>> origin/main", re.DOTALL)
match = conflict2_pattern.search(content)

if match:
    origin_main_code = match.group(1)
    # We will keep origin_main_code
    content = content[:match.start()] + origin_main_code + content[match.end():]
    print("Resolved conflict 2")
else:
    print("Could not find conflict 2")

with open("src/main.ts", "w") as f:
    f.write(content)
