use anyhow::{bail, Context, Result};
use byteorder::{LittleEndian, ReadBytesExt};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct PkgEntry {
    pub name: String,
    pub offset: u32,
    pub length: u32,
}

#[derive(Debug)]
pub struct PkgArchive {
    pub path: PathBuf,
    pub magic: String,
    pub entries: Vec<PkgEntry>,
    pub data_start_offset: u64,
}

impl PkgArchive {
    /// Open and parse a Wallpaper Engine .pkg archive
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut file = File::open(&path).with_context(|| format!("Failed to open {:?}", path))?;

        // Read magic length
        let magic_len = file.read_u32::<LittleEndian>()? as usize;
        if magic_len > 64 {
            bail!("Invalid PKG header: magic length {} too large", magic_len);
        }

        let mut magic_bytes = vec![0u8; magic_len];
        file.read_exact(&mut magic_bytes)?;
        let magic = String::from_utf8_lossy(&magic_bytes).to_string();

        if !magic.starts_with("PKG") {
            bail!("Invalid PKG magic signature: {}", magic);
        }

        // Read entry count
        let entry_count = file.read_u32::<LittleEndian>()? as usize;
        let mut entries = Vec::with_capacity(entry_count);

        for _ in 0..entry_count {
            let name_len = file.read_u32::<LittleEndian>()? as usize;
            if name_len > 4096 {
                bail!("Invalid entry name length: {}", name_len);
            }

            let mut name_bytes = vec![0u8; name_len];
            file.read_exact(&mut name_bytes)?;
            let name = String::from_utf8_lossy(&name_bytes).to_string();

            let offset = file.read_u32::<LittleEndian>()?;
            let length = file.read_u32::<LittleEndian>()?;

            entries.push(PkgEntry {
                name,
                offset,
                length,
            });
        }

        let data_start_offset = file.stream_position()?;

