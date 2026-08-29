//! Decodes raw pending-transaction payloads from the mempool WSS feed into
//! typed events the rest of the [`super`] module can act on.
//!
//! Every entry point here is defensive by construction: malformed or
//! adversarial calldata (truncated input, a garbage array length, an
//! unknown selector) returns `None` rather than panicking or allocating
//! unboundedly. A public mempool is untrusted input, and this decoder runs
//! on every pending transaction that reaches our subscription — it must
//! stay cheap and crash-proof under arbitrary bytes.

use super::PoolKey;
use crate::bridge::Chain;
use sha3::{Digest, Keccak256};

/// Minimal shape of an Alchemy `alchemy_pendingTransactions` (full-tx, not
/// hashes-only) subscription notification this decoder cares about. Extra
/// fields in the real payload are ignored by `serde`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct RawPendingTx {
    pub hash: String,
    pub from: String,
    pub to: Option<String>,
    pub input: String,
    #[serde(rename = "gasPrice", default)]
    pub gas_price: Option<String>,
    #[serde(rename = "maxFeePerGas", default)]
    pub max_fee_per_gas: Option<String>,
}

/// What a decoded pending transaction represents.
#[derive(Debug, Clone, PartialEq)]
pub enum DecodedKind {
    /// A call into one of our own bridge contracts (CCTP's
    /// `MessageTransmitter`, the deBridge gateway, ...). Logged for
    /// visibility; not itself sandwich-detectable the way a DEX swap is.
    BridgeCall { contract: String },
    /// A DEX-router swap whose `path` decoded into a pool this engine
    /// recognizes (both tokens resolve to a known symbol). Fed into
    /// [`super::sandwich::SandwichDetector`].
    DexSwap { pool: PoolKey },
}

/// A pending transaction that matched a signature we watch for.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedPendingTx {
    pub hash: String,
    pub from: String,
    pub gas_price_wei: u128,
    pub kind: DecodedKind,
}

const CCTP_DEPOSIT_FOR_BURN: &str = "depositForBurn(uint256,uint32,bytes32,address)";
const DEBRIDGE_SEND: &str = "send(address,uint256,uint256,bytes,bytes,bool,uint32,bytes)";
const SWAP_EXACT_TOKENS_FOR_TOKENS: &str =
    "swapExactTokensForTokens(uint256,uint256,address[],address,uint256)";
const SWAP_EXACT_ETH_FOR_TOKENS: &str = "swapExactETHForTokens(uint256,address[],address,uint256)";

/// Longest `path` array this decoder will walk. Real swap routes are 2-4
/// hops; this bound exists purely so a malicious/garbage "array length"
/// field in adversarial calldata can never turn into an oversized read.
const MAX_PATH_LEN: usize = 8;

fn selector(signature: &str) -> [u8; 4] {
    let hash = Keccak256::digest(signature.as_bytes());
    [hash[0], hash[1], hash[2], hash[3]]
}

/// Resolves a token contract address to the symbol the router's mock
/// pricing model understands, or `None` for a token this engine doesn't
/// route. Stands in for a real on-chain token registry, in the same spirit
/// as [`crate::router::slippage::pool_depth_usd`]'s mock liquidity depths.
fn resolve_symbol(chain: Chain, address: &str) -> Option<&'static str> {
    const ETHEREUM: &[(&str, &str)] = &[
        ("0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2", "ETH"), // WETH
        ("0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48", "USDC"),
    ];
    const ARBITRUM: &[(&str, &str)] = &[
        ("0x82af49447d8a07e3bd95bd0d56f35241523fbab1", "ETH"), // WETH (Arbitrum)
        ("0xaf88d065e77c8cc2239327c5edb3a432268e5831", "USDC"),
    ];

    let table: &[(&str, &str)] = match chain {
        Chain::Ethereum => ETHEREUM,
        Chain::Arbitrum => ARBITRUM,
        Chain::Solana | Chain::Stellar => &[],
    };
    let addr = address.to_lowercase();
    table
        .iter()
        .find(|(a, _)| *a == addr)
        .map(|(_, symbol)| *symbol)
}

