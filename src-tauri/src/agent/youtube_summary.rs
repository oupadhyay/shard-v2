//! Long-transcript summarization helpers used by the `youtube_transcript`
//! tool. Splits an oversized transcript into UTF-8-safe chunks and asks the
//! background LLM to produce per-chunk summaries followed by a merged
//! coherent summary.

use super::Agent;

/// Conservatively ~20K tokens per chunk, leaving headroom for system prompt
/// and response within the 128K-token context of the background models.
const CHUNK_SIZE: usize = 80_000;

/// Split a transcript into chunks of approximately `max_chars` Unicode
/// characters each, splitting on newline boundaries when possible.
///
/// Guarantees:
/// - never panics on multi-byte UTF-8 input
/// - input fitting in one chunk returns exactly one chunk equal to the input
/// - concatenating all chunks reproduces the input exactly
/// - each chunk except possibly the last is `<= max_chars` characters
pub(crate) fn split_transcript_chunks(text: &str, max_chars: usize) -> Vec<&str> {
    let total_chars = text.chars().count();
    if total_chars <= max_chars {
        return vec![text];
    }

    // Precompute byte offsets for each character boundary, with a sentinel at end.
    let char_offsets: Vec<usize> = text
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(text.len()))
        .collect();

    let mut chunks = Vec::new();
    let mut start_char = 0;

    while start_char < total_chars {
        let remaining_chars = total_chars - start_char;
        if remaining_chars <= max_chars {
            let byte_start = char_offsets[start_char];
            chunks.push(&text[byte_start..]);
            break;
        }

        let end_char = start_char + max_chars;
        let byte_start = char_offsets[start_char];
        let byte_end = char_offsets[end_char];

        // Try to split on the last newline before the character boundary.
        let chunk_slice = &text[byte_start..byte_end];
        let split_byte = if let Some(nl_pos) = chunk_slice.rfind('\n') {
            byte_start + nl_pos + '\n'.len_utf8()
        } else {
            byte_end
        };

        chunks.push(&text[byte_start..split_byte]);

        // Find the char index corresponding to split_byte.
        let next_start_char = char_offsets[start_char..]
            .iter()
            .position(|&offset| offset >= split_byte)
            .map(|pos| start_char + pos)
            .unwrap_or(total_chars);
        start_char = next_start_char;
    }

    chunks
}

impl<R: tauri::Runtime> Agent<R> {
    /// Summarize a long YouTube transcript using the background LLM.
    ///
    /// For transcripts that exceed the background model's context window,
    /// splits the transcript into chunks, summarizes each independently,
    /// then produces a final combined summary. This ensures no information
    /// is lost regardless of transcript length.
    pub(crate) async fn summarize_long_transcript(
        &self,
        config: &crate::config::AppConfig,
        full_transcript: &str,
        title: &str,
    ) -> Result<String, String> {
        let model = config
            .background_model
            .as_deref()
            .unwrap_or("gpt-oss-120b (Groq)");

        log::info!(
            "[YouTube] Summarizing long transcript ({} chars) for \"{}\" via {}",
            full_transcript.chars().count(),
            title,
            model
        );

        let chunks = split_transcript_chunks(full_transcript, CHUNK_SIZE);

        if chunks.len() == 1 {
            let system_prompt = "You are a precise summarization assistant. Given a full YouTube video transcript, produce a comprehensive summary that captures ALL key points, arguments, examples, and conclusions. Organize the summary with clear sections. Do not omit any important topics or details — the user will only see the first portion of the timestamped transcript plus your summary for the rest.";
            let user_message = format!(
                "Summarize the following YouTube video transcript comprehensively. The video is titled \"{}\".\n\n---\n{}",
                title, full_transcript
            );
            return crate::background::call_llm_oneshot(
                &self.http_client,
                config,
                model,
                system_prompt,
                &user_message,
                4000,
                0.3,
            )
            .await;
        }

        log::info!(
            "[YouTube] Transcript split into {} chunks for summarization",
            chunks.len()
        );

        let chunk_system = "You are a precise summarization assistant. You will receive one section of a YouTube video transcript. Produce a detailed summary of THIS section only, capturing all key points, arguments, examples, data, and conclusions. Be thorough — your output will be combined with summaries of other sections.";

        let mut chunk_summaries = Vec::with_capacity(chunks.len());
        for (i, chunk) in chunks.iter().enumerate() {
            let user_message = format!(
                "Summarize section {} of {} from the YouTube video \"{}\". Capture every important detail.\n\n---\n{}",
                i + 1,
                chunks.len(),
                title,
                chunk
            );

            let summary = crate::background::call_llm_oneshot(
                &self.http_client,
                config,
                model,
                chunk_system,
                &user_message,
                3000,
                0.3,
            )
            .await?;

            log::info!(
                "[YouTube] Chunk {}/{} summarized ({} chars)",
                i + 1,
                chunks.len(),
                summary.len()
            );
            chunk_summaries.push(format!(
                "## Section {} of {}\n{}",
                i + 1,
                chunks.len(),
                summary
            ));
        }

        // Final pass: combine chunk summaries into one coherent summary.
        let combined = chunk_summaries.join("\n\n");
        let merge_system = "You are a precise summarization assistant. You will receive multiple section summaries from a single YouTube video. Merge them into ONE coherent, comprehensive summary. Preserve all important details, eliminate redundancy, and organize with clear sections. The user will rely on this as a complete representation of the video's content.";
        let merge_message = format!(
            "Merge the following section summaries from the YouTube video \"{}\" into a single comprehensive summary.\n\n---\n{}",
            title, combined
        );

        crate::background::call_llm_oneshot(
            &self.http_client,
            config,
            model,
            merge_system,
            &merge_message,
            4000,
            0.3,
        )
        .await
    }
}
