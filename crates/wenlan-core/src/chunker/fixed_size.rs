// SPDX-License-Identifier: Apache-2.0
use super::traits::{ChunkContext, ChunkInfo, ChunkingStrategy};
use super::MAX_CONTENT_TOKENS;
use text_splitter::{ChunkConfig, ChunkSizer, TextSplitter};
use tokenizers::Tokenizer;

/// Plain-text chunking strategy backed by `text-splitter`'s [`TextSplitter`],
/// which splits along Unicode semantic boundaries (paragraph → sentence → word
/// → grapheme) and never mid-char.
///
/// Built via [`FixedSizeStrategy::with_tokenizer`] chunks are sized by BGE
/// token count (max [`MAX_CONTENT_TOKENS`]) so no chunk is silently truncated
/// at embed time. [`FixedSizeStrategy::new`] falls back to a character cap when
/// no tokenizer is available.
enum Splitter {
    Chars(TextSplitter<text_splitter::Characters>),
    // Boxed: a `Tokenizer` carries its vocab maps, dwarfing the char variant.
    Tokens(Box<TextSplitter<Tokenizer>>),
}

/// Keep the token splitter's semantic-level search bounded for long inputs.
/// The token budget remains independent of this character window size.
const TOKEN_WINDOW_CHARS: usize = 8192;

fn token_chunk_indices<'a, S: ChunkSizer>(
    splitter: &TextSplitter<S>,
    content: &'a str,
) -> Vec<(usize, &'a str)> {
    let window_splitter = TextSplitter::<text_splitter::Characters>::new(
        ChunkConfig::new(TOKEN_WINDOW_CHARS).with_trim(false),
    );
    let mut indices = Vec::new();
    for (window_offset, window) in window_splitter.chunk_indices(content) {
        indices.extend(
            splitter
                .chunk_indices(window)
                .map(|(offset, chunk)| (window_offset + offset, chunk)),
        );
    }
    indices
}

pub struct FixedSizeStrategy {
    splitter: Splitter,
}

impl FixedSizeStrategy {
    /// Character-based fallback cap (no tokenizer available). Keeps the
    /// historical ~512-char chunk size.
    const MAX_CHARS: usize = 512;

    pub fn new() -> Self {
        let config = ChunkConfig::new(Self::MAX_CHARS);
        Self {
            splitter: Splitter::Chars(TextSplitter::new(config)),
        }
    }

    /// Token-aware constructor: chunks are sized so each encodes to at most
    /// [`MAX_CONTENT_TOKENS`] BGE tokens.
    pub fn with_tokenizer(tokenizer: Tokenizer) -> Self {
        let config =
            ChunkConfig::new((MAX_CONTENT_TOKENS / 2)..=MAX_CONTENT_TOKENS).with_sizer(tokenizer);
        Self {
            splitter: Splitter::Tokens(Box::new(TextSplitter::new(config))),
        }
    }
}

impl Default for FixedSizeStrategy {
    fn default() -> Self {
        Self::new()
    }
}

impl ChunkingStrategy for FixedSizeStrategy {
    fn chunk(&self, context: ChunkContext) -> Vec<ChunkInfo> {
        if context.content.trim().is_empty() {
            return vec![];
        }

        // The two splitter flavours return different `impl Iterator` types, so
        // collect the (offset, text) pairs inside the match before mapping.
        let indices: Vec<(usize, &str)> = match &self.splitter {
            Splitter::Chars(s) => s.chunk_indices(context.content).collect(),
            Splitter::Tokens(s) => token_chunk_indices(s, context.content),
        };

        indices
            .into_iter()
            .map(|(byte_offset, chunk_text)| ChunkInfo {
                content: chunk_text.to_string(),
                chunk_type: "text".to_string(),
                language: None,
                byte_range: Some((byte_offset, byte_offset + chunk_text.len())),
                semantic_unit: Some("paragraph".to_string()),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, collections::HashMap};

    fn make_context<'a>(
        content: &'a str,
        title: &'a str,
        metadata: &'a HashMap<String, String>,
    ) -> ChunkContext<'a> {
        ChunkContext {
            content,
            title,
            file_path: "test.txt",
            metadata,
        }
    }

