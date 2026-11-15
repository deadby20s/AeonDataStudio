//! Tokenizers for tokenized export: built-in tiktoken encodings or a Hugging Face `tokenizer.json`.

use aeon_shared::{TokenizerChoice, TokenizerInfo};

use crate::error::{AppError, AppResult};

pub enum Tokenizer {
    Tiktoken { bpe: tiktoken_rs::CoreBPE, info: TokenizerInfo },
    HuggingFace { tok: Box<tokenizers::Tokenizer>, info: TokenizerInfo },
}

/// Special tokens tried, in order, as the end-of-text token of a Hugging Face tokenizer.
const HF_EOS_CANDIDATES: [&str; 6] = ["<|endoftext|>", "</s>", "<|end_of_text|>", "<eos>", "<|eot_id|>", "[SEP]"];

fn dtype_for(vocab_size: u64) -> &'static str {
    if vocab_size <= 1 << 16 { "uint16" } else { "uint32" }
}

impl Tokenizer {
    pub fn load(choice: &TokenizerChoice) -> AppResult<Self> {
        match choice {
            TokenizerChoice::Tiktoken(name) => {
                // (vocab size, id of <|endoftext|>)
                let (bpe, vocab, eos) = match name.as_str() {
                    "o200k_base" => (tiktoken_rs::o200k_base(), 200_019u64, 199_999u32),
                    "cl100k_base" => (tiktoken_rs::cl100k_base(), 100_277, 100_257),
                    "p50k_base" => (tiktoken_rs::p50k_base(), 50_281, 50_256),
                    "r50k_base" => (tiktoken_rs::r50k_base(), 50_257, 50_256),
                    other => return Err(AppError::InvalidInput(format!("Unknown tiktoken encoding \"{other}\""))),
                };
                let bpe = bpe.map_err(|e| AppError::Internal(format!("Could not load {name}: {e}")))?;
                let info = TokenizerInfo {
                    name: name.clone(),
                    vocab_size: vocab,
                    eos_id: Some(eos),
                    dtype: dtype_for(vocab).to_string(),
                };
                Ok(Tokenizer::Tiktoken { bpe, info })
            }
            TokenizerChoice::HuggingFace(path) => {
                let tok = tokenizers::Tokenizer::from_file(path)
                    .map_err(|e| AppError::InvalidInput(format!("Could not load tokenizer {path}: {e}")))?;
                let vocab = tok.get_vocab_size(true) as u64;
                let eos_id = HF_EOS_CANDIDATES.iter().find_map(|t| tok.token_to_id(t));
                let name = std::path::Path::new(path)
                    .parent()
                    .and_then(|p| p.file_name())
                    .map(|n| format!("{} (tokenizer.json)", n.to_string_lossy()))
                    .unwrap_or_else(|| path.clone());
                let info = TokenizerInfo { name, vocab_size: vocab, eos_id, dtype: dtype_for(vocab).to_string() };
                Ok(Tokenizer::HuggingFace { tok: Box::new(tok), info })
            }
        }
    }

    pub fn info(&self) -> &TokenizerInfo {
        match self {
            Tokenizer::Tiktoken { info, .. } | Tokenizer::HuggingFace { info, .. } => info,
        }
    }

    /// Token ids of `text`, without special tokens.
    pub fn encode(&self, text: &str) -> AppResult<Vec<u32>> {
        match self {
            Tokenizer::Tiktoken { bpe, .. } => Ok(bpe.encode_ordinary(text)),
            Tokenizer::HuggingFace { tok, .. } => tok
                .encode(text, false)
                .map(|e| e.get_ids().to_vec())
                .map_err(|e| AppError::Internal(format!("Tokenizing failed: {e}"))),
        }
    }

    #[cfg(test)]
    pub fn decode(&self, ids: &[u32]) -> String {
        match self {
            Tokenizer::Tiktoken { bpe, .. } => bpe.decode(ids).unwrap_or_default(),
            Tokenizer::HuggingFace { tok, .. } => tok.decode(ids, false).unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiktoken_round_trips_and_reports_metadata() {
        let t = Tokenizer::load(&TokenizerChoice::Tiktoken("r50k_base".into())).unwrap();
        let ids = t.encode("Hello world, café!").unwrap();
        assert!(!ids.is_empty());
        assert_eq!(t.decode(&ids), "Hello world, café!");
        assert_eq!(t.info().dtype, "uint16");
        assert_eq!(t.info().eos_id, Some(50_256));
        let big = Tokenizer::load(&TokenizerChoice::Tiktoken("cl100k_base".into())).unwrap();
        assert_eq!(big.info().dtype, "uint32");
        // "<|endoftext|>" in text is encoded as ordinary text, never as the special token.
        assert!(!big.encode("<|endoftext|>").unwrap().contains(&100_257));
    }

    #[test]
    fn unknown_inputs_are_rejected() {
        assert!(Tokenizer::load(&TokenizerChoice::Tiktoken("nope".into())).is_err());
        assert!(Tokenizer::load(&TokenizerChoice::HuggingFace("C:/does/not/exist.json".into())).is_err());
    }
}