        Ok(Self {
            path,
            magic,
            entries,
            data_start_offset,
        })
    }

    /// Read raw bytes for an entry
    pub fn read_entry(&self, entry: &PkgEntry) -> Result<Vec<u8>> {
        let mut file = File::open(&self.path)?;
        let file_pos = self.data_start_offset + entry.offset as u64;
        file.seek(SeekFrom::Start(file_pos))?;

        let mut buf = vec![0u8; entry.length as usize];
        file.read_exact(&mut buf)?;
        Ok(buf)
    }

    /// Find an entry by case-insensitive name
    pub fn find_entry(&self, name: &str) -> Option<&PkgEntry> {
        self.entries
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case(name))
    }

    /// Read file content by filename
    pub fn read_file(&self, name: &str) -> Result<Vec<u8>> {
        let entry = self
            .find_entry(name)
            .with_context(|| format!("File '{}' not found in PKG archive", name))?;
        self.read_entry(entry)
    }

    /// Extract `scene.json` as a UTF-8 string if present
    pub fn extract_scene_json(&self) -> Result<String> {
        let data = self.read_file("scene.json")?;
        Ok(String::from_utf8(data)?)
    }

    /// Extract all files into a destination directory
    pub fn extract_all<P: AsRef<Path>>(&self, dest_dir: P) -> Result<()> {
        let dest = dest_dir.as_ref();
        std::fs::create_dir_all(dest)?;

        let mut file = File::open(&self.path)?;

        for entry in &self.entries {
            let out_path = dest.join(&entry.name);
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            let file_pos = self.data_start_offset + entry.offset as u64;
            file.seek(SeekFrom::Start(file_pos))?;

            let mut buf = vec![0u8; entry.length as usize];
            file.read_exact(&mut buf)?;

            std::fs::write(&out_path, buf)?;
        }

        Ok(())
    }

    /// Extract the best playable visual asset (4K/1080p texture, video, gif, or image) from the PKG to out_dir
    pub fn extract_playable_asset<P: AsRef<Path>>(&self, out_dir: P) -> Result<Option<PathBuf>> {
        let dest = out_dir.as_ref();
        std::fs::create_dir_all(dest)?;

        // Check if an asset was already extracted and cached previously
        for ext in &["jpg", "jpeg", "png", "mp4", "webm", "gif", "bmp"] {
            let candidate = dest.join(format!("wallpaper.{}", ext));
            if candidate.exists() {
                if let Ok(meta) = candidate.metadata() {
                    if meta.len() > 1024 {
                        return Ok(Some(candidate));
                    }
                }
            }
        }

        // 1. Look for embedded video files (.mp4, .webm)
        for entry in &self.entries {
            let lower = entry.name.to_lowercase();
            if lower.ends_with(".mp4") || lower.ends_with(".webm") {
                let out_file = dest.join("wallpaper.mp4");
                let data = self.read_entry(entry)?;
                std::fs::write(&out_file, data)?;
                return Ok(Some(out_file));
            }
        }

        // 2. Look for high-resolution texture files (.tex) in materials/
        // Sort entries by size descending so we inspect the largest artwork/background first
        let mut tex_entries: Vec<&PkgEntry> = self
            .entries
            .iter()
            .filter(|e| e.name.to_lowercase().ends_with(".tex"))
            .collect();
        tex_entries.sort_by(|a, b| b.length.cmp(&a.length));

        for entry in tex_entries {
            if entry.length < 1024 {
                continue;
            }

            let data = self.read_entry(entry)?;

            // 2a. Check for embedded JPEG (0xFF, 0xD8, 0xFF)
            if let Some(pos) = find_subsequence(&data, b"\xff\xd8\xff") {
                let out_file = dest.join("wallpaper.jpg");
                std::fs::write(&out_file, &data[pos..])?;
                return Ok(Some(out_file));
            }

            // 2b. Check for embedded PNG (0x89, 'P', 'N', 'G')
            if let Some(pos) = find_subsequence(&data, b"\x89PNG") {
                let out_file = dest.join("wallpaper.png");
                std::fs::write(&out_file, &data[pos..])?;
                return Ok(Some(out_file));
            }

            // 2c. Check for embedded MP4 video (ftyp marker)
            if let Some(ftyp_pos) = find_subsequence(&data, b"ftyp") {
                if ftyp_pos >= 4 {
                    let out_file = dest.join("wallpaper.mp4");
                    std::fs::write(&out_file, &data[ftyp_pos - 4..])?;
                    return Ok(Some(out_file));
                }
            }

            // 2d. Check for TEXB block with LZ4-compressed RGBA texture data
            let mut search_pos = 0;
            let mut best_tb: Option<PathBuf> = None;
            let mut max_res = 0u64;
            while let Some(rel_pos) = find_subsequence(&data[search_pos..], b"TEXB") {
                let tb_pos = search_pos + rel_pos;
                if let Some((w, h, out_bmp)) = try_extract_texb_rgba(&data[tb_pos..], dest) {
                    let res = (w as u64) * (h as u64);
                    if res > max_res {
                        max_res = res;
                        best_tb = Some(out_bmp);
                    }
                }
                search_pos = tb_pos + 4;
            }
            if let Some(out_bmp) = best_tb {
                return Ok(Some(out_bmp));
            }
        }

        // 3. Look for embedded animated GIF
        for entry in &self.entries {
            let lower = entry.name.to_lowercase();
            if lower.ends_with(".gif") {
                let out_file = dest.join("wallpaper.gif");
                let data = self.read_entry(entry)?;
                std::fs::write(&out_file, data)?;
                return Ok(Some(out_file));
            }
        }

        // 4. Look for standard image files (.png, .jpg, .jpeg, .webp)
        for entry in &self.entries {
            let lower = entry.name.to_lowercase();
            if lower.ends_with(".png")
                || lower.ends_with(".jpg")
                || lower.ends_with(".jpeg")
                || lower.ends_with(".webp")
            {
                let ext = Path::new(&entry.name)
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("png");
                let out_file = dest.join(format!("wallpaper.{}", ext));
                let data = self.read_entry(entry)?;
                std::fs::write(&out_file, data)?;
                return Ok(Some(out_file));
            }
        }

        Ok(None)
    }

    /// Extract an audio track from the PKG archive if present (.mp3, .ogg, .wav, .flac)
    pub fn extract_audio_asset<P: AsRef<Path>>(&self, out_dir: P) -> Result<Option<PathBuf>> {
        let dest = out_dir.as_ref();
        std::fs::create_dir_all(dest)?;

        for entry in &self.entries {
            let lower = entry.name.to_lowercase();
            if lower.ends_with(".mp3") || lower.ends_with(".ogg") || lower.ends_with(".wav") || lower.ends_with(".flac") {
                if let Some(file_name) = Path::new(&entry.name).file_name() {
                    let out_file = dest.join(file_name);
                    if !out_file.exists() {
                        let data = self.read_entry(entry)?;
                        std::fs::write(&out_file, data)?;
                    }
                    return Ok(Some(out_file));
                }
            }
        }

        Ok(None)
    }
}

