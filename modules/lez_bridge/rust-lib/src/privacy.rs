//! What a flow makes public, for the privacy indicator in every screen (RFP-022 Usability #5, #7;
//! Privacy #7). The texts follow DESIGN.md §8 ("What a private claim and burn still publish")
//! and the spec's statement of the privacy requirement: amount, token and timing are always
//! public; everything else that could link the Ethereum side to the LEZ side must stay on this
//! computer, unless the person chooses otherwise and confirms it.
//!
//! The private path is the default: an empty `choices` object is the fully private plan, and
//! every override that weakens it is listed with what it leaks and needs `confirm_weaker`.

use serde_json::{json, Value};

/// One override a person can choose, what it leaks, and to whom.
struct Override {
    key: &'static str,
    title: &'static str,
    leaks: &'static str,
    who: &'static str,
    flows: &'static [&'static str],
}

const OVERRIDES: [Override; 7] = [
    Override {
        key: "reveal_recipient",
        title: "Mint to my private account by name",
        leaks: "The mint names your private account instead of a one-time address. Anyone can then see that this account received a bridge mint of this token and amount at this time, and connect it to your later activity. The bridge does not sponsor the fee in this mode, so your account pays it.",
        who: "anyone reading the LEZ chain",
        flows: &["claim"],
    },
    Override {
        key: "public_account",
        title: "Use a public account",
        leaks: "A claim into a public account publishes the deposit's secret, which ties your Ethereum deposit (and the address that paid for it) to this LEZ account for good. A burn from a public account shows which account burned. Neither can be undone.",
        who: "anyone reading either chain",
        flows: &["deposit", "claim", "redeem"],
    },
    Override {
        key: "claim_early",
        title: "Claim before the suggested time",
        leaks: "A claim soon after a deposit, while few other deposits of the same amount have arrived, can be matched to it by timing alone: an observer pairs the two events that sit close together.",
        who: "anyone comparing the two chains' timestamps",
        flows: &["claim"],
    },
    Override {
        key: "all_at_once",
        title: "Deposit every piece at the same time",
        leaks: "Several deposits sent together, then claimed together, form a pattern (count, amounts, timing) that stands out from other people's deposits even though each piece is a standard amount.",
        who: "anyone reading the vault's deposits",
        flows: &["deposit"],
    },
    Override {
        key: "odd_amount",
        title: "Keep an amount that is not a standard denomination",
        leaks: "The bridge only moves standard amounts. Anything left over stays where it is; sending it some other way would fingerprint you.",
        who: "anyone reading amounts",
        flows: &["deposit", "redeem"],
    },
    Override {
        key: "self_relay",
        title: "Release on Ethereum myself (no relayer)",
        leaks: "Paying the release's gas yourself means the release transaction comes from an Ethereum address you funded. That address's history can identify you. A relayer, paid a fixed fee from the menu, avoids this.",
        who: "anyone reading Ethereum",
        flows: &["redeem"],
    },
    Override {
        key: "known_recipient",
        title: "Release to an address that is already mine and known",
        leaks: "The release recipient is public on Ethereum by construction. Sending to an address linked to your identity (an exchange deposit address, your main wallet) tells everyone who received the money, though not which burn paid it.",
        who: "anyone reading Ethereum",
        flows: &["redeem"],
    },
];

/// What every flow publishes whatever is chosen.
fn always_public(flow: &str, choices: &Value) -> Vec<Value> {
    let named = choices["reveal_recipient"] == true || choices["public_account"] == true;
    let mut v = vec![
        json!({ "item": "Token and amount", "where": "both chains", "why": "Amounts are fixed denominations, so they say little when other people move the same amount. The fewer such moves, the less they hide." }),
        json!({ "item": "Timing", "where": "both chains", "why": "When you deposit, claim, burn and release is visible. Spreading the steps out is the protection." }),
    ];
    match flow {
        "deposit" => {
            v.push(json!({ "item": "Your Ethereum address and the deposit", "where": "Ethereum", "why": "A deposit is an ordinary token transfer from your address. The commitment it carries reveals nothing about your LEZ account." }));
        }
        "claim" if choices["public_account"] == true => {
            v.push(json!({ "item": "Your public LEZ account and the deposit's secret", "where": "LEZ", "why": "A public claim publishes the secret, which ties it to your Ethereum deposit." }));
        }
        "claim" => {
            if named {
                v.push(json!({ "item": "Your private account, named as the receiver", "where": "LEZ", "why": "You chose to mint to it by name: this mint now points at it." }));
            } else {
                v.push(json!({ "item": "A one-time LEZ address that receives the mint", "where": "LEZ", "why": "Used once, for this claim only; it is not your account and does not lead to it." }));
            }
            v.push(json!({ "item": "The attestation anchor, the bridge and its deployment id", "where": "LEZ", "why": "Shared by every claim under the same anchor, so it narrows the candidates to deposits made before that anchor. Claim against the newest anchor." }));
            v.push(json!({ "item": "A claim nullifier", "where": "LEZ", "why": "Stops a second claim of the same deposit. It cannot be linked to the deposit without your secret." }));
            if !named {
                v.push(json!({ "item": "The fixed sponsor payment and the fee", "where": "LEZ", "why": "The same for every sponsored claim." }));
            }
        }
        "redeem" => {
            v.push(json!({ "item": "An exit commitment and the burned denomination", "where": "LEZ", "why": "The commitment hides your Ethereum recipient." }));
            v.push(json!({ "item": "The release recipient, relayer and relay fee", "where": "Ethereum", "why": "Public by construction. The relay fee is 0 or a value from the vault's fixed menu, so it does not fingerprint you." }));
        }
        _ => {}
    }
    v.push(json!({ "item": "Your network address", "where": "the RPC and sequencer you use", "why": "Outside the bridge's protection. Use endpoints you trust, or your own." }));
    v
}

