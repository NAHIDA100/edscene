//! A self-extracting container: `installer.exe | files | index | footer`.
//!
//! Nothing has to be installed before something can be installed, and the
//! audio and artwork stay outside the source tree because they are only ever
//! appended to the binary by the packaging script.

use serde_json::{json, Value};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

/// Last eight bytes of the file, so a trailing archive is recognisable.
pub const MAGIC: &[u8; 8] = b"EDSPAY01";
const FOOTER: usize = 8 + 8 + 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub off: u64,
    pub len: u64,
}

/// Every path the installer is allowed to write: relative, no upward moves, no
/// drive letters, no reserved names. This is checked with administrator rights
/// in hand, so it has to be strict.
pub fn safe_relpath(name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains('\0') {
        return None;
    }
    // 载荷里的名字一律用 `/` 分隔（collect 就是这么写的）。因此出现反斜杠、开头的
    // 斜杠或盘符冒号，就说明这是从别的平台抄来的绝对路径 —— 例如 Windows 的 UNC
    // `\\server\share`，而 `Path` 在非 Windows 上只把它当成一个怪文件名，has_root
    // 是 false。这道闸跑在管理员权限下，不能依赖宿主平台怎么看这条字符串。
    if name.contains('\\') || name.starts_with('/') || name.contains(':') {
        return None;
    }
    let p = Path::new(name);
    if p.is_absolute() || p.has_root() {
        return None;
    }
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(seg) => {
                let s = seg.to_string_lossy();
                if is_reserved(&s) {
                    return None;
                }
                out.push(s.as_ref());
            }
            // `.` is noise, `..` is an escape.
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

fn is_reserved(name: &str) -> bool {
    const BAD: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
        "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    BAD.contains(&stem.as_str())
}

/// List a directory tree, sorted, with `/`-separated relative names.
pub fn collect(dir: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let mut out = Vec::new();
    walk(dir, dir, &mut out)?;
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
    for e in std::fs::read_dir(dir).map_err(|e| format!("{dir:?}: {e}"))? {
        let e = e.map_err(|e| e.to_string())?;
        let path = e.path();
        if path.is_dir() {
            walk(base, &path, out)?;
            continue;
        }
        let rel = path
            .strip_prefix(base)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        if safe_relpath(&rel).is_none() {
            return Err(format!("文件名不能放进载荷：{rel}"));
        }
        out.push((rel, path));
    }
    Ok(())
}

/// Write `host` plus `files` plus the index into `out`.
pub fn pack(host: &Path, files: &[(String, PathBuf)], out: &Path) -> Result<(), String> {
    let mut w = std::fs::File::create(out).map_err(|e| format!("{out:?}: {e}"))?;
    let mut buf = Vec::new();
    std::fs::File::open(host)
        .and_then(|mut f| f.read_to_end(&mut buf))
        .map_err(|e| format!("{host:?}: {e}"))?;
    w.write_all(&buf).map_err(|e| e.to_string())?;
    let mut off = buf.len() as u64;
    let mut index = Vec::new();
    for (name, src) in files {
        let mut data = Vec::new();
        std::fs::File::open(src)
            .and_then(|mut f| f.read_to_end(&mut data))
            .map_err(|e| format!("{src:?}: {e}"))?;
        w.write_all(&data).map_err(|e| e.to_string())?;
        index.push(json!({"name": name, "off": off, "len": data.len() as u64}));
        off += data.len() as u64;
    }
    let index = serde_json::to_vec(&index).map_err(|e| e.to_string())?;
    let index_off = off;
    w.write_all(&index).map_err(|e| e.to_string())?;
    w.write_all(&index_off.to_le_bytes()).map_err(|e| e.to_string())?;
    w.write_all(&(index.len() as u64).to_le_bytes())
        .map_err(|e| e.to_string())?;
    w.write_all(MAGIC).map_err(|e| e.to_string())?;
    w.flush().map_err(|e| e.to_string())
}

pub struct Container {
    f: File,
    entries: Vec<Entry>,
}

impl Container {
    /// A file with no footer is just a program, not an archive.
    pub fn open(exe: &Path) -> Result<Option<Container>, String> {
        let mut f = File::open(exe).map_err(|e| format!("{exe:?}: {e}"))?;
        let len = f.metadata().map_err(|e| e.to_string())?.len();
        if len < FOOTER as u64 {
            return Ok(None);
        }
        f.seek(SeekFrom::End(-(FOOTER as i64))).map_err(|e| e.to_string())?;
        let mut foot = [0u8; FOOTER];
        f.read_exact(&mut foot).map_err(|e| e.to_string())?;
        if &foot[16..24] != MAGIC {
            return Ok(None);
        }
        let index_off = u64::from_le_bytes(foot[0..8].try_into().unwrap());
        let index_len = u64::from_le_bytes(foot[8..16].try_into().unwrap()) as usize;
        if index_off + index_len as u64 + FOOTER as u64 > len {
            return Err("载荷索引越界".into());
        }
        f.seek(SeekFrom::Start(index_off)).map_err(|e| e.to_string())?;
        let mut raw = vec![0u8; index_len];
        f.read_exact(&mut raw).map_err(|e| e.to_string())?;
        let v: Value = serde_json::from_slice(&raw).map_err(|e| format!("载荷索引损坏: {e}"))?;
        let mut entries = Vec::new();
        for it in v.as_array().ok_or("载荷索引不是数组")? {
            let name = it["name"].as_str().ok_or("索引项缺少 name")?.to_string();
            if safe_relpath(&name).is_none() {
                return Err(format!("载荷里有非法路径：{name}"));
            }
            entries.push(Entry {
                off: it["off"].as_u64().ok_or("索引项缺少 off")?,
                len: it["len"].as_u64().ok_or("索引项缺少 len")?,
                name,
            });
        }
        Ok(Some(Container { f, entries }))
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn total(&self) -> u64 {
        self.entries.iter().map(|e| e.len).sum()
    }

    /// Streams one entry to `dest`; the 18 MB program never sits in RAM twice.
    pub fn extract(&mut self, e: &Entry, dest: &Path) -> Result<(), String> {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{parent:?}: {e}"))?;
        }
        self.f.seek(SeekFrom::Start(e.off)).map_err(|e| e.to_string())?;
        let mut src = (&mut self.f).take(e.len);
        let mut out = File::create(dest).map_err(|e| format!("{dest:?}: {e}"))?;
        std::io::copy(&mut src, &mut out).map_err(|e| e.to_string())?;
        out.flush().map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("eds-pay-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn rejects_every_way_out_of_the_install_directory() {
        for bad in [
            "..",
            "../evil.exe",
            "a/../../evil",
            "/etc/passwd",
            "C:\\Windows\\system32\\evil.exe",
            "\\\\server\\share\\x",
            "sub/../..",
            "CON",
            "nul.txt",
            "",
            "a\0b",
            "C:x",
        ] {
            assert!(safe_relpath(bad).is_none(), "{bad} 竟然通过了");
        }
        assert_eq!(safe_relpath("sounds/开机进入桌面.mp3").unwrap(), PathBuf::from("sounds").join("开机进入桌面.mp3"));
        assert_eq!(safe_relpath("./docs/install-windows.md").unwrap(), PathBuf::from("docs").join("install-windows.md"));
    }

    #[test]
    fn packs_and_reads_back_in_order() {
        let d = tmp("rt");
        let src = d.join("src");
        std::fs::create_dir_all(src.join("sounds")).unwrap();
        std::fs::write(src.join("edscene.txt"), b"program").unwrap();
        std::fs::write(src.join("sounds").join("a.mp3"), [0u8, 1, 2, 3]).unwrap();
        let host = d.join("host.bin");
        std::fs::write(&host, b"INSTALLER-HOST").unwrap();

        let out = d.join("setup.bin");
        pack(&host, &collect(&src).unwrap(), &out).unwrap();

        let mut c = Container::open(&out).unwrap().expect("没有页脚");
        assert_eq!(c.entries().len(), 2);
        assert_eq!(c.total(), 11);
        for e in c.entries().to_vec() {
            let dest = d.join("out").join(safe_relpath(&e.name).unwrap());
            c.extract(&e, &dest).unwrap();
        }
        assert_eq!(std::fs::read(d.join("out/sounds/a.mp3")).unwrap(), [0u8, 1, 2, 3]);
        assert_eq!(std::fs::read(d.join("out/edscene.txt")).unwrap(), b"program");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_plain_exe_is_not_an_archive() {
        let d = tmp("plain");
        let p = d.join("plain.exe");
        std::fs::write(&p, b"MEG").unwrap();
        assert!(Container::open(&p).unwrap().is_none());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_truncated_or_rewritten_footer_is_refused() {
        let d = tmp("broken");
        let p = d.join("x.bin");
        std::fs::write(&p, b"0123456789").unwrap();
        assert!(Container::open(&p).unwrap().is_none());
        std::fs::write(&p, [0u8; 24]).unwrap();
        assert!(Container::open(&p).unwrap().is_none());
        std::fs::remove_dir_all(&d).ok();
    }
}