/// Locate a byte sequence inside a byte buffer
fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Try to parse and extract a TEXB block containing raw or LZ4-compressed RGBA data
fn try_extract_texb_rgba(tb_data: &[u8], dest: &Path) -> Option<(u32, u32, PathBuf)> {
    // Header format: "TEXBxxxx\0" (9 bytes)
    if tb_data.len() < 37 {
        return None;
    }

    // Supported candidate offsets: (w_off, h_off, unc_off, comp_off, payload_start)
    // TEXB0003 / TEXB0004: width at 21, height at 25, uncompressed at 33, compressed at 37, payload at 41
    // TEXB0001 / TEXB0002: width at 17, height at 21, uncompressed at 29, compressed at 33, payload at 37
    let candidates = [
        (21, 25, 33, 37, 41),
        (17, 21, 29, 33, 37),
    ];

    let mut matched = None;
    for (w_off, h_off, unc_off, comp_off, data_off) in candidates {
        if tb_data.len() >= data_off {
            if let (Some(w_slice), Some(h_slice), Some(unc_slice), Some(comp_slice)) = (
                tb_data.get(w_off..w_off + 4),
                tb_data.get(h_off..h_off + 4),
                tb_data.get(unc_off..unc_off + 4),
                tb_data.get(comp_off..comp_off + 4),
            ) {
                let w = u32::from_le_bytes(w_slice.try_into().unwrap());
                let h = u32::from_le_bytes(h_slice.try_into().unwrap());
                let unc = u32::from_le_bytes(unc_slice.try_into().unwrap()) as usize;
                let comp = u32::from_le_bytes(comp_slice.try_into().unwrap()) as usize;

                let expected_len = (w as usize) * (h as usize) * 4;
                if unc == expected_len && w >= 200 && h >= 200 && data_off + comp <= tb_data.len() {
                    matched = Some((w, h, unc, comp, data_off));
                    break;
                }
            }
        }
    }

    let (w, h, uncompressed_size, compressed_size, payload_start) = matched?;
    let payload = &tb_data[payload_start..payload_start + compressed_size];

    let decompressed = if compressed_size == uncompressed_size {
        payload.to_vec()
    } else {
        lz4_decompress_block(payload, uncompressed_size)?
    };

    let out_bmp = dest.join("wallpaper.bmp");
    if write_bmp_rgba(&out_bmp, w, h, &decompressed).is_ok() {
        Some((w, h, out_bmp))
    } else {
        None
    }
}