fn stays_private(flow: &str, choices: &Value) -> Vec<&'static str> {
    let named = choices["reveal_recipient"] == true || choices["public_account"] == true;
    match flow {
        "claim" if choices["public_account"] == true => vec!["Nothing that links the two sides: a public claim publishes the deposit's secret"],
        "claim" if named => vec!["The deposit's secret", "Which Ethereum address paid for the deposit (only timing and amount can hint at it)"],
        "deposit" => vec!["Which LEZ account the deposit is for", "The deposit's secret (it never leaves this computer)"],
        "claim" => vec!["Your LEZ account", "Which deposit you are claiming", "The deposit's secret", "Who paid the claim's fee (the bridge float pays it)"],
        "redeem" => vec!["Which burn pays which release", "Your LEZ account (for a private burn)", "The exit's secret"],
        _ => vec![],
    }
}

/// The report for `choices` (a JSON object: `flow`, override keys set to true, and optional
/// `anonymity_set` and `wait_s`).
pub fn report(choices: &Value) -> Result<Value, String> {
    if !choices.is_object() {
        return Err("choices must be a JSON object, for example {\"flow\":\"claim\"}".to_owned());
    }
    let flow = choices["flow"].as_str().unwrap_or("claim");
    if !matches!(flow, "deposit" | "claim" | "redeem") {
        return Err(format!("unknown flow \"{flow}\": deposit, claim or redeem"));
    }
    for (k, _) in choices.as_object().unwrap() {
        if !matches!(k.as_str(), "flow" | "anonymity_set" | "wait_s" | "confirm_weaker") && !OVERRIDES.iter().any(|o| o.key == k) {
            return Err(format!("unknown choice \"{k}\""));
        }
    }
    let chosen: Vec<&Override> = OVERRIDES.iter().filter(|o| o.flows.contains(&flow) && choices[o.key] == true).collect();
    let available: Vec<Value> = OVERRIDES
        .iter()
        .filter(|o| o.flows.contains(&flow))
        .map(|o| json!({ "key": o.key, "title": o.title, "leaks": o.leaks, "who": o.who, "chosen": choices[o.key] == true, "needs_confirmation": true }))
        .collect();
    let severe = chosen.iter().any(|o| matches!(o.key, "public_account" | "reveal_recipient"));
    let level = if chosen.is_empty() { "private" } else if severe { "exposed" } else { "reduced" };
    let mut notes = Vec::new();
    if let Some(n) = choices["anonymity_set"].as_u64() {
        notes.push(if n < 10 {
            format!("Only {} of this amount so far: an observer has very few candidates to choose from. Waiting for more deposits makes your claim harder to match.", crate::util::count(n, "deposit", "deposits"))
        } else {
            format!("{n} deposits of this amount so far: your claim hides among them.")
        });
    }
    let headline = match level {
        "private" => "Private. Nothing published links the two sides; amount, token and timing are visible.",
        "reduced" => "Less private. The choices below make you easier to match by timing or pattern.",
        _ => "Not private. The choices below link your Ethereum side to your LEZ account.",
    };
    Ok(json!({
        "flow": flow,
        "level": level,
        "headline": headline,
        "always_public": always_public(flow, choices),
        "stays_private": stays_private(flow, choices),
        "overrides": available,
        "weakened_by": chosen.iter().map(|o| json!({ "key": o.key, "title": o.title, "leaks": o.leaks, "who": o.who })).collect::<Vec<_>>(),
        "requires_confirmation": !chosen.is_empty(),
        "confirmed": choices["confirm_weaker"] == true,
        "notes": notes,
        "on_chain_error_text": "If a claim is refused on chain, the only message anyone sees is \"claim refused\": it never says which deposit. The detailed reason shown here is worked out on this computer and is not sent.",
    }))
}

