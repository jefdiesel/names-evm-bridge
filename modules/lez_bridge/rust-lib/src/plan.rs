//! Splitting an amount into the vault's denominations, and when to send the pieces.
//!
//! The bridge moves fixed denominations only (amount privacy, DESIGN §3.1). An amount is cut
//! into the fewest pieces, largest first; whatever no denomination fits stays with the person.
//! Pieces are spread over time (timing privacy): the suggestion puts hours between deposits,
//! with a random part so two people following the advice don't move in step, and advises
//! claiming each piece no sooner than a day after it, once more deposits of the same amount
//! have arrived.

use crate::util::{duration_text, format_amount};
use serde_json::{json, Value};

pub const MAX_PIECES: u32 = 24;
/// Between two deposits of one plan: 6 h plus up to 4 h at random.
pub const GAP_S: u64 = 6 * 3600;
pub const JITTER_S: u64 = 4 * 3600;
/// Claim a piece no sooner than this after its deposit.
pub const CLAIM_WAIT_S: u64 = 24 * 3600;
/// Below this many deposits of the same amount, the indicator warns that a claim is easy to match.
pub const SMALL_SET: u64 = 10;

#[derive(Debug, PartialEq)]
pub struct Split {
    /// (denomination index, count), largest denomination first.
    pub pieces: Vec<(usize, u32)>,
    pub remainder: u128,
}

/// The fewest pieces that make up `amount` from `denoms` (any order), largest first.
pub fn split(amount: u128, denoms: &[u128]) -> Result<Split, String> {
    if denoms.is_empty() || denoms.contains(&0) {
        return Err("this token has no denominations".to_owned());
    }
    let mut order: Vec<usize> = (0..denoms.len()).collect();
    order.sort_by_key(|i| std::cmp::Reverse(denoms[*i]));
    let mut left = amount;
    let mut pieces = Vec::new();
    let mut total = 0u32;
    for i in order {
        let n = left / denoms[i];
        if n > 0 {
            if n > MAX_PIECES as u128 || total + n as u32 > MAX_PIECES {
                return Err(format!("that would take more than {MAX_PIECES} deposits; bridge a smaller amount at a time"));
            }
            total += n as u32;
            pieces.push((i, n as u32));
            left -= n * denoms[i];
        }
    }
    Ok(Split { pieces, remainder: left })
}

/// Offsets (seconds from now) for `n` deposits: the first now, then about every `GAP_S` plus
/// `jitter[i] * JITTER_S` (jitter in 0..1; random in use, fixed in tests).
pub fn schedule(n: usize, jitter: &[f64]) -> Vec<u64> {
    let mut at = 0u64;
    (0..n)
        .map(|i| {
            if i > 0 {
                let j = jitter.get(i).copied().unwrap_or(0.5).clamp(0.0, 1.0);
                at += GAP_S + (j * JITTER_S as f64) as u64;
            }
            at
        })
        .collect()
}

pub fn random_jitter(n: usize) -> Vec<f64> {
    let mut bytes = vec![0u8; n * 2];
    let _ = getrandom::getrandom(&mut bytes);
    bytes.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]]) as f64 / 65535.0).collect()
}

