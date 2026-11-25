use std::io::Read;

use anyhow::{Context, Result, bail};
use flate2::read::ZlibDecoder;

pub const MAX_IMPORT_BYTES: u64 = 16 * 1024 * 1024;
pub const PNG_SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];
const MAX_PNG_CHUNKS: usize = 4096;

/// Safely extracts SillyTavern character JSON from PNG `tEXt` or `iTXt` (`ccv3` / `chara`) chunks.
/// Defends against integer overflows, multi-chunk zlib bombs, silent truncation, and oversized base64.
pub fn extract_png_chara_json(bytes: &[u8]) -> Result<String> {
    if bytes.len() < 8 || !bytes.starts_with(&PNG_SIGNATURE) {
        bail!("不是合法的 PNG 文件");
    }
    let mut pos = 8usize;
    let mut chunk_count = 0usize;
    let mut ccv3: Option<String> = None;
    let mut chara: Option<String> = None;

    while pos.checked_add(12).is_some_and(|end| end <= bytes.len()) {
        chunk_count += 1;
        if chunk_count > MAX_PNG_CHUNKS {
            bail!("PNG chunk 数量超过安全上限（{MAX_PNG_CHUNKS}），已拒绝解析");
        }
        let len = u32::from_be_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
        let chunk_type = &bytes[pos + 4..pos + 8];
        let Some(data_start) = pos.checked_add(8) else {
            break;
        };
        let Some(data_end) = data_start.checked_add(len) else {
            bail!("PNG chunk 长度字段异常溢出");
        };
        let Some(next_pos) = data_end.checked_add(4) else {
            bail!("PNG chunk 边界异常溢出");
        };
        if next_pos > bytes.len() {
            break;
        }
        let data = &bytes[data_start..data_end];

        if chunk_type == b"tEXt" && ccv3.is_none() {
            if let Some((key, val)) = parse_text_chunk(data) {
                if key == "ccv3" {
                    ccv3 = Some(val);
                } else if key == "chara" && chara.is_none() {
                    chara = Some(val);
                }
            }
        } else if chunk_type == b"iTXt" && ccv3.is_none() {
            if let Some((key, val)) = parse_itxt_chunk(data, chara.is_some())? {
                if key == "ccv3" {
                    ccv3 = Some(val);
                } else if key == "chara" && chara.is_none() {
                    chara = Some(val);
                }
            }
        } else if chunk_type == b"IEND" {
            break;
        }
        pos = next_pos;
    }

    let b64 = ccv3
        .or(chara)
        .context("该 PNG 中未找到 SillyTavern `ccv3` 或 `chara` 元数据块")?;
    let decoded = decode_base64(b64.trim()).context("解码 PNG 中的 Base64 角色卡数据失败")?;
    String::from_utf8(decoded).context("解码后的角色卡载荷不是有效的 UTF-8 JSON")
}

fn is_target_keyword(raw_key: &[u8]) -> Option<&'static str> {
    if raw_key.eq_ignore_ascii_case(b"ccv3") {
        Some("ccv3")
    } else if raw_key.eq_ignore_ascii_case(b"chara") {
        Some("chara")
    } else {
        None
    }
}

fn parse_text_chunk(data: &[u8]) -> Option<(&'static str, String)> {
    let nul = data.iter().position(|&b| b == 0)?;
    // Match target keyword BEFORE allocating value bytes!
    let key = is_target_keyword(&data[..nul])?;
    let val = String::from_utf8_lossy(&data[nul + 1..]).to_string();
    Some((key, val))
}

fn parse_itxt_chunk(data: &[u8], has_chara: bool) -> Result<Option<(&'static str, String)>> {
    let Some(nul1) = data.iter().position(|&b| b == 0) else {
        return Ok(None);
    };
    // Filter keyword FIRST so irrelevant or duplicate `chara` chunks NEVER trigger zlib decompression!
    let Some(key) = is_target_keyword(&data[..nul1]) else {
        return Ok(None);
    };
    if key == "chara" && has_chara {
        return Ok(None);
    }
    if nul1 + 3 > data.len() {
        return Ok(None);
    }
    let comp_flag = data[nul1 + 1];
    let comp_method = data[nul1 + 2];
    if comp_flag != 0 && !(comp_flag == 1 && comp_method == 0) {
        bail!("PNG iTXt 块包含不支持的压缩标志或压缩算法");
    }

    let rest = &data[nul1 + 3..];
    let Some(nul2) = rest.iter().position(|&b| b == 0) else {
        return Ok(None);
    };
    let rest = &rest[nul2 + 1..];
    let Some(nul3) = rest.iter().position(|&b| b == 0) else {
        return Ok(None);
    };
    let text_bytes = &rest[nul3 + 1..];

    let text = if comp_flag == 1 {
        let mut dec = ZlibDecoder::new(text_bytes).take(MAX_IMPORT_BYTES + 1);
        let mut out = Vec::new();
        dec.read_to_end(&mut out)
            .context("解压 PNG iTXt 压缩块失败")?;
        if out.len() as u64 > MAX_IMPORT_BYTES {
            bail!("PNG iTXt 解压体积超过 16 MiB 安全上限（疑似压缩炸弹），已拒绝");
        }
        String::from_utf8(out).context("PNG iTXt 解压结果不是合法 UTF-8")?
    } else {
        if text_bytes.len() as u64 > MAX_IMPORT_BYTES {
            bail!("PNG iTXt 文本体积超过 16 MiB 安全上限，已拒绝");
        }
        String::from_utf8(text_bytes.to_vec()).context("PNG iTXt 文本不是合法 UTF-8")?
    };
    Ok(Some((key, text)))
}

fn decode_base64(input: &str) -> Result<Vec<u8>> {
    if (input.len() as u64) > MAX_IMPORT_BYTES * 2 {
        bail!("Base64 载荷超过安全体积上限，已拒绝");
    }
    let cap = (input.len() / 4)
        .saturating_mul(3)
        .min(MAX_IMPORT_BYTES as usize);
    let mut out = Vec::with_capacity(cap);
    let mut buf = 0u32;
    let mut bits = 0u8;
    for b in input.bytes() {
        if b.is_ascii_whitespace() || b == b'=' {
            continue;
        }
        let val = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => bail!("Base64 包含非法字符"),
        };
        buf = (buf << 6) | u32::from(val);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            if out.len() as u64 >= MAX_IMPORT_BYTES {
                bail!("Base64 解码体积超过 16 MiB 安全上限，已拒绝");
            }
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Ok(out)
}
