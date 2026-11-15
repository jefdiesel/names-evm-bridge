//! A QR code as rows of `0`/`1` for the mini-app to draw (QML has no QR library).

use qrcodegen::{QrCode, QrCodeEcc};
use serde_json::{json, Value};

pub fn matrix(text: &str) -> Result<Value, String> {
    let qr = QrCode::encode_text(text, QrCodeEcc::Medium).map_err(|_| "too long for a QR code".to_owned())?;
    let n = qr.size();
    let rows: Vec<String> = (0..n).map(|y| (0..n).map(|x| if qr.get_module(x, y) { '1' } else { '0' }).collect()).collect();
    Ok(json!({ "size": n, "rows": rows, "text": text }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_deposit_uri_fits_and_is_square() {
        let uri = "ethereum:0x8C8b2664De28e5287158db4DB65D448638871b74@11155111/depositETH?uint32=0&bytes32=0xa50e0f44940eca30735460fa9b657d6ab948fc422b29b1293be0f7e5d6b7ad2e&value=1e16";
        let m = super::matrix(uri).unwrap();
        let n = m["size"].as_u64().unwrap() as usize;
        assert!((21..=177).contains(&n));
        assert!(m["rows"].as_array().unwrap().iter().all(|r| r.as_str().unwrap().len() == n));
        // Finder pattern: the top-left 7 modules of row 0 are dark.
        assert!(m["rows"][0].as_str().unwrap().starts_with("1111111"));
    }
}