fn parse_hex_u128(s: &str) -> Option<u128> {
    u128::from_str_radix(s.strip_prefix("0x").unwrap_or(s), 16).ok()
}

/// Reads the big-endian uint256 word at `word` as a `usize`, rejecting any
/// value whose high bytes are non-zero (i.e. too large to be a sane
/// offset/length) instead of silently truncating it.
fn word_to_usize(word: &[u8]) -> Option<usize> {
    if word.len() != 32 {
        return None;
    }
    if word[..24].iter().any(|&b| b != 0) {
        return None;
    }
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&word[24..32]);
    Some(u64::from_be_bytes(buf) as usize)
}

fn addr_from_word(word: &[u8]) -> String {
    format!("0x{}", hex::encode(&word[12..32]))
}

/// Decodes a `address[] path` parameter out of ABI-encoded `args` (the
/// calldata *after* the 4-byte selector) and resolves its first/last
/// entries to a pool. `path_offset_slot` is the zero-indexed head slot
/// holding the array's byte offset, per the target function's signature.
fn decode_swap_path(
    chain: Chain,
    args: &[u8],
    path_offset_slot: usize,
) -> Option<(String, String)> {
    let head_start = path_offset_slot.checked_mul(32)?;
    let head_end = head_start.checked_add(32)?;
    let offset = word_to_usize(args.get(head_start..head_end)?)?;

    let len_end = offset.checked_add(32)?;
    let len = word_to_usize(args.get(offset..len_end)?)?;
    if !(2..=MAX_PATH_LEN).contains(&len) {
        return None;
    }

    let first_start = offset.checked_add(32)?;
    let first_end = first_start.checked_add(32)?;
    let last_start = first_start.checked_add((len - 1).checked_mul(32)?)?;
    let last_end = last_start.checked_add(32)?;
    let first = args.get(first_start..first_end)?;
    let last = args.get(last_start..last_end)?;

    let sym_in = resolve_symbol(chain, &addr_from_word(first))?;
    let sym_out = resolve_symbol(chain, &addr_from_word(last))?;
    Some((sym_in.to_string(), sym_out.to_string()))
}

