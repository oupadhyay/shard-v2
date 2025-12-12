# Rate Limits

Rate limits act as control measures to regulate how frequently users and applications can access our API within specified timeframes. These limits help ensure service stability, fair access, and protection against misuse so that we can serve reliable and fast inference for all.

## [Understanding Rate Limits](#understanding-rate-limits)

Rate limits are measured in:

* **RPM:** Requests per minute
* **RPD:** Requests per day
* **TPM:** Tokens per minute
* **TPD:** Tokens per day
* **ASH:** Audio seconds per hour
* **ASD:** Audio seconds per day

[Cached tokens](https://console.groq.com/docs/prompt-caching) do not count towards your rate limits.

Rate limits apply at the organization level, not individual users. You can hit any limit type depending on which threshold you reach first.

**Example:** Let's say your RPM = 50 and your TPM = 200K. If you were to send 50 requests with only 100 tokens within a minute, you would reach your limit even though you did not send 200K tokens within those 50 requests.

## [Rate Limits](#rate-limits)

The following is a high level summary and there may be exceptions to these limits. You can view the current, exact rate limits for your organization on the [limits page](https://console.groq.com/settings/limits) in your account settings.

**Need higher rate limits?** Upgrade to [Developer plan](https://console.groq.com/settings/billing/plans) to access higher limits, [Batch](https://console.groq.com/docs/batch) and [Flex](https://console.groq.com/docs/flex-processing) processing, and more. Note that the limits shown below are the base limits for the Developer plan, and higher limits are available for select workloads and enterprise use cases.

### Free Plan Limits

| MODEL ID                                      | RPM | RPD  | TPM  | TPD  | ASH  | ASD   |
| --------------------------------------------- | -- | ----- | ---- | ---- | ---- | ----- |
| allam-2-7b                                    | 30 | 7K    | 6K   | 500K | \-   | \-    |
| openai/gpt-oss-20b                            | 30 | 1K    | 8K   | 200K | \-   | \-    |
| qwen/qwen3-32b                                | 60 | 1K    | 6K   | 500K | \-   | \-    |
| whisper-large-v3                              | 20 | 2K    | \-   | \-   | 7.2K | 28.8K |
| whisper-large-v3-turbo                        | 20 | 2K    | \-   | \-   | 7.2K | 28.8K |

## [Rate Limit Headers](#rate-limit-headers)

In addition to viewing your limits on your account's [limits](https://console.groq.com/settings/limits) page, you can also view rate limit information such as remaining requests and tokens in HTTP response headers as follows:

The following headers are set (values are illustrative):

| Header                         | Value    | Notes                                    |
| ------------------------------ | -------- | ---------------------------------------- |
| retry-after                    | 2        | In seconds                               |
| x-ratelimit-limit-requests     | 14400    | Always refers to Requests Per Day (RPD)  |
| x-ratelimit-limit-tokens       | 18000    | Always refers to Tokens Per Minute (TPM) |
| x-ratelimit-remaining-requests | 14370    | Always refers to Requests Per Day (RPD)  |
| x-ratelimit-remaining-tokens   | 17997    | Always refers to Tokens Per Minute (TPM) |
| x-ratelimit-reset-requests     | 2m59.56s | Always refers to Requests Per Day (RPD)  |
| x-ratelimit-reset-tokens       | 7.66s    | Always refers to Tokens Per Minute (TPM) |

## [Handling Rate Limits](#handling-rate-limits)

When you exceed rate limits, our API returns a `429 Too Many Requests` HTTP status code.

**Note**: `retry-after` is only set if you hit the rate limit and status code 429 is returned. The other headers are always included.

## Prompt Caching

Model prompts often contain repetitive content, such as system prompts and tool definitions. Prompt caching automatically reuses computation from recent requests when they share a common prefix, delivering significant cost savings and improved response times while maintaining data privacy through volatile-only storage that expires automatically.

Prompt caching works automatically on all your API requests with no code changes required and no additional fees.

### [How It Works](#how-it-works)

1. **Prefix Matching**: When you send a request, the system examines and identifies matching prefixes from recently processed requests stored temporarily in volatile memory. Prefixes can include system prompts, tool definitions, few-shot examples, and more.
2. **Cache Hit**: If a matching prefix is found, cached computation is reused, dramatically reducing latency and token costs by 50% for cached portions.
3. **Cache Miss**: If no match exists, your prompt is processed normally, with the prefix temporarily cached for potential future matches.
4. **Automatic Expiration**: All cached data automatically expires after 2 hours without use.

Prompt caching works automatically on all your API requests to supported models with no code changes required and no additional fees. Groq tries to maximize cache hits, but this is not guaranteed. Pricing discount will only apply on successful cache hits.

Cached tokens do not count towards your rate limits. However, cached tokens are subtracted from your limits after processing, so it's still possible to hit your limits if you are sending a large number of input tokens in parallel requests.

### [Supported Models](#supported-models)

Prompt caching is currently only supported for the following models:

| Model ID                         | Model                                                             |
| -------------------------------- | ----------------------------------------------------------------- |
| openai/gpt-oss-20b               | [GPT-OSS 20B](https://console.groq.com/docs/model/openai/gpt-oss-20b)                     |
| openai/gpt-oss-120b              | [GPT-OSS 120B](https://console.groq.com/docs/model/openai/gpt-oss-120b)                   |
| openai/gpt-oss-safeguard-20b     | [GPT-OSS-Safeguard 20B](https://console.groq.com/docs/model/openai/gpt-oss-safeguard-20b) |

We're starting with a limited selection of models and will roll out prompt caching to more models soon.

### [Pricing](#pricing)

Prompt caching is provided at no additional cost. There is a 50% discount for cached input tokens.

### [Structuring Prompts for Optimal Caching](#structuring-prompts-for-optimal-caching)

Cache hits are only possible for exact prefix matches within a prompt. To realize caching benefits, you need to think strategically about prompt organization:

#### [Optimal Prompt Structure](#optimal-prompt-structure)

Place static content like instructions and examples at the beginning of your prompt, and put variable content, such as user-specific information, at the end. This maximizes the length of the reusable prefix across different requests.

If you put variable information (like timestamps or user IDs) at the beginning, even identical system instructions later in the prompt won't benefit from caching because the prefixes won't match.

**Place static content first:**

* System prompts and instructions
* Few-shot examples
* Tool definitions
* Schema definitions
* Common context or background information

**Place dynamic content last:**

* User-specific queries
* Variable data
* Timestamps
* Session-specific information
* Unique identifiers

#### [Example Structure](#example-structure)

```curl
[SYSTEM PROMPT - Static]
[TOOL DEFINITIONS - Static]
[FEW-SHOT EXAMPLES - Static]
[COMMON INSTRUCTIONS - Static]
[USER QUERY - Dynamic]
[SESSION DATA - Dynamic]
```

This structure maximizes the likelihood that the static prefix portion will match across different requests, enabling cache hits while keeping user-specific content at the end.

### [Prompt Caching Examples](#prompt-caching-examples)

Multi turn conversationsLarge prompts and contextTool definitions and use

```javascript
import Groq from "groq-sdk";

const groq = new Groq();

async function multiTurnConversation() {
  // Initial conversation with system message and first user input
  const initialMessages = [
    {
      role: "system",
      content: "You are a helpful AI assistant that provides detailed explanations about complex topics. Always provide comprehensive answers with examples and context."
    },
    {
      role: "user",
      content: "What is quantum computing?"
    }
  ];

  // First request - creates cache for system message
  const firstResponse = await groq.chat.completions.create({
    messages: initialMessages,
    model: "openai/gpt-oss-120b"
  });

  console.log("First response:", firstResponse.choices[0].message.content);
  console.log("Usage:", firstResponse.usage);

  // Continue conversation - system message and previous context will be cached
  const conversationMessages = [
    ...initialMessages,
    firstResponse.choices[0].message,
    {
      role: "user",
      content: "Can you give me a simple example of how quantum superposition works?"
    }
  ];

  const secondResponse = await groq.chat.completions.create({
    messages: conversationMessages,
    model: "openai/gpt-oss-120b"
  });

  console.log("Second response:", secondResponse.choices[0].message.content);
  console.log("Usage:", secondResponse.usage);

  // Continue with third turn
  const thirdTurnMessages = [
    ...conversationMessages,
    secondResponse.choices[0].message,
    {
      role: "user",
      content: "How does this relate to quantum entanglement?"
    }
  ];

  const thirdResponse = await groq.chat.completions.create({
    messages: thirdTurnMessages,
    model: "openai/gpt-oss-120b"
  });

  console.log("Third response:", thirdResponse.choices[0].message.content);
  console.log("Usage:", thirdResponse.usage);
}

multiTurnConversation().catch(console.error);
```

#### How Prompt Caching Works in Multi-Turn Conversations

In this example, we demonstrate how to use prompt caching in a multi-turn conversation.

During each turn, the system automatically caches the longest matching prefix from previous requests. The system message and conversation history that remain unchanged between requests will be cached, while only new user messages and assistant responses need fresh processing.

This approach is useful for maintaining context in ongoing conversations without repeatedly processing the same information.

**For the first request:**

* `prompt_tokens`: Number of tokens in the system message and first user message
* `cached_tokens`: 0 (no cache hit on first request)

**For subsequent requests within the cache lifetime:**

* `prompt_tokens`: Total number of tokens in the entire conversation (system message + conversation history + new user message)
* `cached_tokens`: Number of tokens in the system message and previous conversation history that were served from cache

When set up properly, you should see increasing cache efficiency as the conversation grows, with the system message and earlier conversation turns being served from cache while only new content requires processing.

```javascript
import Groq from "groq-sdk";

const groq = new Groq();

async function analyzeLegalDocument() {
  // First request - creates cache for the large legal document
  const systemPrompt = `You are a legal expert AI assistant. Analyze the following legal document and provide detailed insights.

LEGAL DOCUMENT: <entire contents of large legal document>`;

  const firstAnalysis = await groq.chat.completions.create({
    messages: [
      {
        role: "system",
        content: systemPrompt
      },
      {
        role: "user",
        content: "What are the key provisions regarding user account termination in this agreement?"
      }
    ],
    model: "openai/gpt-oss-120b"
  });

  console.log("First analysis:", firstAnalysis.choices[0].message.content);
  console.log("Usage:", firstAnalysis.usage);

  // Second request - legal document will be cached, only new question processed
  const secondAnalysis = await groq.chat.completions.create({
    messages: [
      {
        role: "system",
        content: systemPrompt
      },
      {
        role: "user",
        content: "What are the intellectual property rights implications for users who submit content?"
      }
    ],
    model: "openai/gpt-oss-120b"
  });

  console.log("Second analysis:", secondAnalysis.choices[0].message.content);
  console.log("Usage:", secondAnalysis.usage);

  // Third request - same large context, different question
  const thirdAnalysis = await groq.chat.completions.create({
    messages: [
      {
        role: "system",
        content: systemPrompt
      },
      {
        role: "user",
        content: "Are there any concerning limitations of liability clauses that users should be aware of?"
      }
    ],
    model: "openai/gpt-oss-120b"
  });

  console.log("Third analysis:", thirdAnalysis.choices[0].message.content);
  console.log("Usage:", thirdAnalysis.usage);
}

analyzeLegalDocument().catch(console.error);
```

#### How Prompt Caching Works with Large Context

In this example, we demonstrate caching large static content like legal documents, research papers, or extensive context that remains constant across multiple queries.

The large legal document in the system message represents static content that benefits significantly from caching. Once cached, subsequent requests with different questions about the same document will reuse the cached computation for the document analysis, processing only the new user questions.

This approach is particularly effective for document analysis, research assistance, or any scenario where you need to ask multiple questions about the same large piece of content.

**For the first request:**

* `prompt_tokens`: Total number of tokens in the system message (including the large legal document) and user message
* `cached_tokens`: 0 (no cache hit on first request)

**For subsequent requests within the cache lifetime:**

* `prompt_tokens`: Total number of tokens in the system message (including the large legal document) and user message
* `cached_tokens`: Number of tokens in the entire cached system message (including the large legal document)

The caching efficiency is particularly high in this scenario since the large document (which may be thousands of tokens) is reused across multiple requests, while only small user queries (typically dozens of tokens) need fresh processing.

```javascript
import Groq from "groq-sdk";

const groq = new Groq();

// Define comprehensive tool set
const tools = [
  {
    type: "function",
    function: {
      name: "get_weather",
      description: "Get the current weather in a given location",
      parameters: {
        type: "object",
        properties: {
          location: {
            type: "string",
            description: "The city and state, e.g. San Francisco, CA"
          },
          unit: {
            type: "string",
            enum: ["celsius", "fahrenheit"],
            description: "The unit of temperature"
          }
        },
        required: ["location"]
      }
    }
  },
  {
    type: "function",
    function: {
      name: "calculate_math",
      description: "Perform mathematical calculations",
      parameters: {
        type: "object",
        properties: {
          expression: {
            type: "string",
            description: "Mathematical expression to evaluate, e.g. '2 + 2' or 'sqrt(16)'"
          }
        },
        required: ["expression"]
      }
    }
  },
  {
    type: "function",
    function: {
      name: "search_web",
      description: "Search the web for current information",
      parameters: {
        type: "object",
        properties: {
          query: {
            type: "string",
            description: "Search query"
          },
          num_results: {
            type: "integer",
            description: "Number of results to return",
            minimum: 1,
            maximum: 10,
            default: 5
          }
        },
        required: ["query"]
      }
    }
  },
  {
    type: "function",
    function: {
      name: "get_time",
      description: "Get the current time in a specific timezone",
      parameters: {
        type: "object",
        properties: {
          timezone: {
            type: "string",
            description: "Timezone identifier, e.g. 'America/New_York' or 'UTC'"
          }
        },
        required: ["timezone"]
      }
    }
  }
];

async function useToolsWithCaching() {
  // First request - creates cache for all tool definitions
  const systemPrompt = "You are a helpful assistant with access to various tools. Use the appropriate tools to answer user questions accurately.";
  const firstRequest = await groq.chat.completions.create({
    messages: [
      {
        role: "system",
        content: systemPrompt
      },
      {
        role: "user",
        content: "What's the weather like in New York City?"
      }
    ],
    model: "openai/gpt-oss-120b",
    tools: tools
  });

  console.log("First request response:", firstRequest.choices[0].message);
  console.log("Usage:", firstRequest.usage);

  // Check if the model wants to use tools
  if (firstRequest.choices[0].message.tool_calls) {
    console.log("Tool calls requested:", firstRequest.choices[0].message.tool_calls);
  }

  // Second request - tool definitions will be cached
  const secondRequest = await groq.chat.completions.create({
    messages: [
      {
        role: "system",
        content: systemPrompt
      },
      {
        role: "user",
        content: "Can you calculate the square root of 144 and tell me what time it is in Tokyo?"
      }
    ],
    model: "openai/gpt-oss-120b",
    tools: tools
  });

  console.log("Second request response:", secondRequest.choices[0].message);
  console.log("Usage:", secondRequest.usage);

  if (secondRequest.choices[0].message.tool_calls) {
    console.log("Tool calls requested:", secondRequest.choices[0].message.tool_calls);
  }

  // Third request - same tool definitions cached
  const thirdRequest = await groq.chat.completions.create({
    messages: [
      {
        role: "system",
        content: systemPrompt
      },
      {
        role: "user",
        content: "Search for recent news about artificial intelligence developments."
      }
    ],
    model: "openai/gpt-oss-120b",
    tools: tools
  });

  console.log("Third request response:", thirdRequest.choices[0].message);
  console.log("Usage:", thirdRequest.usage);

  if (thirdRequest.choices[0].message.tool_calls) {
    console.log("Tool calls requested:", thirdRequest.choices[0].message.tool_calls);
  }
}

useToolsWithCaching().catch(console.error);
```

#### [How Prompt Caching Works with Tool Definitions](#how-prompt-caching-works-with-tool-definitions)

In this example, we demonstrate caching tool definitions.

All tool definitions, including their schemas, descriptions, and parameters, are cached as a single prefix when they remain consistent across requests. This is particularly valuable when you have a comprehensive set of tools that you want to reuse across multiple requests without re-processing them each time.

The system message and all tool definitions form the static prefix that gets cached, while user queries remain dynamic and are processed fresh for each request.

This approach is useful when you have a consistent set of tools that you want to reuse across multiple requests without re-processing them each time.

**For the first request:**

* `prompt_tokens`: Total number of tokens in the system message, tool definitions, and user message
* `cached_tokens`: 0 (no cache hit on first request)

**For subsequent requests within the cache lifetime:**

* `prompt_tokens`: Total number of tokens in the system message, tool definitions, and user message
* `cached_tokens`: Number of tokens in all cached tool definitions and system prompt

Tool definitions can be quite lengthy due to detailed parameter schemas and descriptions, making caching particularly beneficial for reducing both latency and costs when the same tool set is used repeatedly.

### [Requirements and Limitations](#requirements-and-limitations)

#### [Caching Requirements](#caching-requirements)

* **Exact Prefix Matching**: Cache hits require exact matches of the beginning of your prompt
* **Minimum Prompt Length**: The minimum cacheable prompt length varies by model, ranging from 128 to 1024 tokens depending on the specific model used

To check how much of your prompt was cached, see the response [usage fields](#response-usage-structure).

#### [What Can Be Cached](#what-can-be-cached)

* **Complete message arrays** including system, user, and assistant messages
* **Tool definitions** and function schemas
* **System instructions** and prompt templates
* **One-shot** and **few-shot examples**
* **Structured output schemas**
* **Large static content** like legal documents, research papers, or extensive context that remains constant across multiple queries
* **Image inputs**, including image URLs and base64-encoded images

#### [Limitations](#limitations)

* **Exact Matching**: Even minor changes in cached portions prevent cache hits and force a new cache to be created
* **No Manual Control**: Cache clearing and management is automatic only

### [Tracking Cache Usage](#tracking-cache-usage)

You can monitor how many tokens are being served from cache by examining the `usage` field in your API response. The response includes detailed token usage information, including how many tokens were cached.

#### [Response Usage Structure](#response-usage-structure)

```json
{
  "id": "chatcmpl-...",
  "model": "openai/gpt-oss-120b",
  "usage": {
      "queue_time": 0.026959759,
      "prompt_tokens": 4641,
      "prompt_time": 0.009995497,
      "completion_tokens": 1817,
      "completion_time": 5.57691751,
      "total_tokens": 6458,
      "total_time": 5.586913007,
      "prompt_tokens_details": {
          "cached_tokens": 4608
      }
  },
  // ... other fields
}
```

#### [Understanding the Fields](#understanding-the-fields)

* **`prompt_tokens`**: Total number of tokens in your input prompt
* **`cached_tokens`**: Number of input tokens that were served from cache (within `prompt_tokens_details`)
* **`completion_tokens`**: Number of tokens in the model's response
* **`total_tokens`**: Sum of prompt and completion tokens

In the example above, out of 4641 prompt tokens, 4608 tokens (99.3%) were served from cache, resulting in significant cost savings and improved response time.

#### [Calculating Cache Hit Rate](#calculating-cache-hit-rate)

To calculate your cache hit rate:

`Cache Hit Rate = cached_tokens / prompt_tokens × 100%`

For the example above: `4608 / 4641 × 100% = 99.3%`

A higher cache hit rate indicates better prompt structure optimization leading to lower latency and more cost savings.

### [Troubleshooting](#troubleshooting)

* Verify that sections that you want to cache are identical between requests
* Check that calls are made within the cache lifetime (a few hours). Calls that are too far apart will not benefit from caching.
* Ensure that `tool_choice`, tool usage, and image usage remain consistent between calls
* Validate that you are caching at least the [minimum number of tokens](#caching-requirements) through the [usage fields](#response-usage-structure).

Changes to cached sections, including `tool_choice` and image usage, will invalidate the cache and require a new cache to be created. Subsequent calls will use the new cache.

### [Frequently Asked Questions](#frequently-asked-questions)

#### [How is data privacy maintained with caching?](#how-is-data-privacy-maintained-with-caching)

All cached data exists only in volatile memory and automatically expires within a few hours. No prompt or response content is ever stored in persistent storage or shared between organizations.

#### [Does caching affect the quality or consistency of responses?](#does-caching-affect-the-quality-or-consistency-of-responses)

No. Prompt caching only affects the processing of the input prompt, not the generation of responses. The actual model inference and response generation occur normally, maintaining identical output quality whether caching is used or not.

#### [Can I disable prompt caching?](#can-i-disable-prompt-caching)

Prompt caching is automatically enabled and cannot be manually disabled. This helps customers benefit from reduced costs and latency. Prompts are not stored in persistent storage.

#### [How do I know if my requests are benefiting from caching?](#how-do-i-know-if-my-requests-are-benefiting-from-caching)

You can track cache usage by examining the `usage` field in your API responses. Cache hits are not guaranteed, but Groq tries to maximize them. See the [Tracking Cache Usage](#tracking-cache-usage) section above for detailed information on how to monitor cached tokens and calculate your cache hit rate.

#### [Are there any additional costs for using prompt caching?](#are-there-any-additional-costs-for-using-prompt-caching)

No. Prompt caching is provided at no additional cost and can help to reduce your costs by 50% for cached tokens while improving response times.

#### [Does caching affect rate limits?](#does-caching-affect-rate-limits)

Cached tokens do not count toward your rate limits.

#### [Can I manually clear or refresh caches?](#can-i-manually-clear-or-refresh-caches)

No manual cache management is available. All cache expiration and cleanup happens automatically.

#### [Does the prompt caching discount work with batch requests?](#does-the-prompt-caching-discount-work-with-batch-requests)

Batch requests can still benefit from prompt caching, but the prompt caching discount does not stack with the batch discount. [Batch requests](https://console.groq.com/docs/batch) already receive a 50% discount on all tokens, and while caching functionality remains active, no additional discount is applied to cached tokens in batch requests.