/// Decompress standard LZ4 block format without external crate dependencies
fn lz4_decompress_block(src: &[u8], uncompressed_size: usize) -> Option<Vec<u8>> {
    let mut dst = Vec::with_capacity(uncompressed_size);
    let mut ip = 0;
    let src_len = src.len();

    while ip < src_len {
        let token = src[ip];
        ip += 1;

        let mut lit_len = (token >> 4) as usize;
        if lit_len == 15 {
            while ip < src_len {
                let s = src[ip] as usize;
                ip += 1;
                lit_len += s;
                if s != 255 {
                    break;
                }
            }
        }

        if ip + lit_len > src_len {
            let take = src_len.saturating_sub(ip);
            dst.extend_from_slice(&src[ip..ip + take]);
            break;
        }

        dst.extend_from_slice(&src[ip..ip + lit_len]);
        ip += lit_len;

        if ip >= src_len || dst.len() >= uncompressed_size {
            break;
        }

        if ip + 2 > src_len {
            break;
        }
        let offset = (src[ip] as usize) | ((src[ip + 1] as usize) << 8);
        ip += 2;
        if offset == 0 || offset > dst.len() {
            break;
        }

        let mut match_len = ((token & 0x0f) as usize) + 4;
        if (token & 0x0f) == 15 {
            while ip < src_len {
                let s = src[ip] as usize;
                ip += 1;
                match_len += s;
                if s != 255 {
                    break;
                }
            }
        }

        for _ in 0..match_len {
            let b = dst[dst.len() - offset];
            dst.push(b);
        }
    }

    if dst.len() == uncompressed_size {
        Some(dst)
    } else {
        None
    }
}

/// Write 32-bit top-down BMP from raw RGBA pixel buffer
fn write_bmp_rgba(dest_path: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<()> {
    use std::io::Write;
    let expected_len = (w as usize) * (h as usize) * 4;
    if rgba.len() < expected_len {
        anyhow::bail!("Insufficient pixel data for BMP");
    }

    let file_size = 54 + expected_len;
    let mut file = File::create(dest_path)?;

    // BMP header (14 bytes)
    let mut bmp_header = [0u8; 14];
    bmp_header[0] = b'B';
    bmp_header[1] = b'M';
    bmp_header[2..6].copy_from_slice(&(file_size as u32).to_le_bytes());
    bmp_header[10..14].copy_from_slice(&54u32.to_le_bytes());

    // DIB header BITMAPINFOHEADER (40 bytes)
    let mut dib_header = [0u8; 40];
    dib_header[0..4].copy_from_slice(&40u32.to_le_bytes());
    dib_header[4..8].copy_from_slice(&(w as i32).to_le_bytes());
    dib_header[8..12].copy_from_slice(&(-(h as i32)).to_le_bytes()); // negative height = top-down
    dib_header[12..14].copy_from_slice(&1u16.to_le_bytes()); // 1 color plane
    dib_header[14..16].copy_from_slice(&32u16.to_le_bytes()); // 32 bpp
    dib_header[16..20].copy_from_slice(&0u32.to_le_bytes()); // BI_RGB (uncompressed)
    dib_header[20..24].copy_from_slice(&(expected_len as u32).to_le_bytes());

    file.write_all(&bmp_header)?;
    file.write_all(&dib_header)?;

    // Convert RGBA to BGRA
    let mut bgra = vec![0u8; expected_len];
    for (src_chunk, dst_chunk) in rgba[..expected_len].chunks_exact(4).zip(bgra.chunks_exact_mut(4)) {
        dst_chunk[0] = src_chunk[2]; // B
        dst_chunk[1] = src_chunk[1]; // G
        dst_chunk[2] = src_chunk[0]; // R
        dst_chunk[3] = src_chunk[3]; // A
    }

    file.write_all(&bgra)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_texb0003() {
        let pkg_path = Path::new("/hdd/SteamLibrary/steamapps/workshop/content/431960/1897907008/scene.pkg");
        if pkg_path.exists() {
            let archive = PkgArchive::open(pkg_path).expect("Failed to open scene.pkg");
            let temp_dir = std::env::temp_dir().join("test_wengine_texb");
            let _ = std::fs::remove_dir_all(&temp_dir);
            std::fs::create_dir_all(&temp_dir).unwrap();

            let asset = archive.extract_playable_asset(&temp_dir).expect("Extraction failed");
            assert!(asset.is_some(), "Expected extracted asset");
            let bmp_path = asset.unwrap();
            assert!(bmp_path.exists(), "Extracted BMP should exist");
            assert_eq!(bmp_path.file_name().unwrap(), "wallpaper.bmp");
            let meta = std::fs::metadata(&bmp_path).unwrap();
            assert!(meta.len() > 1000000, "BMP should be large high-res asset");
            let _ = std::fs::remove_dir_all(&temp_dir);
        }
    }
}
