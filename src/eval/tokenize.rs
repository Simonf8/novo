//! Tokenizers in the Hugging Face `tokenizer.json` format, so a model's own
//! tokenizer can be used as published, plus byte-level BPE training.

use super::value::Value;
use std::path::Path;
use std::sync::Arc;
use tokenizers::models::bpe::{BPE, BpeTrainer};
use tokenizers::models::{ModelWrapper, TrainerWrapper};
use tokenizers::pre_tokenizers::byte_level::ByteLevel;
use tokenizers::{AddedToken, Tokenizer};

type R<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Loads `tokenizer.json`, or the one inside a model directory.
pub fn load(path: &str) -> R<Value> {
    let p = Path::new(path);
    let file = if p.is_dir() {
        p.join("tokenizer.json")
    } else {
        p.to_path_buf()
    };
    let tok = Tokenizer::from_file(&file)
        .map_err(|e| format!("cannot load tokenizer `{}`: {e}", file.display()))?;
    Ok(Value::Tokenizer(Arc::new(tok)))
}

/// Trains a byte-level BPE tokenizer, as GPT-2 and Llama 3 use, on `texts`.
pub fn train(texts: &[String], vocab_size: usize, special: &[String]) -> R<Value> {
    let mut trainer: TrainerWrapper = BpeTrainer::builder()
        .vocab_size(vocab_size)
        .min_frequency(2)
        .show_progress(false)
        .special_tokens(
            special
                .iter()
                .map(|s| AddedToken::from(s.clone(), true))
                .collect(),
        )
        .initial_alphabet(ByteLevel::alphabet().into_iter().collect())
        .build()
        .into();
    let mut tok = Tokenizer::new(ModelWrapper::BPE(BPE::default()));
    tok.with_pre_tokenizer(Some(ByteLevel::new(false, true, true)));
    tok.with_decoder(Some(ByteLevel::default()));
    tok.train(&mut trainer, texts.iter()).map_err(err)?;
    let special: Vec<AddedToken> = special
        .iter()
        .map(|s| AddedToken::from(s.clone(), true))
        .collect();
    tok.add_special_tokens(special).map_err(err)?;
    Ok(Value::Tokenizer(Arc::new(tok)))
}

fn ints(v: &Value) -> R<Vec<u32>> {
    let Value::List(items) = v else {
        return Err("expected a list of token ids".into());
    };
    items
        .read()
        .iter()
        .map(|x| match x {
            Value::Int(i) if (0..=u32::MAX as i64).contains(i) => Ok(*i as u32),
            other => Err(format!("a {} is not a token id", other.type_name())),
        })
        .collect()
}

pub fn method(tok: &Tokenizer, name: &str, args: &[Value]) -> R<Value> {
    let flag = |i: usize, default: bool| match args.get(i) {
        Some(Value::Bool(b)) => *b,
        _ => default,
    };
    let text = |i: usize| match args.get(i) {
        Some(Value::Str(s)) => Ok(s.to_string()),
        _ => Err("expected a String".to_string()),
    };
    Ok(match name {
        "encode" => {
            let enc = tok.encode(text(0)?, flag(1, true)).map_err(err)?;
            Value::list(
                enc.get_ids()
                    .iter()
                    .map(|&i| Value::Int(i64::from(i)))
                    .collect(),
            )
        }
        "decode" => Value::Str(
            tok.decode(&ints(&args[0])?, flag(1, true))
                .map_err(err)?
                .into(),
        ),
        "vocab_size" => Value::Int(tok.get_vocab_size(true) as i64),
        "id" => Value::Int(tok.token_to_id(&text(0)?).map_or(-1, i64::from)),
        "token" => match &args[0] {
            Value::Int(i) => match u32::try_from(*i).ok().and_then(|i| tok.id_to_token(i)) {
                Some(t) => Value::Str(t.into()),
                None => return Err(format!("{i} is not a token id of this tokenizer")),
            },
            _ => return Err("expected an Int".into()),
        },
        "save" => {
            tok.save(text(0)?, false).map_err(err)?;
            Value::None
        }
        _ => return Err(format!("Tokenizer has no method `{name}`")),
    })
}
