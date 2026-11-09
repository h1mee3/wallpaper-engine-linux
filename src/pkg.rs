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

    /// Extract the best playable visual asset (video, gif, or image) from the PKG to out_dir
    pub fn extract_playable_asset<P: AsRef<Path>>(&self, out_dir: P) -> Result<Option<PathBuf>> {
        let dest = out_dir.as_ref();
        std::fs::create_dir_all(dest)?;

        // 1. Look for embedded video files (.mp4, .webm)
        for entry in &self.entries {
            let lower = entry.name.to_lowercase();
            if lower.ends_with(".mp4") || lower.ends_with(".webm") {
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

        // 2. Look for embedded animated GIF
        for entry in &self.entries {
            let lower = entry.name.to_lowercase();
            if lower.ends_with(".gif") {
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

        // 3. Look for standard image files (.png, .jpg, .jpeg, .webp)
        for entry in &self.entries {
            let lower = entry.name.to_lowercase();
            if lower.ends_with(".png") || lower.ends_with(".jpg") || lower.ends_with(".jpeg") || lower.ends_with(".webp") {
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
