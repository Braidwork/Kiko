use crate::result::{Error, Result};

pub struct TokenOutputStream {
    tokenizer: tokenizers::Tokenizer,
    tokens: Vec<u32>,
    prev_index: usize,
    current_index: usize,
}

impl TokenOutputStream {
    pub fn new(tokenizer: tokenizers::Tokenizer) -> Self {
        Self {
            tokenizer,
            tokens: Vec::new(),
            prev_index: 0,
            current_index: 0,
        }
    }

    pub fn into_inner(self) -> tokenizers::Tokenizer {
        self.tokenizer
    }

    fn decode(&self, tokens: &[u32]) -> Result<String> {
        match self.tokenizer.decode(tokens, true) {
            Ok(str) => Ok(str),
            Err(err) => Err(Error::WithMessage(format!("cannot decode: {err}"))),
        }
    }

    // https://github.com/huggingface/text-generation-inference/blob/5ba53d44a18983a4de32d122f4cb46f4a17d9ef6/server/text_generation_server/models/model.py#L68
    pub fn next_token(&mut self, token: u32) -> Result<Option<String>> {
        let prev_text = if self.tokens.is_empty() {
            String::new()
        } else {
            let tokens = &self.tokens[self.prev_index..self.current_index];
            self.decode(tokens)?
        };
        self.tokens.push(token);
        let text = self.decode(&self.tokens[self.prev_index..])?;
        if text.len() > prev_text.len() && text.chars().last().unwrap().is_alphanumeric() {
            let text = text.split_at(prev_text.len());
            self.prev_index = self.current_index;
            self.current_index = self.tokens.len();
            Ok(Some(text.1.to_string()))
        } else {
            Ok(None)
        }
    }

    pub fn decode_rest(&self) -> Result<Option<String>> {
        let prev_text = if self.tokens.is_empty() {
            String::new()
        } else {
            let tokens = &self.tokens[self.prev_index..self.current_index];
            self.decode(tokens)?
        };
        let text = self.decode(&self.tokens[self.prev_index..])?;
        if text.len() > prev_text.len() {
            let text = text.split_at(prev_text.len());
            Ok(Some(text.1.to_string()))
        } else {
            Ok(None)
        }
    }

    pub fn decode_all(&self) -> Result<String> {
        self.decode(&self.tokens)
    }

    pub fn get_token(&self, token_s: &str) -> Option<u32> {
        self.tokenizer.get_vocab(true).get(token_s).copied()
    }

    pub fn tokenizer(&self) -> &tokenizers::Tokenizer {
        &self.tokenizer
    }

    pub fn clear(&mut self) {
        self.tokens.clear();
        self.prev_index = 0;
        self.current_index = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 记录一个实际踩过的坑：`skip_special_tokens=true` **不会**滤掉 `<think>` /
    /// `</think>`（151667 / 151668）—— 它们会被当成普通文本吐给读者。
    ///
    /// 所以"关掉思考"不能靠事后过滤，只能靠提示词（见 `huggingface.rs` 里
    /// Qwen3 分支的说明）。这条测试用真实分词器把这个事实钉住。
    ///
    /// 模型文件不在时跳过（CI 上通常没有）。
    #[test]
    fn the_think_markers_are_not_filtered_by_skip_special_tokens() {
        let path = "data/models/Qwen/Qwen3-0.6B/tokenizer.json";
        if !std::path::Path::new(path).exists() {
            eprintln!("skipping: {path} not present");
            return;
        }
        let tokenizer = tokenizers::Tokenizer::from_file(path).unwrap();
        let mut tos = TokenOutputStream::new(tokenizer);

        // 151668 = `</think>`，后面跟真实回答。
        let mut emitted = String::new();
        for tok in [151668u32, 271, 103942] {
            if let Some(t) = tos.next_token(tok).unwrap() {
                emitted.push_str(&t);
            }
        }
        if let Some(rest) = tos.decode_rest().unwrap() {
            emitted.push_str(&rest);
        }
        assert!(
            emitted.contains("</think>"),
            "if this ever stops holding, the prompt-side workaround could be \
             replaced by filtering: {emitted:?}"
        );
    }
}
