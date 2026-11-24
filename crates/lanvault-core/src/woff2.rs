//! Pure-Rust WOFF2 -> SFNT (OTF/TTF) decoder for untransformed OpenType CFF/TTF fonts.
//!
//! Decompresses bundled `.woff2` font assets (such as `SourceHanSansSC-Regular.woff2`)
//! into standard SFNT bytes so Flutter's `FontLoader` can register them natively across
//! Windows and Android without platform-specific native Brotli `.dll`/`.so` dependencies.

use std::io::Read;

use brotli_decompressor::Decompressor;

use crate::{LanVaultError, Result};

const WOFF2_SIGNATURE: u32 = 0x774F_4632; // "wOF2"
const SFNT_TTC_FLAVOR: u32 = 0x7474_6366; // "ttcf"
const TAG_HEAD: u32 = 0x6865_6164; // "head"
const TAG_GLYF: u32 = 0x676C_7966; // "glyf"
const TAG_LOCA: u32 = 0x6C6F_6361; // "loca"
const MAX_DECOMPRESSED_SFNT_BYTES: usize = 64 * 1024 * 1024;

const KNOWN_W2_TAGS: [u32; 63] = [
    0x636D6170, // cmap   0
    0x68656164, // head   1
    0x68686561, // hhea   2
    0x686D7478, // hmtx   3
    0x6D617870, // maxp   4
    0x6E616D65, // name   5
    0x4F532F32, // OS/2   6
    0x706F7374, // post   7
    0x63767420, // cvt    8
    0x6670676D, // fpgm   9
    0x676C7966, // glyf  10
    0x6C6F6361, // loca  11
    0x70726570, // prep  12
    0x43464620, // CFF   13
    0x564F5247, // VORG  14
    0x45424454, // EBDT  15
    0x45424C43, // EBLC  16
    0x67617370, // gasp  17
    0x68646D78, // hdmx  18
    0x6B65726E, // kern  19
    0x4C545348, // LTSH  20
    0x50434C54, // PCLT  21
    0x56444D58, // VDMX  22
    0x76686561, // vhea  23
    0x766D7478, // vmtx  24
    0x42415345, // BASE  25
    0x47444546, // GDEF  26
    0x47504F53, // GPOS  27
    0x47535542, // GSUB  28
    0x45425343, // EBSC  29
    0x4A535446, // JSTF  30
    0x4D415448, // MATH  31
    0x43424454, // CBDT  32
    0x43424C43, // CBLC  33
    0x434F4C52, // COLR  34
    0x4350414C, // CPAL  35
    0x53564720, // SVG   36
    0x73626978, // sbix  37
    0x61636E74, // acnt  38
    0x61766172, // avar  39
    0x62646174, // bdat  40
    0x626C6F63, // bloc  41
    0x62736C6E, // bsln  42
    0x63766172, // cvar  43
    0x66647363, // fdsc  44
    0x66656174, // feat  45
    0x666D7478, // fmtx  46
    0x66766172, // fvar  47
    0x67766172, // gvar  48
    0x68737479, // hsty  49
    0x6A757374, // just  50
    0x6C636172, // lcar  51
    0x6D6F7274, // mort  52
    0x6D6F7278, // morx  53
    0x6F706264, // opbd  54
    0x70726F70, // prop  55
    0x7472616B, // trak  56
    0x5A617066, // Zapf  57
    0x53696C66, // Silf  58
    0x476C6174, // Glat  59
    0x476C6F63, // Gloc  60
    0x46656174, // Feat  61
    0x53696C6C, // Sill  62
];

struct Woff2TableEntry {
    tag: u32,
    src_offset: usize,
    dst_length: usize,
}

fn read_be_u16(buf: &[u8], pos: usize) -> Result<u16> {
    let bytes: [u8; 2] = buf
        .get(pos..pos + 2)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| LanVaultError::InvalidInput("truncated woff2 u16".into()))?;
    Ok(u16::from_be_bytes(bytes))
}