/// The plan `plan_deposit` returns. `deposits_so_far[i]` is how many deposits of denomination
/// `i` the vault has seen (its anonymity set today).
pub fn deposit_plan(symbol: &str, decimals: u8, amount: u128, denoms: &[u128], fees: &[u128], deposits_so_far: &[u64], jitter: &[f64]) -> Result<Value, String> {
    if amount == 0 {
        return Err("enter an amount above zero".to_owned());
    }
    let s = split(amount, denoms)?;
    if s.pieces.is_empty() {
        let smallest = denoms.iter().min().copied().unwrap_or(0);
        return Err(format!("the smallest amount the bridge moves is {} {symbol}", format_amount(smallest, decimals)));
    }
    let n: usize = s.pieces.iter().map(|(_, c)| *c as usize).sum();
    let offsets = schedule(n, jitter);
    let mut rows = Vec::new();
    let mut fee_total = 0u128;
    let mut k = 0usize;
    let mut warnings = Vec::new();
    for (i, count) in &s.pieces {
        let fee = fees.get(*i).copied().unwrap_or(0);
        fee_total += fee * *count as u128;
        let set = deposits_so_far.get(*i).copied().unwrap_or(0);
        if set < SMALL_SET {
            warnings.push(format!(
                "Only {} of {} {symbol} so far. A claim of this amount is easy to match while few others exist: wait for more before claiming.",
                crate::util::count(set, "deposit", "deposits"),
                format_amount(denoms[*i], decimals)
            ));
        }
        for _ in 0..*count {
            rows.push(json!({
                "denom_index": i,
                "denom": denoms[*i].to_string(),
                "denom_text": format_amount(denoms[*i], decimals),
                "deposit_fee": fee.to_string(),
                "send_after_s": offsets[k],
                "send_after_text": if offsets[k] == 0 { "now".to_owned() } else { format!("in about {}", duration_text(offsets[k])) },
                "claim_after_s": offsets[k] + CLAIM_WAIT_S,
                "anonymity_set": set,
            }));
            k += 1;
        }
    }
    let bridged = amount - s.remainder;
    if s.remainder > 0 {
        warnings.push(format!(
            "{} {symbol} does not fit a denomination and stays with you. Sending an odd amount some other way would make you easy to recognise.",
            format_amount(s.remainder, decimals)
        ));
    }
    let span = offsets.last().copied().unwrap_or(0);
    Ok(json!({
        "symbol": symbol,
        "decimals": decimals,
        "amount": amount.to_string(),
        "bridged": bridged.to_string(),
        "bridged_text": format_amount(bridged, decimals),
        "remainder": s.remainder.to_string(),
        "remainder_text": format_amount(s.remainder, decimals),
        "pieces": rows,
        "summary": s.pieces.iter().map(|(i, c)| json!({ "denom_index": i, "count": c, "denom_text": format_amount(denoms[*i], decimals) })).collect::<Vec<_>>(),
        "deposit_fees": fee_total.to_string(),
        "deposit_fees_text": format_amount(fee_total, decimals),
        "total_to_send": (bridged + fee_total).to_string(),
        "total_to_send_text": format_amount(bridged + fee_total, decimals),
        "spread_s": span,
        "spread_text": if n == 1 { "one deposit".to_owned() } else { format!("{n} deposits over about {}", duration_text(span)) },
        "claim_advice": format!("Claim each piece no sooner than {} after its deposit, and later still while few deposits of the same amount exist.", duration_text(CLAIM_WAIT_S)),
        "lez_fee": "The claim's LEZ fee is paid from the bridge's float (sponsored), so nothing has to fund your private account first.",
        "warnings": warnings,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const USDC: [u128; 3] = [100_000_000, 1_000_000_000, 10_000_000_000];

    #[test]
    fn amounts_split_into_the_fewest_pieces() {
        let s = split(12_300_000_000, &USDC).unwrap();
        assert_eq!(s.pieces, vec![(2, 1), (1, 2), (0, 3)]);
        assert_eq!(s.remainder, 0);
        let s = split(150_500_000, &USDC).unwrap();
        assert_eq!(s.pieces, vec![(0, 1)]);
        assert_eq!(s.remainder, 50_500_000);
        assert!(split(99_000_000, &USDC).unwrap().pieces.is_empty());
        // Order of the vault's list does not matter.
        assert_eq!(split(1_100_000_000, &[1_000_000_000, 100_000_000]).unwrap().pieces, vec![(0, 1), (1, 1)]);
    }

    #[test]
    fn too_many_pieces_is_refused_not_truncated() {
        assert!(split(25 * 10_000_000_000, &USDC).is_err());
        assert!(split(24 * 10_000_000_000, &USDC).is_ok());
        assert!(split(1, &[]).is_err());
    }

    #[test]
    fn deposits_are_spread_out_with_jitter() {
        let at = schedule(3, &[0.0, 0.0, 1.0]);
        assert_eq!(at, vec![0, GAP_S, 2 * GAP_S + JITTER_S]);
        let a = schedule(4, &random_jitter(4));
        assert!(a.windows(2).all(|w| w[1] - w[0] >= GAP_S && w[1] - w[0] <= GAP_S + JITTER_S));
    }

    #[test]
    fn the_plan_counts_fees_warns_on_small_sets_and_keeps_the_remainder() {
        let p = deposit_plan("dUSDC", 6, 1_250_000_000, &USDC, &[0, 0, 0], &[3, 1, 0], &[0.5; 4]).unwrap();
        assert_eq!(p["pieces"].as_array().unwrap().len(), 3);
        assert_eq!(p["bridged_text"], "1,200");
        assert_eq!(p["remainder_text"], "50");
        assert_eq!(p["pieces"][0]["denom_text"], "1,000");
        assert_eq!(p["pieces"][0]["send_after_text"], "now");
        assert_eq!(p["pieces"][1]["claim_after_s"].as_u64().unwrap(), p["pieces"][1]["send_after_s"].as_u64().unwrap() + CLAIM_WAIT_S);
        let w: Vec<&str> = p["warnings"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect();
        assert!(w.iter().any(|x| x.contains("Only 1 deposit of 1,000")));
        assert!(w.iter().any(|x| x.contains("50 dUSDC does not fit")));
        let fees = deposit_plan("dUSDC", 6, 200_000_000, &USDC, &[500_000, 0, 0], &[30, 0, 0], &[0.5; 2]).unwrap();
        assert_eq!(fees["deposit_fees_text"], "1");
        assert_eq!(fees["total_to_send_text"], "201");
        assert!(fees["warnings"].as_array().unwrap().is_empty());
    }

    #[test]
    fn nothing_to_bridge_is_explained() {
        let e = deposit_plan("dUSDC", 6, 5_000_000, &USDC, &[0, 0, 0], &[0, 0, 0], &[]).unwrap_err();
        assert!(e.contains("smallest amount the bridge moves is 100 dUSDC"));
        assert!(deposit_plan("dUSDC", 6, 0, &USDC, &[0, 0, 0], &[0, 0, 0], &[]).is_err());
    }
}