    #[test]
    fn test_empty_text() {
        let strategy = FixedSizeStrategy::new();
        let metadata = HashMap::new();
        let chunks = strategy.chunk(make_context("", "", &metadata));
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_short_text() {
        let strategy = FixedSizeStrategy::new();
        let metadata = HashMap::new();
        let chunks = strategy.chunk(make_context("Hello, world!", "", &metadata));
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].content, "Hello, world!");
        assert_eq!(chunks[0].chunk_type, "text");
    }

    #[test]
    fn test_title_not_prefixed() {
        let strategy = FixedSizeStrategy::new();
        let metadata = HashMap::new();
        let chunks = strategy.chunk(make_context("Some content here", "README.md", &metadata));
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].content, "Some content here");
    }

    #[test]
    fn test_long_text_chunking() {
        let strategy = FixedSizeStrategy::new();
        let metadata = HashMap::new();
        let text = "word ".repeat(200); // ~1000 chars
        let chunks = strategy.chunk(make_context(&text, "", &metadata));
        assert!(chunks.len() > 1);

        // Each chunk should be roughly 512 chars or less
        for chunk in &chunks {
            assert!(
                chunk.content.len() <= 600,
                "Chunk too long: {} chars",
                chunk.content.len()
            );
        }

        // Check byte ranges
        for chunk in &chunks {
            assert!(chunk.byte_range.is_some());
        }
    }

    #[test]
    fn test_paragraph_split() {
        let strategy = FixedSizeStrategy::new();
        let metadata = HashMap::new();
        let para1 = "a".repeat(400);
        let para2 = "b".repeat(200);
        let text = format!("{}\n\n{}", para1, para2);
        let chunks = strategy.chunk(make_context(&text, "", &metadata));
        assert!(!chunks.is_empty());
    }

    #[test]
    fn test_semantic_unit() {
        let strategy = FixedSizeStrategy::new();
        let metadata = HashMap::new();
        let chunks = strategy.chunk(make_context("Test content", "", &metadata));
        assert_eq!(chunks[0].semantic_unit, Some("paragraph".to_string()));
    }

    /// Load the real BGE tokenizer from the shared FastEmbed cache (same
    /// contract as the db tests: skips if the cache is absent).
    fn bge_tokenizer() -> Option<tokenizers::Tokenizer> {
        let cache = crate::db::resolve_fastembed_cache_dir(std::path::Path::new(".nonexistent"))?;
        crate::chunker::load_bge_tokenizer(&cache)
    }

    /// A prose document of roughly 8k tokens.
    fn long_prose() -> String {
        "This is a sentence of prose that describes something meaningful and then keeps going. "
            .repeat(600)
    }

    /// A dense, token-heavy document with no whitespace break points, so each
    /// character maps to roughly one token — a char-capped chunk overshoots the
    /// token budget.
    fn dense_document() -> String {
        "1+2=3;4-5=6;7*8=9;a<b>c|d&e^f~g/h%i.j,k:l;m?n!o@p#q$r".repeat(200)
    }

    fn in_memory_wordpiece_tokenizer() -> Tokenizer {
        use tokenizers::{models::wordpiece::WordPiece, pre_tokenizers::bert::BertPreTokenizer};

        let vocab = [
            ("[UNK]".to_string(), 0),
            ("[CLS]".to_string(), 1),
            ("[SEP]".to_string(), 2),
        ];
        let model = WordPiece::builder()
            .vocab(vocab)
            .unk_token("[UNK]".to_string())
            .build()
            .expect("build in-memory WordPiece model");
        let mut tokenizer = Tokenizer::new(model);
        tokenizer.with_pre_tokenizer(Some(BertPreTokenizer));
        tokenizer
    }

    #[test]
    fn long_token_document_keeps_original_utf8_slices_and_token_budget() {
        let tokenizer = in_memory_wordpiece_tokenizer();
        let strategy = FixedSizeStrategy::with_tokenizer(tokenizer.clone());
        let metadata = HashMap::new();
        let text = "  Mixed Unicode 世界 café 🙂 words\nwith tabs\tand spaces.  ".repeat(600);
        let chunks = strategy.chunk(make_context(&text, "", &metadata));
        assert!(chunks.len() > 1);

        let mut previous_end = 0;
        for chunk in &chunks {
            let (start, end) = chunk.byte_range.expect("chunk byte range");
            assert!(start >= previous_end, "chunk ranges must be monotonic");
            assert!(
                start <= end && end <= text.len(),
                "range must be in document bounds"
            );
            assert!(text.is_char_boundary(start) && text.is_char_boundary(end));
            assert_eq!(chunk.content, text[start..end]);
            assert!(text[previous_end..start].chars().all(char::is_whitespace));

            let token_count = tokenizer
                .encode_fast(chunk.content.as_str(), false)
                .expect("encode chunk")
                .get_ids()
                .len();
            assert!(token_count <= MAX_CONTENT_TOKENS, "{token_count} tokens");
            previous_end = end;
        }
        assert!(text[previous_end..].chars().all(char::is_whitespace));
        assert!(chunks.iter().any(|chunk| chunk.content.contains('世')));
        assert!(chunks.iter().any(|chunk| chunk.content.contains('🙂')));
    }

    #[test]
    fn token_sizing_work_is_bounded_by_semantic_windows() {
        struct CountingSizer {
            calls: Cell<usize>,
            bytes_sized: Cell<usize>,
            largest_input_chars: Cell<usize>,
        }
        impl ChunkSizer for CountingSizer {
            fn size(&self, text: &str) -> usize {
                self.calls.set(self.calls.get() + 1);
                self.bytes_sized.set(self.bytes_sized.get() + text.len());
                self.largest_input_chars
                    .set(self.largest_input_chars.get().max(text.chars().count()));
                text.split_whitespace().count()
            }
        }

        let sizer = CountingSizer {
            calls: Cell::new(0),
            bytes_sized: Cell::new(0),
            largest_input_chars: Cell::new(0),
        };
        let splitter = TextSplitter::new(ChunkConfig::new(32).with_sizer(&sizer));
        let text = "bounded sizing work across many semantic windows ".repeat(4000);
        let indices = token_chunk_indices(&splitter, &text);

        assert!(!indices.is_empty());
        assert!(sizer.calls.get() > 0);
        assert!(sizer.largest_input_chars.get() <= TOKEN_WINDOW_CHARS);
        assert!(
            sizer.bytes_sized.get() <= text.len() * 256,
            "sized {} bytes for {} input bytes",
            sizer.bytes_sized.get(),
            text.len()
        );
    }

    /// Property: every chunk from the text strategy stays within the BGE token
    /// budget once the strategy is tokenizer-aware.
    #[test]
    fn text_chunks_within_token_budget() {
        let Some(tokenizer) = bge_tokenizer() else {
            eprintln!("SKIP text_chunks_within_token_budget: BGE tokenizer not in cache");
            return;
        };
        let strategy = FixedSizeStrategy::with_tokenizer(tokenizer.clone());
        let metadata = HashMap::new();
        let prose = long_prose();
        let dense = dense_document();
        for doc in [prose.as_str(), dense.as_str()] {
            let chunks = strategy.chunk(make_context(doc, "", &metadata));
            assert!(!chunks.is_empty(), "expected at least one chunk");
            for chunk in &chunks {
                let n_tokens = tokenizer
                    .encode_fast(chunk.content.as_str(), false)
                    .expect("encode chunk")
                    .get_ids()
                    .len();
                assert!(
                    n_tokens <= crate::chunker::MAX_CONTENT_TOKENS,
                    "chunk encodes to {n_tokens} tokens (> {}); first 80 chars: {:?}",
                    crate::chunker::MAX_CONTENT_TOKENS,
                    chunk.content.chars().take(80).collect::<String>()
                );
            }
        }
    }
}
