import re

with open('src/dedicated.ts', 'r') as f:
    content = f.read()

# Replace the import
content = content.replace('addMessage as addMessageToChat,', 'addMessage,')

# Replace addMessage("user", text, currentImages);
content = content.replace('addMessage("user", text, currentImages);', 'addMessage(chatArea, "user", text, currentImages);')

# Replace addMessage(displayRole as "user" | "assistant" | "cron", msg.content || "", msg.images, fragment);
content = content.replace('addMessage(displayRole as "user" | "assistant" | "cron", msg.content || "", msg.images, fragment);', 'addMessage(fragment, displayRole as "user" | "assistant" | "cron", msg.content || "", msg.images);')

# Replace if (msg.content) addMessage("assistant", msg.content, msg.images, fragment);
content = content.replace('if (msg.content) addMessage("assistant", msg.content, msg.images, fragment);', 'if (msg.content) addMessage(fragment, "assistant", msg.content, msg.images);')

with open('src/dedicated.ts', 'w') as f:
    f.write(content)