fn read_be_u32(buf: &[u8], pos: usize) -> Result<u32> {
    let bytes: [u8; 4] = buf
        .get(pos..pos + 4)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| LanVaultError::InvalidInput("truncated woff2 u32".into()))?;
    Ok(u32::from_be_bytes(bytes))
}

fn read_base128(buf: &[u8], pos: &mut usize) -> Result<usize> {
    let mut accum: u32 = 0;
    for i in 0..5 {
        let code = *buf
            .get(*pos)
            .ok_or_else(|| LanVaultError::InvalidInput("truncated woff2 base128".into()))?;
        *pos += 1;
        if i == 0 && code == 0x80 {
            return Err(LanVaultError::InvalidInput(
                "invalid woff2 base128 leading zero".into(),
            ));
        }
        if (accum & 0xFE00_0000) != 0 {
            return Err(LanVaultError::InvalidInput("woff2 base128 overflow".into()));
        }
        accum = (accum << 7) | u32::from(code & 0x7F);
        if (code & 0x80) == 0 {
            return Ok(accum as usize);
        }
    }
    Err(LanVaultError::InvalidInput(
        "woff2 base128 exceeded 5 bytes".into(),
    ))
}

fn sfnt_checksum(data: &[u8]) -> u32 {
    let mut sum = 0u32;
    for chunk in data.chunks(4) {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum = sum.wrapping_add(u32::from_be_bytes(word));
    }
    sum
}