/// Refuses `choices` that weaken privacy without `confirm_weaker: true`, naming each leak.
pub fn require_confirmation(choices: &Value) -> Result<(), String> {
    let r = report(choices)?;
    if r["requires_confirmation"] == true && choices["confirm_weaker"] != true {
        let list: Vec<String> = r["weakened_by"].as_array().unwrap().iter().map(|o| format!("{}: {}", o["title"].as_str().unwrap(), o["leaks"].as_str().unwrap())).collect();
        return Err(format!("these choices weaken your privacy and need confirm_weaker: true. {}", list.join(" ")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_private_and_needs_nothing() {
        for flow in ["deposit", "claim", "redeem"] {
            let r = report(&json!({ "flow": flow })).unwrap();
            assert_eq!(r["level"], "private", "{flow}");
            assert_eq!(r["requires_confirmation"], false);
            assert!(r["weakened_by"].as_array().unwrap().is_empty());
            assert!(require_confirmation(&json!({ "flow": flow })).is_ok());
            // Amount, token and timing are always listed as public.
            let items: Vec<&str> = r["always_public"].as_array().unwrap().iter().map(|i| i["item"].as_str().unwrap()).collect();
            assert!(items.contains(&"Token and amount") && items.contains(&"Timing"), "{flow}: {items:?}");
        }
    }

    #[test]
    fn every_weakening_override_says_what_it_leaks_and_needs_confirmation() {
        for o in OVERRIDES.iter() {
            for flow in o.flows {
                let choices = json!({ "flow": flow, o.key: true });
                let r = report(&choices).unwrap();
                assert_ne!(r["level"], "private", "{}", o.key);
                assert_eq!(r["requires_confirmation"], true);
                let w = &r["weakened_by"][0];
                assert_eq!(w["key"], o.key);
                assert!(w["leaks"].as_str().unwrap().len() > 60, "{} explains itself", o.key);
                assert!(!w["who"].as_str().unwrap().is_empty());
                let refused = require_confirmation(&choices).unwrap_err();
                assert!(refused.contains("confirm_weaker") && refused.contains(o.title));
                let mut confirmed = choices.clone();
                confirmed["confirm_weaker"] = Value::from(true);
                assert!(require_confirmation(&confirmed).is_ok());
            }
        }
    }

    #[test]
    fn linking_choices_read_as_exposed() {
        assert_eq!(report(&json!({ "flow": "claim", "reveal_recipient": true })).unwrap()["level"], "exposed");
        assert_eq!(report(&json!({ "flow": "claim", "public_account": true })).unwrap()["level"], "exposed");
        assert_eq!(report(&json!({ "flow": "claim", "claim_early": true })).unwrap()["level"], "reduced");
    }

    #[test]
    fn an_override_of_another_flow_does_not_count_and_unknown_keys_are_refused() {
        assert_eq!(report(&json!({ "flow": "deposit", "self_relay": true })).unwrap()["level"], "private");
        assert!(report(&json!({ "flow": "claim", "reveal": true })).is_err());
        assert!(report(&json!({ "flow": "swap" })).is_err());
        assert!(report(&json!([])).is_err());
    }

    #[test]
    fn what_is_public_follows_the_choice() {
        let private = report(&json!({ "flow": "claim" })).unwrap().to_string();
        assert!(private.contains("one-time LEZ address") && private.contains("Who paid the claim's fee"));
        let named = report(&json!({ "flow": "claim", "reveal_recipient": true })).unwrap();
        let text = named.to_string();
        assert!(text.contains("named as the receiver") && !text.contains("one-time LEZ address") && !text.contains("Who paid the claim's fee"));
        let public = report(&json!({ "flow": "claim", "public_account": true })).unwrap().to_string();
        assert!(public.contains("deposit's secret") && public.contains("public claim publishes"));
    }

    #[test]
    fn a_small_anonymity_set_is_said_plainly() {
        let r = report(&json!({ "flow": "claim", "anonymity_set": 3 })).unwrap();
        assert!(r["notes"][0].as_str().unwrap().contains("Only 3 deposits"));
        assert!(r["on_chain_error_text"].as_str().unwrap().contains("claim refused"));
    }
}
