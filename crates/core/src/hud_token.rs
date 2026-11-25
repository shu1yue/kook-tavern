use crate::config::AppConfig;
use tavern_domain::RoomState;

/// Pure-Rust FIPS 180-4 SHA-256 (zero external crate dependency for locked offline builds).
pub fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut msg = Vec::with_capacity(data.len() + 72);
    msg.extend_from_slice(data);
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for block_idx in 0..(msg.len() / 64) {
        let chunk = &msg[block_idx * 64..(block_idx + 1) * 64];
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g; g = f; f = e; e = d.wrapping_add(t1);
            d = c; c = b; b = a; a = t1.wrapping_add(t2);
        }
        for (slot, delta) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(delta);
        }
    }
    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// Pure-Rust RFC 2104 HMAC-SHA256.
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k_block = [0u8; 64];
    if key.len() > 64 {
        k_block[..32].copy_from_slice(&sha256(key));
    } else {
        k_block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Vec::with_capacity(64 + msg.len());
    for b in k_block {
        inner.push(b ^ 0x36);
    }
    inner.extend_from_slice(msg);
    let inner_hash = sha256(&inner);

    let mut outer = [0u8; 96];
    for i in 0..64 {
        outer[i] = k_block[i] ^ 0x5c;
    }
    outer[64..96].copy_from_slice(&inner_hash);
    sha256(&outer)
}

/// Derives a persistent server-side HUD signing secret (deterministic across restarts so open
/// player HUD windows stay live seamlessly, yet unknown to clients/teammates).
pub fn derive_hud_secret(
    explicit: Option<&str>,
    kook_token: Option<&str>,
    llm_key: Option<&str>,
) -> [u8; 32] {
    if let Some(s) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
        return sha256(format!("tavern-hud-explicit-v1:{s}").as_bytes());
    }
    let kt = kook_token.map(str::trim).unwrap_or("");
    let lk = llm_key.map(str::trim).unwrap_or("");
    if !kt.is_empty() || !lk.is_empty() {
        return sha256(format!("tavern-hud-derived-v1:{kt}:{lk}").as_bytes());
    }
    sha256(b"tavern-hud-local-dev-v1")
}

/// Computes a 128-bit hex HMAC bound to `(server_secret, room.web_token, channel_id, user_id)`.
/// Auto-revokes whenever `room.web_token` rotates on `/reset` or `/start`.
pub fn sign_hud_token(
    secret: &[u8; 32],
    room_web_token: &str,
    channel_id: &str,
    user_id: &str,
) -> String {
    let payload = format!("hud-v1|{room_web_token}|{channel_id}|{user_id}");
    let mac = hmac_sha256(secret, payload.as_bytes());
    let mut hex = String::with_capacity(32);
    for b in &mac[..16] {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{b:02x}");
    }
    hex
}

pub fn verify_hud_token(
    secret: &[u8; 32],
    room_web_token: &str,
    channel_id: &str,
    user_id: &str,
    candidate: &str,
) -> bool {
    if room_web_token.trim().is_empty()
        || channel_id.is_empty()
        || user_id.is_empty()
        || candidate.len() != 32
    {
        return false;
    }
    let expected = sign_hud_token(secret, room_web_token, channel_id, user_id);
    let mut diff = 0u8;
    for (a, b) in expected.bytes().zip(candidate.bytes()) {
        diff |= a ^ b;
    }
    diff == 0
}

pub fn build_player_hud_url(cfg: &AppConfig, room: &RoomState, user_id: &str) -> Option<String> {
    let bind = cfg.web_bind_addr.as_deref()?;
    if room.web_token.trim().is_empty() || user_id.trim().is_empty() {
        return None;
    }
    let base = match cfg.web_public_base_url.as_deref() {
        Some(url) if !url.trim().is_empty() => url.trim_end_matches('/').to_string(),
        _ if bind.starts_with("0.0.0.0:") || bind.starts_with("[::]:") => return None,
        _ => format!("http://{bind}"),
    };
    let secret = cfg.hud_secret();
    let sig = sign_hud_token(&secret, &room.web_token, &room.channel_id, user_id);
    let enc_chan = percent_encode_component(&room.channel_id);
    let enc_uid = percent_encode_component(user_id);
    Some(format!("{base}/hud/{enc_chan}?u={enc_uid}&ptoken={sig}"))
}

fn percent_encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            use std::fmt::Write as _;
            let _ = write!(&mut out, "%{b:02X}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn sha256_and_hmac_match_standard_rfc_vectors() {
        assert_eq!(
            to_hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // RFC 4231 Test Case 2
        assert_eq!(
            to_hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn hud_token_binds_secret_room_channel_and_user() {
        let secret = derive_hud_secret(Some("top-secret-key"), None, None);
        let tok = sign_hud_token(&secret, "room_tok_1", "ch_1", "u_alice");
        assert_eq!(tok.len(), 32);
        assert!(verify_hud_token(&secret, "room_tok_1", "ch_1", "u_alice", &tok));
        assert!(!verify_hud_token(&secret, "room_tok_1", "ch_1", "u_bob", &tok));
        assert!(!verify_hud_token(&secret, "room_tok_2", "ch_1", "u_alice", &tok));
        assert!(!verify_hud_token(&secret, "room_tok_1", "ch_2", "u_alice", &tok));
        let other_secret = derive_hud_secret(Some("wrong-key"), None, None);
        assert!(!verify_hud_token(&other_secret, "room_tok_1", "ch_1", "u_alice", &tok));
    }
}