/// Decodes one raw pending transaction. Returns `None` for anything that
/// isn't a call to a signature this module recognizes, has malformed
/// calldata, or (for a swap) trades a token pair this engine doesn't know.
pub fn decode_pending_tx(chain: Chain, tx: &RawPendingTx) -> Option<DecodedPendingTx> {
    let to = tx.to.as_deref()?;
    let input = tx.input.strip_prefix("0x").unwrap_or(&tx.input);
    if input.len() < 8 {
        return None;
    }
    let sel_bytes = hex::decode(&input[..8]).ok()?;
    let sel: [u8; 4] = sel_bytes.try_into().ok()?;
    let args = hex::decode(&input[8..]).unwrap_or_default();

    let kind = if sel == selector(CCTP_DEPOSIT_FOR_BURN) || sel == selector(DEBRIDGE_SEND) {
        DecodedKind::BridgeCall {
            contract: to.to_lowercase(),
        }
    } else if sel == selector(SWAP_EXACT_TOKENS_FOR_TOKENS) {
        let (asset_in, asset_out) = decode_swap_path(chain, &args, 2)?;
        DecodedKind::DexSwap {
            pool: PoolKey::new(chain, &asset_in, &asset_out),
        }
    } else if sel == selector(SWAP_EXACT_ETH_FOR_TOKENS) {
        let (asset_in, asset_out) = decode_swap_path(chain, &args, 1)?;
        DecodedKind::DexSwap {
            pool: PoolKey::new(chain, &asset_in, &asset_out),
        }
    } else {
        return None;
    };

    let gas_price_wei = tx
        .max_fee_per_gas
        .as_deref()
        .or(tx.gas_price.as_deref())
        .and_then(parse_hex_u128)
        .unwrap_or(0);

    Some(DecodedPendingTx {
        hash: tx.hash.clone(),
        from: tx.from.to_lowercase(),
        gas_price_wei,
        kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const WETH_ETH: &str = "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2";
    const USDC_ETH: &str = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48";
    const UNKNOWN_TOKEN: &str = "0x00000000000000000000000000000000DeadBeef";
    const DUMMY_ADDR: &str = "0x111111111111111111111111111111111111aaaa";

    fn word_u256(n: u64) -> [u8; 32] {
        let mut word = [0u8; 32];
        word[24..32].copy_from_slice(&n.to_be_bytes());
        word
    }

    fn word_addr(addr: &str) -> [u8; 32] {
        let addr = addr.strip_prefix("0x").unwrap_or(addr);
        let bytes = hex::decode(addr).unwrap();
        let mut word = [0u8; 32];
        word[12..32].copy_from_slice(&bytes);
        word
    }

    /// Builds ABI-encoded calldata for `signature`, laying out `head`
    /// slots (each already a 32-byte word) followed by the encoded
    /// `address[] path` tail, mirroring exactly what solc emits for a
    /// function whose only dynamic parameter is `path`.
    fn encode_call(
        signature: &str,
        mut head: Vec<[u8; 32]>,
        path_slot: usize,
        path: &[&str],
    ) -> Vec<u8> {
        let head_len_bytes = head.len() * 32;
        head[path_slot] = word_u256(head_len_bytes as u64);

        let mut tail = Vec::new();
        tail.extend_from_slice(&word_u256(path.len() as u64));
        for addr in path {
            tail.extend_from_slice(&word_addr(addr));
        }

        let mut calldata = selector(signature).to_vec();
        for word in &head {
            calldata.extend_from_slice(word);
        }
        calldata.extend_from_slice(&tail);
        calldata
    }

    fn raw_tx(to: &str, input: Vec<u8>) -> RawPendingTx {
        RawPendingTx {
            hash: "0xhash".to_string(),
            from: "0xFrom".to_string(),
            to: Some(to.to_string()),
            input: format!("0x{}", hex::encode(input)),
            gas_price: Some("0x3b9aca00".to_string()), // 1 gwei
            max_fee_per_gas: None,
        }
    }

    #[test]
    fn decodes_cctp_deposit_for_burn_as_a_bridge_call() {
        let mut calldata = selector(CCTP_DEPOSIT_FOR_BURN).to_vec();
        calldata.extend_from_slice(&word_u256(1_000_000)); // amount
        calldata.extend_from_slice(&word_u256(3)); // destinationDomain
        calldata.extend_from_slice(&[0u8; 32]); // mintRecipient
        calldata.extend_from_slice(&word_addr(USDC_ETH)); // burnToken

        let tx = raw_tx("0x0a992d191deec32afe36203ad87d7d289a738f81", calldata);
        let decoded = decode_pending_tx(Chain::Ethereum, &tx).expect("should decode");
        assert_eq!(
            decoded.kind,
            DecodedKind::BridgeCall {
                contract: "0x0a992d191deec32afe36203ad87d7d289a738f81".to_string()
            }
        );
        assert_eq!(decoded.gas_price_wei, 1_000_000_000);
    }

    #[test]
    fn decodes_a_swap_with_known_tokens_into_a_pool() {
        let head = vec![
            word_u256(1_000),
            word_u256(1),
            [0u8; 32],
            word_addr(DUMMY_ADDR),
            word_u256(0),
        ];
        let calldata = encode_call(SWAP_EXACT_TOKENS_FOR_TOKENS, head, 2, &[WETH_ETH, USDC_ETH]);

        let tx = raw_tx("0xRouter", calldata);
        let decoded = decode_pending_tx(Chain::Ethereum, &tx).expect("should decode");
        assert_eq!(
            decoded.kind,
            DecodedKind::DexSwap {
                pool: PoolKey::new(Chain::Ethereum, "ETH", "USDC")
            }
        );
    }

    #[test]
    fn decodes_swap_exact_eth_for_tokens_with_a_smaller_head() {
        let head = vec![word_u256(1), [0u8; 32], word_addr(DUMMY_ADDR), word_u256(0)];
        let calldata = encode_call(SWAP_EXACT_ETH_FOR_TOKENS, head, 1, &[WETH_ETH, USDC_ETH]);

        let tx = raw_tx("0xRouter", calldata);
        let decoded = decode_pending_tx(Chain::Ethereum, &tx).expect("should decode");
        assert_eq!(
            decoded.kind,
            DecodedKind::DexSwap {
                pool: PoolKey::new(Chain::Ethereum, "ETH", "USDC")
            }
        );
    }

    #[test]
    fn swap_with_an_unknown_token_does_not_decode() {
        let head = vec![
            word_u256(1_000),
            word_u256(1),
            [0u8; 32],
            word_addr(DUMMY_ADDR),
            word_u256(0),
        ];
        let calldata = encode_call(
            SWAP_EXACT_TOKENS_FOR_TOKENS,
            head,
            2,
            &[WETH_ETH, UNKNOWN_TOKEN],
        );

        let tx = raw_tx("0xRouter", calldata);
        assert!(decode_pending_tx(Chain::Ethereum, &tx).is_none());
    }

    #[test]
    fn unknown_selector_is_ignored() {
        let mut calldata = selector("someRandomFunction(uint256)").to_vec();
        calldata.extend_from_slice(&word_u256(1));
        let tx = raw_tx("0xWhatever", calldata);
        assert!(decode_pending_tx(Chain::Ethereum, &tx).is_none());
    }

    #[test]
    fn missing_to_address_is_ignored_not_a_panic() {
        let mut tx = raw_tx("0xRouter", selector(CCTP_DEPOSIT_FOR_BURN).to_vec());
        tx.to = None;
        assert!(decode_pending_tx(Chain::Ethereum, &tx).is_none());
    }

    #[test]
    fn truncated_calldata_is_ignored_not_a_panic() {
        // Selector matches a swap signature, but the head is chopped off
        // entirely: must fail safely rather than panicking on an
        // out-of-bounds slice.
        let calldata = selector(SWAP_EXACT_TOKENS_FOR_TOKENS).to_vec();
        let tx = raw_tx("0xRouter", calldata);
        assert!(decode_pending_tx(Chain::Ethereum, &tx).is_none());
    }

    #[test]
    fn oversized_path_length_is_rejected_not_walked() {
        // A garbage/adversarial length field claiming a huge array must be
        // rejected by the MAX_PATH_LEN bound, not turned into an
        // out-of-bounds read or a large allocation.
        let head = vec![
            word_u256(1_000),
            word_u256(1),
            [0u8; 32],
            word_addr(DUMMY_ADDR),
            word_u256(0),
        ];
        let mut calldata = selector(SWAP_EXACT_TOKENS_FOR_TOKENS).to_vec();
        let head_len_bytes = head.len() * 32;
        let mut head = head;
        head[2] = word_u256(head_len_bytes as u64);
        for word in &head {
            calldata.extend_from_slice(word);
        }
        // Claim an absurd path length with no backing data.
        calldata.extend_from_slice(&word_u256(u64::MAX));

        let tx = raw_tx("0xRouter", calldata);
        assert!(decode_pending_tx(Chain::Ethereum, &tx).is_none());
    }

    #[test]
    fn hex_prefix_is_optional_on_input() {
        let mut calldata = selector(CCTP_DEPOSIT_FOR_BURN).to_vec();
        calldata.extend_from_slice(&word_u256(1));
        calldata.extend_from_slice(&word_u256(3));
        calldata.extend_from_slice(&[0u8; 32]);
        calldata.extend_from_slice(&word_addr(USDC_ETH));

        let mut tx = raw_tx("0xBridge", calldata);
        tx.input = tx.input.trim_start_matches("0x").to_string();
        assert!(decode_pending_tx(Chain::Ethereum, &tx).is_some());
    }

    #[test]
    fn max_fee_per_gas_takes_priority_over_gas_price() {
        let mut calldata = selector(CCTP_DEPOSIT_FOR_BURN).to_vec();
        calldata.extend_from_slice(&word_u256(1));
        calldata.extend_from_slice(&word_u256(3));
        calldata.extend_from_slice(&[0u8; 32]);
        calldata.extend_from_slice(&word_addr(USDC_ETH));

        let mut tx = raw_tx("0xBridge", calldata);
        tx.max_fee_per_gas = Some("0x77359400".to_string()); // 2 gwei
        let decoded = decode_pending_tx(Chain::Ethereum, &tx).unwrap();
        assert_eq!(decoded.gas_price_wei, 2_000_000_000);
    }
}
