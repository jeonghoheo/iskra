use std::time::{SystemTime, UNIX_EPOCH};

use crate::logging;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DailyUsage {
    pub prompt: u64,
    pub completion: u64,
    pub total: u64,
}

const PEAK_PRICES: [(&str, f64, f64); 5] = [
    ("deepseek-chat", 0.30, 1.20),
    ("deepseek-flash", 0.30, 1.20),
    ("deepseek-v4-flash", 0.30, 1.20),
    ("deepseek-reasoner", 1.32, 3.96),
    ("deepseek-v4-pro", 1.32, 3.96),
];

pub fn today() -> DailyUsage {
    let Ok(log) = std::fs::read_to_string(logging::LOG_PATH) else {
        return DailyUsage::default();
    };
    let since = local_midnight_secs().unwrap_or_else(utc_midnight_secs);
    sum_usage_since(&log, since)
}

fn sum_usage_since(log: &str, since: i64) -> DailyUsage {
    let mut usage = DailyUsage::default();
    for line in log.lines() {
        let Some(ts) = line
            .strip_prefix("ts=")
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|value| value.parse::<i64>().ok())
        else {
            continue;
        };
        if ts < since || !line.contains(" model_call ") {
            continue;
        }
        let prompt = extract_value(line, "prompt=").unwrap_or(0);
        let completion = extract_value(line, "completion=").unwrap_or(0);
        let total = extract_value(line, "total=").unwrap_or(prompt + completion);
        usage.prompt += prompt;
        usage.completion += completion;
        usage.total += total;
    }
    usage
}

fn extract_value(line: &str, key: &str) -> Option<u64> {
    let (_, tail) = line.split_once(key)?;
    let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

pub fn self_state_block(model: &str, base_url: &str, usage: &DailyUsage, balance: &str) -> String {
    let mut block = String::from("# Self-state\n");
    block.push_str(&format!("- Provider: {}\n", provider_label(base_url)));
    block.push_str(&format!("- Model: {model}\n"));
    block.push_str(&format!(
        "- Tokens today: {} (input {}, output {})\n",
        usage.total, usage.prompt, usage.completion
    ));
    match estimate_cost(model, usage) {
        Some(cost) => block.push_str(&format!(
            "- Estimated cost today: ~${cost:.6} (estimate; DeepSeek public list peak prices)\n"
        )),
        None => block.push_str("- Estimated cost today: unknown (model not in price table)\n"),
    }
    block.push_str(&format!("- Balance: {balance}\n"));
    block.push_str("- Token and cost figures are estimates and may differ from actual billing.\n");
    block
}

pub fn estimate_cost(model: &str, usage: &DailyUsage) -> Option<f64> {
    let (_, input, output) = PEAK_PRICES.iter().find(|(name, _, _)| *name == model)?;
    Some(usage.prompt as f64 / 1_000_000.0 * input + usage.completion as f64 / 1_000_000.0 * output)
}

fn provider_label(base_url: &str) -> String {
    let rest = base_url
        .strip_prefix("https://")
        .or_else(|| base_url.strip_prefix("http://"));
    let Some(rest) = rest else {
        return "unknown".to_string();
    };
    let host = rest.split('/').next().unwrap_or(rest);
    if host.contains("deepseek") {
        "DeepSeek".to_string()
    } else {
        host.to_string()
    }
}

#[cfg(unix)]
fn local_midnight_secs() -> Option<i64> {
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        if now == -1 {
            return None;
        }
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return None;
        }
        tm.tm_hour = 0;
        tm.tm_min = 0;
        tm.tm_sec = 0;
        tm.tm_isdst = -1;
        let midnight = libc::mktime(&mut tm);
        if midnight == -1 {
            None
        } else {
            Some(midnight as i64)
        }
    }
}

#[cfg(not(unix))]
fn local_midnight_secs() -> Option<i64> {
    None
}

fn utc_midnight_secs() -> i64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    now - now.rem_euclid(86_400)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_usage_since_midnight() {
        let log = "\
ts=100 model_call tools=[none] usage=prompt=10 completion=2 total=12
ts=200 model_call tools=[read_file] usage=prompt=20 completion=3 total=23
ts=50 model_call tools=[none] usage=prompt=99 completion=1 total=100
ts=300 turn_end answer
ts=400 model_error err=timeout
";
        assert_eq!(
            sum_usage_since(log, 100),
            DailyUsage {
                prompt: 30,
                completion: 5,
                total: 35,
            }
        );
    }

    #[test]
    fn assembles_self_state_with_cost_and_balance() {
        let usage = DailyUsage {
            prompt: 1_000_000,
            completion: 1_000_000,
            total: 2_000_000,
        };
        let block = self_state_block(
            "deepseek-chat",
            "https://api.deepseek.com/chat/completions",
            &usage,
            "110.00 CNY",
        );
        assert!(block.contains("Provider: DeepSeek"));
        assert!(block.contains("Model: deepseek-chat"));
        assert!(block.contains("2000000"));
        assert!(block.contains("1.500000"));
        assert!(block.contains("110.00 CNY"));
    }

    #[test]
    fn omits_cost_for_unknown_model() {
        let usage = DailyUsage {
            prompt: 1_000,
            completion: 500,
            total: 1_500,
        };
        let block = self_state_block(
            "unknown-model",
            "https://example.com/v1",
            &usage,
            "unavailable",
        );
        assert!(block.contains("unknown-model"));
        assert!(block.contains("1500"));
        assert!(block.contains("unknown"));
        assert!(!block.contains("~$"));
    }

    #[test]
    fn estimates_known_model_cost() {
        let usage = DailyUsage {
            prompt: 1_000_000,
            completion: 1_000_000,
            total: 2_000_000,
        };
        assert_eq!(estimate_cost("deepseek-chat", &usage), Some(1.5));
        assert_eq!(estimate_cost("deepseek-v4-pro", &usage), Some(5.28));
        assert_eq!(estimate_cost("unknown-model", &usage), None);
    }

    #[test]
    fn labels_provider_from_base_url() {
        assert_eq!(
            provider_label("https://api.deepseek.com/chat/completions"),
            "DeepSeek"
        );
        assert_eq!(provider_label("https://example.com/v1"), "example.com");
        assert_eq!(provider_label("not-a-url"), "unknown");
    }
}