/// Decodes an OpenType CFF/TTF `.woff2` buffer (without transformed glyf/loca tables)
/// into standard SFNT bytes ready for `FontLoader`.
pub fn decode_woff2_to_sfnt(woff2: &[u8]) -> Result<Vec<u8>> {
    if woff2.len() < 48 {
        return Err(LanVaultError::InvalidInput("woff2 header too short".into()));
    }
    if read_be_u32(woff2, 0)? != WOFF2_SIGNATURE {
        return Err(LanVaultError::InvalidInput(
            "invalid woff2 signature".into(),
        ));
    }

    let flavor = read_be_u32(woff2, 4)?;
    if flavor == SFNT_TTC_FLAVOR {
        return Err(LanVaultError::InvalidInput(
            "woff2 TTC font collections are unsupported".into(),
        ));
    }
    let num_tables = usize::from(read_be_u16(woff2, 12)?);
    if num_tables == 0 {
        return Err(LanVaultError::InvalidInput("woff2 has zero tables".into()));
    }
    let total_compressed_size = read_be_u32(woff2, 20)? as usize;

    let mut pos = 48usize;
    let mut entries = Vec::with_capacity(num_tables);
    let mut stream_offset = 0usize;

    for _ in 0..num_tables {
        let flag = *woff2
            .get(pos)
            .ok_or_else(|| LanVaultError::InvalidInput("truncated woff2 table directory".into()))?;
        pos += 1;
        let tag_index = usize::from(flag & 0x3F);
        let xform_version = (flag >> 6) & 0x03;

        let tag = if tag_index == 0x3F {
            let explicit = read_be_u32(woff2, pos)?;
            pos += 4;
            explicit
        } else {
            KNOWN_W2_TAGS[tag_index]
        };

        let is_transformed = if tag == TAG_GLYF || tag == TAG_LOCA {
            xform_version == 0
        } else {
            xform_version != 0
        };
        if is_transformed {
            return Err(LanVaultError::InvalidInput(format!(
                "unsupported transformed woff2 table 0x{tag:08x}"
            )));
        }

        let dst_length = read_base128(woff2, &mut pos)?;
        let next_offset = stream_offset
            .checked_add(dst_length)
            .ok_or_else(|| LanVaultError::InvalidInput("woff2 stream size overflow".into()))?;
        if next_offset > MAX_DECOMPRESSED_SFNT_BYTES {
            return Err(LanVaultError::InvalidInput(
                "woff2 decompressed font exceeds safety limit".into(),
            ));
        }

        entries.push(Woff2TableEntry {
            tag,
            src_offset: stream_offset,
            dst_length,
        });
        stream_offset = next_offset;
    }

    let comp_end = pos
        .checked_add(total_compressed_size)
        .ok_or_else(|| LanVaultError::InvalidInput("woff2 compressed length overflow".into()))?;
    let comp_slice = woff2
        .get(pos..comp_end)
        .ok_or_else(|| LanVaultError::InvalidInput("truncated woff2 brotli payload".into()))?;

    let mut decompressed = Vec::with_capacity(stream_offset);
    let mut decoder = Decompressor::new(comp_slice, 64 * 1024);
    decoder
        .read_to_end(&mut decompressed)
        .map_err(|e| LanVaultError::InvalidInput(format!("woff2 brotli decode failed: {e}")))?;
    if decompressed.len() < stream_offset {
        return Err(LanVaultError::InvalidInput(
            "woff2 brotli stream shorter than declared table bytes".into(),
        ));
    }

    entries.sort_by_key(|e| e.tag);

    let header_size = 12 + num_tables * 16;
    let mut table_offsets = Vec::with_capacity(num_tables);
    let mut cursor = header_size;
    for entry in &entries {
        table_offsets.push(cursor);
        cursor += (entry.dst_length + 3) & !3;
    }

    let mut out = vec![0u8; cursor];
    out[0..4].copy_from_slice(&flavor.to_be_bytes());
    out[4..6].copy_from_slice(&(num_tables as u16).to_be_bytes());
    let log2n = (num_tables as f64).log2().floor() as u16;
    let search_range = (1u16 << log2n) * 16;
    let entry_selector = log2n;
    let range_shift = (num_tables as u16) * 16 - search_range;
    out[6..8].copy_from_slice(&search_range.to_be_bytes());
    out[8..10].copy_from_slice(&entry_selector.to_be_bytes());
    out[10..12].copy_from_slice(&range_shift.to_be_bytes());

    let mut head_table_offset: Option<usize> = None;

    for (idx, entry) in entries.iter().enumerate() {
        let src = &decompressed[entry.src_offset..entry.src_offset + entry.dst_length];
        let dst_off = table_offsets[idx];
        out[dst_off..dst_off + entry.dst_length].copy_from_slice(src);
        if entry.tag == TAG_HEAD && entry.dst_length >= 12 {
            out[dst_off + 8..dst_off + 12].fill(0);
            head_table_offset = Some(dst_off);
        }
        let checksum = sfnt_checksum(&out[dst_off..dst_off + entry.dst_length]);

        let dir_off = 12 + idx * 16;
        out[dir_off..dir_off + 4].copy_from_slice(&entry.tag.to_be_bytes());
        out[dir_off + 4..dir_off + 8].copy_from_slice(&checksum.to_be_bytes());
        out[dir_off + 8..dir_off + 12].copy_from_slice(&(dst_off as u32).to_be_bytes());
        out[dir_off + 12..dir_off + 16].copy_from_slice(&(entry.dst_length as u32).to_be_bytes());
    }

    if let Some(head_off) = head_table_offset {
        let total_checksum = sfnt_checksum(&out);
        let adjustment = 0xB1B0_AFBAu32.wrapping_sub(total_checksum);
        out[head_off + 8..head_off + 12].copy_from_slice(&adjustment.to_be_bytes());
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{decode_woff2_to_sfnt, sfnt_checksum};

    #[test]
    fn rejects_invalid_or_truncated_headers() {
        assert!(decode_woff2_to_sfnt(b"short").is_err());
        let mut bad = vec![0u8; 48];
        bad[0..4].copy_from_slice(b"OTTO");
        assert!(decode_woff2_to_sfnt(&bad).is_err());
    }

    #[test]
    fn decodes_bundled_source_han_sans_woff2_if_present() {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let woff2_path = Path::new(manifest_dir)
            .join("../../flutter_app/assets/fonts/SourceHanSansSC-Regular.woff2");
        if !woff2_path.exists() {
            return;
        }
        let woff2_bytes = std::fs::read(&woff2_path).expect("read bundled woff2");
        let sfnt = decode_woff2_to_sfnt(&woff2_bytes).expect("decode SourceHanSansSC woff2");
        assert_eq!(&sfnt[0..4], b"OTTO");
        assert_eq!(sfnt_checksum(&sfnt), 0xB1B0_AFBA);
    }
}
