import re

with open('src/main.ts', 'r') as f:
    content = f.read()

# Replace the import
content = content.replace('addMessage as addMessageToChat,', 'addMessage,')

# Replace addMessage("user", text, currentImages);
content = content.replace('addMessage("user", text, currentImages);', 'addMessage(chatArea, "user", text, currentImages);')

# Replace addMessage(displayRole as "user" | "assistant" | "cron", msg.content || "", msg.images, fragment);
content = content.replace('addMessage(displayRole as "user" | "assistant" | "cron", msg.content || "", msg.images, fragment);', 'addMessage(fragment, displayRole as "user" | "assistant" | "cron", msg.content || "", msg.images);')

# Replace addMessage("assistant", msg.content, msg.images, fragment);
content = content.replace('addMessage("assistant", msg.content, msg.images, fragment);', 'addMessage(fragment, "assistant", msg.content, msg.images);')

# Replace addMessage("cron", prompt, undefined);
content = content.replace('addMessage("cron", prompt, undefined);', 'addMessage(chatArea, "cron", prompt, undefined);')

with open('src/main.ts', 'w') as f:
    f.write(content)
