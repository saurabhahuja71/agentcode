/// Model pricing table: (input_per_1m, output_per_1m) in USD.
///
/// Extend by adding an arm to `model_price` or a prefix to `PREFIX_PRICES`.
pub fn price_per_1m(model: &str) -> Option<(f64, f64)> {
    if let Some(p) = model_price(model) {
        return Some(p);
    }
    for (prefix, price) in PREFIX_PRICES {
        if model.starts_with(prefix) {
            return Some(*price);
        }
    }
    None
}

/// Compute cost in USD from token usage. Returns None when pricing unknown.
pub fn cost_for(model: &str, input_tokens: u64, output_tokens: u64) -> Option<f64> {
    let (input_price, output_price) = price_per_1m(model)?;
    Some((input_tokens as f64 * input_price + output_tokens as f64 * output_price) / 1_000_000.0)
}

fn model_price(model: &str) -> Option<(f64, f64)> {
    match model {
        "deepseek-chat" | "deepseek-v3" => Some((0.27, 1.10)),
        "deepseek-r1" | "deepseek-reasoner" => Some((0.55, 2.19)),
        "qwen2.5-coder-32b-instruct" => Some((1.20, 1.20)),
        "qwen2.5-coder-7b-instruct" => Some((0.90, 0.90)),
        "llama-3.3-70b-versatile" => Some((0.20, 0.20)),
        "gpt-4o-mini" => Some((0.15, 0.60)),
        "gpt-4o" => Some((2.50, 10.00)),
        "claude-3-5-sonnet" | "claude-3-5-sonnet-20241022" => Some((3.00, 15.00)),
        "claude-3-7-sonnet" => Some((3.00, 15.00)),
        "grok-2" | "grok-2-1212" => Some((2.00, 10.00)),
        _ => None,
    }
}

/// Coarse fallbacks by family prefix (cheapest tier) so partial/renamed
/// model ids still get a cost estimate.
const PREFIX_PRICES: &[(&str, (f64, f64))] = &[
    ("deepseek-chat", (0.27, 1.10)),
    ("deepseek-reasoner", (0.55, 2.19)),
    ("gpt-4o-mini", (0.15, 0.60)),
    ("gpt-4o", (2.50, 10.00)),
    ("claude-3-5-sonnet", (3.00, 15.00)),
    ("claude-3-7-sonnet", (3.00, 15.00)),
    ("grok-2", (2.00, 10.00)),
    ("llama-3.3-70b", (0.20, 0.20)),
    ("qwen2.5-coder-32b", (1.20, 1.20)),
];
