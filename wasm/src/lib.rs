// WASM bindings for jutsu template engine

use jutsu_core::lexer::{Config, LexError, Tok};
use jutsu_core::parser::{Av, PToken};
use jutsu_core::eval;

/// Tokenize a template source with the rust lexer.
/// Returns a list of `(lineno, token_type, value)` tuples.
#[wasm_bindgen]
pub fn wasm_tokenize(source: &str, config: &Config) -> Result<Vec<Tok>, LexError> {
    jutsu_core::pure_api::tokenize(config, source, None)
}

/// Parse a template source into an AST
#[wasm_bindgen]
pub fn wasm_parse(tokens: Vec<PToken>) -> Result<Av, String> {
    jutsu_core::pure_api::parse(tokens).map_err(|e| e.message)
}

/// Evaluate a parsed AST
#[wasm_bindgen]
pub fn wasm_evaluate(av: &Av) -> Result<String, String> {
    jutsu_core::pure_api::evaluate(av).map_err(|e| e)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_wasm_tokenize() {
        let config = Config::default();
        let result = wasm_tokenize("{{ name }}", &config);
        assert!(result.is_ok());
    }
}
